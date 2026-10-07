//! Bounded, canonical wire formats for the immutable file authority.
//!
//! The caller owns root identity, shard membership, stable source IDs, and the
//! aggregate budget. This codec checks only the exact bytes of one pack/block.

use sha2::{Digest as _, Sha256};

const VERSION: u16 = 1;
const POSTING_VERSION: u16 = 2;
const SOURCE_MAGIC: &[u8; 8] = b"QISPACK1";
const POSTING_MAGIC: &[u8; 8] = b"QIPOST02";
const SOURCE_HEADER: usize = 24;
const SOURCE_ROW: usize = 44;
const POSTING_HEADER: usize = 32;
const POSTING_ROW: usize = 48;
pub(super) const DIRECTORY_PAGE_TERMS: u16 = 128;
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
    pub(crate) source_pack_encoded_bytes: usize,
    pub(crate) posting_block_encoded_bytes: usize,
    pub(crate) sources: usize,
    pub(crate) terms: usize,
    pub(crate) memberships: u64,
}

impl CodecLimits {
    fn validate(self) -> Result<(), CodecError> {
        if self.source_pack_encoded_bytes < SOURCE_HEADER
            || self.posting_block_encoded_bytes < POSTING_HEADER
            || self.sources == 0
            || self.terms == 0
            || self.memberships == 0
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
struct SourceRow<'a> {
    digest: [u8; 32],
    body: &'a [u8],
}

pub(crate) struct SourcePackView<'a> {
    rows: Vec<SourceRow<'a>>,
}

impl<'a> SourcePackView<'a> {
    pub(crate) fn get(&self, digest: &[u8; 32]) -> Option<&'a [u8]> {
        let Ok(index) = self.rows.binary_search_by_key(digest, |row| row.digest) else {
            return None;
        };
        self.rows.get(index).map(|row| row.body)
    }

    pub(crate) fn entries(&self) -> impl ExactSizeIterator<Item = ([u8; 32], &'a [u8])> + '_ {
        self.rows.iter().map(|row| (row.digest, row.body))
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

/// Exact wire length shared by the producer's pre-allocation admission and
/// the encoder. The caller still validates gram and ID ordering.
pub(crate) fn posting_block_encoded_len(
    terms: usize,
    memberships: u64,
    limits: &CodecLimits,
) -> Result<usize, CodecError> {
    limits.validate()?;
    if terms > limits.terms || u32::try_from(terms).is_err() {
        return Err(CodecError::Limit("posting term count"));
    }
    if memberships > limits.memberships {
        return Err(CodecError::Limit("posting memberships"));
    }
    let table_bytes = mul(terms, POSTING_ROW, "posting table overflow")?;
    let payload_bytes = mul(usize_from_u64(memberships)?, 8, "posting payload overflow")?;
    let total = add(
        add(POSTING_HEADER, table_bytes, "posting header overflow")?,
        payload_bytes,
        "posting size overflow",
    )?;
    if total > limits.posting_block_encoded_bytes {
        return Err(CodecError::Limit("posting block encoded bytes"));
    }
    Ok(total)
}

#[derive(Clone, Copy, Debug)]
struct PostingRow<'a> {
    gram: [u8; 3],
    count: usize,
    bytes: &'a [u8],
}

pub(crate) struct PostingBlockView<'a> {
    encoded: &'a [u8],
    rows: Vec<PostingRow<'a>>,
    membership_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PostingPageDescriptor {
    pub(crate) first: [u8; 3],
    pub(crate) last: [u8; 3],
    pub(crate) offset: u64,
    pub(crate) terms: u32,
    pub(crate) sha256: [u8; 32],
}

/// One authenticated, fixed-size directory page. Posting lists remain separate
/// range reads; their committed hashes are inside the authenticated table.
pub(crate) struct PostingPageView<'a> {
    table: &'a [u8],
    payload_offset: u64,
    object_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PostingListDescriptor {
    pub(crate) offset: u64,
    pub(crate) count: u32,
    pub(crate) sha256: [u8; 32],
}

struct PostingPageRow {
    gram: [u8; 3],
    list: PostingListDescriptor,
}

impl PostingPageDescriptor {
    pub(crate) fn encoded_bytes(self) -> Result<u64, CodecError> {
        u64::from(self.terms)
            .checked_mul(
                u64::try_from(POSTING_ROW)
                    .map_err(|_error| CodecError::Corrupt("page row length width"))?,
            )
            .ok_or(CodecError::Corrupt("page encoded length overflow"))
    }

    pub(crate) fn decode<'a>(
        &self,
        table: &'a [u8],
        partition_terms: u32,
        object_bytes: u64,
    ) -> Result<PostingPageView<'a>, CodecError> {
        if self.terms == 0
            || self.terms > u32::from(DIRECTORY_PAGE_TERMS)
            || u64::try_from(table.len())
                .map_err(|_error| CodecError::Corrupt("page length width"))?
                != self.encoded_bytes()?
        {
            return Err(CodecError::Corrupt("posting directory page size"));
        }
        let payload_offset = u64::try_from(add(
            POSTING_HEADER,
            mul(
                usize::try_from(partition_terms)
                    .map_err(|_error| CodecError::Corrupt("partition term count width"))?,
                POSTING_ROW,
                "page table length overflow",
            )?,
            "page payload offset overflow",
        )?)
        .map_err(|_error| CodecError::Corrupt("page payload offset width"))?;
        let view = PostingPageView {
            table,
            payload_offset,
            object_bytes,
        };
        let mut previous = None;
        for index in 0..table.chunks_exact(POSTING_ROW).len() {
            let term = view.term(index)?;
            if previous.is_some_and(|gram| gram >= term.gram) {
                return Err(CodecError::Corrupt("posting page gram order"));
            }
            previous = Some(term.gram);
        }
        if view.term(0)?.gram != self.first || previous != Some(self.last) {
            return Err(CodecError::Corrupt("posting page boundary"));
        }
        Ok(view)
    }
}

