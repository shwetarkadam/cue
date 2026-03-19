use anyhow::Result;
use clap::Args;
use cue_core::{
    audio::{AudioCapture, SystemAudioCapture, Utterance},
    config::Config,
    context::ContextEngine,
    kb::KnowledgeBase,
    llm::{self, CompletionConfig},
    output::{build_output, MultiOutput, NotifyOutput, OutputSink, TuiEvent, TuiOutput},
    prompts::get_prompt,
    session::{SessionStore, TranscriptEntry},
    stealth,
    stt::SttEngine,
};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::commands::daemon::{socket_path, start_daemon_socket, DaemonState};

#[derive(Args, Debug)]
pub struct StartArgs {
    /// System prompt template (general, coding, behavioral, system_design, meeting, sales)
    #[arg(short, long, default_value = "general")]
    pub prompt: String,

    /// Trigger mode: auto (trigger on questions), manual (press Enter), hotkey
    #[arg(short, long)]
    pub trigger: Option<String>,

    /// Output mode: stdout, json, tui
    #[arg(long)]
    pub output: Option<String>,

    /// Don't start audio capture, just run the LLM loop
    #[arg(long)]
    pub no_audio: bool,

    /// Enable TUI mode (full-screen split-panel interface)
    #[arg(long)]
    pub tui: bool,

    /// Enable desktop notifications for transcripts and AI responses
    #[arg(long)]
    pub notify: bool,

    /// Also capture system audio via pw-record (PipeWire monitor loopback)
    #[arg(long)]
    pub system_audio: bool,

    /// Run as background daemon; expose Unix socket for `cue ask` to connect to
    #[arg(long)]
    pub daemon: bool,
}

