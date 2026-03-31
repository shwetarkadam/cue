use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::audio::Utterance;
use crate::session::TranscriptEntry;
use super::SttEvent;

/// Default model directory (shared with Handy app)
const DEFAULT_MODEL_DIR: &str = ".local/share/com.pais.handy/models/parakeet-tdt-0.6b-v3-int8";

/// Blank token index (vocab_size = 8193, blank is the next index)
const BLANK_IDX: usize = 8193;
/// Number of duration buckets in TDT output
const NUM_DURATIONS: usize = 4;
/// LSTM hidden dimension
const LSTM_HIDDEN: usize = 640;

/// Convert ort::Error (which is !Send) to anyhow::Error via Display
fn ort_err<T>(r: ort::Result<T>) -> Result<T> {
    r.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Local Parakeet TDT v3 speech-to-text engine using ONNX Runtime.
pub struct ParakeetEngine {
    preprocessor: Session,
    encoder: Session,
    decoder_joint: Session,
    vocab: Vec<String>,
}

impl ParakeetEngine {
    pub fn new(model_dir: Option<&Path>) -> Result<Self> {
        let dir = match model_dir {
            Some(d) => d.to_path_buf(),
            None => Self::default_model_dir()?,
        };

        info!(dir = %dir.display(), "Loading Parakeet TDT model");

        let preprocessor_path = dir.join("nemo128.onnx");
        let encoder_path = dir.join("encoder-model.int8.onnx");
        let decoder_path = dir.join("decoder_joint-model.int8.onnx");
        let vocab_path = dir.join("vocab.txt");

        for p in [&preprocessor_path, &encoder_path, &decoder_path, &vocab_path] {
            if !p.exists() {
                anyhow::bail!("Missing model file: {}", p.display());
            }
        }

        let preprocessor = load_session(&preprocessor_path, 2)
            .context("Failed to load preprocessor")?;
        let encoder = load_session(&encoder_path, 4)
            .context("Failed to load encoder")?;
        let decoder_joint = load_session(&decoder_path, 2)
            .context("Failed to load decoder")?;

        let vocab_text = std::fs::read_to_string(&vocab_path)
            .context("Failed to read vocab.txt")?;
        let vocab: Vec<String> = vocab_text
            .lines()
            .map(|line| line.split_whitespace().next().unwrap_or("").to_string())
            .collect();

        info!(vocab_size = vocab.len(), "Parakeet model loaded");
        Ok(Self { preprocessor, encoder, decoder_joint, vocab })
    }

    fn default_model_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("No home directory")?;
        let dir = home.join(DEFAULT_MODEL_DIR);
        if dir.exists() {
            Ok(dir)
        } else {
            anyhow::bail!(
                "Parakeet model not found at {}. Install via Handy app or download manually.",
                dir.display()
            )
        }
    }

    /// Transcribe 16kHz mono f32 PCM audio to text.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        let (features, feat_len) = self.run_preprocessor(samples)?;
        let (encoded, enc_len) = self.run_encoder(&features, feat_len)?;
        let tokens = self.greedy_decode(&encoded, enc_len)?;
        let text = self.tokens_to_text(&tokens);

        debug!(text_len = text.len(), tokens = tokens.len(), "Parakeet transcription complete");
        Ok(text)
    }

    fn run_preprocessor(&mut self, samples: &[f32]) -> Result<(Vec<f32>, i64)> {
        let n = samples.len();
        let waveforms = ort_err(Tensor::from_array((vec![1i64, n as i64], samples.to_vec())))?;
        let waveforms_lens = ort_err(Tensor::from_array((vec![1i64], vec![n as i64])))?;

        let outputs = ort_err(self.preprocessor.run(ort::inputs![
            "waveforms" => waveforms,
            "waveforms_lens" => waveforms_lens,
        ]))?;

        let (_shape, features_data) = ort_err(outputs["features"].try_extract_tensor::<f32>())?;
        let (_shape, lens_data) = ort_err(outputs["features_lens"].try_extract_tensor::<i64>())?;

        Ok((features_data.to_vec(), lens_data[0]))
    }

    fn run_encoder(&mut self, features: &[f32], feat_len: i64) -> Result<(Vec<f32>, i64)> {
        let t = feat_len as usize;
        let expected_len = 128 * t;
        let feat_slice = if features.len() >= expected_len {
            &features[..expected_len]
        } else {
            features
        };

        let audio_signal = ort_err(Tensor::from_array((vec![1i64, 128, t as i64], feat_slice.to_vec())))?;
        let length = ort_err(Tensor::from_array((vec![1i64], vec![feat_len])))?;

        let outputs = ort_err(self.encoder.run(ort::inputs![
            "audio_signal" => audio_signal,
            "length" => length,
        ]))?;

        let (_shape, encoded_data) = ort_err(outputs["outputs"].try_extract_tensor::<f32>())?;
        let (_shape, lens_data) = ort_err(outputs["encoded_lengths"].try_extract_tensor::<i64>())?;

        Ok((encoded_data.to_vec(), lens_data[0]))
    }

    fn greedy_decode(&mut self, encoded: &[f32], enc_len: i64) -> Result<Vec<usize>> {
        let enc_len = enc_len as usize;
        let enc_dim = 1024;

        let mut states1 = vec![0.0f32; 2 * LSTM_HIDDEN];
        let mut states2 = vec![0.0f32; 2 * LSTM_HIDDEN];

        let mut tokens = Vec::new();
        let mut t = 0usize;
        let mut last_token = 0i32; // SOS/pad — blank is output-only, not a valid embedding index
        let max_steps = enc_len * 10;
        let mut step = 0;

        while t < enc_len && step < max_steps {
            step += 1;

            let frame_start = t * enc_dim;
            let frame_end = frame_start + enc_dim;
            if frame_end > encoded.len() {
                break;
            }
            let enc_frame: Vec<f32> = encoded[frame_start..frame_end].to_vec();

            let encoder_out = ort_err(Tensor::from_array((vec![1i64, enc_dim as i64, 1i64], enc_frame)))?;
            let targets = ort_err(Tensor::from_array((vec![1i64, 1i64], vec![last_token])))?;
            let target_length = ort_err(Tensor::from_array((vec![1i64], vec![1i32])))?;
            let in_states1 = ort_err(Tensor::from_array((vec![2i64, 1, LSTM_HIDDEN as i64], states1.clone())))?;
            let in_states2 = ort_err(Tensor::from_array((vec![2i64, 1, LSTM_HIDDEN as i64], states2.clone())))?;

            let outputs = ort_err(self.decoder_joint.run(ort::inputs![
                "encoder_outputs" => encoder_out,
                "targets" => targets,
                "target_length" => target_length,
                "input_states_1" => in_states1,
                "input_states_2" => in_states2,
            ]))?;

            let (_shape, logits) = ort_err(outputs["outputs"].try_extract_tensor::<f32>())?;

            if logits.len() < BLANK_IDX + 1 {
                t += 1;
                continue;
            }

            let token_logits = &logits[..BLANK_IDX + 1];
            let token_id = argmax(token_logits);

            if token_id == BLANK_IDX {
                t += 1;
            } else {
                tokens.push(token_id);
                last_token = token_id as i32;

                let dur_start = BLANK_IDX + 1;
                let dur_end = dur_start + NUM_DURATIONS;
                if dur_end <= logits.len() {
                    let duration = argmax(&logits[dur_start..dur_end]) + 1;
                    t += duration;
                } else {
                    t += 1;
                }
            }

            // Update LSTM states
            let (_shape, s1) = ort_err(outputs["output_states_1"].try_extract_tensor::<f32>())?;
            states1 = s1.to_vec();
            let (_shape, s2) = ort_err(outputs["output_states_2"].try_extract_tensor::<f32>())?;
            states2 = s2.to_vec();
        }

        Ok(tokens)
    }

    fn tokens_to_text(&self, tokens: &[usize]) -> String {
        let mut text = String::new();
        for &id in tokens {
            if id < self.vocab.len() {
                let token = &self.vocab[id];
                if token.starts_with('<') && token.ends_with('>') {
                    continue;
                }
                // SentencePiece: ▁ (U+2581) = word boundary (space)
                text.push_str(&token.replace('\u{2581}', " "));
            }
        }
        text.trim().to_string()
    }
}