impl PostingPageView<'_> {
    fn term(&self, index: usize) -> Result<PostingPageRow, CodecError> {
        let start = mul(index, POSTING_ROW, "posting page row overflow")?;
        let row = self
            .table
            .get(start..add(start, POSTING_ROW, "posting page end overflow")?)
            .ok_or(CodecError::Corrupt("posting page row missing"))?;
        let gram = row
            .get(..3)
            .ok_or(CodecError::Corrupt("posting page gram missing"))?
            .try_into()
            .map_err(|_error| CodecError::Corrupt("posting page gram"))?;
        if row.get(3) != Some(&0) {
            return Err(CodecError::Corrupt("posting page reserved byte"));
        }
        let offset = self
            .payload_offset
            .checked_add(u64_at(row, 4)?)
            .ok_or(CodecError::Corrupt("posting page list offset overflow"))?;
        let count = u32_at(row, 12)?;
        if count == 0
            || offset
                .checked_add(
                    u64::from(count)
                        .checked_mul(8)
                        .ok_or(CodecError::Corrupt("posting page list length overflow"))?,
                )
                .is_none_or(|end| end > self.object_bytes)
        {
            return Err(CodecError::Corrupt("posting page list range"));
        }
        let sha256 = row
            .get(16..48)
            .ok_or(CodecError::Corrupt("posting page list digest missing"))?
            .try_into()
            .map_err(|_error| CodecError::Corrupt("posting page list digest"))?;
        Ok(PostingPageRow {
            gram,
            list: PostingListDescriptor {
                offset,
                count,
                sha256,
            },
        })
    }

    pub(crate) fn lookup(
        &self,
        gram: [u8; 3],
    ) -> Result<Option<PostingListDescriptor>, CodecError> {
        let mut left = 0;
        let mut right = self.table.chunks_exact(POSTING_ROW).len();
        while left < right {
            let middle = left.midpoint(right);
            let row = self.term(middle)?;
            match row.gram.cmp(&gram) {
                std::cmp::Ordering::Less => left = add(middle, 1, "posting page lookup overflow")?,
                std::cmp::Ordering::Greater => right = middle,
                std::cmp::Ordering::Equal => return Ok(Some(row.list)),
            }
        }
        Ok(None)
    }
}

