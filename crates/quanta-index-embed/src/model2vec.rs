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

    #[expect(
        clippy::indexing_slicing,
        reason = "asset-dependent parity test; every index follows a length assert"
    )]
    #[test]
    #[ignore = "requires the pinned model assets and the generated parity reference fixture"]
    fn full_vector_parity_against_pinned_reference() {
        // RBR-07 full-vector parity rail. The one-sentence/8-component
        // comparison above is not parity; this test is. It verifies the
        // complete 256-dimension vectors, norms, pairwise cosine, batch
        // permutation, and the tokenless-input contract against the
        // fixture generated by tools/benchmark/retrieval/parity_reference.py
        // in the pinned venv.
        let model_dir = std::env::var("QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR")
            .expect("set QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR");
        let reference_path = std::env::var("QUANTA_INDEX_PARITY_REFERENCE")
            .expect("set QUANTA_INDEX_PARITY_REFERENCE");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&reference_path).expect("reference fixture reads"),
        )
        .expect("reference fixture parses");

        // Identity binding: the fixture must describe exactly the pinned
        // model bytes this crate refuses to load anything else for.
        assert_eq!(fixture["profile"], "model2vec-static-potion-code-16M-v2");
        assert_eq!(
            fixture["policy"]["max_length"],
            serde_json::Value::Null,
            "reference must encode unbounded, matching the Rust decoder"
        );
        assert_eq!(
            fixture["model"]["safetensors_sha256"], MODEL_SHA256,
            "fixture model bytes differ from the crate pin"
        );
        assert_eq!(fixture["dimension"], 256);

        let inputs: Vec<&str> = fixture["inputs"]
            .as_array()
            .expect("inputs array")
            .iter()
            .map(|value| value.as_str().expect("input string"))
            .collect();
        // The pinned reference output is the internally-L2-normalized
        // layer (model2vec 0.9.0), compared against the Rust
        // L2Unit-normalized output below.
        let vectors_reference: Vec<Vec<f32>> = fixture["vectors"]
            .as_array()
            .expect("vectors")
            .iter()
            .map(|vector| {
                vector
                    .as_array()
                    .expect("vector")
                    .iter()
                    .map(|value| value.as_f64().expect("f64") as f32)
                    .collect()
            })
            .collect();

        let provider = PotionCodeEmbeddingProvider::from_local_dir(Path::new(&model_dir))
            .expect("pinned model loads");
        let raw = provider.embed_batch(&inputs).expect("raw inference");
        assert_eq!(raw.len(), inputs.len());
        for vector in &raw {
            assert_eq!(vector.len(), POTION_CODE_DIMENSION);
            assert!(vector.iter().all(|value| value.is_finite()));
        }
        // Normalized layer: the full-vector comparison within
        // float16-quantization tolerance.
        let normalized = quanta_index_core::L2UnitEmbeddingProvider::new(provider)
            .expect("one shared normalizer");
        let unit = normalized
            .embed_batch(&inputs)
            .expect("normalized inference");
        const NORM_TOLERANCE: f32 = 0.005;
        for (index, (actual, expected)) in unit.iter().zip(&vectors_reference).enumerate() {
            for (position, (a, e)) in actual.iter().zip(expected).enumerate() {
                assert!(
                    (a - e).abs() < NORM_TOLERANCE,
                    "normalized vector {index} component {position}: {a} != {e}"
                );
            }
        }
        let pairwise_reference: Vec<Vec<f32>> = fixture["pairwise_cosine_upper"]
            .as_array()
            .expect("pairwise")
            .iter()
            .map(|row| {
                row.as_array()
                    .expect("row")
                    .iter()
                    .map(|value| value.as_f64().expect("f64") as f32)
                    .collect()
            })
            .collect();
        const COS_TOLERANCE: f32 = 0.005;
        for (i, row) in pairwise_reference.iter().enumerate() {
            for (offset, expected) in row.iter().enumerate() {
                let j = i + offset + 1;
                let actual: f32 = unit[i].iter().zip(&unit[j]).map(|(a, b)| a * b).sum();
                assert!(
                    (actual - expected).abs() < COS_TOLERANCE,
                    "cosine ({i},{j}): {actual} != {expected}"
                );
            }
        }
        // Batch permutation invariance.
        let mut reversed: Vec<&str> = inputs.clone();
        reversed.reverse();
        let permuted = normalized
            .embed_batch(&reversed)
            .expect("permuted inference");
        for (index, vector) in permuted.iter().rev().enumerate() {
            for (a, e) in vector.iter().zip(&unit[index]) {
                assert!((a - e).abs() < NORM_TOLERANCE, "permutation moved a vector");
            }
        }
        // Tokenless contract: the empty and whitespace-only inputs embed
        // to the same NON-zero pooled vector on both sides — never a
        // fabricated zero vector, never an error.
        let empty = &unit[inputs
            .iter()
            .position(|value| value.is_empty())
            .expect("empty")];
        let whitespace = &unit[inputs
            .iter()
            .position(|value| value.trim().is_empty() && !value.is_empty())
            .expect("whitespace")];
        for (a, e) in empty.iter().zip(whitespace) {
            assert!((a - e).abs() < NORM_TOLERANCE, "tokenless inputs diverged");
        }
        assert!(
            empty.iter().any(|value| value.abs() > 1e-6),
            "tokenless input must not fabricate a zero vector"
        );
        // Duplicate inputs embed identically.
        let first = 0_usize;
        let duplicate = inputs
            .iter()
            .skip(1)
            .position(|value| *value == inputs[first])
            .map(|offset| offset + 1)
            .expect("duplicate input present");
        for (a, e) in unit[first].iter().zip(&unit[duplicate]) {
            assert!((a - e).abs() < NORM_TOLERANCE, "duplicate diverged");
        }
    }
}
