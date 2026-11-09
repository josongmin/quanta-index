//! Immutable File run block directory.
//!
//! Each map key is the exact last posting key in one authenticated Record
//! block. A lower-bound map cursor therefore selects the only possible block
//! for a point key. Range walks include the boundary-crossing block and stop
//! at the first page at or above the exclusive upper bound.

use quanta_index_core::{CoreError, RequestBudgetV1};

use super::immutable_block::{
    ImmutableRunBlock, ImmutableRunBlockBuilder, MAX_BLOCK_BYTES, PushResult,
};
use super::{RunPostingPage, corrupt, invalid};
use crate::lexical_store::{
    AttemptLease, BlobRef, MapCursor, MapEdit, MapRoot, ObjectKind, ObjectStore,
};

/// Canonical bytes minted from a validated logical page.
pub(super) struct BlockPageInput {
    key: [u8; 12],
    encoded: Vec<u8>,
}

impl BlockPageInput {
    pub(super) fn from_page(page: &RunPostingPage) -> Result<Self, CoreError> {
        Ok(Self {
            key: page.key()?,
            encoded: page.encode()?,
        })
    }
}

pub(super) struct BlockDirectoryBuild {
    pub(super) root: MapRoot,
    pub(super) logical_pages: u64,
    /// Sum of one authenticated physical Record root width per block entry.
    pub(super) record_encoded_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BlockDirectoryCensus {
    pub(super) logical_pages: u64,
    pub(super) blocks: u64,
    pub(super) record_encoded_bytes: u64,
}

struct BlockEdits<'a, I> {
    store: &'a ObjectStore,
    lease: &'a AttemptLease,
    source: I,
    builder: ImmutableRunBlockBuilder,
    pending: Option<BlockPageInput>,
    prior_last: Option<[u8; 12]>,
    logical_pages: u64,
    blocks: u64,
    record_encoded_bytes: u64,
    ended: bool,
    failed: bool,
}

impl<I: Iterator<Item = Result<BlockPageInput, CoreError>>> BlockEdits<'_, I> {
    fn publish(&mut self, builder: ImmutableRunBlockBuilder) -> Result<MapEdit, CoreError> {
        let first = builder
            .first_key()
            .ok_or_else(|| invalid("block directory tried to publish an empty block"))?;
        let last = builder
            .last_key()
            .ok_or_else(|| invalid("block directory last key is absent"))?;
        if self.prior_last.is_some_and(|prior| prior >= first) {
            return Err(invalid("block directory ranges overlap or reorder"));
        }
        let bytes = builder.finish()?;
        let reference = self
            .store
            .put_blob(self.lease, ObjectKind::Record, &[], &bytes)?;
        self.blocks = self
            .blocks
            .checked_add(1)
            .ok_or_else(|| invalid("block directory block count overflows"))?;
        self.record_encoded_bytes = self
            .record_encoded_bytes
            .checked_add(reference.root.bytes)
            .ok_or_else(|| invalid("block directory Record byte ledger overflows"))?;
        self.prior_last = Some(last);
        Ok(MapEdit {
            key: last.to_vec(),
            value: Some(reference),
        })
    }

    fn next_edit(&mut self) -> Result<Option<MapEdit>, CoreError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            let row = match self.pending.take() {
                Some(row) => Some(row),
                None => self.source.next().transpose()?,
            };
            let Some(row) = row else {
                self.ended = true;
                if self.builder.is_empty() {
                    return Ok(None);
                }
                let builder = std::mem::replace(&mut self.builder, ImmutableRunBlockBuilder::new());
                return self.publish(builder).map(Some);
            };
            if self.prior_last.is_some_and(|prior| prior >= row.key) {
                return Err(invalid("block directory input reorders completed blocks"));
            }
            match self.builder.try_push_canonical(row.key, &row.encoded)? {
                PushResult::Added => {
                    self.logical_pages = self
                        .logical_pages
                        .checked_add(1)
                        .ok_or_else(|| invalid("block directory page count overflows"))?;
                }
                PushResult::Full => {
                    self.pending = Some(row);
                    let builder =
                        std::mem::replace(&mut self.builder, ImmutableRunBlockBuilder::new());
                    return self.publish(builder).map(Some);
                }
            }
        }
    }
}

impl<I: Iterator<Item = Result<BlockPageInput, CoreError>>> Iterator for BlockEdits<'_, I> {
    type Item = Result<MapEdit, CoreError>;

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

