mod commands;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    ask::AskArgs,
    brain::BrainArgs,
    config_cmd::ConfigArgs,
    history::HistoryArgs,
    kb::KbArgs,
    load::LoadArgs,
    models::ModelsArgs,
    prompts::PromptsArgs,
    providers::ProvidersArgs,
    start::StartArgs,
};

#[derive(Parser, Debug)]
#[command(
    name = "cue",
    about = "Real-time AI meeting companion",
    long_about = "cue — real-time AI assistance for meetings, interviews, and sales calls.\n\nCaptures audio, transcribes with Whisper, and streams AI responses.\n\nQuick start:\n  cue models download tiny\n  ANTHROPIC_API_KEY=sk-ant-... cue start",
    version
)]
struct Cli {
    /// Enable debug logging (or set RUST_LOG=debug)
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start a live session (audio capture + STT + LLM)
    Start(StartArgs),

    /// Ask the AI a question with recent transcript context
    Ask(AskArgs),

    /// Listen and transcribe audio only (no LLM)
    Listen,

    /// Ingest a file into the knowledge base
    Load(LoadArgs),

    /// Manage the knowledge base
    Kb(KbArgs),

    /// Manage your brain (folders, notes, custom prompts)
    Brain(BrainArgs),

    /// View session history
    History(HistoryArgs),

    /// Manage and test LLM providers
    Providers(ProvidersArgs),

    /// List and use system prompts
    Prompts(PromptsArgs),

    /// List audio devices
    Devices,

    /// Manage Whisper models
    Models(ModelsArgs),

    /// View and edit configuration
    Config(ConfigArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let log_level = if cli.verbose { "debug" } else { "warn" };
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    match cli.command {
        Commands::Start(args) => commands::start::run(args).await,
        Commands::Ask(args) => commands::ask::run(args).await,
        Commands::Listen => commands::listen::run().await,
        Commands::Load(args) => commands::load::run(args),
        Commands::Kb(args) => commands::kb::run(args),
        Commands::Brain(args) => commands::brain::run(args),
        Commands::History(args) => commands::history::run(args),
        Commands::Providers(args) => commands::providers::run(args).await,
        Commands::Prompts(args) => commands::prompts::run(args),
        Commands::Devices => commands::devices::run(),
        Commands::Models(args) => commands::models::run(args).await,
        Commands::Config(args) => commands::config_cmd::run(args),
    }
}
