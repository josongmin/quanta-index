//! Decode retained V1 JSON pages with grants for their actual row dimensions.
//! The preflight owns no row backing. The second pass reserves each byte vector
//! exactly, so a page of decimal byte arrays does not need a 16x JSON grant.

use super::*;
use quanta_index_contract::index::{AuxiliaryGenerationKeyV1, HistorySnapshotRowV1};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::io::{self, BufReader, Read};

const MAX_ROWS: usize = quanta_index_contract::index::MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1;
const IO_CHECKPOINT_BYTES: usize = 64 * 1024;
const READER_BYTES: usize = 4096;
const MAX_JSON_STRING_TOKEN_BYTES: usize = 6
    * (quanta_index_contract::RepoId::MAX_UTF8_BYTES_V1
        + quanta_index_contract::RevisionId::MAX_UTF8_BYTES_V1)
    + 128;
// serde_json's reader grows a scratch byte vector while decoding a string.
// At growth, admit the previous and next capacities plus an owned token or
// diagnostic. This uses the validated token bound, independent of page size.
const PARSER_BACKING_BYTES: usize = READER_BYTES + 4 * MAX_JSON_STRING_TOKEN_BYTES;

fn check_string_tokens(payload: &[u8], budget: &mut WorkBudget) -> Result<(), CoreError> {
    let mut in_string = false;
    let mut escaped = false;
    let mut length = 0usize;
    for chunk in payload.chunks(IO_CHECKPOINT_BYTES) {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        budget
            .charge(WorkUsage {
                bytes: chunk.len() as u64,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(budget, error))?;
        for byte in chunk {
            if !in_string {
                if *byte == b'"' {
                    in_string = true;
                    length = 0;
                }
                continue;
            }
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
                continue;
            }
            length += 1;
            if length > MAX_JSON_STRING_TOKEN_BYTES {
                return Err(invalid(
                    "History page JSON string exceeds bounded token width",
                ));
            }
        }
    }
    Ok(())
}

struct WorkReader<'a> {
    input: &'a [u8],
    budget: &'a mut WorkBudget,
    allowance: usize,
    refusal: Option<IndexError>,
}

impl Read for WorkReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.allowance == 0 {
            if let Err(error) = self.budget.checkpoint() {
                self.refusal = Some(error);
                return Err(io::Error::other("History page JSON read refused"));
            }
            self.allowance = IO_CHECKPOINT_BYTES;
        }
        let count = output.len().min(self.input.len()).min(self.allowance);
        if let Err(error) = self.budget.charge(WorkUsage {
            bytes: count as u64,
            ..WorkUsage::default()
        }) {
            self.refusal = Some(error);
            return Err(io::Error::other("History page JSON work refused"));
        }
        output[..count].copy_from_slice(&self.input[..count]);
        self.input = &self.input[count..];
        self.allowance -= count;
        Ok(count)
    }
}

#[derive(Clone, Copy, Default)]
struct RowSize {
    key: usize,
    value: usize,
}

struct RowSizes {
    rows: [RowSize; MAX_ROWS],
    count: usize,
    byte_count: usize,
}

impl Default for RowSizes {
    fn default() -> Self {
        Self {
            rows: [RowSize::default(); MAX_ROWS],
            count: 0,
            byte_count: 0,
        }
    }
}

struct ByteCount(usize);

impl<'de> Deserialize<'de> for ByteCount {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl<'de> Visitor<'de> for Bytes {
            type Value = ByteCount;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a bounded History row byte array")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut count = 0usize;
                while sequence.next_element::<u8>()?.is_some() {
                    count = count
                        .checked_add(1)
                        .filter(|count| *count <= MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1)
                        .ok_or_else(|| {
                            de::Error::custom("History row byte array exceeds page bound")
                        })?;
                }
                Ok(ByteCount(count))
            }
        }
        deserializer.deserialize_seq(Bytes)
    }
}

// The page preflight accepts forward fields, unlike the closed row shapes.
// Resolve known field names without allocating a String for every JSON key.
struct OpenFieldSeed(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for OpenFieldSeed {
    type Value = Option<&'static str>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct Field(&'static [&'static str]);

        impl Visitor<'_> for Field {
            type Value = Option<&'static str>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a History page field")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(self.0.iter().copied().find(|field| *field == value))
            }

            fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(self
                    .0
                    .iter()
                    .copied()
                    .find(|field| field.as_bytes() == value))
            }
        }

        deserializer.deserialize_identifier(Field(self.0))
    }
}

