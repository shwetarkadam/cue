use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LlmProvider, Message, TokenStream};

/// OpenAI-compatible provider (works with OpenAI, Groq, Mistral, OpenRouter, etc.)
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: SecretString,
    base_url: String,
    provider_name: String,
}

impl OpenAiProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        provider_name: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: SecretString::from(api_key.into()),
            base_url: base_url.into(),
            provider_name: provider_name.into(),
        }
    }

    pub fn openai(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "https://api.openai.com/v1", "openai")
    }

    pub fn groq(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "https://api.groq.com/openai/v1", "groq")
    }

    pub fn mistral(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "https://api.mistral.ai/v1", "mistral")
    }

    pub fn openrouter(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "https://openrouter.ai/api/v1", "openrouter")
    }
}

#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    delta: Delta,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<Choice>,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn stream(
        &self,
        messages: &[Message],
        config: &CompletionConfig,
    ) -> Result<TokenStream> {
        let msgs: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();

        let body = serde_json::json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "messages": msgs,
            "stream": true
        });

        let url = format!("{}/chat/completions", self.base_url);
        debug!(provider = %self.provider_name, model = %config.model, "Sending request");

        let response = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send request")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("{} API error {}: {}", self.provider_name, status, text);
        }

        let (tx, rx) = mpsc::channel::<Result<String>>(64);
        let mut bytes_stream = response.bytes_stream();

        tokio::spawn(async move {
            // Buffer partial lines across HTTP chunks — SSE lines don't align with chunk boundaries
            let mut buf = String::new();

            while let Some(chunk_result) = bytes_stream.next().await {
                match chunk_result {
                    Err(e) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Stream error: {}", e))).await;
                        break;
                    }
                    Ok(chunk) => {
                        let text = match std::str::from_utf8(&chunk) {
                            Ok(s) => s,
                            Err(_) => continue,
                        };
                        buf.push_str(text);

                        // Process only complete newline-terminated lines
                        while let Some(pos) = buf.find('\n') {
                            let line = buf[..pos].trim_end_matches('\r').to_string();
                            buf.drain(..=pos);

                            if !line.starts_with("data: ") {
                                continue;
                            }
                            let json_str = line["data: ".len()..].trim();
                            if json_str == "[DONE]" {
                                continue;
                            }
                            match serde_json::from_str::<StreamChunk>(json_str) {
                                Ok(sc) => {
                                    for choice in sc.choices {
                                        if let Some(content) = choice.delta.content {
                                            if !content.is_empty()
                                                && tx.send(Ok(content)).await.is_err()
                                            {
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, line = %json_str, "Failed to parse OpenAI SSE");
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
