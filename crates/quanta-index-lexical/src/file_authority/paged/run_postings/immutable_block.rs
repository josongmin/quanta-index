//! Immutable run-posting block for a directory keyed by each block's last key.
//!
//! The caller stores these bytes as an authenticated Record blob and binds its
//! reference in the run root. This codec proves the dictionary and extents;
//! point lookup decodes only the selected page. The independent seal must walk
//! every page and compare it with the leaf oracle before publication.

use quanta_index_core::CoreError;

use super::{RunPostingPage, corrupt, invalid};

const MAGIC: [u8; 4] = *b"QIRB";
const VERSION: u8 = 2;
const HEADER_BYTES: usize = 15;
const ENTRY_BYTES: usize = 20;
pub(super) const MAX_BLOCK_BYTES: usize = super::MAX_POSTING_PAGE_BYTES;
// Worst canonical v2 page: six-item CBOR header, surface/gram/bucket,
// 256 distinct 32-byte digests encoded as u8 arrays, and 256 (u64,u8)
// posting rows. The wire maximum is 19,993 bytes; one entry adds 35 bytes.
const MAX_V2_PAGE_WIRE_BYTES: usize = 19_993;
const _: () = assert!(MAX_V2_PAGE_WIRE_BYTES + HEADER_BYTES + ENTRY_BYTES <= MAX_BLOCK_BYTES);

#[derive(Clone, Copy)]
struct Entry {
    key: [u8; 12],
    offset: usize,
    length: usize,
}

pub(super) enum PushResult {
    Added,
    Full,
}

/// Stages each already encoded page once. `Full` leaves both the builder and
/// the caller's page unchanged, so the caller can finish and start a new block.
pub(super) struct ImmutableRunBlockBuilder {
    entries: Vec<Entry>,
    payload: Vec<u8>,
}

impl ImmutableRunBlockBuilder {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            payload: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn try_push(
        &mut self,
        key: [u8; 12],
        encoded_page: &[u8],
    ) -> Result<PushResult, CoreError> {
        let page = RunPostingPage::decode(encoded_page)?;
        if page.key()? != key {
            return Err(invalid("immutable block input key differs from page"));
        }
        self.try_push_canonical(key, encoded_page)
    }

    /// The caller must have minted this key and canonical encoding from the
    /// same validated page, or authenticated and checked an inherited page.
    pub(super) fn try_push_canonical(
        &mut self,
        key: [u8; 12],
        encoded_page: &[u8],
    ) -> Result<PushResult, CoreError> {
        if self.entries.last().is_some_and(|prior| prior.key >= key) {
            return Err(invalid(
                "immutable block input keys are not strictly sorted",
            ));
        }
        let next_count = self
            .entries
            .len()
            .checked_add(1)
            .ok_or_else(|| invalid("immutable block entry count overflows"))?;
        let index_bytes = next_count
            .checked_mul(ENTRY_BYTES)
            .ok_or_else(|| invalid("immutable block index width overflows"))?;
        let next_payload = self
            .payload
            .len()
            .checked_add(encoded_page.len())
            .ok_or_else(|| invalid("immutable block payload width overflows"))?;
        let next_total = HEADER_BYTES
            .checked_add(index_bytes)
            .and_then(|width| width.checked_add(next_payload))
            .ok_or_else(|| invalid("immutable block width overflows"))?;
        if next_count > usize::from(u16::MAX) || next_total > MAX_BLOCK_BYTES {
            if self.entries.is_empty() {
                return Err(invalid("one immutable page exceeds block admission"));
            }
            return Ok(PushResult::Full);
        }
        self.entries.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!("reserve immutable block entry: {error}"))
        })?;
        self.payload
            .try_reserve(encoded_page.len())
            .map_err(|error| {
                CoreError::Storage(format!("reserve immutable block page: {error}"))
            })?;
        self.entries.push(Entry {
            key,
            offset: self.payload.len(),
            length: encoded_page.len(),
        });
        self.payload.extend_from_slice(encoded_page);
        Ok(PushResult::Added)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(super) fn first_key(&self) -> Option<[u8; 12]> {
        self.entries.first().map(|entry| entry.key)
    }

    pub(super) fn last_key(&self) -> Option<[u8; 12]> {
        self.entries.last().map(|entry| entry.key)
    }

    pub(super) fn finish(self) -> Result<Vec<u8>, CoreError> {
        if self.entries.is_empty() {
            return Err(invalid("immutable block is empty"));
        }
        let index_bytes = self
            .entries
            .len()
            .checked_mul(ENTRY_BYTES)
            .ok_or_else(|| invalid("immutable block index width overflows"))?;
        let total = HEADER_BYTES
            .checked_add(index_bytes)
            .and_then(|width| width.checked_add(self.payload.len()))
            .ok_or_else(|| invalid("immutable block width overflows"))?;
        if total > MAX_BLOCK_BYTES {
            return Err(invalid("immutable block exceeds byte admission"));
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(total)
            .map_err(|error| CoreError::Storage(format!("reserve immutable block: {error}")))?;
        output.extend_from_slice(&MAGIC);
        output.push(VERSION);
        output.extend_from_slice(
            &u16::try_from(self.entries.len())
                .map_err(|_width| invalid("immutable block count exceeds wire"))?
                .to_be_bytes(),
        );
        output.extend_from_slice(
            &u32::try_from(index_bytes)
                .map_err(|_width| invalid("immutable block index exceeds wire"))?
                .to_be_bytes(),
        );
        output.extend_from_slice(
            &u32::try_from(self.payload.len())
                .map_err(|_width| invalid("immutable block payload exceeds wire"))?
                .to_be_bytes(),
        );
        for entry in self.entries {
            output.extend_from_slice(&entry.key);
            output.extend_from_slice(
                &u32::try_from(entry.offset)
                    .map_err(|_width| invalid("immutable block offset exceeds wire"))?
                    .to_be_bytes(),
            );
            output.extend_from_slice(
                &u32::try_from(entry.length)
                    .map_err(|_width| invalid("immutable block length exceeds wire"))?
                    .to_be_bytes(),
            );
        }
        output.extend_from_slice(&self.payload);
        if output.len() != total {
            return Err(invalid("immutable block output width differs"));
        }
        Ok(output)
    }
}

