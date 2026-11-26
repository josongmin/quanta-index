//! Shape-derived grants for History rows with large strings and collections.

use super::*;
use serde::de::{DeserializeOwned, SeqAccess};

mod json_shape;
pub(super) mod shape_codec;

use shape_codec::{ShapeFieldSeed, ShapeVariantSeed, closed_shape, newtype_shape};

pub(super) const SOURCE_JSON_IO_BUFFER_BYTES: usize = 4096;

fn add(left: u64, right: u64) -> Result<u64, CoreError> {
    left.checked_add(right)
        .ok_or_else(|| invalid("History row decoded backing bound overflow"))
}

fn mul(left: u64, right: u64) -> Result<u64, CoreError> {
    left.checked_mul(right)
        .ok_or_else(|| invalid("History row decoded backing bound overflow"))
}

/// Bound the completed row's backing separately from decoder growth and the
/// boxed conversions that can overlap it before the row enters the state.
pub(super) struct RowBacking {
    pub(super) retained: u64,
    pub(super) peak: u64,
}

impl RowBacking {
    pub(super) fn new(retained: u64, peak: u64) -> Result<Self, CoreError> {
        if retained > peak {
            return Err(invalid("History row decoded backing bound overflow"));
        }
        Ok(Self { retained, peak })
    }

    pub(super) fn reserve_decode(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<(Option<MemoryLease>, Option<MemoryLease>), CoreError> {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        let retained = if self.retained > 0 {
            budget
                .reserve_memory(self.retained, MemoryLifetime::Retained)
                .map_err(|error| history_work_error(budget, error))?
        } else {
            None
        };
        let scratch = if self.peak > self.retained {
            budget
                .reserve_memory(self.peak - self.retained, MemoryLifetime::Temporary)
                .map_err(|error| history_work_error(budget, error))?
        } else {
            None
        };
        Ok((retained, scratch))
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct TextBytes(pub(super) usize);

impl<'de> Deserialize<'de> for TextBytes {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Text;
        impl Visitor<'_> for Text {
            type Value = TextBytes;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a History text field")
            }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<TextBytes, E> {
                Ok(TextBytes(text.len()))
            }
            fn visit_string<E: de::Error>(self, text: String) -> Result<TextBytes, E> {
                Ok(TextBytes(text.len()))
            }
        }
        decoder.deserialize_string(Text)
    }
}

#[derive(Default)]
struct TextItems {
    count: usize,
    bytes: usize,
}

impl<'de> Deserialize<'de> for TextItems {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Items;
        impl<'de> Visitor<'de> for Items {
            type Value = TextItems;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a History text sequence")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<TextItems, A::Error> {
                let mut out = TextItems::default();
                while let Some(item) = seq.next_element::<TextBytes>()? {
                    out.count = out
                        .count
                        .checked_add(1)
                        .ok_or_else(|| de::Error::custom("History text sequence overflow"))?;
                    out.bytes = out
                        .bytes
                        .checked_add(item.0)
                        .ok_or_else(|| de::Error::custom("History text sequence overflow"))?;
                }
                Ok(out)
            }
        }
        decoder.deserialize_seq(Items)
    }
}

#[derive(Default)]
struct ByteItems(usize);

impl<'de> Deserialize<'de> for ByteItems {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Items;
        impl<'de> Visitor<'de> for Items {
            type Value = ByteItems;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a History byte sequence")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ByteItems, A::Error> {
                let mut count = 0usize;
                while seq.next_element::<u8>()?.is_some() {
                    count = count
                        .checked_add(1)
                        .ok_or_else(|| de::Error::custom("History byte sequence overflow"))?;
                }
                Ok(ByteItems(count))
            }
        }
        decoder.deserialize_seq(Items)
    }
}

