//! Canonical File run postings. The 64-document segments remain immutable
//! source witnesses; one run dictionary routes grams to bounded pages of
//! logical IDs and their owning segment digests.

use std::collections::{BTreeMap, BTreeSet};

mod block_directory;
mod first_pass_counts;
mod immutable_block;
mod packed_stream;
#[cfg(test)]
mod packed_stream_tests;
mod page_wire;
#[cfg(test)]
mod page_wire_tests;
use block_directory::{BlockDirectoryCursor, BlockPageInput};
use first_pass_counts::FirstPassCounts;
use packed_stream::{FRESH_LEAF_LOOKAHEAD_GROUPS, PackedEdits, RecordEdit};

use quanta_index_core::{CoreError, RequestBudgetV1};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Digest as _;

use crate::channel_payloads::{decode_cbor_exact, encode_cbor};
use crate::lexical_store::{
    AttemptLease, BlobRef, MAX_MAP_HEIGHT, MAX_PACKED_RECORDS, MAX_RECORD_GROUP_BYTES, MapCursor,
    MapEdit, MapRoot, ObjectKind, ObjectRef, ObjectStore,
};

use super::{
    BoundedVec, MAX_POSTING_PAGE_BYTES, MAX_RECORD_BYTES, MAX_SEGMENT_DOCS, PostingPage, Segment,
};

const ROOT_WIRE_VERSION_V1: u8 = 1;
const ROOT_WIRE_VERSION_V2: u8 = 2;
const GRAM_COUNT_WIRE_VERSION: u8 = 1;
const ID_BUCKET_BITS: u32 = 8;
const MAX_BUCKET_ROWS: usize = 1 << ID_BUCKET_BITS;
// Ciborium worst case for one (u64, [u8; 32]) tuple: tuple header 1,
// integer 9, byte-array header 2, and at most 2 bytes per array element.
const MAX_WIRE_ROW_BYTES: usize = 1 + 9 + 2 + 32 * 2;
const _: () = assert!(MAX_BUCKET_ROWS * MAX_WIRE_ROW_BYTES + 64 <= MAX_POSTING_PAGE_BYTES);

// The producer keeps a policy-bounded run reference list, but decodes only a
// bounded group of source cursors and one gram of posting rows at a time.
// These bounds are staging charges, not an allocator RSS prediction. The map
// decoder admits at most 256 leaf rows or 64 branch rows with keys of at most
// 256 bytes. A cursor retains at most one leaf and one branch per level.
const NEW_RUN_SEGMENT_FAN_IN: usize = 64;
const NEW_RUN_PAGE_FAN_IN: usize = 16;
const DECODED_MAP_LEAF_CHARGE: u64 = 192 * 1024;
const DECODED_MAP_BRANCH_CHARGE: u64 = 96 * 1024;
const MAP_BUILD_AND_CACHE_CHARGE: u64 = 8 * 1024 * 1024;
const CURSOR_AND_REF_CHARGE: u64 = 2 * 1024 * 1024;
const PARTIAL_ROOT_CHARGE: u64 = 1024 * 1024;
const SOURCE_ID_CHARGE: u64 = 4 * 1024 * 1024;
const BUCKET_ROW_CHARGE: usize = 2 * 1024 * 1024;
// One fresh-leaf lookahead retains at most eight 32 KiB groups of bodies,
// 768 keys/refs/domain indices/ready edits, one pending 32 KiB body, and a
// 32 KiB group re-encode. The 2 MiB reservation covers their Vec and BTree
// allocations, including bounded capacity slack, before any source opens.
const FRESH_LEAF_PACK_STAGE_CHARGE: u64 = 2 * 1024 * 1024;
const _: () = assert!(
    FRESH_LEAF_LOOKAHEAD_GROUPS == 8
        && FRESH_LEAF_PACK_STAGE_CHARGE
            >= (FRESH_LEAF_LOOKAHEAD_GROUPS as u64) * (MAX_RECORD_GROUP_BYTES as u64)
                + (MAX_PACKED_RECORDS as u64) * (FRESH_LEAF_LOOKAHEAD_GROUPS as u64) * 512
                + 2 * (MAX_RECORD_GROUP_BYTES as u64)
);
const _: () = assert!(
    NEW_RUN_SEGMENT_FAN_IN * MAX_SEGMENT_DOCS * std::mem::size_of::<RunPostingRow>()
        <= BUCKET_ROW_CHARGE
);

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("file run postings: {reason}"))
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file run postings: {reason}"),
    }
}

fn valid_surface(surface: u8) -> bool {
    surface == 1 || surface == 2
}

/// The fixed bucket is derived from the worst-case 32 KiB canonical page,
/// independent of whether durable logical IDs are contiguous.
pub(crate) fn run_page_key(surface: u8, gram: [u8; 3], id: u64) -> Result<[u8; 12], CoreError> {
    if !valid_surface(surface) {
        return Err(invalid("unknown posting surface"));
    }
    let mut key = [0_u8; 12];
    key[0] = surface;
    key[1..4].copy_from_slice(&gram);
    key[4..].copy_from_slice(&(id >> ID_BUCKET_BITS).to_be_bytes());
    Ok(key)
}

pub(crate) fn run_page_prefix(surface: u8, gram: [u8; 3]) -> Result<[u8; 4], CoreError> {
    if !valid_surface(surface) {
        return Err(invalid("unknown posting surface"));
    }
    Ok([surface, gram[0], gram[1], gram[2]])
}

/// One global route per gram and run. A count is checked against the run's
/// page rows by the independent seal; it is a query estimate, not authority.
pub(crate) fn run_route_key(
    surface: u8,
    gram: [u8; 3],
    count: u64,
    run_id: u64,
) -> Result<[u8; 20], CoreError> {
    if !valid_surface(surface) || count == 0 || run_id == 0 {
        return Err(invalid("run route surface, count or ID differs"));
    }
    let mut key = [0_u8; 20];
    key[..4].copy_from_slice(&run_page_prefix(surface, gram)?);
    key[4..12].copy_from_slice(&count.to_be_bytes());
    key[12..].copy_from_slice(&run_id.to_be_bytes());
    Ok(key)
}

/// One checked counter per `(surface, gram)` in a run. This permits a delta
/// to update only affected route keys without scanning every posting bucket
/// in a potentially large run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunGramCount {
    pub(crate) surface: u8,
    pub(crate) gram: [u8; 3],
    pub(crate) count: u64,
}

impl Serialize for RunGramCount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (GRAM_COUNT_WIRE_VERSION, self.surface, self.gram, self.count).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunGramCount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (version, surface, gram, count) = <(u8, u8, [u8; 3], u64)>::deserialize(deserializer)?;
        if version != GRAM_COUNT_WIRE_VERSION {
            return Err(serde::de::Error::custom("run gram-count version differs"));
        }
        Ok(Self {
            surface,
            gram,
            count,
        })
    }
}

