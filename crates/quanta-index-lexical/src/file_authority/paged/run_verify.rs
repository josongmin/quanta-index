//! Independent run-posting proof from authenticated segment term masks and
//! sorted map cursors. Producer page-edit and merge routines are not used.

use std::collections::{BTreeMap, VecDeque};

use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::lexical_runs::{SegmentInput, SegmentRemoval};
use crate::lexical_store::{BlobRef, MapCursor, MapRoot, ObjectKind, ObjectStore};

use super::run_postings::{
    RunGramCount, RunPostingPage, RunPostingRoot, RunPostingRow, run_page_key, run_route_key,
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
    pending: VecDeque<([u8; 12], RunPostingRow)>,
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
            pending: VecDeque::new(),
            observed: [0; 2],
            finished: false,
        })
    }

    fn next(&mut self) -> Result<Option<([u8; 12], RunPostingRow)>, CoreError> {
        loop {
            checkpoint(self.budget)?;
            if let Some(row) = self.pending.pop_front() {
                return Ok(Some(row));
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
            let mut rows = BTreeMap::<[u8; 12], Vec<RunPostingRow>>::new();
            for (local, id) in self.segment.doc_ids.iter().enumerate() {
                let bit = 1_u64
                    .checked_shl(
                        u32::try_from(local).map_err(|_width| corrupt("leaf local ID width"))?,
                    )
                    .ok_or_else(|| corrupt("leaf local ID exceeds posting mask"))?;
                if mask & bit != 0 {
                    rows.entry(run_page_key(self.surface, gram, *id)?)
                        .or_default()
                        .push(RunPostingRow {
                            logical_id: *id,
                            segment_sha256: self.digest,
                        });
                }
            }
            if rows.is_empty() {
                return Err(corrupt("leaf term mask has no document"));
            }
            for (key, members) in rows {
                for row in members {
                    self.pending.push_back((key, row));
                }
            }
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
    ready: Vec<Option<([u8; 12], RunPostingRow)>>,
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
        let mut ready = Vec::new();
        for stream in &mut streams {
            ready.push(stream.next()?);
        }
        Ok(Self { streams, ready })
    }

    fn next(&mut self) -> Result<Option<BucketDelta>, CoreError> {
        let Some(key) = self.ready.iter().flatten().map(|(key, _)| *key).min() else {
            return Ok(None);
        };
        let mut delta = BucketDelta {
            key,
            removed: BTreeMap::new(),
            added: BTreeMap::new(),
        };
        for (stream, ready) in self.streams.iter_mut().zip(&mut self.ready) {
            while ready.as_ref().is_some_and(|(head, _)| *head == key) {
                let (_, row) = ready
                    .take()
                    .ok_or_else(|| corrupt("changed bucket head absent"))?;
                let side = if stream.removed {
                    &mut delta.removed
                } else {
                    &mut delta.added
                };
                if side.insert(row.logical_id, row.segment_sha256).is_some() {
                    return Err(corrupt(
                        "changed leaves repeat a logical ID within a bucket",
                    ));
                }
                *ready = stream.next()?;
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
        Ok(Self {
            store,
            budget,
            old: base.map_or_else(MapRoot::default, |root| root.pages.clone()),
            target: target.map_or_else(MapRoot::default, |root| root.pages.clone()),
            buckets: ChangedBuckets::new(store, removed, added, budget)?,
            rows: base.map_or([0, 0], |root| [root.path_rows, root.content_rows]),
            record_bytes: base.map_or(0, |root| root.page_record_encoded_bytes),
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
    let mut pages = ChangedPageProof::new(store, old, next, removed, added, budget)?;
    let mut expected = pages.next_changed()?;
    let old_pages = pages.old.clone();
    let new_pages = pages.target.clone();
    store.map_changed_keys(&old_pages, &new_pages, budget, &mut |key, before, after| {
        let Some((expected_key, old_ref, new_ref)) = expected.take() else {
            return Err(corrupt("run page map has unrelated edit"));
        };
        if key != expected_key.as_slice() || before != old_ref.as_ref() || after != new_ref.as_ref()
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
    if pages.rows != next.map_or([0, 0], |root| [root.path_rows, root.content_rows])
        || pages.record_bytes != next.map_or(0, |root| root.page_record_encoded_bytes)
    {
        return Err(corrupt("run page row or Record byte ledger differs"));
    }

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
    Ok(())
}

/// Full sorted census used only for selected clean input runs and the new
/// output run. A single page and gram counter are live at each cursor level.
fn verify_complete_run(
    store: &ObjectStore,
    root: &RunPostingRoot,
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    verify_root_reference(store, root.run_id, root, reference, budget)?;
    let _bundle = verify_bundle_structure(store, root, reference, budget)?;
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
        let mut cursor = store.map_cursor(&root.pages, &[], None, budget)?;
        ready.push(cursor.next()?);
        cursors.push(cursor);
    }
    let mut output = store.map_cursor(&target.0.pages, &[], None, budget)?;
    let mut output_ready = output.next()?;
    while let Some(key) = ready.iter().flatten().map(|(key, _)| key.clone()).min() {
        checkpoint(budget)?;
        let mut expected = BTreeMap::<u64, [u8; 32]>::new();
        for (cursor, head) in cursors.iter_mut().zip(&mut ready) {
            if head.as_ref().is_some_and(|(current, _)| *current == key) {
                let (_, reference) = head
                    .take()
                    .ok_or_else(|| corrupt("input page head absent"))?;
                let page = read_run_page(store, &key, &reference, budget)?;
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
        let Some((output_key, output_ref)) = output_ready.take() else {
            return Err(corrupt("clean merged output omits an input page"));
        };
        if output_key != key {
            return Err(corrupt("clean merged output page key differs"));
        }
        let actual = read_run_page(store, &output_key, &output_ref, budget)?;
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
