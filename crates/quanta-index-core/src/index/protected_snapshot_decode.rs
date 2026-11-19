//! Borrow physical snapshot pages while admitting each decoded JSON allocation.

use core::{fmt, marker::PhantomData};
use std::cell::RefCell;
use std::io::{self, BufRead, Read};

use serde::de::{
    self, Deserialize, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor,
};

use super::super::{
    CatalogView, Digest, IndexError, MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1,
    MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1, MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1, MemoryLease,
    MemoryLifetime, NativeProtectedFrontierV1, NativeProtectedNativeSnapshotV1,
    NativeProtectedSourceAuthorityV1, ObjectRef, StoredObject, WorkBudget,
};

const MAX_ROW_BYTES: usize = MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1 / 2;
const MAX_VECTOR_LOANS: usize = MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1 + 6;
const MAX_JSON_STRING_BYTES: usize = 64;
const IO_CHECKPOINT_BYTES: usize = 65_536;

pub(super) trait SnapshotDecodeWork {
    fn checkpoint(&self) -> Result<(), IndexError>;
    fn reserve_memory(
        &self,
        bytes: u64,
        lifetime: MemoryLifetime,
    ) -> Result<Option<MemoryLease>, IndexError>;
}

impl SnapshotDecodeWork for WorkBudget {
    fn checkpoint(&self) -> Result<(), IndexError> {
        WorkBudget::checkpoint(self)
    }

    fn reserve_memory(
        &self,
        bytes: u64,
        lifetime: MemoryLifetime,
    ) -> Result<Option<MemoryLease>, IndexError> {
        WorkBudget::reserve_memory(self, bytes, lifetime)
    }
}

impl SnapshotDecodeWork for RefCell<&mut WorkBudget> {
    fn checkpoint(&self) -> Result<(), IndexError> {
        let budget = self.borrow();
        budget.checkpoint()
    }

    fn reserve_memory(
        &self,
        bytes: u64,
        lifetime: MemoryLifetime,
    ) -> Result<Option<MemoryLease>, IndexError> {
        let budget = self.borrow();
        budget.reserve_memory(bytes, lifetime)
    }
}

pub(super) struct FundedSnapshot {
    pub(super) data: NativeProtectedNativeSnapshotV1,
    _backing: SnapshotBacking,
}

struct SnapshotBacking {
    vectors: Vec<MemoryLease>,
    _ledger: MemoryLease,
}

struct DecodeBudget<'a> {
    work: &'a dyn SnapshotDecodeWork,
    backing: SnapshotBacking,
    first_refusal: Option<IndexError>,
}

impl DecodeBudget<'_> {
    fn push<T>(
        &mut self,
        values: &mut Vec<T>,
        loan_index: &mut Option<usize>,
        maximum: usize,
        value: T,
    ) -> Result<(), IndexError> {
        if values.len() == values.capacity() {
            let capacity = values.capacity().saturating_mul(2).max(4).min(maximum);
            let additional = capacity
                .checked_sub(values.len())
                .filter(|count| *count != 0)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = capacity
                .checked_mul(core::mem::size_of::<T>())
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or(IndexError::ResourceLimited)?;
            if loan_index.is_none() && self.backing.vectors.len() == MAX_VECTOR_LOANS {
                return Err(IndexError::ResourceLimited);
            }
            // The old allocation and new allocation can overlap during growth.
            let grant = required_loan(self.work, bytes)?;
            values
                .try_reserve_exact(additional)
                .map_err(|_| IndexError::ResourceLimited)?;
            if let Some(index) = *loan_index {
                let previous = self
                    .backing
                    .vectors
                    .get_mut(index)
                    .ok_or(IndexError::DigestConflict)?;
                *previous = grant;
            } else {
                *loan_index = Some(self.backing.vectors.len());
                self.backing.vectors.push(grant);
            }
            if values.capacity() > capacity {
                return Err(IndexError::ResourceLimited);
            }
        }
        values.push(value);
        Ok(())
    }
}

