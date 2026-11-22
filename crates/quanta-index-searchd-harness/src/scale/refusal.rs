//! Failure artifact authority shared by Native scale and source fixtures.

use std::path::Path;

use anyhow::Result as AnyResult;
use serde_json::{Value, json};

use crate::artifact::{GitHeadV1, HostV1};
use crate::native_index;
use crate::scale_failure::{ScaleAdmissionLimit, ScaleRuntimeFailure, ScaleStageError};
use crate::scale_provenance::{DIMENSION, ScaleSourceBinding};
use crate::source_fixture::ScaleTier;

#[must_use]
pub fn refusal_json(
    binding: &ScaleSourceBinding,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
) -> Value {
    refusal_json_with_context(binding, git_head, host, error, DIMENSION, None)
}

/// Common failure record for rails sharing the scale source fixture. The
/// execution context names the request schedule without changing scale's
/// existing refusal schema when it is absent.
pub fn refusal_json_with_context(
    binding: &ScaleSourceBinding,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
    dimension: &str,
    execution: Option<&Value>,
) -> Value {
    let runtime_failure = error.downcast_ref::<ScaleRuntimeFailure>();
    let mut primary = error;
    let mut secondary_failures = Vec::new();
    while let Some(failure) = primary.downcast_ref::<ScaleRuntimeFailure>() {
        secondary_failures.push(json!({
            "context": failure.cleanup_context,
            "message": format!("{:#}", failure.cleanup),
        }));
        primary = failure.primary.as_ref().unwrap_or(&failure.cleanup);
    }
    let stage = primary.downcast_ref::<ScaleStageError>();
    let mut failure = serde_json::Map::new();
    let _previous = failure.insert(
        "stage".to_string(),
        json!(stage.map_or("execution_unclassified", |failure| failure.stage)),
    );
    let _previous = failure.insert(
        "limit".to_string(),
        json!(
            stage
                .and_then(|failure| failure.limit)
                .map(ScaleAdmissionLimit::as_str)
        ),
    );
    let _previous = failure.insert(
        "observed".to_string(),
        json!(stage.and_then(|failure| failure.observed)),
    );
    let _previous = failure.insert(
        "maximum".to_string(),
        json!(stage.and_then(|failure| failure.maximum)),
    );
    let _previous = failure.insert("message".to_string(), json!(format!("{primary:#}")));
    if let Some(runtime_failure) = runtime_failure {
        let _previous = failure.insert(
            "primary".to_string(),
            runtime_failure.primary.as_ref().map_or(
                Value::Null,
                |error| json!({ "message": format!("{error:#}") }),
            ),
        );
        let _previous = failure.insert(
            "cleanup".to_string(),
            json!({
                "context": runtime_failure.cleanup_context,
                "message": format!("{:#}", runtime_failure.cleanup),
            }),
        );
        let _previous = failure.insert("secondary_failures".to_string(), json!(secondary_failures));
    }
    if dimension == "scale-index-native"
        && let Some(prefix) = error.downcast_ref::<native_index::NativeFailurePrefix>()
    {
        let _previous = failure.insert("phase_prefix".to_string(), prefix.phases.clone());
    }
    if dimension == "scale-index-native"
        && let Some(captures) = native_index::failure_capture_detail(error)
    {
        let _previous = failure.insert("child_trace_files".to_string(), captures);
    }
    let mut record = serde_json::Map::new();
    let _previous = record.insert(
        "kind".to_string(),
        json!(format!("quanta-index-{dimension}-failure")),
    );
    let _previous = record.insert("schema_version".to_string(), json!(1));
    let _previous = record.insert(
        "status".to_string(),
        json!(
            if runtime_failure.is_none() && stage.and_then(|failure| failure.limit).is_some() {
                "refused"
            } else {
                "failed"
            }
        ),
    );
    let _previous = record.insert(
        "source".to_string(),
        json!({
            "tier": binding.tier.as_str(),
            "seed": binding.seed,
            "serving_owner_count": binding.serving_owner_count,
            "source_repo_count": binding.source_repo_count,
            "file_count": binding.file_count,
            "corpus_bytes": binding.corpus_bytes,
            "source_bytes": binding.source_bytes,
            "corpus_digest": binding.corpus_digest,
            "corpus_digest_kind": if dimension == "scale-index-native" {
                "generated_scoped_source_repo_path_v1"
            } else if binding.tier == ScaleTier::Small {
                "generated_chunk_content_prefixed_path_v1"
            } else {
                "generated_chunk_content_source_repo_path_v1"
            },
        }),
    );
    let _previous = record.insert(
        "provenance".to_string(),
        json!({
            "git_head": git_head.as_str(),
            "host": host,
        }),
    );
    let _previous = record.insert("failure".to_string(), Value::Object(failure));
    if let Some(execution) = execution {
        let _previous = record.insert("execution".to_string(), execution.clone());
    }
    Value::Object(record)
}

