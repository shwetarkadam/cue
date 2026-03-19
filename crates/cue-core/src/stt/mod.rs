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

        let params = whisper_rs::WhisperContextParameters::default();
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

        let text = result.trim().to_string();
        debug!(segments = num_segments, text_len = text.len(), "Transcription complete");
        Ok(text)
    }

    /// Get the expected model file path
    pub fn model_path(model_name: &str, models_dir: &Path) -> PathBuf {
        models_dir.join(format!("ggml-{}.bin", model_name))
    }
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

    let mut bytes = response.bytes().await.context("Failed to read response body")?;
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
