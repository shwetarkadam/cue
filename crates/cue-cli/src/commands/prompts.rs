use anyhow::Result;
use clap::{Args, Subcommand};
use cue_core::prompts;

#[derive(Args, Debug)]
pub struct PromptsArgs {
    #[command(subcommand)]
    pub command: PromptsCommands,
}

#[derive(Subcommand, Debug)]
pub enum PromptsCommands {
    /// List all available prompts
    List,
    /// Show the content of a prompt
    Show {
        /// Prompt name
        name: String,
    },
    /// Use a prompt (prints the command to use it)
    Use {
        /// Prompt name
        name: String,
    },
}

pub fn run(args: PromptsArgs) -> Result<()> {
    match args.command {
        PromptsCommands::List => {
            println!("Available prompts:\n");
            println!("  Built-in:");
            for name in prompts::list_prompts() {
                println!("    {:<20} {}", name, prompts::prompt_description(name));
            }

            let all = prompts::list_all_prompts();
            let custom: Vec<_> = all
                .iter()
                .filter(|(name, _)| !prompts::list_prompts().contains(&name.as_str()))
                .collect();
            if !custom.is_empty() {
                println!("\n  Custom:");
                for (name, desc) in custom {
                    println!("    {:<20} {}", name, desc);
                }
            }

            println!("\nUsage: cue start --prompt <name>");
            println!("Create custom: cue brain prompt create <name> <content>");
        }
        PromptsCommands::Show { name } => {
            let content = prompts::resolve_prompt(&name);
            println!("=== Prompt: {} ===\n", name);
            println!("{}", content);
        }
        PromptsCommands::Use { name } => {
            let all = prompts::list_all_prompts();
            if !all.iter().any(|(n, _)| n == &name) {
                let names: Vec<_> = all.iter().map(|(n, _)| n.as_str()).collect();
                println!(
                    "Unknown prompt '{}'. Available: {}",
                    name,
                    names.join(", ")
                );
                return Ok(());
            }
            println!("To use the '{}' prompt, run:", name);
            println!("  cue start --prompt {}", name);
        }
    }
    Ok(())
}
