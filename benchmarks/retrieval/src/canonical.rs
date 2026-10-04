//! Canonical JSON authority shared by batch digests and runner records.

use serde_json::Value;

use crate::{BenchError, BenchResult, sha256_hex};

/// The timing-independent result representation shared with the Python phase
/// reader. Only candidate scores may be floating point; their IEEE-754 bits
/// are rendered as fixed-width lowercase hex before canonical JSON encoding.
pub const REQUIRED_RESPONSE_OUTPUT_VALIDATION: &str = "normalized_row_score_bits_sha256_v1";

pub fn required_response_sha256(row: &Value) -> BenchResult<String> {
    Ok(sha256_hex(
        required_response_canonical_json(row)?.as_bytes(),
    ))
}

fn required_response_canonical_json(row: &Value) -> BenchResult<String> {
    let mut normalized = row.clone();
    let object = normalized
        .as_object_mut()
        .ok_or_else(|| BenchError::Protocol("required response is not an object".to_string()))?;
    let _removed_timing = object.remove("timings");
    let candidates = object
        .get_mut("candidates")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            BenchError::Protocol("required response candidates are missing".to_string())
        })?;
    for candidate in candidates {
        let candidate = candidate.as_object_mut().ok_or_else(|| {
            BenchError::Protocol("required response candidate is not an object".to_string())
        })?;
        if let Some(score) = candidate.get_mut("score") {
            let score_bits = score
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    BenchError::Protocol(
                        "required response candidate score is not finite numeric".to_string(),
                    )
                })?;
            *score = Value::String(format!("{:016x}", score_bits.to_bits()));
        }
    }
    canonical_json(&normalized)
}

/// Canonical JSON: sorted keys, no whitespace, raw UTF-8, no floats.
/// Matches `evaluator.canonical` for the pack's string/int-only domain;
/// floats are refused rather than format-guessed.
pub fn canonical_json(value: &Value) -> BenchResult<String> {
    fn render(value: &Value, out: &mut String) -> BenchResult<()> {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(number) => {
                if number.is_f64() {
                    return Err(BenchError::Protocol(
                        "canonical JSON refuses floats (Python float formatting would diverge)"
                            .to_string(),
                    ));
                }
                out.push_str(&number.to_string());
            }
            Value::String(text) => {
                let rendered = serde_json::to_string(text).map_err(|err| BenchError::Json {
                    path: "<pack>".to_string(),
                    message: err.to_string(),
                })?;
                out.push_str(&rendered);
            }
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    render(item, out)?;
                }
                out.push(']');
            }
            Value::Object(object) => {
                out.push('{');
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort();
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let rendered_key =
                        serde_json::to_string(key).map_err(|err| BenchError::Json {
                            path: "<pack>".to_string(),
                            message: err.to_string(),
                        })?;
                    out.push_str(&rendered_key);
                    out.push(':');
                    if let Some(child) = object.get(key.as_str()) {
                        render(child, out)?;
                    }
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut out = String::new();
    render(value, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_sorts_keys_and_holds_utf8() {
        let value: Value = serde_json::from_str(r#"{"b":1,"a":[2,{"z":null,"m":"héllo"}]}"#)
            .expect("fixture parses");
        let rendered = canonical_json(&value).expect("canonical renders");
        assert_eq!(rendered, r#"{"a":[2,{"m":"héllo","z":null}],"b":1}"#);
    }

    #[test]
    fn canonical_form_refuses_floats() {
        let value: Value = serde_json::from_str(r#"{"a":1.5}"#).expect("fixture parses");
        assert!(canonical_json(&value).is_err());
    }

    #[test]
    fn required_response_has_independent_unicode_numeric_and_signed_zero_golden() {
        let row: Value = serde_json::from_str(
            r#"{"task_id":"Té","timings":{"query_latency_ms":1.25},"status":"success","candidates":[{"path":"café.go","score":-0.0},{"score":1e-7,"path":"雪.go"},{"score":2,"path":"b.go"}]}"#,
        )
        .expect("fixed response parses");
        let digest = required_response_sha256(&row).expect("fixed response hashes");
        assert_eq!(
            required_response_canonical_json(&row).expect("fixed response canonicalizes"),
            r#"{"candidates":[{"path":"café.go","score":"8000000000000000"},{"path":"雪.go","score":"3e7ad7f29abcaf48"},{"path":"b.go","score":"4000000000000000"}],"status":"success","task_id":"Té"}"#
        );
        assert_eq!(
            digest,
            "8b1dd6a4b49b147489e1f2a2a4460832df183732852d07982ce2a48f0e0695b5"
        );
        let mut without_timing = row.clone();
        assert!(
            without_timing
                .as_object_mut()
                .expect("object")
                .remove("timings")
                .is_some()
        );
        assert_eq!(
            required_response_sha256(&without_timing).expect("same response"),
            digest
        );
        assert_eq!(row["candidates"][0]["score"].as_f64(), Some(-0.0));
    }

    #[test]
    fn required_response_rejects_unrelated_float_and_bad_scores() {
        for row in [
            serde_json::json!({"candidates": [], "unrelated": 1.25}),
            serde_json::json!({"candidates": [{"score": "1.0"}]}),
            serde_json::json!({"candidates": [false]}),
            serde_json::json!({"status": "success"}),
        ] {
            assert!(required_response_sha256(&row).is_err());
        }
    }
}
