//! Shape-derived grants for History original and derived inputs.

use super::{history_work_error, invalid};
use crate::native_history::row_budget::{
    CommitShape, DiffHunkShape, RefTagShape, RowBacking, TextBytes, decode_json_shape,
    json_token_scratch,
    shape_codec::{closed_shape, newtype_shape},
};
use quanta_index_contract::{
    FileContributorEntry, FileContributorIdentityEntry, HistoryDiffHunkUpsert, HistoryRefMutation,
    index::HistoryReplayPublicationV1,
    lex::{CommitRecord, CommitSha},
};
use quanta_index_core::{
    CoreError,
    index::{WorkBudget, WorkUsage},
};
use serde::{
    Deserialize, Deserializer,
    de::{self, SeqAccess, Visitor},
};
use std::{fmt, mem};

mod cbor_replay;
pub(super) use cbor_replay::decode_cbor_replay;

fn add(left: u64, right: u64) -> Option<u64> {
    left.checked_add(right)
}

fn mul(left: u64, right: u64) -> Option<u64> {
    left.checked_mul(right)
}

#[derive(Clone, Copy, Default)]
struct ShapeSize {
    text: u64,
    vectors: u64,
    transient: u64,
}

impl ShapeSize {
    fn append(&mut self, other: Self) -> Option<()> {
        self.text = add(self.text, other.text)?;
        self.vectors = add(self.vectors, other.vectors)?;
        self.transient = self.transient.max(other.transient);
        Some(())
    }

    fn vector<T>(&mut self, count: usize) -> Option<()> {
        if count != 0 {
            // Vec's geometric growth and a simultaneous old/new allocation
            // are represented separately in the completed and peak grants.
            let slots = add(u64::try_from(count).ok()?, 4)?;
            self.vectors = add(
                self.vectors,
                mul(mul(slots, 2)?, mem::size_of::<T>() as u64)?,
            )?;
        }
        Some(())
    }
}

trait SizeOfShape {
    fn size(self) -> Option<ShapeSize>;
}

impl SizeOfShape for TextBytes {
    fn size(self) -> Option<ShapeSize> {
        Some(ShapeSize {
            text: u64::try_from(self.0).ok()?,
            vectors: 0,
            transient: 0,
        })
    }
}

struct SequenceSize<T> {
    size: ShapeSize,
    count: usize,
    _item: std::marker::PhantomData<T>,
}

impl<'de, T: Deserialize<'de> + SizeOfShape> Deserialize<'de> for SequenceSize<T> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Sequence<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de> + SizeOfShape> Visitor<'de> for Sequence<T> {
            type Value = SequenceSize<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("History JSON sequence")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut size = ShapeSize::default();
                let mut count = 0usize;
                while let Some(item) = seq.next_element::<T>()? {
                    count = count
                        .checked_add(1)
                        .ok_or_else(|| de::Error::custom("History JSON item count overflow"))?;
                    size.append(
                        item.size()
                            .ok_or_else(|| de::Error::custom("History JSON shape overflow"))?,
                    )
                    .ok_or_else(|| de::Error::custom("History JSON shape overflow"))?;
                }
                Ok(SequenceSize {
                    size,
                    count,
                    _item: std::marker::PhantomData,
                })
            }
        }
        decoder.deserialize_seq(Sequence(std::marker::PhantomData))
    }
}

impl<T> SequenceSize<T> {
    fn size_as<U>(mut self) -> Option<ShapeSize> {
        self.size.vector::<U>(self.count)?;
        Some(self.size)
    }
}

impl SizeOfShape for CommitShape {
    fn size(self) -> Option<ShapeSize> {
        let (text, parents, tags, tag_text) = self.batch_sizes().ok()?;
        let mut size = ShapeSize {
            text,
            vectors: 0,
            transient: 0,
        };
        size.vector::<CommitSha>(parents)?;
        size.vector::<Box<str>>(tags)?;
        if tags != 0 {
            // Ordinary Commit decoding first holds Vec<String>, then collects
            // boxed tags. Its current array can overlap old/new vector growth.
            let slots = add(u64::try_from(tags).ok()?, 4)?;
            size.transient = add(
                mul(mul(slots, 3)?, mem::size_of::<String>() as u64)?,
                u64::try_from(tag_text).ok()?,
            )?;
        }
        Some(size)
    }
}

