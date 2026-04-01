use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use cue_core::{
    audio::{AudioCapture, Utterance},
    brain::{BrainStore, BrainNote, BrainFolder, PrompterNote},
    config::Config,
    context::ContextEngine,
    kb::KnowledgeBase,
    llm::{self, CompletionConfig},
    prompts::{get_prompt, list_prompts, prompt_description},
    session::{SessionStore, TranscriptEntry},
    stt::{DeepgramStreamer, SttEvent},
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tracing::error;

// ── Shared state ──────────────────────────────────────────────────────────────

pub struct AppState {
    pub listening: Arc<AtomicBool>,
    pub query_tx: mpsc::Sender<String>,
    pub kb: Arc<KnowledgeBase>,
    pub brain: Arc<BrainStore>,
    pub session_store: Arc<SessionStore>,
    pub active_prompt: Arc<Mutex<String>>,
    pub session_id: i64,
}

// ── Event payloads ────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct TranscriptPayload { channel: String, text: String }

#[derive(Clone, Serialize)]
struct TokenPayload { token: String }

#[derive(Clone, Serialize)]
struct SttPayload { text: String, is_final: bool }

#[derive(Clone, Serialize)]
struct Msg { message: String }

// ── API types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub active_prompt: String,
    pub anthropic_key: String,
    pub openai_key: String,
    pub deepgram_key: String,
    pub groq_key: String,
    pub ollama_endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptInfo {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KbDoc {
    pub id: i64,
    pub name: String,
    pub doc_type: String,
    pub chunk_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    pub query: String,
    pub response: String,
    pub created_at: String,
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
async fn toggle_listening(state: State<'_, AppState>) -> Result<bool, String> {
    let new = !state.listening.load(Ordering::Relaxed);
    state.listening.store(new, Ordering::Relaxed);
    Ok(new)
}

#[tauri::command]
async fn send_query(query: String, state: State<'_, AppState>) -> Result<(), String> {
    state.query_tx.send(query).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn load_history(state: State<'_, AppState>) -> Result<Vec<HistoryItem>, String> {
    state
        .session_store
        .get_recent_exchanges(30)
        .map(|v| {
            v.into_iter()
                .map(|(q, r, t)| HistoryItem { query: q, response: r, created_at: t })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_prompts() -> Vec<PromptInfo> {
    list_prompts()
        .into_iter()
        .map(|id| PromptInfo {
            id: id.to_string(),
            label: prompt_label(id),
            description: prompt_description(id).to_string(),
        })
        .collect()
}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let config = Config::load().map_err(|e| e.to_string())?;
    let active_prompt = state.active_prompt.lock().unwrap().clone();
    Ok(Settings {
        provider: config.provider.default,
        model: config.provider.model,
        active_prompt,
        anthropic_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        openai_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
        deepgram_key: std::env::var("DEEPGRAM_API_KEY").unwrap_or_default(),
        groq_key: std::env::var("GROQ_API_KEY").unwrap_or_default(),
        ollama_endpoint: config.provider.ollama_endpoint,
    })
}

#[tauri::command]
async fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    let env_path = Config::config_dir().join(".env");
    let mut lines = Vec::new();
    if !settings.anthropic_key.is_empty() {
        lines.push(format!("ANTHROPIC_API_KEY={}", settings.anthropic_key));
    }
    if !settings.openai_key.is_empty() {
        lines.push(format!("OPENAI_API_KEY={}", settings.openai_key));
    }
    if !settings.deepgram_key.is_empty() {
        lines.push(format!("DEEPGRAM_API_KEY={}", settings.deepgram_key));
    }
    if !settings.groq_key.is_empty() {
        lines.push(format!("GROQ_API_KEY={}", settings.groq_key));
    }
    lines.push(format!("CUE_PROVIDER={}", settings.provider));
    lines.push(format!("CUE_MODEL={}", settings.model));
    if !settings.ollama_endpoint.is_empty() {
        lines.push(format!("OLLAMA_ENDPOINT={}", settings.ollama_endpoint));
    }
    std::fs::write(&env_path, lines.join("\n") + "\n").map_err(|e| e.to_string())?;

    *state.active_prompt.lock().unwrap() = settings.active_prompt;
    Ok(())
}

#[tauri::command]
async fn list_kb(state: State<'_, AppState>) -> Result<Vec<KbDoc>, String> {
    state
        .kb
        .list_documents()
        .map(|docs| {
            docs.into_iter()
                .map(|d| KbDoc {
                    id: d.id,
                    name: d.name,
                    doc_type: d.doc_type,
                    chunk_count: d.chunk_count,
                    created_at: d.created_at.format("%b %d, %Y").to_string(),
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_kb_doc(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.kb.remove_document(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn ingest_kb_file(path: String, state: State<'_, AppState>) -> Result<KbDoc, String> {
    let p = std::path::PathBuf::from(&path);
    state
        .kb
        .ingest_file(&p)
        .map(|d| KbDoc {
            id: d.id,
            name: d.name,
            doc_type: d.doc_type,
            chunk_count: d.chunk_count,
            created_at: d.created_at.format("%b %d, %Y").to_string(),
        })
        .map_err(|e| e.to_string())
}

// ── Prompter note commands ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PrompterNotePayload {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub sort_order: i64,
    pub prompt_category: String,
}

impl From<PrompterNote> for PrompterNotePayload {
    fn from(n: PrompterNote) -> Self {
        Self {
            id: n.id,
            title: n.title,
            content: n.content,
            sort_order: n.sort_order,
            prompt_category: n.prompt_category,
        }
    }
}

#[tauri::command]
async fn list_prompter_notes(category: String, state: State<'_, AppState>) -> Result<Vec<PrompterNotePayload>, String> {
    state
        .brain
        .list_prompter_notes(&category)
        .map(|v| v.into_iter().map(PrompterNotePayload::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_prompter_note(
    title: String,
    content: String,
    category: String,
    state: State<'_, AppState>,
) -> Result<PrompterNotePayload, String> {
    state
        .brain
        .add_prompter_note(&title, &content, &category)
        .map(PrompterNotePayload::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn update_prompter_note(
    id: i64,
    title: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<PrompterNotePayload, String> {
    state
        .brain
        .update_prompter_note(id, &title, &content)
        .map(PrompterNotePayload::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_prompter_note(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.brain.delete_prompter_note(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn reorder_prompter_notes(ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    state.brain.reorder_prompter_notes(&ids).map_err(|e| e.to_string())
}

// ── Brain note commands ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct BrainNotePayload {
    pub id: i64,
    pub category: String,
    pub content: String,
}

impl From<BrainNote> for BrainNotePayload {
    fn from(n: BrainNote) -> Self {
        Self { id: n.id, category: n.category, content: n.content }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BrainFolderPayload {
    pub id: i64,
    pub name: String,
    pub linked_prompt: Option<String>,
    pub doc_count: usize,
}

#[tauri::command]
async fn list_brain_notes(state: State<'_, AppState>) -> Result<Vec<BrainNotePayload>, String> {
    state.brain.list_notes()
        .map(|v| v.into_iter().map(BrainNotePayload::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_brain_note(category: String, content: String, state: State<'_, AppState>) -> Result<BrainNotePayload, String> {
    state.brain.set_note(&category, &content)
        .map(BrainNotePayload::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_brain_note(category: String, state: State<'_, AppState>) -> Result<(), String> {
    state.brain.remove_note(&category).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_brain_folders(state: State<'_, AppState>) -> Result<Vec<BrainFolderPayload>, String> {
    let folders = state.brain.list_folders().map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for f in folders {
        let doc_count = state.brain.list_documents(&f.name)
            .map(|d| d.len())
            .unwrap_or(0);
        result.push(BrainFolderPayload {
            id: f.id,
            name: f.name,
            linked_prompt: f.linked_prompt,
            doc_count,
        });
    }
    Ok(result)
}

#[tauri::command]
async fn create_brain_folder(name: String, linked_prompt: Option<String>, state: State<'_, AppState>) -> Result<BrainFolderPayload, String> {
    let f = state.brain.create_folder(&name, linked_prompt.as_deref()).map_err(|e| e.to_string())?;
    Ok(BrainFolderPayload { id: f.id, name: f.name, linked_prompt: f.linked_prompt, doc_count: 0 })
}

#[tauri::command]
async fn delete_brain_folder(name: String, state: State<'_, AppState>) -> Result<(), String> {
    state.brain.remove_folder(&name).map_err(|e| e.to_string())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn prompt_label(id: &str) -> String {
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

fn emit_status(app: &AppHandle, msg: impl Into<String>) {
    let _ = app.emit("status", Msg { message: msg.into() });
}

fn emit_error(app: &AppHandle, msg: impl Into<String>) {
    let _ = app.emit("error", Msg { message: msg.into() });
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

async fn run_pipeline(
    app: AppHandle,
    mut query_rx: mpsc::Receiver<String>,
    listening: Arc<AtomicBool>,
    kb: Arc<KnowledgeBase>,
    brain: Arc<BrainStore>,
    session_store: Arc<SessionStore>,
    active_prompt: Arc<Mutex<String>>,
    session_id: i64,
) {
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => { emit_error(&app, format!("Config: {e}")); return; }
    };

    let router = match llm::build_router(&config.provider) {
        Ok(r) => Arc::new(r),
        Err(e) => { emit_error(&app, format!("LLM: {e}")); return; }
    };
    let cc = CompletionConfig {
        temperature: config.provider.temperature,
        max_tokens: config.provider.max_tokens,
        model: config.provider.model.clone(),
    };
    let ctx = Arc::new(
        ContextEngine::new(Arc::clone(&kb), config.rag.clone())
            .with_brain(Arc::clone(&brain)),
    );

    // Audio capture — held alive in this stack frame
    let (utt_tx, utt_rx) = mpsc::channel::<Utterance>(16);
    let _stream = AudioCapture::new(config.audio.clone(), utt_tx)
        .start()
        .map_err(|e| emit_error(&app, format!("Mic: {e}")))
        .ok();

    // Transcript channel
    let (tx_entry, mut rx_entry) = mpsc::channel::<TranscriptEntry>(32);

    // STT event channel for real-time input box updates
    let (stt_tx, mut stt_rx) = mpsc::channel::<SttEvent>(64);
    let stt_app = app.clone();
    tokio::spawn(async move {
        while let Some(evt) = stt_rx.recv().await {
            let _ = stt_app.emit("stt-live", SttPayload {
                text: evt.text,
                is_final: evt.is_final,
            });
        }
    });

    // Deepgram with Ctrl+L gating
    let deepgram_key = config.stt.deepgram_api_key.clone().unwrap_or_default();
    if !deepgram_key.is_empty() {
        let (dg_tx, dg_rx) = mpsc::channel::<Utterance>(32);
        let lis = Arc::clone(&listening);
        tokio::spawn(async move {
            let mut rx = utt_rx;
            while let Some(u) = rx.recv().await {
                if lis.load(Ordering::Relaxed) {
                    let _ = dg_tx.send(u).await;
                }
            }
        });
        let streamer = DeepgramStreamer::new(deepgram_key);
        let tx2 = tx_entry.clone();
        tokio::spawn(async move {
            let mut dg_rx = dg_rx;
            if let Err(e) = streamer.run(&mut dg_rx, tx2, stt_tx, session_id).await {
                error!("Deepgram: {e}");
            }
        });
        emit_status(&app, "Ready — Ctrl+L to listen");
    } else {
        emit_error(&app, "DEEPGRAM_API_KEY not set — add it in Settings");
        emit_status(&app, "Type to ask AI (no audio)");
    }

    let mut recent: Vec<TranscriptEntry> = Vec::new();

    loop {
        tokio::select! {
            Some(entry) = rx_entry.recv() => {
                let _ = app.emit("transcript", TranscriptPayload {
                    channel: entry.channel.clone(),
                    text: entry.text.clone(),
                });
                recent.push(entry);
                let cutoff = chrono::Utc::now() - chrono::Duration::seconds(120);
                recent.retain(|e| e.spoken_at >= cutoff);
            }

            Some(q) = query_rx.recv() => {
                let query = if q.is_empty() {
                    recent.last().map(|e| e.text.clone())
                        .unwrap_or_else(|| "What was just discussed?".into())
                } else { q };

                // Read active prompt each time (may have changed in settings)
                let prompt_name = active_prompt.lock().unwrap().clone();
                let system_prompt = get_prompt(&prompt_name).to_string();

                emit_status(&app, "Thinking...");

                // Build chat history from recent conversation
                let chat_history: Vec<(String, String)> = session_store
                    .get_recent_exchanges(10)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(q, r, _)| (q, r))
                    .collect();

                let msgs = match ctx.build_prompt_with_category(&query, &recent, &chat_history, &system_prompt, Some(&prompt_name)).await {
                    Ok(m) => m,
                    Err(e) => { emit_error(&app, format!("Context: {e}")); continue; }
                };

                match router.stream(&msgs, &cc).await {
                    Ok(mut stream) => {
                        let mut full = String::new();
                        while let Some(res) = stream.next().await {
                            match res {
                                Ok(tok) => {
                                    let _ = app.emit("response-token", TokenPayload { token: tok.clone() });
                                    full.push_str(&tok);
                                }
                                Err(e) => { emit_error(&app, format!("Stream: {e}")); break; }
                            }
                        }
                        let _ = app.emit("response-done", ());
                        emit_status(&app, "Done. Ask anything.");
                        let _ = session_store.add_exchange(
                            session_id, &query, &full,
                            &config.provider.default, &config.provider.model,
                        );
                    }
                    Err(e) => emit_error(&app, format!("LLM: {e}")),
                }
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter("cue_overlay=info,cue_core=info")
        .init();

    // Pre-initialize shared resources
    let _ = Config::ensure_dirs();
    let db = Config::db_path();

    let session_store = Arc::new(
        SessionStore::new(&db).expect("Failed to open session DB"),
    );
    let _ = session_store.init_schema();

    let kb = Arc::new(KnowledgeBase::new(&db).expect("Failed to open KB"));
    let _ = kb.init_schema();

    let brain = Arc::new(BrainStore::new(&db).expect("Failed to open BrainStore"));
    let _ = brain.init_schema();

    let active_prompt = Arc::new(Mutex::new("general".to_string()));

    // Create session upfront
    let session_id = session_store
        .create_session("general")
        .expect("Failed to create session")
        .id;

    let (query_tx, query_rx) = mpsc::channel::<String>(8);
    let listening = Arc::new(AtomicBool::new(false));

    // Clones for pipeline thread
    let kb2 = Arc::clone(&kb);
    let brain2 = Arc::clone(&brain);
    let ss2 = Arc::clone(&session_store);
    let ap2 = Arc::clone(&active_prompt);
    let listening2 = Arc::clone(&listening);

    tauri::Builder::default()
        .manage(AppState { listening, query_tx, kb, brain, session_store, active_prompt, session_id })
        .invoke_handler(tauri::generate_handler![
            toggle_listening,
            send_query,
            load_history,
            get_prompts,
            get_settings,
            save_settings,
            list_kb,
            delete_kb_doc,
            ingest_kb_file,
            list_prompter_notes,
            add_prompter_note,
            update_prompter_note,
            delete_prompter_note,
            reorder_prompter_notes,
            list_brain_notes,
            save_brain_note,
            delete_brain_note,
            list_brain_folders,
            create_brain_folder,
            delete_brain_folder,
        ])
        .setup(move |app| {
            // Auto-float + hide from screencast on Hyprland (no manual config needed)
            let _ = std::process::Command::new("hyprctl")
                .args(["keyword", "windowrulev2", "float,class:cue-overlay"])
                .output();
            let _ = std::process::Command::new("hyprctl")
                .args(["keyword", "windowrulev2", "pin,class:cue-overlay"])
                .output();
            let _ = std::process::Command::new("hyprctl")
                .args(["keyword", "windowrulev2", "noscreencast,class:cue-overlay"])
                .output();
            // Also match on productName "cue" (Wayland app-id)
            let _ = std::process::Command::new("hyprctl")
                .args(["keyword", "windowrulev2", "float,class:cue"])
                .output();
            let _ = std::process::Command::new("hyprctl")
                .args(["keyword", "windowrulev2", "pin,class:cue"])
                .output();
            let _ = std::process::Command::new("hyprctl")
                .args(["keyword", "windowrulev2", "noscreencast,class:cue"])
                .output();

            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(run_pipeline(handle, query_rx, listening2, kb2, brain2, ss2, ap2, session_id));
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
