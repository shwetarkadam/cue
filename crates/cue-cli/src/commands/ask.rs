use anyhow::Result;
use clap::Args;
use cue_core::{
    config::Config,
    context::ContextEngine,
    kb::KnowledgeBase,
    llm::{self, CompletionConfig},
    output::build_output,
    prompts::get_prompt,
    session::SessionStore,
};
use futures::StreamExt;
use std::sync::Arc;

#[derive(Args, Debug)]
pub struct AskArgs {
    /// The question to ask
    pub query: String,

    /// System prompt template
    #[arg(short, long, default_value = "general")]
    pub prompt: String,

    /// Output mode: stdout, json
    #[arg(long)]
    pub output: Option<String>,

    /// Session ID to use for transcript context (defaults to latest)
    #[arg(long)]
    pub session: Option<i64>,
}

pub async fn run(args: AskArgs) -> Result<()> {
    let config = Config::load()?;
    Config::ensure_dirs()?;

    let output_mode = args.output.as_deref().unwrap_or(&config.display.mode);
    let output = build_output(output_mode);

    let db_path = Config::db_path();
    let session_store = Arc::new(SessionStore::new(&db_path)?);
    session_store.init_schema()?;

    let kb = Arc::new(KnowledgeBase::new(&db_path)?);
    kb.init_schema()?;

    // Get transcript context from session
    let session_id = if let Some(id) = args.session {
        id
    } else {
        // Use the latest session, or create a new one
        let sessions = session_store.list_sessions()?;
        if let Some(s) = sessions.first() {
            s.id
        } else {
            session_store.create_session(&args.prompt)?.id
        }
    };

    let transcript = session_store.get_recent_transcript(
        session_id,
        config.rag.transcript_window_secs,
    )?;

    let context_engine = ContextEngine::new(Arc::clone(&kb), config.rag.clone());
    let system_prompt = get_prompt(&args.prompt);

    let messages = context_engine
        .build_prompt(&args.query, &transcript, system_prompt)
        .await?;

    let router = llm::build_router(&config.provider)?;
    let completion_config = CompletionConfig {
        temperature: config.provider.temperature,
        max_tokens: config.provider.max_tokens,
        model: config.provider.model.clone(),
    };

    let mut stream = router.stream(&messages, &completion_config).await?;

    let mut full_response = String::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(token) => {
                output.emit_response_token(&token).await;
                full_response.push_str(&token);
            }
            Err(e) => {
                output.emit_error(&format!("Stream error: {}", e)).await;
                break;
            }
        }
    }
    output.emit_response_done().await;

    // Persist exchange
    let _ = session_store.add_exchange(
        session_id,
        &args.query,
        &full_response,
        &config.provider.default,
        &config.provider.model,
    );

    Ok(())
}