impl<'a> PostingBlockView<'a> {
    #[cfg(test)]
    pub(crate) fn lookup(&self, gram: [u8; 3]) -> Option<PostingIds<'a>> {
        let Ok(index) = self.rows.binary_search_by_key(&gram, |row| row.gram) else {
            return None;
        };
        let row = self.rows.get(index)?;
        Some(PostingIds { bytes: row.bytes })
    }

    pub(crate) fn iter_terms(
        &self,
    ) -> impl ExactSizeIterator<Item = ([u8; 3], usize, PostingIds<'a>)> + '_ {
        self.rows
            .iter()
            .map(|row| (row.gram, row.count, PostingIds { bytes: row.bytes }))
    }

    pub(crate) fn membership_count(&self) -> u64 {
        self.membership_count
    }

    pub(crate) fn page_descriptors(
        &self,
    ) -> impl ExactSizeIterator<Item = Result<PostingPageDescriptor, CodecError>> + '_ {
        self.rows
            .chunks(usize::from(DIRECTORY_PAGE_TERMS))
            .enumerate()
            .map(|(index, rows)| {
                let start = add(
                    POSTING_HEADER,
                    mul(
                        index,
                        mul(
                            usize::from(DIRECTORY_PAGE_TERMS),
                            POSTING_ROW,
                            "page width overflow",
                        )?,
                        "page offset overflow",
                    )?,
                    "page start overflow",
                )?;
                let end = add(
                    start,
                    mul(rows.len(), POSTING_ROW, "page size overflow")?,
                    "page end overflow",
                )?;
                let table = self
                    .encoded
                    .get(start..end)
                    .ok_or(CodecError::Corrupt("page bounds"))?;
                Ok(PostingPageDescriptor {
                    first: rows.first().ok_or(CodecError::Corrupt("empty page"))?.gram,
                    last: rows.last().ok_or(CodecError::Corrupt("empty page"))?.gram,
                    offset: u64::try_from(start)
                        .map_err(|_error| CodecError::Corrupt("page offset width"))?,
                    terms: u32::try_from(rows.len())
                        .map_err(|_error| CodecError::Corrupt("page count width"))?,
                    sha256: Sha256::digest(table).into(),
                })
            })
    }
}

