#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::{Receiver, Sender};
use futures::StreamExt;
use glib::clone;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GBox, Button, CssProvider,
    DrawingArea, Entry, EventControllerMotion, GestureDrag, Label, ListBox, ListBoxRow,
    Orientation, Overlay, PolicyType, ScrolledWindow, Stack, Separator, TextBuffer, TextView,
};
use tokio::sync::mpsc;
use tracing::error;

use cue_core::{
    audio::{AudioCapture, Utterance},
    brain::BrainStore,
    config::Config,
    context::ContextEngine,
    kb::KnowledgeBase,
    llm::{self, CompletionConfig},
    prompts::{list_all_prompts, resolve_prompt},
    session::{SessionStore, TranscriptEntry},
    stt::{DeepgramStreamer, ParakeetStreamer, SttEvent},
    tts::TtsEngine,
};

// ── Pipeline → UI messages ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum UiMsg {
    Token(String),
    Done,
    Status(String),
    Error(String),
    Transcript { channel: String, text: String },
    AutoQuery(String),
    SttLive { text: String, is_final: bool },
}

// ── Window manager IPC ────────────────────────────────────────────────────────

/// Send a command to the platform window manager.
/// Linux/Hyprland: UNIX socket IPC (fast, no process spawn).
/// macOS: no-op for Hyprland-specific commands (GTK4 handles window natively).
#[cfg(target_os = "linux")]
fn hypr_cmd(cmd: &str) {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let socket_path = match (
        std::env::var("XDG_RUNTIME_DIR"),
        std::env::var("HYPRLAND_INSTANCE_SIGNATURE"),
    ) {
        (Ok(xdg), Ok(sig)) => format!("{xdg}/hypr/{sig}/.socket.sock"),
        _ => {
            // Fallback: spawn hyprctl
            let args: Vec<&str> = cmd.split_whitespace().collect();
            if !args.is_empty() {
                let _ = std::process::Command::new("hyprctl").args(&args).output();
            }
            return;
        }
    };

    if let Ok(mut stream) = UnixStream::connect(&socket_path) {
        let _ = stream.write_all(cmd.as_bytes());
    }
}

#[cfg(not(target_os = "linux"))]
fn hypr_cmd(_cmd: &str) {
    // No-op on non-Linux platforms — window management handled by GTK4/OS.
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cue_native=info,cue_core=info".parse().unwrap())
        )
        .init();

    let _ = Config::ensure_dirs();
    let db = Config::db_path();

    let session_store = Arc::new(SessionStore::new(&db).expect("session DB"));
    let _ = session_store.init_schema();
    let kb = Arc::new(KnowledgeBase::new(&db).expect("KB DB"));
    let _ = kb.init_schema();
    let brain = Arc::new(BrainStore::new(&db).expect("brain DB"));
    let _ = brain.init_schema();

    let active_prompt = Arc::new(Mutex::new("general".to_string()));
    let session_id = session_store.create_session("general").expect("session").id;

    let (query_tx, query_rx) = mpsc::channel::<String>(8);
    let listening = Arc::new(AtomicBool::new(false));

    // async_channel: Send end goes to pipeline thread, Recv end drives GTK UI
    let (ui_tx, ui_rx) = async_channel::bounded::<UiMsg>(64);

    // Pipeline thread
    {
        let kb2 = Arc::clone(&kb);
        let brain2 = Arc::clone(&brain);
        let ss2 = Arc::clone(&session_store);
        let ap2 = Arc::clone(&active_prompt);
        let lis2 = Arc::clone(&listening);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run_pipeline(ui_tx, query_rx, lis2, kb2, brain2, ss2, ap2, session_id));
        });
    }

    // Platform-specific window rules
    #[cfg(target_os = "linux")]
    {
        // Hyprland window rules matched by app class (reliable across title changes).
        // no_screen_share: hides window from screen capture/recording.
        let wm_class = "org.freedesktop.sysutil";
        for rule in &[
            "float on",
            "pin on",
            "no_screen_share on",
            "no_shadow on",
            "no_anim on",
            "decorate false",
            "no_initial_focus on",
            "opacity 0.88",
            "size 400 520",
            "move 20 20",
        ] {
            hypr_cmd(&format!("keyword windowrule {rule}, match:class {wm_class}"));
        }
    }

    let app = Application::builder()
        .application_id("org.freedesktop.sysutil")
        .build();

    // Wrap for one-time extraction (connect_activate needs Fn)
    let ui_rx = Arc::new(Mutex::new(Some(ui_rx)));

    app.connect_activate(move |app| {
        // Load CSS
        let provider = CssProvider::new();
        provider.load_from_string(include_str!("style.css"));
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        let ui_rx = ui_rx.lock().unwrap().take().expect("activated once");
        build_window(app, ui_rx, query_tx.clone(), Arc::clone(&listening));
    });

    app.run();
}

// ── Window ────────────────────────────────────────────────────────────────────

