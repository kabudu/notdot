//! Explicit provider-only checks with synthetic clips and the local OS key store.
//! No microphone, model, conversation, memory or credential writes are involved.
#![forbid(unsafe_code)]
use notdot_core::settings::Settings;
use serde::Deserialize;
use std::{io::Read, path::PathBuf, time::Duration};

#[derive(Deserialize)]
struct Case {
    path: PathBuf,
    expected: String,
}

fn read_bounded(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "Cannot open fixture input")?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read fixture input")?;
    if bytes.len() as u64 > limit {
        return Err("Fixture input exceeds its size limit".into());
    }
    Ok(bytes)
}

async fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: transcribe_fixtures SETTINGS_JSON SYNTHETIC_CORPUS_JSON".into());
    }
    let settings = Settings::load(std::path::Path::new(&args[0]))?;
    let provider_id = settings.transcription_id();
    let cases: Vec<Case> =
        serde_json::from_slice(&read_bounded(std::path::Path::new(&args[1]), 65536)?)
            .map_err(|_| "Invalid synthetic corpus manifest")?;
    if cases.is_empty() || cases.len() > 16 {
        return Err("Use 1–16 synthetic clips per invocation".into());
    }
    // Use the existing native-user entry; never accept keys as CLI arguments,
    // print them, copy another OS's vault, or create an empty fallback entry.
    let key = keyring::Entry::new("com.kabudu.notdot", provider_id)
        .map_err(|_| "Cannot open local voice credential entry")?
        .get_password()
        .map_err(|_| "Saved recognition key unavailable in local OS credential store")?;
    let provider = notdot_voice::transcription_provider(
        provider_id,
        key,
        notdot_voice::http_client()?,
        Some("en"),
    )?;
    let mut failed = false;
    for (index, case) in cases.into_iter().enumerate() {
        let bytes = read_bounded(&case.path, 3_000_000)?;
        let mut wav = hound::WavReader::new(std::io::Cursor::new(bytes))
            .map_err(|_| "Invalid fixture WAV")?;
        let spec = wav.spec();
        if spec.channels != 1
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
            || ![8000, 16000, 24000, 44100, 48000].contains(&spec.sample_rate)
            || wav.duration() > spec.sample_rate * 30
        {
            return Err("Use supported-rate mono PCM16 WAV clips of at most 30 seconds".into());
        }
        let pcm: Vec<u8> = wav
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Invalid fixture PCM")?
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            let mut stream = provider.start_transcription(spec.sample_rate).await?;
            let result = async {
                for packet in pcm.chunks(spec.sample_rate as usize / 5) {
                    stream.send(packet).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                stream.commit().await
            }
            .await;
            let stats = stream.stats();
            stream.close().await;
            result.map(|text| (text, stats))
        })
        .await
        .unwrap_or_else(|_| {
            Err(notdot_voice::VoiceError::new(
                notdot_voice::VoiceErrorKind::Network,
                "Synthetic provider request exceeded 45 seconds",
            ))
        });
        let value = match result {
            Ok((text, stats)) => serde_json::json!({
                "case": index + 1, "expected": case.expected,
                "transcript": text.chars().take(600).collect::<String>(),
                "partials": stats.partial_events, "segments": stats.committed_segments,
                "elapsed_ms": start.elapsed().as_millis(), "status": "completed"
            }),
            Err(error) => {
                failed = true;
                serde_json::json!({
                    "case": index + 1, "expected": case.expected,
                    "status": "failed", "error_kind": error.kind,
                    "error": error.to_string(), "elapsed_ms": start.elapsed().as_millis()
                })
            }
        };
        println!("{value}");
    }
    if failed {
        Err("One or more provider requests failed; see case output".into())
    } else {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Synthetic provider check: {error}");
        std::process::exit(1);
    }
}