impl RunGramCount {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if !valid_surface(self.surface) || self.count == 0 {
            return Err(invalid("run gram-count surface or count differs"));
        }
        Ok(())
    }

    pub(crate) fn key(&self) -> Result<[u8; 4], CoreError> {
        self.validate()?;
        run_page_prefix(self.surface, self.gram)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = encode_cbor(self, "file run gram count")?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("run gram count exceeds record admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(corrupt("run gram count exceeds record admission"));
        }
        let count: Self = decode_cbor_exact(bytes)
            .map_err(|error| corrupt(&format!("run gram count decode: {error}")))?;
        count
            .validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if count.encode()? != bytes {
            return Err(corrupt("run gram count encoding is not canonical"));
        }
        Ok(count)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunPostingRow {
    pub(crate) logical_id: u64,
    pub(crate) segment_sha256: [u8; 32],
}

impl Serialize for RunPostingRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.logical_id, self.segment_sha256).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RunPostingRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (logical_id, segment_sha256) = <(u64, [u8; 32])>::deserialize(deserializer)?;
        Ok(Self {
            logical_id,
            segment_sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunPostingPage {
    pub(crate) surface: u8,
    pub(crate) gram: [u8; 3],
    pub(crate) bucket: u64,
    pub(crate) rows: Vec<RunPostingRow>,
}

impl RunPostingPage {
    pub(crate) fn new(
        surface: u8,
        gram: [u8; 3],
        bucket: u64,
        rows: Vec<RunPostingRow>,
    ) -> Result<Self, CoreError> {
        let page = Self {
            surface,
            gram,
            bucket,
            rows,
        };
        page.validate()?;
        Ok(page)
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if !valid_surface(self.surface) || self.rows.is_empty() || self.rows.len() > MAX_BUCKET_ROWS
        {
            return Err(invalid("page surface or row count differs"));
        }
        let mut prior = None;
        for row in &self.rows {
            if row.logical_id >> ID_BUCKET_BITS != self.bucket
                || prior.is_some_and(|id| id >= row.logical_id)
            {
                return Err(invalid("page bucket or logical-ID order differs"));
            }
            prior = Some(row.logical_id);
        }
        Ok(())
    }

    pub(crate) fn key(&self) -> Result<[u8; 12], CoreError> {
        self.validate()?;
        run_page_key(
            self.surface,
            self.gram,
            self.rows
                .first()
                .ok_or_else(|| invalid("empty page"))?
                .logical_id,
        )
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = page_wire::encode_v2(self)?;
        if bytes.len() > MAX_POSTING_PAGE_BYTES {
            return Err(invalid("page exceeds encoded-byte admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_POSTING_PAGE_BYTES {
            return Err(corrupt("page exceeds encoded-byte admission"));
        }
        let page = page_wire::decode(bytes)?;
        page.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        Ok(page)
    }
}

/// The run's page map is keyed by (surface, gram, ID bucket). It is the
/// complete posting authority for this run; leaf posting pages remain source
/// witnesses used by the independent seal and repair path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunPostingRoot {
    pub(crate) run_id: u64,
    pub(crate) pages: MapRoot,
    /// None retains the v1 per-page map; Some counts logical pages in the v2 block directory.
    pub(crate) block_pages: Option<u64>,
    pub(crate) gram_counts: MapRoot,
    pub(crate) path_rows: u64,
    pub(crate) content_rows: u64,
    /// Encoded Record root widths per logical map value. Shared roots remain
    /// fully charged even when delta edits leave only one member in the map.
    /// `MapPage` structure and this root Record are accounted separately.
    pub(crate) page_record_encoded_bytes: u64,
    pub(crate) gram_count_record_encoded_bytes: u64,
}

impl Serialize for RunPostingRoot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(logical_pages) = self.block_pages {
            (
                ROOT_WIRE_VERSION_V2,
                self.run_id,
                &self.pages,
                &self.gram_counts,
                self.path_rows,
                self.content_rows,
                self.page_record_encoded_bytes,
                self.gram_count_record_encoded_bytes,
                logical_pages,
            )
                .serialize(serializer)
        } else {
            (
                ROOT_WIRE_VERSION_V1,
                self.run_id,
                &self.pages,
                &self.gram_counts,
                self.path_rows,
                self.content_rows,
                self.page_record_encoded_bytes,
                self.gram_count_record_encoded_bytes,
            )
                .serialize(serializer)
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RunRootWire {
    V1((u8, u64, MapRoot, MapRoot, u64, u64, u64, u64)),
    V2((u8, u64, MapRoot, MapRoot, u64, u64, u64, u64, u64)),
}

impl<'de> Deserialize<'de> for RunPostingRoot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            version,
            run_id,
            pages,
            gram_counts,
            path_rows,
            content_rows,
            page_record_encoded_bytes,
            gram_count_record_encoded_bytes,
            block_pages,
        ) = match RunRootWire::deserialize(deserializer)? {
            RunRootWire::V1((
                version,
                run_id,
                pages,
                gram_counts,
                path_rows,
                content_rows,
                page_record_encoded_bytes,
                gram_count_record_encoded_bytes,
            )) => (
                version,
                run_id,
                pages,
                gram_counts,
                path_rows,
                content_rows,
                page_record_encoded_bytes,
                gram_count_record_encoded_bytes,
                None,
            ),
            RunRootWire::V2((
                version,
                run_id,
                pages,
                gram_counts,
                path_rows,
                content_rows,
                page_record_encoded_bytes,
                gram_count_record_encoded_bytes,
                logical_pages,
            )) => (
                version,
                run_id,
                pages,
                gram_counts,
                path_rows,
                content_rows,
                page_record_encoded_bytes,
                gram_count_record_encoded_bytes,
                Some(logical_pages),
            ),
        };
        if version
            != if block_pages.is_some() {
                ROOT_WIRE_VERSION_V2
            } else {
                ROOT_WIRE_VERSION_V1
            }
        {
            return Err(serde::de::Error::custom("run posting root version differs"));
        }
        Ok(Self {
            run_id,
            pages,
            block_pages,
            gram_counts,
            path_rows,
            content_rows,
            page_record_encoded_bytes,
            gram_count_record_encoded_bytes,
        })
    }
}

impl RunPostingRoot {
    pub(crate) fn is_block_layout(&self) -> bool {
        self.block_pages.is_some()
    }

    pub(crate) fn logical_pages(&self) -> u64 {
        self.block_pages.unwrap_or(self.pages.entries)
    }

    pub(crate) fn page_at(
        &self,
        store: &ObjectStore,
        key: &[u8; 12],
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<(BlobRef, RunPostingPage)>, CoreError> {
        if self.is_block_layout() {
            return block_directory::read_page(store, &self.pages, key, budget);
        }
        let Some(reference) = store.map_get(&self.pages, key, budget)? else {
            return Ok(None);
        };
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("run posting page object kind differs"));
        }
        let page = RunPostingPage::decode(&store.read_blob(
            &reference,
            u64::try_from(MAX_POSTING_PAGE_BYTES).map_err(|_width| invalid("page width"))?,
            budget,
        )?)?;
        if page.key()? != *key {
            return Err(corrupt("run posting page key differs"));
        }
        Ok(Some((reference, page)))
    }

    pub(crate) fn page_cursor<'a>(
        &self,
        store: &'a ObjectStore,
        lower: &[u8],
        upper: Option<&[u8]>,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<RunPageCursor<'a>, CoreError> {
        if self.is_block_layout() {
            let lower = expand_block_bound(lower)?;
            let upper = upper.map(expand_block_bound).transpose()?;
            return Ok(RunPageCursor::Blocks(BlockDirectoryCursor::new(
                store,
                &self.pages,
                lower,
                upper,
                budget,
            )?));
        }
        Ok(RunPageCursor::Pages {
            store,
            budget,
            cursor: store.map_cursor(&self.pages, lower, upper, budget)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn block_census(
        &self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(u64, u64, u64), CoreError> {
        if !self.is_block_layout() {
            return Err(invalid("block census requires v2 run"));
        }
        let mut cursor = BlockDirectoryCursor::new(store, &self.pages, [0; 12], None, budget)?;
        while cursor.next()?.is_some() {}
        let census = cursor.complete_census()?;
        Ok((
            census.logical_pages,
            census.blocks,
            census.record_encoded_bytes,
        ))
    }

    pub(crate) fn block_at(
        &self,
        store: &ObjectStore,
        last_key: &[u8; 12],
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<([u8; 12], u64), CoreError> {
        let block = self.load_directory_block(store, last_key, reference, budget)?;
        Ok((
            block.first_key()?,
            u64::try_from(block.page_count()).map_err(|_width| corrupt("block count width"))?,
        ))
    }

    pub(crate) fn block_pages(
        &self,
        store: &ObjectStore,
        last_key: &[u8; 12],
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<RunBlockPages, CoreError> {
        Ok(RunBlockPages {
            block: self.load_directory_block(store, last_key, reference, budget)?,
            index: 0,
        })
    }

    /// Returns the next physical block after a committed directory key.
    /// The owned block can be retained while its logical pages are proved.
    pub(crate) fn next_block(
        &self,
        store: &ObjectStore,
        after_last: Option<&[u8; 12]>,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<([u8; 12], BlobRef, RunBlockPages)>, CoreError> {
        if !self.is_block_layout() {
            return Err(invalid("next block requires v2 run"));
        }
        let lower = after_last.map_or_else(Vec::new, |last| {
            let mut lower = Vec::with_capacity(13);
            lower.extend_from_slice(last);
            lower.push(0);
            lower
        });
        let mut cursor = store.map_cursor(&self.pages, &lower, None, budget)?;
        let Some((last, reference)) = cursor.next()? else {
            return Ok(None);
        };
        let last: [u8; 12] = last
            .as_slice()
            .try_into()
            .map_err(|_width| corrupt("block directory key width differs"))?;
        let block = self.block_pages(store, &last, &reference, budget)?;
        let first = block.first_key()?;
        if after_last.is_some_and(|prior| first <= *prior) {
            return Err(corrupt("block directory ranges overlap"));
        }
        Ok(Some((last, reference, block)))
    }

    fn load_directory_block(
        &self,
        store: &ObjectStore,
        last_key: &[u8; 12],
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<immutable_block::ImmutableRunBlock, CoreError> {
        if !self.is_block_layout() {
            return Err(invalid("block access requires v2 run"));
        }
        block_directory::load_block(store, last_key, reference, budget)
    }

    pub(crate) fn load(
        store: &ObjectStore,
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("run posting root object kind differs"));
        }
        let bytes = store.read_blob(
            reference,
            u64::try_from(MAX_RECORD_BYTES).map_err(|_width| invalid("record bound width"))?,
            budget,
        )?;
        Self::decode(&bytes)
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        let posting_rows = self
            .path_rows
            .checked_add(self.content_rows)
            .ok_or_else(|| invalid("run posting row count overflow"))?;
        if self.run_id == 0
            || self.pages.page.is_some() != (self.pages.entries != 0)
            || self.gram_counts.page.is_some() != (self.gram_counts.entries != 0)
            || (self.pages.entries == 0) != (posting_rows == 0)
            || self.block_pages.is_some_and(|logical| {
                logical < self.pages.entries || (logical == 0) != (self.pages.entries == 0)
            })
            || (self.gram_counts.entries == 0) != (posting_rows == 0)
            || (self.pages.entries == 0) != (self.page_record_encoded_bytes == 0)
            || (self.gram_counts.entries == 0) != (self.gram_count_record_encoded_bytes == 0)
        {
            return Err(invalid("run ID, map root or posting counters differ"));
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        let bytes = encode_cbor(self, "file run posting root")?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("run posting root exceeds record admission"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(corrupt("run posting root exceeds record admission"));
        }
        let root: Self = decode_cbor_exact(bytes)
            .map_err(|error| corrupt(&format!("run posting root decode: {error}")))?;
        root.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if root.encode()? != bytes {
            return Err(corrupt("run posting root encoding is not canonical"));
        }
        Ok(root)
    }

    pub(crate) fn object_roots(&self) -> Vec<ObjectRef> {
        [&self.pages, &self.gram_counts]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect()
    }

    /// Logical encoded bundle for one run. Shared content-addressed values
    /// are charged once per owning run, not as unique physical pool blocks.
    pub(crate) fn bundle_encoded_bytes(
        &self,
        reference: &BlobRef,
        store: &ObjectStore,
    ) -> Result<u64, CoreError> {
        self.validate()?;
        let encoded = self.encode()?;
        if reference.root.kind != ObjectKind::Record
            || reference.bytes
                != u64::try_from(encoded.len()).map_err(|_width| invalid("root length width"))?
            || reference.sha256 != <[u8; 32]>::from(sha2::Sha256::digest(&encoded))
        {
            return Err(corrupt("run posting root reference differs"));
        }
        let page_structure = store.map_structure_encoded_bytes(&self.pages, None)?;
        let count_structure = store.map_structure_encoded_bytes(&self.gram_counts, None)?;
        reference
            .root
            .bytes
            .checked_add(self.page_record_encoded_bytes)
            .and_then(|sum| sum.checked_add(self.gram_count_record_encoded_bytes))
            .and_then(|sum| sum.checked_add(page_structure))
            .and_then(|sum| sum.checked_add(count_structure))
            .ok_or_else(|| invalid("run posting bundle bytes overflow"))
    }
}

fn expand_block_bound(bound: &[u8]) -> Result<[u8; 12], CoreError> {
    let mut key = [0; 12];
    match bound.len() {
        0 | 4 | 12 => key
            .get_mut(..bound.len())
            .ok_or_else(|| invalid("block bound width differs"))?
            .copy_from_slice(bound),
        _ => {
            return Err(invalid(
                "block bound must be empty, gram prefix or page key",
            ));
        }
    }
    Ok(key)
}

pub(crate) enum RunPageCursor<'a> {
    Pages {
        store: &'a ObjectStore,
        budget: Option<&'a RequestBudgetV1>,
        cursor: MapCursor<'a>,
    },
    Blocks(BlockDirectoryCursor<'a>),
}

impl RunPageCursor<'_> {
    pub(crate) fn active_block_first_key(&self) -> Result<Option<[u8; 12]>, CoreError> {
        Ok(match self {
            Self::Pages { .. } => None,
            Self::Blocks(cursor) => cursor.active_block_first_key(),
        })
    }

    pub(crate) fn next(&mut self) -> Result<Option<(Vec<u8>, BlobRef, RunPostingPage)>, CoreError> {
        self.next_with(&mut || Ok(()))
    }

    pub(crate) fn next_with(
        &mut self,
        probe: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<Option<(Vec<u8>, BlobRef, RunPostingPage)>, CoreError> {
        match self {
            Self::Blocks(cursor) => cursor
                .next_with(probe)
                .map(|row| row.map(|(key, reference, page)| (key.to_vec(), reference, page))),
            Self::Pages {
                store,
                budget,
                cursor,
            } => {
                let Some((key, reference)) = cursor.next()? else {
                    return Ok(None);
                };
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("run posting page kind differs"));
                }
                probe()?;
                let page = RunPostingPage::decode(
                    &store.read_blob(
                        &reference,
                        u64::try_from(MAX_POSTING_PAGE_BYTES)
                            .map_err(|_width| invalid("page width"))?,
                        *budget,
                    )?,
                )?;
                if page.key()?.as_slice() != key.as_slice() {
                    return Err(corrupt("run posting page key differs"));
                }
                Ok(Some((key, reference, page)))
            }
        }
    }
}

pub(crate) struct RunBlockPages {
    block: immutable_block::ImmutableRunBlock,
    index: usize,
}

impl RunBlockPages {
    pub(crate) fn first_key(&self) -> Result<[u8; 12], CoreError> {
        self.block.first_key()
    }

    pub(crate) fn len(&self) -> usize {
        self.block.page_count()
    }

    pub(crate) fn read_at(
        &self,
        index: usize,
    ) -> Result<Option<([u8; 12], RunPostingPage)>, CoreError> {
        self.block.read_at(index)
    }

    pub(crate) fn next(&mut self) -> Result<Option<(Vec<u8>, RunPostingPage)>, CoreError> {
        let Some((key, page)) = self.block.read_at(self.index)? else {
            return Ok(None);
        };
        self.index = self
            .index
            .checked_add(1)
            .ok_or_else(|| corrupt("block page index overflow"))?;
        Ok(Some((key.to_vec(), page)))
    }
}

/// Updates only changed posting buckets and their gram counters.
///
/// The caller supplies leaf-derived target pages. Independent seal
/// verification proves their rows against the changed physical leaves.
pub(crate) struct RunPostingPageEdit {
    pub(crate) prior: Option<(BlobRef, RunPostingPage)>,
    pub(crate) next: Option<RunPostingPage>,
}

/// Rewrites only physical blocks selected by changed logical page keys.
/// Untouched directory entries and their authenticated Record roots are
/// retained by map_apply. The old and new physical byte ledger is per block.
fn apply_block_page_edits(
    store: &ObjectStore,
    lease: &AttemptLease,
    prior: &RunPostingRoot,
    edits: Vec<(Vec<u8>, Option<RunPostingPage>)>,
) -> Result<(MapRoot, u64, u64), CoreError> {
    let mut groups =
        BTreeMap::<Vec<u8>, (Option<BlobRef>, Vec<(Vec<u8>, Option<RunPostingPage>)>)>::new();
    for (key, page) in edits {
        let mut cursor = store.map_cursor(&prior.pages, &key, None, None)?;
        let selected = cursor.next()?;
        // Surface bytes are 1 or 2, so this sentinel is beyond every legal
        // directory key and holds insertions after the final old block.
        let (route, reference) = selected.map_or_else(
            || (vec![u8::MAX; 12], None),
            |(last, reference)| (last, Some(reference)),
        );
        let group = groups
            .entry(route)
            .or_insert_with(|| (reference.clone(), Vec::new()));
        if group.0 != reference {
            return Err(corrupt("block edit route reference changed"));
        }
        group.1.push((key, page));
    }
    let mut directory_edits = BTreeMap::<Vec<u8>, Option<BlobRef>>::new();
    let mut bytes = prior.page_record_encoded_bytes;
    let mut logical_pages = prior.logical_pages();
    for (last, (reference, changes)) in groups {
        let mut pages = BTreeMap::<Vec<u8>, RunPostingPage>::new();
        if let Some(reference) = &reference {
            let block = block_directory::load_block(store, &last, reference, None)?;
            for row in block.iter() {
                let (key, page) = row?;
                if pages.insert(key.to_vec(), page).is_some() {
                    return Err(corrupt("old block has duplicate page key"));
                }
            }
            bytes = bytes
                .checked_sub(reference.root.bytes)
                .ok_or_else(|| corrupt("block Record ledger underflows"))?;
            let _prior = directory_edits.insert(last.clone(), None);
        }
        let old_count =
            u64::try_from(pages.len()).map_err(|_width| corrupt("old block page count width"))?;
        for (key, next) in changes {
            if let Some(page) = next {
                let _prior = pages.insert(key, page);
            } else if pages.remove(&key).is_none() {
                return Err(corrupt("removed page absent from routed block"));
            }
        }
        let new_count =
            u64::try_from(pages.len()).map_err(|_width| invalid("new block page count width"))?;
        logical_pages = logical_pages
            .checked_sub(old_count)
            .and_then(|count| count.checked_add(new_count))
            .ok_or_else(|| corrupt("block logical page count underflows or overflows"))?;
        let rows = pages.into_iter().map(|(key, page)| {
            if page.key()?.as_slice() != key.as_slice() {
                return Err(corrupt("block replacement key differs"));
            }
            BlockPageInput::from_page(&page)
        });
        let (replacement, produced_pages, produced_bytes) =
            block_directory::build_replacement_edits(store, lease, rows)?;
        if produced_pages != new_count {
            return Err(corrupt("block replacement page count differs"));
        }
        bytes = bytes
            .checked_add(produced_bytes)
            .ok_or_else(|| invalid("block Record ledger overflows"))?;
        for edit in replacement {
            if directory_edits
                .insert(edit.key, edit.value)
                .is_some_and(|old| old.is_some())
            {
                return Err(corrupt("replacement block directory key collides"));
            }
        }
    }
    let directory = if directory_edits.is_empty() {
        prior.pages.clone()
    } else {
        store.map_apply(
            lease,
            &prior.pages,
            directory_edits
                .into_iter()
                .map(|(key, value)| Ok(MapEdit { key, value })),
        )?
    };
    Ok((directory, bytes, logical_pages))
}

pub(crate) fn apply_run_page_edits(
    store: &ObjectStore,
    lease: &AttemptLease,
    prior: Option<&RunPostingRoot>,
    run_id: u64,
    edits: BTreeMap<Vec<u8>, RunPostingPageEdit>,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    if run_id == 0 || prior.is_some_and(|root| root.run_id != run_id) {
        return Err(invalid("run page update ID differs"));
    }
    let empty = RunPostingRoot {
        run_id,
        pages: MapRoot::default(),
        block_pages: None,
        gram_counts: MapRoot::default(),
        path_rows: 0,
        content_rows: 0,
        page_record_encoded_bytes: 0,
        gram_count_record_encoded_bytes: 0,
    };
    let prior = prior.unwrap_or(&empty);
    prior.validate()?;
    let mut page_edits = Vec::new();
    let mut block_edits = Vec::new();
    let mut gram_changes = BTreeMap::<Vec<u8>, (u64, u64)>::new();
    let mut page_bytes = prior.page_record_encoded_bytes;
    let mut path_rows = prior.path_rows;
    let mut content_rows = prior.content_rows;
    for (key, edit) in edits {
        if key.len() != 12 {
            return Err(invalid("changed run posting bucket key width differs"));
        }
        let page_key: [u8; 12] = key
            .as_slice()
            .try_into()
            .map_err(|_width| invalid("changed run posting bucket key width differs"))?;
        let actual_prior = prior.page_at(store, &page_key, None)?;
        if actual_prior.as_ref().map(|(reference, _)| reference)
            != edit.prior.as_ref().map(|(reference, _)| reference)
        {
            return Err(corrupt("old run posting page reference differs"));
        }
        if actual_prior.as_ref().map(|(_, page)| page) != edit.prior.as_ref().map(|(_, page)| page)
        {
            return Err(corrupt("old run posting page bytes differ from reference"));
        }
        let previous = edit.prior.as_ref().map(|(_, page)| page);
        if previous.is_some_and(|page| {
            page.key()
                .map_or(true, |actual| actual.as_slice() != key.as_slice())
        }) {
            return Err(corrupt("old run posting page key differs"));
        }
        let next = edit.next;
        if let Some(page) = &next
            && page.key()?.as_slice() != key.as_slice()
        {
            return Err(invalid("new run posting page key differs"));
        }
        let old_rows = previous.map_or(0, |page| page.rows.len());
        let new_rows = next.as_ref().map_or(0, |page| page.rows.len());
        if previous == next.as_ref() {
            return Err(invalid("declared run posting page edit is unchanged"));
        }
        let gram_prefix = key
            .get(..4)
            .ok_or_else(|| invalid("changed run posting gram key absent"))?;
        let count_delta = gram_changes.entry(gram_prefix.to_vec()).or_default();
        count_delta.0 = count_delta
            .0
            .checked_add(u64::try_from(old_rows).map_err(|_width| invalid("old row count width"))?)
            .ok_or_else(|| invalid("removed gram rows overflow"))?;
        count_delta.1 = count_delta
            .1
            .checked_add(u64::try_from(new_rows).map_err(|_width| invalid("new row count width"))?)
            .ok_or_else(|| invalid("added gram rows overflow"))?;
        let total = match key.first().copied() {
            Some(1) => &mut path_rows,
            Some(2) => &mut content_rows,
            _ => return Err(invalid("changed run posting surface differs")),
        };
        let old_rows = u64::try_from(old_rows).map_err(|_width| invalid("old row width"))?;
        let new_rows = u64::try_from(new_rows).map_err(|_width| invalid("new row width"))?;
        *total = total
            .checked_sub(old_rows)
            .and_then(|sum| sum.checked_add(new_rows))
            .ok_or_else(|| corrupt("run posting row total underflows or overflows"))?;
        if prior.is_block_layout() {
            block_edits.push((key, next));
        } else {
            if let Some((reference, _)) = actual_prior {
                page_bytes = page_bytes
                    .checked_sub(reference.root.bytes)
                    .ok_or_else(|| corrupt("run posting Record ledger underflows"))?;
            }
            let new_ref = next
                .map(|page| store.put_blob(lease, ObjectKind::Record, &[], &page.encode()?))
                .transpose()?;
            if let Some(reference) = &new_ref {
                page_bytes = page_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| invalid("run posting Record ledger overflows"))?;
            }
            page_edits.push(Ok(MapEdit {
                key,
                value: new_ref,
            }));
        }
    }
    let (pages, block_pages) = if prior.is_block_layout() {
        let (pages, bytes, logical_pages) =
            apply_block_page_edits(store, lease, prior, block_edits)?;
        page_bytes = bytes;
        (pages, Some(logical_pages))
    } else {
        let pages = if page_edits.is_empty() {
            prior.pages.clone()
        } else {
            store.map_apply(lease, &prior.pages, page_edits.into_iter())?
        };
        (pages, None)
    };
    let mut gram_edits = Vec::new();
    let mut count_bytes = prior.gram_count_record_encoded_bytes;
    for (key, (removed, added)) in gram_changes {
        let old_ref = store.map_get(&prior.gram_counts, &key, None)?;
        let old_count = old_ref
            .as_ref()
            .map(|reference| {
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("old gram count kind differs"));
                }
                RunGramCount::decode(
                    &store.read_blob(
                        reference,
                        u64::try_from(MAX_RECORD_BYTES)
                            .map_err(|_width| invalid("count bound width"))?,
                        None,
                    )?,
                )
            })
            .transpose()?;
        if old_count.as_ref().is_some_and(|count| {
            count
                .key()
                .map_or(true, |actual| actual.as_slice() != key.as_slice())
        }) {
            return Err(corrupt("old run gram-count key differs"));
        }
        let count = old_count
            .map_or(0, |row| row.count)
            .checked_sub(removed)
            .and_then(|sum| sum.checked_add(added))
            .ok_or_else(|| corrupt("run gram-count underflows or overflows"))?;
        if let Some(reference) = old_ref {
            count_bytes = count_bytes
                .checked_sub(reference.root.bytes)
                .ok_or_else(|| corrupt("run gram-count Record ledger underflows"))?;
        }
        let new_ref = if count == 0 {
            None
        } else {
            let record = RunGramCount {
                surface: *key
                    .first()
                    .ok_or_else(|| corrupt("gram count key has no surface"))?,
                gram: key
                    .get(1..4)
                    .ok_or_else(|| corrupt("gram count key has no gram"))?
                    .try_into()
                    .map_err(|_width| corrupt("gram key width differs"))?,
                count,
            };
            let reference = store.put_blob(lease, ObjectKind::Record, &[], &record.encode()?)?;
            count_bytes = count_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| invalid("run gram-count Record ledger overflows"))?;
            Some(reference)
        };
        gram_edits.push(Ok(MapEdit {
            key,
            value: new_ref,
        }));
    }
    let gram_counts = if gram_edits.is_empty() {
        prior.gram_counts.clone()
    } else {
        store.map_apply(lease, &prior.gram_counts, gram_edits.into_iter())?
    };
    let root = RunPostingRoot {
        run_id,
        pages,
        block_pages,
        gram_counts,
        path_rows,
        content_rows,
        page_record_encoded_bytes: page_bytes,
        gram_count_record_encoded_bytes: count_bytes,
    };
    let reference = store.put_blob(
        lease,
        ObjectKind::Record,
        &root.object_roots(),
        &root.encode()?,
    )?;
    Ok((root, reference))
}