fn required_loan(work: &dyn SnapshotDecodeWork, bytes: u64) -> Result<MemoryLease, IndexError> {
    work.reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)
}

struct ExtraElement;

impl<'de> DeserializeSeed<'de> for ExtraElement {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
        Err(de::Error::custom(
            "protected snapshot sequence exceeds its limit",
        ))
    }
}

fn read_sequence<'de, 'budget, A, T>(
    mut seq: A,
    budget: &mut DecodeBudget<'budget>,
    maximum: usize,
    mut next: impl FnMut(&mut A, &mut DecodeBudget<'budget>) -> Result<Option<T>, A::Error>,
) -> Result<Vec<T>, A::Error>
where
    A: SeqAccess<'de>,
{
    let mut values = Vec::new();
    let mut loan = None;
    loop {
        if values.len() == maximum {
            let _: Option<()> = seq.next_element_seed(ExtraElement)?;
            return Ok(values);
        }
        let Some(value) = next(&mut seq, budget)? else {
            return Ok(values);
        };
        if let Err(cause) = budget.push(&mut values, &mut loan, maximum, value) {
            budget.first_refusal = Some(cause);
            return Err(de::Error::custom("protected snapshot backing refused"));
        }
    }
}

struct ListSeed<'a, 'budget, T> {
    budget: &'a mut DecodeBudget<'budget>,
    maximum: usize,
    element: PhantomData<fn() -> T>,
}

impl<'a, 'budget, T> ListSeed<'a, 'budget, T> {
    fn new(budget: &'a mut DecodeBudget<'budget>, maximum: usize) -> Self {
        Self {
            budget,
            maximum,
            element: PhantomData,
        }
    }
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for ListSeed<'_, '_, T> {
    type Value = Vec<T>;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Vec<T>, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, T: Deserialize<'de>> Visitor<'de> for ListSeed<'_, '_, T> {
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded protected snapshot sequence")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Vec<T>, A::Error> {
        read_sequence(seq, self.budget, self.maximum, |seq, _| seq.next_element())
    }
}

struct RowsSeed<'a, 'budget> {
    budget: &'a mut DecodeBudget<'budget>,
    maximum: usize,
}

impl<'de> DeserializeSeed<'de> for RowsSeed<'_, '_> {
    type Value = Vec<Vec<u8>>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for RowsSeed<'_, '_> {
    type Value = Vec<Vec<u8>>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded protected snapshot row bytes")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        read_sequence(seq, self.budget, self.maximum, |seq, budget| {
            seq.next_element_seed(ListSeed::new(budget, MAX_ROW_BYTES))
        })
    }
}

enum Field {
    WireVersion,
    SourceRootNonce,
    SourceRoutes,
    OperationRows,
    GenesisViews,
    Frontiers,
    SourceAuthorities,
    EventClaims,
    Roots,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct FieldVisitor;

        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a protected Native snapshot field")
            }

            fn visit_str<E: de::Error>(self, field: &str) -> Result<Field, E> {
                match field {
                    "wire_version" => Ok(Field::WireVersion),
                    "source_root_nonce" => Ok(Field::SourceRootNonce),
                    "source_routes" => Ok(Field::SourceRoutes),
                    "operation_rows" => Ok(Field::OperationRows),
                    "genesis_views" => Ok(Field::GenesisViews),
                    "frontiers" => Ok(Field::Frontiers),
                    "source_authorities" => Ok(Field::SourceAuthorities),
                    "event_claims" => Ok(Field::EventClaims),
                    "roots" => Ok(Field::Roots),
                    other => Err(E::unknown_field(
                        other,
                        &[
                            "wire_version",
                            "source_root_nonce",
                            "source_routes",
                            "operation_rows",
                            "genesis_views",
                            "frontiers",
                            "source_authorities",
                            "event_claims",
                            "roots",
                        ],
                    )),
                }
            }
        }

        decoder.deserialize_identifier(FieldVisitor)
    }
}

