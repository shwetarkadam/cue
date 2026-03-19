use anyhow::Result;
use cue_core::{
    audio::{AudioCapture, Utterance},
    config::Config,
    session::{SessionStore, TranscriptEntry},
    stt::SttEngine,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::error;

/// Listen-only mode: transcribe audio without LLM
pub async fn run() -> Result<()> {
    let config = Config::load()?;
    Config::ensure_dirs()?;

    let db_path = Config::db_path();
    let session_store = Arc::new(SessionStore::new(&db_path)?);
    session_store.init_schema()?;
    let session = session_store.create_session("listen")?;

    eprintln!("[cue] Listen mode — transcribing audio (Ctrl+C to stop)");

    let (utterance_tx, mut utterance_rx) = mpsc::channel::<Utterance>(16);
    let audio_capture = AudioCapture::new(config.audio.clone(), utterance_tx);

    let _stream = match audio_capture.start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[cue ERROR] Failed to start audio: {}", e);
            return Err(e);
        }
    };

    let models_dir = Config::models_dir();
    let stt_engine: Option<Arc<SttEngine>> = {
        let model_path = SttEngine::model_path(&config.stt.model, &models_dir);
        if model_path.exists() {
            match SttEngine::new(&config.stt, &models_dir) {
                Ok(e) => Some(Arc::new(e)),
                Err(e) => {
                    eprintln!("[cue ERROR] STT load failed: {}", e);
                    None
                }
            }
        } else {
            eprintln!("[cue] No STT model. Run: cue models download {}", config.stt.model);
            None
        }
    };

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = &mut ctrl_c => {
                eprintln!("[cue] Stopped.");
                break;
            }
            Some(utterance) = utterance_rx.recv() => {
                if let Some(ref engine) = stt_engine {
                    let engine = Arc::clone(engine);
                    let store = Arc::clone(&session_store);
                    let session_id = session.id;
                    let channel = utterance.channel;
                    let samples = utterance.samples;

                    tokio::task::spawn_blocking(move || {
                        match engine.transcribe(&samples) {
                            Ok(text) if !text.trim().is_empty() => {
                                let label = if channel.to_string() == "system" { "THEM" } else { "YOU" };
                                println!("[{}] {}", label, text);
                                let entry = TranscriptEntry {
                                    id: None,
                                    session_id,
                                    channel: channel.to_string(),
                                    text,
                                    spoken_at: chrono::Utc::now(),
                                };
                                let _ = store.add_transcript(&entry);
                            }
                            Ok(_) => {}
                            Err(e) => error!(error = %e, "STT error"),
                        }
                    });
                }
            }
        }
    }

    Ok(())
}