/// A bounded k-way iterator over one bucket from each selected run. It never
/// materializes the full run or a source-wide posting vector.
pub(crate) struct RunPostingUnion<'a> {
    inputs: &'a [&'a RunPostingPage],
    positions: Vec<usize>,
    prior_id: Option<u64>,
}

impl<'a> RunPostingUnion<'a> {
    pub(crate) fn new(inputs: &'a [&'a RunPostingPage]) -> Result<Self, CoreError> {
        let Some(first) = inputs.first() else {
            return Err(invalid("run posting union has no input"));
        };
        for page in inputs {
            page.validate()?;
            if page.surface != first.surface
                || page.gram != first.gram
                || page.bucket != first.bucket
            {
                return Err(invalid("run posting union key differs"));
            }
        }
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(inputs.len())
            .map_err(|error| CoreError::Storage(format!("reserve run posting cursors: {error}")))?;
        positions.resize(inputs.len(), 0);
        Ok(Self {
            inputs,
            positions,
            prior_id: None,
        })
    }

    pub(crate) fn next_row(&mut self) -> Result<Option<RunPostingRow>, CoreError> {
        let mut selected = None;
        for (index, page) in self.inputs.iter().enumerate() {
            let position = *self
                .positions
                .get(index)
                .ok_or_else(|| corrupt("run posting cursor position absent"))?;
            let Some(row) = page.rows.get(position) else {
                continue;
            };
            if selected.is_none_or(|(_, current): (usize, RunPostingRow)| {
                row.logical_id < current.logical_id
            }) {
                selected = Some((index, *row));
            }
        }
        let Some((index, row)) = selected else {
            return Ok(None);
        };
        if self.prior_id.is_some_and(|prior| prior >= row.logical_id) {
            return Err(corrupt("run posting union repeats a logical ID"));
        }
        let position = self
            .positions
            .get_mut(index)
            .ok_or_else(|| corrupt("run posting selected cursor absent"))?;
        *position = position
            .checked_add(1)
            .ok_or_else(|| invalid("run posting cursor overflow"))?;
        self.prior_id = Some(row.logical_id);
        Ok(Some(row))
    }
}

pub(crate) fn merge_page_bucket(inputs: &[&RunPostingPage]) -> Result<RunPostingPage, CoreError> {
    let first = inputs
        .first()
        .ok_or_else(|| invalid("run posting merge has no input"))?;
    let mut union = RunPostingUnion::new(inputs)?;
    let mut rows = Vec::new();
    while let Some(row) = union.next_row()? {
        rows.try_reserve(1)
            .map_err(|error| CoreError::Storage(format!("reserve merged posting row: {error}")))?;
        rows.push(row);
    }
    RunPostingPage::new(first.surface, first.gram, first.bucket, rows)
}

/// Independent page oracle: compare the target against a keyed source census
/// rather than reusing the producer's cursor order or merge implementation.
#[cfg(test)]
pub(crate) fn verify_page_union(
    inputs: &[&RunPostingPage],
    target: &RunPostingPage,
) -> Result<(), CoreError> {
    target
        .validate()
        .map_err(|error| corrupt(&error.to_string()))?;
    let mut expected = BTreeMap::new();
    for page in inputs {
        page.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        if page.surface != target.surface
            || page.gram != target.gram
            || page.bucket != target.bucket
        {
            return Err(corrupt("source and target posting keys differ"));
        }
        for row in &page.rows {
            if expected
                .insert(row.logical_id, row.segment_sha256)
                .is_some()
            {
                return Err(corrupt("source runs repeat a logical ID"));
            }
        }
    }
    if expected.len() != target.rows.len()
        || target
            .rows
            .iter()
            .any(|row| expected.get(&row.logical_id) != Some(&row.segment_sha256))
    {
        return Err(corrupt(
            "target posting page differs from exact source union",
        ));
    }
    Ok(())
}