pub fn write_refusal_artifact(
    binding: &ScaleSourceBinding,
    dir: &Path,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
) -> AnyResult<()> {
    write_refusal_artifact_with_context(binding, dir, git_head, host, error, DIMENSION, None)
}

pub fn write_refusal_artifact_with_context(
    binding: &ScaleSourceBinding,
    dir: &Path,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
    dimension: &str,
    execution: Option<&Value>,
) -> AnyResult<()> {
    crate::artifact::write_json_pretty_noclobber(
        &dir.join("refusal.json"),
        &refusal_json_with_context(binding, git_head, host, error, dimension, execution),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{refusal_json, refusal_json_with_context};
    use crate::artifact::{GitHeadV1, HostV1};
    use crate::scale_failure::{ScaleStageError, stage_or_preserve};
    use crate::scale_provenance::{
        source_binding_for_failure, source_binding_for_failure_in_dimension,
    };
    use crate::source_fixture::{MAX_SCALE_SOURCE_BYTES, ScaleTier, check_admission_counts};

    #[test]
    fn refusal_record_binds_source_and_only_names_typed_limits() -> anyhow::Result<()> {
        let binding = source_binding_for_failure(ScaleTier::Medium, 13)?;
        assert_eq!(binding.source_repo_count, 4);
        assert_eq!(binding.file_count, 256);
        assert_eq!(binding.source_bytes, binding.corpus_bytes + 256);
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let capacity = check_admission_counts(1, MAX_SCALE_SOURCE_BYTES + 1, 1)
            .expect_err("source bytes exceed the fixed bound");
        let typed = anyhow::Error::new(ScaleStageError::source_admission(&anyhow::Error::new(
            capacity,
        )));
        let refused = refusal_json(&binding, &head, &host, &typed);
        assert_eq!(refused["status"], "refused");
        assert_eq!(refused["failure"]["stage"], "source_preflight");
        assert_eq!(
            refused["failure"]["limit"],
            "lexical_total_source_bytes_128m"
        );
        assert_eq!(refused["failure"]["observed"], MAX_SCALE_SOURCE_BYTES + 1);
        assert_eq!(refused["source"]["corpus_digest"], binding.corpus_digest);
        assert!(refused.get("latency").is_none());

        let other = source_binding_for_failure_in_dimension("open-loop", ScaleTier::Medium, 13)?;
        assert_ne!(other.corpus_digest, binding.corpus_digest);
        let execution = json!({"arrival_model": "seeded_poisson", "rates_qps": [25, 50]});
        let open_loop =
            refusal_json_with_context(&other, &head, &host, &typed, "open-loop", Some(&execution));
        assert_eq!(open_loop["kind"], "quanta-index-open-loop-failure");
        assert_eq!(open_loop["execution"], execution);
        assert_eq!(open_loop["failure"]["limit"], refused["failure"]["limit"]);

        let unknown = refusal_json(&binding, &head, &host, &anyhow::anyhow!("runtime failure"));
        assert_eq!(unknown["status"], "failed");
        assert_eq!(unknown["failure"]["stage"], "execution_unclassified");
        assert!(unknown["failure"]["limit"].is_null());
        let ingest = refusal_json(
            &binding,
            &head,
            &host,
            &stage_or_preserve("build_ingest", anyhow::anyhow!("fixed ingest failure")),
        );
        assert_eq!(ingest["failure"]["stage"], "build_ingest");
        assert!(ingest["failure"]["limit"].is_null());
        Ok(())
    }
}