struct RowPreflight {
    family: String,
    row_key: ByteCount,
    value: ByteCount,
}

impl<'de> Deserialize<'de> for RowPreflight {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["family", "row_key", "value"];
        struct Row;

        impl<'de> Visitor<'de> for Row {
            type Value = RowPreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct RowPreflight")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut family, mut row_key, mut value) = (None, None, None);
                while let Some(field) = map.next_key_seed(OpenFieldSeed(FIELDS))? {
                    match field {
                        Some("family") => {
                            if family.is_some() {
                                return Err(de::Error::duplicate_field("family"));
                            }
                            family = Some(map.next_value()?);
                        }
                        Some("row_key") => {
                            if row_key.is_some() {
                                return Err(de::Error::duplicate_field("row_key"));
                            }
                            row_key = Some(map.next_value()?);
                        }
                        Some("value") => {
                            if value.is_some() {
                                return Err(de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        None => {
                            let _: IgnoredAny = map.next_value()?;
                        }
                        _ => unreachable!("field seed returns only declared fields"),
                    }
                }
                Ok(RowPreflight {
                    family: family.ok_or_else(|| de::Error::missing_field("family"))?,
                    row_key: row_key.ok_or_else(|| de::Error::missing_field("row_key"))?,
                    value: value.ok_or_else(|| de::Error::missing_field("value"))?,
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let family = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(0, &"struct RowPreflight with 3 elements")
                })?;
                let row_key = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(1, &"struct RowPreflight with 3 elements")
                })?;
                let value = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(2, &"struct RowPreflight with 3 elements")
                })?;
                Ok(RowPreflight {
                    family,
                    row_key,
                    value,
                })
            }
        }

        deserializer.deserialize_struct("RowPreflight", FIELDS, Row)
    }
}

impl<'de> Deserialize<'de> for RowSizes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Rows;
        impl<'de> Visitor<'de> for Rows {
            type Value = RowSizes;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("at most 1024 History page rows")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut sizes = RowSizes::default();
                while let Some(row) = sequence.next_element::<RowPreflight>()? {
                    if !matches!(
                        row.family.as_str(),
                        "epoch" | "commit" | "ref" | "tag" | "diff-hunk" | "state-meta"
                    ) {
                        return Err(de::Error::custom("History page row family is invalid"));
                    }
                    let slot = sizes
                        .rows
                        .get_mut(sizes.count)
                        .ok_or_else(|| de::Error::custom("History page row count exceeds bound"))?;
                    *slot = RowSize {
                        key: row.row_key.0,
                        value: row.value.0,
                    };
                    sizes.count += 1;
                    sizes.byte_count = sizes
                        .byte_count
                        .checked_add(slot.key)
                        .and_then(|count| count.checked_add(slot.value))
                        .filter(|count| *count <= MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1)
                        .ok_or_else(|| {
                            de::Error::custom("History page decoded bytes exceed bound")
                        })?;
                }
                Ok(sizes)
            }
        }
        deserializer.deserialize_seq(Rows)
    }
}

struct PagePreflight {
    wire_version: u32,
    _generation: IgnoredAny,
    ordinal: u32,
    rows: RowSizes,
}

impl<'de> Deserialize<'de> for PagePreflight {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["wire_version", "generation", "ordinal", "rows"];
        struct Page;

