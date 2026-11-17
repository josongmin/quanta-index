//! Canonical compact File authority root, independent of its engine readers.
use super::{object_codec::MapRoot, run_registry::RunRegistry};
use crate::channel_payloads::{decode_cbor_exact, encode_cbor, leading_cbor_array_version};
use crate::lexical_root_names::MAX_ROOT_BYTES;
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};
pub(crate) const FILE_AUTHORITY_FORMAT: u16 = 22;
pub(crate) fn policy_digest() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-authority-v22-packed-run-postings-policy\0");
    hasher.update(super::file_policy::policy_digest_for_paged());
    hasher.finalize().into()
}
pub(crate) fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("file authority v17: {reason}"))
}
pub(crate) fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file authority v17: {reason}"),
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityRoot {
    pub(crate) policy_sha256: [u8; 32],
    pub(crate) allocation_scope_sha256: [u8; 32],
    pub(crate) source_heads: MapRoot,
    pub(crate) current_docs: MapRoot,
    pub(crate) memberships: MapRoot,
    pub(crate) segments: MapRoot,
    pub(crate) run_registry: RunRegistry,
    /// Authenticated physical-delta registry before an optional whole-cohort
    /// transfer. This is a seal witness, never the query ownership authority.
    pub(crate) premerge_run_registry: Option<RunRegistry>,
    pub(crate) run_postings: MapRoot,
    /// Sum of per-run logical posting bundles. Shared physical objects can
    /// appear in more than one run and are intentionally counted per owner.
    pub(crate) run_index_encoded_bytes: u64,
    pub(crate) compaction_debt: MapRoot,
    pub(crate) path_routes: MapRoot,
    pub(crate) content_routes: MapRoot,
    pub(crate) live_sources: u64,
    pub(crate) live_docs: u64,
    pub(crate) source_bytes: u64,
    pub(crate) membership_count: u64,
}

type RootWire = (
    u16,
    [u8; 32],
    [u8; 32],
    MapRoot,
    MapRoot,
    MapRoot,
    MapRoot,
    RunRegistry,
    (Option<RunRegistry>, MapRoot, u64),
    MapRoot,
    MapRoot,
    MapRoot,
    u64,
    u64,
    u64,
    u64,
);

impl AuthorityRoot {
    pub(crate) fn wire(&self) -> RootWire {
        (
            FILE_AUTHORITY_FORMAT,
            self.policy_sha256,
            self.allocation_scope_sha256,
            self.source_heads.clone(),
            self.current_docs.clone(),
            self.memberships.clone(),
            self.segments.clone(),
            self.run_registry.clone(),
            (
                self.premerge_run_registry.clone(),
                self.run_postings.clone(),
                self.run_index_encoded_bytes,
            ),
            self.compaction_debt.clone(),
            self.path_routes.clone(),
            self.content_routes.clone(),
            self.live_sources,
            self.live_docs,
            self.source_bytes,
            self.membership_count,
        )
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = encode_cbor(&self.wire(), "file authority v22 root")?;
        if bytes.len() > MAX_ROOT_BYTES {
            return Err(invalid("root exceeds byte admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_ROOT_BYTES {
            return Err(corrupt("root exceeds byte admission"));
        }
        let format = leading_cbor_array_version(bytes)
            .map_err(|error| corrupt(&format!("root format decode: {error}")))?;
        if format != u32::from(FILE_AUTHORITY_FORMAT) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!("file authority format {format} requires rebuild"),
            });
        }
        let (
            _format,
            policy_sha256,
            allocation_scope_sha256,
            source_heads,
            current_docs,
            memberships,
            segments,
            run_registry,
            (premerge_run_registry, run_postings, run_index_encoded_bytes),
            compaction_debt,
            path_routes,
            content_routes,
            live_sources,
            live_docs,
            source_bytes,
            membership_count,
        ): RootWire =
            decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("root decode: {error}")))?;
        let root = Self {
            policy_sha256,
            allocation_scope_sha256,
            source_heads,
            current_docs,
            memberships,
            segments,
            run_registry,
            premerge_run_registry,
            run_postings,
            run_index_encoded_bytes,
            compaction_debt,
            path_routes,
            content_routes,
            live_sources,
            live_docs,
            source_bytes,
            membership_count,
        };
        root.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if root.encode()? != bytes {
            return Err(corrupt("root encoding is not canonical"));
        }
        Ok(root)
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        self.run_registry.validate()?;
        if let Some(premerge) = &self.premerge_run_registry {
            premerge.validate()?;
            if premerge.segment_owners.entries != self.segments.entries {
                return Err(invalid("premerge registry segment count differs"));
            }
        }
        if self.policy_sha256 != policy_digest()
            || self.allocation_scope_sha256 == [0; 32]
            || self.live_sources > super::file_policy::policy().source_files
            || self.source_bytes > super::file_policy::policy().source_bytes
            || self.membership_count > super::file_policy::policy().total_memberships
            || self.live_docs != self.live_sources
            || self.source_heads.entries != self.live_sources
            || self.current_docs.entries != self.live_docs
            || self.memberships.entries != self.live_docs
            || self.segments.entries > self.live_docs
            || self.run_registry.segment_owners.entries != self.segments.entries
            || self.run_postings.entries != self.run_registry.runs.entries
            || (self.run_postings.entries == 0) != (self.run_index_encoded_bytes == 0)
            || self.compaction_debt.entries > self.segments.entries
        {
            return Err(invalid(
                "root summary exceeds policy or map cardinality differs",
            ));
        }
        for root in [
            &self.source_heads,
            &self.current_docs,
            &self.memberships,
            &self.segments,
            &self.run_registry.runs,
            &self.run_registry.segment_owners,
            &self.run_postings,
            &self.compaction_debt,
            &self.path_routes,
            &self.content_routes,
        ] {
            if root.page.is_some() != (root.entries != 0) {
                return Err(invalid("map root page and entry count differ"));
            }
        }
        Ok(())
    }

    pub(crate) fn object_roots(&self) -> Vec<super::object_codec::ObjectRef> {
        let mut roots = [
            &self.source_heads,
            &self.current_docs,
            &self.memberships,
            &self.segments,
            &self.run_postings,
            &self.compaction_debt,
            &self.path_routes,
            &self.content_routes,
        ]
        .into_iter()
        .filter_map(|map| map.page.as_ref().map(|page| page.object.clone()))
        .collect::<Vec<_>>();
        roots.extend(self.run_registry.object_roots());
        if let Some(premerge) = &self.premerge_run_registry {
            roots.extend(premerge.object_roots());
        }
        roots
    }
}
