//! Strict, asset-free acceptance boundary for the external parity reference.
//! This validates completeness and internal consistency, not producer custody (T15).

use serde::Deserialize;

use super::{CONFIG_SHA256, MODEL_SHA256, POTION_CODE_DIMENSION, TOKENIZER_SHA256};

const CONSISTENCY_TOLERANCE: f64 = 1e-12;
const NORMALIZATION: &str = "approx-unit-fp16 (rail L2-normalizes both sides)";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ParityFixture {
    schema_version: u32,
    profile: String,
    library: Library,
    model: Model,
    policy: Policy,
    pub(super) inputs: Vec<String>,
    pub(super) vectors: Vec<Vec<f64>>,
    norms: Vec<f64>,
    pub(super) pairwise_cosine_upper: Vec<Vec<f64>>,
    dimension: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Library {
    model2vec: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    id: String,
    revision: String,
    dir_name: String,
    safetensors_sha256: String,
    tokenizer_sha256: String,
    config_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    // Unlike Option<T>, Value is a required field: explicit null is accepted,
    // a missing key is a deserialization error.
    max_length: serde_json::Value,
    normalization: String,
}

fn canonical_inputs() -> Vec<String> {
    vec![
        "refresh access token".into(),
        "parse_and_expression".into(),
        "quanta_index_retrieval_bench::sdk::query_route".into(),
        "fn main() { println!(\"{}\", x); }".into(),
        "한글 검색 αβγ 🚀".into(),
        String::new(),
        "   ".into(),
        "a".repeat(5000),
        "refresh access token".into(),
    ]
}

fn norm(vector: &[f64]) -> f64 {
    vector.iter().map(|value| value * value).sum::<f64>().sqrt()
}

impl ParityFixture {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, String> {
        let fixture: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        fixture.validate()?;
        Ok(fixture)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 2
            || self.profile != "model2vec-static-potion-code-16M-v2"
            || self.library.model2vec != "0.9.0"
            || self.model.id != "minishlab/potion-code-16M-v2"
            || self.model.revision != "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b"
            || self.model.dir_name.is_empty()
            || self.model.safetensors_sha256 != MODEL_SHA256
            || self.model.tokenizer_sha256 != TOKENIZER_SHA256
            || self.model.config_sha256 != CONFIG_SHA256
            || self.policy.max_length != serde_json::Value::Null
            || self.policy.normalization != NORMALIZATION
            || self.dimension != POTION_CODE_DIMENSION
        {
            return Err("reference identity or policy mismatch".into());
        }
        if self.inputs != canonical_inputs() {
            return Err("reference adversarial inputs differ in content or order".into());
        }
        let count = self.inputs.len();
        if self.vectors.len() != count
            || self.norms.len() != count
            || self.pairwise_cosine_upper.len() != count
        {
            return Err("reference vector/norm/pairwise row count mismatch".into());
        }
        for (vector, declared) in self.vectors.iter().zip(&self.norms) {
            let computed = norm(vector);
            if vector.len() != POTION_CODE_DIMENSION
                || vector.iter().any(|value| !value.is_finite())
                || !declared.is_finite()
                || *declared <= 0.0
                || !computed.is_finite()
                || computed <= 0.0
                || (computed - declared).abs() > CONSISTENCY_TOLERANCE
            {
                return Err("reference full vector or norm mismatch".into());
            }
        }
        for (index, row) in self.pairwise_cosine_upper.iter().enumerate() {
            if row.len() != count - index - 1 {
                return Err("reference pairwise triangle width mismatch".into());
            }
            let left = self.vectors.get(index).ok_or("missing left vector")?;
            let left_norm = norm(left);
            for (right, declared) in self.vectors.iter().skip(index + 1).zip(row) {
                let right_norm = norm(right);
                let computed: f64 = left
                    .iter()
                    .zip(right)
                    .map(|(a, b)| (a / left_norm) * (b / right_norm))
                    .sum();
                if !declared.is_finite() || (computed - declared).abs() > CONSISTENCY_TOLERANCE {
                    return Err("reference pairwise cosine inconsistent with full vectors".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> serde_json::Value {
        // An independent, exact basis-vector oracle: norm and each cosine = 1.
        // Asset-free fixture validity is distinct from parity with a real model.
        let vectors: Vec<Vec<f64>> = (0..9)
            .map(|_| {
                let mut vector = vec![0.0; POTION_CODE_DIMENSION];
                *vector.first_mut().expect("nonempty vector") = 1.0;
                vector
            })
            .collect();
        serde_json::json!({
            "schema_version": 2,
            "profile": "model2vec-static-potion-code-16M-v2",
            "library": {"model2vec": "0.9.0"},
            "model": {
                "id": "minishlab/potion-code-16M-v2",
                "revision": "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b",
                "dir_name": "test-only",
                "safetensors_sha256": MODEL_SHA256,
                "tokenizer_sha256": TOKENIZER_SHA256,
                "config_sha256": CONFIG_SHA256
            },
            "policy": {"max_length": null, "normalization": NORMALIZATION},
            "inputs": canonical_inputs(),
            "vectors": vectors,
            "norms": vec![1.0; 9],
            "pairwise_cosine_upper": (0..9).map(|i| vec![1.0; 8-i]).collect::<Vec<_>>(),
            "dimension": POTION_CODE_DIMENSION
        })
    }

    fn parses(value: &serde_json::Value) -> bool {
        ParityFixture::parse(&serde_json::to_vec(value).expect("JSON encodes")).is_ok()
    }

    #[test]
    fn strict_reference_accepts_complete_independent_oracle() {
        assert!(parses(&valid()));
    }

    #[test]
    fn strict_reference_refuses_missing_duplicate_unknown_and_wrong_metadata() {
        let baseline = valid();
        for key in baseline.as_object().expect("object").keys() {
            let mut mutant = baseline.clone();
            assert!(
                mutant
                    .as_object_mut()
                    .expect("object")
                    .remove(key)
                    .is_some()
            );
            assert!(!parses(&mutant), "missing {key}");
        }
        for parent in ["library", "model", "policy"] {
            let fields = baseline
                .get(parent)
                .expect("parent")
                .as_object()
                .expect("object");
            for key in fields.keys() {
                let mut mutant = baseline.clone();
                assert!(
                    mutant
                        .get_mut(parent)
                        .expect("parent")
                        .as_object_mut()
                        .expect("object")
                        .remove(key)
                        .is_some()
                );
                assert!(!parses(&mutant), "missing {parent}.{key}");
                let mut mutant = baseline.clone();
                *mutant
                    .get_mut(parent)
                    .expect("parent")
                    .get_mut(key)
                    .expect("field") = if key == "dir_name" {
                    serde_json::json!("")
                } else {
                    serde_json::json!("forged")
                };
                assert!(!parses(&mutant), "forged {parent}.{key}");
            }
        }
        for (key, replacement) in [
            ("schema_version", serde_json::json!(1)),
            ("profile", serde_json::json!("forged")),
            ("dimension", serde_json::json!(255)),
        ] {
            let mut mutant = baseline.clone();
            *mutant.get_mut(key).expect("field") = replacement;
            assert!(!parses(&mutant), "forged {key}");
        }
        let encoded = serde_json::to_string(&baseline).expect("JSON encodes");
        let duplicate = encoded.replacen(
            "\"schema_version\":2",
            "\"schema_version\":2,\"schema_version\":2",
            1,
        );
        assert!(ParityFixture::parse(duplicate.as_bytes()).is_err());
        let duplicate = encoded.replacen(
            "\"max_length\":null",
            "\"max_length\":null,\"max_length\":null",
            1,
        );
        assert!(ParityFixture::parse(duplicate.as_bytes()).is_err());
        for parent in ["library", "model", "policy"] {
            let mut mutant = baseline.clone();
            assert!(
                mutant
                    .get_mut(parent)
                    .expect("parent")
                    .as_object_mut()
                    .expect("object")
                    .insert("unknown".into(), serde_json::json!(true))
                    .is_none()
            );
            assert!(!parses(&mutant), "unknown {parent} field");
        }
        let mut mutant = baseline;
        assert!(
            mutant
                .as_object_mut()
                .expect("object")
                .insert("unknown".into(), serde_json::json!(true))
                .is_none()
        );
        assert!(!parses(&mutant));
    }

    #[test]
    fn strict_reference_refuses_narrowed_reordered_or_modified_inputs() {
        for variant in 0..4 {
            let mut mutant = valid();
            let inputs = mutant
                .get_mut("inputs")
                .expect("inputs")
                .as_array_mut()
                .expect("array");
            match variant {
                0 => {
                    assert!(inputs.pop().is_some());
                }
                1 => inputs.push(serde_json::json!("extra")),
                2 => inputs.swap(0, 1),
                _ => {
                    *inputs.first_mut().expect("input") = serde_json::json!("forged");
                }
            }
            assert!(!parses(&mutant), "variant {variant}");
        }
    }

    #[test]
    fn strict_reference_refuses_partial_or_forged_numeric_evidence() {
        for field in ["vectors", "norms", "pairwise_cosine_upper"] {
            for count in [0, 8, 10] {
                let mut mutant = valid();
                let values = mutant
                    .get_mut(field)
                    .expect("field")
                    .as_array_mut()
                    .expect("array");
                values.resize(count, serde_json::json!(0));
                assert!(!parses(&mutant), "{field} count {count}");
            }
        }
        for field in ["vectors", "pairwise_cosine_upper"] {
            for count in [0, 1, 255, 257] {
                let mut mutant = valid();
                let row = mutant
                    .get_mut(field)
                    .expect("field")
                    .as_array_mut()
                    .expect("array")
                    .first_mut()
                    .expect("row")
                    .as_array_mut()
                    .expect("array");
                row.resize(count, serde_json::json!(0));
                assert!(!parses(&mutant), "{field} width {count}");
            }
        }
        for field in ["norms", "pairwise_cosine_upper", "vectors"] {
            let mut mutant = valid();
            let first = mutant
                .get_mut(field)
                .expect("field")
                .as_array_mut()
                .expect("array")
                .first_mut()
                .expect("first");
            if let Some(row) = first.as_array_mut() {
                *row.first_mut().expect("component") = serde_json::json!(999);
            } else {
                *first = serde_json::json!(999);
            }
            assert!(!parses(&mutant), "forged {field}");
        }
        let encoded = serde_json::to_string(&valid()).expect("JSON encodes");
        for invalid in ["NaN", "Infinity", "-Infinity", "1e999"] {
            let mutant = encoded.replacen("\"norms\":[1.0", &format!("\"norms\":[{invalid}"), 1);
            assert!(
                ParityFixture::parse(mutant.as_bytes()).is_err(),
                "{invalid}"
            );
        }
        let mut overflow = ParityFixture::parse(encoded.as_bytes()).expect("valid");
        *overflow
            .vectors
            .first_mut()
            .expect("vector")
            .first_mut()
            .expect("component") = f64::MAX;
        assert!(overflow.validate().is_err());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut mutant = ParityFixture::parse(encoded.as_bytes()).expect("valid");
            *mutant.norms.first_mut().expect("norm") = value;
            assert!(mutant.validate().is_err());
            let mut mutant = ParityFixture::parse(encoded.as_bytes()).expect("valid");
            *mutant
                .vectors
                .first_mut()
                .expect("vector")
                .first_mut()
                .expect("component") = value;
            assert!(mutant.validate().is_err());
            let mut mutant = ParityFixture::parse(encoded.as_bytes()).expect("valid");
            *mutant
                .pairwise_cosine_upper
                .first_mut()
                .expect("row")
                .first_mut()
                .expect("component") = value;
            assert!(mutant.validate().is_err());
        }
        let mut zero = ParityFixture::parse(encoded.as_bytes()).expect("valid");
        zero.vectors.first_mut().expect("vector").fill(0.0);
        *zero.norms.first_mut().expect("norm") = 0.0;
        assert!(zero.validate().is_err());
    }
}