/// Apply one changed-leaf bucket without visiting any other gram or bucket.
/// A missing or mismatched retired locator is corruption, never a no-op.
pub(crate) fn apply_page_delta(
    surface: u8,
    gram: [u8; 3],
    bucket: u64,
    old: Option<&RunPostingPage>,
    removed: &[RunPostingRow],
    added: &[RunPostingRow],
) -> Result<Option<RunPostingPage>, CoreError> {
    if !valid_surface(surface) {
        return Err(invalid("delta surface differs"));
    }
    if let Some(page) = old {
        page.validate()?;
        if page.surface != surface || page.gram != gram || page.bucket != bucket {
            return Err(corrupt("delta base page key differs"));
        }
    }
    let mut rows = old.map_or_else(BTreeMap::new, |page| {
        page.rows
            .iter()
            .map(|row| (row.logical_id, row.segment_sha256))
            .collect::<BTreeMap<_, _>>()
    });
    let mut prior = None;
    for row in removed {
        if row.logical_id >> ID_BUCKET_BITS != bucket
            || prior.is_some_and(|id| id >= row.logical_id)
            || rows.remove(&row.logical_id) != Some(row.segment_sha256)
        {
            return Err(corrupt("retired posting row absent or differs"));
        }
        prior = Some(row.logical_id);
    }
    prior = None;
    for row in added {
        if row.logical_id >> ID_BUCKET_BITS != bucket
            || prior.is_some_and(|id| id >= row.logical_id)
            || rows.insert(row.logical_id, row.segment_sha256).is_some()
        {
            return Err(corrupt("added posting row repeats or differs"));
        }
        prior = Some(row.logical_id);
    }
    if rows.is_empty() {
        return Ok(None);
    }
    RunPostingPage::new(
        surface,
        gram,
        bucket,
        rows.into_iter()
            .map(|(logical_id, segment_sha256)| RunPostingRow {
                logical_id,
                segment_sha256,
            })
            .collect(),
    )
    .map(Some)
}

struct NewRunSegmentCursor<'a> {
    segment: Segment,
    digest: [u8; 32],
    terms: MapCursor<'a>,
    ready: Option<(Vec<u8>, BlobRef)>,
    cached: Option<(BlobRef, PostingPage)>,
}

/// Produces strictly increasing (surface, gram, bucket) pages from a bounded
/// group of immutable physical leaves. Only one gram's at most 64 * 64 member
/// rows is staged at a time.
struct NewRunPageStream<'a> {
    store: &'a ObjectStore,
    counts: &'a mut FirstPassCounts,
    sources: Vec<NewRunSegmentCursor<'a>>,
    surface: u8,
    pending_gram: Option<[u8; 3]>,
    pending_buckets: BTreeMap<u64, Vec<RunPostingRow>>,
    failed: bool,
}

impl<'a> NewRunPageStream<'a> {
    fn new(
        store: &'a ObjectStore,
        _lease: &'a AttemptLease,
        references: &[&BlobRef],
        seen_ids: &mut BTreeSet<u64>,
        counts: &'a mut FirstPassCounts,
    ) -> Result<Self, CoreError> {
        if references.is_empty() || references.len() > NEW_RUN_SEGMENT_FAN_IN {
            return Err(invalid("new run leaf group width differs"));
        }
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(references.len())
            .map_err(|error| CoreError::Storage(format!("reserve new run cursors: {error}")))?;
        for reference in references {
            if reference.root.kind != ObjectKind::Segment {
                return Err(corrupt("new run leaf has wrong object kind"));
            }
            let segment = Segment::decode(&store.read_blob(
                reference,
                u64::try_from(MAX_RECORD_BYTES).map_err(|_width| invalid("record bound width"))?,
                None,
            )?)?;
            let max_ids = usize::try_from(super::super::policy().source_files)
                .map_err(|_width| invalid("source policy width"))?;
            for id in &segment.doc_ids {
                if seen_ids.len() >= max_ids || !seen_ids.insert(*id) {
                    return Err(corrupt("new run leaves repeat or exceed logical IDs"));
                }
            }
            let mut terms = store.map_cursor(&segment.path_terms, &[], None, None)?;
            let ready = terms.next()?;
            sources.push(NewRunSegmentCursor {
                segment,
                digest: reference.root.sha256,
                terms,
                ready,
                cached: None,
            });
        }
        Ok(Self {
            store,
            counts,
            sources,
            surface: 1,
            pending_gram: None,
            pending_buckets: BTreeMap::new(),
            failed: false,
        })
    }

    fn emit_bucket(&mut self) -> Result<Option<BlockPageInput>, CoreError> {
        let Some((bucket, mut rows)) = self.pending_buckets.pop_first() else {
            self.pending_gram = None;
            return Ok(None);
        };
        let gram = self
            .pending_gram
            .ok_or_else(|| corrupt("new run bucket has no gram"))?;
        rows.sort_unstable_by_key(|row| row.logical_id);
        if rows.windows(2).any(|pair| {
            let [left, right] = pair else { return true };
            left.logical_id >= right.logical_id
        }) {
            return Err(corrupt("new run leaves repeat a posting logical ID"));
        }
        let page = RunPostingPage::new(self.surface, gram, bucket, rows)?;
        let encoded = BlockPageInput::from_page(&page)?;
        self.counts.observe_page(&page)?;
        Ok(Some(encoded))
    }

    fn next_edit(&mut self) -> Result<Option<BlockPageInput>, CoreError> {
        if !self.pending_buckets.is_empty() {
            return self.emit_bucket();
        }
        loop {
            let smallest = self
                .sources
                .iter()
                .filter_map(|source| source.ready.as_ref().map(|(key, _)| key.as_slice()))
                .min();
            let Some(key) = smallest else {
                if self.surface == 2 {
                    return Ok(None);
                }
                self.surface = 2;
                for source in &mut self.sources {
                    source.terms =
                        self.store
                            .map_cursor(&source.segment.content_terms, &[], None, None)?;
                    source.ready = source.terms.next()?;
                    source.cached = None;
                }
                continue;
            };
            let gram: [u8; 3] = key
                .try_into()
                .map_err(|_width| corrupt("new run leaf term key width differs"))?;
            for source in &mut self.sources {
                let selected = source
                    .ready
                    .as_ref()
                    .is_some_and(|(key, _)| key.as_slice() == gram.as_slice());
                if !selected {
                    continue;
                }
                let (_, page_ref) = source
                    .ready
                    .take()
                    .ok_or_else(|| corrupt("new run selected term disappeared"))?;
                if source
                    .cached
                    .as_ref()
                    .is_none_or(|(reference, _)| *reference != page_ref)
                {
                    if page_ref.root.kind != ObjectKind::FilePostings {
                        return Err(corrupt("new run leaf page kind differs"));
                    }
                    let page = PostingPage::decode(
                        &self.store.read_blob(
                            &page_ref,
                            u64::try_from(MAX_POSTING_PAGE_BYTES)
                                .map_err(|_width| invalid("posting bound width"))?,
                            None,
                        )?,
                        source.segment.doc_ids.len(),
                    )?;
                    source.cached = Some((page_ref, page));
                }
                let page = &source
                    .cached
                    .as_ref()
                    .ok_or_else(|| corrupt("new run leaf page absent"))?
                    .1;
                let position = page
                    .0
                    .binary_search_by_key(&gram, |entry| entry.0)
                    .map_err(|_missing| corrupt("new run leaf page omits routed gram"))?;
                let mask = page
                    .0
                    .get(position)
                    .ok_or_else(|| corrupt("new run leaf gram vanished"))?
                    .1;
                for (local, logical_id) in source.segment.doc_ids.iter().copied().enumerate() {
                    if mask & (1_u64 << local) != 0 {
                        self.pending_buckets
                            .entry(logical_id >> ID_BUCKET_BITS)
                            .or_default()
                            .push(RunPostingRow {
                                logical_id,
                                segment_sha256: source.digest,
                            });
                    }
                }
                source.ready = source.terms.next()?;
            }
            if self.pending_buckets.is_empty() {
                return Err(corrupt("new run leaf term has no posting IDs"));
            }
            self.pending_gram = Some(gram);
            return self.emit_bucket();
        }
    }
}