/// Validated dictionary over one caller-authenticated Record blob. Page bodies
/// are decoded on demand; `iter` and `iter_range` perform the full page check.
pub(super) struct ImmutableRunBlock {
    bytes: Vec<u8>,
    payload_start: usize,
    entries: Vec<Entry>,
}

impl ImmutableRunBlock {
    pub(super) fn decode(bytes: Vec<u8>) -> Result<Self, CoreError> {
        let minimum = HEADER_BYTES
            .checked_add(ENTRY_BYTES)
            .and_then(|width| width.checked_add(1))
            .ok_or_else(|| corrupt("immutable block minimum width overflows"))?;
        if bytes.len() < minimum || bytes.len() > MAX_BLOCK_BYTES {
            return Err(corrupt("immutable block byte admission differs"));
        }
        if bytes.get(..4) != Some(MAGIC.as_slice()) || bytes.get(4) != Some(&VERSION) {
            return Err(corrupt("immutable block magic or version differs"));
        }
        let count = usize::from(u16::from_be_bytes(read_array::<2>(&bytes, 5)?));
        let index_bytes = usize::try_from(u32::from_be_bytes(read_array::<4>(&bytes, 7)?))
            .map_err(|_width| corrupt("immutable block index exceeds platform"))?;
        let payload_bytes = usize::try_from(u32::from_be_bytes(read_array::<4>(&bytes, 11)?))
            .map_err(|_width| corrupt("immutable block payload exceeds platform"))?;
        if count == 0
            || count
                .checked_mul(ENTRY_BYTES)
                .is_none_or(|expected| expected != index_bytes)
        {
            return Err(corrupt("immutable block count or index width differs"));
        }
        let payload_start = HEADER_BYTES
            .checked_add(index_bytes)
            .ok_or_else(|| corrupt("immutable block index boundary overflows"))?;
        let total = payload_start
            .checked_add(payload_bytes)
            .ok_or_else(|| corrupt("immutable block payload boundary overflows"))?;
        if payload_bytes == 0 || total != bytes.len() {
            return Err(corrupt("immutable block sections or trailing bytes differ"));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|error| CoreError::Storage(format!("reserve block dictionary: {error}")))?;
        let mut expected_offset = 0_usize;
        for index in 0..count {
            let start = HEADER_BYTES
                .checked_add(
                    index
                        .checked_mul(ENTRY_BYTES)
                        .ok_or_else(|| corrupt("immutable block entry offset overflows"))?,
                )
                .ok_or_else(|| corrupt("immutable block entry boundary overflows"))?;
            let offset_at = start
                .checked_add(12)
                .ok_or_else(|| corrupt("immutable block offset field overflows"))?;
            let length_at = start
                .checked_add(16)
                .ok_or_else(|| corrupt("immutable block length field overflows"))?;
            let key = read_array::<12>(&bytes, start)?;
            if !matches!(key.first().copied(), Some(1 | 2))
                || entries.last().is_some_and(|prior: &Entry| prior.key >= key)
            {
                return Err(corrupt("immutable block key or order differs"));
            }
            let offset = usize::try_from(u32::from_be_bytes(read_array::<4>(&bytes, offset_at)?))
                .map_err(|_width| corrupt("immutable block offset exceeds platform"))?;
            let length = usize::try_from(u32::from_be_bytes(read_array::<4>(&bytes, length_at)?))
                .map_err(|_width| corrupt("immutable block length exceeds platform"))?;
            if offset != expected_offset || length == 0 {
                return Err(corrupt("immutable block extents have gap or overlap"));
            }
            expected_offset = offset
                .checked_add(length)
                .ok_or_else(|| corrupt("immutable block extent overflows"))?;
            if expected_offset > payload_bytes {
                return Err(corrupt("immutable block extent exceeds payload"));
            }
            entries.push(Entry {
                key,
                offset,
                length,
            });
        }
        if expected_offset != payload_bytes {
            return Err(corrupt("immutable block has unindexed payload bytes"));
        }
        Ok(Self {
            bytes,
            payload_start,
            entries,
        })
    }

