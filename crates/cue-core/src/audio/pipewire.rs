//! System audio capture via pw-record subprocess.
//!
//! pw-record is part of pipewire-utils on all modern Linux systems.
//! This captures from the default monitor (loopback) source, giving us
//! all system audio without needing pipewire-rs FFI bindings.

use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Command, Stdio};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::audio::{Channel, Utterance};
use crate::audio::vad::VadState;

pub struct SystemAudioCapture {
    tx: mpsc::Sender<Utterance>,
    monitor_device: Option<String>,
    vad_sensitivity: f32,
    vad_silence_ms: u32,
    vad_min_speech_ms: u32,
}

impl SystemAudioCapture {
    pub fn new(
        tx: mpsc::Sender<Utterance>,
        monitor_device: Option<String>,
        vad_sensitivity: f32,
        vad_silence_ms: u32,
        vad_min_speech_ms: u32,
    ) -> Self {
        Self {
            tx,
            monitor_device,
            vad_sensitivity,
            vad_silence_ms,
            vad_min_speech_ms,
        }
    }

    /// Check if pw-record is available on this system.
    pub fn is_available() -> bool {
        Command::new("pw-record")
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success() || true) // pw-record --help returns non-zero but exits
            .unwrap_or(false)
    }

    /// Find the default monitor source device name via pactl.
    pub fn find_monitor_device() -> Option<String> {
        let output = Command::new("pactl")
            .args(["list", "sources"])
            .output()
            .ok()?;

        let text = String::from_utf8_lossy(&output.stdout);

        // Find first monitor source (name ends with .monitor)
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Name:") && trimmed.contains(".monitor") {
                let name = trimmed.splitn(2, ':').nth(1)?.trim().to_string();
                return Some(name);
            }
        }
        None
    }

    /// Start capturing system audio in a background OS thread.
    pub fn start(self) -> Result<()> {
        let monitor = self
            .monitor_device
            .clone()
            .or_else(Self::find_monitor_device);

        if monitor.is_none() {
            warn!("No monitor source found; system audio capture may use default device");
        }

        let tx = self.tx;
        let vad_sensitivity = self.vad_sensitivity;
        let vad_silence_ms = self.vad_silence_ms;
        let vad_min_speech_ms = self.vad_min_speech_ms;

        std::thread::spawn(move || {
            if let Err(e) = capture_loop(tx, monitor, vad_sensitivity, vad_silence_ms, vad_min_speech_ms) {
                error!("System audio capture failed: {}", e);
            }
        });

        Ok(())
    }
}

fn capture_loop(
    tx: mpsc::Sender<Utterance>,
    monitor_device: Option<String>,
    vad_sensitivity: f32,
    vad_silence_ms: u32,
    vad_min_speech_ms: u32,
) -> Result<()> {
    let mut cmd = Command::new("pw-record");

    cmd.args([
        "--rate", "16000",
        "--channels", "1",
        "--format", "f32le",
    ]);

    if let Some(ref device) = monitor_device {
        cmd.args(["--target", device]);
        info!(device = %device, "Capturing system audio from monitor device");
    } else {
        info!("Capturing system audio from default monitor");
    }

    // pw-record writes raw PCM to the specified file; "-" = stdout
    cmd.arg("-");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let mut child = cmd
        .spawn()
        .context("Failed to start pw-record. Is pipewire-utils installed? Try: sudo apt install pipewire-audio-client-libraries")?;

    let stdout = child
        .stdout
        .take()
        .context("Failed to get pw-record stdout")?;

    let mut reader = std::io::BufReader::new(stdout);

    // Energy threshold from sensitivity: sensitivity 0.0 → 0.05, 1.0 → ~0.0025
    let energy_threshold = 0.05 * (1.0 - vad_sensitivity * 0.95);
    let mut vad = VadState::new(energy_threshold, vad_min_speech_ms, vad_silence_ms, 16000);

    const CHUNK_SAMPLES: usize = 1600; // 100ms at 16kHz
    const BYTE_SIZE: usize = CHUNK_SAMPLES * 4; // f32le = 4 bytes per sample

    let mut buf = vec![0u8; BYTE_SIZE];
    let mut float_buf = vec![0f32; CHUNK_SAMPLES];

    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                // Convert bytes → f32 (little-endian)
                for (i, chunk) in buf.chunks_exact(4).enumerate() {
                    float_buf[i] =
                        f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }

                if let Some(utterance_samples) = vad.process(&float_buf) {
                    let duration_ms =
                        (utterance_samples.len() as u32 * 1000) / 16000;
                    let utterance = Utterance {
                        samples: utterance_samples,
                        duration_ms,
                        channel: Channel::System,
                    };
                    if tx.blocking_send(utterance).is_err() {
                        break; // Receiver dropped
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                error!("pw-record read error: {}", e);
                break;
            }
        }
    }

    // Clean up child process
    let _ = child.wait();
    Ok(())
}
