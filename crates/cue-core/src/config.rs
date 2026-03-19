use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub stt: SttConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub rag: RagConfig,
    #[serde(default)]
    pub trigger: TriggerConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub stealth: StealthConfig,
    #[serde(default)]
    pub response: ResponseConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "default_provider")]
    pub default: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub failover: Vec<String>,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// API key — loaded from env var, not serialized to disk
    #[serde(skip)]
    pub api_key: Option<String>,
    /// OpenAI-compatible base URL (for OpenAI, Groq, Mistral, OpenRouter)
    #[serde(default = "default_openai_base_url")]
    pub openai_base_url: String,
    /// Ollama endpoint
    #[serde(default = "default_ollama_endpoint")]
    pub ollama_endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    #[serde(default = "default_stt_model")]
    pub model: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_threads")]
    pub threads: u32,
    #[serde(default = "default_beam_size")]
    pub beam_size: u32,
    /// STT backend: "whisper" (default) or "deepgram"
    #[serde(default = "default_stt_backend")]
    pub stt_backend: String,
    /// Deepgram API key — loaded from env DEEPGRAM_API_KEY, not serialized to disk
    #[serde(skip)]
    pub deepgram_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_auto")]
    pub system_device: String,
    #[serde(default = "default_auto")]
    pub mic_device: String,
    #[serde(default = "default_vad_sensitivity")]
    pub vad_sensitivity: f32,
    #[serde(default = "default_vad_silence_timeout_ms")]
    pub vad_silence_timeout_ms: u32,
    #[serde(default = "default_vad_min_speech_ms")]
    pub vad_min_speech_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_transcript_window_secs")]
    pub transcript_window_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    #[serde(default = "default_trigger_mode")]
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default = "default_display_mode")]
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StealthConfig {
    #[serde(default)]
    pub camouflage_enabled: bool,
    #[serde(default = "default_camouflage_name")]
    pub camouflage_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseConfig {
    #[serde(default = "default_response_style")]
    pub style: String,
}

// Default functions
fn default_provider() -> String { "anthropic".to_string() }
fn default_model() -> String { "claude-sonnet-4-20250514".to_string() }
fn default_temperature() -> f32 { 0.3 }
fn default_max_tokens() -> u32 { 512 }
fn default_openai_base_url() -> String { "https://api.openai.com/v1".to_string() }
fn default_ollama_endpoint() -> String { "http://localhost:11434".to_string() }
fn default_stt_model() -> String { "tiny".to_string() }
fn default_language() -> String { "en".to_string() }
fn default_threads() -> u32 { 4 }
fn default_beam_size() -> u32 { 1 }
fn default_stt_backend() -> String { "deepgram".to_string() }
fn default_backend() -> String { "cpal".to_string() }
fn default_auto() -> String { "auto".to_string() }
fn default_vad_sensitivity() -> f32 { 0.5 }
fn default_vad_silence_timeout_ms() -> u32 { 1000 }
fn default_vad_min_speech_ms() -> u32 { 250 }
fn default_true() -> bool { true }
fn default_top_k() -> usize { 5 }
fn default_chunk_size() -> usize { 512 }
fn default_chunk_overlap() -> usize { 64 }
fn default_transcript_window_secs() -> u64 { 120 }
fn default_trigger_mode() -> String { "manual".to_string() }
fn default_display_mode() -> String { "stdout".to_string() }
fn default_camouflage_name() -> String { "pipewire-pulse".to_string() }
fn default_response_style() -> String { "concise".to_string() }

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default: default_provider(),
            model: default_model(),
            failover: vec![],
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            api_key: None,
            openai_base_url: default_openai_base_url(),
            ollama_endpoint: default_ollama_endpoint(),
        }
    }
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            model: default_stt_model(),
            language: default_language(),
            threads: default_threads(),
            beam_size: default_beam_size(),
            stt_backend: default_stt_backend(),
            deepgram_api_key: None,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            system_device: default_auto(),
            mic_device: default_auto(),
            vad_sensitivity: default_vad_sensitivity(),
            vad_silence_timeout_ms: default_vad_silence_timeout_ms(),
            vad_min_speech_ms: default_vad_min_speech_ms(),
        }
    }
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: default_top_k(),
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
            transcript_window_secs: default_transcript_window_secs(),
        }
    }
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self { mode: default_trigger_mode() }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { mode: default_display_mode() }
    }
}

