//! One durable replacement binds repository roots and source-event lineage.
//!
//! Counts and bytes are bounded; retained event identities never expire or get
//! evicted to make room for a new publication.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;

use quanta_index_contract::{
    ACTIVATION_ROOT_INCARNATION_BYTES_V1, GenerationSnapshot, IngestOperationKindV1,
    ManifestGeneration, RepoId, RevisionId, SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, IdempotencyKeyV1, LexicalDocumentNamespaceV1, LexicalDocumentVersionRangeV1,
    LexicalDocumentVersionReservationV1, SourceEventAllocationRequestV1, SourceEventAllocationV1,
    SourceEventBindingV1, SourceEventPhaseV1, SourceEventRecordV1, SourceEventStateV1,
};
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

use super::{ActivationKey, ActiveSearchCorpusHeadV1, CatalogState};
use crate::readiness::search_corpus_generation::PersistedSearchCorpusGenerationRootV1;

pub(super) const MAX_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_ROOTS: usize = 256;
pub(super) const MAX_STREAMS: usize = 256;
pub(super) const MAX_EVENTS: usize = 8192;
pub(super) const MAX_COUNTERS: usize = 8192;
const REPOSITORY_ENVELOPE_FORMAT_VERSION: u32 = 4;
const MAX_MANIFEST_TOKEN_BYTES: usize = 4096;

pub(super) fn bound_manifest_token(token: &str) -> Result<(), CoreError> {
    if token.is_empty()
        || token.len() > MAX_MANIFEST_TOKEN_BYTES
        || !token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(capacity(
            "manifest token must be printable ASCII of 1..=4096 bytes",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct StreamHead {
    pub active: Option<String>,
    pub pending: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct EventHistory {
    pub streams: BTreeMap<String, StreamHead>,
    pub records: BTreeMap<(String, String), SourceEventRecordV1>,
    pub high_water: BTreeMap<RevisionId, ManifestGeneration>,
    pub lexical_high_water: BTreeMap<([u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1], u8), u64>,
    pub lexical_reservations: BTreeMap<(String, String), LexicalDocumentVersionReservationV1>,
}

pub(super) fn corrupt(message: impl Into<String>) -> CoreError {
    CoreError::Storage(format!(
        "activation repository envelope: {}",
        message.into()
    ))
}

pub(super) fn capacity(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("activation repository envelope capacity exhausted: {message}"),
    }
}

pub(super) fn file_name(repo: &RepoId) -> String {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:activation-repository:v2\0");
    hash.update(repo.as_str().as_bytes());
    format!("{:X}--corpus.json", hash.finalize())
}

/// Decode a sequence without trusting an attacker-controlled size hint or
/// allocating the first over-limit element.
struct Rows<T, const N: usize>(Vec<T>);
impl<T: Serialize, const N: usize> Serialize for Rows<T, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Rows<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for RowsVisitor<T, N> {
            type Value = Rows<T, N>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "at most {N} canonical rows")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut rows = Vec::new();
                while rows.len() < N {
                    let Some(row) = seq.next_element()? else {
                        return Ok(Rows(rows));
                    };
                    rows.push(row);
                }
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("repository envelope row limit exceeded"));
                }
                Ok(Rows(rows))
            }
        }
        deserializer.deserialize_seq(RowsVisitor::<T, N>(std::marker::PhantomData))
    }
}