struct SnapshotSeed<'a, 'budget>(&'a mut DecodeBudget<'budget>);

impl<'de> DeserializeSeed<'de> for SnapshotSeed<'_, '_> {
    type Value = NativeProtectedNativeSnapshotV1;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for SnapshotSeed<'_, '_> {
    type Value = NativeProtectedNativeSnapshotV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a protected Native snapshot")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut wire_version = None;
        let mut source_root_nonce: Option<Digest> = None;
        let mut source_routes: Option<ObjectRef> = None;
        let mut operation_rows = None;
        let mut genesis_views: Option<Vec<CatalogView>> = None;
        let mut frontiers: Option<Vec<NativeProtectedFrontierV1>> = None;
        let mut source_authorities: Option<Vec<NativeProtectedSourceAuthorityV1>> = None;
        let mut event_claims = None;
        let mut roots: Option<Vec<ObjectRef>> = None;
        let mut remaining = MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1;
        while let Some(field) = map.next_key()? {
            match field {
                Field::WireVersion => {
                    set_field(&mut wire_version, "wire_version", || map.next_value())?
                }
                Field::SourceRootNonce => {
                    set_field(&mut source_root_nonce, "source_root_nonce", || {
                        map.next_value()
                    })?
                }
                Field::SourceRoutes => {
                    set_field(&mut source_routes, "source_routes", || map.next_value())?
                }
                Field::OperationRows => set_rows(
                    &mut operation_rows,
                    "operation_rows",
                    &mut remaining,
                    |maximum| {
                        map.next_value_seed(RowsSeed {
                            budget: &mut *self.0,
                            maximum,
                        })
                    },
                )?,
                Field::GenesisViews => set_rows(
                    &mut genesis_views,
                    "genesis_views",
                    &mut remaining,
                    |maximum| {
                        map.next_value_seed(ListSeed::new(
                            &mut *self.0,
                            maximum.min(MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1),
                        ))
                    },
                )?,
                Field::Frontiers => {
                    set_rows(&mut frontiers, "frontiers", &mut remaining, |maximum| {
                        map.next_value_seed(ListSeed::new(&mut *self.0, maximum))
                    })?
                }
                Field::SourceAuthorities => set_rows(
                    &mut source_authorities,
                    "source_authorities",
                    &mut remaining,
                    |maximum| map.next_value_seed(ListSeed::new(&mut *self.0, maximum)),
                )?,
                Field::EventClaims => set_rows(
                    &mut event_claims,
                    "event_claims",
                    &mut remaining,
                    |maximum| {
                        map.next_value_seed(RowsSeed {
                            budget: &mut *self.0,
                            maximum,
                        })
                    },
                )?,
                Field::Roots => set_field(&mut roots, "roots", || {
                    map.next_value_seed(ListSeed::new(
                        &mut *self.0,
                        super::super::MAX_RECLAIM_CHILDREN,
                    ))
                })?,
            }
        }
        Ok(NativeProtectedNativeSnapshotV1 {
            wire_version: require(wire_version, "wire_version")?,
            source_root_nonce: require(source_root_nonce, "source_root_nonce")?,
            source_routes: require(source_routes, "source_routes")?,
            operation_rows: require(operation_rows, "operation_rows")?,
            genesis_views: require(genesis_views, "genesis_views")?,
            frontiers: require(frontiers, "frontiers")?,
            source_authorities: require(source_authorities, "source_authorities")?,
            event_claims: require(event_claims, "event_claims")?,
            roots: require(roots, "roots")?,
        })
    }
}

pub(super) fn set_field<T, E: de::Error>(
    slot: &mut Option<T>,
    name: &'static str,
    read: impl FnOnce() -> Result<T, E>,
) -> Result<(), E> {
    if slot.is_some() {
        return Err(E::duplicate_field(name));
    }
    *slot = Some(read()?);
    Ok(())
}

