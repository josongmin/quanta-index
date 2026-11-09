//! Independent run-posting proof from authenticated segment term masks and
//! sorted map cursors. Producer page-edit and merge routines are not used.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};

use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::lexical_runs::{SegmentInput, SegmentRemoval};
use crate::lexical_store::{BlobRef, MapCursor, MapRoot, ObjectKind, ObjectStore};

use super::run_postings::{
    RunBlockPages, RunGramCount, RunPostingPage, RunPostingRoot, run_page_key, run_route_key,
};
use super::{AuthorityRoot, MAX_POSTING_PAGE_BYTES, MAX_RECORD_BYTES, PostingPage, Segment};

type ChangedMapRow = (Vec<u8>, Option<BlobRef>, Option<BlobRef>);

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file run proof: {reason}"),
    }
}

fn checkpoint(budget: Option<&RequestBudgetV1>) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint("file:run:verify")?;
    }
    Ok(())
}

fn checked_rows(rows: usize) -> Result<u64, CoreError> {
    u64::try_from(rows).map_err(|_width| corrupt("posting row count exceeds u64"))
}

fn gram_key(key: &[u8]) -> Result<[u8; 4], CoreError> {
    key.get(..4)
        .ok_or_else(|| corrupt("run page key omits gram"))?
        .try_into()
        .map_err(|_width| corrupt("run gram key width differs"))
}