type StreamRow = (String, Option<String>, Option<String>);
// The journal's kind/repo/revision/generation are fixed by the lexical target;
// only its batch digest is separately encoded. Decoding rebuilds the exact key.
type BoundRow = (GenerationSnapshot, String, u8);
type LexicalRangesRow = ((u64, u64), (u64, u64));
type EventRow = (
    SourcePublicationEvent,
    RevisionId,
    ManifestGeneration,
    [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
    Option<BoundRow>,
    Option<LexicalRangesRow>,
);
type EnvelopeRow = (
    u32,
    RepoId,
    Rows<PersistedSearchCorpusGenerationRootV1, MAX_ROOTS>,
    Rows<StreamRow, MAX_STREAMS>,
    Rows<EventRow, MAX_EVENTS>,
    Rows<(RevisionId, ManifestGeneration), MAX_COUNTERS>,
    Rows<([u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1], u8, u64), MAX_COUNTERS>,
);

pub(super) fn decode(path: &Path, bytes: &[u8]) -> Result<(RepoId, CatalogState), CoreError> {
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(capacity("persisted byte limit"));
    }
    let format = serde_json::from_slice::<serde_json::Value>(bytes)
        .map_err(|error| corrupt(error.to_string()))?
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(serde_json::Value::as_u64);
    if format != Some(u64::from(REPOSITORY_ENVELOPE_FORMAT_VERSION)) {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            message: "activation repository envelope format changed; offline rebuild required"
                .into(),
        });
    }
    let (format, repo, roots, streams, events, counters, lexical_counters): EnvelopeRow =
        serde_json::from_slice(bytes).map_err(|error| corrupt(error.to_string()))?;
    if format != REPOSITORY_ENVELOPE_FORMAT_VERSION
        || path.file_name().and_then(|name| name.to_str()) != Some(file_name(&repo).as_str())
    {
        return Err(corrupt(
            "unsupported format or filename/repository identity mismatch; offline rebuild required",
        ));
    }
    let mut state = CatalogState::default();
    let mut previous_revision: Option<RevisionId> = None;
    for root in roots.0 {
        let (generation, activation_sequence) = root.into_generation()?;
        bound_manifest_token(generation.lexical().manifest_digest.as_str())?;
        bound_manifest_token(generation.semantic().manifest_digest.as_str())?;
        if generation.repo_id() != &repo
            || previous_revision
                .as_ref()
                .is_some_and(|previous| previous >= generation.revision_id())
        {
            return Err(corrupt(
                "roots must be unique, sorted, and belong to the envelope repository",
            ));
        }
        previous_revision = Some(generation.revision_id().clone());
        let _prior = state.roots.insert(
            ActivationKey::for_pair(&repo, generation.revision_id()),
            ActiveSearchCorpusHeadV1 {
                generation,
                activation_sequence,
            },
        );
    }
    let mut history = EventHistory::default();
    let mut previous_stream: Option<String> = None;
    for (stream, active, pending) in streams.0 {
        // Reuse the event-token contract, including for stored head references.
        SourcePublicationEvent {
            stream_id: stream.clone(),
            event_id: active
                .clone()
                .or_else(|| pending.clone())
                .unwrap_or_else(|| "unpublished".into()),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        }
        .validate()
        .map_err(|error| corrupt(error.to_string()))?;
        if previous_stream
            .as_ref()
            .is_some_and(|previous| previous >= &stream)
            || (active.is_none() && pending.is_none())
        {
            return Err(corrupt(
                "stream rows must be sorted, unique and have retained lineage",
            ));
        }
        previous_stream = Some(stream.clone());
        let _prior = history
            .streams
            .insert(stream, StreamHead { active, pending });
    }
    let mut previous_event: Option<(String, String)> = None;
    for (event, revision, generation, root_incarnation, bound, lexical_ranges) in events.0 {
        let key = (event.stream_id.clone(), event.event_id.clone());
        if previous_event
            .as_ref()
            .is_some_and(|previous| previous >= &key)
        {
            return Err(corrupt(
                "event rows must be sorted, unique and belong to the envelope repository",
            ));
        }
        previous_event = Some(key.clone());
        let state = match bound {
            None => SourceEventStateV1::Allocated,
            Some((target, batch_digest, phase)) => {
                bound_manifest_token(&target.manifest_digest)?;
                let journal_key = IdempotencyKeyV1 {
                    kind: IngestOperationKindV1::SearchCorpus,
                    repo_id: target.repo_id.clone(),
                    revision_id: target.revision_id.clone(),
                    generation: target.manifest_generation,
                    batch_digest,
                };
                let phase = match phase {
                    0 => SourceEventPhaseV1::Pending,
                    1 => SourceEventPhaseV1::Staged,
                    2 => SourceEventPhaseV1::Active,
                    _ => return Err(corrupt("unknown source event phase")),
                };
                SourceEventStateV1::Bound {
                    binding: Box::new(SourceEventBindingV1 {
                        event: event.clone(),
                        target,
                        journal_key,
                    }),
                    phase,
                }
            }
        };
        let record = SourceEventRecordV1 {
            allocation: SourceEventAllocationV1 {
                request: SourceEventAllocationRequestV1 {
                    repo_id: repo.clone(),
                    revision_id: revision,
                    event,
                },
                generation,
                root_incarnation,
            },
            state,
        };
        record
            .validate()
            .map_err(|error| corrupt(error.to_string()))?;
        if let Some(((file_first, file_count), (text_first, text_count))) = lexical_ranges {
            let reservation = LexicalDocumentVersionReservationV1 {
                allocation: record.allocation.clone(),
                file: LexicalDocumentVersionRangeV1 {
                    first: file_first,
                    count: file_count,
                    scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                        &root_incarnation,
                        &repo,
                        LexicalDocumentNamespaceV1::File,
                    ),
                },
                native_text: LexicalDocumentVersionRangeV1 {
                    first: text_first,
                    count: text_count,
                    scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                        &root_incarnation,
                        &repo,
                        LexicalDocumentNamespaceV1::NativeText,
                    ),
                },
            };
            let _prior = history
                .lexical_reservations
                .insert(key.clone(), reservation);
        }
        let _prior = history.records.insert(key, record);
    }
    let mut previous_counter: Option<RevisionId> = None;
    for (revision, generation) in counters.0 {
        if generation.get() == 0
            || previous_counter
                .as_ref()
                .is_some_and(|prior| prior >= &revision)
        {
            return Err(corrupt(
                "generation counters must be sorted, unique and nonzero",
            ));
        }
        previous_counter = Some(revision.clone());
        let _prior = history.high_water.insert(revision, generation);
    }
    let mut previous_lexical_counter = None;
    for (incarnation, namespace, water) in lexical_counters.0 {
        let key = (incarnation, namespace);
        if !matches!(namespace, 1 | 2)
            || incarnation == [0; ACTIVATION_ROOT_INCARNATION_BYTES_V1]
            || previous_lexical_counter.is_some_and(|previous| previous >= key)
        {
            return Err(corrupt(
                "lexical counters must have canonical, unique scopes",
            ));
        }
        previous_lexical_counter = Some(key);
        let _prior = history.lexical_high_water.insert(key, water);
    }
    validate_history(&history)?;
    let _prior = state.histories.insert(repo.clone(), history);
    Ok((repo, state))
}

