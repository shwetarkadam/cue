use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

use cue_core::brain::{self, BrainStore};
use cue_core::config::Config;

#[derive(Args, Debug)]
pub struct BrainArgs {
    #[command(subcommand)]
    pub command: BrainCommands,
}

#[derive(Subcommand, Debug)]
pub enum BrainCommands {
    /// Create a new brain folder
    Create {
        /// Folder name (e.g., "introduction", "coding-tips")
        name: String,
        /// Link this folder to a prompt category (e.g., "coding", "behavioral")
        #[arg(short, long)]
        link: Option<String>,
    },

    /// Add a document to a brain folder
    Add {
        /// Folder name
        folder: String,
        /// File path to add
        file: PathBuf,
    },

    /// Write content directly to a brain folder document
    Write {
        /// Folder name
        folder: String,
        /// Document name
        name: String,
        /// Content (use - to read from stdin)
        content: String,
    },

    /// List all brain folders and their documents
    List {
        /// Show documents in a specific folder
        folder: Option<String>,
    },

    /// Show the content of a brain document
    Show {
        /// Folder name
        folder: String,
        /// Document name
        doc: String,
    },

    /// Remove a brain folder or document
    Remove {
        /// Folder name
        folder: String,
        /// Document name (if omitted, removes the entire folder)
        doc: Option<String>,
    },

    /// Set a note for a category
    Note {
        #[command(subcommand)]
        command: NoteCommands,
    },