closed_shape!(
    struct RefDeleteShape {
        name: TextBytes,
    }
);

newtype_shape!(enum RefShape {
    Upsert: RefTagShape,
    Delete: RefDeleteShape,
});

impl SizeOfShape for RefShape {
    fn size(self) -> Option<ShapeSize> {
        match self {
            Self::Upsert(row) => Some(ShapeSize {
                text: row.text_bytes().ok()?,
                vectors: 0,
                transient: 0,
            }),
            Self::Delete(row) => row.name.size(),
        }
    }
}

impl SizeOfShape for DiffHunkShape {
    fn size(self) -> Option<ShapeSize> {
        Some(ShapeSize {
            text: self.text_bytes().ok()?,
            vectors: 0,
            transient: 0,
        })
    }
}

closed_shape!(
    struct BatchShape {
        repo_id: TextBytes,
        revision_id: TextBytes,
        generation: u64,
        manifest_digest: Option<TextBytes> = default,
        batch_digest: TextBytes,
        commits: SequenceSize<CommitShape>,
        refs: SequenceSize<RefShape>,
        tags: SequenceSize<RefShape>,
        diff_hunks: SequenceSize<DiffHunkShape>,
    }
);

impl SizeOfShape for BatchShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.repo_id.size()?;
        size.append(self.revision_id.size()?)?;
        // These two DTO fields retain String capacity, which may be larger
        // than their decoded length after the streaming parser grows them.
        size.append(self.batch_digest.size()?)?;
        size.append(self.batch_digest.size()?)?;
        if let Some(digest) = self.manifest_digest {
            size.append(digest.size()?)?;
            size.append(digest.size()?)?;
        }
        size.append(self.commits.size_as::<CommitRecord>()?)?;
        size.append(self.refs.size_as::<HistoryRefMutation>()?)?;
        size.append(self.tags.size_as::<HistoryRefMutation>()?)?;
        size.append(self.diff_hunks.size_as::<HistoryDiffHunkUpsert>()?)?;
        let _fixed = self.generation;
        Some(size)
    }
}

closed_shape!(
    struct GenerationShape {
        repo_id: TextBytes,
        revision_id: TextBytes,
        generation: u64,
    }
);

impl SizeOfShape for GenerationShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.repo_id.size()?;
        size.append(self.revision_id.size()?)?;
        let _generation = self.generation;
        Some(size)
    }
}

closed_shape!(
    struct FenceShape {
        generation: GenerationShape,
        manifest_digest: Option<TextBytes> = default,
        publication_count: u64,
        first_replay: [u8; 32],
        final_replay: [u8; 32],
        final_durable_sequence: u64,
    }
);

impl SizeOfShape for FenceShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.generation.size()?;
        if let Some(digest) = self.manifest_digest {
            size.append(digest.size()?)?;
            size.append(digest.size()?)?;
        }
        let _fixed = (
            self.publication_count,
            self.first_replay,
            self.final_replay,
            self.final_durable_sequence,
        );
        Some(size)
    }
}

closed_shape!(
    struct SemanticRootsShape {
        row_root_digest: TextBytes,
        membership_root_digest: TextBytes,
    }
);

impl SizeOfShape for SemanticRootsShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.row_root_digest.size()?;
        size.append(self.row_root_digest.size()?)?;
        size.append(self.membership_root_digest.size()?)?;
        size.append(self.membership_root_digest.size()?)?;
        Some(size)
    }
}

closed_shape!(
    struct ReceiptShape {
        generation: u64,
        manifest_digest: Option<TextBytes> = default,
        batch_digest: TextBytes,
        accepted_replace_scopes: u32,
        accepted_tombstone_scopes: u32,
        accepted_semantic_replace_scopes: u32,
        accepted_semantic_tombstone_scopes: u32,
        accepted_clear_surfaces: u32,
        sealed: bool,
        applied: bool,
        durable_sequence: u64,
        semantic_content: Option<SemanticRootsShape> = default,
    }
);

