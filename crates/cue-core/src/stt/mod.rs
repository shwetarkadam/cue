pub mod deepgram;
pub use deepgram::DeepgramStreamer;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::config::SttConfig;

/// The whisper STT engine, wrapping whisper-rs
pub struct SttEngine {
    ctx: whisper_rs::WhisperContext,
    config: SttConfig,
}

impl SttEngine {
    /// Create a new STT engine. The model file must exist at models_dir/ggml-{name}.bin.
    pub fn new(config: &SttConfig, models_dir: &Path) -> Result<Self> {
        let model_path = Self::model_path(&config.model, models_dir);

        if !model_path.exists() {
            return Err(anyhow::anyhow!(
                crate::error::CueError::ModelNotFound(model_path)
            ));
        }

        info!(model = %model_path.display(), "Loading Whisper model");

        let mut params = whisper_rs::WhisperContextParameters::default();
        params.use_gpu(false); // suppress GPU detection noise
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path.to_str().context("Invalid model path")?,
            params,
        )
        .context("Failed to load Whisper model")?;

        info!("Whisper model loaded");
        Ok(Self {
            ctx,
            config: config.clone(),
        })
    }

    /// Transcribe PCM samples (f32, 16kHz, mono) to text.
    /// This is CPU-bound — call from spawn_blocking.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy {
            best_of: 1,
        });

        params.set_language(Some(&self.config.language));
        params.set_n_threads(self.config.threads as i32);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_single_segment(false);
        params.set_no_context(true);

        let mut state = self.ctx.create_state().context("Failed to create Whisper state")?;

        state
            .full(params, samples)
            .context("Whisper transcription failed")?;

        let num_segments = state.full_n_segments().context("Failed to get segment count")?;
        let mut result = String::new();

        for i in 0..num_segments {
            let segment = state
                .full_get_segment_text(i)
                .context("Failed to get segment text")?;
            result.push_str(segment.trim());
            result.push(' ');
        }

        let text = filter_special_tokens(result.trim());
        debug!(segments = num_segments, text_len = text.len(), "Transcription complete");
        Ok(text)
    }

    /// Get the expected model file path
    pub fn model_path(model_name: &str, models_dir: &Path) -> PathBuf {
        models_dir.join(format!("ggml-{}.bin", model_name))
    }
}

/// Filter out whisper special tokens like [Bell], [Music], [_BEL_], (Bell) etc.
/// These appear when whisper hallucinates on silence or background noise.
fn filter_special_tokens(text: &str) -> String {
    // Whisper special tokens that indicate noise/silence, not real speech
    let noise_tokens = [
        "[Bell]", "[Music]", "[Applause]", "[Laughter]", "[Noise]",
        "[_BEL_]", "[_TT_]", "[_MUSIC_]", "[_NOISE_]",
        "(Bell)", "(Music)", "(Applause)", "(Laughter)", "(Noise)",
        "[silence]", "[BLANK_AUDIO]",
    ];

    let mut out = text.to_string();
    for token in &noise_tokens {
        out = out.replace(token, "");
    }

    // Also strip anything matching pattern [Xxx] or (Xxx) where Xxx is Title Case (special token)
    let result = regex_filter_brackets(&out);
    result.trim().to_string()
}

fn regex_filter_brackets(text: &str) -> String {
    // Simple bracket filter without regex dependency:
    // Remove [Word] and (Word) patterns where content starts with uppercase (whisper special tokens)
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let open = chars[i];
        if open == '[' || open == '(' {
            let close = if open == '[' { ']' } else { ')' };
            if let Some(end) = chars[i+1..].iter().position(|&c| c == close) {
                let inner: String = chars[i+1..i+1+end].iter().collect();
                // Skip if inner starts with uppercase (special token) or is all caps
                let first = inner.chars().next().unwrap_or(' ');
                if first.is_uppercase() || inner.chars().all(|c| c.is_uppercase() || c == '_') {
                    i += end + 2; // skip the whole [Token]
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Download a whisper model from HuggingFace
pub async fn download_model(model_name: &str, models_dir: &Path) -> Result<()> {
    let model_path = SttEngine::model_path(model_name, models_dir);

    if model_path.exists() {
        info!(model = %model_path.display(), "Model already exists, skipping download");
        return Ok(());
    }

    std::fs::create_dir_all(models_dir).context("Failed to create models directory")?;

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
        model_name
    );

    info!(url = %url, dest = %model_path.display(), "Downloading Whisper model");

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to start download")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Download failed with status {}: {}",
            response.status(),
            url
        );
    }

    let total_size = response.content_length();

    // Set up progress bar
    let pb = if let Some(total) = total_size {
        let pb = indicatif::ProgressBar::new(total);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );
        Some(pb)
    } else {
        None
    };

    let bytes = response.bytes().await.context("Failed to read response body")?;
    if let Some(ref pb) = pb {
        pb.finish_with_message("Download complete");
    }

    std::fs::write(&model_path, &bytes)
        .with_context(|| format!("Failed to write model to {}", model_path.display()))?;

    info!(model = %model_path.display(), "Model downloaded successfully");
    Ok(())
}

/// List downloaded models
pub fn list_models(models_dir: &Path) -> Result<Vec<String>> {
    if !models_dir.exists() {
        return Ok(vec![]);
    }

    let mut models = Vec::new();
    for entry in std::fs::read_dir(models_dir).context("Failed to read models directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("bin") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                // Strip "ggml-" prefix
                let display_name = name.strip_prefix("ggml-").unwrap_or(name);
                models.push(display_name.to_string());
            }
        }
    }

    models.sort();
    Ok(models)
}