fn set_rows<T, E: de::Error>(
    slot: &mut Option<Vec<T>>,
    name: &'static str,
    remaining: &mut usize,
    read: impl FnOnce(usize) -> Result<Vec<T>, E>,
) -> Result<(), E> {
    set_field(slot, name, || {
        let values = read(*remaining)?;
        *remaining = remaining
            .checked_sub(values.len())
            .ok_or_else(|| E::custom("protected snapshot aggregate exceeds its row limit"))?;
        Ok(values)
    })
}

pub(super) fn require<T, E: de::Error>(value: Option<T>, name: &'static str) -> Result<T, E> {
    value.ok_or_else(|| E::missing_field(name))
}

// This wire has no variable text fields. Limit raw string tokens before
// serde_json's reader can grow scratch, including strings in malformed input.
#[derive(Default)]
struct ParserTokens {
    in_string: bool,
    width: usize,
    number_width: usize,
}

impl ParserTokens {
    fn check_chunk(&mut self, chunk: &[u8]) -> Result<(), IndexError> {
        for &byte in chunk {
            if byte == b'"' {
                self.in_string = !self.in_string;
                self.width = 0;
                self.number_width = 0;
            } else if self.in_string {
                self.width = self.width.saturating_add(1);
                if byte == b'\\' || self.width > MAX_JSON_STRING_BYTES {
                    return Err(IndexError::DigestConflict);
                }
            } else if matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                self.number_width = self.number_width.saturating_add(1);
                if self.number_width > 64 {
                    return Err(IndexError::DigestConflict);
                }
            } else {
                self.number_width = 0;
            }
        }
        Ok(())
    }
}

fn check_parser_tokens(
    first: &[u8],
    second: &[u8],
    budget: &dyn SnapshotDecodeWork,
) -> Result<(), IndexError> {
    let mut tokens = ParserTokens::default();
    for chunk in first
        .chunks(IO_CHECKPOINT_BYTES)
        .chain(second.chunks(IO_CHECKPOINT_BYTES))
    {
        budget.checkpoint()?;
        tokens.check_chunk(chunk)?;
    }
    Ok(())
}

/// Fill one bounded buffer and inspect all its bytes before serde receives any
/// of them. A partial parser read reuses the already-inspected remainder.
struct TokenCheckedReader<'a, R> {
    input: io::BufReader<R>,
    work: &'a dyn SnapshotDecodeWork,
    tokens: ParserTokens,
    inspected_remaining: usize,
}

impl<R: Read> Read for TokenCheckedReader<'_, R> {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        if destination.is_empty() {
            return Ok(0);
        }
        if self.inspected_remaining == 0 {
            self.work.checkpoint().map_err(io::Error::other)?;
        }
        let available = self.input.fill_buf()?;
        if available.is_empty() {
            return Ok(0);
        }
        if self.inspected_remaining == 0 {
            self.tokens
                .check_chunk(available)
                .map_err(io::Error::other)?;
            self.inspected_remaining = available.len();
        }
        let count = destination.len().min(self.inspected_remaining);
        destination[..count].copy_from_slice(&available[..count]);
        self.input.consume(count);
        self.inspected_remaining -= count;
        Ok(count)
    }
}

struct JsonCheckpointReader<'a, R> {
    input: R,
    work: &'a dyn SnapshotDecodeWork,
    allowance: usize,
    refusal: Option<IndexError>,
}

impl<'a, R> JsonCheckpointReader<'a, R> {
    fn new(input: R, work: &'a dyn SnapshotDecodeWork) -> Self {
        Self {
            input,
            work,
            allowance: 0,
            refusal: None,
        }
    }
}

impl<R: Read> Read for JsonCheckpointReader<'_, R> {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        if self.allowance == 0 {
            if let Err(cause) = self.work.checkpoint() {
                self.refusal = Some(cause);
                return Err(io::Error::other("protected JSON read refused"));
            }
            self.allowance = IO_CHECKPOINT_BYTES;
        }
        let width = destination.len().min(self.allowance);
        let (chunk, _) = destination.split_at_mut(width);
        let count = self.input.read(chunk).inspect_err(|error| {
            self.refusal = error
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<IndexError>())
                .cloned();
        })?;
        self.allowance = self.allowance.saturating_sub(count);
        Ok(count)
    }
}