impl Iterator for NewRunPageStream<'_> {
    type Item = Result<BlockPageInput, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_edit() {
            Ok(Some(edit)) => Some(Ok(edit)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

struct RunPageMergeStream<'a> {
    cursors: Vec<RunPageCursor<'a>>,
    ready: Vec<Option<(Vec<u8>, BlobRef, RunPostingPage)>>,
    failed: bool,
}

/// Minimum stream staging charge: one decoded leaf frame per input cursor.
///
/// The execution gate adds branch frames from each supplied map descriptor.
/// Five page frames cover the retained merged page, raw next-page bytes,
/// decoded v2 dictionary/indices/rows, and canonical re-encode scratch. The
/// worst bounded overlap is about 111 KiB before allocator overhead; five
/// 32 KiB frames leave headroom without increasing the caller's staging cap.
/// This charge does not include the caller-owned roots or shared store cache.
pub(crate) fn run_merge_stage_bytes(fan_in: usize) -> Result<u64, CoreError> {
    let count = u64::try_from(fan_in).map_err(|_width| invalid("run merge fan-in width"))?;
    let cursor_bytes = std::mem::size_of::<MapCursor<'_>>()
        .checked_add(std::mem::size_of::<Option<(Vec<u8>, BlobRef)>>())
        .ok_or_else(|| invalid("run merge cursor size overflow"))?;
    let cursor_bytes =
        u64::try_from(cursor_bytes).map_err(|_width| invalid("run merge cursor width"))?;
    let page_bytes = MAX_POSTING_PAGE_BYTES
        .checked_mul(5)
        .ok_or_else(|| invalid("run merge page charge overflow"))?;
    let page_bytes = u64::try_from(page_bytes).map_err(|_width| invalid("page charge width"))?;
    count
        .checked_mul(
            cursor_bytes
                .checked_add(DECODED_MAP_LEAF_CHARGE)
                .ok_or_else(|| invalid("run merge cursor charge overflow"))?,
        )
        .and_then(|bytes| bytes.checked_add(page_bytes))
        .ok_or_else(|| invalid("run merge staging charge overflow"))
}

fn run_merge_map_stage_bytes(pages: &[&MapRoot]) -> Result<u64, CoreError> {
    pages
        .iter()
        .try_fold(run_merge_stage_bytes(pages.len())?, |bytes, root| {
            let height = root.page.as_ref().map_or(0, |page| page.height);
            if height > MAX_MAP_HEIGHT {
                return Err(corrupt("run merge map height exceeds bound"));
            }
            u64::from(height)
                .checked_mul(DECODED_MAP_BRANCH_CHARGE)
                .and_then(|frames| bytes.checked_add(frames))
                .ok_or_else(|| invalid("run merge decoded-frame charge overflow"))
        })
}

impl<'a> RunPageMergeStream<'a> {
    fn new(
        store: &'a ObjectStore,
        _lease: &'a AttemptLease,
        inputs: &[RunPostingRoot],
        max_staging_bytes: u64,
    ) -> Result<Self, CoreError> {
        for root in inputs {
            root.validate()?;
        }
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(inputs.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page roots: {error}")))?;
        pages.extend(inputs.iter().map(|root| &root.pages));
        let charge = run_merge_map_stage_bytes(&pages)?;
        if charge > max_staging_bytes {
            return Err(invalid("run merge exceeds staging packet bound"));
        }
        let mut cursors = Vec::new();
        let mut ready = Vec::new();
        cursors
            .try_reserve_exact(inputs.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page cursors: {error}")))?;
        ready
            .try_reserve_exact(inputs.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page heads: {error}")))?;
        for root in inputs {
            let mut cursor = root.page_cursor(store, &[], None, None)?;
            ready.push(cursor.next()?);
            cursors.push(cursor);
        }
        Ok(Self {
            cursors,
            ready,
            failed: false,
        })
    }

    fn new_block_roots(
        store: &'a ObjectStore,
        _lease: &'a AttemptLease,
        pages: &[&MapRoot],
        max_staging_bytes: u64,
    ) -> Result<Self, CoreError> {
        let cursor_charge = run_merge_map_stage_bytes(pages)?;
        if cursor_charge > max_staging_bytes {
            return Err(invalid("run merge exceeds staging packet bound"));
        }
        let mut cursors = Vec::new();
        let mut ready = Vec::new();
        cursors
            .try_reserve_exact(pages.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page cursors: {error}")))?;
        ready
            .try_reserve_exact(pages.len())
            .map_err(|error| CoreError::Storage(format!("reserve run page heads: {error}")))?;
        for &page_root in pages {
            let mut cursor = RunPageCursor::Blocks(BlockDirectoryCursor::new(
                store, page_root, [0; 12], None, None,
            )?);
            ready.push(cursor.next()?);
            cursors.push(cursor);
        }
        Ok(Self {
            cursors,
            ready,
            failed: false,
        })
    }

    fn next_edit(&mut self) -> Result<Option<BlockPageInput>, CoreError> {
        let Some(key) = self
            .ready
            .iter()
            .filter_map(|row| row.as_ref().map(|(key, _, _)| key))
            .min()
            .cloned()
        else {
            return Ok(None);
        };
        let mut merged: Option<RunPostingPage> = None;
        for index in 0..self.ready.len() {
            if self
                .ready
                .get(index)
                .ok_or_else(|| corrupt("run page merge head absent"))?
                .as_ref()
                .is_some_and(|(candidate, _, _)| *candidate == key)
            {
                let (_, reference, page) = self
                    .ready
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run page merge selected head absent"))?
                    .take()
                    .ok_or_else(|| corrupt("run page head disappeared"))?;
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("run page has wrong object kind"));
                }
                if page.key()?.as_slice() != key {
                    return Err(corrupt("run page map key differs"));
                }
                merged = Some(match merged {
                    Some(prior) => merge_page_bucket(&[&prior, &page])?,
                    None => page,
                });
                let next = self
                    .cursors
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run page merge cursor absent"))?
                    .next()?;
                *self
                    .ready
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run page merge head slot absent"))? = next;
            }
        }
        let merged = merged.ok_or_else(|| corrupt("run page merge has no selected page"))?;
        if merged.key()?.as_slice() != key.as_slice() {
            return Err(corrupt("merged run page key differs"));
        }
        Ok(Some(BlockPageInput::from_page(&merged)?))
    }
}

impl Iterator for RunPageMergeStream<'_> {
    type Item = Result<BlockPageInput, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_edit() {
            Ok(Some(edit)) => Some(Ok(edit)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

struct RunGramCountStream<'a> {
    cursor: RunPageCursor<'a>,
    ready: Option<(Vec<u8>, BlobRef, RunPostingPage)>,
    path_rows: u64,
    content_rows: u64,
    failed: bool,
    block_layout: bool,
    prior_block_root: Option<ObjectRef>,
    page_record_encoded_bytes: u64,
    logical_pages: u64,
}

impl<'a> RunGramCountStream<'a> {
    fn new_block(
        store: &'a ObjectStore,
        _lease: &AttemptLease,
        directory: &MapRoot,
    ) -> Result<Self, CoreError> {
        Self::new_with_layout(store, directory, true)
    }

    fn new_with_layout(
        store: &'a ObjectStore,
        pages: &MapRoot,
        block_layout: bool,
    ) -> Result<Self, CoreError> {
        let mut cursor = if block_layout {
            RunPageCursor::Blocks(BlockDirectoryCursor::new(
                store, pages, [0; 12], None, None,
            )?)
        } else {
            RunPageCursor::Pages {
                store,
                budget: None,
                cursor: store.map_cursor(pages, &[], None, None)?,
            }
        };
        let ready = cursor.next()?;
        Ok(Self {
            cursor,
            ready,
            path_rows: 0,
            content_rows: 0,
            failed: false,
            block_layout,
            prior_block_root: None,
            page_record_encoded_bytes: 0,
            logical_pages: 0,
        })
    }

    fn next_edit(&mut self) -> Result<Option<RecordEdit>, CoreError> {
        let Some((first_key, _, _)) = &self.ready else {
            return Ok(None);
        };
        let prefix: [u8; 4] = first_key
            .get(..4)
            .ok_or_else(|| corrupt("run page key lacks gram prefix"))?
            .try_into()
            .map_err(|_width| corrupt("run page gram prefix width differs"))?;
        let mut count = 0_u64;
        while self
            .ready
            .as_ref()
            .is_some_and(|(key, _, _)| key.get(..4) == Some(prefix.as_slice()))
        {
            let (key, reference, page) = self
                .ready
                .take()
                .ok_or_else(|| corrupt("run count page disappeared"))?;
            if key.len() != 12
                || reference.root.kind != ObjectKind::Record
                || page.key()?.as_slice() != key.as_slice()
            {
                return Err(corrupt("run count page key or kind differs"));
            }
            if !self.block_layout || self.prior_block_root.as_ref() != Some(&reference.root) {
                self.page_record_encoded_bytes = self
                    .page_record_encoded_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| invalid("run page Record bytes overflow"))?;
            }
            self.prior_block_root = Some(reference.root);
            self.logical_pages = self
                .logical_pages
                .checked_add(1)
                .ok_or_else(|| invalid("run logical page count overflows"))?;
            count = count
                .checked_add(
                    u64::try_from(page.rows.len()).map_err(|_width| invalid("page count width"))?,
                )
                .ok_or_else(|| invalid("run gram count overflow"))?;
            self.ready = self.cursor.next()?;
        }
        let total = if prefix[0] == 1 {
            &mut self.path_rows
        } else if prefix[0] == 2 {
            &mut self.content_rows
        } else {
            return Err(corrupt("run gram surface differs"));
        };
        *total = total
            .checked_add(count)
            .ok_or_else(|| invalid("run posting total overflow"))?;
        let record = RunGramCount {
            surface: prefix[0],
            gram: [prefix[1], prefix[2], prefix[3]],
            count,
        };
        Ok(Some(RecordEdit {
            key: prefix.to_vec(),
            bytes: record.encode()?,
        }))
    }
}

impl Iterator for RunGramCountStream<'_> {
    type Item = Result<RecordEdit, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_edit() {
            Ok(Some(edit)) => Some(Ok(edit)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

/// Charge bounded scratch for one fresh-run construction stage.
///
/// Conservative decoded-memory charge for fresh-run bootstrap.
///
/// The cap covers the simultaneously live source cursors, their decoded map
/// frames and cached physical posting pages, one gram's bounded bucket rows,
/// intermediate page roots, and map-build/cache overhead. A 64 KiB encoded
/// `MapPage` can decode into more than 64 KiB of Vec/row storage, so the frame
/// charge is deliberately based on decoded fanout and key bounds.
fn new_run_stage_charge() -> Result<u64, CoreError> {
    let record_bytes = u64::try_from(MAX_RECORD_BYTES).map_err(|_width| invalid("record width"))?;
    let posting_bytes =
        u64::try_from(MAX_POSTING_PAGE_BYTES).map_err(|_width| invalid("posting width"))?;
    let bucket_bytes =
        u64::try_from(BUCKET_ROW_CHARGE).map_err(|_width| invalid("bucket width"))?;
    let frames = u64::from(MAX_MAP_HEIGHT)
        .checked_mul(DECODED_MAP_BRANCH_CHARGE)
        .and_then(|branches| branches.checked_add(DECODED_MAP_LEAF_CHARGE))
        .ok_or_else(|| invalid("new run cursor frame charge overflow"))?;
    let source = frames
        .checked_add(record_bytes)
        // One decoded cached page and one transient physical page payload.
        .and_then(|bytes| bytes.checked_add(posting_bytes.checked_mul(2)?))
        .ok_or_else(|| invalid("new run source charge overflow"))?;
    u64::try_from(NEW_RUN_SEGMENT_FAN_IN)
        .map_err(|_width| invalid("new run fan-in width"))?
        .checked_mul(source)
        .and_then(|bytes| bytes.checked_add(MAP_BUILD_AND_CACHE_CHARGE))
        .and_then(|bytes| bytes.checked_add(CURSOR_AND_REF_CHARGE))
        .and_then(|bytes| bytes.checked_add(PARTIAL_ROOT_CHARGE))
        .and_then(|bytes| bytes.checked_add(SOURCE_ID_CHARGE))
        .and_then(|bytes| bytes.checked_add(bucket_bytes))
        .and_then(|bytes| bytes.checked_add(FRESH_LEAF_PACK_STAGE_CHARGE))
        .ok_or_else(|| invalid("new run staging charge overflow"))
}

/// Build a fresh run from immutable leaves without repeated posting-map mutation.
///
/// Intermediate maps are attempt-local and never appear in the published run
/// registry or root; all writes retain the caller's lease.
pub(crate) fn bootstrap_new_run_postings(
    store: &ObjectStore,
    lease: &AttemptLease,
    run_id: u64,
    references: &[&BlobRef],
    max_staging_bytes: u64,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    bootstrap_new_run_postings_with_count_cache_limit(
        store,
        lease,
        run_id,
        references,
        max_staging_bytes,
        first_pass_counts::MAX_BYTES,
    )
}

fn bootstrap_new_run_postings_with_count_cache_limit(
    store: &ObjectStore,
    lease: &AttemptLease,
    run_id: u64,
    references: &[&BlobRef],
    max_staging_bytes: u64,
    count_cache_limit: u64,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    if run_id == 0 || references.is_empty() {
        return Err(invalid("new run ID or leaf set is empty"));
    }
    let reference_count =
        u64::try_from(references.len()).map_err(|_width| invalid("new run leaf count width"))?;
    if reference_count > super::super::policy().source_files {
        return Err(invalid("new run leaf set exceeds source policy"));
    }
    let required = new_run_stage_charge()?;
    if required > max_staging_bytes {
        return Err(invalid("new run bootstrap exceeds staging admission"));
    }
    // Only one final leaf group can supply final counts directly. Larger runs
    // retain the original independent read pass after their merge levels.
    let cache_limit = if references.len() <= NEW_RUN_SEGMENT_FAN_IN {
        count_cache_limit.min(max_staging_bytes - required)
    } else {
        0
    };
    let mut first_pass_counts = FirstPassCounts::new(cache_limit);
    let mut first_pass_page_bytes = 0_u64;
    let mut first_pass_logical_pages = 0_u64;
    let mut level = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let leaves_phase = crate::causal_profile::begin_phase("file_run_leaf_maps");
    for group in references.chunks(NEW_RUN_SEGMENT_FAN_IN) {
        let stream = crate::causal_profile::timed_work("file_run_leaf_cursor_open", || {
            NewRunPageStream::new(store, lease, group, &mut seen_ids, &mut first_pass_counts)
        })?;
        let group_phase = crate::causal_profile::begin_phase("file_run_leaf_group_map_bootstrap");
        let built = block_directory::build_directory(store, lease, stream)?;
        if references.len() <= NEW_RUN_SEGMENT_FAN_IN {
            first_pass_page_bytes = built.record_encoded_bytes;
            first_pass_logical_pages = built.logical_pages;
        }
        let pages = built.root;
        crate::causal_profile::finish_phase("file_run_leaf_group_map_bootstrap", group_phase);
        level
            .try_reserve(1)
            .map_err(|error| CoreError::Storage(format!("reserve partial run map: {error}")))?;
        level.push(pages);
    }
    crate::causal_profile::finish_phase("file_run_leaf_maps", leaves_phase);
    let merge_phase = crate::causal_profile::begin_phase("file_run_merge_maps");
    while level.len() > 1 {
        let mut next = Vec::new();
        for group in level.chunks(NEW_RUN_PAGE_FAN_IN) {
            let pages = if group.len() == 1 {
                group
                    .first()
                    .ok_or_else(|| corrupt("new run merge group absent"))?
                    .clone()
            } else {
                let mut page_roots = Vec::new();
                page_roots.try_reserve_exact(group.len()).map_err(|error| {
                    CoreError::Storage(format!("reserve partial run roots: {error}"))
                })?;
                page_roots.extend(group.iter());
                let mut stream = RunPageMergeStream::new_block_roots(
                    store,
                    lease,
                    &page_roots,
                    max_staging_bytes,
                )?;
                block_directory::build_directory(store, lease, &mut stream)?.root
            };
            next.try_reserve(1)
                .map_err(|error| CoreError::Storage(format!("reserve merged run map: {error}")))?;
            next.push(pages);
        }
        level = next;
    }
    crate::causal_profile::finish_phase("file_run_merge_maps", merge_phase);
    let counts_phase = crate::causal_profile::begin_phase("file_run_counts");
    let pages = level
        .pop()
        .ok_or_else(|| corrupt("new run produced no page map"))?;
    let (
        gram_counts,
        gram_count_record_encoded_bytes,
        path_rows,
        content_rows,
        page_record_encoded_bytes,
        logical_pages,
    ) = if let Some((counts, path_rows, content_rows)) = first_pass_counts.into_records() {
        let mut packed = PackedEdits::new(store, lease, counts);
        let root = store.map_bootstrap(lease, &mut packed)?;
        (
            root,
            packed.record_encoded_bytes,
            path_rows,
            content_rows,
            first_pass_page_bytes,
            first_pass_logical_pages,
        )
    } else {
        let mut counts = RunGramCountStream::new_block(store, lease, &pages)?;
        let mut packed = PackedEdits::new(store, lease, &mut counts);
        let root = store.map_bootstrap(lease, &mut packed)?;
        let bytes = packed.record_encoded_bytes;
        drop(packed);
        (
            root,
            bytes,
            counts.path_rows,
            counts.content_rows,
            counts.page_record_encoded_bytes,
            counts.logical_pages,
        )
    };
    let root = RunPostingRoot {
        run_id,
        pages,
        block_pages: Some(logical_pages),
        gram_counts,
        path_rows,
        content_rows,
        page_record_encoded_bytes,
        gram_count_record_encoded_bytes,
    };
    let reference = store.put_blob(
        lease,
        ObjectKind::Record,
        &root.object_roots(),
        &root.encode()?,
    )?;
    crate::causal_profile::finish_phase("file_run_counts", counts_phase);
    Ok((root, reference))
}

/// Merges authenticated posting dictionaries of complete old runs.
///
/// Leaf segments may be reused unchanged. Run-level page authority and route
/// counts are rebuilt through bounded page streams.
pub(crate) fn merge_run_posting_roots(
    store: &ObjectStore,
    lease: &AttemptLease,
    new_run_id: u64,
    inputs: &[RunPostingRoot],
    max_staging_bytes: u64,
) -> Result<(RunPostingRoot, BlobRef), CoreError> {
    if new_run_id == 0
        || inputs.len() < 2
        || inputs.windows(2).any(|pair| {
            let [left, right] = pair else { return true };
            left.run_id >= right.run_id
        })
        || inputs.iter().any(|root| root.run_id >= new_run_id)
    {
        return Err(invalid("run merge IDs are not sorted old cohorts"));
    }
    let mut page_stream = RunPageMergeStream::new(store, lease, inputs, max_staging_bytes)?;
    let built = block_directory::build_directory(store, lease, &mut page_stream)?;
    let pages = built.root;
    let logical_pages = built.logical_pages;
    let page_record_encoded_bytes = built.record_encoded_bytes;
    drop(page_stream);
    let mut count_stream = RunGramCountStream::new_block(store, lease, &pages)?;
    let mut packed_counts = PackedEdits::new(store, lease, &mut count_stream);
    let gram_counts = store.map_bootstrap(lease, &mut packed_counts)?;
    let gram_count_record_encoded_bytes = packed_counts.record_encoded_bytes;
    drop(packed_counts);
    let root = RunPostingRoot {
        run_id: new_run_id,
        pages,
        block_pages: Some(logical_pages),
        gram_counts,
        path_rows: count_stream.path_rows,
        content_rows: count_stream.content_rows,
        page_record_encoded_bytes,
        gram_count_record_encoded_bytes,
    };
    let reference = store.put_blob(
        lease,
        ObjectKind::Record,
        &root.object_roots(),
        &root.encode()?,
    )?;
    Ok((root, reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexical_store::AttemptBinding;

    fn row(id: u64, digest: u8) -> RunPostingRow {
        RunPostingRow {
            logical_id: id,
            segment_sha256: [digest; 32],
        }
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 1,
        }
    }

    fn physical_segment(store: &ObjectStore, lease: &AttemptLease, logical_id: u64) -> BlobRef {
        physical_segment_rows(store, lease, vec![logical_id], 1)
    }

    fn physical_segment_rows(
        store: &ObjectStore,
        lease: &AttemptLease,
        doc_ids: Vec<u64>,
        content_mask: u64,
    ) -> BlobRef {
        let count = doc_ids.len();
        let path_mask = if count == 64 {
            u64::MAX
        } else {
            1_u64
                .checked_shl(u32::try_from(count).expect("source count width"))
                .and_then(|mask| mask.checked_sub(1))
                .expect("source mask width")
        };
        let path_page = PostingPage(vec![(*b"abc", path_mask)]);
        let content_page = PostingPage(vec![(*b"xyz", content_mask)]);
        let path_ref = store
            .put_blob(
                lease,
                ObjectKind::FilePostings,
                &[],
                &path_page.encode(count).expect("path page"),
            )
            .expect("path blob");
        let content_ref = store
            .put_blob(
                lease,
                ObjectKind::FilePostings,
                &[],
                &content_page.encode(count).expect("content page"),
            )
            .expect("content blob");
        let path_terms = store
            .map_bootstrap(
                lease,
                std::iter::once(Ok(MapEdit {
                    key: b"abc".to_vec(),
                    value: Some(path_ref),
                })),
            )
            .expect("path term map");
        let content_terms = store
            .map_bootstrap(
                lease,
                std::iter::once(Ok(MapEdit {
                    key: b"xyz".to_vec(),
                    value: Some(content_ref),
                })),
            )
            .expect("content term map");
        let segment = Segment {
            doc_ids,
            path_terms,
            content_terms,
            path_memberships: u64::try_from(count).expect("path count"),
            content_memberships: u64::from(content_mask.count_ones()),
        };
        let deps = [&segment.path_terms, &segment.content_terms]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect::<Vec<_>>();
        store
            .put_blob(
                lease,
                ObjectKind::Segment,
                &deps,
                &segment.encode().expect("segment bytes"),
            )
            .expect("segment blob")
    }

    fn two_page_block_bytes(path: &RunPostingPage, content: &RunPostingPage) -> Vec<u8> {
        let mut block = immutable_block::ImmutableRunBlockBuilder::new();
        for page in [path, content] {
            assert!(matches!(
                block
                    .try_push(
                        page.key().expect("page key"),
                        &page.encode().expect("page bytes")
                    )
                    .expect("push page"),
                immutable_block::PushResult::Added
            ));
        }
        block.finish().expect("block bytes")
    }

    #[test]
    fn run_merge_refuses_decoded_frames_before_loading_or_writing() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let segment = physical_segment(&store, &lease, 1);
        let (root, _) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &[&segment],
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("run postings");
        let mut deep = root.pages.clone();
        // The otherwise valid descriptor points to an absent object. Admission
        // must refuse frame storage before attempting this independent read.
        let page = deep.page.as_mut().expect("page descriptor");
        page.height = 12;
        page.object.sha256 = [0x5a; 32];
        let before = lease.write_cost_snapshot().expect("before refused stream");
        let mut deep_root = root.clone();
        deep_root.pages = deep.clone();
        let refused =
            RunPageMergeStream::new(&store, &lease, &[deep_root, root.clone()], 1_000_000);
        assert!(matches!(refused, Err(CoreError::InvalidContract(message))
            if message == "file run postings: run merge exceeds staging packet bound"));
        assert_eq!(lease.write_cost_snapshot().expect("after refusal"), before);
        let shallow =
            RunPageMergeStream::new(&store, &lease, &[root.clone(), root.clone()], 1_000_000)
                .expect("same fan-in with shallow descriptors is admitted");
        drop(shallow);
        let page = deep.page.as_mut().expect("page descriptor");
        page.height = 13;
        assert!(matches!(run_merge_map_stage_bytes(&[&deep]),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE));
    }

    #[test]
    fn new_run_bootstrap_preserves_full_masks_and_shuffled_interleaved_leaves() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        // Four independently sorted leaves interleave in one full logical-ID
        // bucket. Odd local positions include bit 63 on every source leaf.
        let leaves = (0_u64..4)
            .map(|lane| {
                let ids = (0_u64..64).map(|local| local * 4 + lane + 256).collect();
                physical_segment_rows(&store, &lease, ids, 0xaaaa_aaaa_aaaa_aaaa)
            })
            .collect::<Vec<_>>();
        let reverse = leaves.iter().rev().collect::<Vec<_>>();
        let (root, reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &reverse,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("full-mask bootstrap");
        assert_eq!((root.path_rows, root.content_rows), (256, 128));
        assert_eq!((root.logical_pages(), root.gram_counts.entries), (2, 2));
        assert!(root.is_block_layout());
        let ordered = leaves.iter().collect::<Vec<_>>();
        let (_, ordered_ref) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &ordered,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("ordered bootstrap");
        assert_eq!(reference, ordered_ref);
        for (surface, gram) in [(1_u8, *b"abc"), (2_u8, *b"xyz")] {
            let key = run_page_key(surface, gram, 256).expect("fixed key");
            let (_block_ref, page) = root
                .page_at(&store, &key, None)
                .expect("exact page lookup")
                .expect("posting page");
            let expected = (256_u64..=511)
                .filter(|id| surface == 1 || id & 4 != 0)
                .map(|id| RunPostingRow {
                    logical_id: id,
                    segment_sha256: leaves
                        .get(usize::try_from(id & 3).expect("lane width"))
                        .expect("source lane")
                        .root
                        .sha256,
                })
                .collect::<Vec<_>>();
            assert_eq!(page.rows, expected);
        }
    }

    #[test]
    fn first_pass_counts_match_cold_reread_and_reduce_producer_object_reads() {
        fn build(cache_limit: u64) -> (RunPostingRoot, BlobRef, u64) {
            let directory = tempfile::tempdir().expect("temporary store");
            let store = ObjectStore::open(directory.path()).expect("object store");
            let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
            let leaves = (0_u64..5)
                .map(|lane| {
                    let ids = (0_u64..64).map(|local| local * 5 + lane + 256).collect();
                    physical_segment_rows(&store, &lease, ids, 0xaaaa_aaaa_aaaa_aaaa)
                })
                .collect::<Vec<_>>();
            let references = leaves.iter().collect::<Vec<_>>();
            let producer = store.metered_clone();
            let (root, reference) = bootstrap_new_run_postings_with_count_cache_limit(
                &producer,
                &lease,
                1,
                &references,
                super::super::super::policy().bucket_scratch_bytes,
                cache_limit,
            )
            .expect("bootstrap");
            let reads = producer.read_meter_snapshot().1;
            // A new meter has no producer cache. Its authoritative page scan
            // must reproduce every persisted gram count and both row totals.
            let verifier = store.metered_clone();
            let mut independent = RunGramCountStream::new_block(&verifier, &lease, &root.pages)
                .expect("cold count stream");
            for entry in independent.by_ref() {
                let expected = entry.expect("independent page-derived count");
                let value = verifier
                    .map_get(&root.gram_counts, &expected.key, None)
                    .expect("count lookup")
                    .expect("count value");
                assert_eq!(
                    verifier
                        .read_blob(&value, MAX_RECORD_BYTES as u64, None)
                        .expect("authenticated count"),
                    expected.bytes
                );
            }
            assert_eq!(
                (independent.path_rows, independent.content_rows),
                (320, 160)
            );
            assert_eq!((root.path_rows, root.content_rows), (320, 160));
            assert_eq!(
                root.page_record_encoded_bytes,
                independent.page_record_encoded_bytes
            );
            (root, reference, reads)
        }
        let fast = build(first_pass_counts::MAX_BYTES);
        let fallback = build(0);
        let overflow = build(64); // fewer than two distinct gram entries fit.
        assert_eq!(fast.0, fallback.0);
        assert_eq!(fast.1, fallback.1);
        assert_eq!(overflow.0, fallback.0);
        assert_eq!(overflow.1, fallback.1);
        assert!(
            fast.2 < fallback.2,
            "first pass must remove real producer reads"
        );
    }

    #[test]
    fn new_run_bootstrap_crosses_chunk_and_bucket_with_exact_source_rows() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let leaves = (191_u64..=256)
            .map(|id| (id, physical_segment(&store, &lease, id)))
            .collect::<Vec<_>>();
        let references = leaves
            .iter()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let (root, reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("multi-pass new run");
        assert_eq!(root.logical_pages(), 4);
        assert_eq!(root.gram_counts.entries, 2);
        assert_eq!(root.path_rows, 66);
        assert_eq!(root.content_rows, 66);
        assert_eq!(
            RunPostingRoot::load(&store, &reference, None).expect("reload"),
            root
        );
        for (surface, gram) in [(1_u8, *b"abc"), (2_u8, *b"xyz")] {
            for (first, last) in [(191_u64, 255_u64), (256_u64, 256_u64)] {
                let (_block_ref, page) = root
                    .page_at(
                        &store,
                        &run_page_key(surface, gram, first).expect("key"),
                        None,
                    )
                    .expect("exact page lookup")
                    .expect("page");
                let expected = leaves
                    .iter()
                    .filter(|(id, _)| *id >= first && *id <= last)
                    .map(|(id, source)| RunPostingRow {
                        logical_id: *id,
                        segment_sha256: source.root.sha256,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(page.rows, expected);
            }
            let count_key = [surface, gram[0], gram[1], gram[2]];
            let count_ref = store
                .map_get(&root.gram_counts, &count_key, None)
                .expect("count map")
                .expect("count ref");
            let count = RunGramCount::decode(
                &store
                    .read_blob(
                        &count_ref,
                        u64::try_from(MAX_RECORD_BYTES).expect("bound"),
                        None,
                    )
                    .expect("count bytes"),
            )
            .expect("count");
            assert_eq!(count.count, 66);
        }
        let mut page_bytes = 0_u64;
        store
            .map_scan(&root.pages, &[], None, None, &mut |_key, value| {
                page_bytes = page_bytes
                    .checked_add(value.root.bytes)
                    .expect("page byte sum");
                Ok(())
            })
            .expect("page census");
        assert_eq!(root.page_record_encoded_bytes, page_bytes);
        let (logical_pages, physical_blocks, physical_bytes) = root
            .block_census(&store, None)
            .expect("independent block census");
        assert_eq!(logical_pages, 4);
        assert_eq!(physical_blocks, root.pages.entries);
        assert_eq!(physical_bytes, page_bytes);
    }

    #[test]
    fn new_run_bootstrap_retires_partial_pages_without_collecting_retained_run() {
        use crate::lexical_gc::collect_shared_pool;

        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let original = store
            .begin_attempt(&binding(), &[])
            .expect("original attempt");
        let leaves = (191_u64..=256)
            .map(|id| (id, physical_segment(&store, &original, id)))
            .collect::<Vec<_>>();
        let references = leaves
            .iter()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let (root, final_ref) = bootstrap_new_run_postings(
            &store,
            &original,
            1,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("two-partial bootstrap");
        assert_eq!((root.logical_pages(), root.gram_counts.entries), (4, 2));
        let expected_partial_page = RunPostingPage::new(
            1,
            *b"abc",
            0,
            leaves
                .iter()
                .take(64)
                .map(|(id, reference)| RunPostingRow {
                    logical_id: *id,
                    segment_sha256: reference.root.sha256,
                })
                .collect(),
        )
        .expect("fixed partial page");
        let expected_content_page =
            RunPostingPage::new(2, *b"xyz", 0, expected_partial_page.rows.clone())
                .expect("fixed partial content page");
        let partial_bytes = two_page_block_bytes(&expected_partial_page, &expected_content_page);
        let adopted = store.adopted_roots().expect("original adopted roots");
        let before = original
            .write_cost_snapshot()
            .expect("before fixed-page probe");
        let partial = store
            .put_blob(&original, ObjectKind::Record, &[], &partial_bytes)
            .expect("already-written fixed intermediate page");
        let after = original
            .write_cost_snapshot()
            .expect("after fixed-page probe");
        assert_eq!(after.new_objects, before.new_objects);
        assert_eq!(after.new_encoded_bytes, before.new_encoded_bytes);
        assert!(adopted.contains(&partial.root));
        let partial_len = u64::try_from(partial_bytes.len()).expect("partial length");
        let decoded = immutable_block::ImmutableRunBlock::decode(
            store
                .read_blob(&partial, partial_len, None)
                .expect("authenticated intermediate block"),
        )
        .expect("intermediate block");
        assert_eq!(
            decoded
                .get(&expected_partial_page.key().expect("path key"))
                .expect("path lookup"),
            Some(expected_partial_page)
        );
        assert_eq!(
            decoded
                .get(&expected_content_page.key().expect("content key"))
                .expect("content lookup"),
            Some(expected_content_page)
        );
        let mut protected = leaves
            .iter()
            .map(|(_, reference)| reference.root.clone())
            .collect::<Vec<_>>();
        protected.push(final_ref.root.clone());
        assert_eq!(protected.len(), 67);
        let wrapper = store
            .put_blob(
                &original,
                ObjectKind::Record,
                &protected,
                b"gc-retained-new-run-and-source-leaves-v1",
            )
            .expect("custody wrapper");
        let mut next_binding = binding();
        next_binding.target_generation = 2;
        let retained = store
            .begin_attempt(&next_binding, std::slice::from_ref(&wrapper.root))
            .expect("independent retained attempt");
        original
            .discard_unsealed()
            .expect("retire original attempt");
        let outcome =
            collect_shared_pool(&store, &[], store.gc_epoch().expect("current epoch"), None)
                .expect("actual shared-pool collection");
        assert!(matches!(
            outcome,
            crate::lexical_gc::SharedPoolGcOutcome::Completed { .. }
        ));
        assert!(store.read_blob(&partial, partial_len, None).is_err());
        assert_eq!(
            RunPostingRoot::load(&store, &final_ref, None).expect("retained final root"),
            root
        );
        let (_block_ref, page) = root
            .page_at(
                &store,
                &run_page_key(1, *b"abc", 191).expect("bucket key"),
                None,
            )
            .expect("retained page lookup")
            .expect("retained final page");
        let expected_final = leaves
            .iter()
            .take(65)
            .map(|(id, reference)| RunPostingRow {
                logical_id: *id,
                segment_sha256: reference.root.sha256,
            })
            .collect::<Vec<_>>();
        assert_eq!(page.rows, expected_final);
        assert_eq!((root.path_rows, root.content_rows), (66, 66));
        for (id, source) in &leaves {
            let bytes = store
                .read_blob(
                    source,
                    u64::try_from(MAX_RECORD_BYTES).expect("record bound"),
                    None,
                )
                .expect("protected source bytes");
            let segment = Segment::decode(&bytes).expect("protected source segment");
            assert_eq!(segment.doc_ids, vec![*id]);
        }
        retained
            .discard_unsealed()
            .expect("retire retained attempt");
    }

    #[test]
    fn new_run_bootstrap_crosses_second_merge_pass_with_exact_rows() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let first = physical_segment(&store, &lease, 1);
        let template = Segment::decode(
            &store
                .read_blob(
                    &first,
                    u64::try_from(MAX_RECORD_BYTES).expect("record bound"),
                    None,
                )
                .expect("first segment bytes"),
        )
        .expect("first segment");
        let dependencies = [&template.path_terms, &template.content_terms]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect::<Vec<_>>();
        let mut leaves = vec![(1_u64, first)];
        for id in 2_u64..=1025 {
            let mut segment = template.clone();
            segment.doc_ids = vec![id];
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Segment,
                    &dependencies,
                    &segment.encode().expect("segment bytes"),
                )
                .expect("segment blob");
            leaves.push((id, reference));
        }
        let references = leaves
            .iter()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let (root, _reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            7,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("seventeen partial maps require two merge rounds");
        assert_eq!(root.logical_pages(), 10);
        assert_eq!(root.gram_counts.entries, 2);
        assert_eq!((root.path_rows, root.content_rows), (1025, 1025));
        let expected = leaves
            .iter()
            .map(|(id, reference)| (*id, reference.root.sha256))
            .collect::<BTreeMap<_, _>>();
        let mut seen = BTreeMap::<(u8, u64), [u8; 32]>::new();
        let mut cursor = root
            .page_cursor(&store, &[], None, None)
            .expect("page cursor");
        while let Some((key, _reference, page)) = cursor.next().expect("next page") {
            assert_eq!(page.key().expect("page key").as_slice(), key);
            for row in page.rows {
                assert_eq!(Some(&row.segment_sha256), expected.get(&row.logical_id));
                assert!(
                    seen.insert((page.surface, row.logical_id), row.segment_sha256)
                        .is_none()
                );
            }
        }
        for surface in [1_u8, 2] {
            for id in 1_u64..=1025 {
                assert_eq!(seen.get(&(surface, id)), expected.get(&id));
            }
        }
        assert_eq!(seen.len(), 2050);
    }

    #[test]
    fn new_run_bootstrap_accepts_empty_leaf_term_maps() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let empty = Segment {
            doc_ids: vec![1],
            path_terms: MapRoot::default(),
            content_terms: MapRoot::default(),
            path_memberships: 0,
            content_memberships: 0,
        };
        let empty_ref = store
            .put_blob(
                &lease,
                ObjectKind::Segment,
                &[],
                &empty.encode().expect("empty segment bytes"),
            )
            .expect("empty segment blob");
        let (empty_root, _reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &[&empty_ref],
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("empty run posting root");
        assert_eq!(
            (empty_root.pages.entries, empty_root.gram_counts.entries),
            (0, 0)
        );
        assert_eq!((empty_root.path_rows, empty_root.content_rows), (0, 0));
        let present_ref = physical_segment(&store, &lease, 2);
        let (mixed_root, _reference) = bootstrap_new_run_postings(
            &store,
            &lease,
            2,
            &[&empty_ref, &present_ref],
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("mixed empty and present leaves");
        assert_eq!((mixed_root.path_rows, mixed_root.content_rows), (1, 1));
        assert_eq!(mixed_root.logical_pages(), 2);
    }

    #[test]
    fn new_run_bootstrap_refuses_under_admission_before_writes() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let leaf = physical_segment(&store, &lease, 1);
        let before = lease.write_cost_snapshot().expect("before cost");
        let cap = new_run_stage_charge().expect("stage charge");
        assert!(cap <= super::super::super::policy().bucket_scratch_bytes);
        let below = cap.checked_sub(1).expect("positive stage charge");
        assert!(bootstrap_new_run_postings(&store, &lease, 1, &[&leaf], below).is_err());
        assert_eq!(lease.write_cost_snapshot().expect("after cost"), before);
    }

    #[test]
    fn new_run_bootstrap_refuses_duplicate_ids_across_chunks() {
        use crate::lexical_gc::collect_shared_pool;

        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let keeper = store
            .begin_attempt(&binding(), &[])
            .expect("retained attempt");
        let retained = store
            .put_blob(&keeper, ObjectKind::Record, &[], b"retained-prior")
            .expect("retained prior object");
        let mut failed_binding = binding();
        failed_binding.target_generation = 2;
        let lease = store
            .begin_attempt(&failed_binding, &[])
            .expect("begin attempt");
        let mut leaves = (192_u64..=255)
            .map(|id| physical_segment(&store, &lease, id))
            .collect::<Vec<_>>();
        let duplicate = leaves.first().expect("first leaf").clone();
        leaves.push(duplicate);
        let references = leaves.iter().collect::<Vec<_>>();
        let before = lease.write_cost_snapshot().expect("before bootstrap");
        let error = bootstrap_new_run_postings(
            &store,
            &lease,
            1,
            &references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect_err("duplicate must refuse final run");
        assert!(matches!(error, CoreError::Typed { code, .. }
            if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE));
        let after = lease
            .write_cost_snapshot()
            .expect("after refused bootstrap");
        assert!(after.new_objects > before.new_objects);
        let expected_partial_page = RunPostingPage::new(
            1,
            *b"abc",
            0,
            leaves
                .iter()
                .take(64)
                .enumerate()
                .map(|(index, leaf)| RunPostingRow {
                    logical_id: 192 + u64::try_from(index).expect("logical ID"),
                    segment_sha256: leaf.root.sha256,
                })
                .collect(),
        )
        .expect("fixed partial page");
        let expected_content_page =
            RunPostingPage::new(2, *b"xyz", 0, expected_partial_page.rows.clone())
                .expect("fixed partial content page");
        let expected_partial = two_page_block_bytes(&expected_partial_page, &expected_content_page);
        let partial = store
            .put_blob(&lease, ObjectKind::Record, &[], &expected_partial)
            .expect("already-installed intermediate");
        assert_eq!(
            lease
                .write_cost_snapshot()
                .expect("after lookup")
                .new_objects,
            after.new_objects
        );
        lease.discard_unsealed().expect("discard failed attempt");
        let outcome = collect_shared_pool(&store, &[], store.gc_epoch().expect("epoch"), None)
            .expect("collect failed-attempt objects");
        assert!(matches!(
            outcome,
            crate::lexical_gc::SharedPoolGcOutcome::Completed { .. }
        ));
        assert!(
            store
                .read_blob(
                    &partial,
                    u64::try_from(MAX_POSTING_PAGE_BYTES).expect("posting bound"),
                    None
                )
                .is_err()
        );
        for leaf in &leaves {
            assert!(
                store
                    .read_blob(
                        leaf,
                        u64::try_from(MAX_RECORD_BYTES).expect("segment bound"),
                        None
                    )
                    .is_err()
            );
        }
        assert_eq!(
            store
                .read_blob(&retained, 64, None)
                .expect("protected prior"),
            b"retained-prior"
        );
        keeper.discard_unsealed().expect("retire retained attempt");
    }

    #[test]
    fn authenticated_prior_page_is_reused_for_exact_delta_and_refuses_forgery() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("open store");
        let lease = store.begin_attempt(&binding(), &[]).expect("begin attempt");
        let key = run_page_key(2, *b"abc", 1).expect("bucket key").to_vec();
        let old = RunPostingPage::new(2, *b"abc", 0, vec![row(1, 1), row(3, 1)]).expect("old page");
        let (base, root_ref) = apply_run_page_edits(
            &store,
            &lease,
            None,
            1,
            BTreeMap::from([(
                key.clone(),
                RunPostingPageEdit {
                    prior: None,
                    next: Some(old),
                },
            )]),
        )
        .expect("initial page");
        assert!(!base.is_block_layout());
        assert_eq!(
            RunPostingRoot::load(&store, &root_ref, None).expect("v1 reload"),
            base
        );
        assert_eq!(base.path_rows, 0);
        assert_eq!(base.content_rows, 2);
        let old_ref = store
            .map_get(&base.pages, &key, None)
            .expect("old page lookup")
            .expect("old page reference");
        let authenticated = RunPostingPage::decode(
            &store
                .read_blob(
                    &old_ref,
                    u64::try_from(MAX_POSTING_PAGE_BYTES).expect("bound"),
                    None,
                )
                .expect("old page bytes"),
        )
        .expect("old page decode");
        let next = apply_page_delta(
            2,
            *b"abc",
            0,
            Some(&authenticated),
            &[row(1, 1)],
            &[row(2, 2)],
        )
        .expect("delta")
        .expect("target page");
        let (target, _root_ref) = apply_run_page_edits(
            &store,
            &lease,
            Some(&base),
            1,
            BTreeMap::from([(
                key.clone(),
                RunPostingPageEdit {
                    prior: Some((old_ref.clone(), authenticated.clone())),
                    next: Some(next.clone()),
                },
            )]),
        )
        .expect("exact changed page");
        assert_eq!(target.path_rows, 0);
        assert_eq!(target.content_rows, 2);
        let target_ref = store
            .map_get(&target.pages, &key, None)
            .expect("target page lookup")
            .expect("target page reference");
        assert_eq!(target.page_record_encoded_bytes, target_ref.root.bytes);
        assert_eq!(
            RunPostingPage::decode(
                &store
                    .read_blob(
                        &target_ref,
                        u64::try_from(MAX_POSTING_PAGE_BYTES).expect("bound"),
                        None
                    )
                    .expect("target page bytes")
            )
            .expect("target page decode")
            .rows,
            vec![row(2, 2), row(3, 1)]
        );
        let count_ref = store
            .map_get(
                &target.gram_counts,
                &run_page_prefix(2, *b"abc").expect("gram key"),
                None,
            )
            .expect("gram count lookup")
            .expect("gram count reference");
        assert_eq!(
            RunGramCount::decode(
                &store
                    .read_blob(
                        &count_ref,
                        u64::try_from(MAX_RECORD_BYTES).expect("bound"),
                        None
                    )
                    .expect("gram count bytes")
            )
            .expect("gram count decode")
            .count,
            2
        );

        let forged =
            RunPostingPage::new(2, *b"abc", 0, vec![row(1, 9), row(3, 1)]).expect("forged page");
        let wrong_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[],
                &forged.encode().expect("forged bytes"),
            )
            .expect("forged reference");
        for (reference, page) in [(wrong_ref, authenticated), (old_ref, forged)] {
            assert!(
                apply_run_page_edits(
                    &store,
                    &lease,
                    Some(&base),
                    1,
                    BTreeMap::from([(
                        key.clone(),
                        RunPostingPageEdit {
                            prior: Some((reference, page)),
                            next: Some(next.clone()),
                        },
                    )]),
                )
                .is_err()
            );
        }
        let wrong_key_page =
            RunPostingPage::new(2, *b"xyz", 0, vec![row(1, 1), row(3, 1)]).expect("wrong-key page");
        let wrong_key_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[],
                &wrong_key_page.encode().expect("wrong-key bytes"),
            )
            .expect("wrong-key reference");
        let mut bad_base = base.clone();
        bad_base.pages = store
            .map_apply(
                &lease,
                &base.pages,
                std::iter::once(Ok(MapEdit {
                    key: key.clone(),
                    value: Some(wrong_key_ref.clone()),
                })),
            )
            .expect("forged map value");
        assert!(
            apply_run_page_edits(
                &store,
                &lease,
                Some(&bad_base),
                1,
                BTreeMap::from([(
                    key,
                    RunPostingPageEdit {
                        prior: Some((wrong_key_ref, wrong_key_page)),
                        next: Some(next),
                    },
                )]),
            )
            .is_err()
        );
    }

    #[test]
    fn block_delta_replaces_only_touched_record_and_counts_physical_once() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let pages = (0_u64..512)
            .map(|index| {
                let bucket = index.checked_mul(2).expect("even bucket");
                RunPostingPage::new(
                    2,
                    *b"abc",
                    bucket,
                    vec![row(bucket.checked_mul(256).expect("ID"), 1)],
                )
                .expect("page")
            })
            .collect::<Vec<_>>();
        let built = block_directory::build_directory(
            &store,
            &lease,
            pages.iter().map(BlockPageInput::from_page),
        )
        .expect("block directory");
        assert!(built.root.entries > 1);
        let count = RunGramCount {
            surface: 2,
            gram: *b"abc",
            count: 512,
        };
        let count_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[],
                &count.encode().expect("count"),
            )
            .expect("count Record");
        let counts = store
            .map_bootstrap(
                &lease,
                std::iter::once(Ok(MapEdit {
                    key: count.key().expect("count key").to_vec(),
                    value: Some(count_ref.clone()),
                })),
            )
            .expect("count map");
        let base = RunPostingRoot {
            run_id: 1,
            pages: built.root,
            block_pages: Some(built.logical_pages),
            gram_counts: counts,
            path_rows: 0,
            content_rows: 512,
            page_record_encoded_bytes: built.record_encoded_bytes,
            gram_count_record_encoded_bytes: count_ref.root.bytes,
        };
        base.validate().expect("base root");
        let key = pages.first().expect("first page").key().expect("first key");
        let (old_ref, old_page) = base
            .page_at(&store, &key, None)
            .expect("old lookup")
            .expect("old page");
        let mut tail = store
            .map_cursor(&base.pages, &[], None, None)
            .expect("directory cursor");
        let (first_last_key, first_ref) = tail
            .next()
            .expect("first directory row")
            .expect("first physical block");
        let mut last = None;
        while let Some(entry) = tail.next().expect("directory next") {
            last = Some(entry);
        }
        let (last_key, last_ref) = last.expect("last block");
        assert_ne!(old_ref, last_ref);
        let replacement =
            RunPostingPage::new(2, *b"abc", 0, vec![row(0, 2)]).expect("replacement page");
        let (target, _) = apply_run_page_edits(
            &store,
            &lease,
            Some(&base),
            1,
            BTreeMap::from([(
                key.to_vec(),
                RunPostingPageEdit {
                    prior: Some((old_ref, old_page)),
                    next: Some(replacement.clone()),
                },
            )]),
        )
        .expect("block delta");
        assert!(target.is_block_layout());
        assert_eq!(target.logical_pages(), 512);
        assert_eq!(target.content_rows, 512);
        assert_eq!(
            store
                .map_get(&target.pages, &last_key, None)
                .expect("untouched lookup"),
            Some(last_ref)
        );
        assert_eq!(
            target
                .page_at(&store, &key, None)
                .expect("updated lookup")
                .expect("updated page")
                .1,
            replacement
        );
        let (logical, blocks, physical_bytes) = target.block_census(&store, None).expect("census");
        assert_eq!(logical, 512);
        assert_eq!(blocks, target.pages.entries);
        assert_eq!(physical_bytes, target.page_record_encoded_bytes);
        let preserved_first_ref = store
            .map_get(&target.pages, &first_last_key, None)
            .expect("target first block lookup")
            .expect("target first block");
        assert_ne!(preserved_first_ref, first_ref);

        // Insert into the key gap immediately after one physical boundary.
        // Lower-bound routing must rewrite the following block, preserving the
        // first block's exact authenticated object identity.
        let boundary_bucket = u64::from_be_bytes(
            first_last_key
                .get(4..12)
                .expect("bucket suffix")
                .try_into()
                .expect("bucket width"),
        );
        let gap_bucket = boundary_bucket.checked_add(1).expect("gap bucket");
        let gap_id = gap_bucket.checked_mul(256).expect("gap ID");
        let gap_key = run_page_key(2, *b"abc", gap_id).expect("gap key");
        assert!(
            target
                .page_at(&store, &gap_key, None)
                .expect("gap lookup")
                .is_none()
        );
        let inserted = RunPostingPage::new(2, *b"abc", gap_bucket, vec![row(gap_id, 3)])
            .expect("inserted gap page");
        let (with_gap, _) = apply_run_page_edits(
            &store,
            &lease,
            Some(&target),
            1,
            BTreeMap::from([(
                gap_key.to_vec(),
                RunPostingPageEdit {
                    prior: None,
                    next: Some(inserted.clone()),
                },
            )]),
        )
        .expect("gap insertion");
        assert_eq!(with_gap.logical_pages(), 513);
        assert_eq!(with_gap.content_rows, 513);
        assert_eq!(
            store
                .map_get(&with_gap.pages, &first_last_key, None)
                .expect("preserved boundary"),
            Some(preserved_first_ref)
        );
        assert_eq!(
            with_gap
                .page_at(&store, &gap_key, None)
                .expect("gap page")
                .expect("inserted page")
                .1,
            inserted
        );
        let prior_gap = with_gap
            .page_at(&store, &gap_key, None)
            .expect("prior gap lookup")
            .expect("prior gap page");
        let (without_gap, _) = apply_run_page_edits(
            &store,
            &lease,
            Some(&with_gap),
            1,
            BTreeMap::from([(
                gap_key.to_vec(),
                RunPostingPageEdit {
                    prior: Some(prior_gap),
                    next: None,
                },
            )]),
        )
        .expect("gap deletion");
        assert_eq!(without_gap.logical_pages(), 512);
        assert_eq!(without_gap.content_rows, 512);
        assert!(
            without_gap
                .page_at(&store, &gap_key, None)
                .expect("deleted gap")
                .is_none()
        );
        assert_eq!(
            without_gap
                .block_census(&store, None)
                .expect("deleted census")
                .2,
            without_gap.page_record_encoded_bytes
        );
    }

    #[test]
    fn fixed_keys_and_bucket_boundary() {
        assert_eq!(
            run_page_key(2, *b"abc", 255).expect("first bucket"),
            [2, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            run_page_key(2, *b"abc", 256).expect("second bucket"),
            [2, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            run_route_key(1, *b"xyz", 3, 7).expect("route"),
            [
                1, b'x', b'y', b'z', 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 7,
            ]
        );
        let count = RunGramCount {
            surface: 2,
            gram: *b"abc",
            count: 513,
        };
        assert_eq!(count.key().expect("count key"), [2, b'a', b'b', b'c']);
        assert_eq!(
            RunGramCount::decode(&count.encode().expect("count bytes")).expect("count decode"),
            count
        );
        assert!(RunGramCount { count: 0, ..count }.encode().is_err());
    }

    #[test]
    fn canonical_page_and_exact_independent_union() {
        let first =
            RunPostingPage::new(2, *b"abc", 1, vec![row(256, 1), row(259, 1)]).expect("first page");
        let second = RunPostingPage::new(2, *b"abc", 1, vec![row(257, 2)]).expect("second page");
        let merged = merge_page_bucket(&[&first, &second]).expect("merged page");
        assert_eq!(merged.rows, vec![row(256, 1), row(257, 2), row(259, 1)]);
        verify_page_union(&[&first, &second], &merged).expect("independent union");
        let bytes = merged.encode().expect("encoded page");
        assert_eq!(
            RunPostingPage::decode(&bytes).expect("decoded page"),
            merged
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert!(RunPostingPage::decode(&trailing).is_err());
        let missing = RunPostingPage::new(2, *b"abc", 1, vec![row(256, 1), row(259, 1)])
            .expect("missing row page");
        assert!(verify_page_union(&[&first, &second], &missing).is_err());
        let extra = RunPostingPage::new(
            2,
            *b"abc",
            1,
            vec![row(256, 1), row(257, 2), row(258, 3), row(259, 1)],
        )
        .expect("extra row page");
        assert!(verify_page_union(&[&first, &second], &extra).is_err());
    }

    #[test]
    fn duplicate_ids_and_wrong_locator_are_refused() {
        let first = RunPostingPage::new(1, *b"abc", 0, vec![row(1, 1)]).expect("first page");
        let duplicate =
            RunPostingPage::new(1, *b"abc", 0, vec![row(1, 2)]).expect("duplicate page");
        assert!(merge_page_bucket(&[&first, &duplicate]).is_err());
        let forged =
            RunPostingPage::new(1, *b"abc", 0, vec![row(1, 3)]).expect("forged locator page");
        assert!(verify_page_union(&[&first], &forged).is_err());
        assert!(RunPostingPage::new(1, *b"abc", 0, vec![row(1, 1), row(1, 1)]).is_err());
        assert!(RunPostingPage::new(1, *b"abc", 0, vec![row(256, 1)]).is_err());
    }

    #[test]
    fn changed_leaf_delta_updates_only_its_bucket() {
        let old =
            RunPostingPage::new(2, *b"abc", 0, vec![row(1, 1), row(3, 1)]).expect("base page");
        let target = apply_page_delta(2, *b"abc", 0, Some(&old), &[row(1, 1)], &[row(2, 2)])
            .expect("changed bucket")
            .expect("nonempty target");
        assert_eq!(target.rows, vec![row(2, 2), row(3, 1)]);
        assert!(apply_page_delta(2, *b"abc", 0, Some(&old), &[row(1, 2)], &[]).is_err());
        assert!(apply_page_delta(2, *b"abc", 0, Some(&old), &[], &[row(3, 2)]).is_err());
        assert!(apply_page_delta(2, *b"abc", 0, Some(&old), &[], &[row(256, 2)]).is_err());
        assert!(
            apply_page_delta(2, *b"abc", 0, Some(&old), &[row(1, 1), row(3, 1)], &[])
                .expect("all retired")
                .is_none()
        );
    }
}