impl SizeOfShape for ReceiptShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.batch_digest.size()?;
        size.append(self.batch_digest.size()?)?;
        if let Some(digest) = self.manifest_digest {
            size.append(digest.size()?)?;
            size.append(digest.size()?)?;
        }
        if let Some(roots) = self.semantic_content {
            size.append(roots.size()?)?;
        }
        let _fixed = (
            self.generation,
            self.accepted_replace_scopes,
            self.accepted_tombstone_scopes,
            self.accepted_semantic_replace_scopes,
            self.accepted_semantic_tombstone_scopes,
            self.accepted_clear_surfaces,
            self.sealed,
            self.applied,
            self.durable_sequence,
        );
        Some(size)
    }
}

closed_shape!(
    struct PublicationShape {
        ordinal: u64,
        before: [u8; 32],
        after: [u8; 32],
        batch: BatchShape,
        ack: ReceiptShape,
    }
);

impl SizeOfShape for PublicationShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.batch.size()?;
        size.append(self.ack.size()?)?;
        let _fixed = (self.ordinal, self.before, self.after);
        Some(size)
    }
}

closed_shape!(
    struct SourceShape {
        wire_version: u32,
        fence: FenceShape,
        publications: SequenceSize<PublicationShape>,
    }
);

impl SizeOfShape for SourceShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.fence.size()?;
        size.append(self.publications.size_as::<HistoryReplayPublicationV1>()?)?;
        let _version = self.wire_version;
        Some(size)
    }
}

closed_shape!(
    struct ContributorIdentityShape {
        canonical: TextBytes,
        name: Option<TextBytes> = default,
        email: Option<TextBytes> = default,
    }
);

impl SizeOfShape for ContributorIdentityShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = ShapeSize::default();
        for text in [Some(self.canonical), self.name, self.email]
            .into_iter()
            .flatten()
        {
            let text = text.size()?;
            size.append(text)?;
            size.append(text)?;
        }
        Some(size)
    }
}

closed_shape!(
    struct ContributorEntryShape {
        source_repo_id: TextBytes,
        repo_relative_path: TextBytes,
        contributors: SequenceSize<ContributorIdentityShape>,
    }
);

impl SizeOfShape for ContributorEntryShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self
            .contributors
            .size_as::<FileContributorIdentityEntry>()?;
        size.append(self.source_repo_id.size()?)?;
        size.append(self.repo_relative_path.size()?)?;
        size.append(self.repo_relative_path.size()?)?;
        Some(size)
    }
}

closed_shape!(
    struct ContributorBatchShape {
        repo_id: TextBytes,
        revision_id: TextBytes,
        generation: u64,
        batch_digest: TextBytes,
        entries: SequenceSize<ContributorEntryShape>,
    }
);

impl SizeOfShape for ContributorBatchShape {
    fn size(self) -> Option<ShapeSize> {
        let mut size = self.entries.size_as::<FileContributorEntry>()?;
        size.append(self.repo_id.size()?)?;
        size.append(self.revision_id.size()?)?;
        size.append(self.batch_digest.size()?)?;
        size.append(self.batch_digest.size()?)?;
        Some(size)
    }
}

