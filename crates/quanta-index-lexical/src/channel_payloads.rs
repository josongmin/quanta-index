//! Decoding the payloads a lexical channel op carries.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{BatchIngestMode, ChunkRecord, ManifestGeneration};
use quanta_index_core::CoreError;
use serde::de::value::SeqAccessDeserializer;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::marker::PhantomData;

/// Decode exactly one CBOR value. `ciborium::from_reader` does not require EOF,
/// so a successful prefix alone cannot authenticate a persisted artifact or
/// an ingest payload.
pub(crate) fn decode_cbor_exact<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut reader = std::io::Cursor::new(bytes);
    let value = ciborium::from_reader(&mut reader).map_err(|error| error.to_string())?;
    if usize::try_from(reader.position()) != Ok(bytes.len()) {
        return Err("trailing CBOR bytes".to_string());
    }
    Ok(value)
}

/// Read only the outer array header and its first unsigned integer. Older
/// manifest versions must be refused before their different bodies are read.
pub(crate) fn leading_cbor_array_version(bytes: &[u8]) -> Result<u32, &'static str> {
    fn header(bytes: &[u8], offset: usize) -> Result<(u8, Option<u64>, usize), &'static str> {
        let first = *bytes.get(offset).ok_or("truncated manifest header")?;
        let next = offset.checked_add(1).ok_or("manifest offset overflow")?;
        let additional = first & 31;
        if additional == 31 {
            return Ok((first >> 5, None, next));
        }
        let width = match additional {
            0..=23 => return Ok((first >> 5, Some(u64::from(additional)), next)),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err("invalid manifest header"),
        };
        let end = next.checked_add(width).ok_or("manifest offset overflow")?;
        let encoded = bytes.get(next..end).ok_or("truncated manifest header")?;
        let mut value = 0_u64;
        for byte in encoded {
            value = value
                .checked_mul(256)
                .and_then(|prior| prior.checked_add(u64::from(*byte)))
                .ok_or("manifest header overflow")?;
        }
        Ok((first >> 5, Some(value), end))
    }

    let (major, count, next) = header(bytes, 0)?;
    if major != 4 {
        return Err("manifest is not an array");
    }
    if count == Some(0) {
        return Err("manifest has no leading format version");
    }
    let (major, version, after_version) = header(bytes, next)?;
    let version = if major == 0 {
        version.ok_or("manifest format version is not a u32")?
    } else if major == 6 && version == Some(2) {
        // ciborium's Value decoder normalizes definite BIGPOS byte strings
        // fitting u128 into Value::Integer. The old leading-version check
        // therefore accepted a small tagged positive version as an integer.
        let (body_major, length, body_start) = header(bytes, after_version)?;
        let length = length.ok_or("manifest has no leading format version")?;
        if body_major != 2 || length > 16 {
            return Err("manifest has no leading format version");
        }
        let body_end = body_start
            .checked_add(
                usize::try_from(length).map_err(|_error| "manifest version length overflow")?,
            )
            .ok_or("manifest version length overflow")?;
        let body = bytes
            .get(body_start..body_end)
            .ok_or("truncated manifest version")?;
        let mut number = 0_u128;
        for byte in body {
            number = number
                .checked_mul(256)
                .and_then(|prior| prior.checked_add(u128::from(*byte)))
                .ok_or("manifest format version is not a u32")?;
        }
        u64::try_from(number).map_err(|_error| "manifest format version is not a u32")?
    } else {
        return Err("manifest has no leading format version");
    };
    u32::try_from(version).map_err(|_overflow| "manifest format version is not a u32")
}

/// Decode one bounded manifest member with the prior Value semantics.
///
/// Shape preflight bounds the member before this adapter runs.
#[derive(Debug)]
pub(crate) struct ValueCompatible<T>(pub(crate) T);

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for ValueCompatible<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = ciborium::Value::deserialize(deserializer)?;
        value
            .deserialized()
            .map(ValueCompatible)
            .map_err(de::Error::custom)
    }
}

// Widest row: six shard fields plus a 32-element digest, at most 40 Value
// nodes. Charge each node and up to ciborium's 256 recursion wrappers.
const MAX_MANIFEST_MEMBER_NODES: u64 = 40 * 257;

