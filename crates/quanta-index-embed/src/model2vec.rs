//! Pinned, local-only `Model2Vec` provider for Semble's potion-code-16M-v2 model.

use std::{fs, path::Path};

use model2vec_rs::model::StaticModel;
use quanta_index_contract::EmbeddingNormalization;
use quanta_index_core::{CoreError, EMBED_CHECKPOINT, RequestBudgetV1, TextEmbeddingProvider};
use sha2::{Digest, Sha256};

pub const POTION_CODE_MODEL_ID: &str = "model2vec:minishlab/potion-code-16M-v2";
// Include the encoder and no-truncation policy: changing either changes vector identity.
pub const POTION_CODE_MODEL_REVISION: &str =
    "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v1";
pub const POTION_CODE_DIMENSION: usize = 256;
const BATCH_SIZE: usize = 1024;
const TOKENIZER_SHA256: &str = "107bbdcbad4bff1d299b7a4c3a2fb17c52890688b7dd0e4c9deab79d3c4f3d45";
const MODEL_SHA256: &str = "75cf7a6c2171b230ad19b1e7d8e0b1aee86da5a02af8e7cacedd9921d227623c";
const CONFIG_SHA256: &str = "148e5691a6fcc553437156859701fba017a1ba5d340b170f17e0f3668fb861a7";

pub struct PotionCodeEmbeddingProvider {
    model: StaticModel,
}

impl PotionCodeEmbeddingProvider {
    /// Load only the exact upstream snapshot, never implicitly download or accept mutable weights.
    pub fn from_local_dir(dir: &Path) -> Result<Self, CoreError> {
        if !dir.is_absolute() {
            return Err(CoreError::InvalidContract(
                "model2vec: model directory must be absolute".to_string(),
            ));
        }
        let tokenizer = read_verified(dir, "tokenizer.json", TOKENIZER_SHA256)?;
        let weights = read_verified(dir, "model.safetensors", MODEL_SHA256)?;
        let config = read_verified(dir, "config.json", CONFIG_SHA256)?;
        // The shared Quanta wrapper performs the same final L2 normalization on
        // both query and corpus vectors. Do not apply Model2Vec normalization twice.
        let model = StaticModel::from_bytes(&tokenizer, &weights, &config, Some(false))
            .map_err(|error| CoreError::Storage(format!("model2vec: invalid model: {error}")))?;
        Ok(Self { model })
    }

    fn encode(
        &self,
        texts: &[&str],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH_SIZE) {
            if let Some(budget) = budget {
                budget.checkpoint(EMBED_CHECKPOINT)?;
            }
            let inputs = batch
                .iter()
                .map(|text| (*text).to_string())
                .collect::<Vec<_>>();
            let encoded = self.model.encode_with_args(&inputs, None, BATCH_SIZE);
            if encoded.len() != batch.len()
                || encoded
                    .iter()
                    .any(|vector| vector.len() != POTION_CODE_DIMENSION)
            {
                return Err(CoreError::Storage(
                    "model2vec: unexpected vector count or dimension".to_string(),
                ));
            }
            vectors.extend(encoded);
        }
        Ok(vectors)
    }
}

fn read_verified(dir: &Path, name: &str, expected: &str) -> Result<Vec<u8>, CoreError> {
    let path = dir.join(name);
    let bytes = fs::read(&path).map_err(|error| {
        CoreError::Storage(format!("model2vec: read {}: {error}", path.display()))
    })?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        return Err(CoreError::InvalidContract(format!(
            "model2vec: {name} SHA-256 mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(bytes)
}

impl TextEmbeddingProvider for PotionCodeEmbeddingProvider {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        self.encode(texts, None)
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        self.encode(texts, Some(budget))
    }

    fn model_id(&self) -> &str {
        POTION_CODE_MODEL_ID
    }

    fn model_revision(&self) -> &str {
        POTION_CODE_MODEL_REVISION
    }

    fn dimension(&self) -> usize {
        POTION_CODE_DIMENSION
    }

    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_model_fails_closed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let result = PotionCodeEmbeddingProvider::from_local_dir(directory.path());
        assert!(result.is_err());
    }

    #[test]
    fn relative_path_fails_closed() {
        let result = PotionCodeEmbeddingProvider::from_local_dir(Path::new("model"));
        assert!(matches!(result, Err(CoreError::InvalidContract(_))));
    }

    #[test]
    fn changed_tokenizer_is_refused_before_loading_weights() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(
            directory.path().join("tokenizer.json"),
            b"not the pinned tokenizer",
        )
        .expect("fixture write");
        let result = PotionCodeEmbeddingProvider::from_local_dir(directory.path());
        assert!(
            matches!(result, Err(CoreError::InvalidContract(message)) if message.contains("SHA-256 mismatch"))
        );
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "ignored fixture test; indices follow a length assert or a bounded zip"
    )]
    #[test]
    #[ignore = "requires the pinned 33 MB upstream model assets"]
    fn pinned_model_embeds_identically_for_repeated_inputs() {
        let dir = std::env::var("QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR")
            .expect("set QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR");
        let provider = PotionCodeEmbeddingProvider::from_local_dir(Path::new(&dir))
            .expect("pinned model loads");
        let vectors = provider
            .embed_batch(&["refresh access token", "refresh access token"])
            .expect("inference");
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), POTION_CODE_DIMENSION);
        assert_eq!(vectors[0], vectors[1]);
        assert!(vectors[0].iter().all(|value| value.is_finite()));
        let normalized = quanta_index_core::L2UnitEmbeddingProvider::new(provider)
            .expect("raw model needs one shared normalizer");
        let output = normalized
            .embed_batch(&["refresh access token"])
            .expect("normalized inference");
        // Semble's pinned Python Model2Vec 0.9.0 reference, max_length=None.
        // The upstream Python implementation returns float16, while the Rust
        // decoder pools/normalizes in float32, so compare within quantization.
        let reference = [
            -0.225_830_08,
            0.030_624_39,
            -0.054_656_98,
            -0.047_088_62,
            0.034_332_28,
            0.053_680_42,
            0.066_162_11,
            -0.037_017_82,
        ];
        for (actual, expected) in output[0].iter().zip(reference).take(reference.len()) {
            assert!((actual - expected).abs() < 0.005, "{actual} != {expected}");
        }
    }
}
