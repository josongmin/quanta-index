//! Pinned, local-only `Model2Vec` provider for Semble's potion-code-16M-v2 model.

use std::{fs, path::Path};

use model2vec_rs::model::StaticModel;
use quanta_index_contract::EmbeddingNormalization;
use quanta_index_core::{CoreError, EMBED_CHECKPOINT, RequestBudgetV1, TextEmbeddingProvider};
use sha2::{Digest, Sha256};

pub const POTION_CODE_MODEL_ID: &str = "model2vec:minishlab/potion-code-16M-v2";
// V1 is the historical effective policy: tokenizer.json retains its 512-token
// cap even when encode_with_args receives None. Keep that identity stable.
pub const POTION_CODE_MODEL_REVISION: &str =
    "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v1";
pub const POTION_CODE_FULL_V2_MODEL_REVISION: &str =
    "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v2";
pub const POTION_CODE_DIMENSION: usize = 256;
const BATCH_SIZE: usize = 1024;
const TOKENIZER_SHA256: &str = "107bbdcbad4bff1d299b7a4c3a2fb17c52890688b7dd0e4c9deab79d3c4f3d45";
const MODEL_SHA256: &str = "75cf7a6c2171b230ad19b1e7d8e0b1aee86da5a02af8e7cacedd9921d227623c";
const CONFIG_SHA256: &str = "148e5691a6fcc553437156859701fba017a1ba5d340b170f17e0f3668fb861a7";

#[cfg(test)]
mod parity_capture;
#[cfg(test)]
mod parity_fixture;

pub struct PotionCodeEmbeddingProvider {
    model: StaticModel,
    policy: PotionCodeEncodingPolicy,
}

/// Explicit encoder policy. V1 preserves historical vectors; V2 removes the
/// pinned tokenizer's 512-token cap in memory and requires a new generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PotionCodeEncodingPolicy {
    Pinned512V1,
    FullLengthV2,
}

impl PotionCodeEncodingPolicy {
    #[must_use]
    pub const fn model_revision(self) -> &'static str {
        match self {
            Self::Pinned512V1 => POTION_CODE_MODEL_REVISION,
            Self::FullLengthV2 => POTION_CODE_FULL_V2_MODEL_REVISION,
        }
    }

    #[must_use]
    pub const fn selector(self) -> &'static str {
        match self {
            Self::Pinned512V1 => "potion-code",
            Self::FullLengthV2 => "potion-code-full-v2",
        }
    }
}

impl PotionCodeEmbeddingProvider {
    /// Load only the exact upstream snapshot, never implicitly download or accept mutable weights.
    pub fn from_local_dir(dir: &Path) -> Result<Self, CoreError> {
        Self::from_local_dir_with_policy(dir, PotionCodeEncodingPolicy::Pinned512V1)
    }

    /// Select V2 explicitly; the default constructor preserves V1 vectors.
    pub fn from_local_dir_with_policy(
        dir: &Path,
        policy: PotionCodeEncodingPolicy,
    ) -> Result<Self, CoreError> {
        if !dir.is_absolute() {
            return Err(CoreError::InvalidContract(
                "model2vec: model directory must be absolute".to_string(),
            ));
        }
        let tokenizer = read_verified(dir, "tokenizer.json", TOKENIZER_SHA256)?;
        let tokenizer = match policy {
            PotionCodeEncodingPolicy::Pinned512V1 => tokenizer,
            PotionCodeEncodingPolicy::FullLengthV2 => {
                tokenizer_without_persisted_truncation(&tokenizer)?
            }
        };
        let weights = read_verified(dir, "model.safetensors", MODEL_SHA256)?;
        let config = read_verified(dir, "config.json", CONFIG_SHA256)?;
        // The shared Quanta wrapper performs the same final L2 normalization on
        // both query and corpus vectors. Do not apply Model2Vec normalization twice.
        let model = StaticModel::from_bytes(&tokenizer, &weights, &config, Some(false))
            .map_err(|error| CoreError::Storage(format!("model2vec: invalid model: {error}")))?;
        Ok(Self { model, policy })
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

/// Remove the pinned tokenizer's 512-token cap in memory.
///
/// `model2vec-rs`'s
/// `encode_with_args(None, ..)` bypasses its own cap but still honors the
/// tokenizer's serialized cap. Clear that cap only after verifying the raw
/// asset digest; the installed snapshot remains unchanged.
fn tokenizer_without_persisted_truncation(bytes: &[u8]) -> Result<Vec<u8>, CoreError> {
    let mut tokenizer: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        CoreError::InvalidContract(format!("model2vec: tokenizer JSON: {error}"))
    })?;
    let object = tokenizer.as_object_mut().ok_or_else(|| {
        CoreError::InvalidContract("model2vec: tokenizer JSON must be an object".to_string())
    })?;
    let max_length = object
        .get("truncation")
        .and_then(|value| value.get("max_length"))
        .and_then(serde_json::Value::as_u64);
    if max_length != Some(512) {
        return Err(CoreError::InvalidContract(
            "model2vec: pinned tokenizer truncation must be 512 before disabling it".to_string(),
        ));
    }
    let _pinned_truncation = object.insert("truncation".to_string(), serde_json::Value::Null);
    serde_json::to_vec(&tokenizer)
        .map_err(|error| CoreError::InvalidContract(format!("model2vec: tokenizer JSON: {error}")))
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
        self.policy.model_revision()
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