/// Publishes dependency-free Record blobs and a sorted last-key directory.
/// The caller's attempt lease remains responsible for publication custody.
pub(super) fn build_directory(
    store: &ObjectStore,
    lease: &AttemptLease,
    pages: impl Iterator<Item = Result<BlockPageInput, CoreError>>,
) -> Result<BlockDirectoryBuild, CoreError> {
    let mut edits = BlockEdits {
        store,
        lease,
        source: pages,
        builder: ImmutableRunBlockBuilder::new(),
        pending: None,
        prior_last: None,
        logical_pages: 0,
        blocks: 0,
        record_encoded_bytes: 0,
        ended: false,
        failed: false,
    };
    let root = store.map_bootstrap(lease, &mut edits)?;
    if edits.blocks != root.entries || (edits.logical_pages == 0) != (edits.blocks == 0) {
        return Err(invalid("block directory page or block count differs"));
    }
    Ok(BlockDirectoryBuild {
        root,
        logical_pages: edits.logical_pages,
        record_encoded_bytes: edits.record_encoded_bytes,
    })
}

/// Builds replacement block entries without installing an intermediate map.
/// Callers apply the returned edits to a prior directory after removing the
/// touched old block keys. Record bytes count each replacement exactly once.
pub(super) fn build_replacement_edits(
    store: &ObjectStore,
    lease: &AttemptLease,
    pages: impl Iterator<Item = Result<BlockPageInput, CoreError>>,
) -> Result<(Vec<MapEdit>, u64, u64), CoreError> {
    let mut source = BlockEdits {
        store,
        lease,
        source: pages,
        builder: ImmutableRunBlockBuilder::new(),
        pending: None,
        prior_last: None,
        logical_pages: 0,
        blocks: 0,
        record_encoded_bytes: 0,
        ended: false,
        failed: false,
    };
    let mut edits = Vec::new();
    for edit in &mut source {
        edits.push(edit?);
    }
    Ok((edits, source.logical_pages, source.record_encoded_bytes))
}

pub(super) fn load_block(
    store: &ObjectStore,
    directory_key: &[u8],
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<ImmutableRunBlock, CoreError> {
    let last: [u8; 12] = directory_key
        .try_into()
        .map_err(|_width| corrupt("block directory last-key width differs"))?;
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("block directory value is not a Record"));
    }
    if !store
        .object_dependencies(&reference.root, budget)?
        .is_empty()
    {
        return Err(corrupt("block directory Record has dependencies"));
    }
    let max_bytes = u64::try_from(MAX_BLOCK_BYTES)
        .map_err(|_width| corrupt("block byte admission exceeds u64"))?;
    let block = ImmutableRunBlock::decode(store.read_blob(reference, max_bytes, budget)?)?;
    if block.last_key()? != last {
        return Err(corrupt("block directory key differs from block last key"));
    }
    Ok(block)
}

/// Exact lookup: one lower-bound directory entry and one selected page decode.
pub(super) fn read_page(
    store: &ObjectStore,
    directory: &MapRoot,
    key: &[u8; 12],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(BlobRef, RunPostingPage)>, CoreError> {
    let mut cursor = store.map_cursor(directory, key, None, budget)?;
    let Some((last, reference)) = cursor.next()? else {
        return Ok(None);
    };
    let block = load_block(store, &last, &reference, budget)?;
    block
        .get(key)
        .map(|page| page.map(|page| (reference, page)))
}

/// One decoded block and one ordered map cursor remain live. A complete walk
/// is the seal's input; that seal must still compare all pages with leaf facts.
pub(in crate::file_authority::paged) struct BlockDirectoryCursor<'a> {
    store: &'a ObjectStore,
    budget: Option<&'a RequestBudgetV1>,
    cursor: MapCursor<'a>,
    lower: [u8; 12],
    upper: Option<[u8; 12]>,
    block: Option<ImmutableRunBlock>,
    active_ref: Option<BlobRef>,
    active_first: Option<[u8; 12]>,
    page_index: usize,
    prior_last: Option<[u8; 12]>,
    census: BlockDirectoryCensus,
    ended: bool,
}

