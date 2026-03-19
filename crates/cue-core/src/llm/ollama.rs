use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LlmProvider, Message, TokenStream};

/// Ollama local LLM provider (streaming NDJSON)
pub struct OllamaProvider {
    client: reqwest::Client,
    endpoint: String,
}

impl OllamaProvider {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OllamaChunk {
    message: Option<OllamaMessage>,
    #[allow(dead_code)]
    done: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: Option<String>,
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn stream(
        &self,
        messages: &[Message],
        config: &CompletionConfig,
    ) -> Result<TokenStream> {
        let ollama_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();

        let body = serde_json::json!({
            "model": config.model,
            "messages": ollama_messages,
            "stream": true,
            "options": {
                "temperature": config.temperature,
                "num_predict": config.max_tokens
            }
        });

        let url = format!("{}/api/chat", self.endpoint);
        debug!(model = %config.model, "Sending request to Ollama");

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to connect to Ollama — is it running? Try: ollama serve")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama API error {}: {}", status, text);
        }

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

                        // Ollama streams NDJSON — one JSON object per line
                        for line in text.lines() {
                            let line = line.trim();
                            if line.is_empty() {
                                continue;
                            }
                            match serde_json::from_str::<OllamaChunk>(line) {
                                Ok(chunk) => {
                                    if let Some(msg) = chunk.message {
                                        if let Some(content) = msg.content {
                                            if !content.is_empty()
                                                && tx.send(Ok(content)).await.is_err()
                                            {
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, line = %line, "Failed to parse Ollama NDJSON");
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