fn read_run_page(
    store: &ObjectStore,
    key: &[u8],
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<RunPostingPage, CoreError> {
    if key.len() != 12 || reference.root.kind != ObjectKind::Record {
        return Err(corrupt("run posting page key or object kind differs"));
    }
    let page = RunPostingPage::decode(&store.read_blob(
        reference,
        u64::try_from(MAX_POSTING_PAGE_BYTES).map_err(|_width| corrupt("page limit width"))?,
        budget,
    )?)?;
    if page.key()?.as_slice() != key
        || !store
            .object_dependencies(&reference.root, budget)?
            .is_empty()
    {
        return Err(corrupt("run posting page key or dependencies differ"));
    }
    Ok(page)
}

fn read_count(
    store: &ObjectStore,
    key: &[u8],
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<RunGramCount, CoreError> {
    if key.len() != 4 || reference.root.kind != ObjectKind::Record {
        return Err(corrupt("run gram-count key or object kind differs"));
    }
    let count = RunGramCount::decode(&store.read_blob(
        reference,
        u64::try_from(MAX_RECORD_BYTES).map_err(|_width| corrupt("count limit width"))?,
        budget,
    )?)?;
    if count.key()?.as_slice() != key
        || !store
            .object_dependencies(&reference.root, budget)?
            .is_empty()
    {
        return Err(corrupt("run gram-count key or dependencies differ"));
    }
    Ok(count)
}

fn verify_root_reference(
    store: &ObjectStore,
    expected_id: u64,
    root: &RunPostingRoot,
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    if root.run_id != expected_id || RunPostingRoot::load(store, reference, budget)? != *root {
        return Err(corrupt("run posting root reference or ID differs"));
    }
    let mut expected_deps = root.object_roots();
    expected_deps.sort();
    expected_deps.dedup();
    if store.object_dependencies(&reference.root, budget)? != expected_deps {
        return Err(corrupt("run posting root dependency inventory differs"));
    }
    Ok(())
}

fn verify_bundle_structure(
    store: &ObjectStore,
    root: &RunPostingRoot,
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<u64, CoreError> {
    let pages = store.map_structure_encoded_bytes(&root.pages, budget)?;
    let counts = store.map_structure_encoded_bytes(&root.gram_counts, budget)?;
    reference
        .root
        .bytes
        .checked_add(root.page_record_encoded_bytes)
        .and_then(|sum| sum.checked_add(root.gram_count_record_encoded_bytes))
        .and_then(|sum| sum.checked_add(pages))
        .and_then(|sum| sum.checked_add(counts))
        .ok_or_else(|| corrupt("run bundle encoded bytes overflow"))
}

fn optional_run<'a>(
    store: &ObjectStore,
    run_id: u64,
    value: Option<(&'a RunPostingRoot, &'a BlobRef)>,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<&'a RunPostingRoot>, CoreError> {
    if let Some((root, reference)) = value {
        verify_root_reference(store, run_id, root, reference, budget)?;
        let _bundle = verify_bundle_structure(store, root, reference, budget)?;
        Ok(Some(root))
    } else {
        Ok(None)
    }
}

fn load_segment(
    store: &ObjectStore,
    key: &[u8],
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<(Segment, [u8; 32]), CoreError> {
    let digest: [u8; 32] = key
        .try_into()
        .map_err(|_width| corrupt("segment key width differs"))?;
    if reference.root.kind != ObjectKind::Segment || reference.root.sha256 != digest {
        return Err(corrupt("changed segment key or object kind differs"));
    }
    let segment = Segment::decode(&store.read_blob(
        reference,
        u64::try_from(MAX_RECORD_BYTES).map_err(|_width| corrupt("segment limit width"))?,
        budget,
    )?)?;
    let mut roots = [
        segment.path_terms.page.as_ref(),
        segment.content_terms.page.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(|page| page.object.clone())
    .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    if store.object_dependencies(&reference.root, budget)? != roots {
        return Err(corrupt("changed segment dependency inventory differs"));
    }
    Ok((segment, digest))
}

struct SegmentTerms<'a> {
    store: &'a ObjectStore,
    budget: Option<&'a RequestBudgetV1>,
    segment: Segment,
    digest: [u8; 32],
    removed: bool,
    surface: u8,
    cursor: MapCursor<'a>,
    cached: Option<(BlobRef, PostingPage, usize)>,
    pending_gram: [u8; 3],
    pending_mask: u64,
    observed: [u64; 2],
    finished: bool,
}

impl<'a> SegmentTerms<'a> {
    fn new(
        store: &'a ObjectStore,
        segment: Segment,
        digest: [u8; 32],
        removed: bool,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        let cursor = store.map_cursor(&segment.path_terms, &[], None, budget)?;
        Ok(Self {
            store,
            budget,
            segment,
            digest,
            removed,
            surface: 1,
            cursor,
            cached: None,
            pending_gram: [0; 3],
            pending_mask: 0,
            observed: [0; 2],
            finished: false,
        })
    }

    fn next(&mut self) -> Result<Option<([u8; 12], u64)>, CoreError> {
        loop {
            checkpoint(self.budget)?;
            if self.pending_mask != 0 {
                let local = usize::try_from(self.pending_mask.trailing_zeros())
                    .map_err(|_width| corrupt("leaf local ID width differs"))?;
                self.pending_mask &= self.pending_mask.wrapping_sub(1);
                let id = *self
                    .segment
                    .doc_ids
                    .get(local)
                    .ok_or_else(|| corrupt("leaf term mask exceeds segment"))?;
                // The segment decoder proves increasing logical IDs, so set
                // bits in local-ID order also give increasing bucket order.
                return Ok(Some((
                    run_page_key(self.surface, self.pending_gram, id)?,
                    id,
                )));
            }
            if self.finished {
                return Ok(None);
            }
            let Some((key, reference)) = self.cursor.next()? else {
                if self
                    .cached
                    .as_ref()
                    .is_some_and(|(_, page, seen)| *seen != page.0.len())
                {
                    return Err(corrupt("leaf term map omits gram from final page"));
                }
                if self.surface == 1 {
                    self.surface = 2;
                    self.cursor = self.store.map_cursor(
                        &self.segment.content_terms,
                        &[],
                        None,
                        self.budget,
                    )?;
                    self.cached = None;
                    continue;
                }
                if self.observed
                    != [
                        self.segment.path_memberships,
                        self.segment.content_memberships,
                    ]
                {
                    return Err(corrupt(
                        "changed segment term masks differ from membership totals",
                    ));
                }
                self.finished = true;
                return Ok(None);
            };
            let gram: [u8; 3] = key
                .as_slice()
                .try_into()
                .map_err(|_width| corrupt("leaf term key width differs"))?;
            if reference.root.kind != ObjectKind::FilePostings {
                return Err(corrupt("leaf term page kind differs"));
            }
            if self
                .cached
                .as_ref()
                .is_none_or(|(cached, _, _)| cached != &reference)
            {
                if self
                    .cached
                    .as_ref()
                    .is_some_and(|(_, page, seen)| *seen != page.0.len())
                {
                    return Err(corrupt("leaf term map omits gram from prior page"));
                }
                let page = PostingPage::decode(
                    &self.store.read_blob(
                        &reference,
                        u64::try_from(MAX_POSTING_PAGE_BYTES)
                            .map_err(|_width| corrupt("leaf page limit width"))?,
                        self.budget,
                    )?,
                    self.segment.doc_ids.len(),
                )?;
                if !self
                    .store
                    .object_dependencies(&reference.root, self.budget)?
                    .is_empty()
                {
                    return Err(corrupt("leaf posting page has dependencies"));
                }
                self.cached = Some((reference, page, 0));
            }
            let (_, page, seen) = self
                .cached
                .as_mut()
                .ok_or_else(|| corrupt("leaf page absent"))?;
            let (observed_gram, mask) = *page
                .0
                .get(*seen)
                .ok_or_else(|| corrupt("leaf term offset absent"))?;
            if observed_gram != gram {
                return Err(corrupt("leaf term map order differs from page terms"));
            }
            *seen = (*seen)
                .checked_add(1)
                .ok_or_else(|| corrupt("leaf page cursor overflows"))?;
            let surface_index = match self.surface {
                1 => 0,
                2 => 1,
                _ => return Err(corrupt("leaf surface differs")),
            };
            let observed = self
                .observed
                .get_mut(surface_index)
                .ok_or_else(|| corrupt("leaf surface slot absent"))?;
            *observed = observed
                .checked_add(u64::from(mask.count_ones()))
                .ok_or_else(|| corrupt("leaf membership count overflow"))?;
            self.pending_gram = gram;
            self.pending_mask = mask;
        }
    }
}

struct BucketDelta {
    key: [u8; 12],
    removed: BTreeMap<u64, [u8; 32]>,
    added: BTreeMap<u64, [u8; 32]>,
}

struct ChangedBuckets<'a> {
    streams: Vec<SegmentTerms<'a>>,
    ready: BinaryHeap<Reverse<([u8; 12], usize, u64)>>,
}

impl<'a> ChangedBuckets<'a> {
    fn new(
        store: &'a ObjectStore,
        removed: &[SegmentRemoval],
        added: &[SegmentInput],
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        let mut streams = Vec::new();
        for row in removed {
            let (segment, digest) = load_segment(store, &row.key, &row.reference, budget)?;
            streams.push(SegmentTerms::new(store, segment, digest, true, budget)?);
        }
        for row in added {
            let (segment, digest) = load_segment(store, &row.key, &row.reference, budget)?;
            streams.push(SegmentTerms::new(store, segment, digest, false, budget)?);
        }
        let mut ready = BinaryHeap::with_capacity(streams.len());
        for (index, stream) in streams.iter_mut().enumerate() {
            if let Some((key, id)) = stream.next()? {
                ready.push(Reverse((key, index, id)));
            }
        }
        Ok(Self { streams, ready })
    }

    fn next(&mut self) -> Result<Option<BucketDelta>, CoreError> {
        let Some(&Reverse((key, _, _))) = self.ready.peek() else {
            return Ok(None);
        };
        let mut delta = BucketDelta {
            key,
            removed: BTreeMap::new(),
            added: BTreeMap::new(),
        };
        while matches!(self.ready.peek(), Some(Reverse((head, _, _))) if *head == key) {
            let Reverse((_, index, first_id)) = self
                .ready
                .pop()
                .ok_or_else(|| corrupt("changed bucket head absent"))?;
            let stream = self
                .streams
                .get_mut(index)
                .ok_or_else(|| corrupt("changed bucket stream absent"))?;
            let side = if stream.removed {
                &mut delta.removed
            } else {
                &mut delta.added
            };
            let mut id = first_id;
            loop {
                if side.insert(id, stream.digest).is_some() {
                    return Err(corrupt(
                        "changed leaves repeat a logical ID within a bucket",
                    ));
                }
                let Some((next_key, next_id)) = stream.next()? else {
                    break;
                };
                if next_key != key {
                    self.ready.push(Reverse((next_key, index, next_id)));
                    break;
                }
                id = next_id;
            }
        }
        Ok(Some(delta))
    }
}

fn page_at(
    store: &ObjectStore,
    map: &MapRoot,
    key: &[u8],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(BlobRef, RunPostingPage)>, CoreError> {
    store
        .map_get(map, key, budget)?
        .map(|reference| {
            let page = read_run_page(store, key, &reference, budget)?;
            Ok((reference, page))
        })
        .transpose()
}

fn count_at(
    store: &ObjectStore,
    map: &MapRoot,
    key: &[u8],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(BlobRef, RunGramCount)>, CoreError> {
    store
        .map_get(map, key, budget)?
        .map(|reference| {
            let count = read_count(store, key, &reference, budget)?;
            Ok((reference, count))
        })
        .transpose()
}

struct ChangedPageProof<'a> {
    store: &'a ObjectStore,
    budget: Option<&'a RequestBudgetV1>,
    old: MapRoot,
    target: MapRoot,
    buckets: ChangedBuckets<'a>,
    rows: [u64; 2],
    record_bytes: u64,
    fresh_pages: Option<MapCursor<'a>>,
    fresh_counts: Option<FreshCountProof<'a>>,
}

/// A fresh run has no inherited count rows. Consume its complete count map in
/// gram order while the independently checked changed buckets are still live.
/// One gram total and one authenticated map cursor replace a second leaf scan.
struct FreshCountProof<'a> {
    cursor: MapCursor<'a>,
    ready: Option<(Vec<u8>, BlobRef)>,
    gram: Option<[u8; 4]>,
    rows: u64,
    record_bytes: u64,
}

impl<'a> FreshCountProof<'a> {
    fn new(
        store: &'a ObjectStore,
        target: &RunPostingRoot,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        let mut cursor = store.map_cursor(&target.gram_counts, &[], None, budget)?;
        let ready = cursor.next()?;
        Ok(Self {
            cursor,
            ready,
            gram: None,
            rows: 0,
            record_bytes: 0,
        })
    }

    fn finish_gram(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let gram = self.gram.ok_or_else(|| corrupt("fresh run gram absent"))?;
        let (key, reference) = self
            .ready
            .take()
            .ok_or_else(|| corrupt("fresh run count map omits gram"))?;
        if key.as_slice() != gram.as_slice()
            || read_count(store, &key, &reference, budget)?.count != self.rows
        {
            return Err(corrupt("fresh run gram count differs from proved buckets"));
        }
        self.record_bytes = self
            .record_bytes
            .checked_add(reference.root.bytes)
            .ok_or_else(|| corrupt("fresh run count Record ledger overflows"))?;
        self.ready = self.cursor.next()?;
        Ok(())
    }

    fn observe(
        &mut self,
        store: &ObjectStore,
        key: &[u8],
        page_rows: usize,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let gram = gram_key(key)?;
        if self.gram.is_some_and(|prior| prior != gram) {
            self.finish_gram(store, budget)?;
            self.rows = 0;
        }
        self.gram = Some(gram);
        self.rows = self
            .rows
            .checked_add(checked_rows(page_rows)?)
            .ok_or_else(|| corrupt("fresh run gram rows overflow"))?;
        Ok(())
    }

    fn finish(
        &mut self,
        store: &ObjectStore,
        expected_record_bytes: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        if self.gram.is_some() {
            self.finish_gram(store, budget)?;
        }
        if self.ready.is_some() || self.record_bytes != expected_record_bytes {
            return Err(corrupt("fresh run count map adds a gram or Record ledger differs"));
        }
        Ok(())
    }
}

impl<'a> ChangedPageProof<'a> {
    fn new(
        store: &'a ObjectStore,
        base: Option<&RunPostingRoot>,
        target: Option<&RunPostingRoot>,
        removed: &[SegmentRemoval],
        added: &[SegmentInput],
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        let fresh = target.filter(|_| base.is_none() && removed.is_empty());
        Ok(Self {
            store,
            budget,
            old: base.map_or_else(MapRoot::default, |root| root.pages.clone()),
            target: target.map_or_else(MapRoot::default, |root| root.pages.clone()),
            buckets: ChangedBuckets::new(store, removed, added, budget)?,
            rows: base.map_or([0, 0], |root| [root.path_rows, root.content_rows]),
            record_bytes: base.map_or(0, |root| root.page_record_encoded_bytes),
            fresh_pages: fresh
                .map(|root| store.map_cursor(&root.pages, &[], None, budget))
                .transpose()?,
            fresh_counts: fresh
                .map(|root| FreshCountProof::new(store, root, budget))
                .transpose()?,
        })
    }

    fn next_changed(&mut self) -> Result<Option<ChangedMapRow>, CoreError> {
        while let Some(delta) = self.buckets.next()? {
            let surface = *delta
                .key
                .first()
                .ok_or_else(|| corrupt("bucket surface absent"))?;
            if !(surface == 1 || surface == 2) {
                return Err(corrupt("bucket surface differs"));
            }
            let index = match surface {
                1 => 0,
                2 => 1,
                _ => return Err(corrupt("bucket surface differs")),
            };
            let removed_rows = checked_rows(delta.removed.len())?;
            let added_rows = checked_rows(delta.added.len())?;
            let rows = self
                .rows
                .get_mut(index)
                .ok_or_else(|| corrupt("bucket surface slot absent"))?;
            *rows = rows
                .checked_sub(removed_rows)
                .and_then(|sum| sum.checked_add(added_rows))
                .ok_or_else(|| corrupt("changed run row total underflows or overflows"))?;
            if let Some(cursor) = &mut self.fresh_pages {
                let (key, reference) = cursor
                    .next()?
                    .ok_or_else(|| corrupt("fresh run page map omits a bucket"))?;
                if key.as_slice() != delta.key {
                    return Err(corrupt(
                        "fresh run page key differs from changed leaf oracle",
                    ));
                }
                let page = read_run_page(self.store, &key, &reference, self.budget)?;
                // The decoder enforces strictly ordered logical IDs. Compare
                // the authenticated rows directly to the independent leaf oracle.
                if !delta.removed.is_empty()
                    || !page
                        .rows
                        .iter()
                        .map(|row| (row.logical_id, row.segment_sha256))
                        .eq(delta.added)
                {
                    return Err(corrupt("fresh run bucket differs from changed leaf oracle"));
                }
                self.fresh_counts
                    .as_mut()
                    .ok_or_else(|| corrupt("fresh run count proof absent"))?
                    .observe(self.store, &key, page.rows.len(), self.budget)?;
                self.record_bytes = self
                    .record_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| corrupt("run page Record ledger overflows"))?;
                return Ok(Some((key, None, Some(reference))));
            }
            let old = page_at(self.store, &self.old, &delta.key, self.budget)?;
            let next = page_at(self.store, &self.target, &delta.key, self.budget)?;
            let mut expected = old.as_ref().map_or_else(BTreeMap::new, |(_, page)| {
                page.rows
                    .iter()
                    .map(|row| (row.logical_id, row.segment_sha256))
                    .collect()
            });
            let original = expected.clone();
            for (id, digest) in delta.removed {
                if expected.remove(&id) != Some(digest) {
                    return Err(corrupt("retired leaf row is absent from prior run page"));
                }
            }
            for (id, digest) in delta.added {
                if expected.insert(id, digest).is_some() {
                    return Err(corrupt("added leaf row duplicates a live run ID"));
                }
            }
            let target_rows = next.as_ref().map_or_else(BTreeMap::new, |(_, page)| {
                page.rows
                    .iter()
                    .map(|row| (row.logical_id, row.segment_sha256))
                    .collect()
            });
            let bucket_presence_matches = matches!(
                (expected.is_empty(), next.as_ref()),
                (true, None) | (false, Some(_))
            );
            if expected != target_rows || !bucket_presence_matches {
                return Err(corrupt(
                    "target run bucket differs from changed leaf oracle",
                ));
            }
            let old_ref = old.map(|(reference, _)| reference);
            let new_ref = next.map(|(reference, _)| reference);
            if expected == original {
                if old_ref != new_ref {
                    return Err(corrupt("unchanged run bucket was rewritten"));
                }
                continue;
            }
            if old_ref == new_ref {
                return Err(corrupt("changed run bucket kept its prior reference"));
            }
            if let Some(reference) = &old_ref {
                self.record_bytes = self
                    .record_bytes
                    .checked_sub(reference.root.bytes)
                    .ok_or_else(|| corrupt("run page Record ledger underflows"))?;
            }
            if let Some(reference) = &new_ref {
                self.record_bytes = self
                    .record_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| corrupt("run page Record ledger overflows"))?;
            }
            return Ok(Some((delta.key.to_vec(), old_ref, new_ref)));
        }
        Ok(None)
    }
}