impl<'a> BlockDirectoryCursor<'a> {
    pub(super) fn new(
        store: &'a ObjectStore,
        directory: &MapRoot,
        lower: [u8; 12],
        upper: Option<[u8; 12]>,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        if upper.is_some_and(|bound| bound <= lower) {
            return Err(invalid("block directory upper bound is not above lower"));
        }
        // Do not set the map's upper bound: the block whose last key crosses
        // it contains the final eligible pages and must be read.
        let cursor = store.map_cursor(directory, &lower, None, budget)?;
        Ok(Self {
            store,
            budget,
            cursor,
            lower,
            upper,
            block: None,
            active_ref: None,
            active_first: None,
            page_index: 0,
            prior_last: None,
            census: BlockDirectoryCensus {
                logical_pages: 0,
                blocks: 0,
                record_encoded_bytes: 0,
            },
            ended: false,
        })
    }

    /// Available only after an exhaustive unbounded walk. This independently
    /// recomputes the producer's page count and per-block physical byte ledger.
    #[cfg(test)]
    pub(super) fn complete_census(&self) -> Result<BlockDirectoryCensus, CoreError> {
        if !self.ended || self.lower != [0; 12] || self.upper.is_some() {
            return Err(invalid("block directory census requires a complete walk"));
        }
        Ok(self.census)
    }

    pub(super) fn active_block_first_key(&self) -> Option<[u8; 12]> {
        self.active_first
    }

    #[cfg(test)]
    pub(super) fn next(
        &mut self,
    ) -> Result<Option<([u8; 12], BlobRef, RunPostingPage)>, CoreError> {
        self.next_with(&mut || Ok(()))
    }