    pub(super) fn first_key(&self) -> Result<[u8; 12], CoreError> {
        self.entries
            .first()
            .map(|entry| entry.key)
            .ok_or_else(|| corrupt("immutable block first key absent"))
    }

    pub(super) fn last_key(&self) -> Result<[u8; 12], CoreError> {
        self.entries
            .last()
            .map(|entry| entry.key)
            .ok_or_else(|| corrupt("immutable block last key absent"))
    }

    pub(super) fn get(&self, key: &[u8; 12]) -> Result<Option<RunPostingPage>, CoreError> {
        let Ok(index) = self.entries.binary_search_by_key(key, |entry| entry.key) else {
            return Ok(None);
        };
        self.entries
            .get(index)
            .map(|entry| self.read_page(entry).map(Some))
            .unwrap_or_else(|| Err(corrupt("immutable block selected entry absent")))
    }

    pub(super) fn page_count(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn lower_bound(&self, key: &[u8; 12]) -> usize {
        self.entries.partition_point(|entry| entry.key < *key)
    }

    pub(super) fn key_at(&self, index: usize) -> Option<[u8; 12]> {
        self.entries.get(index).map(|entry| entry.key)
    }

    pub(super) fn read_at(
        &self,
        index: usize,
    ) -> Result<Option<([u8; 12], RunPostingPage)>, CoreError> {
        self.entries
            .get(index)
            .map(|entry| self.read_page(entry).map(|page| Some((entry.key, page))))
            .unwrap_or(Ok(None))
    }

    pub(super) fn iter(
        &self,
    ) -> impl Iterator<Item = Result<([u8; 12], RunPostingPage), CoreError>> + '_ {
        self.entries
            .iter()
            .map(|entry| self.read_page(entry).map(|page| (entry.key, page)))
    }

    #[cfg(test)]
    pub(super) fn iter_range(
        &self,
        lower: &[u8; 12],
        upper_exclusive: Option<&[u8; 12]>,
    ) -> Result<impl Iterator<Item = Result<([u8; 12], RunPostingPage), CoreError>> + '_, CoreError>
    {
        if upper_exclusive.is_some_and(|upper| upper <= lower) {
            return Err(invalid(
                "immutable block scan upper bound is not above lower",
            ));
        }
        let start = self.entries.partition_point(|entry| entry.key < *lower);
        let end = upper_exclusive.map_or(self.entries.len(), |upper| {
            self.entries.partition_point(|entry| entry.key < *upper)
        });
        Ok(self
            .entries
            .get(start..end)
            .into_iter()
            .flatten()
            .map(|entry| self.read_page(entry).map(|page| (entry.key, page))))
    }

    fn read_page(&self, entry: &Entry) -> Result<RunPostingPage, CoreError> {
        let start = self
            .payload_start
            .checked_add(entry.offset)
            .ok_or_else(|| corrupt("immutable block page start overflows"))?;
        let end = start
            .checked_add(entry.length)
            .ok_or_else(|| corrupt("immutable block page end overflows"))?;
        let bytes = self
            .bytes
            .get(start..end)
            .ok_or_else(|| corrupt("immutable block page extent differs"))?;
        let page = RunPostingPage::decode(bytes)?;
        if page.key()? != entry.key {
            return Err(corrupt("immutable block dictionary key differs from page"));
        }
        Ok(page)
    }
}

