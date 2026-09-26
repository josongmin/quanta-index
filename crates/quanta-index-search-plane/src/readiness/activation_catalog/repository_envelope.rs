//! One durable replacement contains all roots and source-event lineage for a
//! repository. Counts and bytes are bounded; retained event identities never
//! expire or get evicted to make room for a new publication.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;

use quanta_index_contract::{
    GenerationSnapshot, IngestOperationKindV1, RepoId, RevisionId, SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, IdempotencyKeyV1, SourceEventBindingV1, SourceEventPhaseV1, SourceEventRecordV1,
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
const FORMAT: u32 = 2;
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
type EventRow = (SourcePublicationEvent, GenerationSnapshot, String, u8);
type EnvelopeRow = (
    u32,
    RepoId,
    Rows<PersistedSearchCorpusGenerationRootV1, MAX_ROOTS>,
    Rows<StreamRow, MAX_STREAMS>,
    Rows<EventRow, MAX_EVENTS>,
);

pub(super) fn decode(path: &Path, bytes: &[u8]) -> Result<(RepoId, CatalogState), CoreError> {
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(capacity("persisted byte limit"));
    }
    let (format, repo, roots, streams, events): EnvelopeRow =
        serde_json::from_slice(bytes).map_err(|error| corrupt(error.to_string()))?;
    if format != FORMAT
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
    for (event, target, batch_digest, phase) in events.0 {
        let key = (event.stream_id.clone(), event.event_id.clone());
        if previous_event
            .as_ref()
            .is_some_and(|previous| previous >= &key)
            || target.repo_id != repo
        {
            return Err(corrupt(
                "event rows must be sorted, unique and belong to the envelope repository",
            ));
        }
        previous_event = Some(key.clone());
        let journal_key = IdempotencyKeyV1 {
            kind: IngestOperationKindV1::SearchCorpus,
            repo_id: target.repo_id.clone(),
            revision_id: target.revision_id.clone(),
            generation: target.manifest_generation,
            batch_digest,
        };
        let binding = SourceEventBindingV1 {
            event,
            target,
            journal_key,
        };
        bound_manifest_token(&binding.target.manifest_digest)?;
        binding
            .validate()
            .map_err(|error| corrupt(error.to_string()))?;
        let phase = match phase {
            0 => SourceEventPhaseV1::Pending,
            1 => SourceEventPhaseV1::Staged,
            2 => SourceEventPhaseV1::Active,
            _ => return Err(corrupt("unknown source event phase")),
        };
        let _prior = history
            .records
            .insert(key, SourceEventRecordV1 { binding, phase });
    }
    validate_history(&history)?;
    let _prior = state.histories.insert(repo.clone(), history);
    Ok((repo, state))
}

pub(super) fn validate_history(history: &EventHistory) -> Result<(), CoreError> {
    if history.streams.len() > MAX_STREAMS || history.records.len() > MAX_EVENTS {
        return Err(capacity("stream or retained event limit"));
    }
    for (stream, head) in &history.streams {
        for (id, active) in [(head.active.as_ref(), true), (head.pending.as_ref(), false)] {
            if let Some(id) = id {
                let record = history
                    .records
                    .get(&(stream.clone(), id.clone()))
                    .ok_or_else(|| corrupt("stream head references missing event"))?;
                if (record.phase == SourceEventPhaseV1::Active) != active {
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
            if record.phase != SourceEventPhaseV1::Active {
                return Err(corrupt("unaccepted ancestor in active lineage"));
            }
            cursor = record.binding.event.expected_base_event_id.as_ref();
        }
    }
    for ((stream, event_id), record) in &history.records {
        if record.phase == SourceEventPhaseV1::Active
            && !accepted.contains(&(stream.clone(), event_id.clone()))
        {
            return Err(corrupt("orphan accepted event outside active lineage"));
        }
        let head = history
            .streams
            .get(stream)
            .ok_or_else(|| corrupt("event has no stream"))?;
        if record.binding.event.stream_id != *stream || record.binding.event.event_id != *event_id {
            return Err(corrupt("event map key mismatch"));
        }
        if record.phase != SourceEventPhaseV1::Active
            && (head.pending.as_ref() != Some(event_id)
                || record.binding.event.expected_base_event_id != head.active)
        {
            return Err(corrupt(
                "unresolved event does not own the pending slot or active base",
            ));
        }
        if let Some(base) = &record.binding.event.expected_base_event_id {
            let base_record = history
                .records
                .get(&(stream.clone(), base.clone()))
                .ok_or_else(|| corrupt("event base was not retained"))?;
            if base_record.phase != SourceEventPhaseV1::Active {
                return Err(corrupt("event base is not active history"));
            }
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
        .flat_map(|history| history.records.values())
        .map(|record| {
            (
                record.binding.event.clone(),
                record.binding.target.clone(),
                record.binding.journal_key.batch_digest.clone(),
                match record.phase {
                    SourceEventPhaseV1::Pending => 0,
                    SourceEventPhaseV1::Staged => 1,
                    SourceEventPhaseV1::Active => 2,
                },
            )
        })
        .collect();
    let row: EnvelopeRow = (
        FORMAT,
        repo.clone(),
        Rows(roots),
        Rows(streams),
        Rows(events),
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
    if metadata.len() > MAX_ENVELOPE_BYTES as u64 {
        return Err(capacity("persisted byte limit"));
    }
    let mut bytes = Vec::new();
    let _read = file
        .take((MAX_ENVELOPE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| corrupt(error.to_string()))?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(capacity("persisted byte limit"));
    }
    Ok(bytes)
}
