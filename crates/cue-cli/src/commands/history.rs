use anyhow::Result;
use clap::{Args, Subcommand};
use cue_core::{config::Config, session::SessionStore};

#[derive(Args, Debug)]
pub struct HistoryArgs {
    #[command(subcommand)]
    pub command: HistoryCommands,
}

#[derive(Subcommand, Debug)]
pub enum HistoryCommands {
    /// List all sessions
    List,
    /// Search transcript history
    Search {
        /// Search query
        query: String,
    },
    /// Show full transcript for a session
    Show {
        /// Session ID
        id: i64,
    },
    /// Export a session to Markdown or JSON
    Export {
        /// Session ID
        id: i64,
        /// Output format: md or json
        #[arg(long, default_value = "md")]
        format: String,
        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
}

pub fn run(args: HistoryArgs) -> Result<()> {
    Config::ensure_dirs()?;
    let db_path = Config::db_path();
    let store = SessionStore::new(&db_path)?;
    store.init_schema()?;

    match args.command {
        HistoryCommands::List => {
            let sessions = store.list_sessions()?;
            if sessions.is_empty() {
                println!("No sessions recorded.");
                println!("Start a session with: cue start");
            } else {
                println!("{:<6} {:<12} {:<15} {}", "ID", "DATE", "PROMPT", "TITLE");
                println!("{}", "-".repeat(60));
                for s in &sessions {
                    println!(
                        "{:<6} {:<12} {:<15} {}",
                        s.id,
                        s.started_at.format("%Y-%m-%d"),
                        s.prompt_template,
                        s.title.as_deref().unwrap_or("(untitled)")
                    );
                }
                println!("\n{} sessions", sessions.len());
            }
        }

        HistoryCommands::Search { query } => {
            let entries = store.search_transcript(&query)?;
            if entries.is_empty() {
                println!("No results found for: {}", query);
            } else {
                println!("Found {} transcript entries:\n", entries.len());
                for entry in &entries {
                    println!(
                        "[{}] [session:{}] [{}] {}",
                        entry.spoken_at.format("%Y-%m-%d %H:%M"),
                        entry.session_id,
                        entry.channel.to_uppercase(),
                        entry.text
                    );
                }
            }
        }

        HistoryCommands::Show { id } => {
            let transcript = store.get_recent_transcript(id, u64::MAX)?;
            let exchanges = store.get_exchanges(id)?;

            if transcript.is_empty() && exchanges.is_empty() {
                println!("Session {} not found or empty.", id);
                return Ok(());
            }

            println!("=== Session {} Transcript ===\n", id);
            for entry in &transcript {
                let label = if entry.channel == "system" { "THEM" } else { "YOU" };
                println!(
                    "[{}] [{}] {}",
                    entry.spoken_at.format("%H:%M:%S"),
                    label,
                    entry.text
                );
            }

            if !exchanges.is_empty() {
                println!("\n=== AI Exchanges ===\n");
                for (query, response, created_at) in &exchanges {
                    println!("Q [{}]: {}", created_at, query);
                    println!("A: {}\n", response);
                }
            }
        }

        HistoryCommands::Export { id, format, output } => {
            export_session(&store, id, &format, output.as_deref())?;
        }
    }
    Ok(())
}

fn export_session(
    store: &SessionStore,
    session_id: i64,
    format: &str,
    output_path: Option<&str>,
) -> Result<()> {
    // Load session metadata from list
    let sessions = store.list_sessions()?;
    let session = sessions.iter().find(|s| s.id == session_id).cloned();

    let transcript = store.get_recent_transcript(session_id, u64::MAX)?;
    let exchanges = store.get_exchanges(session_id)?;

    if transcript.is_empty() && exchanges.is_empty() {
        anyhow::bail!("Session {} not found or empty.", session_id);
    }

    let content = match format {
        "json" => export_json(session_id, &session, &transcript, &exchanges)?,
        "md" | "markdown" | _ => export_markdown(session_id, &session, &transcript, &exchanges)?,
    };

    match output_path {
        Some(path) => {
            std::fs::write(path, &content)?;
            println!("Exported session {} to {}", session_id, path);
        }
        None => print!("{}", content),
    }

    Ok(())
}

fn export_markdown(
    session_id: i64,
    session: &Option<cue_core::session::Session>,
    transcript: &[cue_core::session::TranscriptEntry],
    exchanges: &[(String, String, String)],
) -> Result<String> {
    let mut out = String::new();

    let title = session
        .as_ref()
        .and_then(|s| s.title.clone())
        .unwrap_or_else(|| format!("Session {}", session_id));

    let date = session
        .as_ref()
        .map(|s| s.started_at.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "Unknown date".to_string());

    let prompt = session
        .as_ref()
        .map(|s| s.prompt_template.clone())
        .unwrap_or_default();

    out.push_str(&format!("# Session: {}\n\n", title));
    out.push_str(&format!("**Date:** {}  \n", date));
    out.push_str(&format!("**Prompt template:** {}  \n\n", prompt));

    if !transcript.is_empty() {
        out.push_str("## Transcript\n\n");
        for entry in transcript {
            let label = if entry.channel == "system" {
                "[SYS]"
            } else {
                "[MIC]"
            };
            out.push_str(&format!(
                "`{}` **{}** {}\n\n",
                entry.spoken_at.format("%H:%M:%S"),
                label,
                entry.text
            ));
        }
    }

    if !exchanges.is_empty() {
        out.push_str("## AI Exchanges\n\n");
        for (i, (query, response, created_at)) in exchanges.iter().enumerate() {
            out.push_str(&format!("### Query {}\n\n", i + 1));
            out.push_str(&format!("_{}_ \n\n", created_at));
            out.push_str(&format!("**Q:** {}\n\n", query));
            out.push_str(&format!("**A:** {}\n\n", response));
            out.push_str("---\n\n");
        }
    }

    Ok(out)
}

fn export_json(
    session_id: i64,
    session: &Option<cue_core::session::Session>,
    transcript: &[cue_core::session::TranscriptEntry],
    exchanges: &[(String, String, String)],
) -> Result<String> {
    let json = serde_json::json!({
        "session_id": session_id,
        "title": session.as_ref().and_then(|s| s.title.clone()),
        "prompt_template": session.as_ref().map(|s| &s.prompt_template),
        "started_at": session.as_ref().map(|s| s.started_at.to_rfc3339()),
        "transcript": transcript.iter().map(|e| serde_json::json!({
            "channel": e.channel,
            "text": e.text,
            "spoken_at": e.spoken_at.to_rfc3339(),
        })).collect::<Vec<_>>(),
        "exchanges": exchanges.iter().map(|(q, r, ts)| serde_json::json!({
            "query": q,
            "response": r,
            "created_at": ts,
        })).collect::<Vec<_>>(),
    });

    Ok(serde_json::to_string_pretty(&json)?)
}
