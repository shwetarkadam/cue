//! Unix socket daemon support for `cue start --daemon`.
//!
//! The daemon listens on `$XDG_RUNTIME_DIR/cue.sock` (fallback: `/tmp/cue-$USER.sock`).
//!
//! JSON-line protocol:
//!   Request:  {"type":"ask","query":"what was just said?"}\n
//!   Response: {"type":"token","text":"..."}\n  (one per token)
//!             {"type":"done"}\n
//!   Error:    {"type":"error","message":"..."}\n

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use cue_core::{
    context::ContextEngine,
    llm::{CompletionConfig, LlmRouter},
    session::{SessionStore, TranscriptEntry},
};
use futures::StreamExt;

/// Returns the Unix socket path for this user.
pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("cue.sock");
    }
    // Fallback: /tmp/cue-$USER.sock
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    PathBuf::from(format!("/tmp/cue-{}.sock", user))
}

/// Shared session state passed to each client handler.
#[derive(Clone)]
pub struct DaemonState {
    pub session_store: Arc<SessionStore>,
    pub context_engine: Arc<ContextEngine>,
    pub router: Arc<LlmRouter>,
    pub completion_config: CompletionConfig,
    pub system_prompt: String,
    pub session_id: i64,
    pub transcript_rx: Arc<tokio::sync::Mutex<Vec<TranscriptEntry>>>,
    pub provider: String,
    pub model: String,
}

/// Start the Unix socket listener. Runs forever until cancelled.
pub async fn start_daemon_socket(state: DaemonState) -> Result<()> {
    let path = socket_path();

    // Remove stale socket file
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove stale socket: {}", path.display()))?;
    }

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("Failed to bind Unix socket: {}", path.display()))?;

    info!(path = %path.display(), "Daemon socket listening");
    eprintln!("[cue] Daemon socket: {}", path.display());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, state).await {
                        warn!("Daemon client error: {}", e);
                    }
                });
            }
            Err(e) => error!("Socket accept error: {}", e),
        }
    }
}

async fn handle_client(mut stream: UnixStream, state: DaemonState) -> Result<()> {
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({"type": "error", "message": format!("Invalid JSON: {}", e)});
                writer.write_all(format!("{}\n", err).as_bytes()).await?;
                continue;
            }
        };

        match request.get("type").and_then(|v| v.as_str()) {
            Some("ask") => {
                let query = request
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("What was just said?")
                    .to_string();

                let transcript = {
                    let guard = state.transcript_rx.lock().await;
                    guard.clone()
                };

                let messages = match state
                    .context_engine
                    .build_prompt(&query, &transcript, &state.system_prompt)
                    .await
                {
                    Ok(m) => m,
                    Err(e) => {
                        let err = serde_json::json!({"type": "error", "message": format!("Context error: {}", e)});
                        writer.write_all(format!("{}\n", err).as_bytes()).await?;
                        continue;
                    }
                };

                match state.router.stream(&messages, &state.completion_config).await {
                    Ok(mut stream_rx) => {
                        let mut full_response = String::new();
                        while let Some(result) = stream_rx.next().await {
                            match result {
                                Ok(token) => {
                                    full_response.push_str(&token);
                                    let msg = serde_json::json!({"type": "token", "text": token});
                                    writer.write_all(format!("{}\n", msg).as_bytes()).await?;
                                }
                                Err(e) => {
                                    let err = serde_json::json!({"type": "error", "message": format!("Stream error: {}", e)});
                                    writer.write_all(format!("{}\n", err).as_bytes()).await?;
                                    break;
                                }
                            }
                        }
                        let done = serde_json::json!({"type": "done"});
                        writer.write_all(format!("{}\n", done).as_bytes()).await?;

                        // Persist exchange
                        let _ = state.session_store.add_exchange(
                            state.session_id,
                            &query,
                            &full_response,
                            &state.provider,
                            &state.model,
                        );
                    }
                    Err(e) => {
                        let err = serde_json::json!({"type": "error", "message": format!("LLM error: {}", e)});
                        writer.write_all(format!("{}\n", err).as_bytes()).await?;
                    }
                }
            }
            Some("ping") => {
                let pong = serde_json::json!({"type": "pong"});
                writer.write_all(format!("{}\n", pong).as_bytes()).await?;
            }
            Some(t) => {
                let err = serde_json::json!({"type": "error", "message": format!("Unknown request type: {}", t)});
                writer.write_all(format!("{}\n", err).as_bytes()).await?;
            }
            None => {
                let err = serde_json::json!({"type": "error", "message": "Missing 'type' field"});
                writer.write_all(format!("{}\n", err).as_bytes()).await?;
            }
        }
    }

    Ok(())
}
