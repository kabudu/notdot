//! Debug-only, explicit synthetic-corpus evaluation through native credentials.
//! Does not touch conversation history or memory. Never compiled into releases.
use crate::{Settings, credentials};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc, time::Instant};
#[derive(Deserialize)]
struct Case {
    path: PathBuf,
    expected: String,
}
pub async fn run(path: PathBuf) -> Result<(), String> {
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 65536 {
        return Err("Corpus manifest too large".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Corpus manifest too large".into());
    }
    let cases: Vec<Case> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if cases.is_empty() || cases.len() > 64 {
        return Err("Use 1–64 synthetic clips".into());
    }
    let dir = PathBuf::from(std::env::var_os("HOME").ok_or("Home missing")?)
        .join("Library/Application Support")
        .join(crate::app_context().config().identifier.clone());
    let settings = Settings::load(&dir.join("settings.json"))?;
    let key = Arc::new(credentials::Credentials::read_only())
        .read(settings.transcription_id(), true)
        .await?;
    let provider = notdot_voice::transcription_provider(
        settings.transcription_id(),
        key,
        notdot_voice::http_client()?,
        Some("en"),
    )?;
    for case in cases {
        if std::fs::metadata(&case.path)
            .map_err(|e| e.to_string())?
            .len()
            > 3_000_000
        {
            return Err("Synthetic clip too large".into());
        }
        let bytes = std::fs::read(&case.path).map_err(|e| e.to_string())?;
        if bytes.len() > 3_000_000 {
            return Err("Synthetic clip too large".into());
        }
        let mut wav =
            hound::WavReader::new(std::io::Cursor::new(&bytes)).map_err(|e| e.to_string())?;
        let spec = wav.spec();
        if spec.channels != 1
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
        {
            return Err("Use mono PCM16 WAV".into());
        }
        if wav.duration() > spec.sample_rate.saturating_mul(30) {
            return Err("Use clips of at most 30 seconds".into());
        }
        let pcm: Vec<u8> = wav
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let start = Instant::now();
        let mut stream = provider
            .start_transcription(spec.sample_rate)
            .await
            .map_err(|e| e.to_string())?;
        for chunk in pcm.chunks(spec.sample_rate as usize / 5) {
            stream.send(chunk).await.map_err(|e| e.to_string())?;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let realtime = stream.commit().await.map_err(|e| e.to_string())?;
        stream.close().await;
        let realtime_ms = start.elapsed().as_millis();
        let start = Instant::now();
        let batch = if settings.transcription_id() == "deepgram" {
            None
        } else {
            Some(
                provider
                    .transcribe(notdot_voice::AudioClip {
                        bytes,
                        mime: "audio/wav".into(),
                    })
                    .await
                    .map_err(|e| e.to_string())?,
            )
        };
        println!(
            "{}",
            serde_json::json!({"provider":settings.transcription_id(),"path":case.path,"expected":case.expected,"realtime":realtime,"batch":batch,"realtime_ms":realtime_ms,"batch_ms":start.elapsed().as_millis()})
        );
    }
    Ok(())
}
