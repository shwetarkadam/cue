use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::TtsConfig;

/// KittenTTS client — communicates with a long-running Python subprocess
/// via JSON-line protocol over stdin/stdout.
pub struct TtsEngine {
    config: TtsConfig,
    process: Mutex<Option<TtsChild>>,
}

struct TtsChild {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl TtsEngine {
    pub fn new(config: TtsConfig) -> Self {
        Self {
            config,
            process: Mutex::new(None),
        }
    }

    /// Ensure the Python TTS server is running. Returns Ok(()) if ready.
    pub fn ensure_started(&self) -> Result<()> {
        let mut guard = self.process.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }

        let script_path = self.resolve_script_path()?;
        info!(script = %script_path, model = %self.config.model, "Starting KittenTTS server");

        let mut child = Command::new(&self.config.python_bin)
            .arg(&script_path)
            .env("KITTEN_MODEL", &self.config.model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!(
                "Failed to start KittenTTS server (python: {}, script: {})",
                self.config.python_bin, script_path
            ))?;

        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);

        // Wait for the "ready" signal
        let mut line = String::new();
        reader.read_line(&mut line)
            .context("Failed to read KittenTTS startup response")?;

        let resp: serde_json::Value = serde_json::from_str(line.trim())
            .context("Invalid JSON from KittenTTS server")?;

        if resp.get("ok") != Some(&serde_json::Value::Bool(true)) {
            let err = resp.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error");
            bail!("KittenTTS failed to start: {err}");
        }

        info!("KittenTTS server ready");
        *guard = Some(TtsChild { child, reader });
        Ok(())
    }

    /// Generate speech from text. Returns the path to a WAV file.
    pub fn speak(&self, text: &str) -> Result<String> {
        self.ensure_started()?;

        let mut guard = self.process.lock().unwrap();
        let proc = guard.as_mut().unwrap();

        let request = serde_json::json!({
            "text": text,
            "voice": self.config.voice,
            "speed": self.config.speed,
        });

        let stdin = proc.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", request)
            .context("Failed to write to KittenTTS server")?;
        stdin.flush()?;

        // Read lines until we get valid JSON (skip any stray prints from Python libs)
        let resp: serde_json::Value = loop {
            let mut line = String::new();
            proc.reader.read_line(&mut line)
                .context("Failed to read KittenTTS response")?;
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                break v;
            }
            debug!(line = %trimmed, "Skipping non-JSON TTS output");
        };

        if resp.get("ok") == Some(&serde_json::Value::Bool(true)) {
            let path = resp.get("path")
                .and_then(|p| p.as_str())
                .context("Missing path in TTS response")?;
            debug!(path = %path, "TTS generated");
            Ok(path.to_string())
        } else {
            let err = resp.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error");
            bail!("TTS generation failed: {err}");
        }
    }

    /// Play a WAV file using the system's audio player.
    pub fn play_wav(path: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        let players: &[&str] = &["afplay"];

        #[cfg(target_os = "linux")]
        let players: &[&str] = &["pw-play", "paplay", "aplay"];

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let players: &[&str] = &["pw-play", "paplay", "aplay", "afplay"];

        for player in players {
            match Command::new(player).arg(path).status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        warn!("No audio player found (tried {:?})", players);
        Ok(())
    }

    /// Generate speech and play it. Non-blocking: spawns playback in background.
    pub fn speak_and_play(&self, text: &str) -> Result<()> {
        let path = self.speak(text)?;
        // Spawn playback in background thread so we don't block
        std::thread::spawn(move || {
            if let Err(e) = Self::play_wav(&path) {
                error!(error = %e, "TTS playback failed");
            }
            // Clean up temp file after playback
            let _ = std::fs::remove_file(&path);
        });
        Ok(())
    }

    fn resolve_script_path(&self) -> Result<String> {
        // Check custom path first
        if !self.config.script_path.is_empty() {
            return Ok(self.config.script_path.clone());
        }

        // Look for bundled script next to the binary
        if let Ok(exe) = std::env::current_exe() {
            let dir = exe.parent().unwrap_or(std::path::Path::new("."));
            let bundled = dir.join("kittentts_server.py");
            if bundled.exists() {
                return Ok(bundled.to_string_lossy().to_string());
            }
        }

        // Look in data dir
        let data_script = crate::config::Config::data_dir().join("scripts").join("kittentts_server.py");
        if data_script.exists() {
            return Ok(data_script.to_string_lossy().to_string());
        }

        bail!(
            "KittenTTS server script not found. Place kittentts_server.py in \
             ~/.local/share/cue/scripts/ or set tts.script_path in config."
        )
    }
}

impl Drop for TtsEngine {
    fn drop(&mut self) {
        if let Some(mut proc) = self.process.lock().unwrap().take() {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }
}