        impl<'de> Visitor<'de> for Page {
            type Value = PagePreflight;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct PagePreflight")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut wire_version, mut generation, mut ordinal, mut rows) =
                    (None, None, None, None);
                while let Some(field) = map.next_key_seed(OpenFieldSeed(FIELDS))? {
                    match field {
                        Some("wire_version") => {
                            if wire_version.is_some() {
                                return Err(de::Error::duplicate_field("wire_version"));
                            }
                            wire_version = Some(map.next_value()?);
                        }
                        Some("generation") => {
                            if generation.is_some() {
                                return Err(de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value::<IgnoredAny>()?);
                        }
                        Some("ordinal") => {
                            if ordinal.is_some() {
                                return Err(de::Error::duplicate_field("ordinal"));
                            }
                            ordinal = Some(map.next_value()?);
                        }
                        Some("rows") => {
                            if rows.is_some() {
                                return Err(de::Error::duplicate_field("rows"));
                            }
                            rows = Some(map.next_value()?);
                        }
                        None => {
                            let _: IgnoredAny = map.next_value()?;
                        }
                        _ => unreachable!("field seed returns only declared fields"),
                    }
                }
                Ok(PagePreflight {
                    wire_version: wire_version
                        .ok_or_else(|| de::Error::missing_field("wire_version"))?,
                    _generation: generation
                        .ok_or_else(|| de::Error::missing_field("generation"))?,
                    ordinal: ordinal.ok_or_else(|| de::Error::missing_field("ordinal"))?,
                    rows: rows.ok_or_else(|| de::Error::missing_field("rows"))?,
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let wire_version = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(0, &"struct PagePreflight with 4 elements")
                })?;
                let generation = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(1, &"struct PagePreflight with 4 elements")
                })?;
                let ordinal = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(2, &"struct PagePreflight with 4 elements")
                })?;
                let rows = sequence.next_element()?.ok_or_else(|| {
                    de::Error::invalid_length(3, &"struct PagePreflight with 4 elements")
                })?;
                Ok(PagePreflight {
                    wire_version,
                    _generation: generation,
                    ordinal,
                    rows,
                })
            }
        }

        deserializer.deserialize_struct("PagePreflight", FIELDS, Page)
    }
}

struct ExactBytes(usize);

impl<'de> DeserializeSeed<'de> for ExactBytes {
    type Value = Vec<u8>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct Bytes(usize);
        impl<'de> Visitor<'de> for Bytes {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a History row byte array of the preflight length")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(self.0)
                    .map_err(|_| de::Error::custom("History row allocation refused"))?;
                if bytes.capacity() > self.0 {
                    return Err(de::Error::custom("History row allocation exceeded grant"));
                }
                for _ in 0..self.0 {
                    bytes.push(
                        sequence
                            .next_element::<u8>()?
                            .ok_or_else(|| de::Error::custom("History row byte count changed"))?,
                    );
                }
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("History row byte count changed"));
                }
                Ok(bytes)
            }
        }
        deserializer.deserialize_seq(Bytes(self.0))
    }
}

struct ExactRow(RowSize);

impl<'de> DeserializeSeed<'de> for ExactRow {
    type Value = HistorySnapshotRowV1;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct Row(RowSize);
        impl<'de> Visitor<'de> for Row {
            type Value = HistorySnapshotRowV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("one History snapshot row")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut family, mut row_key, mut value) = (None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "family" if family.is_none() => family = Some(map.next_value::<String>()?),
                        "row_key" if row_key.is_none() => {
                            row_key = Some(map.next_value_seed(ExactBytes(self.0.key))?)
                        }
                        "value" if value.is_none() => {
                            value = Some(map.next_value_seed(ExactBytes(self.0.value))?)
                        }
                        _ => return Err(de::Error::custom("History page row field differs")),
                    }
                }
                Ok(HistorySnapshotRowV1 {
                    family: family.ok_or_else(|| de::Error::missing_field("family"))?,
                    row_key: row_key.ok_or_else(|| de::Error::missing_field("row_key"))?,
                    value: value.ok_or_else(|| de::Error::missing_field("value"))?,
                })
            }
        }
        deserializer.deserialize_map(Row(self.0))
    }
}

struct ExactRows<'a>(&'a RowSizes);

impl<'de> DeserializeSeed<'de> for ExactRows<'_> {
    type Value = Vec<HistorySnapshotRowV1>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct Rows<'a>(&'a RowSizes);
        impl<'de> Visitor<'de> for Rows<'_> {
            type Value = Vec<HistorySnapshotRowV1>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("the preflighted History page rows")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut rows = Vec::new();
                rows.try_reserve_exact(self.0.count)
                    .map_err(|_| de::Error::custom("History page rows allocation refused"))?;
                if rows.capacity() > self.0.count {
                    return Err(de::Error::custom(
                        "History page rows allocation exceeded grant",
                    ));
                }
                for size in self.0.rows.iter().take(self.0.count) {
                    rows.push(
                        sequence
                            .next_element_seed(ExactRow(*size))?
                            .ok_or_else(|| de::Error::custom("History page row count changed"))?,
                    );
                }
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("History page row count changed"));
                }
                Ok(rows)
            }
        }
        deserializer.deserialize_seq(Rows(self.0))
    }
}

struct ExactPage<'a>(&'a RowSizes);