struct ChangedGramProof<'a> {
    store: &'a ObjectStore,
    budget: Option<&'a RequestBudgetV1>,
    old: MapRoot,
    target: MapRoot,
    buckets: ChangedBuckets<'a>,
    pending: Option<BucketDelta>,
    record_bytes: u64,
}

impl<'a> ChangedGramProof<'a> {
    fn new(
        store: &'a ObjectStore,
        base: Option<&RunPostingRoot>,
        target: Option<&RunPostingRoot>,
        removed: &[SegmentRemoval],
        added: &[SegmentInput],
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            store,
            budget,
            old: base.map_or_else(MapRoot::default, |root| root.gram_counts.clone()),
            target: target.map_or_else(MapRoot::default, |root| root.gram_counts.clone()),
            buckets: ChangedBuckets::new(store, removed, added, budget)?,
            pending: None,
            record_bytes: base.map_or(0, |root| root.gram_count_record_encoded_bytes),
        })
    }

    fn next_changed(&mut self) -> Result<Option<ChangedMapRow>, CoreError> {
        loop {
            let first = match self.pending.take() {
                Some(first) => Some(first),
                None => self.buckets.next()?,
            };
            let Some(first) = first else {
                return Ok(None);
            };
            let key = gram_key(&first.key)?;
            let mut removed = checked_rows(first.removed.len())?;
            let mut added = checked_rows(first.added.len())?;
            while let Some(next) = self.buckets.next()? {
                if gram_key(&next.key)? != key {
                    self.pending = Some(next);
                    break;
                }
                removed = removed
                    .checked_add(checked_rows(next.removed.len())?)
                    .ok_or_else(|| corrupt("removed gram count overflows"))?;
                added = added
                    .checked_add(checked_rows(next.added.len())?)
                    .ok_or_else(|| corrupt("added gram count overflows"))?;
            }
            let old = count_at(self.store, &self.old, &key, self.budget)?;
            let next = count_at(self.store, &self.target, &key, self.budget)?;
            let prior = old.as_ref().map_or(0, |(_, count)| count.count);
            let expected = prior
                .checked_sub(removed)
                .and_then(|sum| sum.checked_add(added))
                .ok_or_else(|| corrupt("gram count delta underflows or overflows"))?;
            if next.as_ref().map_or(0, |(_, count)| count.count) != expected
                || (expected == 0) != next.is_none()
            {
                return Err(corrupt("target gram count differs from leaf row delta"));
            }
            let old_ref = old.map(|(reference, _)| reference);
            let new_ref = next.map(|(reference, _)| reference);
            if old_ref == new_ref {
                continue;
            }
            if let Some(reference) = &old_ref {
                self.record_bytes = self
                    .record_bytes
                    .checked_sub(reference.root.bytes)
                    .ok_or_else(|| corrupt("gram-count Record ledger underflows"))?;
            }
            if let Some(reference) = &new_ref {
                self.record_bytes = self
                    .record_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| corrupt("gram-count Record ledger overflows"))?;
            }
            return Ok(Some((key.to_vec(), old_ref, new_ref)));
        }
    }
}

struct ChangedBlockPage {
    key: [u8; 12],
    page: RunPostingPage,
    block_last: [u8; 12],
}

/// Decode only directory entries proven changed by the authenticated map diff.
/// One validated physical block and one posting page remain live per side.
struct ChangedBlockPages<'a> {
    store: &'a ObjectStore,
    root: &'a RunPostingRoot,
    budget: Option<&'a RequestBudgetV1>,
    entries: VecDeque<([u8; 12], BlobRef)>,
    pages: Option<RunBlockPages>,
    active_last: Option<[u8; 12]>,
    active_witnessed: bool,
    require_witness: bool,
    active_expected_pages: u64,
    active_seen_pages: u64,
    changed_blocks: u64,
    changed_pages: u64,
    changed_bytes: u64,
    exhausted: bool,
}

