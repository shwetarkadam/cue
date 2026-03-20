use anyhow::Result;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async_with_config, tungstenite::Message};
use tracing::{debug, error, info};

use crate::audio::Utterance;
use crate::session::TranscriptEntry;

#[derive(Debug, serde::Deserialize)]
struct DeepgramResponse {
    channel: Option<DeepgramChannel>,
    is_final: Option<bool>,
    speech_final: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
struct DeepgramChannel {
    alternatives: Vec<DeepgramAlternative>,
}

#[derive(Debug, serde::Deserialize)]
struct DeepgramAlternative {
    transcript: String,
    #[allow(dead_code)]
    confidence: Option<f64>,
}

pub struct DeepgramStreamer {
    api_key: String,
}

impl DeepgramStreamer {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Stream audio to Deepgram with automatic reconnect on connection drop.
    /// Returns when `utterance_rx` is closed (app shutdown).
    pub async fn run(
        &self,
        utterance_rx: &mut mpsc::Receiver<Utterance>,
        transcript_tx: mpsc::Sender<TranscriptEntry>,
        session_id: i64,
    ) -> Result<()> {
        loop {
            match self.run_once(utterance_rx, &transcript_tx, session_id).await {
                Ok(true) => {
                    info!("Deepgram: audio source closed, shutting down");
                    return Ok(());
                }
                Ok(false) => {
                    info!("Deepgram: connection closed by server, reconnecting in 4s…");
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                }
                Err(e) => {
                    error!("Deepgram: {e}, reconnecting in 4s…");
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                }
            }
        }
    }

    /// One WebSocket session. Returns:
    ///   Ok(true)  = utterance_rx closed (normal app shutdown)
    ///   Ok(false) = server closed the connection (reconnect needed)
    ///   Err(_)    = send/connect error (reconnect needed)
    async fn run_once(
        &self,
        utterance_rx: &mut mpsc::Receiver<Utterance>,
        transcript_tx: &mpsc::Sender<TranscriptEntry>,
        session_id: i64,
    ) -> Result<bool> {
        let url = "wss://api.deepgram.com/v1/listen?\
            model=nova-2&language=en&encoding=linear16&sample_rate=16000\
            &channels=1&punctuate=true&smart_format=true&interim_results=false\
            &keepalive=true";

        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(url)
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Host", "api.deepgram.com")
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", generate_ws_key())
            .header("Sec-WebSocket-Version", "13")
            .body(())
            .unwrap();

        info!("Connecting to Deepgram WebSocket…");
        let (ws, _) = connect_async_with_config(request, None, false)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to Deepgram: {e}"))?;
        info!("Connected to Deepgram");

        let (mut ws_sink, mut ws_recv) = ws.split();

        // Oneshot: receiver task signals sender when WS closes
        let (close_tx, mut close_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn WS receiver task
        let tx = transcript_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = ws_recv.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(resp) = serde_json::from_str::<DeepgramResponse>(&text) {
                            let is_final = resp.is_final.unwrap_or(false)
                                || resp.speech_final.unwrap_or(false);
                            if is_final {
                                if let Some(channel) = resp.channel {
                                    if let Some(alt) = channel.alternatives.first() {
                                        let t = alt.transcript.trim().to_string();
                                        if !t.is_empty() {
                                            debug!(text = %t, "Deepgram transcript");
                                            let entry = TranscriptEntry {
                                                id: None,
                                                session_id,
                                                channel: "mic".to_string(),
                                                text: t,
                                                spoken_at: chrono::Utc::now(),
                                            };
                                            let _ = tx.send(entry).await;
                                        }
                                    }
                                }
                            }
                        } else {
                            debug!(raw = %text, "Unparseable Deepgram response");
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("Deepgram WebSocket closed");
                        break;
                    }
                    Err(e) => {
                        error!(error = %e, "Deepgram WebSocket error");
                        break;
                    }
                    _ => {}
                }
            }
            let _ = close_tx.send(());
        });

        // Sender loop: read utterances and forward to Deepgram.
        // Send KeepAlive JSON every 8s of silence so Deepgram doesn't close.
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(8));
        keepalive.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                utterance = utterance_rx.recv() => {
                    match utterance {
                        Some(u) => {
                            keepalive.reset();
                            let pcm: Vec<u8> = u.samples.iter()
                                .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                                .flat_map(|s: i16| s.to_le_bytes())
                                .collect();
                            if let Err(e) = ws_sink.send(Message::Binary(pcm.into())).await {
                                error!(error = %e, "Failed to send audio to Deepgram");
                                return Ok(false); // trigger reconnect
                            }
                        }
                        None => {
                            // Sender dropped — app is shutting down
                            let _ = ws_sink.send(Message::Close(None)).await;
                            return Ok(true);
                        }
                    }
                }
                _ = keepalive.tick() => {
                    // No audio for 8s — send KeepAlive to hold the connection
                    let msg = Message::Text(r#"{"type": "KeepAlive"}"#.into());
                    if let Err(e) = ws_sink.send(msg).await {
                        error!(error = %e, "KeepAlive send failed");
                        return Ok(false);
                    }
                }
                _ = &mut close_rx => {
                    // WS closed by server side
                    return Ok(false); // trigger reconnect
                }
            }
        }
    }
}

fn generate_ws_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    use std::fmt::Write;
    let mut key = String::new();
    for b in nonce.to_le_bytes().iter().chain(b"cue_deepgram") {
        write!(key, "{:02x}", b).ok();
    }
    let bytes = key.as_bytes();
    let subset: Vec<u8> = bytes.iter().take(18).cloned().collect();
    base64_encode(&subset)
}

fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i] as usize;
        let b1 = if i + 1 < input.len() { input[i + 1] as usize } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] as usize } else { 0 };
        out.push(CHARS[b0 >> 2] as char);
        out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if i + 1 < input.len() { CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char } else { '=' });
        out.push(if i + 2 < input.len() { CHARS[b2 & 0x3f] as char } else { '=' });
        i += 3;
    }
    out
}
