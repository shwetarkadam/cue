use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// A chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".to_string(), content: content.into() }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".to_string(), content: content.into() }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".to_string(), content: content.into() }
    }
}

/// Configuration for a completion request
#[derive(Debug, Clone)]
pub struct CompletionConfig {
    pub temperature: f32,
    pub max_tokens: u32,
    pub model: String,
}

/// Streaming token output
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<String>> + Send>>;

/// Trait for LLM providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Stream completion tokens
    async fn stream(
        &self,
        messages: &[Message],
        config: &CompletionConfig,
    ) -> Result<TokenStream>;

    /// Provider name for logging
    fn name(&self) -> &str;
}