closed_shape!(
    pub(super) struct CommitShape {
        _wire_version as wire_version: u32,
        sha: TextBytes,
        parents: TextItems,
        _author_time_ms as author_time_ms: u64,
        _committer_time_ms as committer_time_ms: u64,
        _applied_at_ms as applied_at_ms: u64,
        author: TextBytes,
        author_name: Option<TextBytes> = default,
        author_email: Option<TextBytes> = default,
        committer: TextBytes,
        committer_name: Option<TextBytes> = default,
        committer_email: Option<TextBytes> = default,
        message: TextBytes,
        _is_merge as is_merge: bool,
        tags: TextItems,
    }
);

impl CommitShape {
    fn text_bytes(&self) -> Result<u64, CoreError> {
        let mut text = add(self.sha.0 as u64, self.parents.bytes as u64)?;
        for length in [
            self.author.0,
            self.author_name.as_ref().map_or(0, |text| text.0),
            self.author_email.as_ref().map_or(0, |text| text.0),
            self.committer.0,
            self.committer_name.as_ref().map_or(0, |text| text.0),
            self.committer_email.as_ref().map_or(0, |text| text.0),
            self.message.0,
            self.tags.bytes,
        ] {
            text = add(text, length as u64)?;
        }
        Ok(text)
    }

    pub(super) fn batch_sizes(&self) -> Result<(u64, usize, usize, usize), CoreError> {
        Ok((
            self.text_bytes()?,
            self.parents.count,
            self.tags.count,
            self.tags.bytes,
        ))
    }

    fn collection_backing(&self, key_bytes: usize) -> Result<u64, CoreError> {
        // Parents use Vec<CommitSha>. Tags first use Vec<String>,
        // then Vec<Box<str>> while the original slots are still live.
        let items = add(add(self.parents.count as u64, self.tags.count as u64)?, 8)?;
        let vector_peak = mul(
            items,
            2 * (std::mem::size_of::<String>() + std::mem::size_of::<Box<str>>()) as u64,
        )?;
        add(vector_peak, add(key_backing(key_bytes)?, 4096)?)
    }

    fn backing(&self, key_bytes: usize) -> Result<RowBacking, CoreError> {
        let text = self.text_bytes()?;
        let base = self.collection_backing(key_bytes)?;
        // CBOR can grow owned scalar backing before it is converted to a box.
        RowBacking::new(add(text, base)?, add(mul(text, 4)?, base)?)
    }
}

fn key_backing(bytes: usize) -> Result<u64, CoreError> {
    // JSON byte arrays grow a Vec<u8> from its minimum capacity. During a
    // growth, both the old capacity and the new capacity can be resident.
    mul(bytes.max(8) as u64, 3)
}

struct NativeShape<T> {
    key: ByteItems,
    data: NativeDataShape<T>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NativeShape<T> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["key", "data"];
        struct Shape<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Shape<T> {
            type Value = NativeShape<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a NativeShape History row")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut key = None;
                let mut data = None;
                while let Some(field) = map.next_key_seed(ShapeFieldSeed(FIELDS))? {
                    match field {
                        "key" => {
                            if key.is_some() {
                                return Err(de::Error::duplicate_field("key"));
                            }
                            key = Some(map.next_value::<ByteItems>()?);
                        }
                        "data" => {
                            if data.is_some() {
                                return Err(de::Error::duplicate_field("data"));
                            }
                            data = Some(map.next_value::<NativeDataShape<T>>()?);
                        }
                        _ => unreachable!("row field seed only returns schema keys"),
                    }
                }
                Ok(NativeShape {
                    key: key.ok_or_else(|| de::Error::missing_field("key"))?,
                    data: data.ok_or_else(|| de::Error::missing_field("data"))?,
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let key = seq
                    .next_element::<ByteItems>()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let data = seq
                    .next_element::<NativeDataShape<T>>()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(3, &self));
                }
                Ok(NativeShape { key, data })
            }
        }
        decoder.deserialize_struct("NativeShape", FIELDS, Shape(std::marker::PhantomData))
    }
}