    pub(super) fn next_with(
        &mut self,
        probe: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<Option<([u8; 12], BlobRef, RunPostingPage)>, CoreError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            if let Some(block) = &self.block {
                if let Some(key) = block.key_at(self.page_index) {
                    if self.upper.is_some_and(|upper| key >= upper) {
                        self.ended = true;
                        return Ok(None);
                    }
                    probe()?;
                    let selected = block
                        .read_at(self.page_index)?
                        .ok_or_else(|| corrupt("block directory selected page absent"))?;
                    self.page_index = self
                        .page_index
                        .checked_add(1)
                        .ok_or_else(|| corrupt("block directory page index overflows"))?;
                    self.census.logical_pages =
                        self.census.logical_pages.checked_add(1).ok_or_else(|| {
                            corrupt("block directory census page count overflows")
                        })?;
                    let reference = self
                        .active_ref
                        .clone()
                        .ok_or_else(|| corrupt("block directory active reference absent"))?;
                    return Ok(Some((selected.0, reference, selected.1)));
                }
                self.block = None;
                self.active_ref = None;
                self.active_first = None;
            }
            let Some((last, reference)) = self.cursor.next()? else {
                self.ended = true;
                return Ok(None);
            };
            let block = load_block(self.store, &last, &reference, self.budget)?;
            let first = block.first_key()?;
            let last = block.last_key()?;
            if self.prior_last.is_some_and(|prior| prior >= first) {
                return Err(corrupt("block directory ranges overlap or reorder"));
            }
            self.census.blocks = self
                .census
                .blocks
                .checked_add(1)
                .ok_or_else(|| corrupt("block directory census block count overflows"))?;
            self.census.record_encoded_bytes = self
                .census
                .record_encoded_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| corrupt("block directory census Record bytes overflow"))?;
            self.prior_last = Some(last);
            if self.upper.is_some_and(|upper| first >= upper) {
                self.ended = true;
                return Ok(None);
            }
            self.page_index = block.lower_bound(&self.lower);
            self.active_ref = Some(reference);
            self.active_first = Some(first);
            self.block = Some(block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_authority::paged::run_postings::RunPostingRow;
    use crate::lexical_store::AttemptBinding;

    fn page(gram: [u8; 3], id: u64) -> RunPostingPage {
        RunPostingPage::new(
            2,
            gram,
            id >> 8,
            vec![RunPostingRow {
                logical_id: id,
                segment_sha256: [9; 32],
            }],
        )
        .expect("page")
    }

    #[test]
    fn cold_directory_point_and_crossing_range_are_exact() {
        let temp = tempfile::tempdir().expect("directory");
        let store = ObjectStore::open(temp.path()).expect("store");
        let binding = AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 1,
        };
        let lease = store.begin_attempt(&binding, &[]).expect("attempt");
        let pages = (0_u64..512)
            .map(|id| page(*b"abc", id.checked_mul(256).expect("ID")))
            .collect::<Vec<_>>();
        let build = build_directory(
            &store,
            &lease,
            pages.iter().map(|page| BlockPageInput::from_page(page)),
        )
        .expect("directory");
        assert_eq!(build.logical_pages, 512);
        assert!(build.root.entries > 1);
        let cold = ObjectStore::open(temp.path()).expect("cold store");
        let mut rows = cold
            .map_cursor(&build.root, &[], None, None)
            .expect("map cursor");
        let mut physical_bytes = 0_u64;
        let (first_last_key, first_reference) = rows.next().expect("first row").expect("block");
        physical_bytes = physical_bytes
            .checked_add(first_reference.root.bytes)
            .expect("first physical ledger");
        while let Some((_key, reference)) = rows.next().expect("row") {
            physical_bytes = physical_bytes
                .checked_add(reference.root.bytes)
                .expect("physical ledger");
        }
        assert_eq!(physical_bytes, build.record_encoded_bytes);
        let target = pages.get(300).expect("target");
        assert_eq!(
            read_page(&cold, &build.root, &target.key().expect("key"), None)
                .expect("point")
                .map(|(_reference, page)| page),
            Some(target.clone())
        );
        let missing = page(*b"abd", 0).key().expect("key");
        assert!(
            read_page(&cold, &build.root, &missing, None)
                .expect("missing")
                .is_none()
        );
        let boundary = pages
            .iter()
            .position(|page| page.key().expect("key").as_slice() == first_last_key)
            .expect("first directory boundary");
        let lower_index = boundary.checked_sub(1).expect("left of boundary");
        let upper_index = boundary.checked_add(2).expect("right of boundary");
        let lower = pages.get(lower_index).expect("lower").key().expect("key");
        let upper = pages.get(upper_index).expect("upper").key().expect("key");
        let mut cursor =
            BlockDirectoryCursor::new(&cold, &build.root, lower, Some(upper), None).expect("range");
        let mut observed = Vec::new();
        while let Some((key, _reference, _page)) = cursor.next().expect("next") {
            observed.push(key);
        }
        let expected = pages
            .get(lower_index..upper_index)
            .expect("expected boundary pages")
            .iter()
            .map(|page| page.key().expect("key"))
            .collect::<Vec<_>>();
        assert_eq!(observed, expected);
        let mut full = BlockDirectoryCursor::new(&cold, &build.root, [0; 12], None, None)
            .expect("full cursor");
        let mut full_keys = Vec::new();
        while let Some((key, _reference, _page)) = full.next().expect("full next") {
            full_keys.push(key);
        }
        assert_eq!(
            full_keys,
            pages
                .iter()
                .map(|page| page.key().expect("key"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            full.complete_census().expect("complete census"),
            BlockDirectoryCensus {
                logical_pages: build.logical_pages,
                blocks: build.root.entries,
                record_encoded_bytes: build.record_encoded_bytes,
            }
        );
        assert!(cursor.complete_census().is_err());

        let first_page_key = pages.first().expect("first page").key().expect("key");
        let wrong_root = store
            .map_bootstrap(
                &lease,
                std::iter::once(Ok(MapEdit {
                    key: first_page_key.to_vec(),
                    value: Some(first_reference),
                })),
            )
            .expect("forged directory");
        assert!(read_page(&cold, &wrong_root, &first_page_key, None).is_err());

        let left = pages.get(0..2).expect("left overlap pages");
        let right = pages.get(1..3).expect("right overlap pages");
        let mut overlapping_edits = Vec::new();
        for group in [left, right] {
            let mut builder = ImmutableRunBlockBuilder::new();
            for page in group {
                assert!(matches!(
                    builder.try_push(page.key().expect("key"), &page.encode().expect("page")),
                    Ok(PushResult::Added)
                ));
            }
            let last = builder.last_key().expect("last");
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Record,
                    &[],
                    &builder.finish().expect("block"),
                )
                .expect("Record");
            overlapping_edits.push(Ok(MapEdit {
                key: last.to_vec(),
                value: Some(reference),
            }));
        }
        let overlapping_root = store
            .map_bootstrap(&lease, overlapping_edits.into_iter())
            .expect("overlap directory");
        let mut overlapping =
            BlockDirectoryCursor::new(&cold, &overlapping_root, [0; 12], None, None)
                .expect("overlap cursor");
        assert!(overlapping.next().expect("first").is_some());
        assert!(overlapping.next().expect("second").is_some());
        assert!(overlapping.next().is_err());
    }
}