/// Parse a borrowed JSON body after the caller has bounded and admitted its
/// parser scratch. Small bodies retain serde_json's optimized slice reader.
pub(super) fn deserialize_borrowed_json_v1<T: DeserializeOwned>(
    payload: &[u8],
    work: &dyn SnapshotDecodeWork,
    context: &'static str,
) -> Result<T, IndexError> {
    let invalid = |error| IndexError::CorruptObject(format!("{context}: {error}"));
    if payload.len() <= IO_CHECKPOINT_BYTES {
        work.checkpoint()?;
        return serde_json::from_slice(payload).map_err(invalid);
    }
    let mut reader = JsonCheckpointReader::new(payload, work);
    serde_json::from_reader(&mut reader)
        .map_err(|error| reader.refusal.unwrap_or_else(|| invalid(error)))
}

pub(super) fn decode(
    first: &[u8],
    second: &[u8],
    work: &WorkBudget,
) -> Result<FundedSnapshot, IndexError> {
    check_parser_tokens(first, second, work)?;
    decode_reader(first.chain(second), work)
}

pub(super) fn decode_owned(
    first: StoredObject,
    second: Option<StoredObject>,
    work: &WorkBudget,
) -> Result<FundedSnapshot, IndexError> {
    check_parser_tokens(
        &first.payload,
        second.as_ref().map_or(&[], |page| page.payload.as_slice()),
        work,
    )?;
    decode_reader(
        OwnedSnapshotPages {
            current: Some(first),
            next: second,
            offset: 0,
        },
        work,
    )
}

/// Decode provisional bytes from an opened physical source. The caller must
/// still authenticate the completed page streams and ordered child closure.
pub(super) fn decode_streamed(
    input: impl Read,
    work: &dyn SnapshotDecodeWork,
) -> Result<FundedSnapshot, IndexError> {
    let buffer_bytes =
        u64::try_from(IO_CHECKPOINT_BYTES).map_err(|_| IndexError::ResourceLimited)?;
    let _buffer = required_loan(work, buffer_bytes)?;
    decode_reader(
        TokenCheckedReader {
            input: io::BufReader::with_capacity(IO_CHECKPOINT_BYTES, input),
            work,
            tokens: ParserTokens::default(),
            inspected_remaining: 0,
        },
        work,
    )
}

struct OwnedSnapshotPages {
    current: Option<StoredObject>,
    next: Option<StoredObject>,
    offset: usize,
}

impl Read for OwnedSnapshotPages {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let Some(page) = self.current.as_ref() else {
            return Ok(0);
        };
        let remaining = &page.payload[self.offset..];
        let width = remaining.len().min(output.len());
        output[..width].copy_from_slice(&remaining[..width]);
        self.offset += width;
        if self.offset == page.payload.len() {
            self.current = self.next.take();
            self.offset = 0;
        }
        Ok(width)
    }
}