/// Check manifest shape without constructing a Value tree.
///
/// Collections use owner row limits. Other arrays have at most 32 elements.
/// Byte strings are refused except for short CBOR numeric bignums.
pub(crate) fn preflight_manifest_shape(
    bytes: &[u8],
    fields: usize,
    collections: &[usize],
    max_collection_rows: usize,
    allow_text: bool,
) -> Result<(), &'static str> {
    struct Scan<'a> {
        bytes: &'a [u8],
        offset: usize,
        allow_text: bool,
    }
    impl Scan<'_> {
        fn header(&mut self) -> Result<(u8, Option<u64>), &'static str> {
            let first = *self
                .bytes
                .get(self.offset)
                .ok_or("truncated manifest CBOR")?;
            self.offset = self
                .offset
                .checked_add(1)
                .ok_or("manifest offset overflow")?;
            let additional = first & 31;
            if additional == 31 {
                return Ok((first >> 5, None));
            }
            let width = match additional {
                0..=23 => return Ok((first >> 5, Some(u64::from(additional)))),
                24 => 1,
                25 => 2,
                26 => 4,
                27 => 8,
                _ => return Err("invalid manifest CBOR header"),
            };
            let end = self
                .offset
                .checked_add(width)
                .ok_or("manifest offset overflow")?;
            let encoded = self
                .bytes
                .get(self.offset..end)
                .ok_or("truncated manifest CBOR")?;
            self.offset = end;
            let mut value = 0_u64;
            for byte in encoded {
                value = value
                    .checked_mul(256)
                    .and_then(|prior| prior.checked_add(u64::from(*byte)))
                    .ok_or("manifest CBOR header overflow")?;
            }
            Ok((first >> 5, Some(value)))
        }

        fn consume_bytes(&mut self, length: u64) -> Result<(), &'static str> {
            let length =
                usize::try_from(length).map_err(|_error| "manifest byte length overflow")?;
            self.offset = self
                .offset
                .checked_add(length)
                .ok_or("manifest offset overflow")?;
            if self.offset > self.bytes.len() {
                return Err("truncated manifest CBOR");
            }
            Ok(())
        }

        fn take_break(&mut self) -> Result<bool, &'static str> {
            if self.bytes.get(self.offset) == Some(&0xff) {
                self.offset = self
                    .offset
                    .checked_add(1)
                    .ok_or("manifest offset overflow")?;
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn value(
            &mut self,
            depth: u16,
            array_bound: usize,
            bignum_bytes: bool,
            collection_root: bool,
            nodes: &mut u64,
        ) -> Result<(), &'static str> {
            // Match ciborium's maximum recursion depth for tagged legacy
            // rows; decoded-node admission below bounds fanout separately.
            if depth > 256 {
                return Err("manifest CBOR nesting exceeds row shape");
            }
            *nodes = nodes
                .checked_add(1)
                .ok_or("manifest member node count overflow")?;
            if *nodes > MAX_MANIFEST_MEMBER_NODES {
                return Err("manifest member exceeds its decoded node ceiling");
            }
            let (major, argument) = self.header()?;
            match (major, argument) {
                (0 | 1 | 7, Some(_)) => Ok(()),
                (2, Some(length)) if bignum_bytes && !collection_root && length <= 16 => {
                    self.consume_bytes(length)
                }
                (2, None) if bignum_bytes && !collection_root => {
                    let mut aggregate = 0_u64;
                    while !self.take_break()? {
                        let (chunk_major, length) = self.header()?;
                        if chunk_major != 2 {
                            return Err("manifest bignum has a wrong chunk");
                        }
                        let length = length.ok_or("nested indefinite bignum")?;
                        aggregate = aggregate
                            .checked_add(length)
                            .ok_or("manifest bignum length overflow")?;
                        if aggregate > 16 {
                            return Err("manifest bignum exceeds 16 bytes");
                        }
                        self.consume_bytes(length)?;
                    }
                    Ok(())
                }
                (2, _) => Err("manifest byte string is not an array"),
                (3, Some(length)) if self.allow_text => self.consume_bytes(length),
                (3, None) if self.allow_text => {
                    while !self.take_break()? {
                        let (chunk_major, length) = self.header()?;
                        if chunk_major != 3 {
                            return Err("manifest text has a wrong chunk");
                        }
                        self.consume_bytes(length.ok_or("nested indefinite text")?)?;
                    }
                    Ok(())
                }
                (3, _) => Err("text is not a text-authority manifest field"),
                (4, count) => {
                    let bound = u64::try_from(array_bound)
                        .map_err(|_error| "manifest array bound overflow")?;
                    if count.is_some_and(|count| count > bound) {
                        return Err("manifest array count exceeds its field ceiling");
                    }
                    let mut seen = 0_u64;
                    loop {
                        if count.is_some_and(|count| seen == count)
                            || (count.is_none() && self.take_break()?)
                        {
                            break;
                        }
                        if seen == bound {
                            return Err("manifest array count exceeds its field ceiling");
                        }
                        if collection_root {
                            let mut row_nodes = 0;
                            self.value(
                                depth.checked_add(1).ok_or("manifest CBOR depth overflow")?,
                                32,
                                false,
                                false,
                                &mut row_nodes,
                            )?;
                        } else {
                            self.value(
                                depth.checked_add(1).ok_or("manifest CBOR depth overflow")?,
                                32,
                                false,
                                false,
                                nodes,
                            )?;
                        }
                        seen = seen.checked_add(1).ok_or("manifest array count overflow")?;
                    }
                    Ok(())
                }
                (6, Some(tag)) => self.value(
                    depth.checked_add(1).ok_or("manifest CBOR depth overflow")?,
                    array_bound,
                    tag == 2 || tag == 3,
                    collection_root,
                    nodes,
                ),
                _ => Err("invalid manifest CBOR value"),
            }
        }
    }
    let mut scan = Scan {
        bytes,
        offset: 0,
        allow_text,
    };
    let (major, count) = scan.header()?;
    if major != 4 {
        return Err("manifest is not an array");
    }
    let fields_u64 = u64::try_from(fields).map_err(|_error| "manifest field count overflow")?;
    if count.is_some_and(|count| count != fields_u64) {
        return Err("manifest row field count differs");
    }
    for field in 0..fields {
        let bound = if collections.contains(&field) {
            max_collection_rows
        } else {
            32
        };
        let mut member_nodes = 0;
        scan.value(
            1,
            bound,
            false,
            collections.contains(&field),
            &mut member_nodes,
        )?;
    }
    if count.is_none() && !scan.take_break()? {
        return Err("manifest indefinite row has extra fields");
    }
    if scan.offset != bytes.len() {
        return Err("trailing manifest CBOR bytes");
    }
    Ok(())
}

