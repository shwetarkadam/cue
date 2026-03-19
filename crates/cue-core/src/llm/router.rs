use anyhow::Result;
use tracing::{info, warn};

use super::provider::{CompletionConfig, LlmProvider, Message, TokenStream};

/// LLM router with primary + fallback chain
pub struct LlmRouter {
    primary: Box<dyn LlmProvider>,
    fallbacks: Vec<Box<dyn LlmProvider>>,
}

impl LlmRouter {
    pub fn new(primary: Box<dyn LlmProvider>, fallbacks: Vec<Box<dyn LlmProvider>>) -> Self {
        Self { primary, fallbacks }
    }

    pub fn single(provider: Box<dyn LlmProvider>) -> Self {
        Self {
            primary: provider,
            fallbacks: vec![],
        }
    }

    /// Stream tokens, trying primary first then fallbacks
    pub async fn stream(
        &self,
        messages: &[Message],
        config: &CompletionConfig,
    ) -> Result<TokenStream> {
        // Try primary
        match self.primary.stream(messages, config).await {
            Ok(stream) => {
                info!(provider = %self.primary.name(), "Using primary provider");
                return Ok(stream);
            }
            Err(e) => {
                warn!(
                    provider = %self.primary.name(),
                    error = %e,
                    "Primary provider failed, trying fallbacks"
                );
            }
        }

        // Try fallbacks
        for fallback in &self.fallbacks {
            match fallback.stream(messages, config).await {
                Ok(stream) => {
                    info!(provider = %fallback.name(), "Using fallback provider");
                    return Ok(stream);
                }
                Err(e) => {
                    warn!(
                        provider = %fallback.name(),
                        error = %e,
                        "Fallback provider failed"
                    );
                }
            }
        }

        Err(anyhow::anyhow!(crate::error::CueError::AllProvidersFailed))
    }

    pub fn primary_name(&self) -> &str {
        self.primary.name()
    }
}