fn build_window(
    app: &Application,
    ui_rx: Receiver<UiMsg>,
    query_tx: mpsc::Sender<String>,
    listening: Arc<AtomicBool>,
) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("System Utility")
        .default_width(400)
        .default_height(520)
        .decorated(false)
        .build();

    window.set_size_request(400, 520);

    // macOS: set opacity and keep-above via GTK4 (since Hyprland rules don't apply)
    #[cfg(target_os = "macos")]
    {
        window.set_opacity(0.88);
    }

    // Track whether window is "maximized" (anchored to all edges)
    let expanded = Arc::new(AtomicBool::new(false));

    // Background: Cairo-painted dark glass panel (bypasses GTK4 CSS rendering quirks)
    let bg = DrawingArea::new();
    bg.set_vexpand(true);
    bg.set_hexpand(true);
    bg.set_draw_func(|_, cr, w, h| {
        let r = 14.0_f64;
        let (wf, hf) = (w as f64, h as f64);

        // Rounded rect path
        cr.new_sub_path();
        cr.arc(wf - r, r, r, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(wf - r, hf - r, r, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(r, hf - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(r, r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
        cr.close_path();

        // Glass fill — slightly more translucent for blur-through
        cr.set_source_rgba(8.0 / 255.0, 8.0 / 255.0, 18.0 / 255.0, 0.72);
        cr.fill_preserve().unwrap();

        // Subtle inner glow at top edge (glassmorphism highlight)
        let grad = gtk4::cairo::LinearGradient::new(0.0, 0.0, 0.0, 60.0);
        grad.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.04);
        grad.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
        cr.set_source(&grad).unwrap();
        cr.fill_preserve().unwrap();

        // Border — soft white
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
        cr.set_line_width(0.8);
        cr.stroke().unwrap();
    });

    // Content sits on top of the painted background
    let glass = GBox::new(Orientation::Vertical, 0);
    glass.set_vexpand(true);
    glass.set_hexpand(true);

    let overlay = Overlay::new();
    overlay.set_child(Some(&bg));
    overlay.add_overlay(&glass);
    overlay.set_vexpand(true);

    // ── Drag-to-resize handles ─────────────────────────────────────────────

    // Right edge: drag right/left to widen/narrow the panel
    let right_hovered = Arc::new(AtomicBool::new(false));
    let right_handle = DrawingArea::new();
    right_handle.set_width_request(10);
    right_handle.set_vexpand(true);
    right_handle.set_halign(Align::End);
    right_handle.set_valign(Align::Fill);
    right_handle.set_cursor_from_name(Some("ew-resize"));
    right_handle.set_draw_func(clone!(
        #[strong] right_hovered,
        move |_, cr, _, h| {
            if right_hovered.load(Ordering::Relaxed) {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
                cr.rectangle(0.0, 0.0, 10.0, h as f64);
                let _ = cr.fill();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
                let cy = h as f64 / 2.0;
                for dy in [-8.0_f64, 0.0, 8.0] {
                    cr.arc(5.0, cy + dy, 1.8, 0.0, std::f64::consts::TAU);
                    let _ = cr.fill();
                }
            }
        }
    ));
    let hover_right = EventControllerMotion::new();
    hover_right.connect_enter(clone!(
        #[strong] right_hovered, #[weak] right_handle,
        move |_, _, _| { right_hovered.store(true, Ordering::Relaxed); right_handle.queue_draw(); }
    ));
    hover_right.connect_leave(clone!(
        #[strong] right_hovered, #[weak] right_handle,
        move |_| { right_hovered.store(false, Ordering::Relaxed); right_handle.queue_draw(); }
    ));
    right_handle.add_controller(hover_right);

    let last_rx = Arc::new(Mutex::new(0.0_f64));
    let drag_right = GestureDrag::new();
    drag_right.connect_drag_begin(clone!(
        #[strong] last_rx,
        move |_, _, _| { *last_rx.lock().unwrap() = 0.0; }
    ));
    drag_right.connect_drag_update(clone!(
        #[strong] last_rx,
        move |_, offset_x, _| {
            let prev = *last_rx.lock().unwrap();
            let dx = (offset_x - prev) as i32;
            if dx != 0 {
                hypr_cmd(&format!("dispatch resizeactive {dx} 0"));
                *last_rx.lock().unwrap() = offset_x;
            }
        }
    ));
    right_handle.add_controller(drag_right);
    overlay.add_overlay(&right_handle);

    // Bottom edge: drag down/up to grow/shrink the panel
    let bot_hovered = Arc::new(AtomicBool::new(false));
    let bottom_handle = DrawingArea::new();
    bottom_handle.set_height_request(10);
    bottom_handle.set_hexpand(true);
    bottom_handle.set_halign(Align::Fill);
    bottom_handle.set_valign(Align::End);
    bottom_handle.set_cursor_from_name(Some("s-resize"));
    bottom_handle.set_draw_func(clone!(
        #[strong] bot_hovered,
        move |_, cr, w, _| {
            if bot_hovered.load(Ordering::Relaxed) {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
                cr.rectangle(0.0, 0.0, w as f64, 10.0);
                let _ = cr.fill();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
                let cx = w as f64 / 2.0;
                for dx in [-8.0_f64, 0.0, 8.0] {
                    cr.arc(cx + dx, 5.0, 1.8, 0.0, std::f64::consts::TAU);
                    let _ = cr.fill();
                }
            }
        }
    ));
    let hover_bot = EventControllerMotion::new();
    hover_bot.connect_enter(clone!(
        #[strong] bot_hovered, #[weak] bottom_handle,
        move |_, _, _| { bot_hovered.store(true, Ordering::Relaxed); bottom_handle.queue_draw(); }
    ));
    hover_bot.connect_leave(clone!(
        #[strong] bot_hovered, #[weak] bottom_handle,
        move |_| { bot_hovered.store(false, Ordering::Relaxed); bottom_handle.queue_draw(); }
    ));
    bottom_handle.add_controller(hover_bot);

    let last_by = Arc::new(Mutex::new(0.0_f64));
    let drag_bottom = GestureDrag::new();
    drag_bottom.connect_drag_begin(clone!(
        #[strong] last_by,
        move |_, _, _| { *last_by.lock().unwrap() = 0.0; }
    ));
    drag_bottom.connect_drag_update(clone!(
        #[strong] last_by,
        move |_, _, offset_y| {
            let prev = *last_by.lock().unwrap();
            let dy = (offset_y - prev) as i32;
            if dy != 0 {
                hypr_cmd(&format!("dispatch resizeactive 0 {dy}"));
                *last_by.lock().unwrap() = offset_y;
            }
        }
    ));
    bottom_handle.add_controller(drag_bottom);
    overlay.add_overlay(&bottom_handle);

    window.set_child(Some(&overlay));

    // Main stack: chat ↔ settings
    let stack = Stack::new();
    stack.set_vexpand(true);
    glass.append(&stack);

    // ── Chat page ─────────────────────────────────────────────────────────

    let chat_page = GBox::new(Orientation::Vertical, 0);
    chat_page.set_vexpand(true);

    let (titlebar, dot, status_lbl, mic_btn, expand_btn, gear_btn) = make_titlebar();
    let grip = make_grip_handle();
    titlebar.prepend(&grip);
    chat_page.append(&titlebar);
    chat_page.append(&make_sep());

    let (transcript_scroll, transcript_box) = make_transcript_area();
    chat_page.append(&transcript_scroll);
    chat_page.append(&make_sep());

    let (conv_scroll, conv_list) = make_conv_area();
    chat_page.append(&conv_scroll);
    chat_page.append(&make_sep());

    let (input_bar, input_entry, send_btn) = make_input_bar();
    chat_page.append(&input_bar);

    stack.add_named(&chat_page, Some("chat"));

    stack.set_visible_child_name("chat");

    // ── Wire up buttons ───────────────────────────────────────────────────

    // Gear: open settings as a separate normal tiling window
    gear_btn.connect_clicked(clone!(
        #[weak] app,
        move |_| { open_settings_window(&app); }
    ));

    // Expand/collapse via Hyprland fullscreen toggle
    expand_btn.connect_clicked(clone!(
        #[weak] expand_btn,
        #[strong] expanded,
        move |_| {
            let now = !expanded.load(Ordering::Relaxed);
            expanded.store(now, Ordering::Relaxed);
            hypr_cmd("dispatch fullscreen 1");
            expand_btn.set_label(if now { "\u{2923}" } else { "\u{2922}" }); // ⤣ ⤢
        }
    ));

    // Titlebar drag → move overlay via Hyprland IPC moveactive
    let last_dx = Arc::new(Mutex::new(0.0_f64));
    let last_dy = Arc::new(Mutex::new(0.0_f64));

    let move_drag = GestureDrag::new();
    move_drag.set_propagation_phase(gtk4::PropagationPhase::Capture);
    move_drag.connect_drag_begin(clone!(
        #[strong] last_dx, #[strong] last_dy,
        #[weak]   titlebar,
        move |_, _, _| {
            *last_dx.lock().unwrap() = 0.0;
            *last_dy.lock().unwrap() = 0.0;
            titlebar.set_cursor_from_name(Some("grabbing"));
        }
    ));
    move_drag.connect_drag_update(clone!(
        #[strong] last_dx, #[strong] last_dy,
        move |_, offset_x, offset_y| {
            let prev_x = *last_dx.lock().unwrap();
            let prev_y = *last_dy.lock().unwrap();
            let dx = (offset_x - prev_x) as i32;
            let dy = (offset_y - prev_y) as i32;
            if dx != 0 || dy != 0 {
                hypr_cmd(&format!("dispatch moveactive {dx} {dy}"));
                *last_dx.lock().unwrap() = offset_x;
                *last_dy.lock().unwrap() = offset_y;
            }
        }
    ));
    move_drag.connect_drag_end(clone!(
        #[weak] titlebar,
        move |_, _, _| { titlebar.set_cursor_from_name(Some("default")); }
    ));
    titlebar.add_controller(move_drag);

    // STT live transcription state (shared with toggle_listen + UI event loop)
    let stt_committed: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stt_interim: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    // Mic toggle
    mic_btn.connect_clicked(clone!(
        #[weak] dot,
        #[weak] mic_btn,
        #[weak] status_lbl,
        #[strong] listening,
        #[strong] stt_committed,
        #[strong] stt_interim,
        move |_| { toggle_listen(&listening, &dot, &mic_btn, &status_lbl, &stt_committed, &stt_interim); }
    ));

    // Ctrl+L shortcut
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.connect_key_pressed(clone!(
        #[weak] dot,
        #[weak] mic_btn,
        #[weak] status_lbl,
        #[weak] window,
        #[strong] listening,
        #[strong] stt_committed,
        #[strong] stt_interim,
        #[upgrade_or] glib::Propagation::Proceed,
        move |_, key, _, mods| {
            let ctrl = mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
            let shift = mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
            if ctrl && key == gtk4::gdk::Key::l {
                toggle_listen(&listening, &dot, &mic_btn, &status_lbl, &stt_committed, &stt_interim);
                glib::Propagation::Stop
            } else if ctrl && shift && key == gtk4::gdk::Key::H {
                // Ctrl+Shift+H: toggle overlay visibility (hide during screen share)
                window.set_visible(!window.is_visible());
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    ));
    window.add_controller(key_ctrl);

    // Current streaming response state: (buf, streaming_tv, ai_container, accumulated_text)
    type SlotInner = (TextBuffer, TextView, GBox, String);
    let current_slot: Arc<Mutex<Option<SlotInner>>> = Arc::new(Mutex::new(None));

    // Send query action
    let do_send = {
        let query_tx = query_tx.clone();
        let conv_list = conv_list.clone();
        let conv_scroll = conv_scroll.clone();
        let current_slot = Arc::clone(&current_slot);
        let input_entry = input_entry.clone();
        let stt_committed = Arc::clone(&stt_committed);
        let stt_interim = Arc::clone(&stt_interim);
        move || {
            let text = input_entry.text().trim().to_string();
            if text.is_empty() { return; }
            input_entry.set_text("");
            *stt_committed.lock().unwrap() = String::new();
            *stt_interim.lock().unwrap() = String::new();
            let (buf, tv, container) = append_conv_row(&conv_list, &text);
            *current_slot.lock().unwrap() = Some((buf, tv, container, String::new()));
            scroll_to_bottom(&conv_scroll);
            let tx = query_tx.clone();
            glib::spawn_future_local(async move { let _ = tx.send(text).await; });
        }
    };

    send_btn.connect_clicked(clone!(
        #[strong] do_send,
        move |_| do_send()
    ));
    input_entry.connect_activate(clone!(
        #[strong] do_send,
        move |_| do_send()
    ));

    // ── Receive pipeline events via async channel ─────────────────────────

    let auto_query_tx = query_tx.clone();
    glib::spawn_future_local(clone!(
        #[weak] status_lbl,
        #[weak] transcript_box,
        #[weak] transcript_scroll,
        #[weak] conv_scroll,
        #[weak] conv_list,
        #[strong] input_entry,
        #[strong] current_slot,
        #[strong] stt_committed,
        #[strong] stt_interim,
        async move {
            while let Ok(msg) = ui_rx.recv().await {
                match msg {
                    UiMsg::Token(tok) => {
                        let mut guard = current_slot.lock().unwrap();
                        if let Some((buf, _, _, text)) = guard.as_mut() {
                            let mut end = buf.end_iter();
                            buf.insert(&mut end, &tok);
                            text.push_str(&tok);
                        }
                        drop(guard);
                        scroll_to_bottom(&conv_scroll);
                    }
                    UiMsg::Done => {
                        let slot = current_slot.lock().unwrap().take();
                        if let Some((buf, _, _, text)) = slot {
                            render_markdown_in_buffer(&buf, &text);
                        }
                        status_lbl.set_text("Done. Ask anything.");
                        status_lbl.remove_css_class("error");
                        scroll_to_bottom(&conv_scroll);
                    }
                    UiMsg::Status(s) => {
                        status_lbl.set_text(&s);
                        status_lbl.remove_css_class("error");
                    }
                    UiMsg::Error(e) => {
                        status_lbl.set_text(&format!("⚠ {e}"));
                        status_lbl.add_css_class("error");
                    }
                    UiMsg::Transcript { channel, text } => {
                        append_transcript_line(&transcript_box, &channel, &text);
                        trim_transcript(&transcript_box, 5);
                        scroll_to_bottom(&transcript_scroll);
                    }
                    UiMsg::AutoQuery(text) => {
                        // Speech transcript → auto-send as AI query
                        let (buf, tv, container) = append_conv_row(&conv_list, &text);
                        *current_slot.lock().unwrap() = Some((buf, tv, container, String::new()));
                        scroll_to_bottom(&conv_scroll);
                        let tx = auto_query_tx.clone();
                        glib::spawn_future_local(async move { let _ = tx.send(text).await; });
                    }
                    UiMsg::SttLive { text, is_final } => {
                        if is_final {
                            let mut committed = stt_committed.lock().unwrap();
                            if !text.is_empty() {
                                if !committed.is_empty() {
                                    committed.push(' ');
                                }
                                committed.push_str(&text);
                            }
                            *stt_interim.lock().unwrap() = String::new();
                        } else {
                            *stt_interim.lock().unwrap() = text;
                        }
                        let committed = stt_committed.lock().unwrap().clone();
                        let interim = stt_interim.lock().unwrap().clone();
                        let combined = if !committed.is_empty() && !interim.is_empty() {
                            format!("{} {}", committed, interim)
                        } else if !committed.is_empty() {
                            committed
                        } else {
                            interim
                        };
                        input_entry.set_text(&combined);
                        input_entry.set_position(-1); // cursor at end
                    }
                }
            }
        }
    ));

    window.present();

    #[cfg(target_os = "macos")]
    apply_macos_stealth();
}

/// macOS stealth: hide from screen capture, float above other windows,
/// exclude from Exposé/Mission Control, and camouflage process name.
#[cfg(target_os = "macos")]
fn apply_macos_stealth() {
    use cocoa::appkit::NSApp;
    use cocoa::base::{id, nil};
    use objc::runtime::YES;

    unsafe {
        let app: id = NSApp();
        if app.is_null() {
            return;
        }
        let windows: id = msg_send![app, windows];
        if windows.is_null() || windows == nil {
            return;
        }
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let win: id = msg_send![windows, objectAtIndex: i];
            if win.is_null() || win == nil {
                continue;
            }

            // NSWindowSharingNone = 0 — hides window from screen recording/sharing
            let _: () = msg_send![win, setSharingType: 0i64];

            // NSFloatingWindowLevel = 3 — keeps window above normal windows
            let _: () = msg_send![win, setLevel: 3i64];

            // Collection behavior: exclude from Exposé + Mission Control + Spaces
            // NSWindowCollectionBehaviorStationary (1 << 4) = 16
            // NSWindowCollectionBehaviorCanJoinAllSpaces (1 << 0) = 1
            // NSWindowCollectionBehaviorIgnoresCycle (1 << 6) = 64
            let behavior: u64 = (1 << 0) | (1 << 4) | (1 << 6);
            let _: () = msg_send![win, setCollectionBehavior: behavior];

            // Exclude from window list in Dock and Cmd+Tab
            let _: () = msg_send![win, setExcludedFromWindowsMenu: YES];
        }

        // Hide from Dock + Cmd+Tab by setting activation policy to Accessory
        // NSApplicationActivationPolicyAccessory = 1
        let _: () = msg_send![app, setActivationPolicy: 1i64];
    }

    // Camouflage process name in Activity Monitor / ps
    macos_camouflage_process("System Preferences");
}

/// Overwrite argv[0] to camouflage the process name in `ps` / Activity Monitor.
#[cfg(target_os = "macos")]
fn macos_camouflage_process(name: &str) {
    use std::ffi::CString;
    if let Ok(cname) = CString::new(name) {
        unsafe {
            let args = std::env::args_os().collect::<Vec<_>>();
            if !args.is_empty() {
                // pthread_setname_np sets the thread name (visible in Activity Monitor)
                libc::pthread_setname_np(cname.as_ptr());
            }
        }
    }
}



// ── UI helpers ────────────────────────────────────────────────────────────────

fn make_grip_handle() -> DrawingArea {
    let grip = DrawingArea::new();
    grip.set_width_request(14);
    grip.set_height_request(14);
    grip.set_valign(Align::Center);
    grip.set_cursor_from_name(Some("grab"));
    grip.set_draw_func(|_, cr, _, _| {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
        // 2x2 dot grid
        for col in 0..2_i32 {
            for row in 0..2_i32 {
                let x = 3.0 + col as f64 * 5.0;
                let y = 3.0 + row as f64 * 5.0;
                cr.arc(x, y, 1.2, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        }
    });
    grip
}

fn make_sep() -> Separator {
    let s = Separator::new(Orientation::Horizontal);
    s.add_css_class("divider");
    s
}

fn make_titlebar() -> (GBox, Label, Label, Button, Button, Button) {
    let bar = GBox::new(Orientation::Horizontal, 6);
    bar.add_css_class("titlebar");
    bar.set_hexpand(true);

    let dot = Label::new(None);
    dot.add_css_class("dot");
    dot.set_size_request(6, 6);
    bar.append(&dot);

    let name = Label::new(Some("cue"));
    name.add_css_class("app-name");
    bar.append(&name);

    let status = Label::new(Some("Starting…"));
    status.add_css_class("status-label");
    status.set_hexpand(true);
    status.set_halign(Align::End);
    status.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    bar.append(&status);

    // Unicode icons — no icon theme dependency
    let mic = Button::with_label("\u{25CF}"); // filled circle
    mic.add_css_class("icon-btn");
    bar.append(&mic);

    let expand = Button::with_label("\u{2922}"); // NE arrow ⤢
    expand.add_css_class("icon-btn");
    bar.append(&expand);

    let gear = Button::with_label("\u{2699}"); // gear ⚙
    gear.add_css_class("icon-btn");
    bar.append(&gear);

    (bar, dot, status, mic, expand, gear)
}

fn toggle_listen(
    listening: &Arc<AtomicBool>,
    dot: &Label,
    mic_btn: &Button,
    status: &Label,
    stt_committed: &Arc<Mutex<String>>,
    stt_interim: &Arc<Mutex<String>>,
) {
    let new = !listening.load(Ordering::Relaxed);
    listening.store(new, Ordering::Relaxed);
    if new {
        // Starting to listen — reset STT accumulators
        *stt_committed.lock().unwrap() = String::new();
        *stt_interim.lock().unwrap() = String::new();
        dot.add_css_class("active");
        mic_btn.add_css_class("mic-btn");
        status.add_css_class("listening");
        status.set_text("listening…");
    } else {
        // Stopped — commit any remaining interim text
        let interim = stt_interim.lock().unwrap().clone();
        if !interim.is_empty() {
            let mut committed = stt_committed.lock().unwrap();
            if !committed.is_empty() {
                committed.push(' ');
            }
            committed.push_str(&interim);
            *stt_interim.lock().unwrap() = String::new();
        }
        dot.remove_css_class("active");
        mic_btn.remove_css_class("mic-btn");
        status.remove_css_class("listening");
        status.set_text("Ready");
    }
}

fn make_transcript_area() -> (ScrolledWindow, GBox) {
    let scroll = ScrolledWindow::new();
    scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
    scroll.set_max_content_height(72);
    scroll.set_propagate_natural_height(true);

    let vbox = GBox::new(Orientation::Vertical, 0);
    vbox.add_css_class("transcript-area");

    let ph = Label::new(Some("No audio yet — Ctrl+L to listen"));
    ph.add_css_class("status-label");
    ph.set_halign(Align::Start);
    ph.set_widget_name("transcript-placeholder");
    vbox.append(&ph);

    scroll.set_child(Some(&vbox));
    (scroll, vbox)
}

fn append_transcript_line(container: &GBox, channel: &str, text: &str) {
    // Remove placeholder on first real line
    if let Some(ph) = container.first_child() {
        if ph.widget_name() == "transcript-placeholder" {
            container.remove(&ph);
        }
    }
    let row = GBox::new(Orientation::Horizontal, 4);
    row.add_css_class("transcript-line");

    let ch = Label::new(Some(
        if channel == "system" || channel == "SYS" { "[SYS]" } else { "[MIC]" }
    ));
    ch.add_css_class(if channel == "system" || channel == "SYS" { "ch-sys" } else { "ch-mic" });

    let lbl = Label::new(Some(text));
    lbl.set_wrap(true);
    lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    lbl.set_halign(Align::Start);
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);

    row.append(&ch);
    row.append(&lbl);
    container.append(&row);
}

fn trim_transcript(container: &GBox, max: usize) {
    let mut children = vec![];
    let mut c = container.first_child();
    while let Some(w) = c {
        let next = w.next_sibling();
        children.push(w);
        c = next;
    }
    if children.len() > max {
        for old in &children[..children.len() - max] {
            container.remove(old);
        }
    }
}

fn make_conv_area() -> (ScrolledWindow, ListBox) {
    let scroll = ScrolledWindow::new();
    scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
    scroll.set_vexpand(true);

    let list = ListBox::new();
    list.add_css_class("conv-area");
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.set_show_separators(false);

    let ph = Label::new(Some("Ask a question or press Ctrl+L to listen"));
    ph.add_css_class("conv-empty");
    ph.set_halign(Align::Center);
    ph.set_valign(Align::Center);
    ph.set_vexpand(true);
    ph.set_justify(gtk4::Justification::Center);
    ph.set_widget_name("conv-placeholder");
    list.append(&ph);

    scroll.set_child(Some(&list));
    (scroll, list)
}

/// Returns (TextBuffer for streaming, streaming TextView, AI bubble container)
fn append_conv_row(list: &ListBox, query: &str) -> (TextBuffer, TextView, GBox) {
    // Remove placeholder
    if let Some(ph) = list.first_child() {
        if ph.widget_name() == "conv-placeholder" {
            list.remove(&ph);
        }
    }

    let row_box = GBox::new(Orientation::Vertical, 4);
    row_box.set_margin_top(2);
    row_box.set_margin_bottom(2);
    row_box.set_margin_start(2);
    row_box.set_margin_end(2);

    // User bubble — right aligned
    let user_wrap = GBox::new(Orientation::Horizontal, 0);
    user_wrap.set_halign(Align::End);
    let user_lbl = Label::new(Some(query));
    user_lbl.add_css_class("user-bubble");
    user_lbl.set_wrap(true);
    user_lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    user_lbl.set_max_width_chars(38);
    user_lbl.set_xalign(0.0);
    user_wrap.append(&user_lbl);
    row_box.append(&user_wrap);

    // AI response — streaming TextView while generating
    let tv = TextView::new();
    tv.add_css_class("ai-bubble");
    tv.set_wrap_mode(gtk4::WrapMode::WordChar);
    tv.set_editable(false);
    tv.set_cursor_visible(false);
    tv.set_hexpand(true);
    let buf = tv.buffer();
    row_box.append(&tv);

    let row = ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_child(Some(&row_box));
    list.append(&row);

    (buf, tv, row_box)
}


fn make_input_bar() -> (GBox, Entry, Button) {
    let bar = GBox::new(Orientation::Horizontal, 6);
    bar.add_css_class("input-bar");
    bar.set_hexpand(true);

    let entry = Entry::new();
    entry.add_css_class("glass-entry");
    entry.set_placeholder_text(Some("Ask anything…"));
    entry.set_hexpand(true);

    let send = Button::with_label("\u{2191}"); // up arrow ↑
    send.add_css_class("icon-btn");
    send.add_css_class("active-btn");

    bar.append(&entry);
    bar.append(&send);

    (bar, entry, send)
}

fn scroll_to_bottom(scroll: &ScrolledWindow) {
    let adj = scroll.vadjustment();
    glib::idle_add_local_once(move || {
        adj.set_value(adj.upper() - adj.page_size());
    });
}

// ── Settings window (separate tiling window) ──────────────────────────────────

fn open_settings_window(app: &Application) {
    let win = ApplicationWindow::builder()
        .application(app)
        .title("Preferences")
        .default_width(640)
        .default_height(600)
        .decorated(true)   // normal tiling window with decorations
        .build();

    // NO layer shell init → this is a regular tiling window

    let page = GBox::new(Orientation::Vertical, 0);
    page.set_vexpand(true);
    build_settings_content(&page);
    win.set_child(Some(&page));
    win.present();
}

fn build_settings_content(page: &GBox) {
    // Scrollable content (no custom titlebar — window has native decorations)
    let scroll = ScrolledWindow::new();
    scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
    scroll.set_vexpand(true);

    let content = GBox::new(Orientation::Vertical, 12);
    content.add_css_class("settings-area");

    // ── Prompt mode ───────────────────────────────────────────────────────
    append_section_title(&content, "Prompt Mode");

    for (id, desc) in list_all_prompts() {
        let row = GBox::new(Orientation::Vertical, 2);
        row.add_css_class("prompt-row");

        let t = Label::new(Some(&prompt_label_for(&id)));
        t.add_css_class("prompt-title");
        t.set_halign(Align::Start);

        let d = Label::new(Some(&desc));
        d.add_css_class("prompt-desc");
        d.set_halign(Align::Start);
        d.set_wrap(true);
        d.set_xalign(0.0);

        row.append(&t);
        row.append(&d);
        content.append(&row);
    }

    // ── Brain ───────────────────────────────────────────────────────────────────
    append_section_title(&content, "Brain");

    let db = Config::db_path();
    if let Ok(brain) = BrainStore::new(&db) {
        let _ = brain.init_schema();

        let brain_box = GBox::new(Orientation::Vertical, 6);
        brain_box.add_css_class("brain-section");

        match brain.list_folders() {
            Ok(folders) if folders.is_empty() => {
                let empty = Label::new(Some("No brain folders yet. Use CLI:"));
                empty.add_css_class("brain-empty");
                empty.set_halign(Align::Start);
                empty.set_wrap(true);
                brain_box.append(&empty);

                let cmd = Label::new(Some("cue brain create <name> --link <prompt>"));
                cmd.add_css_class("code-block");
                cmd.set_halign(Align::Start);
                cmd.set_xalign(0.0);
                cmd.set_selectable(true);
                brain_box.append(&cmd);
            }
            Ok(folders) => {
                for f in &folders {
                    let row = GBox::new(Orientation::Vertical, 2);
                    row.add_css_class("brain-folder-row");

                    let name_lbl = Label::new(Some(&f.name));
                    name_lbl.add_css_class("brain-folder-name");
                    name_lbl.set_halign(Align::Start);
                    row.append(&name_lbl);

                    if let Ok(docs) = brain.list_documents(&f.name) {
                        if !docs.is_empty() {
                            let doc_lbl = Label::new(Some(&format!("{} doc(s)", docs.len())));
                            doc_lbl.add_css_class("brain-doc-name");
                            doc_lbl.set_halign(Align::Start);
                            row.append(&doc_lbl);
                        }
                    }

                    if let Some(ref lp) = f.linked_prompt {
                        let link_lbl = Label::new(Some(&format!("Linked: {}", lp)));
                        link_lbl.add_css_class("brain-folder-link");
                        link_lbl.set_halign(Align::Start);
                        row.append(&link_lbl);
                    }

                    brain_box.append(&row);
                }

                let count = Label::new(Some(&format!("{} folder(s) total", folders.len())));
                count.add_css_class("brain-folder-link");
                count.set_halign(Align::Start);
                brain_box.append(&count);
            }
            Err(_) => {
                let err = Label::new(Some("Could not load brain data"));
                err.add_css_class("brain-empty");
                err.set_halign(Align::Start);
                brain_box.append(&err);
            }
        }

        match brain.list_notes() {
            Ok(notes) if !notes.is_empty() => {
                let notes_title = Label::new(Some("Notes:"));
                notes_title.add_css_class("brain-folder-name");
                notes_title.set_halign(Align::Start);
                notes_title.set_margin_top(4);
                brain_box.append(&notes_title);

                for n in &notes {
                    let note_text = format!("[{}] {}", n.category, n.content);
                    let note_lbl = Label::new(Some(&note_text));
                    note_lbl.add_css_class("brain-note-text");
                    note_lbl.set_halign(Align::Start);
                    note_lbl.set_wrap(true);
                    note_lbl.set_xalign(0.0);
                    brain_box.append(&note_lbl);
                }
            }
            _ => {}
        }

        content.append(&brain_box);
    }

    // ── API Keys ──────────────────────────────────────────────────────────
    append_section_title(&content, "API Keys");

    let hint = Label::new(Some("Saved to ~/.config/cue/.env \u{2014} restart to apply"));
    hint.add_css_class("settings-label");
    hint.set_halign(Align::Start);
    hint.set_wrap(true);
    content.append(&hint);

    for (label_text, env_var) in &[
        ("Anthropic", "ANTHROPIC_API_KEY"),
        ("OpenAI", "OPENAI_API_KEY"),
        ("OpenRouter", "OPENROUTER_API_KEY"),
        ("Deepgram (STT)", "DEEPGRAM_API_KEY"),
        ("Groq", "GROQ_API_KEY"),
    ] {
        let lbl = Label::new(Some(label_text));
        lbl.add_css_class("settings-label");
        lbl.set_halign(Align::Start);
        content.append(&lbl);

        let entry = Entry::new();
        entry.add_css_class("settings-entry");
        entry.set_input_purpose(gtk4::InputPurpose::Password);
        entry.set_visibility(false);
        entry.set_hexpand(true);
        if std::env::var(env_var).is_ok() {
            entry.set_placeholder_text(Some("••••••••••••• (set)"));
        }
        let var = env_var.to_string();
        entry.connect_activate(move |e| {
            let val = e.text().to_string();
            if !val.is_empty() {
                std::env::set_var(&var, &val);
            }
        });
        content.append(&entry);
    }

    // ── Hyprland rules ─────────────────────────────────────────────────────
    append_section_title(&content, "Hyprland Rules (persistent)");

    let code = Label::new(Some(
        "windowrule = float on, match:class org.freedesktop.sysutil\n\
         windowrule = pin on, match:class org.freedesktop.sysutil\n\
         windowrule = no_screen_share on, match:class org.freedesktop.sysutil\n\
         windowrule = opacity 0.88, match:class org.freedesktop.sysutil\n\
         windowrule = decorate false, match:class org.freedesktop.sysutil",
    ));
    code.add_css_class("code-block");
    code.set_halign(Align::Fill);
    code.set_xalign(0.0);
    code.set_selectable(true);
    content.append(&code);

    let note = Label::new(Some(
        "Add to ~/.config/hypr/hyprland.conf for persistent rules.\n\
         The app auto-dispatches these via hyprctl on every launch.\n\
         noscreencast hides the window from screen share/recording.",
    ));
    note.add_css_class("settings-label");
    note.set_halign(Align::Start);
    note.set_wrap(true);
    note.set_xalign(0.0);
    content.append(&note);

    scroll.set_child(Some(&content));
    page.append(&scroll);
}

fn append_section_title(parent: &GBox, text: &str) {
    let lbl = Label::new(Some(text));
    lbl.add_css_class("settings-section-title");
    lbl.set_halign(Align::Start);
    lbl.set_margin_top(8);
    parent.append(&lbl);
}

fn prompt_label_for(id: &str) -> String {
    match id {
        "coding" => "Coding Interview",
        "behavioral" => "Behavioral Interview",
        "system_design" => "System Design",
        "meeting" => "Meeting Assistant",
        "sales" => "Sales",
        _ => "General",
    }
    .to_string()
}

// ── Markdown → TextBuffer tags (in-place, no widget swap) ────────────────────

fn render_markdown_in_buffer(buf: &TextBuffer, markdown: &str) {
    ensure_markdown_tags(buf);
    buf.set_text("");

    let mut in_code_block = false;
    let lines: Vec<&str> = markdown.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        if i > 0 { buf.insert(&mut buf.end_iter(), "\n"); }

        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            let off = buf.end_iter().offset();
            buf.insert(&mut buf.end_iter(), line);
            buf.apply_tag_by_name("md-code", &buf.iter_at_offset(off), &buf.end_iter());
            continue;
        }

        // Headings: strip prefix, render content, apply heading tag
        let (content, is_heading) = if let Some(r) = line.strip_prefix("### ")
            .or_else(|| line.strip_prefix("## "))
            .or_else(|| line.strip_prefix("# "))
        {
            (r, true)
        } else {
            (*line, false)
        };

        // List bullets
        let content = if let Some(r) = content.strip_prefix("- ").or_else(|| content.strip_prefix("* ")) {
            format!("• {r}")
        } else {
            content.to_string()
        };

        let line_start = buf.end_iter().offset();
        insert_inline_markdown(buf, &content);

        if is_heading {
            buf.apply_tag_by_name(
                "md-heading",
                &buf.iter_at_offset(line_start),
                &buf.end_iter(),
            );
        }
    }
}

fn insert_inline_markdown(buf: &TextBuffer, text: &str) {
    let mut chars = text.chars().peekable();
    let mut plain = String::new();

    while let Some(c) = chars.next() {
        if c == '`' {
            flush_plain(buf, &mut plain);
            let span = collect_until(&mut chars, '`');
            let off = buf.end_iter().offset();
            buf.insert(&mut buf.end_iter(), &span);
            buf.apply_tag_by_name("md-code", &buf.iter_at_offset(off), &buf.end_iter());
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next(); // consume second *
            flush_plain(buf, &mut plain);
            let span = collect_until_double(&mut chars, '*');
            let off = buf.end_iter().offset();
            buf.insert(&mut buf.end_iter(), &span);
            buf.apply_tag_by_name("md-bold", &buf.iter_at_offset(off), &buf.end_iter());
        } else if c == '*' {
            flush_plain(buf, &mut plain);
            let span = collect_until(&mut chars, '*');
            let off = buf.end_iter().offset();
            buf.insert(&mut buf.end_iter(), &span);
            buf.apply_tag_by_name("md-italic", &buf.iter_at_offset(off), &buf.end_iter());
        } else {
            plain.push(c);
        }
    }
    flush_plain(buf, &mut plain);
}

fn flush_plain(buf: &TextBuffer, s: &mut String) {
    if !s.is_empty() {
        buf.insert(&mut buf.end_iter(), s);
        s.clear();
    }
}

fn collect_until(chars: &mut std::iter::Peekable<std::str::Chars>, delim: char) -> String {
    let mut out = String::new();
    for c in chars.by_ref() {
        if c == delim { break; }
        out.push(c);
    }
    out
}

fn collect_until_double(chars: &mut std::iter::Peekable<std::str::Chars>, delim: char) -> String {
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == delim && chars.peek() == Some(&delim) {
            chars.next();
            break;
        }
        out.push(c);
    }
    out
}

fn ensure_markdown_tags(buf: &TextBuffer) {
    let table = buf.tag_table();
    if table.lookup("md-bold").is_some() { return; } // already added

    let bold = gtk4::TextTag::builder().name("md-bold").weight(700).build();
    table.add(&bold);

    let italic = gtk4::TextTag::builder()
        .name("md-italic")
        .style(gtk4::pango::Style::Italic)
        .build();
    table.add(&italic);

    let code = gtk4::TextTag::builder()
        .name("md-code")
        .family("monospace")
        .foreground("#4ade80")
        .build();
    table.add(&code);

    let heading = gtk4::TextTag::builder()
        .name("md-heading")
        .weight(700)
        .scale(1.1)
        .build();
    table.add(&heading);
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

async fn run_pipeline(
    ui_tx: Sender<UiMsg>,
    mut query_rx: mpsc::Receiver<String>,
    listening: Arc<AtomicBool>,
    kb: Arc<KnowledgeBase>,
    brain: Arc<BrainStore>,
    session_store: Arc<SessionStore>,
    active_prompt: Arc<Mutex<String>>,
    session_id: i64,
) {
    macro_rules! send {
        ($msg:expr) => {{ let _ = ui_tx.send($msg).await; }};
    }

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => { send!(UiMsg::Error(format!("Config: {e}"))); return; }
    };

    let router = match llm::build_router(&config.provider) {
        Ok(r) => Arc::new(r),
        Err(e) => { send!(UiMsg::Error(format!("LLM: {e}"))); return; }
    };

    let cc = CompletionConfig {
        temperature: config.provider.temperature,
        max_tokens: config.provider.max_tokens,
        model: config.provider.model.clone(),
    };

    let ctx = Arc::new(
        ContextEngine::new(Arc::clone(&kb), config.rag.clone()).with_brain(brain),
    );

    // TTS engine (lazy-started on first use)
    let tts: Option<Arc<TtsEngine>> = if config.tts.enabled {
        Some(Arc::new(TtsEngine::new(config.tts.clone())))
    } else {
        None
    };

    let (utt_tx, utt_rx) = mpsc::channel::<Utterance>(16);
    let _stream = AudioCapture::new(config.audio.clone(), utt_tx)
        .start()
        .map_err(|e| { let _ = ui_tx.try_send(UiMsg::Error(format!("Mic: {e}"))); })
        .ok();

    let (tx_entry, mut rx_entry) = mpsc::channel::<TranscriptEntry>(32);

    // STT event channel for real-time input box updates
    let (stt_tx, mut stt_rx) = mpsc::channel::<SttEvent>(64);
    let stt_ui_tx = ui_tx.clone();
    tokio::spawn(async move {
        while let Some(evt) = stt_rx.recv().await {
            tracing::info!(text = %evt.text, is_final = evt.is_final, "STT event received");
            let _ = stt_ui_tx.send(UiMsg::SttLive {
                text: evt.text,
                is_final: evt.is_final,
            }).await;
        }
    });

    // Gate utterances through listening flag
    let (gated_tx, gated_rx) = mpsc::channel::<Utterance>(32);
    let lis = Arc::clone(&listening);
    tokio::spawn(async move {
        let mut rx = utt_rx;
        while let Some(u) = rx.recv().await {
            if lis.load(Ordering::Relaxed) {
                let _ = gated_tx.send(u).await;
            }
        }
    });

    // Try Parakeet (local) first, fall back to Deepgram (cloud)
    match ParakeetStreamer::new(None) {
        Ok(streamer) => {
            let tx2 = tx_entry.clone();
            tokio::spawn(async move {
                if let Err(e) = streamer.run(gated_rx, tx2, stt_tx, session_id).await {
                    error!("Parakeet: {e}");
                }
            });
            send!(UiMsg::Status("Ready (Parakeet local) — Ctrl+L to listen".into()));
        }
        Err(e) => {
            tracing::info!("Parakeet not available: {e}, trying Deepgram");
            let deepgram_key = config.stt.deepgram_api_key.clone().unwrap_or_default();
            if !deepgram_key.is_empty() {
                let streamer = DeepgramStreamer::new(deepgram_key);
                let tx2 = tx_entry.clone();
                tokio::spawn(async move {
                    let mut rx = gated_rx;
                    if let Err(e) = streamer.run(&mut rx, tx2, stt_tx, session_id).await {
                        error!("Deepgram: {e}");
                    }
                });
                send!(UiMsg::Status("Ready (Deepgram) — Ctrl+L to listen".into()));
            } else {
                send!(UiMsg::Error("No STT available — install Parakeet model or add Deepgram key".into()));
                send!(UiMsg::Status("Type to ask AI (no audio)".into()));
            }
        }
    }

    let mut recent: Vec<TranscriptEntry> = Vec::new();
    // (user_query, ai_response) pairs for multi-turn context
    let mut chat_history: Vec<(String, String)> = Vec::new();

    loop {
        tokio::select! {
            Some(entry) = rx_entry.recv() => {
                send!(UiMsg::Transcript {
                    channel: entry.channel.clone(),
                    text: entry.text.clone(),
                });
                // Auto-send mic transcripts as queries to the AI
                if entry.channel == "mic" && !entry.text.trim().is_empty() {
                    let _ = ui_tx.send(UiMsg::AutoQuery(entry.text.clone())).await;
                }
                recent.push(entry);
                let cutoff = chrono::Utc::now() - chrono::Duration::seconds(120);
                recent.retain(|e| e.spoken_at >= cutoff);
            }

            Some(q) = query_rx.recv() => {
                let query = if q.is_empty() {
                    recent.last().map(|e| e.text.clone())
                        .unwrap_or_else(|| "What was just discussed?".into())
                } else { q };

                let prompt_name = active_prompt.lock().unwrap().clone();
                let system_prompt = resolve_prompt(&prompt_name);

                send!(UiMsg::Status("Thinking…".into()));

                let msgs = match ctx.build_prompt_with_category(&query, &recent, &chat_history, &system_prompt, Some(&prompt_name)).await {
                    Ok(m) => m,
                    Err(e) => { send!(UiMsg::Error(format!("Context: {e}"))); continue; }
                };

                match router.stream(&msgs, &cc).await {
                    Ok(mut stream) => {
                        let mut full = String::new();
                        while let Some(res) = stream.next().await {
                            match res {
                                Ok(tok) => { send!(UiMsg::Token(tok.clone())); full.push_str(&tok); }
                                Err(e) => { send!(UiMsg::Error(format!("Stream: {e}"))); break; }
                            }
                        }
                        send!(UiMsg::Done);
                        if !full.is_empty() {
                            // Speak the response via TTS (background thread)
                            if let Some(ref tts) = tts {
                                let tts = Arc::clone(tts);
                                let text = full.clone();
                                tokio::task::spawn_blocking(move || {
                                    if let Err(e) = tts.speak_and_play(&text) {
                                        tracing::warn!(error = %e, "TTS failed");
                                    }
                                });
                            }
                            chat_history.push((query.clone(), full.clone()));
                            // Keep at most 20 exchanges in memory
                            if chat_history.len() > 20 {
                                chat_history.remove(0);
                            }
                        }
                        let _ = session_store.add_exchange(
                            session_id, &query, &full,
                            &config.provider.default, &config.provider.model,
                        );
                    }
                    Err(e) => send!(UiMsg::Error(format!("LLM: {e}"))),
                }
            }
        }
    }
}