impl<'de> DeserializeSeed<'de> for ExactPage<'_> {
    type Value = HistorySnapshotPageV1;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct Page<'a>(&'a RowSizes);
        impl<'de> Visitor<'de> for Page<'_> {
            type Value = HistorySnapshotPageV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("one V1 History snapshot page")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut wire_version, mut generation, mut ordinal, mut rows) =
                    (None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "wire_version" if wire_version.is_none() => {
                            wire_version = Some(map.next_value::<u32>()?)
                        }
                        "generation" if generation.is_none() => {
                            generation = Some(map.next_value::<AuxiliaryGenerationKeyV1>()?)
                        }
                        "ordinal" if ordinal.is_none() => ordinal = Some(map.next_value::<u32>()?),
                        "rows" if rows.is_none() => {
                            rows = Some(map.next_value_seed(ExactRows(self.0))?)
                        }
                        _ => return Err(de::Error::custom("History page field differs")),
                    }
                }
                Ok(HistorySnapshotPageV1 {
                    wire_version: wire_version
                        .ok_or_else(|| de::Error::missing_field("wire_version"))?,
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    ordinal: ordinal.ok_or_else(|| de::Error::missing_field("ordinal"))?,
                    rows: rows.ok_or_else(|| de::Error::missing_field("rows"))?,
                })
            }
        }
        deserializer.deserialize_map(Page(self.0))
    }
}