impl<'a> ChangedBlockPages<'a> {
    fn new(
        store: &'a ObjectStore,
        root: &'a RunPostingRoot,
        entries: VecDeque<([u8; 12], BlobRef)>,
        require_witness: bool,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            store,
            root,
            budget,
            entries,
            pages: None,
            active_last: None,
            active_witnessed: false,
            require_witness,
            active_expected_pages: 0,
            active_seen_pages: 0,
            changed_blocks: 0,
            changed_pages: 0,
            changed_bytes: 0,
            exhausted: false,
        })
    }

    fn next_page(&mut self) -> Result<Option<ChangedBlockPage>, CoreError> {
        if self.exhausted {
            return Ok(None);
        }
        loop {
            checkpoint(self.budget)?;
            if let Some(pages) = &mut self.pages {
                if let Some((key, page)) = pages.next()? {
                    self.active_seen_pages = self
                        .active_seen_pages
                        .checked_add(1)
                        .ok_or_else(|| corrupt("changed block page count overflows"))?;
                    let key: [u8; 12] = key
                        .as_slice()
                        .try_into()
                        .map_err(|_width| corrupt("changed block page key width differs"))?;
                    return Ok(Some(ChangedBlockPage {
                        key,
                        page,
                        block_last: self
                            .active_last
                            .ok_or_else(|| corrupt("changed block last key absent"))?,
                    }));
                }
                if self.active_seen_pages != self.active_expected_pages {
                    return Err(corrupt("changed block page count differs"));
                }
                if self.require_witness && !self.active_witnessed {
                    return Err(corrupt("unchanged run block was rewritten"));
                }
                self.pages = None;
                self.active_last = None;
            }
            let Some((last, reference)) = self.entries.pop_front() else {
                self.exhausted = true;
                return Ok(None);
            };
            let (first, page_count) =
                self.root
                    .block_at(self.store, &last, &reference, self.budget)?;
            // A changed block must not overlap inherited neighbors. The
            // authenticated directory's lower-bound lookup identifies the
            // first possible block; the next entry supplies its first key.
            let mut at_first =
                self.store
                    .map_cursor(&self.root.pages, &first, None, self.budget)?;
            let (selected_last, selected_ref) = at_first
                .next()?
                .ok_or_else(|| corrupt("changed block first key is absent from directory"))?;
            if selected_last.as_slice() != last.as_slice() || selected_ref != reference {
                return Err(corrupt("changed block overlaps its predecessor"));
            }
            let _next = self.root.next_block(self.store, Some(&last), self.budget)?;
            self.changed_blocks = self
                .changed_blocks
                .checked_add(1)
                .ok_or_else(|| corrupt("run block count overflows"))?;
            self.changed_pages = self
                .changed_pages
                .checked_add(page_count)
                .ok_or_else(|| corrupt("run logical page count overflows"))?;
            self.changed_bytes = self
                .changed_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| corrupt("run block Record ledger overflows"))?;
            self.pages = Some(
                self.root
                    .block_pages(self.store, &last, &reference, self.budget)?,
            );
            self.active_last = Some(last);
            self.active_witnessed = false;
            self.active_expected_pages = page_count;
            self.active_seen_pages = 0;
        }
    }

    fn witness(&mut self, last: [u8; 12]) -> Result<(), CoreError> {
        if self.active_last != Some(last) {
            return Err(corrupt("changed block witness does not match active block"));
        }
        self.active_witnessed = true;
        Ok(())
    }

    fn witness_routed_key(&mut self, key: &[u8; 12]) -> Result<(), CoreError> {
        let Some(active_last) = self.active_last else {
            return Ok(());
        };
        let mut route = self
            .store
            .map_cursor(&self.root.pages, key, None, self.budget)?;
        if route
            .next()?
            .is_some_and(|(last, _reference)| last.as_slice() == active_last.as_slice())
        {
            self.active_witnessed = true;
        }
        Ok(())
    }

    fn finish(&self) -> Result<(u64, u64, u64), CoreError> {
        if !self.exhausted || !self.entries.is_empty() || self.pages.is_some() {
            return Err(corrupt("changed run block stream did not exhaust"));
        }
        Ok((self.changed_blocks, self.changed_pages, self.changed_bytes))
    }
}

fn verify_v2_fresh_run(
    store: &ObjectStore,
    root: &RunPostingRoot,
    added: &[SegmentInput],
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    let mut pages = root.page_cursor(store, &[], None, budget)?;
    let mut buckets = ChangedBuckets::new(store, &[], added, budget)?;
    let mut counts = FreshCountProof::new(store, root, budget)?;
    let mut rows = [0_u64; 2];
    let mut observed_pages = 0_u64;
    let mut observed_blocks = 0_u64;
    let mut record_bytes = 0_u64;
    let mut prior_block_first = None;
    while let Some((key, reference, page)) = pages.next()? {
        checkpoint(budget)?;
        let first = pages
            .active_block_first_key()?
            .ok_or_else(|| corrupt("fresh run block first key is absent"))?;
        if prior_block_first != Some(first) {
            observed_blocks = observed_blocks
                .checked_add(1)
                .ok_or_else(|| corrupt("fresh run block count overflows"))?;
            record_bytes = record_bytes
                .checked_add(reference.root.bytes)
                .ok_or_else(|| corrupt("fresh run Record ledger overflows"))?;
            prior_block_first = Some(first);
        }
        let delta = buckets
            .next()?
            .ok_or_else(|| corrupt("fresh run adds a page absent from leaf oracle"))?;
        if key.as_slice() != delta.key || !delta.removed.is_empty() {
            return Err(corrupt("fresh run page key differs from leaf oracle"));
        }
        if !page
            .rows
            .iter()
            .map(|row| (row.logical_id, row.segment_sha256))
            .eq(delta.added)
        {
            return Err(corrupt("fresh run bucket differs from changed leaf oracle"));
        }
        let slot = match key.first() {
            Some(1) => 0,
            Some(2) => 1,
            _ => return Err(corrupt("fresh run page surface differs")),
        };
        rows[slot] = rows[slot]
            .checked_add(checked_rows(page.rows.len())?)
            .ok_or_else(|| corrupt("fresh run row total overflows"))?;
        observed_pages = observed_pages
            .checked_add(1)
            .ok_or_else(|| corrupt("fresh run logical page count overflows"))?;
        counts.observe(store, &key, page.rows.len(), budget)?;
    }
    if buckets.next()?.is_some()
        || observed_pages != root.logical_pages()
        || observed_blocks != root.pages.entries
        || record_bytes != root.page_record_encoded_bytes
        || rows != [root.path_rows, root.content_rows]
    {
        return Err(corrupt("fresh run omits a leaf page or row total differs"));
    }
    counts.finish(store, root.gram_count_record_encoded_bytes, budget)?;
    Ok(())
}

