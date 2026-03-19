use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum CueError {
    #[error("Audio device not found: {0}")]
    AudioDevice(String),

    #[error("STT model not found at {0}")]
    ModelNotFound(PathBuf),

    #[error("All LLM providers failed")]
    AllProvidersFailed,

    #[error("Knowledge base error: {0}")]
    KbError(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CueError>;