/// Consume a fixed manifest row, including an indefinite outer break.
///
/// A tuple alone leaves that break unread in ciborium. This wrapper checks
/// there is no extra field and consumes the break before exact EOF checking.
#[derive(Debug)]
pub(crate) struct ExactManifestRow<T>(pub(crate) T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ExactManifestRow<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for RowVisitor<T> {
            type Value = ExactManifestRow<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a complete manifest row")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let row = T::deserialize(SeqAccessDeserializer::new(&mut access))?;
                if access.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("manifest row has extra fields"));
                }
                Ok(ExactManifestRow(row))
            }
        }
        deserializer.deserialize_seq(RowVisitor::<T>(PhantomData))
    }
}

/// Decode a collection with bounded row allocation.
///
/// Definite and indefinite arrays obey the owner format's row limit.
#[derive(Debug)]
pub(crate) struct BoundedCborRows<T, const MAX: usize>(pub(crate) Vec<T>);

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for BoundedCborRows<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor<T, const MAX: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for RowsVisitor<T, MAX> {
            type Value = BoundedCborRows<T, MAX>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "at most {MAX} manifest rows")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                if access.size_hint().is_some_and(|count| count > MAX) {
                    return Err(de::Error::custom("manifest row count exceeds its ceiling"));
                }
                let mut rows = Vec::new();
                while rows.len() < MAX {
                    let Some(row) = access.next_element()? else {
                        return Ok(BoundedCborRows(rows));
                    };
                    rows.try_reserve(1).map_err(de::Error::custom)?;
                    rows.push(row);
                }
                if access.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("manifest row count exceeds its ceiling"));
                }
                Ok(BoundedCborRows(rows))
            }
        }
        deserializer.deserialize_seq(RowsVisitor::<T, MAX>(PhantomData))
    }
}

#[cfg(test)]
mod bounded_manifest_tests {
    use super::{BoundedCborRows, decode_cbor_exact, leading_cbor_array_version};