    /// Manage custom system prompts
    Prompt {
        #[command(subcommand)]
        command: PromptCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum NoteCommands {
    /// Set/update a note for a category
    Set {
        /// Category name (matches prompt names: general, coding, behavioral, etc.)
        category: String,
        /// Note content
        content: String,
    },
    /// Show a note for a category
    Show {
        /// Category name
        category: String,
    },
    /// List all notes
    List,
    /// Remove a note
    Remove {
        /// Category name
        category: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptCommands {
    /// Create a new custom system prompt
    Create {
        /// Prompt name
        name: String,
        /// Short description
        #[arg(short, long, default_value = "Custom prompt")]
        description: String,
        /// Prompt content (the system prompt text)
        content: String,
    },
    /// List custom prompts
    List,
    /// Show a custom prompt
    Show {
        /// Prompt name
        name: String,
    },
    /// Remove a custom prompt
    Remove {
        /// Prompt name
        name: String,
    },
}

pub fn run(args: BrainArgs) -> Result<()> {
    let db_path = Config::db_path();
    let store = BrainStore::new(&db_path)?;
    store.init_schema()?;

    match args.command {
        BrainCommands::Create { name, link } => {
            store.create_folder(&name, link.as_deref())?;
            println!("Created brain folder: {}", name);
            if let Some(ref l) = link {
                println!("  Linked to prompt: {}", l);
            }
        }

        BrainCommands::Add { folder, file } => {
            let doc = store.add_document(&folder, &file)?;
            println!("Added '{}' to brain/{}", doc.name, folder);
        }

        BrainCommands::Write {
            folder,
            name,
            content,
        } => {
            let actual_content = if content == "-" {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                buf
            } else {
                content
            };
            store.add_document_content(&folder, &name, &actual_content)?;
            println!("Wrote '{}' to brain/{}", name, folder);
        }

        BrainCommands::List { folder } => {
            if let Some(folder_name) = folder {
                let docs = store.list_documents(&folder_name)?;
                if docs.is_empty() {
                    println!("No documents in brain/{}", folder_name);
                } else {
                    println!("brain/{}:\n", folder_name);
                    for doc in docs {
                        let preview: String = doc.content.chars().take(80).collect();
                        let preview = preview.replace('\n', " ");
                        println!("  {} — {}", doc.name, preview);
                    }
                }
            } else {
                let folders = store.list_folders()?;
                if folders.is_empty() {
                    println!("No brain folders yet. Create one with: cue brain create <name>");
                    return Ok(());
                }
                println!("Brain folders:\n");
                for f in &folders {
                    let docs = store.list_documents(&f.name).unwrap_or_default();
                    let link_str = f
                        .linked_prompt
                        .as_ref()
                        .map(|l| format!(" -> {}", l))
                        .unwrap_or_default();
                    println!("  {}{} ({} docs)", f.name, link_str, docs.len());
                }
            }
        }

        BrainCommands::Show { folder, doc } => {
            let docs = store.list_documents(&folder)?;
            if let Some(d) = docs.iter().find(|d| d.name == doc) {
                println!("=== brain/{}/{} ===\n", folder, doc);
                println!("{}", d.content);
            } else {
                println!("Document '{}' not found in brain/{}", doc, folder);
            }
        }

        BrainCommands::Remove { folder, doc } => {
            if let Some(doc_name) = doc {
                store.remove_document(&folder, &doc_name)?;
                println!("Removed brain/{}/{}", folder, doc_name);
            } else {
                store.remove_folder(&folder)?;
                println!("Removed brain folder: {}", folder);
            }
        }

        BrainCommands::Note { command } => match command {
            NoteCommands::Set { category, content } => {
                store.set_note(&category, &content)?;
                println!("Note saved for category: {}", category);
            }
            NoteCommands::Show { category } => {
                if let Some(note) = store.get_note(&category)? {
                    println!("=== Note: {} ===\n", category);
                    println!("{}", note.content);
                } else {
                    println!("No note for category: {}", category);
                }
            }
            NoteCommands::List => {
                let notes = store.list_notes()?;
                if notes.is_empty() {
                    println!("No notes yet. Add one with: cue brain note set <category> <content>");
                } else {
                    println!("Notes:\n");
                    for n in notes {
                        let preview: String = n.content.chars().take(60).collect();
                        let preview = preview.replace('\n', " ");
                        println!("  {:<20} {}", n.category, preview);
                    }
                }
            }
            NoteCommands::Remove { category } => {
                store.remove_note(&category)?;
                println!("Note removed for category: {}", category);
            }
        },

        BrainCommands::Prompt { command } => match command {
            PromptCommands::Create {
                name,
                description,
                content,
            } => {
                brain::ensure_prompts_dir()?;
                let path = brain::prompts_dir().join(format!("{}.toml", name));
                let toml_content = format!(
                    "name = {:?}\ndescription = {:?}\ncontent = {:?}\n",
                    name, description, content
                );
                std::fs::write(&path, toml_content)?;
                println!("Custom prompt '{}' saved to {}", name, path.display());
            }
            PromptCommands::List => {
                let custom = brain::load_custom_prompts()?;
                if custom.is_empty() {
                    println!(
                        "No custom prompts. Create one with: cue brain prompt create <name> <content>"
                    );
                } else {
                    println!("Custom prompts:\n");
                    for p in custom {
                        println!("  {:<20} {}", p.name, p.description);
                    }
                }
            }
            PromptCommands::Show { name } => {
                let custom = brain::load_custom_prompts()?;
                if let Some(p) = custom.iter().find(|p| p.name == name) {
                    println!("=== Custom Prompt: {} ===\n", p.name);
                    println!("{}", p.content);
                } else {
                    println!("Custom prompt '{}' not found", name);
                }
            }
            PromptCommands::Remove { name } => {
                let path = brain::prompts_dir().join(format!("{}.toml", name));
                if path.exists() {
                    std::fs::remove_file(&path)?;
                    println!("Custom prompt '{}' removed", name);
                } else {
                    // Try .md
                    let md_path = brain::prompts_dir().join(format!("{}.md", name));
                    if md_path.exists() {
                        std::fs::remove_file(&md_path)?;
                        println!("Custom prompt '{}' removed", name);
                    } else {
                        println!("Custom prompt '{}' not found", name);
                    }
                }
            }
        },
    }

    Ok(())
}