pub(super) fn validate_history(history: &EventHistory) -> Result<(), CoreError> {
    if history.streams.len() > MAX_STREAMS
        || history.records.len() > MAX_EVENTS
        || history.high_water.len() > MAX_COUNTERS
        || history.lexical_high_water.len() > MAX_COUNTERS
        || history.lexical_reservations.len() > MAX_EVENTS
    {
        return Err(capacity(
            "stream, retained event or generation counter limit",
        ));
    }
    validate_lexical_ranges(history)?;
    for (stream, head) in &history.streams {
        for (id, active) in [(head.active.as_ref(), true), (head.pending.as_ref(), false)] {
            if let Some(id) = id {
                let record = history
                    .records
                    .get(&(stream.clone(), id.clone()))
                    .ok_or_else(|| corrupt("stream head references missing event"))?;
                if (matches!(
                    record.state,
                    SourceEventStateV1::Bound {
                        phase: SourceEventPhaseV1::Active,
                        ..
                    }
                )) != active
                {
                    return Err(corrupt("stream head phase mismatch"));
                }
            }
        }
    }
    // Every accepted event is an ancestor of exactly one active high-water.
    // This rejects forged branches, orphaned active rows and cycles on reopen.
    let mut accepted = std::collections::BTreeSet::new();
    for (stream, head) in &history.streams {
        let mut cursor = head.active.as_ref();
        while let Some(id) = cursor {
            let key = (stream.clone(), id.clone());
            if !accepted.insert(key.clone()) {
                return Err(corrupt("cyclic accepted event lineage"));
            }
            let record = history
                .records
                .get(&key)
                .ok_or_else(|| corrupt("missing accepted ancestor"))?;
            if !matches!(
                record.state,
                SourceEventStateV1::Bound {
                    phase: SourceEventPhaseV1::Active,
                    ..
                }
            ) {
                return Err(corrupt("unaccepted ancestor in active lineage"));
            }
            cursor = record
                .allocation
                .request
                .event
                .expected_base_event_id
                .as_ref();
        }
    }
    let mut targets = std::collections::BTreeSet::new();
    for ((stream, event_id), record) in &history.records {
        // Recovery and persistence must enforce the same generation ownership
        // as live reservation. A different manifest does not make one physical
        // generation a second target, even across producer streams.
        record
            .validate()
            .map_err(|error| corrupt(error.to_string()))?;
        let target = &record.allocation;
        if !targets.insert((
            &target.request.repo_id,
            &target.request.revision_id,
            target.generation,
        )) {
            return Err(corrupt(
                "target generation belongs to multiple source events",
            ));
        }
        if matches!(
            record.state,
            SourceEventStateV1::Bound {
                phase: SourceEventPhaseV1::Active,
                ..
            }
        ) && !accepted.contains(&(stream.clone(), event_id.clone()))
        {
            return Err(corrupt("orphan accepted event outside active lineage"));
        }
        let head = history
            .streams
            .get(stream)
            .ok_or_else(|| corrupt("event has no stream"))?;
        if record.allocation.request.event.stream_id != *stream
            || record.allocation.request.event.event_id != *event_id
        {
            return Err(corrupt("event map key mismatch"));
        }
        if !matches!(
            record.state,
            SourceEventStateV1::Bound {
                phase: SourceEventPhaseV1::Active,
                ..
            }
        ) && (head.pending.as_ref() != Some(event_id)
            || record.allocation.request.event.expected_base_event_id != head.active)
        {
            return Err(corrupt(
                "unresolved event does not own the pending slot or active base",
            ));
        }
        if let Some(base) = &record.allocation.request.event.expected_base_event_id {
            let base_record = history
                .records
                .get(&(stream.clone(), base.clone()))
                .ok_or_else(|| corrupt("event base was not retained"))?;
            if !matches!(
                base_record.state,
                SourceEventStateV1::Bound {
                    phase: SourceEventPhaseV1::Active,
                    ..
                }
            ) {
                return Err(corrupt("event base is not active history"));
            }
        }
        if history
            .high_water
            .get(&target.request.revision_id)
            .is_none_or(|water| *water < target.generation)
        {
            return Err(corrupt(
                "source event exceeds its retained generation high-water",
            ));
        }
    }
    Ok(())
}

