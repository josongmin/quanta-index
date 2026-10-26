//! Source-bound capability and producer-event commitments. Missing coverage
//! cannot be interpreted as an empty, completely indexed source universe.

use std::fmt;
use std::io::Write;

use quanta_index_contract_base::SourceFileRevision;
use serde::de::Visitor;
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};

use crate::ChunkRecord;
use crate::lex::{LanguageCode, SymbolRecord};

/// Completeness is explicit and independent of the number of indexed facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolCoverage {
    Complete { symbol_count: u64 },
    NotRequested,
    Unsupported,
    ParseFailed,
    ProducerFailed,
}

/// Producer promise needed to rule out a symbol match from complete source
/// bytes when that file's symbol census is incomplete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolNameSourcePolicyV1 {
    /// No source-spelling guarantee; incomplete coverage must be refused.
    Unspecified,
    /// Every ASCII local name the producer can emit occurs verbatim in its
    /// definition's source-byte span. Ingest checks every emitted record.
    RawAsciiLocalName,
}

/// Producer-attested source revision and policy, bound to the exact unit set.
/// The source hash is not independent verification of bytes absent from ingress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileCoverage {
    pub source: SourceFileRevision,
    pub language: LanguageCode,
    pub producer_policy_sha256: [u8; 32],
    pub symbol_name_source_policy: SymbolNameSourcePolicyV1,
    pub unit_set_sha256: [u8; 32],
    pub text_admitted: bool,
    pub symbols: SymbolCoverage,
}

impl Serialize for SymbolNameSourcePolicyV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Unspecified => "unspecified",
            Self::RawAsciiLocalName => "raw_ascii_local_name_v1",
        })
    }
}

impl<'de> Deserialize<'de> for SymbolNameSourcePolicyV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PolicyVisitor;
        impl Visitor<'_> for PolicyVisitor {
            type Value = SymbolNameSourcePolicyV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a symbol name source policy")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "unspecified" => Ok(SymbolNameSourcePolicyV1::Unspecified),
                    "raw_ascii_local_name_v1" => Ok(SymbolNameSourcePolicyV1::RawAsciiLocalName),
                    _ => Err(E::unknown_variant(
                        value,
                        &["unspecified", "raw_ascii_local_name_v1"],
                    )),
                }
            }
        }
        deserializer.deserialize_str(PolicyVisitor)
    }
}

#[path = "coverage_snapshot.rs"]
mod snapshot;
pub use snapshot::{FileCoverageIter, FileCoverageSnapshot};

/// An event is identified independently of a target generation or ingest batch.
/// The durable publication owner checks replay and expected-base lineage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePublicationEvent {
    pub stream_id: String,
    pub event_id: String,
    pub expected_base_event_id: Option<String>,
    pub payload_sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceCoverageError {
    InvalidSource(&'static str),
    InvalidEventToken {
        field: &'static str,
    },
    SelfReferentialEvent,
    DuplicateCandidateId(String),
    /// Borrowed-scratch validation cannot allocate an owned error payload.
    DuplicateCandidateIdWithoutPayload,
    ScratchLengthMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    Encode(String),
}

impl fmt::Display for SourceCoverageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource(error) => write!(formatter, "source-file identity: {error}"),
            Self::InvalidEventToken { field } => write!(
                formatter,
                "source event {field} must contain 1..=512 printable ASCII bytes without whitespace"
            ),
            Self::SelfReferentialEvent => {
                formatter.write_str("source event cannot be its own base")
            }
            Self::DuplicateCandidateId(id) => {
                write!(formatter, "duplicate source-file unit ID: {id}")
            }
            Self::DuplicateCandidateIdWithoutPayload => {
                formatter.write_str("duplicate source-file unit ID")
            }
            Self::ScratchLengthMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "source-file {field} scratch length {actual} differs from expected {expected}"
            ),
            Self::Encode(error) => write!(formatter, "source-file unit encoding: {error}"),
        }
    }
}

impl std::error::Error for SourceCoverageError {}

impl SourceFileCoverage {
    pub fn validate(&self) -> Result<(), SourceCoverageError> {
        self.source
            .validate()
            .map_err(SourceCoverageError::InvalidSource)
    }
}

impl SourcePublicationEvent {
    pub fn validate(&self) -> Result<(), SourceCoverageError> {
        for (field, value) in [
            ("stream_id", Some(self.stream_id.as_str())),
            ("event_id", Some(self.event_id.as_str())),
            (
                "expected_base_event_id",
                self.expected_base_event_id.as_deref(),
            ),
        ] {
            if let Some(value) = value
                && !admitted::valid_event_token_v1(value)
            {
                return Err(SourceCoverageError::InvalidEventToken { field });
            }
        }
        if self.expected_base_event_id.as_deref() == Some(self.event_id.as_str()) {
            return Err(SourceCoverageError::SelfReferentialEvent);
        }
        Ok(())
    }
}

