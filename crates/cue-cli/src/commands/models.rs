use anyhow::Result;
use clap::{Args, Subcommand};
use cue_core::{config::Config, stt};

#[derive(Args, Debug)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsCommands,
}

#[derive(Subcommand, Debug)]
pub enum ModelsCommands {
    /// List downloaded models
    List,
    /// Download a whisper model
    Download {
        /// Model name: tiny, base, small, medium, large
        #[arg(default_value = "tiny")]
        name: String,
    },
}

pub async fn run(args: ModelsArgs) -> Result<()> {
    match args.command {
        ModelsCommands::List => {
            let models_dir = Config::models_dir();
            let models = stt::list_models(&models_dir)?;

            if models.is_empty() {
                println!("No models downloaded.");
                println!("Run: cue models download tiny");
            } else {
                println!("Downloaded models ({}/):", models_dir.display());
                for model in &models {
                    let path = stt::SttEngine::model_path(model, &models_dir);
                    let size = std::fs::metadata(&path)
                        .map(|m| format!("{:.1} MB", m.len() as f64 / 1_048_576.0))
                        .unwrap_or_else(|_| "?".to_string());
                    println!("  {} ({})", model, size);
                }
            }
        }
        ModelsCommands::Download { name } => {
            Config::ensure_dirs()?;
            let models_dir = Config::models_dir();
            println!("Downloading whisper-{}...", name);
            stt::download_model(&name, &models_dir).await?;
            println!("Done! Model saved to {}", stt::SttEngine::model_path(&name, &models_dir).display());
        }
    }
    Ok(())
}
