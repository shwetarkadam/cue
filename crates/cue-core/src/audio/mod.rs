#[cfg(target_os = "linux")]
pub mod pipewire;
pub mod vad;

#[cfg(target_os = "linux")]
pub use pipewire::SystemAudioCapture;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{FftFixedIn, Resampler};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::AudioConfig;
use vad::VadState;

pub use vad::VadPhase;

/// Which channel (audio source) an utterance came from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// System audio (loopback / monitor)
    System,
    /// Microphone input
    Mic,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::System => write!(f, "system"),
            Channel::Mic => write!(f, "mic"),
        }
    }
}

/// A detected speech utterance
#[derive(Debug, Clone)]
pub struct Utterance {
    /// PCM samples at 16kHz, mono, f32
    pub samples: Vec<f32>,
    /// Duration of the utterance in milliseconds
    pub duration_ms: u32,
    /// Which channel this utterance came from
    pub channel: Channel,
}

/// Information about an audio device
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub channels: u16,
    pub sample_rate: u32,
}

/// Audio capture using cpal
///
/// For v0.1 we capture the default input device (microphone).
/// System audio loopback on Linux requires PipeWire's STREAM_CAPTURE_SINK
/// which will be added in v0.2.
///
/// TODO(v0.2): Add pipewire-rs direct capture for system audio loopback
pub struct AudioCapture {
    config: AudioConfig,
    tx: mpsc::Sender<Utterance>,
}

impl AudioCapture {
    pub fn new(config: AudioConfig, tx: mpsc::Sender<Utterance>) -> Self {
        Self { config, tx }
    }

    /// Start audio capture on a background OS thread (not tokio — cpal uses its own thread)
    /// Returns immediately; capture runs until the stream is dropped.
    pub fn start(&self) -> Result<cpal::Stream> {
        let host = cpal::default_host();

        let device = if self.config.mic_device == "auto" {
            host.default_input_device()
                .context("No default input device found")?
        } else {
            host.input_devices()?
                .find(|d| {
                    d.name().map(|n| n == self.config.mic_device).unwrap_or(false)
                })
                .with_context(|| format!("Input device '{}' not found", self.config.mic_device))?
        };

        let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
        info!(device = %device_name, "Starting audio capture");

        let supported_config = device
            .default_input_config()
            .context("Failed to get default input config")?;

        info!(
            sample_rate = supported_config.sample_rate().0,
            channels = supported_config.channels(),
            sample_format = ?supported_config.sample_format(),
            "Audio device config"
        );

        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;
        let tx = self.tx.clone();
        let vad_sensitivity = self.config.vad_sensitivity;
        let vad_silence_ms = self.config.vad_silence_timeout_ms;
        let vad_min_speech_ms = self.config.vad_min_speech_ms;

        // Target sample rate for whisper
        const TARGET_SAMPLE_RATE: u32 = 16000;

        // Build the cpal stream
        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::F32 => {
                build_input_stream_f32(
                    &device,
                    &supported_config.into(),
                    sample_rate,
                    channels,
                    tx,
                    vad_sensitivity,
                    vad_silence_ms,
                    vad_min_speech_ms,
                    TARGET_SAMPLE_RATE,
                )?
            }
            cpal::SampleFormat::I16 => {
                build_input_stream_i16(
                    &device,
                    &supported_config.into(),
                    sample_rate,
                    channels,
                    tx,
                    vad_sensitivity,
                    vad_silence_ms,
                    vad_min_speech_ms,
                    TARGET_SAMPLE_RATE,
                )?
            }
            cpal::SampleFormat::U16 => {
                build_input_stream_u16(
                    &device,
                    &supported_config.into(),
                    sample_rate,
                    channels,
                    tx,
                    vad_sensitivity,
                    vad_silence_ms,
                    vad_min_speech_ms,
                    TARGET_SAMPLE_RATE,
                )?
            }
            fmt => anyhow::bail!("Unsupported sample format: {:?}", fmt),
        };