fn decode_reader(
    input: impl Read,
    work: &dyn SnapshotDecodeWork,
) -> Result<FundedSnapshot, IndexError> {
    let ledger_bytes = u64::try_from(MAX_VECTOR_LOANS * core::mem::size_of::<MemoryLease>())
        .map_err(|_| IndexError::ResourceLimited)?;
    // serde_json only sees bounded unescaped ASCII schema names. Include its
    // finite reader scratch in the same loan as the fixed grant ledger.
    let ledger = required_loan(
        work,
        ledger_bytes
            .checked_add(4096)
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    let mut vectors = Vec::new();
    vectors
        .try_reserve_exact(MAX_VECTOR_LOANS)
        .map_err(|_| IndexError::ResourceLimited)?;
    if vectors.capacity() > MAX_VECTOR_LOANS {
        return Err(IndexError::ResourceLimited);
    }
    let mut budget = DecodeBudget {
        work,
        backing: SnapshotBacking {
            vectors,
            _ledger: ledger,
        },
        first_refusal: None,
    };
    let mut input = JsonCheckpointReader::new(input, work);
    let decoded = {
        let mut json = serde_json::Deserializer::from_reader(&mut input);
        SnapshotSeed(&mut budget)
            .deserialize(&mut json)
            .and_then(|data| {
                json.end()?;
                Ok(data)
            })
    };
    let data = decoded.map_err(|cause| {
        budget
            .first_refusal
            .take()
            .or(input.refusal)
            .unwrap_or_else(|| {
                IndexError::CorruptObject(format!("protected Native snapshot: {cause}"))
            })
    })?;
    Ok(FundedSnapshot {
        data,
        _backing: budget.backing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::WorkLimits;
    use std::time::{Duration, Instant};

    struct CancelOnSecondRead(std::cell::Cell<u8>);

    impl SnapshotDecodeWork for CancelOnSecondRead {
        fn checkpoint(&self) -> Result<(), IndexError> {
            let count = self.0.get() + 1;
            self.0.set(count);
            if count == 2 {
                return Err(IndexError::Cancelled);
            }
            Ok(())
        }

        fn reserve_memory(
            &self,
            _bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<Option<MemoryLease>, IndexError> {
            panic!("borrowed JSON reader must use the caller's parser loan")
        }
    }

    #[test]
    fn borrowed_route_head_count_preserves_refusal_inside_read() {
        use super::super::RouteHeadCounts;

        // An invalid head body can be one large ignored value in the count
        // pass. Keep the wire's depth and token limits satisfied.
        let mut bytes = b"{\"wire_version\":1,\"heads\":[[".to_vec();
        while bytes.len() < IO_CHECKPOINT_BYTES {
            bytes.extend_from_slice(b"0,");
        }
        bytes.extend_from_slice(b"0]],\"repomap_heads\":[]}");
        assert!(serde_json::from_slice::<RouteHeadCounts>(&bytes).is_ok());
        let work = CancelOnSecondRead(std::cell::Cell::new(0));
        assert!(matches!(
            deserialize_borrowed_json_v1::<RouteHeadCounts>(
                &bytes,
                &work,
                "protected Source routes"
            ),
            Err(IndexError::Cancelled)
        ));
        assert_eq!(work.0.get(), 2);
    }

    #[test]
    fn streamed_token_refusal_precedes_delivery_of_next_buffer() {
        let mut bytes = vec![b' '; IO_CHECKPOINT_BYTES - 5];
        bytes.extend_from_slice(b"\"abcd");
        bytes.extend(std::iter::repeat(b'a').take(MAX_JSON_STRING_BYTES + 1));
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        );
        let work = RefCell::new(&mut budget);
        let mut reader = JsonCheckpointReader::new(
            TokenCheckedReader {
                input: io::BufReader::with_capacity(IO_CHECKPOINT_BYTES, bytes.as_slice()),
                work: &work,
                tokens: ParserTokens::default(),
                inspected_remaining: 0,
            },
            &work,
        );
        let mut first = vec![0; IO_CHECKPOINT_BYTES];
        assert_eq!(
            reader.read(&mut first).expect("first inspected buffer"),
            first.len()
        );
        // A sequential page opener may reacquire the same budget here.
        {
            let reopened = work.borrow_mut();
            reopened
                .checkpoint()
                .expect("work borrow released after read");
        }
        let mut next = [0; 1];
        let refusal = reader
            .read(&mut next)
            .expect_err("overlong token must refuse");
        assert_eq!(
            refusal
                .get_ref()
                .and_then(|source| source.downcast_ref::<IndexError>()),
            Some(&IndexError::DigestConflict)
        );
        assert_eq!(next, [0]);
        assert_eq!(reader.refusal, Some(IndexError::DigestConflict));
    }
}