impl Default for StealthConfig {
    fn default() -> Self {
        Self {
            camouflage_enabled: false,
            camouflage_name: default_camouflage_name(),
        }
    }
}

impl Default for ResponseConfig {
    fn default() -> Self {
        Self { style: default_response_style() }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            stt: SttConfig::default(),
            audio: AudioConfig::default(),
            rag: RagConfig::default(),
            trigger: TriggerConfig::default(),
            display: DisplayConfig::default(),
            stealth: StealthConfig::default(),
            response: ResponseConfig::default(),
        }
    }
}

impl Config {
    /// Load config from file + env vars. All fields have defaults so this always succeeds.
    pub fn load() -> Result<Self> {
        // Load .env file if present
        let env_path = Self::config_dir().join(".env");
        if env_path.exists() {
            dotenvy::from_path(&env_path).ok();
        }
        // Also try current directory
        dotenvy::dotenv().ok();

        // Start with defaults
        let mut config = Config::default();

        // Try to load TOML config
        let config_path = Self::config_dir().join("cue.toml");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
            config = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;
        }

        // Override with environment variables
        if let Ok(val) = std::env::var("CUE_PROVIDER") {
            config.provider.default = val;
        }
        if let Ok(val) = std::env::var("CUE_MODEL") {
            config.provider.model = val;
        }
        if let Ok(val) = std::env::var("CUE_TEMPERATURE") {
            if let Ok(f) = val.parse::<f32>() {
                config.provider.temperature = f;
            }
        }
        if let Ok(val) = std::env::var("CUE_MAX_TOKENS") {
            if let Ok(n) = val.parse::<u32>() {
                config.provider.max_tokens = n;
            }
        }

        // API keys — resolve based on provider
        let api_key = match config.provider.default.as_str() {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            "groq" => std::env::var("GROQ_API_KEY").ok(),
            "mistral" => std::env::var("MISTRAL_API_KEY").ok(),
            "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
            _ => None,
        };
        config.provider.api_key = api_key;

        // Deepgram API key
        config.stt.deepgram_api_key = std::env::var("DEEPGRAM_API_KEY").ok();

        // STT backend from env
        if let Ok(val) = std::env::var("CUE_STT_BACKEND") {
            config.stt.stt_backend = val;
        }

        // Display mode from env
        if let Ok(val) = std::env::var("CUE_DISPLAY") {
            config.display.mode = val;
        }

        // Trigger mode from env
        if let Ok(val) = std::env::var("CUE_TRIGGER") {
            config.trigger.mode = val;
        }

        Ok(config)
    }

    /// XDG data directory: ~/.local/share/cue
    pub fn data_dir() -> PathBuf {
        let base = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local/share")
            });
        base.join("cue")
    }

    /// XDG config directory: ~/.config/cue
    pub fn config_dir() -> PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            });
        base.join("cue")
    }

    /// Whisper model directory: ~/.local/share/cue/models
    pub fn models_dir() -> PathBuf {
        Self::data_dir().join("models")
    }

    /// SQLite database path: ~/.local/share/cue/cue.db
    pub fn db_path() -> PathBuf {
        Self::data_dir().join("cue.db")
    }

    /// Ensure all required directories exist
    pub fn ensure_dirs() -> Result<()> {
        std::fs::create_dir_all(Self::data_dir())?;
        std::fs::create_dir_all(Self::config_dir())?;
        std::fs::create_dir_all(Self::models_dir())?;
        Ok(())
    }

    /// Serialize config back to TOML string
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("Failed to serialize config")
    }
}
