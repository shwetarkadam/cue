pub mod anthropic;
pub mod ollama;
pub mod openai;
pub mod provider;
pub mod router;

pub use provider::{CompletionConfig, LlmProvider, Message, TokenStream};
pub use router::LlmRouter;

use anyhow::Result;
use crate::config::ProviderConfig;

/// Build an LlmRouter from config
pub fn build_router(config: &ProviderConfig) -> Result<LlmRouter> {
    let primary = build_single_provider(&config.default, config)?;

    let mut fallbacks: Vec<Box<dyn LlmProvider>> = Vec::new();
    for name in &config.failover {
        match build_single_provider(name, config) {
            Ok(p) => fallbacks.push(p),
            Err(e) => {
                tracing::warn!(provider = %name, error = %e, "Failed to build fallback provider");
            }
        }
    }

    Ok(LlmRouter::new(primary, fallbacks))
}

fn build_single_provider(name: &str, config: &ProviderConfig) -> Result<Box<dyn LlmProvider>> {
    match name {
        "anthropic" => {
            let key = config.api_key.as_deref().unwrap_or("")
                .to_string();
            if key.is_empty() {
                // Still build the provider; it will fail at request time with a clear error
                tracing::warn!("ANTHROPIC_API_KEY not set");
            }
            Ok(Box::new(anthropic::AnthropicProvider::new(key)))
        }
        "openai" => {
            let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
            Ok(Box::new(openai::OpenAiProvider::openai(key)))
        }
        "groq" => {
            let key = std::env::var("GROQ_API_KEY").unwrap_or_default();
            Ok(Box::new(openai::OpenAiProvider::groq(key)))
        }
        "mistral" => {
            let key = std::env::var("MISTRAL_API_KEY").unwrap_or_default();
            Ok(Box::new(openai::OpenAiProvider::mistral(key)))
        }
        "openrouter" => {
            let key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
            Ok(Box::new(openai::OpenAiProvider::openrouter(key)))
        }
        "ollama" => {
            Ok(Box::new(ollama::OllamaProvider::new(config.ollama_endpoint.clone())))
        }
        other => {
            // Try as OpenAI-compatible with custom base URL
            tracing::warn!(provider = %other, "Unknown provider, treating as OpenAI-compatible");
            let key = config.api_key.as_deref().unwrap_or("").to_string();
            Ok(Box::new(openai::OpenAiProvider::new(
                key,
                config.openai_base_url.clone(),
                other,
            )))
        }
    }
}