pub(super) fn history_batch_backing(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<RowBacking, CoreError> {
    shape_backing::<BatchShape>(payload, budget)
}

pub(super) fn history_source_backing(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<RowBacking, CoreError> {
    shape_backing::<SourceShape>(payload, budget)
}

pub(super) fn contributor_batch_backing(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<RowBacking, CoreError> {
    shape_backing::<ContributorBatchShape>(payload, budget)
}

fn shape_backing<T: for<'de> Deserialize<'de> + SizeOfShape>(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<RowBacking, CoreError> {
    let (scratch_bytes, scratch) = json_token_scratch(payload, budget)?;
    budget
        .charge(WorkUsage {
            bytes: payload.len() as u64,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let shape: T = decode_json_shape(payload)?;
    let size = shape
        .size()
        .ok_or_else(|| invalid("History JSON shape bound overflow"))?;
    drop(scratch);
    decode_backing(size, scratch_bytes.reader)
}

fn decode_backing(size: ShapeSize, parser_scratch: u64) -> Result<RowBacking, CoreError> {
    let retained = add(
        add(size.text, size.vectors)
            .ok_or_else(|| invalid("History source backing bound overflow"))?,
        4096,
    )
    .ok_or_else(|| invalid("History source backing bound overflow"))?;
    let peak = add(
        retained,
        add(
            add(parser_scratch, size.vectors)
                .ok_or_else(|| invalid("History source decode bound overflow"))?,
            size.transient,
        )
        .ok_or_else(|| invalid("History source decode bound overflow"))?,
    )
    .ok_or_else(|| invalid("History source decode bound overflow"))?;
    RowBacking::new(retained, peak)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_history::build::source_input::{
        decode_history_batch_object, decode_history_source_object,
    };
    use crate::native_history::build::{LoadedBootSourceV1, decode_boot_source_v1};
    use quanta_index_contract::{
        BatchPublishReceipt, FileContributorIngestBatch, HistoryIngestBatch, ManifestGeneration,
        RepoId, RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2,
        index::{
            AuxiliaryGenerationKeyV1, HistoryReplayFenceV1, HistoryReplayPublicationV1,
            HistorySourceInputV1,
        },
    };
    use quanta_index_core::index::{
        Digest, IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
        MemoryScopePort, ObjectRef, RegisteredObject, StoredObject, WorkLimits,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    #[test]
    fn history_shape_decoder_keeps_nullable_fields_and_closed_maps() {
        let batch = br#"{"repo_id":"r","revision_id":"v","generation":1,"batch_digest":"d","commits":[],"refs":[],"tags":[],"diff_hunks":[]}"#;
        let decoded: BatchShape = decode_json_shape(batch).expect("omitted nullable digest");
        assert!(decoded.manifest_digest.is_none());
        assert_eq!(decoded.commits.count, 0);

        let identity: ContributorIdentityShape =
            decode_json_shape(br#"{"canonical":"a@example.test"}"#)
                .expect("omitted nullable identity fields");
        assert!(identity.name.is_none());
        assert!(identity.email.is_none());

        for invalid in [
            br#"{"repo_id":"r","revision_id":"v","generation":1,"batch_digest":"d","commits":[],"refs":[],"tags":[],"diff_hunks":[],"unknown":0}"#.as_slice(),
            br#"{"repo_id":"r","revision_id":"v","generation":1,"batch_digest":"d","batch_digest":"e","commits":[],"refs":[],"tags":[],"diff_hunks":[]}"#.as_slice(),
        ] {
            assert!(decode_json_shape::<BatchShape>(invalid).is_err());
        }
    }

    #[test]
    fn history_ref_shape_keeps_external_newtype_variants() {
        let upsert: RefShape = decode_json_shape(br#"{"Upsert":{"name":"main","sha":"abc"}}"#)
            .expect("externally tagged upsert");
        assert!(matches!(upsert, RefShape::Upsert(_)));
        let delete: RefShape =
            decode_json_shape(br#"{"Delete":{"name":"main"}}"#).expect("externally tagged delete");
        assert!(matches!(delete, RefShape::Delete(_)));
        for invalid in [
            br#""Upsert""#.as_slice(),
            br#"{"0":{"name":"main","sha":"abc"}}"#.as_slice(),
            br#"{"Delete":{"name":"main","other":0}}"#.as_slice(),
        ] {
            assert!(decode_json_shape::<RefShape>(invalid).is_err());
        }
    }

    struct Ledger {
        live: AtomicU64,
        limit: u64,
    }
    struct Scope(Arc<Ledger>);

    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            assert!(self.live.fetch_sub(bytes, Ordering::AcqRel) >= bytes);
        }
    }

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                    live.checked_add(bytes).filter(|next| *next <= self.0.limit)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn work(limit: u64) -> (WorkBudget, Arc<Ledger>) {
        let ledger = Arc::new(Ledger {
            live: AtomicU64::new(0),
            limit,
        });
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                mutations: 1000,
                bytes: 100_000_000,
            },
            Instant::now() + Duration::from_secs(20),
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        (work, ledger)
    }

    #[test]
    fn boot_source_version_funds_escaped_strings_and_bounds_errors() {
        let reference = ObjectRef(Digest([7; 32]));
        for escape in ["", r"\n"] {
            let payload = format!(r#"{{"wire_version":"{escape}{}"}}"#, "x".repeat(32 * 1024));
            let raw_bytes = payload.len() as u64;
            let (mut budget, ledger) = work(raw_bytes + 4096);
            let raw = budget
                .reserve_memory(raw_bytes, MemoryLifetime::Retained)
                .expect("original BOOT source grant");
            let stored = StoredObject::new(
                RegisteredObject::unscoped(reference, raw_bytes, Vec::new()),
                payload.into_bytes(),
                raw,
            );
            let rejected = decode_boot_source_v1(reference, &stored, &mut budget);
            if escape.is_empty() {
                assert!(matches!(
                    rejected,
                    Err(CoreError::InvalidContract(message))
                        if message.len() < 1024 && message.contains("line 1")
                ));
            } else {
                assert!(matches!(
                    rejected,
                    Err(CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::ServerOverloaded,
                        ..
                    })
                ));
            }
            assert_eq!(ledger.live.load(Ordering::Acquire), raw_bytes);
            drop(stored);
            assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn contributor_input_is_funded_under_original_scope() -> Result<(), Box<dyn std::error::Error>>
    {
        use crate::native_history::build::source_input::decode_history_contributor_object;

        const MIB: u64 = 1024 * 1024;
        const ENTRIES: usize = 2048;
        let repo = RepoId::new("contributor-input")?;
        let directory = "history-directory/".repeat(135);
        let output = FileContributorIngestBatch {
            repo_id: repo.clone(),
            revision_id: RevisionId::new("main")?,
            generation: ManifestGeneration::new(2),
            batch_digest: "fixture-digest".into(),
            entries: (0..ENTRIES)
                .map(|index| FileContributorEntry {
                    source_repo_id: repo.clone(),
                    repo_relative_path: RepoRelativePath::new(format!("{directory}{index:04}.rs")),
                    contributors: vec![FileContributorIdentityEntry {
                        canonical: "fixture@example.test".into(),
                        name: Some("Fixture Author".into()),
                        email: Some("fixture@example.test".into()),
                    }],
                })
                .collect(),
        };
        let payload = serde_json::to_vec(&output)?;
        drop(output);
        let payload_bytes = u64::try_from(payload.len())?;
        assert!(payload_bytes > 4 * MIB);
        let reference = ObjectRef(Digest([10; 32]));
        let stored = StoredObject::new(
            RegisteredObject::unscoped(reference, payload_bytes, Vec::new()),
            payload,
            None,
        );
        let (mut budget, ledger) = work(64 * MIB);
        let input = budget.reserve_memory(payload_bytes, MemoryLifetime::Retained)?;
        let decoded = decode_history_contributor_object(reference, &stored, &mut budget)?;
        assert_eq!(decoded.value.entries.len(), ENTRIES);
        let last = decoded
            .value
            .entries
            .last()
            .ok_or("last contributor missing")?;
        assert_eq!(last.source_repo_id, repo);
        assert!(last.repo_relative_path.as_str().ends_with("2047.rs"));
        assert_eq!(last.contributors[0].canonical, "fixture@example.test");
        assert!(ledger.live.load(Ordering::Acquire) > payload_bytes);
        drop(decoded);
        assert_eq!(ledger.live.load(Ordering::Acquire), payload_bytes);
        drop(input);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);

        let (mut small, ledger) = work(1024);
        assert!(matches!(
            decode_history_contributor_object(reference, &stored, &mut small),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }

    fn replay_commit_batch() -> HistoryIngestBatch {
        let sha = CommitSha::from_bytes([7; 20]);
        HistoryIngestBatch {
            repo_id: RepoId::new("large-original").expect("repo"),
            revision_id: RevisionId::new("main").expect("revision"),
            generation: ManifestGeneration::new(1),
            manifest_digest: None,
            batch_digest: "digest".into(),
            commits: vec![CommitRecord {
                wire_version: 1,
                sha,
                parents: Vec::new(),
                author_time_ms: 1,
                committer_time_ms: 1,
                applied_at_ms: 1,
                author: "a".into(),
                author_name: None,
                author_email: None,
                committer: "a".into(),
                committer_name: None,
                committer_email: None,
                message: "".into(),
                is_merge: false,
                tags: Vec::new(),
            }],
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        }
    }

    fn replay_source(batch: HistoryIngestBatch) -> HistorySourceInputV1 {
        HistorySourceInputV1 {
            wire_version: 1,
            fence: HistoryReplayFenceV1 {
                generation: AuxiliaryGenerationKeyV1 {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                },
                manifest_digest: None,
                publication_count: 1,
                first_replay: Digest([1; 32]),
                final_replay: Digest([2; 32]),
                final_durable_sequence: 1,
            },
            publications: vec![HistoryReplayPublicationV1 {
                ordinal: 1,
                before: Digest([1; 32]),
                after: Digest([2; 32]),
                ack: BatchPublishReceipt::empty_for(batch.generation, None, "digest")
                    .recorded_at(1),
                batch,
            }],
        }
    }

    #[test]
    fn compact_replay_keeps_original_bytes_and_ack_under_the_object_limit() -> Result<(), CoreError>
    {
        const WIRE_CAP: usize = quanta_index_contract::index::MAX_HISTORY_SOURCE_INPUT_BYTES_V1;
        const TEXT_BYTES: usize = 3 * 1024 * 1024;
        // The JSON spelling of each U+0001 occupies six bytes.
        let mut batch = replay_commit_batch();
        batch.commits[0].message = "\u{0001}".repeat(TEXT_BYTES).into();
        batch.refs.push(HistoryRefMutation::Upsert(
            quanta_index_contract::HistoryRefUpsert {
                name: "refs/heads/main".into(),
                sha: batch.commits[0].sha,
            },
        ));
        assert!(serde_json::to_vec(&batch).expect("JSON body").len() > WIRE_CAP);
        let input = replay_source(batch);
        let encoded = quanta_index_ipc::encode_cbor_payload(&input).expect("canonical CBOR");
        assert!(encoded.len() < WIRE_CAP);
        let raw_bytes = encoded.len() as u64;
        let reference = ObjectRef(Digest([11; 32]));
        let (mut budget, ledger) = work(64 * 1024 * 1024);
        let raw = budget
            .reserve_memory(raw_bytes, MemoryLifetime::Retained)
            .expect("compact raw backing");
        let stored = StoredObject::new(
            RegisteredObject::unscoped(reference, raw_bytes, Vec::new()),
            encoded,
            raw,
        );
        let LoadedBootSourceV1::Replay(decoded) =
            decode_boot_source_v1(reference, &stored, &mut budget)?
        else {
            return Err(invalid("compact History source must select Replay"));
        };
        assert_eq!(decoded.value, input);
        assert!(ledger.live.load(Ordering::Acquire) > raw_bytes);
        drop(decoded);
        assert_eq!(ledger.live.load(Ordering::Acquire), raw_bytes);
        drop(stored);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn compact_replay_refuses_unfunded_and_noncanonical_bytes() {
        let input = replay_source(replay_commit_batch());
        let bytes = quanta_index_ipc::encode_cbor_payload(&input).expect("canonical CBOR");
        let (mut small, ledger) = work(bytes.len() as u64);
        assert!(matches!(
            decode_cbor_replay(&bytes, &mut small),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);

        // A two-byte map header with the same count must not get a new ID.
        assert_eq!(bytes.first(), Some(&0xa3));
        let mut noncanonical = vec![0xb8, 3];
        noncanonical.extend_from_slice(&bytes[1..]);
        let (mut budget, ledger) = work(64 * 1024);
        assert!(matches!(
            decode_cbor_replay(&noncanonical, &mut budget),
            Err(CoreError::InvalidContract(message)) if message.contains("not canonical")
        ));
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    }

    #[test]
    fn large_original_commit_decodes_with_retained_grant_and_refunds() -> Result<(), CoreError> {
        const WIRE_CAP: usize = quanta_index_contract::index::MAX_HISTORY_SOURCE_INPUT_BYTES_V1;
        let mut batch = replay_commit_batch();
        let batch_overhead = serde_json::to_vec(&batch)
            .expect("empty-message History input")
            .len();
        let batch_message_bytes = WIRE_CAP - batch_overhead;
        batch.commits[0].message = "m".repeat(batch_message_bytes).into();
        let payload = serde_json::to_vec(&batch).expect("canonical History input");
        assert_eq!(payload.len(), WIRE_CAP);
        batch.commits[0].message = "".into();
        let mut carrier = replay_source(batch);
        let carrier_overhead = serde_json::to_vec(&carrier)
            .expect("empty-message History carrier")
            .len();
        let carrier_message_bytes = WIRE_CAP - carrier_overhead;
        carrier.publications[0].batch.commits[0].message = "m".repeat(carrier_message_bytes).into();
        carrier
            .validate_shape()
            .expect("retained-original carrier shape");
        let carrier_payload = serde_json::to_vec(&carrier).expect("canonical History carrier");
        assert_eq!(carrier_payload.len(), WIRE_CAP);
        drop(carrier);
        let reference = ObjectRef(Digest([8; 32]));
        let stored = StoredObject::new(
            RegisteredObject::unscoped(reference, payload.len() as u64, Vec::new()),
            payload,
            None,
        );
        let carrier_reference = ObjectRef(Digest([9; 32]));
        let carrier_stored = StoredObject::new(
            RegisteredObject::unscoped(carrier_reference, carrier_payload.len() as u64, Vec::new()),
            carrier_payload,
            None,
        );
        let (mut budget, ledger) = work(64 * 1024 * 1024);
        let raw_bytes = stored.payload.len() as u64;
        let raw = budget
            .reserve_memory(raw_bytes, MemoryLifetime::Retained)
            .expect("original object payload grant");
        let decoded = decode_history_batch_object(reference, &stored, &mut budget)
            .expect("admit large original History batch under 64 MiB");
        let message = decoded.value.commits[0].message.as_bytes();
        assert_eq!(message.len(), batch_message_bytes);
        assert!(message.iter().all(|byte| *byte == b'm'));
        assert!(ledger.live.load(Ordering::Acquire) > raw_bytes);
        drop(decoded);
        assert_eq!(ledger.live.load(Ordering::Acquire), raw_bytes);
        drop(raw);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);

        let (mut small, small_ledger) = work(raw_bytes);
        let raw = small
            .reserve_memory(raw_bytes, MemoryLifetime::Retained)
            .expect("small scope retains original payload");
        assert!(matches!(
            decode_history_batch_object(reference, &stored, &mut small),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        assert_eq!(small_ledger.live.load(Ordering::Acquire), raw_bytes);
        drop(raw);
        assert_eq!(small_ledger.live.load(Ordering::Acquire), 0);

        let (mut carrier_budget, carrier_ledger) = work(64 * 1024 * 1024);
        let carrier_raw_bytes = carrier_stored.payload.len() as u64;
        let carrier_raw = carrier_budget
            .reserve_memory(carrier_raw_bytes, MemoryLifetime::Retained)
            .expect("nested source payload grant");
        let LoadedBootSourceV1::Replay(decoded) =
            decode_boot_source_v1(carrier_reference, &carrier_stored, &mut carrier_budget)?
        else {
            return Err(invalid(
                "retained-original History source must select Replay",
            ));
        };
        let message = decoded.value.publications[0].batch.commits[0]
            .message
            .as_bytes();
        assert_eq!(message.len(), carrier_message_bytes);
        assert!(message.iter().all(|byte| *byte == b'm'));
        assert!(carrier_ledger.live.load(Ordering::Acquire) > carrier_raw_bytes);
        drop(decoded);
        assert_eq!(
            carrier_ledger.live.load(Ordering::Acquire),
            carrier_raw_bytes
        );
        drop(carrier_raw);
        assert_eq!(carrier_ledger.live.load(Ordering::Acquire), 0);
        let (mut carrier_small, carrier_small_ledger) = work(carrier_raw_bytes);
        let carrier_raw = carrier_small
            .reserve_memory(carrier_raw_bytes, MemoryLifetime::Retained)
            .expect("small scope retains nested source payload");
        assert!(matches!(
            decode_history_source_object(carrier_reference, &carrier_stored, &mut carrier_small),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        assert_eq!(
            carrier_small_ledger.live.load(Ordering::Acquire),
            carrier_raw_bytes
        );
        drop(carrier_raw);
        assert_eq!(carrier_small_ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }
}