/// Streaming wrapper: receives audio utterances, transcribes locally, emits STT events.
pub struct ParakeetStreamer {
    model_dir: Option<PathBuf>,
}

impl ParakeetStreamer {
    pub fn new(model_dir: Option<&Path>) -> Result<Self> {
        let dir = match model_dir {
            Some(d) => {
                if !d.exists() {
                    anyhow::bail!("Model directory not found: {}", d.display());
                }
                Some(d.to_path_buf())
            }
            None => {
                let home = dirs::home_dir().context("No home directory")?;
                let default = home.join(DEFAULT_MODEL_DIR);
                if !default.exists() {
                    anyhow::bail!("Parakeet model not found at {}", default.display());
                }
                None
            }
        };
        Ok(Self { model_dir: dir })
    }

    pub async fn run(
        self,
        mut utterance_rx: mpsc::Receiver<Utterance>,
        transcript_tx: mpsc::Sender<TranscriptEntry>,
        stt_tx: mpsc::Sender<SttEvent>,
        session_id: i64,
    ) -> Result<()> {
        let model_dir = self.model_dir.clone();
        let mut engine = tokio::task::spawn_blocking(move || {
            ParakeetEngine::new(model_dir.as_deref())
        }).await??;

        info!("Parakeet streaming engine ready");

        while let Some(u) = utterance_rx.recv().await {
            let samples = u.samples;
            if samples.len() < 1600 {
                continue; // skip < 100ms
            }

            // Show interim indicator
            let _ = stt_tx.send(SttEvent {
                text: "...".to_string(),
                is_final: false,
            }).await;

            match engine.transcribe(&samples) {
                Ok(text) if !text.is_empty() => {
                    debug!(text = %text, "Parakeet transcription");
                    let _ = stt_tx.send(SttEvent {
                        text: text.clone(),
                        is_final: true,
                    }).await;
                    let _ = transcript_tx.send(TranscriptEntry {
                        id: None,
                        session_id,
                        channel: "mic".to_string(),
                        text,
                        spoken_at: chrono::Utc::now(),
                    }).await;
                }
                Ok(_) => {
                    let _ = stt_tx.send(SttEvent {
                        text: String::new(),
                        is_final: true,
                    }).await;
                }
                Err(e) => error!("Parakeet: {e}"),
            }
        }

        info!("Parakeet: audio source closed");
        Ok(())
    }
}

fn load_session(path: &Path, threads: usize) -> Result<Session> {
    let builder = Session::builder()
        .map_err(|e| anyhow::anyhow!("Session builder: {e}"))?;
    let mut builder = builder.with_intra_threads(threads)
        .map_err(|e| anyhow::anyhow!("Set threads: {e}"))?;
    builder.commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("Load model {}: {e}", path.display()))
}

fn argmax(slice: &[f32]) -> usize {
    slice
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0)
}
