//! Canonical JSON authority shared by batch digests and runner records.

use serde_json::Value;

use crate::{BenchError, BenchResult, sha256_hex};

/// The timing-independent result representation shared with the Python phase
/// reader.
///
/// Scored candidate floats are rendered as fixed-width lowercase IEEE-754
/// bits. The explicit `hybrid_file` route also projects its typed file and
/// witness scores while excluding response-local explanation observations.
pub const REQUIRED_RESPONSE_OUTPUT_VALIDATION: &str = "normalized_row_score_bits_sha256_v1";

pub fn required_response_sha256(row: &Value) -> BenchResult<String> {
    Ok(sha256_hex(
        required_response_canonical_json(row)?.as_bytes(),
    ))
}

fn normalize_score_bits(score: &mut Value, error: &'static str) -> BenchResult<()> {
    let bits = score
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| BenchError::Protocol(error.to_string()))?
        .to_bits();
    *score = Value::String(format!("{bits:016x}"));
    Ok(())
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
            normalize_score_bits(
                score,
                "required response candidate score is not finite numeric",
            )?;
        }
    }
    if object.get("route").and_then(Value::as_str) == Some("hybrid_file")
        && let Some(evidence) = object.get_mut("hybrid_file_evidence")
    {
        let response = evidence
            .get_mut("response")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                BenchError::Protocol(
                    "required response hybrid file evidence lacks typed response".to_string(),
                )
            })?;
        if response.remove("explanation").is_none() {
            return Err(BenchError::Protocol(
                "required response hybrid file evidence lacks explanation".to_string(),
            ));
        }
        let results = response
            .get_mut("results")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                BenchError::Protocol(
                    "required response hybrid file evidence lacks results".to_string(),
                )
            })?;
        for hit in results {
            let hit = hit.as_object_mut().ok_or_else(|| {
                BenchError::Protocol(
                    "required response hybrid file hit is not an object".to_string(),
                )
            })?;
            let score = hit.get_mut("file_rrf_score").ok_or_else(|| {
                BenchError::Protocol(
                    "required response hybrid file hit lacks RRF score".to_string(),
                )
            })?;
            normalize_score_bits(score, "required response hybrid file RRF score is invalid")?;
            for lane in ["lexical", "dense"] {
                let witness = hit.get_mut(lane).ok_or_else(|| {
                    BenchError::Protocol(format!(
                        "required response hybrid file hit lacks {lane} witness field"
                    ))
                })?;
                if witness.is_null() {
                    continue;
                }
                let score = witness
                    .get_mut("candidate")
                    .and_then(Value::as_object_mut)
                    .and_then(|candidate| candidate.get_mut("score"))
                    .ok_or_else(|| {
                        BenchError::Protocol(format!(
                            "required response hybrid file {lane} witness lacks candidate score"
                        ))
                    })?;
                normalize_score_bits(
                    score,
                    "required response hybrid file witness candidate score is invalid",
                )?;
            }
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
        let first_score = row
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
            .and_then(|candidate| candidate.get("score"))
            .and_then(Value::as_f64)
            .expect("fixed response first candidate has a numeric score");
        assert_eq!(first_score.to_bits(), (-0.0_f64).to_bits());
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

    #[test]
    fn hybrid_file_required_response_has_independent_typed_evidence_golden() {
        let row: Value = serde_json::from_str(
            r#"{"route":"hybrid_file","status":"success","candidates":[{"path":"src/a.rs","score":0.5}],"timings":{"query_latency_ms":1.25},"hybrid_file_evidence":{"schema_version":1,"lane_top_k":8,"file_top_k":3,"response":{"generation":{"repo_id":"r"},"file_top_k":3,"bounded_file_count":1,"lexical_pool_sha256":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"dense_pool_sha256":[2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2],"results":[{"file":{"source_repo_id":"r","repo_relative_path":"src/a.rs"},"file_rrf_score":0.5,"lexical":{"file_rank":1,"candidate":{"score":-0.0}},"dense":null}],"lexical_window":{"returned":1},"dense_window":{"returned":0},"explanation":{"request_id":7,"stage_timings":[1.5]}}}}"#,
        )
        .expect("fixed hybrid file response parses");
        assert_eq!(
            required_response_canonical_json(&row).expect("typed response canonicalizes"),
            r#"{"candidates":[{"path":"src/a.rs","score":"3fe0000000000000"}],"hybrid_file_evidence":{"file_top_k":3,"lane_top_k":8,"response":{"bounded_file_count":1,"dense_pool_sha256":[2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2],"dense_window":{"returned":0},"file_top_k":3,"generation":{"repo_id":"r"},"lexical_pool_sha256":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"lexical_window":{"returned":1},"results":[{"dense":null,"file":{"repo_relative_path":"src/a.rs","source_repo_id":"r"},"file_rrf_score":"3fe0000000000000","lexical":{"candidate":{"score":"8000000000000000"},"file_rank":1}}]},"schema_version":1},"route":"hybrid_file","status":"success"}"#
        );
        let digest = required_response_sha256(&row).expect("typed response hashes");
        assert_eq!(
            digest,
            "bc83e7d83d8f9fb08d27db4bb71a197f5167d0cbfdc4a23c379516399aa7fd08"
        );

        let mut observation_changed = row.clone();
        observation_changed["timings"]["query_latency_ms"] = serde_json::json!(99.5);
        observation_changed["hybrid_file_evidence"]["response"]["explanation"] =
            serde_json::json!({"request_id": 999, "stage_timings": [98.5]});
        assert_eq!(
            required_response_sha256(&observation_changed).expect("same semantic response"),
            digest
        );

        let mut dense_witness = row.clone();
        dense_witness["hybrid_file_evidence"]["response"]["results"][0]["dense"] =
            serde_json::json!({"file_rank": 1, "candidate": {"score": 1.25}});
        assert!(
            required_response_canonical_json(&dense_witness)
                .expect("both witnesses canonicalize")
                .contains("\"dense\":{\"candidate\":{\"score\":\"3ff4000000000000\"}")
        );

        for pointer in [
            "/hybrid_file_evidence/response/results/0/file_rrf_score",
            "/hybrid_file_evidence/response/results/0/lexical/candidate/score",
            "/hybrid_file_evidence/response/results/0/lexical/file_rank",
            "/hybrid_file_evidence/response/results/0/file/source_repo_id",
            "/hybrid_file_evidence/response/lexical_window/returned",
            "/hybrid_file_evidence/response/file_top_k",
            "/hybrid_file_evidence/response/bounded_file_count",
            "/hybrid_file_evidence/response/lexical_pool_sha256",
            "/hybrid_file_evidence/response/dense_pool_sha256",
            "/hybrid_file_evidence/file_top_k",
        ] {
            let mut changed = row.clone();
            *changed
                .pointer_mut(pointer)
                .expect("fixed semantic witness") = if pointer.ends_with("source_repo_id") {
                Value::from("other-repo")
            } else if pointer.ends_with("pool_sha256") {
                Value::Array(vec![Value::from(0); 32])
            } else if pointer.ends_with("file_rank")
                || pointer.ends_with("returned")
                || pointer.ends_with("bounded_file_count")
                || pointer.ends_with("file_top_k")
            {
                Value::from(2)
            } else {
                serde_json::json!(0.75)
            };
            assert_ne!(
                required_response_sha256(&changed).expect("changed semantic witness hashes"),
                digest,
                "{pointer} must remain in required response digest"
            );
        }

        let mut unrelated_float = row.clone();
        unrelated_float["hybrid_file_evidence"]["response"]["results"][0]["other"] =
            serde_json::json!(1.5);
        assert!(required_response_sha256(&unrelated_float).is_err());
        let mut old_route = row.clone();
        old_route["route"] = Value::from("hybrid");
        assert!(required_response_sha256(&old_route).is_err());
        let failure = serde_json::json!({
            "route": "hybrid_file", "status": "error", "candidates": [],
            "error": {"code": "sdk_usage"}, "timings": {"query_latency_ms": 1.5},
        });
        assert!(required_response_sha256(&failure).is_ok());
    }
}
