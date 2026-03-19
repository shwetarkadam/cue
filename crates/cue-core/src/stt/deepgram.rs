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

    /// Start streaming: reads raw f32 samples from utterance_rx,
    /// sends to Deepgram WebSocket, emits TranscriptEntry via transcript_tx.
    pub async fn run(
        self,
        mut utterance_rx: mpsc::Receiver<Utterance>,
        transcript_tx: mpsc::Sender<TranscriptEntry>,
        session_id: i64,
    ) -> Result<()> {
        let url = "wss://api.deepgram.com/v1/listen?model=nova-2&language=en&encoding=linear16&sample_rate=16000&channels=1&punctuate=true&smart_format=true&interim_results=false";

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

        info!("Connecting to Deepgram WebSocket...");
        let (ws_stream, _) = connect_async_with_config(request, None, false)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to Deepgram: {}", e))?;
        info!("Connected to Deepgram");

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        // Spawn receiver task: read Deepgram responses
        let transcript_tx_clone = transcript_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = ws_stream.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<DeepgramResponse>(&text) {
                            Ok(resp) => {
                                let is_final = resp.is_final.unwrap_or(false)
                                    || resp.speech_final.unwrap_or(false);
                                if is_final {
                                    if let Some(channel) = resp.channel {
                                        if let Some(alt) = channel.alternatives.first() {
                                            let text = alt.transcript.trim().to_string();
                                            if !text.is_empty() {
                                                debug!(text = %text, "Deepgram transcript");
                                                let entry = TranscriptEntry {
                                                    id: None,
                                                    session_id,
                                                    channel: "mic".to_string(),
                                                    text,
                                                    spoken_at: chrono::Utc::now(),
                                                };
                                                let _ = transcript_tx_clone.send(entry).await;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => debug!(error = %e, raw = %text, "Failed to parse Deepgram response"),
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
        });

        // Send audio: read utterances, convert f32 to i16, send as binary
        while let Some(utterance) = utterance_rx.recv().await {
            let pcm_i16: Vec<i16> = utterance
                .samples
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .collect();

            // Convert i16 slice to bytes (little-endian)
            let bytes: Vec<u8> = pcm_i16
                .iter()
                .flat_map(|&s| s.to_le_bytes())
                .collect();

            if let Err(e) = ws_sink.send(Message::Binary(bytes.into())).await {
                error!(error = %e, "Failed to send audio to Deepgram");
                break;
            }
        }

        // Send close frame
        let _ = ws_sink.send(Message::Close(None)).await;
        Ok(())
    }
}

fn generate_ws_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // Simple base64-like key (not cryptographically correct but works for handshake)
    use std::fmt::Write;
    let mut key = String::new();
    for b in nonce.to_le_bytes().iter().chain(b"cue_deepgram") {
        write!(key, "{:02x}", b).ok();
    }
    // Must be 24 base64 chars — pad/truncate
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
        out.push(if i + 1 < input.len() {
            CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char
        } else {
            '='
        });
        out.push(if i + 2 < input.len() {
            CHARS[b2 & 0x3f] as char
        } else {
            '='
        });
        i += 3;
    }
    out
}
