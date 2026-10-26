//! File authority V20: bounded immutable leaves and run posting authority.
//!
//! The root contains only authenticated map descriptors. Source bodies and
//! posting pages are selected through the shared object store. A map value
//! carries the complete source identity so a digest-key collision is refused.

mod producer;
mod reader;
mod run_postings;
mod run_verify;
mod verify;
#[cfg(test)]
pub(crate) use producer::build_for_seal;
pub(crate) use producer::{
    FileAuthorityMutation, FileSourceEdit, PreparedFileCompaction,
    build_for_seal_prepared_with_policy, prepare_compaction, prepare_hard_scan_window,
};
pub(crate) use reader::{FileMetadata, PagedFileAuthority};
#[cfg(test)]
pub(crate) use verify::verify_for_seal;
pub(crate) use verify::{
    FileScrubCursor, replay_hard_scan_window, verify_all_for_scrub,
    verify_for_seal_with_merge_facts, verify_scrub_step,
};

use quanta_index_contract::{SourceFileCoverage, SourceFileKey};
use quanta_index_core::CoreError;
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::channel_payloads::{decode_cbor_exact, encode_cbor, leading_cbor_array_version};
pub(super) use crate::lexical_root_names::MAX_ROOT_BYTES;
use crate::lexical_runs::RunRegistry;
use crate::lexical_store::{BlobRef, MapRoot, ObjectStore};

pub(super) const FILE_AUTHORITY_FORMAT: u16 = 20;
pub(super) const MAX_RECORD_BYTES: usize = 32 * 1024;
pub(super) const MAX_SEGMENT_DOCS: usize = 64;
pub(super) const MAX_POSTING_PAGE_BYTES: usize = 32 * 1024;
pub(super) const MAX_POSTING_PAGE_TERMS: usize = 1024;

struct BoundedVec<T, const LIMIT: usize>(Vec<T>);

impl<'de, T: Deserialize<'de>, const LIMIT: usize> Deserialize<'de> for BoundedVec<T, LIMIT> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundedVisitor<T, const LIMIT: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const LIMIT: usize> Visitor<'de> for BoundedVisitor<T, LIMIT> {
            type Value = BoundedVec<T, LIMIT>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "a sequence with at most {LIMIT} entries")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                if seq.size_hint().is_some_and(|count| count > LIMIT) {
                    return Err(serde::de::Error::custom("sequence count exceeds admission"));
                }
                let mut rows = Vec::new();
                while rows.len() < LIMIT {
                    let Some(row) = seq.next_element()? else {
                        return Ok(BoundedVec(rows));
                    };
                    rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                    rows.push(row);
                }
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom("sequence count exceeds admission"));
                }
                Ok(BoundedVec(rows))
            }
        }
        deserializer.deserialize_seq(BoundedVisitor::<T, LIMIT>(std::marker::PhantomData))
    }
}

/// Infallible by construction: hashes the fixed policy domain and its policy digest.
pub(super) fn policy_digest() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-authority-v20-run-postings-policy\0");
    hasher.update(super::policy_digest_for_paged());
    hasher.finalize().into()
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("file authority v17: {reason}"))
}

