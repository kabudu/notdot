//! Speaker embeddings from the pinned CAM++ model via sherpa-onnx.
use crate::{EMBEDDING_DIM, MODEL_BYTES, MODEL_SHA256};
use std::{io::Read, path::Path};

/// Something that turns voiced audio into an L2-normalised speaker embedding.
/// Implemented by [`CamPlusPlus`]; tests may supply their own.
pub trait Extractor: Send + Sync {
    fn embed(&self, rate: u32, voiced: &[f32]) -> Result<Vec<f32>, String>;
    fn dim(&self) -> usize;
    /// Identifies the model so stored voiceprints are invalidated when it changes.
    fn model_id(&self) -> [u8; 32];
}

pub struct CamPlusPlus {
    inner: sherpa_onnx::SpeakerEmbeddingExtractor,
    model_id: [u8; 32],
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Verify size, type and SHA-256 of a model file without following symlinks.
pub fn verify_model(path: &Path) -> Result<[u8; 32], String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "Speaker model is missing")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != MODEL_BYTES {
        return Err("Speaker model is not the expected file".into());
    }
    let mut file = std::fs::File::open(path)
        .map_err(|_| "Speaker model is unreadable")?
        .take(MODEL_BYTES + 1);
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| "Speaker model is unreadable")?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let digest = ctx.finish();
    if hex(digest.as_ref()) != MODEL_SHA256 {
        return Err("Speaker model checksum does not match".into());
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(digest.as_ref());
    Ok(id)
}

impl CamPlusPlus {
    /// Load the model after verifying its checksum. One inference thread keeps
    /// CPU use predictable alongside wake detection.
    pub fn load(path: &Path) -> Result<Self, String> {
        let model_id = verify_model(path)?;
        let config = sherpa_onnx::SpeakerEmbeddingExtractorConfig {
            model: Some(
                path.to_str()
                    .ok_or("Speaker model path is not UTF-8")?
                    .to_owned(),
            ),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        };
        let inner = sherpa_onnx::SpeakerEmbeddingExtractor::create(&config)
            .ok_or("Speaker model could not be loaded")?;
        if inner.dim() as usize != EMBEDDING_DIM {
            return Err("Speaker model has an unexpected embedding size".into());
        }
        Ok(Self { inner, model_id })
    }
}

pub fn normalise(mut v: Vec<f32>) -> Result<Vec<f32>, String> {
    if v.iter().any(|x| !x.is_finite()) {
        return Err("Speaker embedding is not finite".into());
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-6 {
        return Err("Speaker embedding is empty".into());
    }
    v.iter_mut().for_each(|x| *x /= norm);
    Ok(v)
}

impl Extractor for CamPlusPlus {
    fn embed(&self, rate: u32, voiced: &[f32]) -> Result<Vec<f32>, String> {
        let stream = self
            .inner
            .create_stream()
            .ok_or("Speaker stream could not be created")?;
        stream.accept_waveform(rate as i32, voiced);
        stream.input_finished();
        if !self.inner.is_ready(&stream) {
            return Err("Not enough speech for a voiceprint".into());
        }
        let v = self
            .inner
            .compute(&stream)
            .ok_or("Speaker embedding failed")?;
        if v.len() != EMBEDDING_DIM {
            return Err("Speaker embedding has an unexpected size".into());
        }
        normalise(v)
    }
    fn dim(&self) -> usize {
        EMBEDDING_DIM
    }
    fn model_id(&self) -> [u8; 32] {
        self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tampered_or_missing_models_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.onnx");
        assert!(verify_model(&path).is_err());
        std::fs::write(&path, vec![0u8; MODEL_BYTES as usize]).unwrap();
        assert_eq!(
            verify_model(&path).unwrap_err(),
            "Speaker model checksum does not match"
        );
        std::fs::write(&path, b"short").unwrap();
        assert!(verify_model(&path).is_err());
    }
    #[test]
    fn normalisation_rejects_degenerate_vectors() {
        assert!(normalise(vec![0.0; 4]).is_err());
        assert!(normalise(vec![f32::NAN, 1.0]).is_err());
        let v = normalise(vec![3.0, 4.0]).unwrap();
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
    }
}
