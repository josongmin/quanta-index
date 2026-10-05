//! Bounded, canonical wire formats for the immutable file authority.
//!
//! The caller owns root identity, shard membership, stable source IDs, and the
//! aggregate budget. This codec checks only the exact bytes of one pack/block.

use sha2::{Digest as _, Sha256};

const VERSION: u16 = 1;
const SOURCE_MAGIC: &[u8; 8] = b"QISPACK1";
const POSTING_MAGIC: &[u8; 8] = b"QIPOST01";
const SOURCE_HEADER: usize = 24;
const SOURCE_ROW: usize = 44;
const POSTING_HEADER: usize = 32;
const POSTING_ROW: usize = 16;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodecError {
    /// Producer input or caller-selected policy is invalid.
    Invalid(&'static str),
    /// Committed encoded bytes are malformed or noncanonical.
    Corrupt(&'static str),
    /// A committed source body fails its declared SHA-256.
    DigestMismatch,
    /// The caller's explicit per-object bound was exceeded.
    Limit(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CodecLimits {
    pub(crate) max_source_pack_encoded_bytes: usize,
    pub(crate) max_posting_block_encoded_bytes: usize,
    pub(crate) max_sources: usize,
    pub(crate) max_terms: usize,
    pub(crate) max_memberships: u64,
}

impl CodecLimits {
    fn validate(self) -> Result<(), CodecError> {
        if self.max_source_pack_encoded_bytes < SOURCE_HEADER
            || self.max_posting_block_encoded_bytes < POSTING_HEADER
            || self.max_sources == 0
            || self.max_terms == 0
            || self.max_memberships == 0
        {
            return Err(CodecError::Invalid(
                "codec limits must be explicit and positive",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SourcePackInput<'a> {
    pub(crate) digest: [u8; 32],
    pub(crate) bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
struct SourceRow {
    digest: [u8; 32],
    offset: usize,
    len: usize,
}

pub(crate) struct SourcePackView<'a> {
    payload: &'a [u8],
    rows: Vec<SourceRow>,
}

impl<'a> SourcePackView<'a> {
    pub(crate) fn get(&self, digest: &[u8; 32]) -> Option<&'a [u8]> {
        let Ok(index) = self.rows.binary_search_by_key(digest, |row| row.digest) else {
            return None;
        };
        let row = self.rows.get(index)?;
        self.payload.get(row.offset..row.offset + row.len)
    }

    pub(crate) fn entries(&self) -> impl ExactSizeIterator<Item = ([u8; 32], &'a [u8])> + '_ {
        self.rows.iter().map(|row| {
            // Every range was checked and hashed by decode_source_pack.
            (row.digest, &self.payload[row.offset..row.offset + row.len])
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PostingSurface {
    Path,
    Content,
}

impl PostingSurface {
    fn wire(self) -> u8 {
        match self {
            Self::Path => 1,
            Self::Content => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PostingInput<'a> {
    pub(crate) gram: [u8; 3],
    pub(crate) source_ids: &'a [u64],
}

#[derive(Clone, Copy, Debug)]
struct PostingRow {
    gram: [u8; 3],
    offset: usize,
    count: usize,
}

pub(crate) struct PostingBlockView<'a> {
    payload: &'a [u8],
    payload_offset: usize,
    rows: Vec<PostingRow>,
    membership_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PostingTermDescriptor {
    pub(crate) gram: [u8; 3],
    pub(crate) offset: u64,
    pub(crate) count: u32,
    pub(crate) sha256: [u8; 32],
}

impl<'a> PostingBlockView<'a> {
    #[cfg(test)]
    pub(crate) fn lookup(&self, gram: [u8; 3]) -> Option<PostingIds<'a>> {
        let Ok(index) = self.rows.binary_search_by_key(&gram, |row| row.gram) else {
            return None;
        };
        let row = self.rows.get(index)?;
        let len = row.count.checked_mul(8)?;
        Some(PostingIds {
            bytes: self.payload.get(row.offset..row.offset.checked_add(len)?)?,
        })
    }

    pub(crate) fn iter_terms(
        &self,
    ) -> impl ExactSizeIterator<Item = ([u8; 3], usize, PostingIds<'a>)> + '_ {
        self.rows.iter().map(|row| {
            // Every range was checked by decode_posting_block.
            let bytes = &self.payload[row.offset..row.offset + row.count * 8];
            (row.gram, row.count, PostingIds { bytes })
        })
    }

    pub(crate) fn membership_count(&self) -> u64 {
        self.membership_count
    }

    pub(crate) fn term_descriptors(
        &self,
    ) -> impl ExactSizeIterator<Item = Result<PostingTermDescriptor, CodecError>> + '_ {
        self.rows.iter().map(|row| {
            let start = self
                .payload_offset
                .checked_add(row.offset)
                .ok_or(CodecError::Corrupt("term descriptor offset overflow"))?;
            let length = row
                .count
                .checked_mul(8)
                .ok_or(CodecError::Corrupt("term descriptor length overflow"))?;
            let end = row
                .offset
                .checked_add(length)
                .ok_or(CodecError::Corrupt("term descriptor end overflow"))?;
            let bytes = self
                .payload
                .get(row.offset..end)
                .ok_or(CodecError::Corrupt("term descriptor bounds"))?;
            Ok(PostingTermDescriptor {
                gram: row.gram,
                offset: u64::try_from(start)
                    .map_err(|_| CodecError::Corrupt("term descriptor offset width"))?,
                count: u32::try_from(row.count)
                    .map_err(|_| CodecError::Corrupt("term descriptor count width"))?,
                sha256: Sha256::digest(bytes).into(),
            })
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PostingIds<'a> {
    bytes: &'a [u8],
}

impl Iterator for PostingIds<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        let (word, rest) = self.bytes.split_first_chunk::<8>()?;
        self.bytes = rest;
        Some(u64::from_le_bytes(*word))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.bytes.len() / 8;
        (count, Some(count))
    }
}

impl ExactSizeIterator for PostingIds<'_> {}

fn add(a: usize, b: usize, why: &'static str) -> Result<usize, CodecError> {
    a.checked_add(b).ok_or(CodecError::Corrupt(why))
}

fn mul(a: usize, b: usize, why: &'static str) -> Result<usize, CodecError> {
    a.checked_mul(b).ok_or(CodecError::Corrupt(why))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, CodecError> {
    let end = add(at, 4, "integer offset overflow")?;
    let word: [u8; 4] = bytes
        .get(at..end)
        .ok_or(CodecError::Corrupt("truncated u32"))?
        .try_into()
        .map_err(|_| CodecError::Corrupt("truncated u32"))?;
    Ok(u32::from_le_bytes(word))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, CodecError> {
    let end = add(at, 2, "integer offset overflow")?;
    let word: [u8; 2] = bytes
        .get(at..end)
        .ok_or(CodecError::Corrupt("truncated u16"))?
        .try_into()
        .map_err(|_| CodecError::Corrupt("truncated u16"))?;
    Ok(u16::from_le_bytes(word))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, CodecError> {
    let end = add(at, 8, "integer offset overflow")?;
    let word: [u8; 8] = bytes
        .get(at..end)
        .ok_or(CodecError::Corrupt("truncated u64"))?
        .try_into()
        .map_err(|_| CodecError::Corrupt("truncated u64"))?;
    Ok(u64::from_le_bytes(word))
}

fn usize_from_u64(value: u64) -> Result<usize, CodecError> {
    usize::try_from(value).map_err(|_| CodecError::Corrupt("encoded length exceeds usize"))
}

pub(crate) fn encode_source_pack(
    sources: &[SourcePackInput<'_>],
    limits: &CodecLimits,
) -> Result<Vec<u8>, CodecError> {
    limits.validate()?;
    if sources.len() > limits.max_sources || sources.len() > u32::MAX as usize {
        return Err(CodecError::Limit("source count"));
    }
    let table_bytes = mul(sources.len(), SOURCE_ROW, "source table overflow")?;
    let mut total = add(SOURCE_HEADER, table_bytes, "source pack header overflow")?;
    let mut payload_bytes = 0_usize;
    let mut prior = None;
    for source in sources {
        if prior.is_some_and(|previous| previous >= source.digest) {
            return Err(CodecError::Invalid(
                "source digests must be strictly sorted",
            ));
        }
        prior = Some(source.digest);
        if source.bytes.len() > MAX_SOURCE_BYTES || source.bytes.len() > u32::MAX as usize {
            return Err(CodecError::Limit("source exceeds 8 MiB"));
        }
        let observed: [u8; 32] = Sha256::digest(source.bytes).into();
        if observed != source.digest {
            return Err(CodecError::Invalid("source bytes differ from digest"));
        }
        payload_bytes = add(payload_bytes, source.bytes.len(), "source payload overflow")?;
        total = add(total, source.bytes.len(), "source pack size overflow")?;
        if total > limits.max_source_pack_encoded_bytes {
            return Err(CodecError::Limit("source pack encoded bytes"));
        }
    }
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_| CodecError::Limit("source pack allocation"))?;
    out.extend_from_slice(SOURCE_MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&(sources.len() as u32).to_le_bytes());
    out.extend_from_slice(
        &u64::try_from(payload_bytes)
            .map_err(|_| CodecError::Limit("source payload length"))?
            .to_le_bytes(),
    );
    let mut offset = 0_u64;
    for source in sources {
        out.extend_from_slice(&source.digest);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(source.bytes.len() as u32).to_le_bytes());
        offset = offset
            .checked_add(source.bytes.len() as u64)
            .ok_or(CodecError::Limit("source offset"))?;
    }
    for source in sources {
        out.extend_from_slice(source.bytes);
    }
    Ok(out)
}

pub(crate) fn decode_source_pack<'a>(
    bytes: &'a [u8],
    limits: &CodecLimits,
) -> Result<SourcePackView<'a>, CodecError> {
    limits.validate()?;
    if bytes.len() > limits.max_source_pack_encoded_bytes {
        return Err(CodecError::Limit("source pack encoded bytes"));
    }
    if bytes.len() < SOURCE_HEADER
        || bytes.get(..8) != Some(SOURCE_MAGIC.as_slice())
        || u16_at(bytes, 8)? != VERSION
        || u16_at(bytes, 10)? != 0
    {
        return Err(CodecError::Corrupt("source pack header"));
    }
    let count = u32_at(bytes, 12)? as usize;
    if count > limits.max_sources {
        return Err(CodecError::Limit("source count"));
    }
    let payload_len = usize_from_u64(u64_at(bytes, 16)?)?;
    let table_end = add(
        SOURCE_HEADER,
        mul(count, SOURCE_ROW, "source table overflow")?,
        "source table end",
    )?;
    let expected = add(table_end, payload_len, "source pack size overflow")?;
    if expected != bytes.len() {
        return Err(CodecError::Corrupt("source pack length or trailing bytes"));
    }
    let payload = &bytes[table_end..];
    let mut rows = Vec::new();
    rows.try_reserve_exact(count)
        .map_err(|_| CodecError::Limit("source table allocation"))?;
    let mut offset = 0_usize;
    let mut prior = None;
    for index in 0..count {
        let start = SOURCE_HEADER + index * SOURCE_ROW;
        let digest: [u8; 32] = bytes[start..start + 32]
            .try_into()
            .map_err(|_| CodecError::Corrupt("source digest row"))?;
        if prior.is_some_and(|previous| previous >= digest) {
            return Err(CodecError::Corrupt("unsorted or duplicate source digest"));
        }
        prior = Some(digest);
        let encoded_offset = usize_from_u64(u64_at(bytes, start + 32)?)?;
        let len = u32_at(bytes, start + 40)? as usize;
        if encoded_offset != offset {
            return Err(CodecError::Corrupt("noncontiguous source offset"));
        }
        if len > MAX_SOURCE_BYTES {
            return Err(CodecError::Limit("source exceeds 8 MiB"));
        }
        let end = add(offset, len, "source offset overflow")?;
        let body = payload
            .get(offset..end)
            .ok_or(CodecError::Corrupt("source bounds"))?;
        let observed: [u8; 32] = Sha256::digest(body).into();
        if observed != digest {
            return Err(CodecError::DigestMismatch);
        }
        rows.push(SourceRow {
            digest,
            offset,
            len,
        });
        offset = end;
    }
    if offset != payload_len {
        return Err(CodecError::Corrupt("unused source payload"));
    }
    Ok(SourcePackView { payload, rows })
}

pub(crate) fn encode_posting_block(
    surface: PostingSurface,
    postings: &[PostingInput<'_>],
    limits: &CodecLimits,
) -> Result<Vec<u8>, CodecError> {
    limits.validate()?;
    if postings.len() > limits.max_terms || postings.len() > u32::MAX as usize {
        return Err(CodecError::Limit("posting term count"));
    }
    let table_bytes = mul(postings.len(), POSTING_ROW, "posting table overflow")?;
    let mut total = add(POSTING_HEADER, table_bytes, "posting header overflow")?;
    let mut memberships = 0_u64;
    let mut prior_gram = None;
    for posting in postings {
        if prior_gram.is_some_and(|previous| previous >= posting.gram) {
            return Err(CodecError::Invalid("posting grams must be strictly sorted"));
        }
        prior_gram = Some(posting.gram);
        if posting.source_ids.is_empty() || posting.source_ids.len() > u32::MAX as usize {
            return Err(CodecError::Invalid(
                "posting IDs must be nonempty and bounded",
            ));
        }
        if posting.source_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(CodecError::Invalid("posting IDs must be strictly sorted"));
        }
        memberships = memberships
            .checked_add(posting.source_ids.len() as u64)
            .ok_or(CodecError::Limit("membership count overflow"))?;
        if memberships > limits.max_memberships {
            return Err(CodecError::Limit("posting memberships"));
        }
        total = add(
            total,
            mul(posting.source_ids.len(), 8, "posting bytes overflow")?,
            "posting size overflow",
        )?;
        if total > limits.max_posting_block_encoded_bytes {
            return Err(CodecError::Limit("posting block encoded bytes"));
        }
    }
    let payload_bytes = mul(usize_from_u64(memberships)?, 8, "posting payload overflow")?;
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_| CodecError::Limit("posting allocation"))?;
    out.extend_from_slice(POSTING_MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.push(surface.wire());
    out.push(0);
    out.extend_from_slice(&(postings.len() as u32).to_le_bytes());
    out.extend_from_slice(&memberships.to_le_bytes());
    out.extend_from_slice(
        &u64::try_from(payload_bytes)
            .map_err(|_| CodecError::Limit("posting payload length"))?
            .to_le_bytes(),
    );
    let mut offset = 0_u64;
    for posting in postings {
        out.extend_from_slice(&posting.gram);
        out.push(0);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(posting.source_ids.len() as u32).to_le_bytes());
        offset = offset
            .checked_add((posting.source_ids.len() as u64) * 8)
            .ok_or(CodecError::Limit("posting offset"))?;
    }
    for posting in postings {
        for id in posting.source_ids {
            out.extend_from_slice(&id.to_le_bytes());
        }
    }
    Ok(out)
}

pub(crate) fn decode_posting_block<'a>(
    bytes: &'a [u8],
    expected_surface: PostingSurface,
    limits: &CodecLimits,
) -> Result<PostingBlockView<'a>, CodecError> {
    decode_posting_block_inner(bytes, expected_surface, limits, || Ok(()))
}

fn decode_posting_block_inner<'a, F>(
    bytes: &'a [u8],
    expected_surface: PostingSurface,
    limits: &CodecLimits,
    mut checkpoint: F,
) -> Result<PostingBlockView<'a>, CodecError>
where
    F: FnMut() -> Result<(), CodecError>,
{
    limits.validate()?;
    checkpoint()?;
    if bytes.len() > limits.max_posting_block_encoded_bytes {
        return Err(CodecError::Limit("posting block encoded bytes"));
    }
    if bytes.len() < POSTING_HEADER
        || bytes.get(..8) != Some(POSTING_MAGIC.as_slice())
        || u16_at(bytes, 8)? != VERSION
        || bytes.get(10) != Some(&expected_surface.wire())
        || bytes.get(11) != Some(&0)
    {
        return Err(CodecError::Corrupt("posting block header or surface"));
    }
    let count = u32_at(bytes, 12)? as usize;
    let memberships = u64_at(bytes, 16)?;
    if count > limits.max_terms || memberships > limits.max_memberships {
        return Err(CodecError::Limit("posting terms or memberships"));
    }
    let payload_len = usize_from_u64(u64_at(bytes, 24)?)?;
    if payload_len != mul(usize_from_u64(memberships)?, 8, "posting payload overflow")? {
        return Err(CodecError::Corrupt("posting payload membership size"));
    }
    let table_end = add(
        POSTING_HEADER,
        mul(count, POSTING_ROW, "posting table overflow")?,
        "posting table end",
    )?;
    if add(table_end, payload_len, "posting block length overflow")? != bytes.len() {
        return Err(CodecError::Corrupt(
            "posting block length or trailing bytes",
        ));
    }
    let payload = &bytes[table_end..];
    let mut rows = Vec::new();
    rows.try_reserve_exact(count)
        .map_err(|_| CodecError::Limit("posting table allocation"))?;
    let mut offset = 0_usize;
    let mut observed_memberships = 0_u64;
    let mut prior_gram = None;
    for index in 0..count {
        if index.is_multiple_of(64) {
            checkpoint()?;
        }
        let start = POSTING_HEADER + index * POSTING_ROW;
        let gram: [u8; 3] = bytes[start..start + 3]
            .try_into()
            .map_err(|_| CodecError::Corrupt("posting gram row"))?;
        if bytes[start + 3] != 0 || prior_gram.is_some_and(|previous| previous >= gram) {
            return Err(CodecError::Corrupt("posting gram order or reserved byte"));
        }
        prior_gram = Some(gram);
        let encoded_offset = usize_from_u64(u64_at(bytes, start + 4)?)?;
        let ids = u32_at(bytes, start + 12)? as usize;
        if ids == 0 || encoded_offset != offset {
            return Err(CodecError::Corrupt("posting count or offset"));
        }
        observed_memberships = observed_memberships
            .checked_add(ids as u64)
            .ok_or(CodecError::Corrupt("membership count overflow"))?;
        let end = add(
            offset,
            mul(ids, 8, "posting range overflow")?,
            "posting range end",
        )?;
        let encoded_ids = payload
            .get(offset..end)
            .ok_or(CodecError::Corrupt("posting bounds"))?;
        let mut prior_id = None;
        for (position, chunk) in encoded_ids.chunks_exact(8).enumerate() {
            if position.is_multiple_of(1024) {
                checkpoint()?;
            }
            let id = u64::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| CodecError::Corrupt("posting ID width"))?,
            );
            if prior_id.is_some_and(|previous| previous >= id) {
                return Err(CodecError::Corrupt("unsorted or duplicate posting ID"));
            }
            prior_id = Some(id);
        }
        rows.push(PostingRow {
            gram,
            offset,
            count: ids,
        });
        offset = end;
    }
    if observed_memberships != memberships || offset != payload_len {
        return Err(CodecError::Corrupt(
            "posting membership count or unused payload",
        ));
    }
    Ok(PostingBlockView {
        payload,
        payload_offset: table_end,
        rows,
        membership_count: memberships,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CodecError, CodecLimits, PostingInput, PostingSurface, SourcePackInput,
        decode_posting_block, decode_source_pack, encode_posting_block, encode_source_pack,
    };

    fn limits() -> CodecLimits {
        CodecLimits {
            max_source_pack_encoded_bytes: 4096,
            max_posting_block_encoded_bytes: 4096,
            max_sources: 4,
            max_terms: 4,
            max_memberships: 8,
        }
    }

    fn abc_digest() -> [u8; 32] {
        // Independent SHA-256 of literal b"abc".
        [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ]
    }

    fn source_fixture() -> Vec<u8> {
        let mut fixed = Vec::new();
        fixed.extend_from_slice(b"QISPACK1");
        fixed.extend_from_slice(&1_u16.to_le_bytes());
        fixed.extend_from_slice(&0_u16.to_le_bytes());
        fixed.extend_from_slice(&1_u32.to_le_bytes());
        fixed.extend_from_slice(&3_u64.to_le_bytes());
        fixed.extend_from_slice(&abc_digest());
        fixed.extend_from_slice(&0_u64.to_le_bytes());
        fixed.extend_from_slice(&3_u32.to_le_bytes());
        fixed.extend_from_slice(b"abc");
        fixed
    }

    fn posting_fixture() -> Vec<u8> {
        let mut fixed = Vec::new();
        fixed.extend_from_slice(b"QIPOST01");
        fixed.extend_from_slice(&1_u16.to_le_bytes());
        fixed.push(1); // Path surface.
        fixed.push(0); // Reserved.
        fixed.extend_from_slice(&1_u32.to_le_bytes());
        fixed.extend_from_slice(&2_u64.to_le_bytes());
        fixed.extend_from_slice(&16_u64.to_le_bytes());
        fixed.extend_from_slice(b"abc");
        fixed.push(0); // Reserved.
        fixed.extend_from_slice(&0_u64.to_le_bytes());
        fixed.extend_from_slice(&2_u32.to_le_bytes());
        fixed.extend_from_slice(&7_u64.to_le_bytes());
        fixed.extend_from_slice(&9_u64.to_le_bytes());
        fixed
    }

    #[test]
    fn fixed_source_pack_wire_and_full_iteration() {
        let fixed = source_fixture();
        let encoded = encode_source_pack(
            &[SourcePackInput {
                digest: abc_digest(),
                bytes: b"abc",
            }],
            &limits(),
        )
        .unwrap();
        assert_eq!(encoded, fixed);
        let decoded = decode_source_pack(&fixed, &limits()).unwrap();
        assert_eq!(decoded.get(&abc_digest()), Some(b"abc".as_slice()));
        assert_eq!(
            decoded.entries().collect::<Vec<_>>(),
            vec![(abc_digest(), b"abc".as_slice())]
        );
    }

    #[test]
    fn source_pack_refuses_corruption_and_limits() {
        let fixed = source_fixture();
        let mut trailing = fixed.clone();
        trailing.push(0);
        assert!(matches!(
            decode_source_pack(&trailing, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut reserved = fixed.clone();
        reserved[10] = 1;
        assert!(matches!(
            decode_source_pack(&reserved, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut offset = fixed.clone();
        offset[24 + 32] = 1;
        assert!(matches!(
            decode_source_pack(&offset, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut body = fixed.clone();
        *body.last_mut().unwrap() = b'd';
        assert_eq!(
            decode_source_pack(&body, &limits()).err(),
            Some(CodecError::DigestMismatch)
        );
        let tight = CodecLimits {
            max_source_pack_encoded_bytes: fixed.len() - 1,
            ..limits()
        };
        assert!(matches!(
            decode_source_pack(&fixed, &tight),
            Err(CodecError::Limit(_))
        ));
        let mut huge_count = fixed;
        huge_count[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decode_source_pack(&huge_count, &limits()),
            Err(CodecError::Limit(_))
        ));
    }

    #[test]
    fn fixed_posting_wire_query_slice_and_full_iteration() {
        let fixed = posting_fixture();
        let encoded = encode_posting_block(
            PostingSurface::Path,
            &[PostingInput {
                gram: *b"abc",
                source_ids: &[7, 9],
            }],
            &limits(),
        )
        .unwrap();
        assert_eq!(encoded, fixed);
        let decoded = decode_posting_block(&fixed, PostingSurface::Path, &limits()).unwrap();
        assert_eq!(decoded.membership_count(), 2);
        assert_eq!(
            decoded.lookup(*b"abc").unwrap().collect::<Vec<_>>(),
            vec![7, 9]
        );
        assert!(decoded.lookup(*b"def").is_none());
        assert_eq!(
            decoded
                .iter_terms()
                .map(|(gram, count, ids)| (gram, count, ids.collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
            vec![(*b"abc", 2, vec![7, 9])]
        );
    }

    #[test]
    fn posting_refuses_surface_count_order_offset_and_trailing_mutants() {
        let fixed = posting_fixture();
        assert!(matches!(
            decode_posting_block(&fixed, PostingSurface::Content, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut duplicate_id = fixed.clone();
        duplicate_id[56..64].copy_from_slice(&7_u64.to_le_bytes());
        assert!(matches!(
            decode_posting_block(&duplicate_id, PostingSurface::Path, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut offset = fixed.clone();
        offset[32 + 4] = 1;
        assert!(matches!(
            decode_posting_block(&offset, PostingSurface::Path, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut count = fixed.clone();
        count[16..24].copy_from_slice(&3_u64.to_le_bytes());
        assert!(matches!(
            decode_posting_block(&count, PostingSurface::Path, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut trailing = fixed.clone();
        trailing.push(0);
        assert!(matches!(
            decode_posting_block(&trailing, PostingSurface::Path, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let tight = CodecLimits {
            max_memberships: 1,
            ..limits()
        };
        assert!(matches!(
            decode_posting_block(&fixed, PostingSurface::Path, &tight),
            Err(CodecError::Limit(_))
        ));
    }

    #[test]
    fn producer_refuses_noncanonical_and_unverified_inputs() {
        assert!(matches!(
            encode_source_pack(
                &[SourcePackInput {
                    digest: [0; 32],
                    bytes: b"abc",
                }],
                &limits()
            ),
            Err(CodecError::Invalid(_))
        ));
        assert!(matches!(
            encode_posting_block(
                PostingSurface::Path,
                &[PostingInput {
                    gram: *b"abc",
                    source_ids: &[9, 7]
                }],
                &limits()
            ),
            Err(CodecError::Invalid(_))
        ));
        assert!(matches!(
            encode_posting_block(
                PostingSurface::Path,
                &[PostingInput {
                    gram: *b"abc",
                    source_ids: &[7, 7]
                }],
                &limits()
            ),
            Err(CodecError::Invalid(_))
        ));
    }
}