fn corrupt(reason: &str) -> CoreError {
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
    fn wire(&self) -> RootWire {
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
        let bytes = encode_cbor(&self.wire(), "file authority v20 root")?;
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

    fn validate(&self) -> Result<(), CoreError> {
        self.run_registry.validate()?;
        if let Some(premerge) = &self.premerge_run_registry {
            premerge.validate()?;
            if premerge.segment_owners.entries != self.segments.entries {
                return Err(invalid("premerge registry segment count differs"));
            }
        }
        if self.policy_sha256 != policy_digest()
            || self.allocation_scope_sha256 == [0; 32]
            || self.live_sources > super::policy().source_files
            || self.source_bytes > super::policy().source_bytes
            || self.membership_count > super::policy().total_memberships
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

    pub(crate) fn object_roots(&self) -> Vec<crate::lexical_store::ObjectRef> {
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

    /// Active logical index bytes include leaf segment bundles, run posting
    /// bundles, and the outer run-postings map structure. Source bodies,
    /// current/source maps, route maps, and unique pool occupancy are separate.
    pub(crate) fn maintenance_snapshot(
        &self,
        store: &ObjectStore,
    ) -> Result<quanta_index_core::lexical_maintenance::TargetCounters, CoreError> {
        let outer_structure = store.map_structure_encoded_bytes(&self.run_postings, None)?;
        let bytes = self
            .run_registry
            .active_index_encoded_bytes
            .checked_add(self.run_index_encoded_bytes)
            .and_then(|sum| sum.checked_add(outer_structure))
            .ok_or_else(|| invalid("active logical index bytes overflow"))?;
        quanta_index_core::lexical_maintenance::TargetCounters::new(
            self.run_registry.runs.entries,
            self.segments.entries,
            bytes,
        )
    }
}

/// One current source and its exact ingress coverage commitment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceHead {
    pub(crate) coverage: SourceFileCoverage,
    pub(crate) body: BlobRef,
    pub(crate) logical_id: u64,
    pub(crate) source_bytes: u64,
    pub(crate) indexed_text_bytes: u64,
    pub(crate) folded_text_bytes: u64,
    pub(crate) posting_memberships: u64,
    pub(crate) derived_input_sha256: [u8; 32],
}

impl Serialize for SourceHead {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.coverage,
            &self.body,
            self.logical_id,
            self.source_bytes,
            self.indexed_text_bytes,
            self.folded_text_bytes,
            self.posting_memberships,
            self.derived_input_sha256,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourceHead {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            coverage,
            body,
            logical_id,
            source_bytes,
            indexed_text_bytes,
            folded_text_bytes,
            posting_memberships,
            derived_input_sha256,
        ) = <(
            SourceFileCoverage,
            BlobRef,
            u64,
            u64,
            u64,
            u64,
            u64,
            [u8; 32],
        )>::deserialize(deserializer)?;
        Ok(Self {
            coverage,
            body,
            logical_id,
            source_bytes,
            indexed_text_bytes,
            folded_text_bytes,
            posting_memberships,
            derived_input_sha256,
        })
    }
}

impl SourceHead {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        self.coverage
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        let max_source_bytes = u64::try_from(super::MAX_FILE_BYTES)
            .map_err(|error| invalid(&format!("source byte limit width: {error}")))?;
        if self.logical_id == 0
            || self.body.bytes != self.source_bytes
            || self.body.sha256 != self.coverage.source.source_sha256
            || self.source_bytes > max_source_bytes
            || self.posting_memberships > super::policy().total_memberships
        {
            return Err(invalid("source head body, ID or membership differs"));
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        encode_bounded(self, MAX_RECORD_BYTES, "source head")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let head: Self = decode_bounded(bytes, MAX_RECORD_BYTES, "source head")?;
        head.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        Ok(head)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveDoc {
    pub(crate) source_key_sha256: [u8; 32],
    pub(crate) logical_id: u64,
    pub(crate) segment_sha256: [u8; 32],
    pub(crate) local_id: u32,
}

impl Serialize for LiveDoc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            self.source_key_sha256,
            self.logical_id,
            self.segment_sha256,
            self.local_id,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LiveDoc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (source_key_sha256, logical_id, segment_sha256, local_id) =
            <([u8; 32], u64, [u8; 32], u32)>::deserialize(deserializer)?;
        Ok(Self {
            source_key_sha256,
            logical_id,
            segment_sha256,
            local_id,
        })
    }
}

impl LiveDoc {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        if self.logical_id == 0 {
            return Err(invalid("zero logical ID"));
        }
        encode_bounded(self, MAX_RECORD_BYTES, "live document")
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let row: Self = decode_bounded(bytes, MAX_RECORD_BYTES, "live document")?;
        if row.logical_id == 0 {
            return Err(corrupt("zero logical ID"));
        }
        Ok(row)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Segment {
    pub(crate) doc_ids: Vec<u64>,
    pub(crate) path_terms: MapRoot,
    pub(crate) content_terms: MapRoot,
    pub(crate) path_memberships: u64,
    pub(crate) content_memberships: u64,
}

impl Serialize for Segment {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.doc_ids,
            &self.path_terms,
            &self.content_terms,
            self.path_memberships,
            self.content_memberships,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Segment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (BoundedVec(doc_ids), path_terms, content_terms, path_memberships, content_memberships) =
            <(
                BoundedVec<u64, MAX_SEGMENT_DOCS>,
                MapRoot,
                MapRoot,
                u64,
                u64,
            )>::deserialize(deserializer)?;
        Ok(Self {
            doc_ids,
            path_terms,
            content_terms,
            path_memberships,
            content_memberships,
        })
    }
}

impl Segment {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.doc_ids.is_empty()
            || self.doc_ids.len() > MAX_SEGMENT_DOCS
            || self
                .doc_ids
                .windows(2)
                .any(|pair| pair.first() >= pair.get(1))
        {
            return Err(invalid(
                "segment logical IDs are not nonempty, bounded and increasing",
            ));
        }
        if self
            .path_memberships
            .checked_add(self.content_memberships)
            .is_none_or(|total| total > super::policy().total_memberships)
        {
            return Err(invalid("segment membership exceeds policy"));
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        encode_bounded(self, MAX_RECORD_BYTES, "segment descriptor")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let segment: Self = decode_bounded(bytes, MAX_RECORD_BYTES, "segment descriptor")?;
        segment
            .validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        Ok(segment)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PostingPage(pub(crate) Vec<([u8; 3], u64)>);

impl Serialize for PostingPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PostingPage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let BoundedVec(rows) =
            BoundedVec::<([u8; 3], u64), MAX_POSTING_PAGE_TERMS>::deserialize(deserializer)?;
        Ok(Self(rows))
    }
}

impl PostingPage {
    pub(crate) fn validate(&self, doc_count: usize) -> Result<(), CoreError> {
        if self.0.is_empty()
            || self.0.len() > MAX_POSTING_PAGE_TERMS
            || self
                .0
                .windows(2)
                .any(|pair| pair.first().map(|row| row.0) >= pair.get(1).map(|row| row.0))
        {
            return Err(invalid("posting page term order or count differs"));
        }
        if doc_count == 0 || doc_count > MAX_SEGMENT_DOCS {
            return Err(invalid("posting page segment width differs"));
        }
        let remaining = MAX_SEGMENT_DOCS
            .checked_sub(doc_count)
            .ok_or_else(|| invalid("posting page shift width differs"))?;
        let shift = u32::try_from(remaining)
            .map_err(|error| invalid(&format!("posting page shift width: {error}")))?;
        let admitted = u64::MAX
            .checked_shr(shift)
            .ok_or_else(|| invalid("posting page shift exceeds mask width"))?;
        for (_, mask) in &self.0 {
            if *mask == 0 || *mask & !admitted != 0 {
                return Err(invalid("posting page local-ID bitmask exceeds segment"));
            }
        }
        Ok(())
    }

    pub(crate) fn encode(&self, doc_count: usize) -> Result<Vec<u8>, CoreError> {
        self.validate(doc_count)?;
        encode_bounded(self, MAX_POSTING_PAGE_BYTES, "posting page")
    }

    pub(crate) fn decode(bytes: &[u8], doc_count: usize) -> Result<Self, CoreError> {
        let page: Self = decode_bounded(bytes, MAX_POSTING_PAGE_BYTES, "posting page")?;
        page.validate(doc_count)
            .map_err(|error| corrupt(&error.to_string()))?;
        Ok(page)
    }
}

pub(super) fn source_key_digest(key: &SourceFileKey) -> Result<[u8; 32], CoreError> {
    key.validate().map_err(invalid)?;
    let encoded = encode_cbor(key, "file source key")?;
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-source-key-v17\0");
    hasher.update(encoded);
    Ok(hasher.finalize().into())
}

pub(super) fn doc_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

pub(super) fn membership_key(source_digest: [u8; 32], id: u64) -> [u8; 40] {
    let mut key = [0_u8; 40];
    key[..32].copy_from_slice(&source_digest);
    key[32..].copy_from_slice(&id.to_be_bytes());
    key
}

#[cfg(test)]
pub(super) fn route_key(
    surface: u8,
    gram: [u8; 3],
    posting_count: u32,
    segment: [u8; 32],
) -> [u8; 40] {
    let mut key = [0_u8; 40];
    key[0] = surface;
    key[1..4].copy_from_slice(&gram);
    key[4..8].copy_from_slice(&posting_count.to_be_bytes());
    key[8..].copy_from_slice(&segment);
    key
}

fn encode_bounded<T: Serialize>(
    value: &T,
    ceiling: usize,
    name: &str,
) -> Result<Vec<u8>, CoreError> {
    let bytes = encode_cbor(value, name)?;
    if bytes.len() > ceiling {
        return Err(invalid(&format!("{name} exceeds byte admission")));
    }
    Ok(bytes)
}

fn decode_bounded<T: for<'de> Deserialize<'de> + Serialize>(
    bytes: &[u8],
    ceiling: usize,
    name: &str,
) -> Result<T, CoreError> {
    if bytes.len() > ceiling {
        return Err(corrupt(&format!("{name} exceeds byte admission")));
    }
    let value: T =
        decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("{name} decode: {error}")))?;
    let canonical = encode_cbor(&value, name)?;
    if canonical != bytes {
        return Err(corrupt(&format!("{name} encoding is not canonical")));
    }
    Ok(value)
}