fn verify_v2_changed_pages(
    store: &ObjectStore,
    base: Option<&RunPostingRoot>,
    target: Option<&RunPostingRoot>,
    removed: &[SegmentRemoval],
    added: &[SegmentInput],
    budget: Option<&RequestBudgetV1>,
) -> Result<[u64; 2], CoreError> {
    let old_root = base.map_or_else(MapRoot::default, |root| root.pages.clone());
    let next_root = target.map_or_else(MapRoot::default, |root| root.pages.clone());
    let mut old_entries = VecDeque::new();
    let mut next_entries = VecDeque::new();
    store.map_changed_keys(&old_root, &next_root, budget, &mut |key, before, after| {
        let last: [u8; 12] = key
            .try_into()
            .map_err(|_width| corrupt("changed block directory key width differs"))?;
        if let Some(reference) = before {
            if old_entries.back().is_some_and(|(prior, _)| *prior >= last) {
                return Err(corrupt("changed old block keys reorder"));
            }
            old_entries.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!("reserve changed old block: {error}"))
            })?;
            old_entries.push_back((last, reference.clone()));
        }
        if let Some(reference) = after {
            if next_entries.back().is_some_and(|(prior, _)| *prior >= last) {
                return Err(corrupt("changed target block keys reorder"));
            }
            next_entries.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!("reserve changed target block: {error}"))
            })?;
            next_entries.push_back((last, reference.clone()));
        }
        Ok(())
    })?;
    let mut old = base
        .map(|root| ChangedBlockPages::new(store, root, old_entries, true, budget))
        .transpose()?;
    let mut next = target
        .map(|root| ChangedBlockPages::new(store, root, next_entries, false, budget))
        .transpose()?;
    let mut old_head = old
        .as_mut()
        .map(ChangedBlockPages::next_page)
        .transpose()?
        .flatten();
    let mut next_head = next
        .as_mut()
        .map(ChangedBlockPages::next_page)
        .transpose()?
        .flatten();
    let mut buckets = ChangedBuckets::new(store, removed, added, budget)?;
    let mut delta_head = buckets.next()?;
    let mut rows = base.map_or([0, 0], |root| [root.path_rows, root.content_rows]);
    loop {
        checkpoint(budget)?;
        let key = [
            old_head.as_ref().map(|head| head.key),
            next_head.as_ref().map(|head| head.key),
            delta_head.as_ref().map(|head| head.key),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(key) = key else { break };
        let changed_old = if old_head.as_ref().is_some_and(|head| head.key == key) {
            old_head.take()
        } else {
            None
        };
        let changed_next = if next_head.as_ref().is_some_and(|head| head.key == key) {
            next_head.take()
        } else {
            None
        };
        let delta = if delta_head.as_ref().is_some_and(|head| head.key == key) {
            let selected = delta_head.take();
            delta_head = buckets.next()?;
            selected
        } else {
            None
        };
        let old_page = match (&changed_old, base, delta.as_ref()) {
            (Some(head), _, _) => Some(head.page.clone()),
            (None, Some(root), Some(_)) => root.page_at(store, &key, budget)?.map(|(_, page)| page),
            _ => None,
        };
        let next_page = match (&changed_next, target, delta.as_ref()) {
            (Some(head), _, _) => Some(head.page.clone()),
            (None, Some(root), Some(_)) => root.page_at(store, &key, budget)?.map(|(_, page)| page),
            _ => None,
        };
        let original = old_page.as_ref().map_or_else(BTreeMap::new, |page| {
            page.rows
                .iter()
                .map(|row| (row.logical_id, row.segment_sha256))
                .collect()
        });
        let mut expected = original.clone();
        if let Some(delta) = delta {
            for (id, digest) in delta.removed {
                if expected.remove(&id) != Some(digest) {
                    return Err(corrupt("retired leaf row is absent from prior run page"));
                }
            }
            for (id, digest) in delta.added {
                if expected.insert(id, digest).is_some() {
                    return Err(corrupt("added leaf row duplicates a live run ID"));
                }
            }
        }
        let actual = next_page.as_ref().map_or_else(BTreeMap::new, |page| {
            page.rows
                .iter()
                .map(|row| (row.logical_id, row.segment_sha256))
                .collect()
        });
        if expected != actual
            || (expected.is_empty() && next_page.is_some())
            || (!expected.is_empty() && next_page.is_none())
        {
            return Err(corrupt(
                "target run bucket differs from changed leaf oracle",
            ));
        }
        let slot = match key[0] {
            1 => 0,
            2 => 1,
            _ => return Err(corrupt("changed run page surface differs")),
        };
        let old_rows = checked_rows(old_page.as_ref().map_or(0, |page| page.rows.len()))?;
        let next_rows = checked_rows(next_page.as_ref().map_or(0, |page| page.rows.len()))?;
        rows[slot] = rows[slot]
            .checked_sub(old_rows)
            .and_then(|sum| sum.checked_add(next_rows))
            .ok_or_else(|| corrupt("changed run row total underflows or overflows"))?;
        if expected != original {
            if let Some(head) = &changed_old {
                old.as_mut()
                    .ok_or_else(|| corrupt("old changed block stream absent"))?
                    .witness(head.block_last)?;
            } else if let Some(stream) = old.as_mut() {
                // A newly inserted bucket has no old page head. Its lower-bound
                // route still makes the old physical block a real touched block.
                stream.witness_routed_key(&key)?;
            }
            if let Some(head) = &changed_next {
                next.as_mut()
                    .ok_or_else(|| corrupt("new changed block stream absent"))?
                    .witness(head.block_last)?;
            }
        }
        if changed_old.is_some() {
            old_head = old
                .as_mut()
                .map(ChangedBlockPages::next_page)
                .transpose()?
                .flatten();
        }
        if changed_next.is_some() {
            next_head = next
                .as_mut()
                .map(ChangedBlockPages::next_page)
                .transpose()?
                .flatten();
        }
    }
    let (old_blocks, old_pages, old_bytes) = old
        .as_ref()
        .map(ChangedBlockPages::finish)
        .transpose()?
        .unwrap_or((0, 0, 0));
    let (next_blocks, next_pages, next_bytes) = next
        .as_ref()
        .map(ChangedBlockPages::finish)
        .transpose()?
        .unwrap_or((0, 0, 0));
    let old_blocks_total = base.map_or(0, |root| root.pages.entries);
    let old_pages_total = base.map_or(0, RunPostingRoot::logical_pages);
    let old_bytes_total = base.map_or(0, |root| root.page_record_encoded_bytes);
    if old_blocks_total
        .checked_sub(old_blocks)
        .and_then(|sum| sum.checked_add(next_blocks))
        != Some(target.map_or(0, |root| root.pages.entries))
        || old_pages_total
            .checked_sub(old_pages)
            .and_then(|sum| sum.checked_add(next_pages))
            != Some(target.map_or(0, RunPostingRoot::logical_pages))
        || old_bytes_total
            .checked_sub(old_bytes)
            .and_then(|sum| sum.checked_add(next_bytes))
            != Some(target.map_or(0, |root| root.page_record_encoded_bytes))
    {
        return Err(corrupt("changed run block count or Record ledger differs"));
    }
    Ok(rows)
}

/// Prove changed physical leaf rows and the corresponding run page edits.
///
/// The proof also covers rolling gram counters and the Record ledger.
/// Unchanged pages inherit authenticated map references.
pub(crate) fn verify_changed_run(
    store: &ObjectStore,
    run_id: u64,
    base: Option<(&RunPostingRoot, &BlobRef)>,
    target: Option<(&RunPostingRoot, &BlobRef)>,
    removed: &[SegmentRemoval],
    added: &[SegmentInput],
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    if run_id == 0 || (base.is_none() && target.is_none()) {
        return Err(corrupt("changed run ID or roots are absent"));
    }
    let old = optional_run(store, run_id, base, budget)?;
    let next = optional_run(store, run_id, target, budget)?;
    if old
        .zip(next)
        .is_some_and(|(old, next)| old.is_block_layout() != next.is_block_layout())
    {
        return Err(corrupt("ordinary run delta changes page layout"));
    }
    let v2 = old.is_some_and(RunPostingRoot::is_block_layout)
        || next.is_some_and(RunPostingRoot::is_block_layout);
    let pages_phase = crate::causal_profile::begin_phase("seal_file_run_changed_pages");
    if v2 {
        if old.is_none() && removed.is_empty() {
            let fresh = next.ok_or_else(|| corrupt("fresh block run is absent"))?;
            verify_v2_fresh_run(store, fresh, added, budget)?;
            crate::causal_profile::finish_phase("seal_file_run_changed_pages", pages_phase);
            return Ok(());
        }
        let rows = verify_v2_changed_pages(store, old, next, removed, added, budget)?;
        if rows != next.map_or([0, 0], |root| [root.path_rows, root.content_rows]) {
            return Err(corrupt("run page row ledger differs"));
        }
    } else {
        let mut pages = ChangedPageProof::new(store, old, next, removed, added, budget)?;
        if pages.fresh_pages.is_some() {
            // The fresh map has no inherited rows. Exact ordered equality and
            // exhaustion prove its complete domain without a second map traversal.
            while pages.next_changed()?.is_some() {}
            if let Some(cursor) = &mut pages.fresh_pages
                && cursor.next()?.is_some()
            {
                return Err(corrupt("fresh run page map adds a bucket"));
            }
        } else {
            let mut expected = pages.next_changed()?;
            let old_pages = pages.old.clone();
            let new_pages = pages.target.clone();
            store.map_changed_keys(&old_pages, &new_pages, budget, &mut |key, before, after| {
                let Some((expected_key, old_ref, new_ref)) = expected.take() else {
                    return Err(corrupt("run page map has unrelated edit"));
                };
                if key != expected_key.as_slice()
                    || before != old_ref.as_ref()
                    || after != new_ref.as_ref()
                {
                    return Err(corrupt(
                        "run page map diff differs from changed leaf oracle",
                    ));
                }
                expected = pages.next_changed()?;
                Ok(())
            })?;
            if expected.is_some() || pages.next_changed()?.is_some() {
                return Err(corrupt("changed leaf page edit omitted from map diff"));
            }
        }
        if pages.rows != next.map_or([0, 0], |root| [root.path_rows, root.content_rows])
            || pages.record_bytes != next.map_or(0, |root| root.page_record_encoded_bytes)
        {
            return Err(corrupt("run page row or Record byte ledger differs"));
        }
        if let Some(counts) = &mut pages.fresh_counts {
            crate::causal_profile::finish_phase("seal_file_run_changed_pages", pages_phase);
            let counts_phase = crate::causal_profile::begin_phase("seal_file_run_changed_counts");
            counts.finish(
                store,
                next.map_or(0, |root| root.gram_count_record_encoded_bytes),
                budget,
            )?;
            crate::causal_profile::finish_phase("seal_file_run_changed_counts", counts_phase);
            return Ok(());
        }
    }
    crate::causal_profile::finish_phase("seal_file_run_changed_pages", pages_phase);
    let counts_phase = crate::causal_profile::begin_phase("seal_file_run_changed_counts");
    let mut counts = ChangedGramProof::new(store, old, next, removed, added, budget)?;
    let mut expected = counts.next_changed()?;
    let old_counts = counts.old.clone();
    let new_counts = counts.target.clone();
    store.map_changed_keys(
        &old_counts,
        &new_counts,
        budget,
        &mut |key, before, after| {
            let Some((expected_key, old_ref, new_ref)) = expected.take() else {
                return Err(corrupt("run gram-count map has unrelated edit"));
            };
            if key != expected_key.as_slice()
                || before != old_ref.as_ref()
                || after != new_ref.as_ref()
            {
                return Err(corrupt("run gram-count map diff differs from leaf oracle"));
            }
            expected = counts.next_changed()?;
            Ok(())
        },
    )?;
    if expected.is_some()
        || counts.next_changed()?.is_some()
        || counts.record_bytes != next.map_or(0, |root| root.gram_count_record_encoded_bytes)
    {
        return Err(corrupt("run gram-count edit or Record byte ledger differs"));
    }
    crate::causal_profile::finish_phase("seal_file_run_changed_counts", counts_phase);
    Ok(())
}