#[derive(Clone, Debug)]
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
        let count = self.bytes.chunks_exact(8).len();
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
        .map_err(|_error| CodecError::Corrupt("truncated u32"))?;
    Ok(u32::from_le_bytes(word))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, CodecError> {
    let end = add(at, 2, "integer offset overflow")?;
    let word: [u8; 2] = bytes
        .get(at..end)
        .ok_or(CodecError::Corrupt("truncated u16"))?
        .try_into()
        .map_err(|_error| CodecError::Corrupt("truncated u16"))?;
    Ok(u16::from_le_bytes(word))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, CodecError> {
    let end = add(at, 8, "integer offset overflow")?;
    let word: [u8; 8] = bytes
        .get(at..end)
        .ok_or(CodecError::Corrupt("truncated u64"))?
        .try_into()
        .map_err(|_error| CodecError::Corrupt("truncated u64"))?;
    Ok(u64::from_le_bytes(word))
}

fn usize_from_u64(value: u64) -> Result<usize, CodecError> {
    usize::try_from(value).map_err(|_error| CodecError::Corrupt("encoded length exceeds usize"))
}

pub(crate) fn encode_source_pack(
    sources: &[SourcePackInput<'_>],
    limits: &CodecLimits,
) -> Result<Vec<u8>, CodecError> {
    limits.validate()?;
    if sources.len() > limits.sources {
        return Err(CodecError::Limit("source count"));
    }
    let source_count =
        u32::try_from(sources.len()).map_err(|_error| CodecError::Limit("source count"))?;
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
        if source.bytes.len() > MAX_SOURCE_BYTES {
            return Err(CodecError::Limit("source exceeds 8 MiB"));
        }
        let observed: [u8; 32] = Sha256::digest(source.bytes).into();
        if observed != source.digest {
            return Err(CodecError::Invalid("source bytes differ from digest"));
        }
        payload_bytes = add(payload_bytes, source.bytes.len(), "source payload overflow")?;
        total = add(total, source.bytes.len(), "source pack size overflow")?;
        if total > limits.source_pack_encoded_bytes {
            return Err(CodecError::Limit("source pack encoded bytes"));
        }
    }
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_error| CodecError::Limit("source pack allocation"))?;
    out.extend_from_slice(SOURCE_MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&source_count.to_le_bytes());
    out.extend_from_slice(
        &u64::try_from(payload_bytes)
            .map_err(|_error| CodecError::Limit("source payload length"))?
            .to_le_bytes(),
    );
    let mut offset = 0_u64;
    for source in sources {
        out.extend_from_slice(&source.digest);
        out.extend_from_slice(&offset.to_le_bytes());
        let source_len = u32::try_from(source.bytes.len())
            .map_err(|_error| CodecError::Limit("source exceeds 8 MiB"))?;
        out.extend_from_slice(&source_len.to_le_bytes());
        offset = offset
            .checked_add(u64::from(source_len))
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
    if bytes.len() > limits.source_pack_encoded_bytes {
        return Err(CodecError::Limit("source pack encoded bytes"));
    }
    if bytes.len() < SOURCE_HEADER
        || bytes.get(..8) != Some(SOURCE_MAGIC.as_slice())
        || u16_at(bytes, 8)? != VERSION
        || u16_at(bytes, 10)? != 0
    {
        return Err(CodecError::Corrupt("source pack header"));
    }
    let count =
        usize::try_from(u32_at(bytes, 12)?).map_err(|_error| CodecError::Limit("source count"))?;
    if count > limits.sources {
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
    let payload = bytes
        .get(table_end..)
        .ok_or(CodecError::Corrupt("source pack payload bounds"))?;
    let mut rows = Vec::new();
    rows.try_reserve_exact(count)
        .map_err(|_error| CodecError::Limit("source table allocation"))?;
    let mut offset = 0_usize;
    let mut prior = None;
    for index in 0..count {
        let start = add(
            SOURCE_HEADER,
            mul(index, SOURCE_ROW, "source row offset overflow")?,
            "source row start overflow",
        )?;
        let digest_end = add(start, 32, "source digest end overflow")?;
        let digest: [u8; 32] = bytes
            .get(start..digest_end)
            .ok_or(CodecError::Corrupt("source digest row"))?
            .try_into()
            .map_err(|_error| CodecError::Corrupt("source digest row"))?;
        if prior.is_some_and(|previous| previous >= digest) {
            return Err(CodecError::Corrupt("unsorted or duplicate source digest"));
        }
        prior = Some(digest);
        let encoded_offset = usize_from_u64(u64_at(bytes, digest_end)?)?;
        let len_at = add(start, 40, "source length offset overflow")?;
        let len = usize::try_from(u32_at(bytes, len_at)?)
            .map_err(|_error| CodecError::Corrupt("source length width"))?;
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
        rows.push(SourceRow { digest, body });
        offset = end;
    }
    if offset != payload_len {
        return Err(CodecError::Corrupt("unused source payload"));
    }
    Ok(SourcePackView { rows })
}

pub(crate) fn encode_posting_block(
    surface: PostingSurface,
    postings: &[PostingInput<'_>],
    limits: &CodecLimits,
) -> Result<Vec<u8>, CodecError> {
    limits.validate()?;
    if postings.len() > limits.terms {
        return Err(CodecError::Limit("posting term count"));
    }
    let posting_count =
        u32::try_from(postings.len()).map_err(|_error| CodecError::Limit("posting term count"))?;
    let mut memberships = 0_u64;
    let mut prior_gram = None;
    for posting in postings {
        if prior_gram.is_some_and(|previous| previous >= posting.gram) {
            return Err(CodecError::Invalid("posting grams must be strictly sorted"));
        }
        prior_gram = Some(posting.gram);
        if posting.source_ids.is_empty() || u32::try_from(posting.source_ids.len()).is_err() {
            return Err(CodecError::Invalid(
                "posting IDs must be nonempty and bounded",
            ));
        }
        if posting
            .source_ids
            .iter()
            .zip(posting.source_ids.iter().skip(1))
            .any(|(left, right)| left >= right)
        {
            return Err(CodecError::Invalid("posting IDs must be strictly sorted"));
        }
        let posting_memberships = u64::try_from(posting.source_ids.len())
            .map_err(|_error| CodecError::Limit("membership count overflow"))?;
        memberships = memberships
            .checked_add(posting_memberships)
            .ok_or(CodecError::Limit("membership count overflow"))?;
        if memberships > limits.memberships {
            return Err(CodecError::Limit("posting memberships"));
        }
        // Preserve the original prefix-by-prefix limit check, using the same
        // exact length calculation that producer admission uses.
        let _prefix_len = posting_block_encoded_len(postings.len(), memberships, limits)?;
    }
    let total = posting_block_encoded_len(postings.len(), memberships, limits)?;
    let payload_bytes = mul(usize_from_u64(memberships)?, 8, "posting payload overflow")?;
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_error| CodecError::Limit("posting allocation"))?;
    out.extend_from_slice(POSTING_MAGIC);
    out.extend_from_slice(&POSTING_VERSION.to_le_bytes());
    out.push(surface.wire());
    out.push(0);
    out.extend_from_slice(&posting_count.to_le_bytes());
    out.extend_from_slice(&memberships.to_le_bytes());
    out.extend_from_slice(
        &u64::try_from(payload_bytes)
            .map_err(|_error| CodecError::Limit("posting payload length"))?
            .to_le_bytes(),
    );
    let mut offset = 0_u64;
    for posting in postings {
        out.extend_from_slice(&posting.gram);
        out.push(0);
        out.extend_from_slice(&offset.to_le_bytes());
        let id_count = u32::try_from(posting.source_ids.len())
            .map_err(|_error| CodecError::Invalid("posting IDs must be nonempty and bounded"))?;
        out.extend_from_slice(&id_count.to_le_bytes());
        let mut list_hasher = Sha256::new();
        for id in posting.source_ids {
            list_hasher.update(id.to_le_bytes());
        }
        out.extend_from_slice(&list_hasher.finalize());
        offset = offset
            .checked_add(
                u64::from(id_count)
                    .checked_mul(8)
                    .ok_or(CodecError::Limit("posting offset"))?,
            )
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
    if bytes.len() > limits.posting_block_encoded_bytes {
        return Err(CodecError::Limit("posting block encoded bytes"));
    }
    if bytes.len() < POSTING_HEADER
        || bytes.get(..8) != Some(POSTING_MAGIC.as_slice())
        || u16_at(bytes, 8)? != POSTING_VERSION
        || bytes.get(10) != Some(&expected_surface.wire())
        || bytes.get(11) != Some(&0)
    {
        return Err(CodecError::Corrupt("posting block header or surface"));
    }
    let count = usize::try_from(u32_at(bytes, 12)?)
        .map_err(|_error| CodecError::Limit("posting term count"))?;
    let memberships = u64_at(bytes, 16)?;
    if count > limits.terms || memberships > limits.memberships {
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
    let payload = bytes
        .get(table_end..)
        .ok_or(CodecError::Corrupt("posting payload bounds"))?;
    let mut rows = Vec::new();
    rows.try_reserve_exact(count)
        .map_err(|_error| CodecError::Limit("posting table allocation"))?;
    let mut offset = 0_usize;
    let mut observed_memberships = 0_u64;
    let mut prior_gram = None;
    for index in 0..count {
        if index.is_multiple_of(64) {
            checkpoint()?;
        }
        let start = add(
            POSTING_HEADER,
            mul(index, POSTING_ROW, "posting row offset overflow")?,
            "posting row start overflow",
        )?;
        let gram_end = add(start, 3, "posting gram end overflow")?;
        let gram: [u8; 3] = bytes
            .get(start..gram_end)
            .ok_or(CodecError::Corrupt("posting gram row"))?
            .try_into()
            .map_err(|_error| CodecError::Corrupt("posting gram row"))?;
        if bytes.get(gram_end) != Some(&0) || prior_gram.is_some_and(|previous| previous >= gram) {
            return Err(CodecError::Corrupt("posting gram order or reserved byte"));
        }
        prior_gram = Some(gram);
        let offset_at = add(start, 4, "posting offset position overflow")?;
        let count_at = add(start, 12, "posting count position overflow")?;
        let encoded_offset = usize_from_u64(u64_at(bytes, offset_at)?)?;
        let ids = usize::try_from(u32_at(bytes, count_at)?)
            .map_err(|_error| CodecError::Corrupt("posting ID count width"))?;
        if ids == 0 || encoded_offset != offset {
            return Err(CodecError::Corrupt("posting count or offset"));
        }
        observed_memberships = observed_memberships
            .checked_add(
                u64::try_from(ids)
                    .map_err(|_error| CodecError::Corrupt("membership count width"))?,
            )
            .ok_or(CodecError::Corrupt("membership count overflow"))?;
        let end = add(
            offset,
            mul(ids, 8, "posting range overflow")?,
            "posting range end",
        )?;
        let encoded_ids = payload
            .get(offset..end)
            .ok_or(CodecError::Corrupt("posting bounds"))?;
        let digest_at = add(start, 16, "posting digest offset")?;
        let digest_end = add(digest_at, 32, "posting digest end")?;
        let expected_digest = bytes
            .get(digest_at..digest_end)
            .ok_or(CodecError::Corrupt("posting list digest missing"))?;
        let mut list_hasher = Sha256::new();
        for chunk in encoded_ids.chunks(64 * 1024) {
            checkpoint()?;
            list_hasher.update(chunk);
        }
        if list_hasher.finalize().as_slice() != expected_digest {
            return Err(CodecError::DigestMismatch);
        }
        let mut prior_id = None;
        for (position, chunk) in encoded_ids.chunks_exact(8).enumerate() {
            if position.is_multiple_of(1024) {
                checkpoint()?;
            }
            let id = u64::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_error| CodecError::Corrupt("posting ID width"))?,
            );
            if prior_id.is_some_and(|previous| previous >= id) {
                return Err(CodecError::Corrupt("unsorted or duplicate posting ID"));
            }
            prior_id = Some(id);
        }
        rows.push(PostingRow {
            gram,
            count: ids,
            bytes: encoded_ids,
        });
        offset = end;
    }
    if observed_memberships != memberships || offset != payload_len {
        return Err(CodecError::Corrupt(
            "posting membership count or unused payload",
        ));
    }
    Ok(PostingBlockView {
        encoded: bytes,
        rows,
        membership_count: memberships,
    })
}

#[cfg(test)]
mod tests {
    use sha2::Digest as _;

    use super::{
        CodecError, CodecLimits, PostingInput, PostingSurface, SourcePackInput,
        decode_posting_block, decode_source_pack, encode_posting_block, encode_source_pack,
    };

    fn limits() -> CodecLimits {
        CodecLimits {
            source_pack_encoded_bytes: 4096,
            posting_block_encoded_bytes: 4096,
            sources: 4,
            terms: 4,
            memberships: 8,
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
        fixed.extend_from_slice(b"QIPOST02");
        fixed.extend_from_slice(&2_u16.to_le_bytes());
        fixed.push(1); // Path surface.
        fixed.push(0); // Reserved.
        fixed.extend_from_slice(&1_u32.to_le_bytes());
        fixed.extend_from_slice(&2_u64.to_le_bytes());
        fixed.extend_from_slice(&16_u64.to_le_bytes());
        fixed.extend_from_slice(b"abc");
        fixed.push(0); // Reserved.
        fixed.extend_from_slice(&0_u64.to_le_bytes());
        fixed.extend_from_slice(&2_u32.to_le_bytes());
        // Independent SHA-256 of the little-endian u64 pair [7, 9].
        fixed.extend_from_slice(&[
            0x74, 0x01, 0x63, 0x92, 0xe5, 0x73, 0x38, 0x10, 0x4f, 0xe7, 0xfc, 0x15, 0x67, 0xb4,
            0x2e, 0xd5, 0x63, 0xc8, 0xfa, 0xc1, 0x6f, 0xa8, 0x07, 0x08, 0xb6, 0xbc, 0xca, 0xec,
            0x06, 0x2e, 0xd2, 0xd3,
        ]);
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
        *reserved.get_mut(10).unwrap() = 1;
        assert!(matches!(
            decode_source_pack(&reserved, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut offset = fixed.clone();
        *offset.get_mut(56).unwrap() = 1;
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
            source_pack_encoded_bytes: fixed.len() - 1,
            ..limits()
        };
        assert!(matches!(
            decode_source_pack(&fixed, &tight),
            Err(CodecError::Limit(_))
        ));
        let mut huge_count = fixed;
        huge_count
            .get_mut(12..16)
            .unwrap()
            .copy_from_slice(&u32::MAX.to_le_bytes());
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
        duplicate_id
            .get_mut(88..96)
            .unwrap()
            .copy_from_slice(&7_u64.to_le_bytes());
        assert!(matches!(
            decode_posting_block(&duplicate_id, PostingSurface::Path, &limits()),
            Err(CodecError::DigestMismatch)
        ));
        let mut offset = fixed.clone();
        *offset.get_mut(36).unwrap() = 1;
        assert!(matches!(
            decode_posting_block(&offset, PostingSurface::Path, &limits()),
            Err(CodecError::Corrupt(_))
        ));
        let mut count = fixed.clone();
        count
            .get_mut(16..24)
            .unwrap()
            .copy_from_slice(&3_u64.to_le_bytes());
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
            memberships: 1,
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

    #[test]
    fn authenticated_pages_preserve_cross_page_list_offsets_and_digests() {
        let ids: Vec<[u64; 1]> = (1..=260).map(|id| [id]).collect();
        let postings: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(index, ids)| PostingInput {
                gram: [
                    b'a',
                    u8::try_from(index.checked_div(256).expect("byte width")).expect("high byte"),
                    u8::try_from(index.checked_rem(256).expect("byte width")).expect("low byte"),
                ],
                source_ids: ids,
            })
            .collect();
        let large = CodecLimits {
            posting_block_encoded_bytes: 16_384,
            terms: 260,
            memberships: 260,
            ..limits()
        };
        let encoded =
            encode_posting_block(PostingSurface::Content, &postings, &large).expect("encode");
        let view = decode_posting_block(&encoded, PostingSurface::Content, &large).expect("decode");
        let pages: Vec<_> = view
            .page_descriptors()
            .map(|page| page.expect("page"))
            .collect();
        assert_eq!(
            pages.iter().map(|page| page.terms).collect::<Vec<_>>(),
            [128, 128, 4]
        );
        assert_eq!(pages.get(1).expect("second page").offset, 6176);
        for index in [0_usize, 127, 128, 255, 256, 259] {
            let page = pages
                .get(index.checked_div(128).expect("page width"))
                .expect("page");
            let start = usize::try_from(page.offset).expect("offset");
            let end = start
                .checked_add(
                    usize::try_from(page.encoded_bytes().expect("page bytes"))
                        .expect("page length"),
                )
                .expect("page end");
            let table = encoded.get(start..end).expect("page bytes");
            assert_eq!(<[u8; 32]>::from(sha2::Sha256::digest(table)), page.sha256);
            let page_view = page
                .decode(table, 260, u64::try_from(encoded.len()).expect("length"))
                .expect("page decode");
            let list_descriptor = page_view
                .lookup(postings.get(index).expect("posting").gram)
                .expect("lookup")
                .expect("found");
            assert_eq!(list_descriptor.count, 1);
            let start = usize::try_from(list_descriptor.offset).expect("offset");
            let list = encoded
                .get(start..start.checked_add(8).expect("ID end"))
                .expect("ID bytes");
            assert_eq!(
                u64::from_le_bytes(list.try_into().expect("ID bytes")),
                u64::try_from(index.checked_add(1).expect("next ID")).expect("ID")
            );
            assert_eq!(
                <[u8; 32]>::from(sha2::Sha256::digest(list)),
                list_descriptor.sha256
            );
        }
        let mut old = encoded;
        *old.get_mut(7).expect("magic suffix") = b'1';
        old.get_mut(8..10)
            .expect("version")
            .copy_from_slice(&1_u16.to_le_bytes());
        assert!(matches!(
            decode_posting_block(&old, PostingSurface::Content, &large),
            Err(CodecError::Corrupt(_))
        ));
    }
}
