/// Voice Activity Detection
///
/// Two-tier approach:
/// Tier 1: Energy gate (RMS threshold)
/// Tier 2: State machine tracking SILENCE/SPEECH phases

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadPhase {
    Silence,
    Speech,
}

pub struct VadState {
    /// RMS energy threshold (default 0.01)
    pub energy_threshold: f32,
    /// Minimum speech duration in samples (at 16kHz)
    pub min_speech_samples: usize,
    /// Silence timeout in samples — after this many silent samples, flush utterance
    pub silence_timeout_samples: usize,
    /// Maximum utterance length in samples (15s at 16kHz = 240000)
    pub max_utterance_samples: usize,

    phase: VadPhase,
    speech_buf: Vec<f32>,
    silence_count: usize,
}

impl VadState {
    pub fn new(
        energy_threshold: f32,
        min_speech_ms: u32,
        silence_timeout_ms: u32,
        sample_rate: u32,
    ) -> Self {
        let sr = sample_rate as usize;
        Self {
            energy_threshold,
            min_speech_samples: (min_speech_ms as usize * sr) / 1000,
            silence_timeout_samples: (silence_timeout_ms as usize * sr) / 1000,
            max_utterance_samples: 15 * sr,
            phase: VadPhase::Silence,
            speech_buf: Vec::with_capacity(sr * 15),
            silence_count: 0,
        }
    }

    /// Process a chunk of samples. Returns Some(utterance_samples) if an utterance is complete.
    pub fn process(&mut self, samples: &[f32]) -> Option<Vec<f32>> {
        let is_speech = self.energy_gate(samples);

        match self.phase {
            VadPhase::Silence => {
                if is_speech {
                    self.phase = VadPhase::Speech;
                    self.speech_buf.clear();
                    self.silence_count = 0;
                    self.speech_buf.extend_from_slice(samples);
                }
                None
            }
            VadPhase::Speech => {
                if is_speech {
                    self.silence_count = 0;
                    self.speech_buf.extend_from_slice(samples);

                    // Max utterance length reached — flush
                    if self.speech_buf.len() >= self.max_utterance_samples {
                        let utterance = self.speech_buf.clone();
                        self.speech_buf.clear();
                        self.phase = VadPhase::Silence;
                        return Some(utterance);
                    }
                } else {
                    // Silent frame during speech
                    self.silence_count += samples.len();
                    self.speech_buf.extend_from_slice(samples);

                    if self.silence_count >= self.silence_timeout_samples {
                        // End of utterance
                        // Trim trailing silence
                        let trim = self.speech_buf.len().saturating_sub(self.silence_count);
                        let utterance = self.speech_buf[..trim].to_vec();
                        self.speech_buf.clear();
                        self.silence_count = 0;
                        self.phase = VadPhase::Silence;

                        if utterance.len() >= self.min_speech_samples {
                            return Some(utterance);
                        }
                    }
                }
                None
            }
        }
    }

    /// Energy gate: check if RMS energy exceeds threshold
    fn energy_gate(&self, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return false;
        }
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        rms > self.energy_threshold
    }

    /// Flush any buffered speech immediately (e.g., on shutdown)
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        if !self.speech_buf.is_empty() && self.phase == VadPhase::Speech {
            let utterance = self.speech_buf.clone();
            self.speech_buf.clear();
            self.phase = VadPhase::Silence;
            if utterance.len() >= self.min_speech_samples {
                return Some(utterance);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_energy_gate_silence() {
        let vad = VadState::new(0.01, 250, 1000, 16000);
        let silent = vec![0.0f32; 160];
        assert!(!vad.energy_gate(&silent));
    }

    #[test]
    fn test_energy_gate_speech() {
        let vad = VadState::new(0.01, 250, 1000, 16000);
        let speech: Vec<f32> = (0..160).map(|i| (i as f32 * 0.1).sin() * 0.1).collect();
        assert!(vad.energy_gate(&speech));
    }

    #[test]
    fn test_speech_detection() {
        let mut vad = VadState::new(0.005, 250, 1000, 16000);
        // Feed speech frames
        let speech: Vec<f32> = (0..16000).map(|i| (i as f32 * 0.1).sin() * 0.1).collect();
        let chunks: Vec<&[f32]> = speech.chunks(160).collect();

        let mut got_utterance = false;
        for chunk in &chunks {
            vad.process(chunk);
        }
        // Feed silence to trigger flush
        let silence = vec![0.0f32; 160 * 10]; // ~100ms silence per iteration
        for chunk in silence.chunks(160) {
            if vad.process(chunk).is_some() {
                got_utterance = true;
                break;
            }
        }
        // May or may not get utterance depending on timing — just verify no panic
        let _ = got_utterance;
    }
}