enum NativeDataShape<T> {
    Row(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NativeDataShape<T> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        const VARIANTS: &[&str] = &["Row"];
        struct Shape<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Shape<T> {
            type Value = NativeDataShape<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a NativeDataShape History row variant")
            }

            fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                let (name, value) = de::EnumAccess::variant_seed(data, ShapeVariantSeed(VARIANTS))?;
                match name {
                    "Row" => value.newtype_variant::<T>().map(NativeDataShape::Row),
                    _ => unreachable!("row variant seed only returns schema names"),
                }
            }
        }
        decoder.deserialize_enum("NativeDataShape", VARIANTS, Shape(std::marker::PhantomData))
    }
}

newtype_shape!(enum NativeCommitShape {
    Commit: CommitShape,
});

newtype_shape!(enum NativeRefTagShape {
    Ref: RefTagShape,
    Tag: RefTagShape,
});

newtype_shape!(enum NativeDiffHunkShape {
    DiffHunk: DiffHunkShape,
});

closed_shape!(
    pub(super) struct DiffHunkShape {
        commit_sha: TextBytes,
        file_path: TextBytes,
        record: DiffRecordShape,
    }
);

closed_shape!(
    struct DiffRecordShape {
        _wire_version as wire_version: u32,
        hunk_header: TextBytes,
        _side as side: quanta_index_contract::DiffHunkSide,
        added_text: TextBytes,
        removed_text: TextBytes,
        touched_text: TextBytes,
        _byte_start as byte_start: u32,
        _byte_end as byte_end: u32,
    }
);

impl DiffHunkShape {
    pub(super) fn text_bytes(&self) -> Result<u64, CoreError> {
        let mut text = add(self.commit_sha.0 as u64, self.file_path.0 as u64)?;
        for width in [
            self.record.hunk_header.0,
            self.record.added_text.0,
            self.record.removed_text.0,
            self.record.touched_text.0,
        ] {
            text = add(text, width as u64)?;
        }
        Ok(text)
    }
}

closed_shape!(
    pub(super) struct RefTagShape {
        name: TextBytes,
        sha: TextBytes,
    }
);

impl RefTagShape {
    pub(super) fn text_bytes(&self) -> Result<u64, CoreError> {
        add(self.name.0 as u64, self.sha.0 as u64)
    }
}

