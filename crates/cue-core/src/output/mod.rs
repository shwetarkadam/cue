use async_trait::async_trait;
use std::io::Write;
use std::sync::Mutex;
use tokio::sync::mpsc;

/// Output sink trait — all output modes implement this
#[async_trait]
pub trait OutputSink: Send + Sync {
    /// Emit a transcript entry (what was heard)
    async fn emit_transcript(&self, channel: &str, text: &str);
    /// Emit a streaming response token
    async fn emit_response_token(&self, token: &str);
    /// Called when response streaming is complete
    async fn emit_response_done(&self);
    /// Emit a status message
    async fn emit_status(&self, msg: &str);
    /// Emit an error message
    async fn emit_error(&self, msg: &str);
}

/// Stdout output: transcript → stderr, response tokens → stdout
pub struct StdoutOutput;

#[async_trait]
impl OutputSink for StdoutOutput {
    async fn emit_transcript(&self, channel: &str, text: &str) {
        let label = if channel == "system" { "THEM" } else { "YOU" };
        eprintln!("[{}] {}", label, text);
    }

    async fn emit_response_token(&self, token: &str) {
        print!("{}", token);
        let _ = std::io::stdout().flush();
    }

    async fn emit_response_done(&self) {
        println!();
    }

    async fn emit_status(&self, msg: &str) {
        eprintln!("[cue] {}", msg);
    }

    async fn emit_error(&self, msg: &str) {
        eprintln!("[cue ERROR] {}", msg);
    }
}

/// JSON output: emit structured JSON lines to stdout
pub struct JsonOutput;

#[async_trait]
impl OutputSink for JsonOutput {
    async fn emit_transcript(&self, channel: &str, text: &str) {
        let json = serde_json::json!({
            "type": "transcript",
            "channel": channel,
            "text": text,
            "ts": chrono::Utc::now().to_rfc3339()
        });
        println!("{}", json);
    }

    async fn emit_response_token(&self, token: &str) {
        let json = serde_json::json!({
            "type": "token",
            "text": token
        });
        println!("{}", json);
    }

    async fn emit_response_done(&self) {
        let json = serde_json::json!({
            "type": "done",
            "ts": chrono::Utc::now().to_rfc3339()
        });
        println!("{}", json);
    }

    async fn emit_status(&self, msg: &str) {
        let json = serde_json::json!({
            "type": "status",
            "text": msg,
            "ts": chrono::Utc::now().to_rfc3339()
        });
        println!("{}", json);
    }

    async fn emit_error(&self, msg: &str) {
        let json = serde_json::json!({
            "type": "error",
            "text": msg,
            "ts": chrono::Utc::now().to_rfc3339()
        });
        eprintln!("{}", json);
    }
}

// ─── TUI events ──────────────────────────────────────────────────────────────

/// Events sent from session code to the TUI renderer
#[derive(Debug, Clone)]
pub enum TuiEvent {
    Transcript { channel: String, text: String },
    ResponseToken(String),
    ResponseDone,
    Status(String),
    Error(String),
    Quit,
}

/// TUI output: forwards all events through an mpsc channel to the TUI thread
pub struct TuiOutput {
    tx: mpsc::Sender<TuiEvent>,
}

impl TuiOutput {
    pub fn new(tx: mpsc::Sender<TuiEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl OutputSink for TuiOutput {
    async fn emit_transcript(&self, channel: &str, text: &str) {
        let _ = self
            .tx
            .send(TuiEvent::Transcript {
                channel: channel.to_string(),
                text: text.to_string(),
            })
            .await;
    }

    async fn emit_response_token(&self, token: &str) {
        let _ = self
            .tx
            .send(TuiEvent::ResponseToken(token.to_string()))
            .await;
    }

    async fn emit_response_done(&self) {
        let _ = self.tx.send(TuiEvent::ResponseDone).await;
    }

    async fn emit_status(&self, msg: &str) {
        let _ = self.tx.send(TuiEvent::Status(msg.to_string())).await;
    }

    async fn emit_error(&self, msg: &str) {
        let _ = self.tx.send(TuiEvent::Error(msg.to_string())).await;
    }
}

// ─── Notify output ────────────────────────────────────────────────────────────

/// Desktop notification output via libnotify / D-Bus
pub struct NotifyOutput {
    app_name: String,
    response_buf: Mutex<String>,
}

impl NotifyOutput {
    pub fn new() -> Self {
        Self {
            app_name: "cue".to_string(),
            response_buf: Mutex::new(String::new()),
        }
    }
}

impl Default for NotifyOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputSink for NotifyOutput {
    async fn emit_transcript(&self, channel: &str, text: &str) {
        // Only notify on system audio (interviewer / other party)
        if channel == "system" {
            let _ = notify_rust::Notification::new()
                .appname(&self.app_name)
                .summary("Cue — Transcript")
                .body(text)
                .timeout(notify_rust::Timeout::Milliseconds(4000))
                .show();
        }
    }

    async fn emit_response_token(&self, token: &str) {
        if let Ok(mut buf) = self.response_buf.lock() {
            buf.push_str(token);
        }
    }

    async fn emit_response_done(&self) {
        let text = {
            let mut buf = self.response_buf.lock().unwrap_or_else(|e| e.into_inner());
            let out = buf.clone();
            buf.clear();
            out
        };
        if !text.trim().is_empty() {
            // Truncate long responses for the notification body
            let body: String = text.chars().take(300).collect();
            let body = if text.len() > 300 {
                format!("{}…", body)
            } else {
                body
            };
            let _ = notify_rust::Notification::new()
                .appname(&self.app_name)
                .summary("Cue — AI Response")
                .body(&body)
                .timeout(notify_rust::Timeout::Milliseconds(6000))
                .show();
        }
    }

    async fn emit_status(&self, _msg: &str) {
        // Status messages are not notified
    }

    async fn emit_error(&self, msg: &str) {
        let _ = notify_rust::Notification::new()
            .appname(&self.app_name)
            .summary("Cue — Error")
            .body(msg)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    }
}

// ─── Multi output ─────────────────────────────────────────────────────────────

/// Fan-out output: sends every event to multiple sinks simultaneously
pub struct MultiOutput {
    sinks: Vec<Box<dyn OutputSink>>,
}

impl MultiOutput {
    pub fn new(sinks: Vec<Box<dyn OutputSink>>) -> Self {
        Self { sinks }
    }
}

#[async_trait]
impl OutputSink for MultiOutput {
    async fn emit_transcript(&self, channel: &str, text: &str) {
        for sink in &self.sinks {
            sink.emit_transcript(channel, text).await;
        }
    }

    async fn emit_response_token(&self, token: &str) {
        for sink in &self.sinks {
            sink.emit_response_token(token).await;
        }
    }

    async fn emit_response_done(&self) {
        for sink in &self.sinks {
            sink.emit_response_done().await;
        }
    }

    async fn emit_status(&self, msg: &str) {
        for sink in &self.sinks {
            sink.emit_status(msg).await;
        }
    }

    async fn emit_error(&self, msg: &str) {
        for sink in &self.sinks {
            sink.emit_error(msg).await;
        }
    }
}

/// Build an output sink from config display mode string
pub fn build_output(mode: &str) -> Box<dyn OutputSink> {
    match mode {
        "json" => Box::new(JsonOutput),
        _ => Box::new(StdoutOutput),
    }
}
