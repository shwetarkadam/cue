use anyhow::Result;
use clap::{Args, Subcommand};
use cue_core::{config::Config, llm};
use futures::StreamExt;

#[derive(Args, Debug)]
pub struct ProvidersArgs {
    #[command(subcommand)]
    pub command: ProvidersCommands,
}

#[derive(Subcommand, Debug)]
pub enum ProvidersCommands {
    /// List configured providers
    List,
    /// Test all configured providers
    Test,
}

pub async fn run(args: ProvidersArgs) -> Result<()> {
    let config = Config::load()?;

    match args.command {
        ProvidersCommands::List => {
            println!("Configured providers:");
            println!();
            println!("  Primary:  {}", config.provider.default);
            println!("  Model:    {}", config.provider.model);
            println!(
                "  API Key:  {}",
                if config.provider.api_key.is_some() { "set" } else { "NOT SET" }
            );
            if !config.provider.failover.is_empty() {
                println!("  Failover: {}", config.provider.failover.join(", "));
            }
            println!();
            println!("Supported providers:");
            println!("  anthropic  — claude-* models (requires ANTHROPIC_API_KEY)");
            println!("  openai     — gpt-* models (requires OPENAI_API_KEY)");
            println!("  groq       — llama/mixtral (requires GROQ_API_KEY)");
            println!("  mistral    — mistral-* models (requires MISTRAL_API_KEY)");
            println!("  openrouter — any model via openrouter.ai (requires OPENROUTER_API_KEY)");
            println!("  ollama     — local models via Ollama (no key needed)");
        }
        ProvidersCommands::Test => {
            println!("Testing provider: {}...", config.provider.default);

            if config.provider.api_key.is_none() && config.provider.default != "ollama" {
                println!("  SKIP — API key not set");
                return Ok(());
            }

            let router = llm::build_router(&config.provider)?;
            let config_lm = cue_core::llm::CompletionConfig {
                temperature: 0.0,
                max_tokens: 32,
                model: config.provider.model.clone(),
            };

            let messages = vec![cue_core::llm::Message::user("Say 'OK' and nothing else.")];

            match router.stream(&messages, &config_lm).await {
                Ok(mut stream) => {
                    let mut response = String::new();
                    while let Some(token) = stream.next().await {
                        match token {
                            Ok(t) => response.push_str(&t),
                            Err(e) => {
                                println!("  FAIL — stream error: {}", e);
                                return Ok(());
                            }
                        }
                    }
                    println!("  OK — response: {}", response.trim());
                }
                Err(e) => {
                    println!("  FAIL — {}", e);
                }
            }
        }
    }
    Ok(())
}
