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
            for name in prompts::list_prompts() {
                println!("  {:<20} {}", name, prompts::prompt_description(name));
            }
            println!("\nUsage: cue start --prompt <name>");
        }
        PromptsCommands::Show { name } => {
            let content = prompts::get_prompt(&name);
            println!("=== Prompt: {} ===\n", name);
            println!("{}", content);
        }
        PromptsCommands::Use { name } => {
            // Validate
            let valid = prompts::list_prompts();
            if !valid.contains(&name.as_str()) {
                println!(
                    "Unknown prompt '{}'. Available: {}",
                    name,
                    valid.join(", ")
                );
                return Ok(());
            }
            println!("To use the '{}' prompt, run:", name);
            println!("  cue start --prompt {}", name);
        }
    }
    Ok(())
}