#[cfg(test)]
mod codec_tests {
    use super::{AuthorityRoot, PostingPage, Segment};
    use crate::channel_payloads::encode_cbor;
    use crate::lexical_store::MapRoot;
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_core::CoreError;

    #[test]
    fn older_root_format_is_refused_before_current_tuple_decode() {
        let old_shape = encode_cbor(&(18_u16,), "old file root fixture").expect("fixture");
        assert!(matches!(
            AuthorityRoot::decode(&old_shape),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                ..
            })
        ));
        let malformed_current =
            encode_cbor(&(19_u16,), "malformed current file root").expect("fixture");
        assert!(AuthorityRoot::decode(&malformed_current).is_err());
    }

    #[test]
    fn posting_page_rejects_noncanonical_or_impossible_memberships() {
        let valid = PostingPage(vec![(*b"abc", 0b101), (*b"bcd", 0b010)]);
        let encoded = valid.encode(3).expect("valid page");
        assert_eq!(PostingPage::decode(&encoded, 3).expect("decode"), valid);

        let duplicate_term = encode_cbor(
            &vec![(*b"abc", 0b001_u64), (*b"abc", 0b010_u64)],
            "duplicate term fixture",
        )
        .expect("fixture");
        assert!(PostingPage::decode(&duplicate_term, 3).is_err());

        let empty_mask =
            encode_cbor(&vec![(*b"abc", 0_u64)], "empty posting fixture").expect("fixture");
        assert!(PostingPage::decode(&empty_mask, 3).is_err());

        let out_of_range =
            encode_cbor(&vec![(*b"abc", 0b1000_u64)], "outside segment fixture").expect("fixture");
        assert!(PostingPage::decode(&out_of_range, 3).is_err());

        let mut trailing = encoded;
        trailing.push(0);
        assert!(PostingPage::decode(&trailing, 3).is_err());
    }

    #[test]
    fn segment_rejects_unbounded_or_unsorted_logical_ids() {
        let maps = (MapRoot::default(), MapRoot::default());
        let oversized = encode_cbor(
            &(
                vec![1_u64; 65],
                maps.0.clone(),
                maps.1.clone(),
                0_u64,
                0_u64,
            ),
            "oversized segment fixture",
        )
        .expect("fixture");
        assert!(Segment::decode(&oversized).is_err());

        let unsorted = encode_cbor(
            &(vec![2_u64, 1], maps.0, maps.1, 0_u64, 0_u64),
            "unsorted segment fixture",
        )
        .expect("fixture");
        assert!(Segment::decode(&unsorted).is_err());
    }
}
