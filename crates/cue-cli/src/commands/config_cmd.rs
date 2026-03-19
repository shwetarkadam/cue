use anyhow::Result;
use clap::{Args, Subcommand};
use cue_core::config::Config;

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommands>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Print current config as TOML
    Print,
    /// Show config file path
    Path,
    /// Initialize config file with defaults
    Init,
}

pub fn run(args: ConfigArgs) -> Result<()> {
    let command = args.command.unwrap_or(ConfigCommands::Print);

    match command {
        ConfigCommands::Print => {
            let config = Config::load()?;
            let toml = config.to_toml()?;
            println!("{}", toml);
        }
        ConfigCommands::Path => {
            let config_dir = Config::config_dir();
            let config_file = config_dir.join("cue.toml");
            println!("Config file:  {}", config_file.display());
            println!("Data dir:     {}", Config::data_dir().display());
            println!("Models dir:   {}", Config::models_dir().display());
            println!("Database:     {}", Config::db_path().display());
            println!(
                "Config exists: {}",
                if config_file.exists() { "yes" } else { "no" }
            );
        }
        ConfigCommands::Init => {
            Config::ensure_dirs()?;
            let config_file = Config::config_dir().join("cue.toml");
            if config_file.exists() {
                println!("Config file already exists: {}", config_file.display());
                println!("Use 'cue config print' to view it.");
            } else {
                let config = Config::default();
                let toml = config.to_toml()?;
                std::fs::write(&config_file, toml)?;
                println!("Config file created: {}", config_file.display());
                println!("Edit it to customize cue's behavior.");
            }
        }
    }
    Ok(())
}