pub async fn run(args: StartArgs) -> Result<()> {
    let mut config = Config::load()?;
    Config::ensure_dirs()?;

    // Apply process camouflage if configured
    if config.stealth.camouflage_enabled {
        stealth::apply_camouflage(&config.stealth.camouflage_name);
    }

    // Override config from args
    if let Some(trigger) = args.trigger {
        config.trigger.mode = trigger;
    }
    if let Some(output_mode) = &args.output {
        config.display.mode = output_mode.clone();
    }

    // Determine effective output mode: --tui flag overrides --output
    let use_tui = args.tui || config.display.mode == "tui";

    // Build the output sink(s)
    let (output, tui_event_rx): (Box<dyn OutputSink>, Option<mpsc::Receiver<TuiEvent>>) = {
        if use_tui {
            let (tui_tx, tui_rx) = mpsc::channel::<TuiEvent>(128);
            let tui_out = Box::new(TuiOutput::new(tui_tx));
            if args.notify {
                let multi = MultiOutput::new(vec![tui_out, Box::new(NotifyOutput::new())]);
                (Box::new(multi), Some(tui_rx))
            } else {
                (tui_out, Some(tui_rx))
            }
        } else if args.notify {
            let base = build_output(&config.display.mode);
            let multi = MultiOutput::new(vec![base, Box::new(NotifyOutput::new())]);
            (Box::new(multi), None)
        } else {
            (build_output(&config.display.mode), None)
        }
    };

    output.emit_status("Starting cue...").await;

    // Check for API key
    if config.provider.api_key.is_none() && config.provider.default != "ollama" {
        let key_var = match config.provider.default.as_str() {
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            "groq" => "GROQ_API_KEY",
            _ => "API_KEY",
        };
        output
            .emit_error(&format!(
                "No API key found. Set {} environment variable or add to ~/.config/cue/.env",
                key_var
            ))
            .await;
    }

    // Initialize database
    let db_path = Config::db_path();
    let session_store = Arc::new(SessionStore::new(&db_path)?);
    session_store.init_schema()?;

    let kb = Arc::new(KnowledgeBase::new(&db_path)?);
    kb.init_schema()?;

    // Create session
    let session = session_store.create_session(&args.prompt)?;
    let session_id = session.id;
    info!(session_id = session_id, "Session started");

    output
        .emit_status(&format!(
            "Session {} started | Prompt: {} | Trigger: {}",
            session_id, args.prompt, config.trigger.mode
        ))
        .await;

    // Build LLM router
    let router = Arc::new(llm::build_router(&config.provider)?);
    let completion_config = CompletionConfig {
        temperature: config.provider.temperature,
        max_tokens: config.provider.max_tokens,
        model: config.provider.model.clone(),
    };

    // Build context engine
    let context_engine = Arc::new(ContextEngine::new(Arc::clone(&kb), config.rag.clone()));

    let system_prompt = get_prompt(&args.prompt).to_string();

    // Channel for utterances from audio capture
    let (utterance_tx, mut utterance_rx) = mpsc::channel::<Utterance>(16);

    // ── Microphone capture ──────────────────────────────────────────────────
    let _audio_stream = if !args.no_audio {
        let audio_capture = AudioCapture::new(config.audio.clone(), utterance_tx.clone());
        match audio_capture.start() {
            Ok(stream) => {
                output.emit_status("Microphone capture started").await;
                Some(stream)
            }
            Err(e) => {
                warn!(error = %e, "Failed to start microphone capture");
                output
                    .emit_error(&format!(
                        "Audio capture failed: {}. Running without audio.",
                        e
                    ))
                    .await;
                None
            }
        }
    } else {
        output
            .emit_status("Running without audio capture (--no-audio)")
            .await;
        None
    };

    // ── System audio capture via pw-record ──────────────────────────────────
    if args.system_audio {
        if SystemAudioCapture::is_available() {
            let monitor = SystemAudioCapture::find_monitor_device();
            let sys_capture = SystemAudioCapture::new(
                utterance_tx.clone(),
                monitor.clone(),
                config.audio.vad_sensitivity,
                config.audio.vad_silence_timeout_ms,
                config.audio.vad_min_speech_ms,
            );
            match sys_capture.start() {
                Ok(()) => {
                    output
                        .emit_status(&format!(
                            "System audio capture started (monitor: {})",
                            monitor.as_deref().unwrap_or("auto")
                        ))
                        .await;
                }
                Err(e) => {
                    warn!(error = %e, "Failed to start system audio capture");
                    output
                        .emit_error(&format!("System audio failed: {}", e))
                        .await;
                }
            }
        } else {
            output
                .emit_error(
                    "pw-record not found. Install pipewire-utils: sudo apt install pipewire-audio",
                )
                .await;
        }
    }

    // Channel for transcript entries
    let (transcript_tx, mut transcript_rx) = mpsc::channel::<TranscriptEntry>(32);

    // Load STT engine (if model exists)
    let models_dir = Config::models_dir();
    let model_path =
        cue_core::stt::SttEngine::model_path(&config.stt.model, &models_dir);

    let stt_engine: Option<Arc<SttEngine>> = if model_path.exists() {
        match SttEngine::new(&config.stt, &models_dir) {
            Ok(engine) => {
                output
                    .emit_status(&format!("STT loaded: whisper-{}", config.stt.model))
                    .await;
                Some(Arc::new(engine))
            }
            Err(e) => {
                warn!(error = %e, "Failed to load STT engine");
                output
                    .emit_error(&format!(
                        "STT load failed: {}. Transcription disabled.",
                        e
                    ))
                    .await;
                None
            }
        }
    } else {
        output
            .emit_error(&format!(
                "Whisper model not found. Run: cue models download {}",
                config.stt.model
            ))
            .await;
        None
    };

    // STT worker: reads utterances, transcribes, sends TranscriptEntry
    let transcript_tx_clone = transcript_tx.clone();
    let session_store_stt = Arc::clone(&session_store);

    let _stt_task = tokio::spawn(async move {
        while let Some(utterance) = utterance_rx.recv().await {
            if let Some(ref engine) = stt_engine {
                let engine = Arc::clone(engine);
                let samples = utterance.samples.clone();
                let channel = utterance.channel;
                let tx = transcript_tx_clone.clone();
                let store = Arc::clone(&session_store_stt);

                tokio::task::spawn_blocking(move || {
                    match engine.transcribe(&samples) {
                        Ok(text) if !text.trim().is_empty() => {
                            let entry = TranscriptEntry {
                                id: None,
                                session_id,
                                channel: channel.to_string(),
                                text: text.clone(),
                                spoken_at: chrono::Utc::now(),
                            };
                            let _ = store.add_transcript(&entry);
                            let _ = tx.blocking_send(entry);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            error!(error = %e, "STT transcription failed");
                        }
                    }
                });
            }
        }
    });

    let trigger_mode = config.trigger.mode.clone();

    output
        .emit_status(&format!(
            "Ready! Trigger mode: {}{}",
            trigger_mode,
            match trigger_mode.as_str() {
                "manual" => " — press Enter to query AI (or type a question)",
                "auto" => " — auto-triggers on questions ending with '?'",
                _ => "",
            }
        ))
        .await;

    if args.daemon {
        output
            .emit_status(&format!(
                "Daemon mode: socket at {}",
                socket_path().display()
            ))
            .await;
    }

    // Stdin reader: reads lines in a blocking thread, sends to async channel
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(8);

    // Only read stdin if we're not in TUI mode (TUI has its own keyboard handling)
    if !use_tui {
        tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if stdin_tx.blocking_send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Shared transcript buffer (for daemon socket access)
    let shared_transcript: Arc<tokio::sync::Mutex<Vec<TranscriptEntry>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Start daemon socket task if requested
    if args.daemon {
        let daemon_state = DaemonState {
            session_store: Arc::clone(&session_store),
            context_engine: Arc::clone(&context_engine),
            router: Arc::clone(&router),
            completion_config: completion_config.clone(),
            system_prompt: system_prompt.clone(),
            session_id,
            transcript_rx: Arc::clone(&shared_transcript),
            provider: config.provider.default.clone(),
            model: config.provider.model.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = start_daemon_socket(daemon_state).await {
                error!("Daemon socket error: {}", e);
            }
        });
    }

    // ── SIGUSR1 signal handler ───────────────────────────────────────────────
    // Sending SIGUSR1 to the process triggers an immediate LLM query of recent transcript.
    let (sigusr1_tx, mut sigusr1_rx) = mpsc::channel::<()>(4);
    {
        let sigusr1_tx = sigusr1_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            if let Ok(mut sig) = signal(SignalKind::user_defined1()) {
                loop {
                    sig.recv().await;
                    let _ = sigusr1_tx.send(()).await;
                }
            }
        });
    }

    // ── TUI task ─────────────────────────────────────────────────────────────
    if let Some(tui_rx) = tui_event_rx {
        tokio::spawn(crate::tui::run_tui(tui_rx));
    }

    // ── Main event loop ──────────────────────────────────────────────────────
    let mut recent_transcript: Vec<TranscriptEntry> = Vec::new();
    let window_secs = config.rag.transcript_window_secs;
    let trigger_mode_main = trigger_mode.clone();

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = &mut ctrl_c => {
                output.emit_status("Shutting down...").await;
                let _ = session_store.end_session(session_id);
                break;
            }

            Some(()) = sigusr1_rx.recv() => {
                // SIGUSR1 received — trigger LLM with last 60s of transcript
                let query = recent_transcript
                    .iter()
                    .rev()
                    .find(|e| e.channel == "system")
                    .map(|e| e.text.clone())
                    .unwrap_or_else(|| "Summarize the recent conversation".to_string());

                trigger_llm(
                    &query,
                    &recent_transcript,
                    &system_prompt,
                    &context_engine,
                    &router,
                    &completion_config,
                    &*output,
                    &session_store,
                    session_id,
                    &config.provider.model,
                    &config.provider.default,
                ).await;
            }

            Some(entry) = transcript_rx.recv() => {
                output.emit_transcript(&entry.channel, &entry.text).await;

                recent_transcript.push(entry.clone());
                let cutoff = chrono::Utc::now() - chrono::Duration::seconds(window_secs as i64);
                recent_transcript.retain(|e| e.spoken_at >= cutoff);

                // Keep shared transcript in sync for daemon
                {
                    let mut guard = shared_transcript.lock().await;
                    *guard = recent_transcript.clone();
                }

                // Auto-trigger on "?" at end of system audio
                if trigger_mode_main == "auto" {
                    let text = entry.text.trim().to_string();
                    if text.ends_with('?') {
                        trigger_llm(
                            &text,
                            &recent_transcript,
                            &system_prompt,
                            &context_engine,
                            &router,
                            &completion_config,
                            &*output,
                            &session_store,
                            session_id,
                            &config.provider.model,
                            &config.provider.default,
                        ).await;
                    }
                }
            }

            Some(line) = stdin_rx.recv() => {
                let line = line.trim().to_string();

                if line.is_empty() && trigger_mode_main == "manual" {
                    let query = recent_transcript
                        .last()
                        .map(|e| e.text.clone())
                        .unwrap_or_else(|| "What was just discussed?".to_string());

                    trigger_llm(
                        &query,
                        &recent_transcript,
                        &system_prompt,
                        &context_engine,
                        &router,
                        &completion_config,
                        &*output,
                        &session_store,
                        session_id,
                        &config.provider.model,
                        &config.provider.default,
                    ).await;
                } else if !line.is_empty() {
                    trigger_llm(
                        &line,
                        &recent_transcript,
                        &system_prompt,
                        &context_engine,
                        &router,
                        &completion_config,
                        &*output,
                        &session_store,
                        session_id,
                        &config.provider.model,
                        &config.provider.default,
                    ).await;
                }
            }
        }
    }

    info!("Session {} ended", session_id);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn trigger_llm(
    query: &str,
    transcript: &[TranscriptEntry],
    system_prompt: &str,
    context_engine: &ContextEngine,
    router: &cue_core::llm::LlmRouter,
    completion_config: &CompletionConfig,
    output: &dyn OutputSink,
    session_store: &SessionStore,
    session_id: i64,
    model: &str,
    provider: &str,
) {
    output.emit_status("Thinking...").await;

    let messages = match context_engine
        .build_prompt(query, transcript, system_prompt)
        .await
    {
        Ok(m) => m,
        Err(e) => {
            output
                .emit_error(&format!("Failed to build prompt: {}", e))
                .await;
            return;
        }
    };

    match router.stream(&messages, completion_config).await {
        Ok(mut stream) => {
            let mut full_response = String::new();
            while let Some(result) = stream.next().await {
                match result {
                    Ok(token) => {
                        output.emit_response_token(&token).await;
                        full_response.push_str(&token);
                    }
                    Err(e) => {
                        output
                            .emit_error(&format!("Stream error: {}", e))
                            .await;
                        break;
                    }
                }
            }
            output.emit_response_done().await;

            if let Err(e) = session_store.add_exchange(
                session_id,
                query,
                &full_response,
                provider,
                model,
            ) {
                warn!(error = %e, "Failed to persist exchange");
            }
        }
        Err(e) => {
            output
                .emit_error(&format!("LLM error: {}", e))
                .await;
        }
    }
}