/// Full sorted census used only for selected clean input runs and the new
/// output run. A single page and gram counter are live at each cursor level.
fn verify_complete_block_run(
    store: &ObjectStore,
    root: &RunPostingRoot,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    let mut pages = root.page_cursor(store, &[], None, budget)?;
    let mut counts = store.map_cursor(&root.gram_counts, &[], None, budget)?;
    let mut ready_count = counts.next()?;
    let mut prior_gram: Option<[u8; 4]> = None;
    let mut gram_rows = 0_u64;
    let mut rows = [0_u64; 2];
    let mut observed_pages = 0_u64;
    let mut observed_blocks = 0_u64;
    let mut prior_block_first = None;
    let mut page_record_bytes = 0_u64;
    let mut count_record_bytes = 0_u64;
    while let Some((key, block_ref, page)) = pages.next()? {
        checkpoint(budget)?;
        let first = pages
            .active_block_first_key()?
            .ok_or_else(|| corrupt("complete run block first key is absent"))?;
        if prior_block_first != Some(first) {
            observed_blocks = observed_blocks
                .checked_add(1)
                .ok_or_else(|| corrupt("complete run block count overflows"))?;
            page_record_bytes = page_record_bytes
                .checked_add(block_ref.root.bytes)
                .ok_or_else(|| corrupt("complete run block Record ledger overflows"))?;
            prior_block_first = Some(first);
        }
        let gram = gram_key(&key)?;
        if prior_gram.is_some_and(|prior| prior != gram) {
            let prior = prior_gram.ok_or_else(|| corrupt("prior run gram absent"))?;
            let Some((count_key, count_ref)) = ready_count.take() else {
                return Err(corrupt("run gram-count map omits page gram"));
            };
            if count_key.as_slice() != &prior[..]
                || read_count(store, &count_key, &count_ref, budget)?.count != gram_rows
            {
                return Err(corrupt("run gram-count differs from complete pages"));
            }
            count_record_bytes = count_record_bytes
                .checked_add(count_ref.root.bytes)
                .ok_or_else(|| corrupt("run count Record ledger overflows"))?;
            ready_count = counts.next()?;
            gram_rows = 0;
        }
        prior_gram = Some(gram);
        let page_rows = checked_rows(page.rows.len())?;
        gram_rows = gram_rows
            .checked_add(page_rows)
            .ok_or_else(|| corrupt("run gram rows overflow"))?;
        let slot = match gram[0] {
            1 => 0,
            2 => 1,
            _ => return Err(corrupt("run page surface differs")),
        };
        rows[slot] = rows[slot]
            .checked_add(page_rows)
            .ok_or_else(|| corrupt("run surface rows overflow"))?;
        observed_pages = observed_pages
            .checked_add(1)
            .ok_or_else(|| corrupt("run logical page count overflows"))?;
    }
    if let Some(prior) = prior_gram {
        let Some((count_key, count_ref)) = ready_count.take() else {
            return Err(corrupt("run gram-count map omits final page gram"));
        };
        if count_key.as_slice() != &prior[..]
            || read_count(store, &count_key, &count_ref, budget)?.count != gram_rows
        {
            return Err(corrupt("run final gram count differs from pages"));
        }
        count_record_bytes = count_record_bytes
            .checked_add(count_ref.root.bytes)
            .ok_or_else(|| corrupt("run count Record ledger overflows"))?;
        ready_count = counts.next()?;
    }
    if ready_count.is_some()
        || rows != [root.path_rows, root.content_rows]
        || observed_pages != root.logical_pages()
        || observed_blocks != root.pages.entries
        || page_record_bytes != root.page_record_encoded_bytes
        || count_record_bytes != root.gram_count_record_encoded_bytes
    {
        return Err(corrupt("run complete page/count or Record ledger differs"));
    }
    Ok(())
}

fn verify_complete_run(
    store: &ObjectStore,
    root: &RunPostingRoot,
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    verify_root_reference(store, root.run_id, root, reference, budget)?;
    let _bundle = verify_bundle_structure(store, root, reference, budget)?;
    if root.is_block_layout() {
        return verify_complete_block_run(store, root, budget);
    }
    let mut pages = store.map_cursor(&root.pages, &[], None, budget)?;
    let mut counts = store.map_cursor(&root.gram_counts, &[], None, budget)?;
    let mut ready_count = counts.next()?;
    let mut prior_gram: Option<[u8; 4]> = None;
    let mut gram_rows = 0_u64;
    let mut rows = [0_u64; 2];
    let mut page_record_bytes = 0_u64;
    let mut count_record_bytes = 0_u64;
    while let Some((key, page_ref)) = pages.next()? {
        checkpoint(budget)?;
        let gram = gram_key(&key)?;
        if prior_gram.is_some_and(|prior| prior != gram) {
            let prior = prior_gram.ok_or_else(|| corrupt("prior run gram absent"))?;
            let Some((count_key, count_ref)) = ready_count.take() else {
                return Err(corrupt("run gram-count map omits page gram"));
            };
            if count_key.as_slice() != &prior[..]
                || read_count(store, &count_key, &count_ref, budget)?.count != gram_rows
            {
                return Err(corrupt("run gram-count differs from complete pages"));
            }
            count_record_bytes = count_record_bytes
                .checked_add(count_ref.root.bytes)
                .ok_or_else(|| corrupt("run count Record ledger overflows"))?;
            ready_count = counts.next()?;
            gram_rows = 0;
        }
        prior_gram = Some(gram);
        let page = read_run_page(store, &key, &page_ref, budget)?;
        let page_rows = checked_rows(page.rows.len())?;
        gram_rows = gram_rows
            .checked_add(page_rows)
            .ok_or_else(|| corrupt("run gram rows overflow"))?;
        let slot = match gram[0] {
            1 => 0,
            2 => 1,
            _ => return Err(corrupt("run page surface differs")),
        };
        let surface_rows = rows
            .get_mut(slot)
            .ok_or_else(|| corrupt("run surface slot absent"))?;
        *surface_rows = surface_rows
            .checked_add(page_rows)
            .ok_or_else(|| corrupt("run surface rows overflow"))?;
        page_record_bytes = page_record_bytes
            .checked_add(page_ref.root.bytes)
            .ok_or_else(|| corrupt("run page Record ledger overflows"))?;
    }
    if let Some(prior) = prior_gram {
        let Some((count_key, count_ref)) = ready_count.take() else {
            return Err(corrupt("run gram-count map omits final page gram"));
        };
        if count_key.as_slice() != &prior[..]
            || read_count(store, &count_key, &count_ref, budget)?.count != gram_rows
        {
            return Err(corrupt("run final gram count differs from pages"));
        }
        count_record_bytes = count_record_bytes
            .checked_add(count_ref.root.bytes)
            .ok_or_else(|| corrupt("run count Record ledger overflows"))?;
        ready_count = counts.next()?;
    }
    if ready_count.is_some()
        || rows != [root.path_rows, root.content_rows]
        || page_record_bytes != root.page_record_encoded_bytes
        || count_record_bytes != root.gram_count_record_encoded_bytes
    {
        return Err(corrupt("run complete page/count or Record ledger differs"));
    }
    Ok(())
}

/// Prove a clean merge by independently comparing every output page against
/// the exact keyed union of complete selected input pages. Duplicate logical
/// IDs, even with identical locators, are refused.
pub(crate) fn verify_clean_run_union(
    store: &ObjectStore,
    selected: &[(RunPostingRoot, BlobRef)],
    target: (&RunPostingRoot, &BlobRef),
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    if selected.len() < 2
        || selected
            .windows(2)
            .any(|pair| match (pair.first(), pair.get(1)) {
                (Some(left), Some(right)) => left.0.run_id >= right.0.run_id,
                _ => true,
            })
        || selected
            .last()
            .is_some_and(|last| last.0.run_id >= target.0.run_id)
    {
        return Err(corrupt("clean run input IDs or output ID differ"));
    }
    verify_complete_run(store, target.0, target.1, budget)?;
    let mut cursors = Vec::new();
    let mut ready = Vec::new();
    for (root, reference) in selected {
        verify_complete_run(store, root, reference, budget)?;
        let mut cursor = root.page_cursor(store, &[], None, budget)?;
        ready.push(cursor.next()?);
        cursors.push(cursor);
    }
    let mut output = target.0.page_cursor(store, &[], None, budget)?;
    let mut output_ready = output.next()?;
    while let Some(key) = ready.iter().flatten().map(|(key, _, _)| key.clone()).min() {
        checkpoint(budget)?;
        let mut expected = BTreeMap::<u64, [u8; 32]>::new();
        for (cursor, head) in cursors.iter_mut().zip(&mut ready) {
            if head.as_ref().is_some_and(|(current, _, _)| *current == key) {
                let (_, _reference, page) = head
                    .take()
                    .ok_or_else(|| corrupt("input page head absent"))?;
                for row in page.rows {
                    if expected
                        .insert(row.logical_id, row.segment_sha256)
                        .is_some()
                    {
                        return Err(corrupt("selected clean runs duplicate a logical ID"));
                    }
                }
                *head = cursor.next()?;
            }
        }
        let Some((output_key, _output_ref, actual)) = output_ready.take() else {
            return Err(corrupt("clean merged output omits an input page"));
        };
        if output_key != key {
            return Err(corrupt("clean merged output page key differs"));
        }
        if actual.rows.len() != expected.len()
            || actual
                .rows
                .iter()
                .any(|row| expected.get(&row.logical_id) != Some(&row.segment_sha256))
        {
            return Err(corrupt("clean merged output rows differ from input union"));
        }
        output_ready = output.next()?;
    }
    if output_ready.is_some() {
        return Err(corrupt("clean merged output adds a page"));
    }
    Ok(())
}