impl Serialize for SymbolCoverage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (tag, count) = match *self {
            Self::Complete { symbol_count } => ("complete", Some(symbol_count)),
            Self::NotRequested => ("not_requested", None),
            Self::Unsupported => ("unsupported", None),
            Self::ParseFailed => ("parse_failed", None),
            Self::ProducerFailed => ("producer_failed", None),
        };
        let mut state =
            serializer.serialize_struct("SymbolCoverage", if count.is_some() { 2 } else { 1 })?;
        state.serialize_field("state", tag)?;
        if let Some(count) = count {
            state.serialize_field("symbol_count", &count)?;
        }
        state.end()
    }
}

// Closed maps: unknown, missing and duplicate fields all fail. The shared
// ordinary/admitted visitors live in `admitted`; this macro owns wire encoding.
macro_rules! coverage_record_serde {
    ($name:ident { $($field:ident: $ty:ty),+ $(,)? } validate $validate:expr) => {
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                ($validate)(self).map_err(serde::ser::Error::custom)?;
                let mut record = serializer.serialize_struct(stringify!($name), FIELDS.len())?;
                $(record.serialize_field(stringify!($field), &self.$field)?;)+
                record.end()
            }
        }

    };
}

coverage_record_serde!(SourceFileCoverage {
    source: SourceFileRevision,
    language: LanguageCode,
    producer_policy_sha256: [u8; 32],
    symbol_name_source_policy: SymbolNameSourcePolicyV1,
    unit_set_sha256: [u8; 32],
    text_admitted: bool,
    symbols: SymbolCoverage,
} validate SourceFileCoverage::validate);

coverage_record_serde!(SourcePublicationEvent {
    stream_id: String,
    event_id: String,
    expected_base_event_id: Option<String>,
    payload_sha256: [u8; 32],
} validate SourcePublicationEvent::validate);

mod admitted;

#[derive(Debug)]
pub enum SourceCoverageProgressError<E> {
    Invalid(SourceCoverageError),
    Progress(E),
}

impl<E> From<SourceCoverageError> for SourceCoverageProgressError<E> {
    fn from(error: SourceCoverageError) -> Self {
        Self::Invalid(error)
    }
}