fn shape_scratch(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<MemoryLease>, CoreError> {
    // The shape pass discards each owned scalar before visiting the next one.
    // Three payload lengths cover old/new growth of an indefinite CBOR scalar.
    // This loan ends before the typed decode grant.
    budget
        .reserve_memory(
            add(mul(payload.len() as u64, 3)?, 4096)?,
            MemoryLifetime::Temporary,
        )
        .map_err(|error| history_work_error(budget, error))
}

pub(super) struct JsonTokenScratch {
    pub(super) reader: u64,
    slice: u64,
}

#[derive(Default)]
struct SliceScratch {
    capacity: u64,
    peak: u64,
}

impl SliceScratch {
    fn append(&mut self, length: u64, bytes: u64) -> Result<(), CoreError> {
        let required = add(length, bytes)?;
        if required > self.capacity {
            let grown = required.max(mul(self.capacity, 2)?).max(8);
            self.peak = self.peak.max(add(self.capacity, grown)?);
            self.capacity = grown;
        }
        Ok(())
    }
}

fn json_escape(payload: &[u8], index: &mut usize) -> Result<char, CoreError> {
    fn unit(payload: &[u8], index: &mut usize) -> Result<u16, CoreError> {
        let bytes = payload
            .get(*index..*index + 4)
            .ok_or_else(|| invalid("History JSON unicode escape is incomplete"))?;
        *index += 4;
        bytes.iter().try_fold(0_u16, |value, byte| {
            let digit = match *byte {
                b'0'..=b'9' => u16::from(*byte - b'0'),
                b'a'..=b'f' => u16::from(*byte - b'a' + 10),
                b'A'..=b'F' => u16::from(*byte - b'A' + 10),
                _ => return Err(invalid("History JSON unicode escape is invalid")),
            };
            Ok((value << 4) | digit)
        })
    }

    let escape = *payload
        .get(*index)
        .ok_or_else(|| invalid("History JSON escape is incomplete"))?;
    *index += 1;
    match escape {
        b'"' | b'\\' | b'/' => Ok(char::from(escape)),
        b'b' => Ok('\u{0008}'),
        b'f' => Ok('\u{000c}'),
        b'n' => Ok('\n'),
        b'r' => Ok('\r'),
        b't' => Ok('\t'),
        b'u' => {
            let high = unit(payload, index)?;
            let code = if (0xd800..=0xdbff).contains(&high) {
                if payload.get(*index..*index + 2) != Some(&b"\\u"[..]) {
                    return Err(invalid("History JSON surrogate pair is incomplete"));
                }
                *index += 2;
                let low = unit(payload, index)?;
                if !(0xdc00..=0xdfff).contains(&low) {
                    return Err(invalid("History JSON surrogate pair is invalid"));
                }
                0x10000 + (u32::from(high - 0xd800) << 10) + u32::from(low - 0xdc00)
            } else {
                u32::from(high)
            };
            char::from_u32(code).ok_or_else(|| invalid("History JSON surrogate pair is invalid"))
        }
        _ => Err(invalid("History JSON escape is invalid")),
    }
}

pub(super) fn json_token_scratch(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<(JsonTokenScratch, Option<MemoryLease>), CoreError> {
    budget
        .charge(WorkUsage {
            bytes: payload.len() as u64,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let mut token_start = None;
    let mut raw_start = 0;
    let mut decoded = 0;
    let mut largest_token = 0;
    let mut slice = SliceScratch::default();
    let mut index = 0;
    let mut next_checkpoint = 0;
    while index < payload.len() {
        if index >= next_checkpoint {
            budget
                .checkpoint()
                .map_err(|error| history_work_error(budget, error))?;
            next_checkpoint = index.saturating_add(64 * 1024);
        }
        let byte = payload[index];
        let end = index;
        index += 1;
        if let Some(start) = token_start {
            if byte != b'"' && byte != b'\\' {
                continue;
            }
            let raw = (end - raw_start) as u64;
            // SliceRead appends raw segments only after entering the escape
            // path. Its scratch capacity survives the end of each string.
            if byte == b'\\' || raw_start != start {
                slice.append(decoded, raw)?;
            }
            decoded = add(decoded, raw)?;
            if byte == b'\\' {
                let scalar = json_escape(payload, &mut index)?;
                let width = scalar.len_utf8() as u64;
                slice.append(decoded, width)?;
                decoded = add(decoded, width)?;
                raw_start = index;
            } else {
                largest_token = largest_token.max(decoded);
                token_start = None;
            }
        } else if byte == b'"' {
            token_start = Some(index);
            raw_start = index;
            decoded = 0;
        }
    }
    if token_start.is_some() {
        return Err(invalid("History Native JSON string is unterminated"));
    }

    // The shape deserializer bounds diagnostics before formatting an
    // unexpected field value. This grant is independent of the raw wire loan.
    let memory = budget
        .reserve_memory(add(slice.peak, 4096)?, MemoryLifetime::Temporary)
        .map_err(|error| history_work_error(budget, error))?;

    // IoRead pushes a byte or one UTF-8 scalar at a time. Starting at eight
    // bytes, every append fits within the next doubling.
    let capacity = largest_token
        .checked_next_power_of_two()
        .ok_or_else(|| invalid("History JSON scratch capacity overflow"))?
        .max(8);
    let previous = if capacity > 8 { capacity / 2 } else { 0 };
    let reader = add(add(capacity, previous)?, SOURCE_JSON_IO_BUFFER_BYTES as u64)?;
    Ok((
        JsonTokenScratch {
            reader,
            slice: slice.peak,
        },
        memory,
    ))
}

pub(super) fn decode_json_shape<T: DeserializeOwned>(payload: &[u8]) -> Result<T, CoreError> {
    json_shape::from_slice(payload).map_err(|error| CoreError::InvalidContract(error.to_string()))
}

pub(super) fn commit_cbor_backing(
    payload: &[u8],
    key_bytes: usize,
    budget: &mut WorkBudget,
) -> Result<RowBacking, CoreError> {
    let scratch = shape_scratch(payload, budget)?;
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    budget
        .charge(WorkUsage {
            bytes: payload.len() as u64,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))?;
    let shape: CommitShape = quanta_index_ipc::decode_cbor_payload(payload)
        .map_err(|error| invalid(&error.to_string()))?;
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    drop(scratch);
    shape.backing(key_bytes)
}

pub(super) fn native_json_backing(
    family: AuxiliaryRowFamilyV1,
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
    let retained = match family {
        AuxiliaryRowFamilyV1::Commit => {
            let row: NativeShape<NativeCommitShape> = decode_json_shape(payload)?;
            let NativeDataShape::Row(NativeCommitShape::Commit(commit)) = row.data;
            add(commit.text_bytes()?, commit.collection_backing(row.key.0)?)?
        }
        AuxiliaryRowFamilyV1::Ref | AuxiliaryRowFamilyV1::Tag => {
            let row: NativeShape<NativeRefTagShape> = decode_json_shape(payload)?;
            let ref_tag = match (family, row.data) {
                (
                    AuxiliaryRowFamilyV1::Ref,
                    NativeDataShape::Row(NativeRefTagShape::Ref(value)),
                )
                | (
                    AuxiliaryRowFamilyV1::Tag,
                    NativeDataShape::Row(NativeRefTagShape::Tag(value)),
                ) => value,
                _ => return Err(invalid("History typed row differs from its family")),
            };
            let base = add(key_backing(row.key.0)?, add(ref_tag.sha.0 as u64, 4096)?)?;
            add(ref_tag.name.0 as u64, base)?
        }
        AuxiliaryRowFamilyV1::DiffHunk => {
            let row: NativeShape<NativeDiffHunkShape> = decode_json_shape(payload)?;
            let NativeDataShape::Row(NativeDiffHunkShape::DiffHunk(hunk)) = row.data;
            let text = hunk.text_bytes()?;
            let base = add(key_backing(row.key.0)?, 4096)?;
            add(mul(text, 2)?, base)?
        }
        _ => return Err(invalid("History JSON shape family is invalid")),
    };
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    drop(scratch);
    // The shape pass has checked field and variant types. The typed SliceRead
    // decode copies each borrowed string into its final backing and uses the
    // parser's scratch only for escaped strings.
    RowBacking::new(retained, add(retained, scratch_bytes.slice)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        DiffHunkSide, ManifestGeneration, RepoId, RevisionId,
        lex::{CommitRecord, CommitSha, DiffHunkRecord},
    };
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use quanta_index_core::{AuxiliaryDomainV1, AuxiliaryRowKeyV1};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    #[test]
    fn history_row_shapes_preserve_nullable_fields_and_tagged_variants() {
        let commit = br#"{"wire_version":1,"sha":"abc","parents":[],"author_time_ms":1,"committer_time_ms":1,"applied_at_ms":1,"author":"a","committer":"c","message":"m","is_merge":false,"tags":[]}"#;
        let shape: CommitShape = decode_json_shape(commit).expect("omitted nullable commit fields");
        assert!(shape.author_name.is_none());
        assert!(shape.author_email.is_none());
        assert!(shape.committer_name.is_none());
        assert!(shape.committer_email.is_none());

        let row: NativeShape<NativeRefTagShape> =
            decode_json_shape(br#"{"key":[1],"data":{"Row":{"Ref":{"name":"main","sha":"abc"}}}}"#)
                .expect("tagged Native row");
        assert_eq!(row.key.0, 1);
        assert!(matches!(
            row.data,
            NativeDataShape::Row(NativeRefTagShape::Ref(_))
        ));
        for invalid in [
            br#"{"key":[1],"data":{"Row":{"Ref":{"name":"main","sha":"abc","other":0}}}}"#
                .as_slice(),
            br#"{"key":[1],"data":{"Row":{"0":{"name":"main","sha":"abc"}}}}"#.as_slice(),
            br#"{"key":[1],"data":{"Row":{"Ref":{"name":"main","sha":"abc"}}},"key":[2]}"#
                .as_slice(),
        ] {
            assert!(decode_json_shape::<NativeShape<NativeRefTagShape>>(invalid).is_err());
        }

        // A fixed CBOR map with byte-string keys, independent of the decoder.
        let cbor = b"\xa2\x44name\x64main\x43sha\x63abc";
        let shape: RefTagShape =
            quanta_index_ipc::decode_cbor_payload(cbor).expect("known byte keys");
        assert_eq!(shape.name.0, 4);
        assert_eq!(shape.sha.0, 3);
    }

    struct Ledger {
        live: AtomicU64,
        maximum: u64,
    }
    struct Scope(Arc<Ledger>);
    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            assert!(self.live.fetch_sub(bytes, Ordering::AcqRel) >= bytes);
        }
    }
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes == 0 {
                return Err(IndexError::InvalidInput(
                    "live memory reservation must be nonzero".into(),
                ));
            }
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
    fn work(maximum: u64) -> (WorkBudget, Arc<Ledger>) {
        let ledger = Arc::new(Ledger {
            live: AtomicU64::new(0),
            maximum,
        });
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
        (budget, ledger)
    }

    #[test]
    fn equal_decode_peak_never_requests_zero_scratch() -> Result<(), CoreError> {
        let (mut budget, ledger) = work(4096);
        let (retained, scratch) = RowBacking::new(4096, 4096)?.reserve_decode(&mut budget)?;
        assert!(scratch.is_none());
        assert_eq!(ledger.live.load(Ordering::Acquire), 4096);
        drop(retained);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }
    fn record(message: &str, tags: Vec<Box<str>>) -> CommitRecord {
        CommitRecord {
            wire_version: 1,
            sha: CommitSha::from_bytes([7; 20]),
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
            message: message.into(),
            is_merge: false,
            tags,
        }
    }
    fn row(record: &CommitRecord) -> AuxiliaryRowV1 {
        AuxiliaryRowV1 {
            key: AuxiliaryRowKeyV1 {
                domain: AuxiliaryDomainV1::History,
                generation: quanta_index_core::AuxiliaryGenerationKeyV1 {
                    repo_id: RepoId::new("large-commit").expect("repo"),
                    revision_id: RevisionId::new("main").expect("revision"),
                    generation: ManifestGeneration::new(1),
                },
                family: AuxiliaryRowFamilyV1::Commit,
                row_key: record.sha.as_bytes().to_vec(),
            },
            value: quanta_index_ipc::encode_cbor_payload(record).expect("commit CBOR"),
        }
    }

    #[test]
    fn json_scratch_tracks_slice_appends_and_reader_capacity_separately() -> Result<(), CoreError> {
        let payload = br#"["123456789abc\n", "bbbbbbbbbbb\n"]"#;
        let (mut budget, ledger) = work(8192);
        let (scratch, loan) = json_token_scratch(payload, &mut budget)?;
        // SliceRead first appends a 12-byte run, then doubles it for the escape.
        // IoRead crosses from eight to sixteen with one byte at a time.
        assert_eq!(scratch.slice, 36);
        assert_eq!(scratch.reader, SOURCE_JSON_IO_BUFFER_BYTES as u64 + 24);
        assert_eq!(ledger.live.load(Ordering::Acquire), 4096 + 36);
        drop(loan);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn shape_error_keeps_normal_diagnostics_and_bounds_an_echoed_token() -> Result<(), CoreError> {
        let input = br#""wrong""#;
        let original = serde_json::from_slice::<u64>(input)
            .expect_err("integer field must reject a string")
            .to_string();
        assert!(matches!(
            decode_json_shape::<u64>(input),
            Err(CoreError::InvalidContract(message)) if message == original
        ));

        let payload = format!("\"{}\"", "x".repeat(8192));
        let (mut budget, ledger) = work(4096);
        let (_, scratch) = json_token_scratch(payload.as_bytes(), &mut budget)?;
        assert!(matches!(
            decode_json_shape::<u64>(payload.as_bytes()),
            Err(CoreError::InvalidContract(message))
                if message.len() < 1024 && message.contains("line 1")
        ));
        drop(scratch);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn long_commit_shape_fits_original_budget() {
        let record = record(&"m".repeat(5 * 1024 * 1024), Vec::new());
        let row = row(&record);
        let (mut budget, ledger) = work(64 * 1024 * 1024);
        let source = budget
            .reserve_memory((row.value.len() * 2) as u64, MemoryLifetime::Retained)
            .expect("existing source custody");
        let bound = commit_cbor_backing(&row.value, row.key.row_key.len(), &mut budget)
            .expect("typed commit shape");
        let projection = budget
            .reserve_memory(bound.peak, MemoryLifetime::Temporary)
            .expect("64 MiB projection grant");
        let decoded = decode_history_row(&row).expect("typed commit decode");
        assert!(
            matches!(&decoded, HistoryReadbackRowV1::Commit(commit) if commit.message.len() == 5 * 1024 * 1024)
        );
        let data = NativeRowData::Row(decoded);
        let native = NativeRowRef {
            key: &row.key.row_key,
            data: &data,
        };
        let json = serde_json::to_vec(&native).expect("typed Native V1 JSON");
        assert!(json.len() <= MAX_HISTORY_SOURCE_INPUT_BYTES_V1);
        drop(native);
        drop(data);
        drop(projection);
        let json_backing = native_json_backing(AuxiliaryRowFamilyV1::Commit, &json, &mut budget)
            .expect("typed V1 shape");
        let (readback, scratch) = json_backing
            .reserve_decode(&mut budget)
            .expect("64 MiB Native readback grant");
        let restored: NativeRow = serde_json::from_slice(&json).expect("Native typed Commit");
        assert!(matches!(
            &restored.data,
            NativeRowData::Row(HistoryReadbackRowV1::Commit(_))
        ));
        drop(restored);
        drop(scratch);
        drop(readback);
        drop(source);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    }

    #[test]
    fn retained_commit_does_not_hold_decode_peak_during_cbor_reread() {
        let record = record(&"m".repeat(12 * 1024 * 1024), Vec::new());
        let row = row(&record);
        drop(record);
        assert!(row.value.len() < MAX_HISTORY_SOURCE_INPUT_BYTES_V1);
        let (mut budget, ledger) = work(64 * 1024 * 1024);
        let raw = budget
            .reserve_memory(row.value.len() as u64, MemoryLifetime::Temporary)
            .expect("source row backing");
        let backing = commit_cbor_backing(&row.value, row.key.row_key.len(), &mut budget)
            .expect("typed Commit shape");
        assert!(backing.peak + 2 * row.value.len() as u64 > 64 * 1024 * 1024);
        let (retained, scratch) = backing
            .reserve_decode(&mut budget)
            .expect("typed Commit decode admission");
        let decoded = decode_history_row(&row).expect("typed Commit decode");
        let HistoryReadbackRowV1::Commit(decoded) = decoded else {
            panic!("wrong History row family")
        };
        let mut state = HistoryAuthorityState::default();
        state.restore_commit(decoded.sha, decoded);
        drop(scratch);

        let (mut low, low_ledger) = work(backing.peak - 1);
        let low_backing = commit_cbor_backing(&row.value, row.key.row_key.len(), &mut low)
            .expect("shape preflight does not retain scratch");
        assert!(low_backing.reserve_decode(&mut low).is_err());
        assert_eq!(low_ledger.live.load(Ordering::Acquire), 0);

        let generation = row.key.generation.clone();
        drop(row);
        drop(raw);
        let epoch = AuxEpochV1::new(1);
        let mut projected = false;
        crate::auxiliary_authority::history_source::visit_history_state_rows(
            &generation,
            epoch,
            &state,
            &mut budget,
            &mut |row, budget| {
                if row.key.family == AuxiliaryRowFamilyV1::Commit {
                    let typed = borrowed_projection::from_source(&state, epoch, row)?;
                    let width = json_len(&typed, budget)?;
                    let encoded = encode_json_sized(&typed, width, budget)?;
                    assert!(
                        (12 * 1024 * 1024..MAX_HISTORY_SOURCE_INPUT_BYTES_V1)
                            .contains(&encoded.bytes.len())
                    );
                    projected = true;
                }
                Ok(())
            },
        )
        .expect("canonical History CBOR reread and V1 projection under original 64 MiB");
        assert!(projected);
        drop(state);
        drop(retained);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    }

    #[test]
    fn many_empty_tags_refuse_and_refund() {
        let record = record("", vec!["".into(); 100_000]);
        let row = row(&record);
        let (mut budget, ledger) = work(2 * 1024 * 1024);
        let bound = commit_cbor_backing(&row.value, row.key.row_key.len(), &mut budget)
            .expect("collection shape");
        assert!(bound.peak > 2 * 1024 * 1024);
        assert!(bound.reserve_decode(&mut budget).is_err());
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    }

    #[test]
    fn escaped_diff_hunk_v1_json_fits_original_budget() {
        let record = DiffHunkRecord {
            wire_version: 1,
            hunk_header: "@@".into(),
            side: DiffHunkSide::After,
            added_text: "\u{0001}".repeat(5 * 1024 * 1024 / 2).into_boxed_str(),
            removed_text: "".into(),
            touched_text: "".into(),
            byte_start: 0,
            byte_end: 5 * 1024 * 1024 / 2,
        };
        let sha = CommitSha::from_bytes([7; 20]);
        let data = NativeRowData::Row(HistoryReadbackRowV1::DiffHunk(HistoryDiffHunkUpsert {
            commit_sha: sha,
            file_path: "big.rs".into(),
            record,
        }));
        let mut key = sha.as_bytes().to_vec();
        key.extend_from_slice(b"\x1fbig.rs");
        let native = NativeRowRef {
            key: &key,
            data: &data,
        };
        let json = serde_json::to_vec(&native).expect("typed V1 JSON");
        assert!(json.len() > 15 * 1024 * 1024);
        assert!(json.len() <= MAX_HISTORY_SOURCE_INPUT_BYTES_V1);
        drop(native);
        drop(data);
        let (mut budget, ledger) = work(64 * 1024 * 1024);
        let raw = budget
            .reserve_memory(json.len() as u64, MemoryLifetime::Retained)
            .expect("original immutable object custody");
        let backing = native_json_backing(AuxiliaryRowFamilyV1::DiffHunk, &json, &mut budget)
            .expect("typed DiffHunk shape");
        let (readback, scratch) = backing
            .reserve_decode(&mut budget)
            .expect("64 MiB V1 DiffHunk readback grant");
        let typed: NativeDiffHunkRow = serde_json::from_slice(&json).expect("typed V1 DiffHunk");
        let NativeDiffHunkData::Row(NativeDiffHunkValue::DiffHunk(hunk)) = typed.data;
        assert_eq!(hunk.record.added_text.len(), 5 * 1024 * 1024 / 2);
        drop(hunk);
        drop(typed.key);
        drop(scratch);
        drop(readback);
        drop(raw);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
    }
}