    #[test]
    fn pinned_tokenizer_cap_is_removed_only_in_memory() {
        let raw = br#"{"truncation":{"max_length":512},"model":{}}"#;
        let decoded = tokenizer_without_persisted_truncation(raw).expect("pinned cap");
        let value: serde_json::Value = serde_json::from_slice(&decoded).expect("valid JSON");
        assert!(
            value
                .get("truncation")
                .is_some_and(serde_json::Value::is_null)
        );
        assert_eq!(value.get("model"), Some(&serde_json::json!({})));
        assert!(tokenizer_without_persisted_truncation(br#"{"truncation":null}"#).is_err());
        assert!(
            tokenizer_without_persisted_truncation(br#"{"truncation":{"max_length":256}}"#)
                .is_err()
        );
    }

    #[test]
    fn policy_identity_and_default_remain_distinct() {
        assert_eq!(
            PotionCodeEncodingPolicy::Pinned512V1.model_revision(),
            POTION_CODE_MODEL_REVISION
        );
        assert_eq!(
            PotionCodeEncodingPolicy::FullLengthV2.model_revision(),
            POTION_CODE_FULL_V2_MODEL_REVISION
        );
        assert_ne!(
            POTION_CODE_MODEL_REVISION,
            POTION_CODE_FULL_V2_MODEL_REVISION
        );
        assert_eq!(
            PotionCodeEncodingPolicy::Pinned512V1.selector(),
            "potion-code"
        );
        assert_eq!(
            PotionCodeEncodingPolicy::FullLengthV2.selector(),
            "potion-code-full-v2"
        );
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "the negative control and corrected output are length-checked before indexing"
    )]
    #[test]
    #[ignore = "requires the pinned 33 MB upstream model assets"]
    fn pinned_model_uses_tokens_after_the_old_512_token_cap() {
        let dir = std::env::var("QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR")
            .expect("set QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR");
        let dir = Path::new(&dir);
        let provider = PotionCodeEmbeddingProvider::from_local_dir_with_policy(
            dir,
            PotionCodeEncodingPolicy::FullLengthV2,
        )
        .expect("pinned model loads");
        let prefix = "route ".repeat(600);
        let left = format!("{prefix}render content type");
        let right = format!("{prefix}binding form values");
        // Independent negative control: the raw pinned tokenizer cap makes
        // these distinct long inputs exactly equal under the old encoder.
        let raw_tokenizer =
            read_verified(dir, "tokenizer.json", TOKENIZER_SHA256).expect("pinned tokenizer reads");
        let weights =
            read_verified(dir, "model.safetensors", MODEL_SHA256).expect("pinned weights read");
        let config = read_verified(dir, "config.json", CONFIG_SHA256).expect("pinned config reads");
        let legacy = StaticModel::from_bytes(&raw_tokenizer, &weights, &config, Some(false))
            .expect("legacy model loads");
        let legacy_vectors =
            legacy.encode_with_args(&[left.clone(), right.clone()], None, BATCH_SIZE);
        assert_eq!(legacy_vectors.len(), 2);
        assert_eq!(
            legacy_vectors[0], legacy_vectors[1],
            "old cap masks both suffixes"
        );
        let default =
            PotionCodeEmbeddingProvider::from_local_dir(dir).expect("default V1 model loads");
        assert_eq!(default.model_revision(), POTION_CODE_MODEL_REVISION);
        assert_eq!(
            default
                .embed_batch(&[&left, &right])
                .expect("default inference"),
            legacy_vectors,
            "default constructor must preserve historical effective-512 vectors"
        );
        assert_eq!(
            provider.model_revision(),
            POTION_CODE_FULL_V2_MODEL_REVISION
        );
        let vectors = provider
            .embed_batch(&[&prefix, &left, &right])
            .expect("long-input inference");
        assert_eq!(vectors.len(), 3);
        assert_ne!(
            vectors[0], vectors[1],
            "suffix after token 512 must affect pooling"
        );
        assert_ne!(
            vectors[0], vectors[2],
            "suffix after token 512 must affect pooling"
        );
        assert_ne!(
            vectors[1], vectors[2],
            "distinct suffixes must affect pooling"
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
        const NORM_TOLERANCE: f64 = 0.002;
        const COS_TOLERANCE: f64 = 0.005;

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
        // Validation happens before loading assets or inference. Missing JSON
        // keys must never be confused with an explicit null policy.
        let reference_bytes = std::fs::read(&reference_path).expect("reference fixture reads");
        let fixture = parity_fixture::ParityFixture::parse(&reference_bytes)
            .expect("reference fixture is complete, pinned and internally consistent");
        let inputs: Vec<&str> = fixture.inputs.iter().map(String::as_str).collect();
        // The pinned reference output is the internally-L2-normalized
        // layer (model2vec 0.9.0), compared against the Rust
        // L2Unit-normalized output below.
        let vectors_reference = &fixture.vectors;

        let provider = PotionCodeEmbeddingProvider::from_local_dir_with_policy(
            Path::new(&model_dir),
            PotionCodeEncodingPolicy::FullLengthV2,
        )
        .expect("pinned V2 model loads");
        // Audit hardening: a truncated fixture must fail loudly here,
        // not narrow the zip comparison below.
        assert_eq!(
            vectors_reference.len(),
            inputs.len(),
            "fixture holds a wrong number of reference vectors"
        );
        for vector in vectors_reference {
            assert_eq!(
                vector.len(),
                POTION_CODE_DIMENSION,
                "fixture vector is not full-width"
            );
        }
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
        assert_eq!(unit.len(), inputs.len());
        for vector in &unit {
            assert_eq!(vector.len(), POTION_CODE_DIMENSION);
            assert!(vector.iter().all(|value| value.is_finite()));
            let norm = vector
                .iter()
                .map(|value| f64::from(*value).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!((norm - 1.0).abs() < NORM_TOLERANCE, "Rust L2 norm {norm}");
        }
        // Audit finding: 0.005 was ~100x the fp16 quantization step for
        // the observed component magnitudes. Empirical floor: the
        // tokenless (empty-pool) vector deviates up to ~1.3e-3 through
        // the fp16->fp32 pooling path, so 2e-3 is the tight bound that
        // still admits the real edge (2.5x tighter than the old 5e-3).
        for (index, (actual, expected)) in unit.iter().zip(vectors_reference).enumerate() {
            for (position, (a, e)) in actual.iter().zip(expected).enumerate() {
                assert!(
                    (f64::from(*a) - e).abs() < NORM_TOLERANCE,
                    "normalized vector {index} component {position}: {a} != {e}"
                );
            }
        }
        let pairwise_reference = &fixture.pairwise_cosine_upper;
        for (i, row) in pairwise_reference.iter().enumerate() {
            for (offset, expected) in row.iter().enumerate() {
                let j = i + offset + 1;
                let actual: f32 = unit[i].iter().zip(&unit[j]).map(|(a, b)| a * b).sum();
                assert!(
                    (f64::from(actual) - expected).abs() < COS_TOLERANCE,
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
        assert_eq!(permuted.len(), unit.len());
        for (index, vector) in permuted.iter().rev().enumerate() {
            assert_eq!(vector.len(), POTION_CODE_DIMENSION);
            assert!(vector.iter().all(|value| value.is_finite()));
            for (a, e) in vector.iter().zip(&unit[index]) {
                assert!(
                    (f64::from(*a) - f64::from(*e)).abs() < NORM_TOLERANCE,
                    "permutation moved a vector"
                );
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
            assert!(
                (f64::from(*a) - f64::from(*e)).abs() < NORM_TOLERANCE,
                "tokenless inputs diverged"
            );
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
            assert!(
                (f64::from(*a) - f64::from(*e)).abs() < NORM_TOLERANCE,
                "duplicate diverged"
            );
        }
        // Emitted only after all assertions succeed. The terminal wrapper,
        // not this output, must bind binary/source/dependencies and exit state.
        if let Some(path) = std::env::var_os("QUANTA_INDEX_PARITY_CAPTURE") {
            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "model2vec-native-parity-capture",
                "reference_schema_version": 2,
                "reference_sha256": format!("{:x}", Sha256::digest(&reference_bytes)),
                "model": {
                    "id": POTION_CODE_MODEL_ID,
                    "revision": POTION_CODE_FULL_V2_MODEL_REVISION,
                    "safetensors_sha256": MODEL_SHA256,
                    "tokenizer_sha256": TOKENIZER_SHA256,
                    "config_sha256": CONFIG_SHA256
                },
                "policy": {"max_length": null, "raw_precision": "f32", "unit_normalization": "L2UnitEmbeddingProvider"},
                "dimension": POTION_CODE_DIMENSION,
                "inputs": inputs,
                "raw_vectors": raw,
                "raw_norms": parity_capture::norms(&raw),
                "unit_vectors": unit,
                "unit_norms": parity_capture::norms(&unit),
                "unit_pairwise_cosine_upper": parity_capture::cosine_triangle(&unit),
                "permuted_inputs": reversed,
                "permuted_unit_vectors": permuted,
                "component_tolerance": NORM_TOLERANCE,
                "cosine_tolerance": COS_TOLERANCE,
                "custody": "unqualified: requires independent terminal/source/binary/dependency binding"
            });
            parity_capture::write_new_external(Path::new(&path), &payload)
                .expect("optional native parity capture writes once outside the source repository");
        }
    }
}