#[cfg(test)]
pub(crate) fn hash_canonical_cbor_with_progress<T, E, F>(
    domain: &[u8],
    value: &T,
    progress: &mut F,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    T: Serialize,
    F: FnMut() -> Result<(), E>,
{
    hash_canonical_cbor_with_progress_and_work(domain, value, progress, &mut |_| Ok(()))
}

pub(crate) fn hash_canonical_cbor_with_progress_and_work<T, E, F, W>(
    domain: &[u8],
    value: &T,
    progress: &mut F,
    work: &mut W,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    T: Serialize,
    F: FnMut() -> Result<(), E>,
    W: FnMut(usize) -> Result<(), E>,
{
    const HASH_WINDOW_BYTES: usize = 64 * 1024;
    const HASH_WINDOW_WRITES: usize = 256;

    struct DigestWriter<'a, E, F, W> {
        hasher: Sha256,
        progress: &'a mut F,
        work: &'a mut W,
        progress_error: Option<E>,
        bytes_since_progress: usize,
        writes_since_progress: usize,
    }

    impl<E, F: FnMut() -> Result<(), E>, W: FnMut(usize) -> Result<(), E>> DigestWriter<'_, E, F, W> {
        fn checkpoint(&mut self) -> std::io::Result<()> {
            match (self.progress)() {
                Ok(()) => {
                    self.bytes_since_progress = 0;
                    self.writes_since_progress = 0;
                    Ok(())
                }
                Err(error) => {
                    self.progress_error = Some(error);
                    Err(std::io::Error::other("source coverage progress refused"))
                }
            }
        }
    }

    impl<E, F: FnMut() -> Result<(), E>, W: FnMut(usize) -> Result<(), E>> Write
        for DigestWriter<'_, E, F, W>
    {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            for chunk in bytes.chunks(HASH_WINDOW_BYTES) {
                if self.bytes_since_progress.saturating_add(chunk.len()) > HASH_WINDOW_BYTES
                    || self.writes_since_progress >= HASH_WINDOW_WRITES
                {
                    self.checkpoint()?;
                }
                if let Err(error) = (self.work)(chunk.len()) {
                    self.progress_error = Some(error);
                    return Err(std::io::Error::other("source coverage work refused"));
                }
                self.hasher.update(chunk);
                self.bytes_since_progress = self
                    .bytes_since_progress
                    .checked_add(chunk.len())
                    .ok_or_else(|| std::io::Error::other("source coverage byte count overflow"))?;
                self.writes_since_progress = self
                    .writes_since_progress
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("source coverage write count overflow"))?;
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    progress().map_err(SourceCoverageProgressError::Progress)?;
    let mut writer = DigestWriter {
        hasher: Sha256::new(),
        progress,
        work,
        progress_error: None,
        bytes_since_progress: 0,
        writes_since_progress: 0,
    };
    (writer.work)(domain.len()).map_err(SourceCoverageProgressError::Progress)?;
    writer.hasher.update(domain);
    let encoded = ciborium::into_writer(value, &mut writer);
    if let Some(error) = writer.progress_error.take() {
        return Err(SourceCoverageProgressError::Progress(error));
    }
    encoded.map_err(|error| {
        SourceCoverageProgressError::Invalid(SourceCoverageError::Encode(error.to_string()))
    })?;
    (writer.progress)().map_err(SourceCoverageProgressError::Progress)?;
    Ok(writer.hasher.finalize().into())
}

/// SHA-256 of the domain followed by a fixed-order CBOR pair `(chunks, symbols)`.
///
/// Each vector is sorted by its typed ID and uses the complete record's manual
/// wire serializer. Input order has no authority; duplicate IDs never collapse.
pub fn source_file_unit_set_sha256(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
) -> Result<[u8; 32], SourceCoverageError> {
    match source_file_unit_set_sha256_with_progress(chunks, symbols, &mut || {
        Ok::<_, std::convert::Infallible>(())
    }) {
        Ok(digest) => Ok(digest),
        Err(SourceCoverageProgressError::Invalid(error)) => Err(error),
        Err(SourceCoverageProgressError::Progress(never)) => match never {},
    }
}

/// The canonical unit-set digest with bounded collection and hash checkpoints.
/// Sorting is bracketed by checkpoints but cannot be interrupted internally.
pub fn source_file_unit_set_sha256_with_progress<E, F>(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
    progress: &mut F,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
{
    // The ordinary API owns its temporary index storage. Native callers use
    // the borrowed-scratch entry below and must fund their scratch separately.
    progress().map_err(SourceCoverageProgressError::Progress)?;
    let mut chunk_order = vec![0; chunks.len()];
    let mut symbol_order = vec![0; symbols.len()];
    source_file_unit_set_sha256_with_scratch_impl(
        chunks,
        symbols,
        &mut chunk_order,
        &mut symbol_order,
        progress,
        |id| SourceCoverageError::DuplicateCandidateId(id.to_owned()),
        None,
        &mut |_| Ok(()),
    )
}

/// Hash the same canonical unit-set wire using caller-owned index scratch.
///
/// The two slices must have exactly one slot per input record. This function
/// allocates no collection backing on success. The caller owns scratch birth,
/// its funding, and the backing-before-lease drop order; a slice alone proves
/// none of those properties. Invalid record serialization may still construct
/// an ordinary diagnostic error.
pub fn source_file_unit_set_sha256_with_borrowed_scratch<E, F>(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
    chunk_order: &mut [usize],
    symbol_order: &mut [usize],
    progress: &mut F,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
{
    source_file_unit_set_sha256_with_scratch_impl(
        chunks,
        symbols,
        chunk_order,
        symbol_order,
        progress,
        |_id| SourceCoverageError::DuplicateCandidateIdWithoutPayload,
        None,
        &mut |_| Ok(()),
    )
}

type UnitSetComparisonV3<'a, E> =
    dyn FnMut(usize, usize) -> Result<std::cmp::Ordering, SourceCoverageProgressError<E>> + 'a;
type UnitSetSortCallbackV3<'a, E> = dyn FnMut(
        &mut [usize],
        &mut UnitSetComparisonV3<'_, E>,
    ) -> Result<(), SourceCoverageProgressError<E>>
    + 'a;
type UnitSetComparisonAdmissionV3<'a, E> = dyn FnMut(&str, &str) -> Result<(), E> + 'a;

// Borrowed invocation arguments, not an execution/resource port owned by the
// wire contract. The producer retains its actual sorter and admission owner.
struct UnitSetSortCallbacksV3<'a, E> {
    sort: &'a mut UnitSetSortCallbackV3<'a, E>,
    admit_comparison: &'a mut UnitSetComparisonAdmissionV3<'a, E>,
}

/// Canonical unit-set digest over borrowed scratch with caller-owned sorting.
///
/// No additional collection is allocated. Both ordering and the complete
/// index permutation are checked before duplicate checks or hashing.
/// Comparison admission also covers the post-sort certification comparisons.
/// The caller supplies borrowed callbacks: sorting must use the given canonical
/// comparator, admit moves before swaps, and preserve its original refusal.
/// Comparison admission runs before ID comparison and through certification.
pub fn source_file_unit_set_sha256_with_borrowed_scratch_and_sort_v3<E, F, S, C>(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
    chunk_order: &mut [usize],
    symbol_order: &mut [usize],
    progress: &mut F,
    sort: &mut S,
    admit_comparison: &mut C,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
    S: FnMut(
        &mut [usize],
        &mut UnitSetComparisonV3<'_, E>,
    ) -> Result<(), SourceCoverageProgressError<E>>,
    C: FnMut(&str, &str) -> Result<(), E>,
{
    source_file_unit_set_sha256_with_borrowed_scratch_sort_and_work_v3(
        chunks,
        symbols,
        chunk_order,
        symbol_order,
        progress,
        sort,
        admit_comparison,
        &mut |_| Ok(()),
    )
}

/// Borrowed unit-set digest with admitted domain and encoded-byte hash work.
///
/// Scratch birth, sorting, and comparison admission stay with the caller;
/// refusal propagates through the original progress error type.
/// The separate borrowed callbacks keep their original admission and refusal
/// owners while sharing the canonical digest implementation.
pub fn source_file_unit_set_sha256_with_borrowed_scratch_sort_and_work_v3<E, F, S, C, W>(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
    chunk_order: &mut [usize],
    symbol_order: &mut [usize],
    progress: &mut F,
    sort: &mut S,
    admit_comparison: &mut C,
    work: &mut W,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
    S: FnMut(
        &mut [usize],
        &mut UnitSetComparisonV3<'_, E>,
    ) -> Result<(), SourceCoverageProgressError<E>>,
    C: FnMut(&str, &str) -> Result<(), E>,
    W: FnMut(usize) -> Result<(), E>,
{
    source_file_unit_set_sha256_with_scratch_impl(
        chunks,
        symbols,
        chunk_order,
        symbol_order,
        progress,
        |_id| SourceCoverageError::DuplicateCandidateIdWithoutPayload,
        Some(UnitSetSortCallbacksV3 {
            sort,
            admit_comparison,
        }),
        work,
    )
}

fn sort_and_certify_indices_v3<'a, E>(
    indices: &mut [usize],
    id_at: impl Fn(usize) -> Option<&'a str>,
    sort: &mut UnitSetSortCallbacksV3<'_, E>,
) -> Result<(), SourceCoverageProgressError<E>> {
    let admit_comparison = &mut sort.admit_comparison;
    let mut compare = |left, right| {
        let left_id = id_at(left).ok_or(SourceCoverageError::InvalidSource(
            "unit-set sort index out of range",
        ))?;
        let right_id = id_at(right).ok_or(SourceCoverageError::InvalidSource(
            "unit-set sort index out of range",
        ))?;
        admit_comparison(left_id, right_id).map_err(SourceCoverageProgressError::Progress)?;
        Ok((left_id, left).cmp(&(right_id, right)))
    };
    (sort.sort)(indices, &mut compare)?;
    let mut previous = None;
    for &index in indices.iter() {
        if id_at(index).is_none() {
            return Err(
                SourceCoverageError::InvalidSource("unit-set sort index out of range").into(),
            );
        }
        if let Some(previous) = previous
            && compare(previous, index)? != std::cmp::Ordering::Less
        {
            // Strict ID+ordinal order makes every in-range index distinct.
            // With exactly N slots, that proves a complete N-index permutation.
            return Err(SourceCoverageError::InvalidSource(
                "unit-set sorter did not produce a canonical permutation",
            )
            .into());
        }
        previous = Some(index);
    }
    Ok(())
}

struct OrderedRecords<'a, T> {
    records: &'a [T],
    order: &'a [usize],
}

impl<T: Serialize> Serialize for OrderedRecords<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.order.len()))?;
        for index in self.order {
            let record = self
                .records
                .get(*index)
                .ok_or_else(|| serde::ser::Error::custom("unit-set scratch index out of range"))?;
            sequence.serialize_element(record)?;
        }
        sequence.end()
    }
}

