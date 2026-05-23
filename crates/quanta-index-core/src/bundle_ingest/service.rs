use quanta_index_contract::PreparedBundleOutbox;

use crate::CoreError;

pub struct BundlePolicy;

impl BundlePolicy {
    pub fn validate_outbox(outbox: &PreparedBundleOutbox) -> Result<(), CoreError> {
        if outbox.outbox_id.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "outbox_id must not be empty".into(),
            ));
        }

        if outbox.manifest_ref.relative_path.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "manifest_ref.relative_path must not be empty".into(),
            ));
        }

        if outbox.bundle_schema_version == 0 {
            return Err(CoreError::InvalidContract(
                "bundle_schema_version must be non-zero".into(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        BundleArtifactRef, BundleEncoding, BundleMode, ManifestDigest, PreparedBundleOutbox,
        RepoId, RevisionId,
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
}