struct RouteCountCursor<'a> {
    run_id: u64,
    old: bool,
    cursor: MapCursor<'a>,
    ready: Option<(Vec<u8>, BlobRef)>,
}

struct ChangedRoutes<'a> {
    store: &'a ObjectStore,
    budget: Option<&'a RequestBudgetV1>,
    sources: Vec<RouteCountCursor<'a>>,
    pending: VecDeque<ChangedMapRow>,
}

impl<'a> ChangedRoutes<'a> {
    fn new(
        store: &'a ObjectStore,
        old: &AuthorityRoot,
        target: &AuthorityRoot,
        changed_run_ids: &[u64],
        surface: u8,
        budget: Option<&'a RequestBudgetV1>,
    ) -> Result<Self, CoreError> {
        if !(surface == 1 || surface == 2) {
            return Err(corrupt("route surface differs"));
        }
        let surface_upper = surface
            .checked_add(1)
            .ok_or_else(|| corrupt("route surface upper bound overflows"))?;
        let mut sources = Vec::new();
        for run_id in changed_run_ids {
            let run_key = run_id.to_be_bytes();
            for (root, was_old) in [(old, true), (target, false)] {
                let Some(reference) = store.map_get(&root.run_postings, &run_key, budget)? else {
                    continue;
                };
                let posting = RunPostingRoot::load(store, &reference, budget)?;
                verify_root_reference(store, *run_id, &posting, &reference, budget)?;
                let mut cursor = store.map_cursor(
                    &posting.gram_counts,
                    &[surface],
                    Some(&[surface_upper]),
                    budget,
                )?;
                let ready = cursor.next()?;
                sources.push(RouteCountCursor {
                    run_id: *run_id,
                    old: was_old,
                    cursor,
                    ready,
                });
            }
        }
        Ok(Self {
            store,
            budget,
            sources,
            pending: VecDeque::new(),
        })
    }

    fn next_changed(&mut self) -> Result<Option<ChangedMapRow>, CoreError> {
        loop {
            checkpoint(self.budget)?;
            if let Some(change) = self.pending.pop_front() {
                return Ok(Some(change));
            }
            let Some(gram) = self
                .sources
                .iter()
                .filter_map(|source| source.ready.as_ref().map(|(key, _)| key.clone()))
                .min()
            else {
                return Ok(None);
            };
            if gram.len() != 4 {
                return Err(corrupt("changed run gram-count key width differs"));
            }
            let mut before = BTreeMap::<Vec<u8>, BlobRef>::new();
            let mut after = BTreeMap::<Vec<u8>, BlobRef>::new();
            for source in &mut self.sources {
                if source.ready.as_ref().is_some_and(|(key, _)| *key == gram) {
                    let (key, reference) = source
                        .ready
                        .take()
                        .ok_or_else(|| corrupt("route count head absent"))?;
                    let count = read_count(self.store, &key, &reference, self.budget)?;
                    let route =
                        run_route_key(count.surface, count.gram, count.count, source.run_id)?
                            .to_vec();
                    let side = if source.old { &mut before } else { &mut after };
                    if side.insert(route, reference).is_some() {
                        return Err(corrupt("changed run route repeats a count key"));
                    }
                    source.ready = source.cursor.next()?;
                }
            }
            let mut keys = before
                .keys()
                .chain(after.keys())
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            for key in keys {
                let prior = before.get(&key).cloned();
                let next = after.get(&key).cloned();
                if prior != next {
                    self.pending.push_back((key, prior, next));
                }
            }
        }
    }
}