fn source_file_unit_set_sha256_with_scratch_impl<E, F, D, W>(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
    chunk_order: &mut [usize],
    symbol_order: &mut [usize],
    progress: &mut F,
    duplicate_error: D,
    mut sort: Option<UnitSetSortCallbacksV3<'_, E>>,
    work: &mut W,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
    D: FnOnce(&str) -> SourceCoverageError,
    W: FnMut(usize) -> Result<(), E>,
{
    fn advance_cursor(cursor: &mut usize) -> Result<(), SourceCoverageError> {
        *cursor = cursor
            .checked_add(1)
            .ok_or(SourceCoverageError::InvalidSource(
                "unit-set merge ordinal overflow",
            ))?;
        Ok(())
    }

    if chunk_order.len() != chunks.len() {
        return Err(SourceCoverageError::ScratchLengthMismatch {
            field: "chunk",
            expected: chunks.len(),
            actual: chunk_order.len(),
        }
        .into());
    }
    if symbol_order.len() != symbols.len() {
        return Err(SourceCoverageError::ScratchLengthMismatch {
            field: "symbol",
            expected: symbols.len(),
            actual: symbol_order.len(),
        }
        .into());
    }
    progress().map_err(SourceCoverageProgressError::Progress)?;
    for (index, slot) in chunk_order.iter_mut().enumerate() {
        if index != 0 && index % 32 == 0 {
            progress().map_err(SourceCoverageProgressError::Progress)?;
        }
        *slot = index;
    }
    for (index, slot) in symbol_order.iter_mut().enumerate() {
        if index != 0 && index % 32 == 0 {
            progress().map_err(SourceCoverageProgressError::Progress)?;
        }
        *slot = index;
    }
    progress().map_err(SourceCoverageProgressError::Progress)?;
    if let Some(sort) = sort.as_mut() {
        sort_and_certify_indices_v3(
            chunk_order,
            |index| chunks.get(index).map(|row| row.chunk_id.as_str()),
            sort,
        )?;
        sort_and_certify_indices_v3(
            symbol_order,
            |index| symbols.get(index).map(|row| row.symbol_id.as_str()),
            sort,
        )?;
    } else {
        // Tie-break by original ordinal so the first adjacent equal pair is the
        // first duplicate in input order, even when three or more IDs are equal.
        chunk_order.sort_unstable_by_key(|index| {
            (chunks.get(*index).map(|row| row.chunk_id.as_str()), *index)
        });
        symbol_order.sort_unstable_by_key(|index| {
            (
                symbols.get(*index).map(|row| row.symbol_id.as_str()),
                *index,
            )
        });
    }
    progress().map_err(SourceCoverageProgressError::Progress)?;

    // Pick the earliest second occurrence in the original chunks-then-symbols
    // sequence. The ordinary API retains its prior duplicate-error payload.
    let mut duplicate: Option<(usize, &str)> = None;
    for (ordinal, pair) in chunk_order.windows(2).enumerate() {
        if ordinal != 0 && ordinal % 32 == 0 {
            progress().map_err(SourceCoverageProgressError::Progress)?;
        }
        let [left_index, right_index] = pair else {
            continue;
        };
        let (Some(left), Some(right)) = (chunks.get(*left_index), chunks.get(*right_index)) else {
            return Err(SourceCoverageError::InvalidSource("chunk scratch index").into());
        };
        work(
            left.chunk_id
                .as_str()
                .len()
                .checked_add(right.chunk_id.as_str().len())
                .ok_or(SourceCoverageError::InvalidSource(
                    "unit-set comparison work overflow",
                ))?,
        )
        .map_err(SourceCoverageProgressError::Progress)?;
        if left.chunk_id == right.chunk_id {
            let second = (*left_index).max(*right_index);
            if duplicate.is_none_or(|(prior, _)| second < prior) {
                duplicate = Some((second, left.chunk_id.as_str()));
            }
        }
    }
    for (ordinal, pair) in symbol_order.windows(2).enumerate() {
        if ordinal != 0 && ordinal % 32 == 0 {
            progress().map_err(SourceCoverageProgressError::Progress)?;
        }
        let [left_index, right_index] = pair else {
            continue;
        };
        let (Some(left), Some(right)) = (symbols.get(*left_index), symbols.get(*right_index))
        else {
            return Err(SourceCoverageError::InvalidSource("symbol scratch index").into());
        };
        work(
            left.symbol_id
                .as_str()
                .len()
                .checked_add(right.symbol_id.as_str().len())
                .ok_or(SourceCoverageError::InvalidSource(
                    "unit-set comparison work overflow",
                ))?,
        )
        .map_err(SourceCoverageProgressError::Progress)?;
        if left.symbol_id == right.symbol_id {
            let second = chunks
                .len()
                .checked_add((*left_index).max(*right_index))
                .ok_or(SourceCoverageError::InvalidSource(
                    "unit-set ordinal overflow",
                ))?;
            if duplicate.is_none_or(|(prior, _)| second < prior) {
                duplicate = Some((second, left.symbol_id.as_str()));
            }
        }
    }
    let mut chunk_at = 0usize;
    let mut symbol_at = 0usize;
    while let (Some(&chunk_index), Some(&symbol_index)) =
        (chunk_order.get(chunk_at), symbol_order.get(symbol_at))
    {
        let inspected =
            chunk_at
                .checked_add(symbol_at)
                .ok_or(SourceCoverageError::InvalidSource(
                    "unit-set merge ordinal overflow",
                ))?;
        if inspected != 0 && inspected % 32 == 0 {
            progress().map_err(SourceCoverageProgressError::Progress)?;
        }
        let (Some(chunk), Some(symbol)) = (chunks.get(chunk_index), symbols.get(symbol_index))
        else {
            return Err(SourceCoverageError::InvalidSource("unit-set scratch index").into());
        };
        work(
            chunk
                .chunk_id
                .as_str()
                .len()
                .checked_add(symbol.symbol_id.as_str().len())
                .ok_or(SourceCoverageError::InvalidSource(
                    "unit-set comparison work overflow",
                ))?,
        )
        .map_err(SourceCoverageProgressError::Progress)?;
        match chunk.chunk_id.as_str().cmp(symbol.symbol_id.as_str()) {
            core::cmp::Ordering::Less => advance_cursor(&mut chunk_at)?,
            core::cmp::Ordering::Greater => advance_cursor(&mut symbol_at)?,
            core::cmp::Ordering::Equal => {
                let second = chunks.len().checked_add(symbol_index).ok_or(
                    SourceCoverageError::InvalidSource("unit-set ordinal overflow"),
                )?;
                if duplicate.is_none_or(|(prior, _)| second < prior) {
                    duplicate = Some((second, chunk.chunk_id.as_str()));
                }
                advance_cursor(&mut chunk_at)?;
                advance_cursor(&mut symbol_at)?;
            }
        }
    }
    progress().map_err(SourceCoverageProgressError::Progress)?;
    if let Some((_second, id)) = duplicate {
        return Err(duplicate_error(id).into());
    }
    hash_canonical_cbor_with_progress_and_work(
        b"quanta-index:source-file-unit-set:v1\0",
        &(
            OrderedRecords {
                records: chunks,
                order: chunk_order,
            },
            OrderedRecords {
                records: symbols,
                order: symbol_order,
            },
        ),
        progress,
        work,
    )
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]
mod tests {
    use super::*;
    use crate::{ChunkId, RepoId, RepoRelativePath, RevisionId};
    use quanta_index_contract_base::SourceFileKey;
    use std::collections::BTreeSet;
    use std::fmt::Write as _;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn completed_digest<T>(
        result: Result<T, SourceCoverageProgressError<std::convert::Infallible>>,
    ) -> Result<T, SourceCoverageError> {
        match result {
            Ok(value) => Ok(value),
            Err(SourceCoverageProgressError::Invalid(error)) => Err(error),
            Err(SourceCoverageProgressError::Progress(never)) => match never {},
        }
    }

    fn event() -> SourcePublicationEvent {
        SourcePublicationEvent {
            stream_id: "producer-main".into(),
            event_id: "event-2".into(),
            expected_base_event_id: Some("event-1".into()),
            payload_sha256: [3; 32],
        }
    }

    fn coverage() -> Result<SourceFileCoverage, Box<dyn std::error::Error>> {
        Ok(SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source")?,
                    repo_relative_path: RepoRelativePath::new("empty.rs"),
                },
                revision_id: RevisionId::new("revision")?,
                source_sha256: [2; 32],
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [1; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: source_file_unit_set_sha256(&[], &[])?,
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 0 },
        })
    }

    #[test]
    fn all_symbol_states_and_complete_zero_round_trip_distinctly() -> TestResult {
        let states = [
            SymbolCoverage::Complete { symbol_count: 0 },
            SymbolCoverage::Complete { symbol_count: 2 },
            SymbolCoverage::NotRequested,
            SymbolCoverage::Unsupported,
            SymbolCoverage::ParseFailed,
            SymbolCoverage::ProducerFailed,
        ];
        let mut encoded = BTreeSet::new();
        for state in states {
            let mut bytes = Vec::new();
            ciborium::into_writer(&state, &mut bytes)?;
            assert_eq!(
                ciborium::from_reader::<SymbolCoverage, _>(bytes.as_slice())?,
                state
            );
            assert!(encoded.insert(bytes));
        }
        Ok(())
    }

    #[test]
    fn malformed_symbol_states_never_become_complete() {
        for invalid in [
            r"{}",
            r#"{"state":"complete"}"#,
            r#"{"state":"complete","symbol_count":null}"#,
            r#"{"state":"complete","symbol_count":-1}"#,
            r#"{"state":"complete","symbol_count":0,"symbol_count":1}"#,
            r#"{"state":"complete","symbol_count":0,"extra":false}"#,
            r#"{"state":"unknown"}"#,
            r#"{"state":"parse_failed","symbol_count":0}"#,
            r#"{"state":"parse_failed","state":"complete","symbol_count":0}"#,
        ] {
            assert!(
                serde_json::from_str::<SymbolCoverage>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn coverage_and_event_closed_maps_reject_missing_unknown_duplicate_fields() -> TestResult {
        fn exercise<T>(value: &T) -> TestResult
        where
            T: Serialize + for<'de> Deserialize<'de> + Eq + fmt::Debug,
        {
            let mut bytes = Vec::new();
            ciborium::into_writer(value, &mut bytes)?;
            assert_eq!(&ciborium::from_reader::<T, _>(bytes.as_slice())?, value);
            let ciborium::Value::Map(fields) = ciborium::from_reader(bytes.as_slice())? else {
                return Err("fixture must encode a map".into());
            };
            for field in &fields {
                let mut duplicate = fields.clone();
                duplicate.push(field.clone());
                let missing = fields
                    .iter()
                    .filter(|entry| entry.0 != field.0)
                    .cloned()
                    .collect();
                let mut unknown = fields.clone();
                unknown.push((
                    ciborium::Value::Text("unknown".into()),
                    ciborium::Value::Null,
                ));
                for invalid in [duplicate, missing, unknown] {
                    let mut bytes = Vec::new();
                    ciborium::into_writer(&ciborium::Value::Map(invalid), &mut bytes)?;
                    assert!(ciborium::from_reader::<T, _>(bytes.as_slice()).is_err());
                }
            }
            Ok(())
        }
        exercise(&coverage()?)?;
        exercise(&event())?;
        let mut initial = event();
        initial.expected_base_event_id = None;
        exercise(&initial)
    }

    #[test]
    fn event_tokens_are_bounded_and_validate_before_serialization() -> TestResult {
        for invalid in [
            String::new(),
            "a b".into(),
            "a\n".into(),
            "é".into(),
            "x".repeat(513),
        ] {
            for field in ["stream", "event", "base"] {
                let mut request = event();
                match field {
                    "stream" => request.stream_id.clone_from(&invalid),
                    "event" => request.event_id.clone_from(&invalid),
                    _ => request.expected_base_event_id = Some(invalid.clone()),
                }
                assert!(request.validate().is_err());
                assert!(serde_json::to_string(&request).is_err());
            }
        }
        let mut boundary = event();
        boundary.event_id = "x".repeat(512);
        boundary.validate()?;
        boundary.expected_base_event_id = Some(boundary.event_id.clone());
        assert_eq!(
            boundary.validate(),
            Err(SourceCoverageError::SelfReferentialEvent)
        );
        Ok(())
    }

    pub(super) fn chunk(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new("a.rs"),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: text.into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    }

    #[test]
    fn unit_digest_is_order_independent_and_binds_complete_records() -> TestResult {
        let first = chunk("a", "alpha")?;
        let second = chunk("b", "beta")?;
        let canonical = source_file_unit_set_sha256(&[first.clone(), second.clone()], &[])?;
        assert_eq!(
            canonical,
            source_file_unit_set_sha256(&[second.clone(), first.clone()], &[])?
        );
        for field in ["text", "path", "source", "span", "parent"] {
            let mut changed = first.clone();
            match field {
                "text" => changed.text = "other".into(),
                "path" => changed.repo_relative_path = RepoRelativePath::new("b.rs"),
                "source" => changed.source_repo_id = Some(RepoId::new("different-source")?),
                "span" => changed.start_line = 2,
                _ => changed.parent_chunk_id = Some(ChunkId::new("parent")),
            }
            assert_ne!(
                canonical,
                source_file_unit_set_sha256(&[changed, second.clone()], &[])?
            );
        }
        assert!(matches!(
            source_file_unit_set_sha256(&[first.clone(), first], &[]),
            Err(SourceCoverageError::DuplicateCandidateId(_))
        ));
        Ok(())
    }

    #[test]
    fn unit_digest_progress_stops_inside_large_canonical_body() -> TestResult {
        let large = chunk("a", &"x".repeat(262_144))?;
        let original = source_file_unit_set_sha256(std::slice::from_ref(&large), &[])?;
        let mut checkpoints = 0usize;
        let resumed = source_file_unit_set_sha256_with_progress(
            std::slice::from_ref(&large),
            &[],
            &mut || {
                checkpoints = checkpoints.saturating_add(1);
                Ok::<_, &'static str>(())
            },
        );
        assert!(matches!(resumed, Ok(digest) if digest == original));
        assert!(checkpoints >= 5);

        let mut checkpoints = 0usize;
        let interrupted = source_file_unit_set_sha256_with_progress(
            std::slice::from_ref(&large),
            &[],
            &mut || {
                checkpoints = checkpoints.saturating_add(1);
                if checkpoints == 5 {
                    Err("stop")
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(
            interrupted,
            Err(SourceCoverageProgressError::Progress("stop"))
        ));
        assert_eq!(
            source_file_unit_set_sha256(std::slice::from_ref(&large), &[])?,
            original
        );
        Ok(())
    }

    #[test]
    fn empty_unit_digest_matches_fixed_cbor_golden() -> TestResult {
        // Independent oracle: SHA256(domain || 0x82 0x80 0x80), the CBOR
        // pair of empty arrays. No source-file-byte completeness claim.
        let digest = source_file_unit_set_sha256(&[], &[])?;
        let mut hex = String::with_capacity(64);
        for byte in digest {
            write!(&mut hex, "{byte:02x}")?;
        }
        assert_eq!(
            hex,
            "03b78693ecc7e7294c11af4366b387162c2ebe9547683a8ac614a717f0027d32"
        );
        Ok(())
    }

    #[test]
    fn symbol_records_are_committed_and_cross_kind_ids_are_rejected() -> TestResult {
        use crate::SymbolId;
        use crate::lex::{SymbolKindCode, SymbolKindFamily, SymbolRelationship, SymbolSpan};

        let symbol = SymbolRecord {
            symbol_id: SymbolId::new("symbol-a"),
            repo_relative_path: RepoRelativePath::new("a.rs"),
            language: LanguageCode::new("rust")?,
            symbol_kind: SymbolKindCode::new("function")?,
            symbol_kind_family: Some(SymbolKindFamily::Callable),
            local_name: "name".into(),
            qualified_name: "crate::name".into(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: "a.rs".into(),
                byte_start: 0,
                byte_end: 4,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: None,
            relationship: SymbolRelationship::Def,
        };
        let canonical = source_file_unit_set_sha256(&[], std::slice::from_ref(&symbol))?;
        let mut renamed = symbol.clone();
        renamed.qualified_name = "other::name".into();
        assert_ne!(canonical, source_file_unit_set_sha256(&[], &[renamed])?);
        let mut changed_signature = symbol.clone();
        changed_signature.signature = Some("fn name(x: u32)".into());
        assert_ne!(
            canonical,
            source_file_unit_set_sha256(&[], &[changed_signature])?
        );
        assert!(matches!(
            source_file_unit_set_sha256(
                &[chunk("symbol-a", "name")?],
                std::slice::from_ref(&symbol)
            ),
            Err(SourceCoverageError::DuplicateCandidateId(_))
        ));
        assert!(matches!(
            source_file_unit_set_sha256(&[], &[symbol.clone(), symbol]),
            Err(SourceCoverageError::DuplicateCandidateId(_))
        ));
        Ok(())
    }

    #[test]
    fn borrowed_scratch_reuses_fixed_ordered_cbor_formula() -> TestResult {
        let first = chunk("a", "alpha")?;
        let second = chunk("b", "beta")?;
        let unordered = [second.clone(), first.clone()];
        let mut chunk_order = [usize::MAX; 2];
        let mut symbol_order = [];
        let actual = completed_digest(source_file_unit_set_sha256_with_borrowed_scratch(
            &unordered,
            &[],
            &mut chunk_order,
            &mut symbol_order,
            &mut || Ok::<_, std::convert::Infallible>(()),
        ))?;
        // Independent wire oracle: Ciborium serializes the two already-ordered
        // borrowed records as the original two-array pair, not index slots.
        let expected = completed_digest(hash_canonical_cbor_with_progress(
            b"quanta-index:source-file-unit-set:v1\0",
            &(vec![&first, &second], Vec::<&SymbolRecord>::new()),
            &mut || Ok::<_, std::convert::Infallible>(()),
        ))?;
        assert_eq!(actual, expected);
        assert_eq!(chunk_order, [1, 0]);
        assert_eq!(
            completed_digest(source_file_unit_set_sha256_with_borrowed_scratch(
                &[],
                &[],
                &mut [],
                &mut [],
                &mut || Ok::<_, std::convert::Infallible>(()),
            ))?,
            source_file_unit_set_sha256(&[], &[])?
        );
        Ok(())
    }

    #[test]
    fn borrowed_scratch_refuses_shape_and_duplicate_without_owned_id() -> TestResult {
        let first = chunk("a", "alpha")?;
        let second = chunk("a", "beta")?;
        let mut short = [0usize; 1];
        let mut no_symbols = [];
        let wrong_shape = source_file_unit_set_sha256_with_borrowed_scratch(
            &[first.clone(), second.clone()],
            &[],
            &mut short,
            &mut no_symbols,
            &mut || Ok::<_, std::convert::Infallible>(()),
        );
        assert!(matches!(
            wrong_shape,
            Err(SourceCoverageProgressError::Invalid(
                SourceCoverageError::ScratchLengthMismatch {
                    field: "chunk",
                    expected: 2,
                    actual: 1
                }
            ))
        ));
        let mut order = [usize::MAX; 2];
        let duplicate = source_file_unit_set_sha256_with_borrowed_scratch(
            &[first, second],
            &[],
            &mut order,
            &mut no_symbols,
            &mut || Ok::<_, std::convert::Infallible>(()),
        );
        assert!(matches!(
            duplicate,
            Err(SourceCoverageProgressError::Invalid(
                SourceCoverageError::DuplicateCandidateIdWithoutPayload
            ))
        ));
        let symbol = SymbolRecord {
            symbol_id: crate::SymbolId::new("a"),
            repo_relative_path: RepoRelativePath::new("a.rs"),
            language: LanguageCode::new("rust")?,
            symbol_kind: crate::lex::SymbolKindCode::new("function")?,
            symbol_kind_family: None,
            local_name: "a".into(),
            qualified_name: "crate::a".into(),
            signature: None,
            visibility: None,
            definition_span: crate::lex::SymbolSpan {
                path: "a.rs".into(),
                byte_start: 0,
                byte_end: 1,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: None,
            relationship: crate::lex::SymbolRelationship::Def,
        };
        let mut chunk_order = [0usize; 1];
        let mut symbol_order = [0usize; 1];
        let cross_kind = source_file_unit_set_sha256_with_borrowed_scratch(
            &[chunk("a", "alpha")?],
            &[symbol],
            &mut chunk_order,
            &mut symbol_order,
            &mut || Ok::<_, std::convert::Infallible>(()),
        );
        assert!(matches!(
            cross_kind,
            Err(SourceCoverageProgressError::Invalid(
                SourceCoverageError::DuplicateCandidateIdWithoutPayload
            ))
        ));
        // The ordinary error names the first duplicate in original input
        // order even when another ID has a larger equal-key group.
        let first_duplicate = source_file_unit_set_sha256(
            &[
                chunk("a", "first")?,
                chunk("b", "first")?,
                chunk("b", "second")?,
                chunk("a", "second")?,
                chunk("a", "third")?,
            ],
            &[],
        );
        assert_eq!(
            first_duplicate,
            Err(SourceCoverageError::DuplicateCandidateId("b".into()))
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "source_coverage/sort_tests_v3.rs"]
mod sort_tests_v3;