        stream.play().context("Failed to start audio stream")?;
        info!("Audio capture started");
        Ok(stream)
    }

    /// List all available audio devices
    pub fn list_devices() -> Result<Vec<DeviceInfo>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();

        let default_input = host.default_input_device()
            .and_then(|d| d.name().ok());
        let default_output = host.default_output_device()
            .and_then(|d| d.name().ok());

        // Input devices
        if let Ok(inputs) = host.input_devices() {
            for device in inputs {
                let name = device.name().unwrap_or_else(|_| "unknown".to_string());
                let is_default_input = default_input.as_deref() == Some(&name);

                let (channels, sample_rate) = device
                    .default_input_config()
                    .map(|c| (c.channels(), c.sample_rate().0))
                    .unwrap_or((1, 16000));

                devices.push(DeviceInfo {
                    name,
                    is_default_input,
                    is_default_output: false,
                    channels,
                    sample_rate,
                });
            }
        }

        // Output devices
        if let Ok(outputs) = host.output_devices() {
            for device in outputs {
                let name = device.name().unwrap_or_else(|_| "unknown".to_string());
                let is_default_output = default_output.as_deref() == Some(&name);

                // Skip if already in list as input
                if devices.iter().any(|d| d.name == name) {
                    if let Some(d) = devices.iter_mut().find(|d| d.name == name) {
                        d.is_default_output = is_default_output;
                    }
                    continue;
                }

                let (channels, sample_rate) = device
                    .default_output_config()
                    .map(|c| (c.channels(), c.sample_rate().0))
                    .unwrap_or((2, 44100));

                devices.push(DeviceInfo {
                    name,
                    is_default_input: false,
                    is_default_output,
                    channels,
                    sample_rate,
                });
            }
        }

        Ok(devices)
    }
}

fn build_input_stream_f32(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: u32,
    channels: usize,
    tx: mpsc::Sender<Utterance>,
    vad_sensitivity: f32,
    vad_silence_ms: u32,
    vad_min_speech_ms: u32,
    target_sample_rate: u32,
) -> Result<cpal::Stream> {
    let mut state = AudioState::new(
        sample_rate,
        channels,
        tx,
        vad_sensitivity,
        vad_silence_ms,
        vad_min_speech_ms,
        target_sample_rate,
    )?;

    let stream = device.build_input_stream(
        config,
        move |data: &[f32], _: &_| {
            state.process_f32(data);
        },
        |err| error!(error = %err, "Audio stream error"),
        None,
    )?;

    Ok(stream)
}

fn build_input_stream_i16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: u32,
    channels: usize,
    tx: mpsc::Sender<Utterance>,
    vad_sensitivity: f32,
    vad_silence_ms: u32,
    vad_min_speech_ms: u32,
    target_sample_rate: u32,
) -> Result<cpal::Stream> {
    let mut state = AudioState::new(
        sample_rate,
        channels,
        tx,
        vad_sensitivity,
        vad_silence_ms,
        vad_min_speech_ms,
        target_sample_rate,
    )?;

    let stream = device.build_input_stream(
        config,
        move |data: &[i16], _: &_| {
            let f32_data: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
            state.process_f32(&f32_data);
        },
        |err| error!(error = %err, "Audio stream error"),
        None,
    )?;

    Ok(stream)
}

fn build_input_stream_u16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: u32,
    channels: usize,
    tx: mpsc::Sender<Utterance>,
    vad_sensitivity: f32,
    vad_silence_ms: u32,
    vad_min_speech_ms: u32,
    target_sample_rate: u32,
) -> Result<cpal::Stream> {
    let mut state = AudioState::new(
        sample_rate,
        channels,
        tx,
        vad_sensitivity,
        vad_silence_ms,
        vad_min_speech_ms,
        target_sample_rate,
    )?;

    let stream = device.build_input_stream(
        config,
        move |data: &[u16], _: &_| {
            let f32_data: Vec<f32> = data
                .iter()
                .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                .collect();
            state.process_f32(&f32_data);
        },
        |err| error!(error = %err, "Audio stream error"),
        None,
    )?;

    Ok(stream)
}