/// Prove global query route changes from changed runs' gram counts.
///
/// Count-map cursors advance lazily by gram. The authenticated map diff
/// rejects unrelated global route edits.
pub(crate) fn verify_global_run_routes(
    store: &ObjectStore,
    old: &AuthorityRoot,
    target: &AuthorityRoot,
    changed_run_ids: &[u64],
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    if changed_run_ids
        .windows(2)
        .any(|pair| match (pair.first(), pair.get(1)) {
            (Some(left), Some(right)) => left >= right,
            _ => true,
        })
        || changed_run_ids.contains(&0)
    {
        return Err(corrupt("changed run IDs repeat or are not increasing"));
    }
    for (surface, before, after) in [
        (1, &old.path_routes, &target.path_routes),
        (2, &old.content_routes, &target.content_routes),
    ] {
        let mut routes = ChangedRoutes::new(store, old, target, changed_run_ids, surface, budget)?;
        let mut expected = routes.next_changed()?;
        store.map_changed_keys(before, after, budget, &mut |key, previous, current| {
            let Some((route_key, prior_ref, next_ref)) = expected.take() else {
                return Err(corrupt("global run route has unrelated edit"));
            };
            if key != route_key.as_slice()
                || previous != prior_ref.as_ref()
                || current != next_ref.as_ref()
            {
                return Err(corrupt("global route map differs from changed run counts"));
            }
            expected = routes.next_changed()?;
            Ok(())
        })?;
        if expected.is_some() || routes.next_changed()?.is_some() {
            return Err(corrupt("changed run count route omitted from global map"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod fresh_count_tests {
    use super::*;
    use crate::file_authority::paged::run_postings::bootstrap_new_run_postings;
    use crate::lexical_store::{AttemptBinding, AttemptLease, MapEdit};

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

    fn segment(store: &ObjectStore, lease: &AttemptLease, ids: Vec<u64>) -> BlobRef {
        let len = ids.len();
        let mask = (1_u64 << len) - 1;
        let mut surfaces = Vec::new();
        for gram in [*b"abc", *b"xyz"] {
            let page = PostingPage(vec![(gram, mask)]);
            let page_ref = store
                .put_blob(
                    lease,
                    ObjectKind::FilePostings,
                    &[],
                    &page.encode(len).expect("fixed posting page"),
                )
                .expect("physical posting page");
            surfaces.push(
                store
                    .map_bootstrap(
                        lease,
                        std::iter::once(Ok(MapEdit {
                            key: gram.to_vec(),
                            value: Some(page_ref),
                        })),
                    )
                    .expect("physical term map"),
            );
        }
        let leaf = Segment {
            doc_ids: ids,
            path_terms: surfaces.remove(0),
            content_terms: surfaces.remove(0),
            path_memberships: len as u64,
            content_memberships: len as u64,
        };
        let deps = [&leaf.path_terms, &leaf.content_terms]
            .into_iter()
            .filter_map(|root| root.page.as_ref().map(|page| page.object.clone()))
            .collect::<Vec<_>>();
        store
            .put_blob(
                lease,
                ObjectKind::Segment,
                &deps,
                &leaf.encode().expect("fixed segment"),
            )
            .expect("physical segment")
    }

    fn path_only_segment(store: &ObjectStore, lease: &AttemptLease, id: u64) -> BlobRef {
        let page = PostingPage(vec![(*b"abc", 1)]);
        let page_ref = store
            .put_blob(
                lease,
                ObjectKind::FilePostings,
                &[],
                &page.encode(1).expect("path-only posting page"),
            )
            .expect("path-only physical posting page");
        let path_terms = store
            .map_bootstrap(
                lease,
                std::iter::once(Ok(MapEdit {
                    key: b"abc".to_vec(),
                    value: Some(page_ref),
                })),
            )
            .expect("path-only term map");
        let leaf = Segment {
            doc_ids: vec![id],
            path_terms,
            content_terms: MapRoot::default(),
            path_memberships: 1,
            content_memberships: 0,
        };
        let deps = leaf
            .path_terms
            .page
            .as_ref()
            .map(|page| vec![page.object.clone()])
            .expect("path-only term map root");
        store
            .put_blob(
                lease,
                ObjectKind::Segment,
                &deps,
                &leaf.encode().expect("path-only segment"),
            )
            .expect("path-only physical segment")
    }

    fn input(reference: &BlobRef) -> SegmentInput {
        SegmentInput {
            key: reference.root.sha256.to_vec(),
            reference: reference.clone(),
            index_encoded_bytes: 0,
        }
    }

    fn new_run(
        store: &ObjectStore,
        lease: &AttemptLease,
        references: &[&BlobRef],
    ) -> (RunPostingRoot, BlobRef) {
        bootstrap_new_run_postings(
            store,
            lease,
            1,
            references,
            super::super::super::policy().bucket_scratch_bytes,
        )
        .expect("physical run")
    }

    fn rewritten_counts(
        store: &ObjectStore,
        lease: &AttemptLease,
        source: &RunPostingRoot,
        change: impl FnOnce(&mut Vec<(Vec<u8>, BlobRef)>, &ObjectStore, &AttemptLease),
    ) -> (RunPostingRoot, BlobRef) {
        let mut cursor = store
            .map_cursor(&source.gram_counts, &[], None, None)
            .expect("count cursor");
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().expect("count row") {
            rows.push(row);
        }
        change(&mut rows, store, lease);
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        let mut altered = source.clone();
        altered.gram_count_record_encoded_bytes = rows.iter().map(|(_, r)| r.root.bytes).sum();
        altered.gram_counts = store
            .map_bootstrap(
                lease,
                rows.into_iter().map(|(key, value)| {
                    Ok(MapEdit {
                        key,
                        value: Some(value),
                    })
                }),
            )
            .expect("altered count map");
        let reference = store
            .put_blob(
                lease,
                ObjectKind::Record,
                &altered.object_roots(),
                &altered.encode().expect("altered root wire"),
            )
            .expect("altered root object");
        (altered, reference)
    }

    #[test]
    fn fresh_run_proves_multiple_buckets_and_final_gram_from_one_leaf_pass() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let leaf = segment(&store, &lease, vec![1, 257, 513]);
        let interleaved = segment(&store, &lease, vec![2, 259, 769]);
        let (root, reference) = new_run(&store, &lease, &[&interleaved, &leaf]);
        assert!(root.is_block_layout());
        assert_eq!((root.logical_pages(), root.gram_counts.entries), (8, 2));
        assert!(root.pages.entries > 0);
        assert_eq!((root.path_rows, root.content_rows), (6, 6));
        for key in [b"\x01abc".as_slice(), b"\x02xyz".as_slice()] {
            let count_ref = store
                .map_get(&root.gram_counts, key, None)
                .expect("count lookup")
                .expect("literal gram count");
            assert_eq!(
                read_count(&store, key, &count_ref, None)
                    .expect("authenticated count")
                    .count,
                6
            );
        }
        verify_changed_run(
            &store,
            1,
            None,
            Some((&root, &reference)),
            &[],
            &[input(&interleaved), input(&leaf)],
            None,
        )
        .expect("independent fresh run proof");
        verify_complete_run(&store, &root, &reference, None)
            .expect("independent complete run scrub");
    }

    #[test]
    fn fresh_v2_rejects_self_consistent_run_missing_one_middle_logical_page() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let included = segment(&store, &lease, vec![1, 513]);
        let omitted = path_only_segment(&store, &lease, 257);
        let (root, reference) = new_run(&store, &lease, &[&included]);
        assert!(root.is_block_layout());
        assert_eq!(root.logical_pages(), 4);
        // The persisted V2 block directory, physical byte ledger, row totals,
        // and count records form a valid run for the included leaf alone.
        verify_changed_run(
            &store,
            1,
            None,
            Some((&root, &reference)),
            &[],
            &[input(&included)],
            None,
        )
        .expect("self-consistent subset run");
        verify_complete_run(&store, &root, &reference, None)
            .expect("self-consistent subset full census");
        // The independent source oracle now supplies one missing path page at
        // bucket 1, between the two included path buckets.
        assert!(matches!(
            verify_changed_run(
                &store,
                1,
                None,
                Some((&root, &reference)),
                &[],
                &[input(&included), input(&omitted)],
                None,
            ),
            Err(CoreError::Typed { code, message })
                if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
                    && message.contains("fresh run page key differs from leaf oracle")
        ));
    }

    #[test]
    fn fresh_run_refuses_wrong_logical_page_count_and_extra_block() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let leaf = segment(&store, &lease, vec![1, 257]);
        let (root, _) = new_run(&store, &lease, &[&leaf]);
        let mut blocks = store
            .map_cursor(&root.pages, &[], None, None)
            .expect("block directory");
        let (_last, last_ref) = blocks
            .next()
            .expect("first block entry")
            .expect("first block");
        let extra = super::run_page_key(2, *b"zzz", 257).expect("extra page key");
        for defect in 0..2 {
            let mut altered = root.clone();
            if defect == 0 {
                altered.block_pages = Some(root.logical_pages() - 1);
            } else {
                altered.pages = store
                    .map_apply(
                        &lease,
                        &root.pages,
                        std::iter::once(Ok(MapEdit {
                            key: extra.to_vec(),
                            value: Some(last_ref.clone()),
                        })),
                    )
                    .expect("authenticated altered block directory");
            }
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Record,
                    &altered.object_roots(),
                    &altered.encode().expect("altered run wire"),
                )
                .expect("authenticated altered run");
            assert!(matches!(
                verify_changed_run(&store, 1, None, Some((&altered, &reference)), &[], &[input(&leaf)], None),
                Err(CoreError::Typed { code, .. })
                    if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
            ));
        }
    }

    #[test]
    fn fresh_run_refuses_malformed_wrong_missing_and_extra_authenticated_counts() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let leaf = segment(&store, &lease, vec![1, 257, 513]);
        let (root, _) = new_run(&store, &lease, &[&leaf]);
        for defect in 0..4 {
            let (bad, bad_ref) = rewritten_counts(&store, &lease, &root, |rows, store, lease| {
                match defect {
                    0 => {
                        rows[0].1 = store
                            .put_blob(lease, ObjectKind::Record, &[], b"not a gram count")
                            .expect("malformed authenticated count object");
                    }
                    1 => {
                        let count = RunGramCount {
                            surface: 1,
                            gram: *b"abc",
                            count: 4,
                        };
                        rows[0].1 = store
                            .put_blob(
                                lease,
                                ObjectKind::Record,
                                &[],
                                &count.encode().expect("wrong count wire"),
                            )
                            .expect("wrong count object");
                    }
                    2 => {
                        assert!(rows.pop().is_some());
                    }
                    _ => {
                        let count = RunGramCount {
                            surface: 2,
                            gram: *b"zzz",
                            count: 1,
                        };
                        let reference = store
                            .put_blob(
                                lease,
                                ObjectKind::Record,
                                &[],
                                &count.encode().expect("extra count wire"),
                            )
                            .expect("extra count object");
                        rows.push((count.key().expect("extra count key").to_vec(), reference));
                    }
                }
            });
            assert!(matches!(
                verify_changed_run(
                    &store,
                    1,
                    None,
                    Some((&bad, &bad_ref)),
                    &[],
                    &[input(&leaf)],
                    None,
                ),
                Err(CoreError::Typed { code, .. })
                    if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
            ));
        }
        let mut bad_ledger = root.clone();
        bad_ledger.gram_count_record_encoded_bytes += 1;
        let bad_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &bad_ledger.object_roots(),
                &bad_ledger.encode().expect("bad ledger root wire"),
            )
            .expect("bad ledger root object");
        assert!(matches!(
            verify_changed_run(
                &store,
                1,
                None,
                Some((&bad_ledger, &bad_ref)),
                &[],
                &[input(&leaf)],
                None,
            ),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
        ));
        let mut bad_block_ledger = root.clone();
        bad_block_ledger.page_record_encoded_bytes += 1;
        let bad_block_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &bad_block_ledger.object_roots(),
                &bad_block_ledger
                    .encode()
                    .expect("bad block ledger root wire"),
            )
            .expect("bad block ledger root object");
        assert!(matches!(
            verify_changed_run(
                &store,
                1,
                None,
                Some((&bad_block_ledger, &bad_block_ref)),
                &[],
                &[input(&leaf)],
                None,
            ),
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
        ));
    }

    #[test]
    fn existing_run_delta_keeps_the_independent_changed_gram_path() {
        let directory = tempfile::tempdir().expect("temporary store");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let first = segment(&store, &lease, vec![1]);
        let added = segment(&store, &lease, vec![257]);
        let (old, old_ref) = new_run(&store, &lease, &[&first]);
        let (next, next_ref) = new_run(&store, &lease, &[&first, &added]);
        verify_changed_run(
            &store,
            1,
            Some((&old, &old_ref)),
            Some((&next, &next_ref)),
            &[],
            &[input(&added)],
            None,
        )
        .expect("independent existing-run delta proof");
    }
}