fn read_array<const N: usize>(bytes: &[u8], start: usize) -> Result<[u8; N], CoreError> {
    let end = start
        .checked_add(N)
        .ok_or_else(|| corrupt("immutable block field offset overflows"))?;
    bytes
        .get(start..end)
        .ok_or_else(|| corrupt("immutable block field is truncated"))?
        .try_into()
        .map_err(|_| corrupt("immutable block field width differs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_authority::paged::run_postings::RunPostingRow;

    fn page(gram: [u8; 3], id: u64) -> RunPostingPage {
        RunPostingPage::new(
            2,
            gram,
            id >> 8,
            vec![RunPostingRow {
                logical_id: id,
                segment_sha256: [7; 32],
            }],
        )
        .expect("page")
    }

    fn build(pages: &[RunPostingPage]) -> Vec<u8> {
        let mut builder = ImmutableRunBlockBuilder::new();
        for page in pages {
            assert!(matches!(
                builder.try_push(page.key().expect("key"), &page.encode().expect("page")),
                Ok(PushResult::Added)
            ));
        }
        builder.finish().expect("block")
    }

    fn overwrite<const N: usize>(bytes: &mut [u8], start: usize, replacement: [u8; N]) {
        let end = start.checked_add(N).expect("test extent");
        bytes
            .get_mut(start..end)
            .expect("test slice")
            .copy_from_slice(&replacement);
    }

    #[test]
    fn independent_wire_vector_and_exact_lookup() {
        let expected = page(*b"abc", 1);
        let mut page_wire = vec![
            0x86, 2, 2, 0x83, 0x18, b'a', 0x18, b'b', 0x18, b'c', 0, 0x81, 0x98, 32,
        ];
        page_wire.extend_from_slice(&[7; 32]);
        page_wire.extend_from_slice(&[0x81, 0x82, 1, 0]);
        let page_width = u32::try_from(page_wire.len()).expect("test page width");
        let mut wire = Vec::new();
        wire.extend_from_slice(b"QIRB");
        wire.push(2);
        wire.extend_from_slice(&1_u16.to_be_bytes());
        wire.extend_from_slice(&20_u32.to_be_bytes());
        wire.extend_from_slice(&page_width.to_be_bytes());
        wire.extend_from_slice(&[2, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 0]);
        wire.extend_from_slice(&0_u32.to_be_bytes());
        wire.extend_from_slice(&page_width.to_be_bytes());
        wire.extend_from_slice(&page_wire);
        assert_eq!(build(&[expected.clone()]), wire);
        let block = ImmutableRunBlock::decode(wire).expect("dictionary");
        assert_eq!(block.page_count(), 1);
        assert_eq!(
            block.first_key().expect("first"),
            expected.key().expect("key")
        );
        assert_eq!(
            block.last_key().expect("last"),
            expected.key().expect("key")
        );
        assert_eq!(
            block.get(&expected.key().expect("key")).expect("lookup"),
            Some(expected)
        );
        assert!(
            block
                .get(&page(*b"abd", 1).key().expect("key"))
                .expect("missing")
                .is_none()
        );
    }

    #[test]
    fn ordered_range_and_full_seal_iteration() {
        let pages = [page(*b"abc", 1), page(*b"abd", 256), page(*b"abe", 512)];
        let bytes = build(&pages);
        let block = ImmutableRunBlock::decode(bytes).expect("dictionary");
        assert_eq!(
            block.first_key().expect("first"),
            pages.first().expect("page").key().expect("key")
        );
        assert_eq!(
            block.last_key().expect("last"),
            pages.last().expect("page").key().expect("key")
        );
        assert_eq!(
            block
                .iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("seal")
                .len(),
            3
        );
        let selected = block
            .iter_range(
                &pages.get(1).expect("middle").key().expect("key"),
                Some(&pages.get(2).expect("end").key().expect("key")),
            )
            .expect("valid range")
            .collect::<Result<Vec<_>, _>>()
            .expect("range");
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected.first().expect("selected").1,
            pages.get(1).expect("middle").clone()
        );
        let last = pages.get(2).expect("last").key().expect("key");
        let first = pages.first().expect("first").key().expect("key");
        assert!(block.iter_range(&last, Some(&first)).is_err());
    }

    #[test]
    fn builder_full_keeps_input_for_next_block() {
        let mut builder = ImmutableRunBlockBuilder::new();
        let mut accepted = 0_u64;
        loop {
            let id = accepted.checked_mul(256).expect("test ID");
            let source = page(*b"abc", id);
            let bytes = source.encode().expect("page");
            match builder
                .try_push(source.key().expect("key"), &bytes)
                .expect("push")
            {
                PushResult::Added => accepted = accepted.checked_add(1).expect("test count"),
                PushResult::Full => {
                    assert!(accepted > 0);
                    let old_bytes = builder.finish().expect("finish");
                    let old = ImmutableRunBlock::decode(old_bytes).expect("block");
                    assert_eq!(
                        old.iter().count(),
                        usize::try_from(accepted).expect("count")
                    );
                    let mut next = ImmutableRunBlockBuilder::new();
                    assert!(matches!(
                        next.try_push(source.key().expect("key"), &bytes),
                        Ok(PushResult::Added)
                    ));
                    break;
                }
            }
        }
    }

    #[test]
    fn maximum_legal_v2_page_fits_one_block() {
        let rows = (0_u16..256)
            .map(|index| {
                let mut digest = [255_u8; 32];
                *digest.first_mut().expect("digest") = u8::try_from(index).expect("byte");
                RunPostingRow {
                    logical_id: u64::MAX
                        .checked_sub(255)
                        .and_then(|id| id.checked_add(u64::from(index)))
                        .expect("ID"),
                    segment_sha256: digest,
                }
            })
            .collect();
        let page = RunPostingPage::new(2, [255; 3], u64::MAX >> 8, rows).expect("max page");
        let bytes = page.encode().expect("canonical v2");
        let mut builder = ImmutableRunBlockBuilder::new();
        assert!(matches!(
            builder.try_push(page.key().expect("key"), &bytes),
            Ok(PushResult::Added)
        ));
        let block = builder.finish().expect("block");
        assert!(block.len() <= MAX_BLOCK_BYTES);
        assert_eq!(
            ImmutableRunBlock::decode(block)
                .expect("dictionary")
                .get(&page.key().expect("key"))
                .expect("lookup"),
            Some(page)
        );
    }

    #[test]
    fn point_lookup_decodes_only_the_selected_page() {
        let first = page(*b"abc", 1);
        let second = page(*b"abd", 256);
        let mut bytes = build(&[first.clone(), second.clone()]);
        let second_body = HEADER_BYTES
            .checked_add(ENTRY_BYTES.checked_mul(2).expect("index"))
            .and_then(|offset| offset.checked_add(first.encode().expect("page").len()))
            .expect("second body");
        overwrite(&mut bytes, second_body, [0]);
        let block = ImmutableRunBlock::decode(bytes).expect("dictionary");
        assert_eq!(
            block.get(&first.key().expect("key")).expect("point"),
            Some(first)
        );
        assert!(block.iter().nth(1).expect("second page").is_err());
    }

    #[test]
    fn rejects_unsorted_keys_and_corrupt_extents() {
        let first = page(*b"abc", 1);
        let second = page(*b"abd", 256);
        let mut builder = ImmutableRunBlockBuilder::new();
        assert!(matches!(
            builder.try_push(second.key().expect("key"), &second.encode().expect("page")),
            Ok(PushResult::Added)
        ));
        assert!(
            builder
                .try_push(first.key().expect("key"), &first.encode().expect("page"))
                .is_err()
        );
        assert!(
            builder
                .try_push(second.key().expect("key"), &second.encode().expect("page"))
                .is_err()
        );
        let source = build(&[first, second]);
        let mut truncated = source.clone();
        let _removed = truncated.pop();
        assert!(ImmutableRunBlock::decode(truncated).is_err());
        let mut trailing = source.clone();
        trailing.push(0);
        assert!(ImmutableRunBlock::decode(trailing).is_err());
        let mut overlap = source.clone();
        let second_entry_offset = HEADER_BYTES.checked_add(ENTRY_BYTES).expect("entry");
        let offset_field = second_entry_offset.checked_add(12).expect("offset");
        overwrite(&mut overlap, offset_field, 0_u32.to_be_bytes());
        assert!(ImmutableRunBlock::decode(overlap).is_err());
        let mut reordered = source.clone();
        overwrite(
            &mut reordered,
            HEADER_BYTES,
            [2, b'z', b'z', b'z', 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert!(ImmutableRunBlock::decode(reordered).is_err());
        let mut forged = source;
        overwrite(
            &mut forged,
            HEADER_BYTES,
            [2, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 1],
        );
        let block = ImmutableRunBlock::decode(forged).expect("dictionary-only decode");
        assert!(block.iter().next().expect("first page").is_err());
    }
}
