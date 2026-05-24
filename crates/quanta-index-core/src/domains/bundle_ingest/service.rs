use quanta_index_contract::{
    BundleArtifactRef, PreparedBundleOutbox, PublishedSearchBundleDeltaApplyRequest,
    SearchBundleMutationOp,
};

use crate::CoreError;

pub struct BundlePolicy;

impl BundlePolicy {
    pub fn validate_outbox(outbox: &PreparedBundleOutbox) -> Result<(), CoreError> {
        if outbox.outbox_id.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "outbox_id must not be empty".into(),
            ));
        }

        Self::validate_artifact_ref("manifest_ref", &outbox.manifest_ref)?;

        if outbox.bundle_schema_version == 0 {
            return Err(CoreError::InvalidContract(
                "bundle_schema_version must be non-zero".into(),
            ));
        }

        Ok(())
    }

    /// Validate an `BundleArtifactRef` shape used as a payload pointer.
    /// `context` is included in error messages to identify which ref failed.
    pub fn validate_artifact_ref(
        context: &str,
        reference: &BundleArtifactRef,
    ) -> Result<(), CoreError> {
        if reference.relative_path.trim().is_empty() {
            return Err(CoreError::InvalidContract(format!(
                "{context}.relative_path must not be empty"
            )));
        }
        if reference.byte_length == 0 {
            return Err(CoreError::InvalidContract(format!(
                "{context}.byte_length must be non-zero"
            )));
        }
        if reference.content_digest.as_str().trim().is_empty() {
            return Err(CoreError::InvalidContract(format!(
                "{context}.content_digest must not be empty"
            )));
        }
        Ok(())
    }

    /// Validate a delta-apply request shape. Defers row-level effects to the
    /// adapter (transactional INSERT OR IGNORE handles idempotency), but
    /// rejects requests where the delta envelope is inconsistent with the
    /// request's (repo, rev, generation) target before any SQL runs.
    pub fn validate_delta(
        request: &PublishedSearchBundleDeltaApplyRequest,
    ) -> Result<(), CoreError> {
        if request.delta.repo_id != request.repo_id {
            return Err(CoreError::InvalidContract(
                "delta.repo_id does not match request.repo_id".into(),
            ));
        }
        if request.delta.revision_id != request.revision_id {
            return Err(CoreError::InvalidContract(
                "delta.revision_id does not match request.revision_id".into(),
            ));
        }
        if request.delta.manifest_generation != request.generation.manifest_generation {
            return Err(CoreError::InvalidContract(
                "delta.manifest_generation does not match request.generation.manifest_generation"
                    .into(),
            ));
        }
        for (index, op) in request.delta.operations.iter().enumerate() {
            Self::validate_delta_op(index, op)?;
        }
        Ok(())
    }

    fn validate_delta_op(index: usize, op: &SearchBundleMutationOp) -> Result<(), CoreError> {
        match op {
            SearchBundleMutationOp::UpsertChunk {
                chunk_identity,
                text_digest,
            } => {
                Self::require_nonempty(index, "UpsertChunk.chunk_identity", chunk_identity)?;
                Self::require_nonempty(index, "UpsertChunk.text_digest", text_digest)?;
            }
            SearchBundleMutationOp::DeleteChunk { chunk_identity } => {
                Self::require_nonempty(index, "DeleteChunk.chunk_identity", chunk_identity)?;
            }
            SearchBundleMutationOp::UpsertSymbol {
                symbol_id,
                symbol_digest,
            } => {
                Self::require_nonempty(index, "UpsertSymbol.symbol_id", symbol_id)?;
                Self::require_nonempty(index, "UpsertSymbol.symbol_digest", symbol_digest)?;
            }
            SearchBundleMutationOp::DeleteSymbol { symbol_id } => {
                Self::require_nonempty(index, "DeleteSymbol.symbol_id", symbol_id)?;
            }
            SearchBundleMutationOp::UpsertEmbedding {
                entity_id,
                input_digest,
            } => {
                Self::require_nonempty(index, "UpsertEmbedding.entity_id", entity_id)?;
                Self::require_nonempty(index, "UpsertEmbedding.input_digest", input_digest)?;
            }
            SearchBundleMutationOp::DeleteEmbedding { entity_id } => {
                Self::require_nonempty(index, "DeleteEmbedding.entity_id", entity_id)?;
            }
        }
        Ok(())
    }

    fn require_nonempty(index: usize, field: &str, value: &str) -> Result<(), CoreError> {
        if value.trim().is_empty() {
            return Err(CoreError::InvalidContract(format!(
                "delta.operations[{index}].{field} must not be empty"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, ManifestDigest,
        ManifestGeneration, PreparedBundleOutbox, PublishedGenerationSet,
        PublishedSearchBundleDeltaApplyRequest, RepoId, RevisionId, SearchBundleMutationDelta,
        SearchBundleMutationOp,
    };

    use super::BundlePolicy;
    use crate::CoreError;

    #[test]
    fn rejects_empty_outbox_id() {
        let result = BundlePolicy::validate_outbox(&sample_outbox("", "bundle/manifest.json", 1));
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn rejects_empty_manifest_relative_path() {
        let result = BundlePolicy::validate_outbox(&sample_outbox("outbox-1", "", 1));
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn rejects_zero_bundle_schema_version() {
        let result =
            BundlePolicy::validate_outbox(&sample_outbox("outbox-1", "bundle/manifest.json", 0));
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn rejects_zero_byte_length_artifact_ref() {
        let bad = BundleArtifactRef {
            relative_path: "bundle/manifest.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 0,
            content_digest: ManifestDigest::new("digest"),
        };
        let result = BundlePolicy::validate_artifact_ref("manifest_ref", &bad);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn rejects_empty_digest_artifact_ref() {
        let bad = BundleArtifactRef {
            relative_path: "bundle/manifest.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 128,
            content_digest: ManifestDigest::new(""),
        };
        let result = BundlePolicy::validate_artifact_ref("manifest_ref", &bad);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn delta_validation_rejects_repo_id_mismatch() {
        let mut request = sample_delta_request();
        request.delta.repo_id = RepoId::new("other-repo");
        let result = BundlePolicy::validate_delta(&request);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn delta_validation_rejects_revision_id_mismatch() {
        let mut request = sample_delta_request();
        request.delta.revision_id = RevisionId::new("other-rev");
        let result = BundlePolicy::validate_delta(&request);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn delta_validation_rejects_generation_mismatch() {
        let mut request = sample_delta_request();
        request.delta.manifest_generation = ManifestGeneration::new(99);
        let result = BundlePolicy::validate_delta(&request);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn delta_validation_rejects_empty_op_fields() {
        let mut request = sample_delta_request();
        request.delta.operations = vec![SearchBundleMutationOp::UpsertChunk {
            chunk_identity: "   ".into(),
            text_digest: "digest".into(),
        }];
        let result = BundlePolicy::validate_delta(&request);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn delta_validation_accepts_consistent_request() {
        let request = sample_delta_request();
        let result = BundlePolicy::validate_delta(&request);
        assert!(result.is_ok(), "unexpected result: {result:?}");
    }

    #[test]
    fn delta_validation_accepts_empty_operations() {
        let mut request = sample_delta_request();
        request.delta.operations = vec![];
        let result = BundlePolicy::validate_delta(&request);
        assert!(result.is_ok(), "unexpected result: {result:?}");
    }

    fn sample_outbox(
        outbox_id: &str,
        relative_path: &str,
        bundle_schema_version: u32,
    ) -> PreparedBundleOutbox {
        PreparedBundleOutbox {
            outbox_id: outbox_id.into(),
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_digest: ManifestDigest::new("digest"),
            bundle_schema_version,
            prepared_at_ms: 1,
            mode: BundleMode::ServeOnly,
            manifest_ref: BundleArtifactRef {
                relative_path: relative_path.into(),
                encoding: BundleEncoding::Json,
                byte_length: 128,
                content_digest: ManifestDigest::new("digest"),
            },
            base_generation: None,
            changed_artifact_mask: 1,
        }
    }

    fn sample_delta_request() -> PublishedSearchBundleDeltaApplyRequest {
        let generation = PublishedGenerationSet {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            lexical_generation: GenerationId::new(10),
            symbol_generation: GenerationId::new(11),
            structural_generation: None,
            history_generation: None,
            semantic_generation: None,
            metadata_generation: None,
        };
        let delta = SearchBundleMutationDelta {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            operations: vec![
                SearchBundleMutationOp::UpsertChunk {
                    chunk_identity: "chunk-1".into(),
                    text_digest: "tdig".into(),
                },
                SearchBundleMutationOp::DeleteSymbol {
                    symbol_id: "sym-1".into(),
                },
            ],
        };
        PublishedSearchBundleDeltaApplyRequest {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            generation,
            delta,
        }
    }
}