pub(super) fn decode_v1(
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
) -> Result<(HistorySnapshotPageV1, Option<MemoryLease>), CoreError> {
    let registered_bytes = stored
        .payload
        .len()
        .checked_add(quanta_index_contract::index::NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1)
        .and_then(|bytes| u64::try_from(bytes).ok());
    if stored.registration.reference != reference
        || !stored.registration.children.is_empty()
        || stored.payload.len() > MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1
        || registered_bytes != Some(stored.registration.bytes)
    {
        return Err(invalid(
            "History snapshot page identity, child set or length is invalid",
        ));
    }
    check_string_tokens(&stored.payload, budget)?;
    let _scratch = budget
        .reserve_memory(PARSER_BACKING_BYTES as u64, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    let mut reader = BufReader::with_capacity(
        READER_BYTES,
        WorkReader {
            input: &stored.payload,
            budget,
            allowance: 0,
            refusal: None,
        },
    );
    let decoded = serde_json::from_reader::<_, PagePreflight>(&mut reader);
    let refusal = reader.get_mut().refusal.take();
    drop(reader);
    if let Some(error) = refusal {
        return Err(history_work_error(budget, error));
    }
    let preflight = decoded.map_err(|error| invalid(&error.to_string()))?;
    if preflight.wire_version != 1 || preflight.ordinal == 0 || preflight.rows.count == 0 {
        return Err(invalid("History snapshot page preflight shape is invalid"));
    }
    let bytes = preflight
        .rows
        .byte_count
        .checked_add(
            preflight
                .rows
                .count
                .checked_mul(std::mem::size_of::<HistorySnapshotRowV1>() + 64)
                .ok_or_else(|| invalid("History snapshot row metadata bound overflow"))?,
        )
        .and_then(|count| {
            count.checked_add(
                quanta_index_contract::RepoId::MAX_UTF8_BYTES_V1
                    + quanta_index_contract::RevisionId::MAX_UTF8_BYTES_V1
                    + 4096,
            )
        })
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| invalid("History snapshot decoded backing bound overflow"))?;
    let memory = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;
    let mut reader = BufReader::with_capacity(
        READER_BYTES,
        WorkReader {
            input: &stored.payload,
            budget,
            allowance: 0,
            refusal: None,
        },
    );
    let mut decoder = serde_json::Deserializer::from_reader(&mut reader);
    let page = ExactPage(&preflight.rows).deserialize(&mut decoder);
    let ended = decoder.end();
    drop(decoder);
    let refusal = reader.get_mut().refusal.take();
    drop(reader);
    if let Some(error) = refusal {
        return Err(history_work_error(budget, error));
    }
    let page = page.map_err(|error| invalid(&error.to_string()))?;
    ended.map_err(|error| invalid(&error.to_string()))?;
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    Ok((page, memory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        DiffHunkSide, ManifestGeneration, RepoId, RevisionId, lex::DiffHunkRecord,
    };
    use quanta_index_core::index::{
        MemoryLeaseOwner, MemoryScope, MemoryScopePort, RegisteredObject, WorkLimits,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    struct Ledger {
        live: AtomicU64,
        maximum: u64,
    }

    struct Scope(Arc<Ledger>);

    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let previous = self.live.fetch_sub(bytes, Ordering::AcqRel);
            assert!(previous >= bytes, "History decode ledger underflow");
        }
    }

    impl MemoryScopePort for Scope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.maximum)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn budget(ledger: &Arc<Ledger>) -> WorkBudget {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                mutations: 1_000_000,
                bytes: 1_000_000_000,
            },
            Instant::now() + Duration::from_secs(60),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        budget
    }

    #[test]
    fn preflight_preserves_serde_map_and_positional_shapes() {
        let mapped: PagePreflight = serde_json::from_str(
            r#"{"wire_version":1,"generation":null,"ordinal":1,"rows":[{"family":"epoch","row_key":[1],"value":[2],"future":{"nested":true}}],"future":null}"#,
        )
        .expect("unknown map fields and nullable ignored generation");
        assert_eq!(mapped.rows.count, 1);
        assert_eq!(mapped.rows.byte_count, 2);

        let positional: PagePreflight = serde_json::from_str(r#"[1,null,1,[["epoch",[1],[2]]]]"#)
            .expect("page and row positional forms");
        assert_eq!(positional.rows.count, mapped.rows.count);
        assert_eq!(positional.rows.byte_count, mapped.rows.byte_count);

        assert!(
            serde_json::from_str::<PagePreflight>(
                r#"{"wire_version":1,"generation":null,"generation":null,"ordinal":1,"rows":[]}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<RowPreflight>(r#"["epoch",[1],[2],null]"#).is_err());
        assert!(serde_json::from_str::<RowPreflight>(r#"{"family":"epoch","value":[]}"#).is_err());
    }

    #[test]
    fn v1_large_numeric_page_decodes_under_original_64_mib_and_refunds() {
        let record = DiffHunkRecord {
            wire_version: 1,
            hunk_header: "@@".into(),
            side: DiffHunkSide::After,
            added_text: "x".repeat(3 * 1024 * 1024).into_boxed_str(),
            removed_text: "".into(),
            touched_text: "".into(),
            byte_start: 0,
            byte_end: 3 * 1024 * 1024,
        };
        let value = quanta_index_ipc::encode_cbor_payload(&record).expect("producer History row");
        let page = HistorySnapshotPageV1 {
            wire_version: 1,
            generation: AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("history-large").expect("repo"),
                revision_id: RevisionId::new("main").expect("revision"),
                generation: ManifestGeneration::new(1),
            },
            ordinal: 1,
            rows: vec![HistorySnapshotRowV1 {
                family: "diff-hunk".into(),
                row_key: vec![1; 20],
                value,
            }],
        };
        let payload = serde_json::to_vec(&page).expect("page JSON");
        assert!(payload.len() > 8 * 1024 * 1024);
        let reference = quanta_index_contract::index::native_immutable_object_ref_v1(&payload, &[])
            .expect("page reference");
        let stored = StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                (quanta_index_contract::index::NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1
                    + payload.len()) as u64,
                Vec::new(),
            ),
            payload,
            None,
        );

        let ledger = Arc::new(Ledger {
            live: AtomicU64::new(0),
            maximum: 64 * 1024 * 1024,
        });
        let mut work = budget(&ledger);
        let malformed = StoredObject::new(
            RegisteredObject::unscoped(reference, stored.payload.len() as u64, Vec::new()),
            stored.payload.clone(),
            None,
        );
        assert!(decode_v1(reference, &malformed, &mut work).is_err());
        drop(malformed);
        let raw = work
            .reserve_memory(stored.payload.len() as u64, MemoryLifetime::Temporary)
            .expect("original raw page loan");
        let (decoded, decoded_loan) =
            decode_v1(reference, &stored, &mut work).expect("funded large V1 page decode");
        assert_eq!(decoded, page);
        drop(decoded);
        drop(decoded_loan);
        drop(raw);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);

        let low = Arc::new(Ledger {
            live: AtomicU64::new(0),
            maximum: stored.payload.len() as u64,
        });
        let mut work = budget(&low);
        let raw = work
            .reserve_memory(stored.payload.len() as u64, MemoryLifetime::Temporary)
            .expect("low-budget raw page loan");
        assert!(decode_v1(reference, &stored, &mut work).is_err());
        drop(raw);
        assert_eq!(low.live.load(Ordering::Acquire), 0);
    }
}