    #[test]
    fn definite_count_refuses_before_reading_an_element() {
        // Array(3) announces more rows than the bound but contains no body.
        // An EOF error would mean the decoder attempted to read or reserve it.
        let error = decode_cbor_exact::<BoundedCborRows<u8, 2>>(&[0x83])
            .expect_err("over-limit array must refuse before its first element");
        assert!(
            error.contains("manifest row count exceeds its ceiling"),
            "{error}"
        );
    }

    #[test]
    fn indefinite_array_is_bounded_and_exact() {
        let rows = decode_cbor_exact::<BoundedCborRows<u8, 2>>(&[0x9f, 1, 2, 0xff])
            .expect("two rows fit the bound");
        assert_eq!(rows.0, vec![1, 2]);
        let error = decode_cbor_exact::<BoundedCborRows<u8, 2>>(&[0x9f, 1, 2, 3, 0xff])
            .expect_err("third indefinite row exceeds the bound");
        assert!(
            error.contains("manifest row count exceeds its ceiling"),
            "{error}"
        );
    }

    #[test]
    fn version_probe_accepts_legacy_shape_without_decoding_its_body() {
        assert_eq!(leading_cbor_array_version(&[0x81, 15]), Ok(15));
        assert_eq!(leading_cbor_array_version(&[0x9f, 2]), Ok(2));
        assert_eq!(leading_cbor_array_version(&[0x81, 0xc2, 0x41, 15]), Ok(15));
        assert_eq!(leading_cbor_array_version(&[0x81, 0xc2, 0x41, 2]), Ok(2));
        assert!(leading_cbor_array_version(&[0x81, 0xc0, 15]).is_err());
        assert!(leading_cbor_array_version(&[0x80]).is_err());
        assert!(leading_cbor_array_version(&[0x81, 0x20]).is_err());
    }

    #[test]
    fn oversized_byte_string_is_refused_before_seq_materialization() {
        // ciborium can present a byte string as SeqAccess after allocating
        // its contents. Manifest binary values are only 32-byte digests.
        let error = super::preflight_manifest_shape(&[0x81, 0x58, 33], 1, &[], 2, true)
            .expect_err("binary value is not a manifest array");
        assert_eq!(error, "manifest byte string is not an array");
    }

    #[test]
    fn malformed_nested_tree_refuses_before_value_materialization() {
        let mut bytes = vec![0x81, 0x98, 32]; // one field containing array(32)
        for _ in 0..32 {
            bytes.extend_from_slice(&[0x98, 32]);
            for _ in 0..32 {
                bytes.extend_from_slice(&[0x98, 32]);
                bytes.extend_from_slice(&[0; 32]);
            }
        }
        let error = super::preflight_manifest_shape(&bytes, 1, &[], 2, true)
            .expect_err("nested Value tree exceeds one member's node ceiling");
        assert_eq!(error, "manifest member exceeds its decoded node ceiling");
    }
}

pub(crate) fn decode_chunk_payload(bytes: &[u8]) -> Result<ChunkRecord, CoreError> {
    decode_cbor_exact::<ChunkRecord>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: chunk payload decode: {err}")))
}

pub(crate) fn decode_symbol_payload(bytes: &[u8]) -> Result<SymbolRecord, CoreError> {
    decode_cbor_exact::<SymbolRecord>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: symbol payload decode: {err}")))
}

/// A length or a count as `u64`; a platform where `usize` exceeds `u64`
/// is refused rather than truncated.
pub(crate) fn count_from_len(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: count overflow: {err}")))
}

pub(crate) fn encode_cbor<T>(value: &T, label: &str) -> Result<Vec<u8>, CoreError>
where
    T: serde::Serialize,
{
    let mut payload = Vec::new();
    ciborium::into_writer(value, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: encode {label}: {err}")))?;
    Ok(payload)
}

pub(crate) fn decode_replace_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusReplaceScope,
    ),
    CoreError,
> {
    decode_cbor_exact::<(
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusReplaceScope,
    )>(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("lexical: replace scope payload decode: {err}"))
    })
}

pub(crate) fn decode_tombstone_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusTombstoneScope,
    ),
    CoreError,
> {
    decode_cbor_exact::<(
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusTombstoneScope,
    )>(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("lexical: tombstone scope payload decode: {err}"))
    })
}
