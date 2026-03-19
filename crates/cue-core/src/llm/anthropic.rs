use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LlmProvider, Message, TokenStream};

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: SecretString,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: SecretString::from(api_key.into()),
        }
    }
}

/// Anthropic SSE delta
#[derive(Debug, Deserialize)]
struct ContentBlockDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseData {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<ContentBlockDelta>,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn stream(
        &self,
        messages: &[Message],
        config: &CompletionConfig,
    ) -> Result<TokenStream> {
        // Separate system from conversation messages
        let mut system_content: Option<String> = None;
        let mut conv_messages: Vec<serde_json::Value> = Vec::new();

        for m in messages {
            if m.role == "system" {
                system_content = Some(m.content.clone());
            } else {
                conv_messages.push(serde_json::json!({
                    "role": m.role,
                    "content": m.content
                }));
            }
        }

        let mut body = serde_json::json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "messages": conv_messages,
            "stream": true
        });

        if let Some(sys) = system_content {
            body["system"] = serde_json::Value::String(sys);
        }

        debug!(model = %config.model, "Sending request to Anthropic");

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send request to Anthropic")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status, text);
        }

        // Use mpsc channel to convert the bytes stream into a token stream
        let (tx, rx) = mpsc::channel::<Result<String>>(64);
        let mut bytes_stream = response.bytes_stream();

        tokio::spawn(async move {
            while let Some(chunk_result) = bytes_stream.next().await {
                match chunk_result {
                    Err(e) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Stream error: {}", e))).await;
                        break;
                    }
                    Ok(chunk) => {
                        let text = match std::str::from_utf8(&chunk) {
                            Ok(s) => s.to_string(),
                            Err(_) => continue,
                        };

                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let json_str = &line["data: ".len()..];
                                if json_str == "[DONE]" {
                                    continue;
                                }
                                match serde_json::from_str::<SseData>(json_str) {
                                    Ok(data) => {
                                        if data.event_type == "content_block_delta" {
                                            if let Some(delta) = data.delta {
                                                if delta.delta_type == "text_delta" {
                                                    if let Some(text) = delta.text {
                                                        if tx.send(Ok(text)).await.is_err() {
                                                            return;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Failed to parse Anthropic SSE");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        let stream = ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }
}
