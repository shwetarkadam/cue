use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use cue_core::{
    audio::{AudioCapture, Utterance},
    config::Config,
    context::ContextEngine,
    kb::KnowledgeBase,
    llm::{self, CompletionConfig},
    prompts::get_prompt,
    session::{SessionStore, TranscriptEntry},
    stt::DeepgramStreamer,
};
use futures::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tracing::error;

// ── Event payloads ────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct TranscriptPayload {
    channel: String,
    text: String,
}

#[derive(Clone, Serialize)]
struct TokenPayload {
    token: String,
}

#[derive(Clone, Serialize)]
struct MessagePayload {
    message: String,
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct AppState {
    pub listening: Arc<AtomicBool>,
    pub query_tx: mpsc::Sender<String>,
}

// ── Commands (frontend → backend) ────────────────────────────────────────────

#[tauri::command]
async fn toggle_listening(state: State<'_, AppState>) -> Result<bool, String> {
    let new = !state.listening.load(Ordering::Relaxed);
    state.listening.store(new, Ordering::Relaxed);
    Ok(new)
}

#[tauri::command]
async fn send_query(query: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .query_tx
        .send(query)
        .await
        .map_err(|e| e.to_string())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn emit_status(app: &AppHandle, msg: impl Into<String>) {
    let _ = app.emit("status", MessagePayload { message: msg.into() });
}

fn emit_error(app: &AppHandle, msg: impl Into<String>) {
    let _ = app.emit("error", MessagePayload { message: msg.into() });
}

// ── Core pipeline (runs in dedicated OS thread with single-threaded runtime) ──

async fn run_pipeline(
    app: AppHandle,
    mut query_rx: mpsc::Receiver<String>,
    listening: Arc<AtomicBool>,
) {
    // Load config
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            emit_error(&app, format!("Config load failed: {e}"));
            return;
        }
    };
    let _ = Config::ensure_dirs();

    // Init DB
    let db = Config::db_path();
    let session_store = match SessionStore::new(&db) {
        Ok(s) => {
            let _ = s.init_schema();
            Arc::new(s)
        }
        Err(e) => {
            emit_error(&app, format!("Database error: {e}"));
            return;
        }
    };
    let kb = match KnowledgeBase::new(&db) {
        Ok(k) => {
            let _ = k.init_schema();
            Arc::new(k)
        }
        Err(e) => {
            emit_error(&app, format!("Knowledge base error: {e}"));
            return;
        }
    };

    let session = match session_store.create_session("general") {
        Ok(s) => s,
        Err(e) => {
            emit_error(&app, format!("Session error: {e}"));
            return;
        }
    };
    let session_id = session.id;

    // LLM
    let router = match llm::build_router(&config.provider) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            emit_error(&app, format!("LLM router error: {e}"));
            return;
        }
    };
    let cc = CompletionConfig {
        temperature: config.provider.temperature,
        max_tokens: config.provider.max_tokens,
        model: config.provider.model.clone(),
    };
    let ctx = Arc::new(ContextEngine::new(Arc::clone(&kb), config.rag.clone()));
    let prompt = get_prompt("general").to_string();

    // Audio capture — _stream kept alive in this stack frame
    let (utt_tx, utt_rx) = mpsc::channel::<Utterance>(16);
    let _stream = AudioCapture::new(config.audio.clone(), utt_tx)
        .start()
        .map_err(|e| emit_error(&app, format!("Microphone error: {e}")))
        .ok();

    // Transcript channel
    let (tx_entry, mut rx_entry) = mpsc::channel::<TranscriptEntry>(32);

    // Deepgram STT with Ctrl+L gating
    let deepgram_key = config.stt.deepgram_api_key.clone().unwrap_or_default();
    if !deepgram_key.is_empty() {
        let (dg_tx, dg_rx) = mpsc::channel::<Utterance>(32);
        let lis = Arc::clone(&listening);

        // Gate: only forward utterances when listening is active
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
            if let Err(e) = streamer.run(dg_rx, tx2, session_id).await {
                error!("Deepgram error: {e}");
            }
        });

        emit_status(&app, "Ready — press Ctrl+L to start listening");
    } else {
        emit_error(&app, "DEEPGRAM_API_KEY not set — transcript disabled");
        emit_status(&app, "Type a question to query AI");
    }

    // ── Main event loop ───────────────────────────────────────────────────────
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
                    recent.last()
                        .map(|e| e.text.clone())
                        .unwrap_or_else(|| "What was just discussed?".to_string())
                } else {
                    q
                };

                emit_status(&app, "Thinking...");

                let msgs = match ctx.build_prompt(&query, &recent, &prompt).await {
                    Ok(m) => m,
                    Err(e) => {
                        emit_error(&app, format!("Prompt build failed: {e}"));
                        continue;
                    }
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
                                Err(e) => {
                                    emit_error(&app, format!("Stream error: {e}"));
                                    break;
                                }
                            }
                        }
                        let _ = app.emit("response-done", ());
                        emit_status(&app, "Done. Ask anything.");
                        let _ = session_store.add_exchange(
                            session_id,
                            &query,
                            &full,
                            &config.provider.default,
                            &config.provider.model,
                        );
                    }
                    Err(e) => emit_error(&app, format!("LLM error: {e}")),
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

    let (query_tx, query_rx) = mpsc::channel::<String>(8);
    let listening = Arc::new(AtomicBool::new(false));
    let listening2 = Arc::clone(&listening);

    tauri::Builder::default()
        .manage(AppState { listening, query_tx })
        .invoke_handler(tauri::generate_handler![toggle_listening, send_query])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Dedicated OS thread with single-threaded runtime so cpal::Stream (!Send) is safe
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to build tokio runtime");
                rt.block_on(run_pipeline(handle, query_rx, listening2));
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