/// Per-stream state for audio processing
struct AudioState {
    channels: usize,
    tx: mpsc::Sender<Utterance>,
    vad: VadState,
    resampler: Option<FftFixedIn<f32>>,
    resample_input_buf: Vec<Vec<f32>>,
    #[allow(dead_code)]
    resample_output_buf: Vec<Vec<f32>>,
    #[allow(dead_code)]
    source_sample_rate: u32,
    target_sample_rate: u32,
    /// Chunk size in samples for resampler input
    chunk_size: usize,
    /// Leftover samples waiting for a full chunk
    pending: Vec<f32>,
}

impl AudioState {
    fn new(
        source_sample_rate: u32,
        channels: usize,
        tx: mpsc::Sender<Utterance>,
        vad_sensitivity: f32,
        vad_silence_ms: u32,
        vad_min_speech_ms: u32,
        target_sample_rate: u32,
    ) -> Result<Self> {
        // Use energy threshold based on sensitivity: higher sensitivity = lower threshold
        // sensitivity 0.0 → threshold 0.05, sensitivity 1.0 → threshold 0.001
        let energy_threshold = 0.02 + (1.0 - vad_sensitivity) * 0.08;

        let vad = VadState::new(
            energy_threshold,
            vad_min_speech_ms,
            vad_silence_ms,
            target_sample_rate,
        );

        let (resampler, chunk_size) = if source_sample_rate != target_sample_rate {
            // Process in chunks of ~10ms at source rate
            let chunk = (source_sample_rate / 100) as usize;
            let r = FftFixedIn::<f32>::new(
                source_sample_rate as usize,
                target_sample_rate as usize,
                chunk,
                2,
                1, // mono after downmix
            )
            .context("Failed to create resampler")?;
            debug!(
                source_rate = source_sample_rate,
                target_rate = target_sample_rate,
                chunk_size = chunk,
                "Resampler created"
            );
            (Some(r), chunk)
        } else {
            let chunk = (source_sample_rate / 100) as usize;
            (None, chunk)
        };

        let resample_output_buf = {
            let out_len = (chunk_size as f64 * target_sample_rate as f64 / source_sample_rate as f64) as usize + 16;
            vec![vec![0.0f32; out_len]; 1]
        };

        Ok(Self {
            channels,
            tx,
            vad,
            resampler,
            resample_input_buf: vec![vec![0.0f32; chunk_size]; 1],
            resample_output_buf,
            source_sample_rate,
            target_sample_rate,
            chunk_size,
            pending: Vec::new(),
        })
    }

    fn process_f32(&mut self, data: &[f32]) {
        // Downmix to mono
        let mono: Vec<f32> = if self.channels == 1 {
            data.to_vec()
        } else {
            data.chunks(self.channels)
                .map(|frame| frame.iter().sum::<f32>() / self.channels as f32)
                .collect()
        };

        // Resample to target rate if needed
        let resampled = if let Some(ref mut resampler) = self.resampler {
            self.pending.extend_from_slice(&mono);
            let mut out = Vec::new();

            while self.pending.len() >= self.chunk_size {
                let chunk: Vec<f32> = self.pending.drain(..self.chunk_size).collect();
                self.resample_input_buf[0].copy_from_slice(&chunk);

                match resampler.process(&self.resample_input_buf, None) {
                    Ok(resampled_chunk) => {
                        out.extend_from_slice(&resampled_chunk[0]);
                    }
                    Err(e) => {
                        warn!(error = %e, "Resampler error");
                    }
                }
            }
            out
        } else {
            mono
        };

        if resampled.is_empty() {
            return;
        }

        // Run VAD on ~10ms chunks at target rate
        let vad_chunk_size = (self.target_sample_rate / 100) as usize;
        for chunk in resampled.chunks(vad_chunk_size) {
            if let Some(utterance_samples) = self.vad.process(chunk) {
                let duration_ms = (utterance_samples.len() as u32 * 1000) / self.target_sample_rate;
                let utterance = Utterance {
                    samples: utterance_samples,
                    duration_ms,
                    channel: Channel::Mic,
                };
                debug!(duration_ms = duration_ms, "VAD: utterance detected");
                if let Err(_) = self.tx.try_send(utterance) {
                    warn!("Audio channel full, dropping utterance");
                }
            }
        }
    }
}