fn validate_lexical_ranges(history: &EventHistory) -> Result<(), CoreError> {
    let mut intervals: BTreeMap<(_, u8), Vec<(u64, u64)>> = BTreeMap::new();
    for (key, reservation) in &history.lexical_reservations {
        reservation
            .validate()
            .map_err(|error| corrupt(error.to_string()))?;
        let record = history
            .records
            .get(key)
            .ok_or_else(|| corrupt("lexical reservation has no retained source event"))?;
        if record.allocation != reservation.allocation || record.binding_v1().is_none() {
            return Err(corrupt(
                "lexical reservation differs from its original event allocation",
            ));
        }
        for (namespace, range) in [(1, &reservation.file), (2, &reservation.native_text)] {
            let scope = (reservation.allocation.root_incarnation, namespace);
            let water = history
                .lexical_high_water
                .get(&scope)
                .ok_or_else(|| corrupt("lexical reservation lost its durable counter"))?;
            let ranges = intervals.entry(scope).or_default();
            if range.count != 0 {
                let last = range
                    .first
                    .checked_add(range.count.saturating_sub(1))
                    .ok_or_else(|| corrupt("lexical reservation overflow"))?;
                if last > *water {
                    return Err(corrupt("lexical reservation exceeds its durable counter"));
                }
                ranges.push((range.first, last));
            }
        }
    }
    if intervals.len() != history.lexical_high_water.len() {
        return Err(corrupt("orphan lexical document counter"));
    }
    for (scope, ranges) in &mut intervals {
        ranges.sort_unstable();
        let mut last = 0;
        for &(first, end) in ranges.iter() {
            if first <= last {
                return Err(corrupt("overlapping lexical document reservations"));
            }
            last = end;
        }
        if history.lexical_high_water.get(scope) != Some(&last) {
            return Err(corrupt(
                "lexical counter differs from retained reservation high-water",
            ));
        }
    }
    Ok(())
}

/// Two passes avoid allocating an oversized serialized body. The first writer
/// counts against the byte cap; the second allocates exactly that bounded size.
pub(super) fn encode(repo: &RepoId, state: &CatalogState) -> Result<Vec<u8>, CoreError> {
    let root_count = state
        .roots
        .keys()
        .filter(|key| &key.repo_id == repo)
        .count();
    if root_count > MAX_ROOTS {
        return Err(capacity("revision root limit"));
    }
    let history = state.histories.get(repo);
    if let Some(history) = history {
        validate_history(history)?;
    }
    let roots: Vec<_> = state
        .roots
        .iter()
        .filter(|(key, _)| &key.repo_id == repo)
        .map(|(_, head)| {
            PersistedSearchCorpusGenerationRootV1::from_generation(
                &head.generation,
                head.activation_sequence,
            )
        })
        .collect();
    let streams: Vec<_> = history
        .into_iter()
        .flat_map(|history| history.streams.iter())
        .map(|(id, head)| (id.clone(), head.active.clone(), head.pending.clone()))
        .collect();
    let events: Vec<_> = history
        .into_iter()
        .flat_map(|history| {
            history
                .records
                .iter()
                .map(move |(key, record)| (history, key, record))
        })
        .map(|(history, key, record)| {
            let bound = match &record.state {
                SourceEventStateV1::Allocated => None,
                SourceEventStateV1::Bound { binding, phase } => Some((
                    binding.target.clone(),
                    binding.journal_key.batch_digest.clone(),
                    match phase {
                        SourceEventPhaseV1::Pending => 0,
                        SourceEventPhaseV1::Staged => 1,
                        SourceEventPhaseV1::Active => 2,
                    },
                )),
            };
            (
                record.allocation.request.event.clone(),
                record.allocation.request.revision_id.clone(),
                record.allocation.generation,
                record.allocation.root_incarnation,
                bound,
                history.lexical_reservations.get(key).map(|reservation| {
                    (
                        (reservation.file.first, reservation.file.count),
                        (reservation.native_text.first, reservation.native_text.count),
                    )
                }),
            )
        })
        .collect();
    let counters: Vec<_> = history
        .into_iter()
        .flat_map(|history| history.high_water.iter())
        .map(|(revision, generation)| (revision.clone(), *generation))
        .collect();
    let row: EnvelopeRow = (
        REPOSITORY_ENVELOPE_FORMAT_VERSION,
        repo.clone(),
        Rows(roots),
        Rows(streams),
        Rows(events),
        Rows(counters),
        Rows(
            history
                .into_iter()
                .flat_map(|history| history.lexical_high_water.iter())
                .map(|(&(incarnation, namespace), &water)| (incarnation, namespace, water))
                .collect(),
        ),
    );
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, &row).map_err(|error| capacity(&error.to_string()))?;
    let mut bytes = Vec::with_capacity(counter.0);
    serde_json::to_writer(&mut bytes, &row).map_err(|error| corrupt(error.to_string()))?;
    Ok(bytes)
}
struct ByteCounter(usize);
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .0
            .checked_add(bytes.len())
            .filter(|next| *next <= MAX_ENVELOPE_BYTES)
            .ok_or_else(|| std::io::Error::other("repository envelope byte limit exceeded"))?;
        self.0 = next;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn read_bounded(path: &Path) -> Result<Vec<u8>, CoreError> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|error| corrupt(error.to_string()))?;
        std::fs::File::from(fd)
    };
    #[cfg(not(unix))]
    let file = {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|error| corrupt(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(corrupt("symlink envelope"));
        }
        std::fs::File::open(path).map_err(|error| corrupt(error.to_string()))?
    };
    let metadata = file
        .metadata()
        .map_err(|error| corrupt(error.to_string()))?;
    if !metadata.is_file() {
        return Err(corrupt("nonregular envelope"));
    }
    if metadata.len()
        > u64::try_from(MAX_ENVELOPE_BYTES).map_err(|error| corrupt(error.to_string()))?
    {
        return Err(capacity("persisted byte limit"));
    }
    let mut bytes = Vec::new();
    let _read = file
        .take(u64::try_from(MAX_ENVELOPE_BYTES + 1).map_err(|error| corrupt(error.to_string()))?)
        .read_to_end(&mut bytes)
        .map_err(|error| corrupt(error.to_string()))?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(capacity("persisted byte limit"));
    }
    Ok(bytes)
}
