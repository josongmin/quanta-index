//! Independent V17 changed-segment oracle. It derives exact gram membership
//! from authenticated source bodies; the posting producer is never called.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::SourceFileKey;
use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::trigrams_of;
use sha2::{Digest as _, Sha256};

use crate::lexical_allocation::DocVersionReservation;
use crate::lexical_runs::{
    RunCandidate, RunRegistry, RunScanWindow, RunScrubCursor, RunSelectionProgress, SegmentInput,
    SegmentRemoval, bounded_candidate_scan_limit, segment_index_encoded_bytes,
};
use crate::lexical_store::{
    Authority as ScanAuthority, CandidateWitness, EligibilityWitness, IneligibleReason, ScanLane,
    WindowWitness,
};
use crate::lexical_store::{BlobRef, MapCursor, MapRoot, ObjectKind, ObjectRef, ObjectStore};
use crate::maintenance_merge_facts::FileRunMergeFacts;

use super::producer::{FileAuthorityMutation, FileSourceEdit};
use super::run_postings::{
    RunGramCount, RunPostingPage, RunPostingRoot, run_merge_stage_bytes, run_page_key,
    run_route_key,
};
use super::run_verify::{verify_changed_run, verify_clean_run_union, verify_global_run_routes};
use super::{
    AuthorityRoot, LiveDoc, PostingPage, Segment, SourceHead, doc_key, membership_key,
    source_key_digest,
};

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file authority v17 seal: {reason}"),
    }
}

fn checkpoint(budget: Option<&RequestBudgetV1>, stage: &'static str) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint(stage)?;
    }
    Ok(())
}

fn size_u64(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value).map_err(|_width_error| corrupt("size exceeds u64"))
}

fn index_usize(value: u32) -> Result<usize, CoreError> {
    usize::try_from(value).map_err(|_width_error| corrupt("local ID exceeds usize"))
}

fn route_parts(key: &[u8], surface: u8) -> Result<([u8; 3], u64, u64), CoreError> {
    if key.len() != 20 || key.first() != Some(&surface) {
        return Err(corrupt("route key shape differs"));
    }
    let gram = key
        .get(1..4)
        .ok_or_else(|| corrupt("route gram absent"))?
        .try_into()
        .map_err(|_width_error| corrupt("route gram width differs"))?;
    let count_bytes: [u8; 8] = key
        .get(4..12)
        .ok_or_else(|| corrupt("route count absent"))?
        .try_into()
        .map_err(|_width_error| corrupt("route count width differs"))?;
    let run_bytes: [u8; 8] = key
        .get(12..20)
        .ok_or_else(|| corrupt("route run absent"))?
        .try_into()
        .map_err(|_width_error| corrupt("route run width differs"))?;
    Ok((
        gram,
        u64::from_be_bytes(count_bytes),
        u64::from_be_bytes(run_bytes),
    ))
}

fn source_head(
    store: &ObjectStore,
    root: &AuthorityRoot,
    digest: [u8; 32],
    budget: Option<&RequestBudgetV1>,
) -> Result<(BlobRef, SourceHead), CoreError> {
    let reference = store
        .map_get(&root.source_heads, &digest, budget)?
        .ok_or_else(|| corrupt("current source head is absent"))?;
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("source head kind differs"));
    }
    let bytes = store.read_blob(&reference, size_u64(super::MAX_RECORD_BYTES)?, budget)?;
    let head = SourceHead::decode(&bytes)?;
    if source_key_digest(&head.coverage.source.file)? != digest {
        return Err(corrupt("source key digest differs from map key"));
    }
    let mut expected = vec![head.body.root.clone()];
    expected.sort();
    if store.object_dependencies(&reference.root, budget)? != expected {
        return Err(corrupt("source head omits or adds body dependency"));
    }
    Ok((reference, head))
}

fn optional_source_head(
    store: &ObjectStore,
    root: &AuthorityRoot,
    key: &SourceFileKey,
) -> Result<Option<(BlobRef, SourceHead)>, CoreError> {
    let digest = source_key_digest(key)?;
    if store.map_get(&root.source_heads, &digest, None)?.is_none() {
        return Ok(None);
    }
    let row = source_head(store, root, digest, None)?;
    if row.1.coverage.source.file != *key {
        return Err(corrupt("source key digest collision"));
    }
    Ok(Some(row))
}

fn live_doc(
    store: &ObjectStore,
    root: &AuthorityRoot,
    id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<LiveDoc, CoreError> {
    let reference = store
        .map_get(&root.current_docs, &doc_key(id), budget)?
        .ok_or_else(|| corrupt("current logical ID is absent"))?;
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("current record kind differs"));
    }
    let row = LiveDoc::decode(&store.read_blob(
        &reference,
        size_u64(super::MAX_RECORD_BYTES)?,
        budget,
    )?)?;
    if row.logical_id != id {
        return Err(corrupt("current record ID differs from key"));
    }
    Ok(row)
}

fn derive_grams(bytes: &[u8]) -> Result<Vec<[u8; 3]>, CoreError> {
    // A direct raw-window oracle uses no fixed universe scan for small files.
    // Its scratch is at most the 2 MiB bitmap alternative below. Larger
    // normalized surfaces use the fixed 24-bit universe instead of allocating
    // a vector proportional to their potentially expanded byte length.
    let windows = bytes.len().saturating_sub(2);
    // Floor(2 MiB / three raw bytes); preserve the fixed scratch bound.
    if windows <= 699_050 {
        let mut grams = Vec::new();
        grams.try_reserve_exact(windows).map_err(|error| {
            CoreError::Storage(format!(
                "file authority v17 seal: reserve raw oracle grams: {error}"
            ))
        })?;
        for gram in bytes.windows(3) {
            let raw: [u8; 3] = gram
                .try_into()
                .map_err(|_width_error| corrupt("raw gram width differs"))?;
            grams.push(raw);
        }
        grams.sort_unstable();
        grams.dedup();
        return Ok(grams);
    }
    // This independent raw-byte oracle never calls the posting producer and
    // has 2 MiB fixed bitmap scratch for large normalized surfaces.
    let mut bitmap = Vec::new();
    bitmap.try_reserve_exact(1 << 21).map_err(|error| {
        CoreError::Storage(format!(
            "file authority v17 seal: reserve oracle bitmap: {error}"
        ))
    })?;
    bitmap.resize(1 << 21, 0_u8);
    for [first, second, third] in trigrams_of(bytes) {
        let value = (usize::from(first) << 16) | (usize::from(second) << 8) | usize::from(third);
        let slot = bitmap
            .get_mut(value >> 3)
            .ok_or_else(|| corrupt("raw gram exceeds bitmap universe"))?;
        *slot |= 1_u8 << (value & 7);
    }
    let count = bitmap.iter().try_fold(0_usize, |sum, byte| {
        sum.checked_add(index_usize(byte.count_ones())?)
            .ok_or_else(|| corrupt("oracle gram count overflow"))
    })?;
    let mut grams = Vec::new();
    grams.try_reserve_exact(count).map_err(|error| {
        CoreError::Storage(format!(
            "file authority v17 seal: reserve oracle grams: {error}"
        ))
    })?;
    for (index, bits) in bitmap.into_iter().enumerate() {
        for bit in 0..8_u8 {
            if bits & (1_u8 << bit) != 0 {
                let value = (index << 3) | usize::from(bit);
                grams.push([
                    u8::try_from((value >> 16) & 0xff)
                        .map_err(|_width_error| corrupt("first gram byte exceeds u8"))?,
                    u8::try_from((value >> 8) & 0xff)
                        .map_err(|_width_error| corrupt("second gram byte exceeds u8"))?,
                    u8::try_from(value & 0xff)
                        .map_err(|_width_error| corrupt("third gram byte exceeds u8"))?,
                ]);
            }
        }
    }
    Ok(grams)
}

/// Independent k-way stream over source-derived gram vectors. Memory is
/// bounded by one source vector per segment local ID plus at most 64 heads.
struct ExpectedTerms<'a> {
    sources: &'a [Vec<[u8; 3]>],
    offsets: Vec<usize>,
    ready: BTreeSet<([u8; 3], usize)>,
}

impl<'a> ExpectedTerms<'a> {
    fn new(sources: &'a [Vec<[u8; 3]>]) -> Result<Self, CoreError> {
        let mut offsets = Vec::new();
        offsets.try_reserve_exact(sources.len()).map_err(|error| {
            CoreError::Storage(format!(
                "file authority v17 seal: reserve oracle cursors: {error}"
            ))
        })?;
        let mut ready = BTreeSet::new();
        for (local, grams) in sources.iter().enumerate() {
            offsets.push(0);
            if let Some(gram) = grams.first() {
                let _inserted = ready.insert((*gram, local));
            }
        }
        Ok(Self {
            sources,
            offsets,
            ready,
        })
    }

    fn next_term(&mut self) -> Result<Option<([u8; 3], u64)>, CoreError> {
        let Some((gram, local)) = self.ready.pop_first() else {
            return Ok(None);
        };
        let mut mask = 0_u64;
        self.advance(local, gram, &mut mask)?;
        while self.ready.first().is_some_and(|(next, _)| *next == gram) {
            let (_, local) = self
                .ready
                .pop_first()
                .ok_or_else(|| corrupt("oracle merge head disappeared"))?;
            self.advance(local, gram, &mut mask)?;
        }
        Ok(Some((gram, mask)))
    }

    fn advance(&mut self, local: usize, gram: [u8; 3], mask: &mut u64) -> Result<(), CoreError> {
        if local >= super::MAX_SEGMENT_DOCS {
            return Err(corrupt("oracle local ID exceeds bitmask width"));
        }
        *mask |= 1_u64 << local;
        let offset = self
            .offsets
            .get_mut(local)
            .ok_or_else(|| corrupt("oracle cursor offset absent"))?;
        *offset = offset
            .checked_add(1)
            .ok_or_else(|| corrupt("oracle cursor overflow"))?;
        let grams = self
            .sources
            .get(local)
            .ok_or_else(|| corrupt("oracle source absent"))?;
        if let Some(next) = grams.get(*offset) {
            if *next <= gram {
                return Err(corrupt("oracle source grams are not strictly ordered"));
            }
            let _inserted = self.ready.insert((*next, local));
        }
        Ok(())
    }
}

/// Authenticates each posting page once, including its complete term inventory.
struct TermPageStream<'a> {
    store: &'a ObjectStore,
    cursor: MapCursor<'a>,
    page_cache: Option<(BlobRef, PostingPage, usize)>,
    seen_pages: BTreeSet<[u8; 32]>,
    doc_count: usize,
    finished: bool,
}

impl<'a> TermPageStream<'a> {
    fn new(store: &'a ObjectStore, terms: &MapRoot, doc_count: usize) -> Result<Self, CoreError> {
        Ok(Self {
            store,
            cursor: store.map_cursor(terms, &[], None, None)?,
            page_cache: None,
            seen_pages: BTreeSet::new(),
            doc_count,
            finished: false,
        })
    }

    fn next_term(&mut self) -> Result<Option<([u8; 3], u64)>, CoreError> {
        if self.finished {
            return Ok(None);
        }
        let Some((key, page_ref)) = self.cursor.next()? else {
            if let Some((_, page, consumed)) = self.page_cache.take()
                && consumed != page.0.len()
            {
                return Err(corrupt("last posting page contains an unrouted term"));
            }
            self.finished = true;
            return Ok(None);
        };
        let gram: [u8; 3] = key
            .as_slice()
            .try_into()
            .map_err(|_conversion_error| corrupt("term map key is not a gram"))?;
        if page_ref.root.kind != ObjectKind::FilePostings {
            return Err(corrupt("term map page kind differs"));
        }
        if self
            .page_cache
            .as_ref()
            .is_none_or(|(prior, _, _)| *prior != page_ref)
        {
            if let Some((_, page, consumed)) = self.page_cache.take()
                && consumed != page.0.len()
            {
                return Err(corrupt("posting page contains an unrouted term"));
            }
            if !self.seen_pages.insert(page_ref.root.sha256) {
                return Err(corrupt("posting page is referenced nonconsecutively"));
            }
            let bytes =
                self.store
                    .read_blob(&page_ref, size_u64(super::MAX_POSTING_PAGE_BYTES)?, None)?;
            self.page_cache = Some((page_ref, PostingPage::decode(&bytes, self.doc_count)?, 0));
        }
        let (_, page, consumed) = self
            .page_cache
            .as_mut()
            .ok_or_else(|| corrupt("posting page cache is absent"))?;
        let (page_gram, page_mask) = page
            .0
            .get(*consumed)
            .ok_or_else(|| corrupt("posting page omits a routed term"))?;
        if *page_gram != gram {
            return Err(corrupt("term map gram differs from posting page"));
        }
        *consumed = consumed
            .checked_add(1)
            .ok_or_else(|| corrupt("posting page cursor overflows"))?;
        Ok(Some((gram, *page_mask)))
    }
}

fn verify_term_map(
    store: &ObjectStore,
    _digest: [u8; 32],
    _surface: u8,
    terms: &MapRoot,
    expected_sources: &[Vec<[u8; 3]>],
    doc_count: usize,
) -> Result<(), CoreError> {
    let mut stream = TermPageStream::new(store, terms, doc_count)?;
    let mut expected = ExpectedTerms::new(expected_sources)?;
    while let Some((gram, page_mask)) = stream.next_term()? {
        let (expected_gram, expected_mask) = expected
            .next_term()?
            .ok_or_else(|| corrupt("term map contains an extra gram"))?;
        if gram != expected_gram || page_mask != expected_mask {
            return Err(corrupt("posting IDs differ from source-body gram oracle"));
        }
    }
    if expected.next_term()?.is_some() {
        return Err(corrupt("term map omits a source-body gram"));
    }
    Ok(())
}

/// Verify one newly produced or compacted segment against its current sources.
///
/// The outer sealed-generation owner authenticates the base. This function
/// independently checks every logical membership of the selected segment.
fn verify_segment(
    store: &ObjectStore,
    root: &AuthorityRoot,
    reference: &BlobRef,
) -> Result<(), CoreError> {
    if reference.root.kind != ObjectKind::Segment {
        return Err(corrupt("segment value has wrong kind"));
    }
    let digest = reference.root.sha256;
    let committed = store
        .map_get(&root.segments, &digest, None)?
        .ok_or_else(|| corrupt("segment map omits selected segment"))?;
    if committed != *reference {
        return Err(corrupt("segment map value differs"));
    }
    let segment =
        Segment::decode(&store.read_blob(reference, size_u64(super::MAX_RECORD_BYTES)?, None)?)?;
    let mut dependencies = Vec::<ObjectRef>::new();
    for map in [&segment.path_terms, &segment.content_terms] {
        if let Some(page) = &map.page {
            dependencies.push(page.object.clone());
        }
    }
    dependencies.sort();
    dependencies.dedup();
    if store.object_dependencies(&reference.root, None)? != dependencies {
        return Err(corrupt(
            "segment descriptor and object dependency set differ",
        ));
    }
    let mut expected_path = vec![Vec::new(); segment.doc_ids.len()];
    let mut expected_content = vec![Vec::new(); segment.doc_ids.len()];
    let mut path_memberships = 0_u64;
    let mut content_memberships = 0_u64;
    for (local, id) in segment.doc_ids.iter().enumerate() {
        let locator = live_doc(store, root, *id, None)?;
        if locator.segment_sha256 != digest || index_usize(locator.local_id)? != local {
            return Err(corrupt(
                "current document locator differs from segment membership",
            ));
        }
        let (head_ref, head) = source_head(store, root, locator.source_key_sha256, None)?;
        if head.logical_id != *id {
            return Err(corrupt("source head and segment logical ID differ"));
        }
        let membership = store
            .map_get(
                &root.memberships,
                &membership_key(locator.source_key_sha256, *id),
                None,
            )?
            .ok_or_else(|| corrupt("source membership row is absent"))?;
        if membership != head_ref {
            return Err(corrupt("source membership and head reference differ"));
        }
        if head.body.root.kind != ObjectKind::FileBody {
            return Err(corrupt("source body kind differs"));
        }
        let bytes = store.read_blob(&head.body, size_u64(super::super::MAX_FILE_BYTES)?, None)?;
        if size_u64(bytes.len())? != head.source_bytes
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != head.coverage.source.source_sha256
        {
            return Err(corrupt("source body differs from coverage revision"));
        }
        let raw = if head.coverage.text_admitted {
            Some(
                std::str::from_utf8(&bytes)
                    .map_err(|_conversion_error| corrupt("text source is not UTF-8"))?,
            )
        } else {
            None
        };
        let scratch = usize::try_from(super::super::policy().bucket_scratch_bytes)
            .map_err(|error| corrupt(&format!("normalization scratch width: {error}")))?;
        let (_, folded_path, indexed_content, folded_content) = super::super::normalized_surfaces(
            head.coverage.source.file.repo_relative_path.as_str(),
            raw,
            0,
            scratch,
        )?;
        if size_u64(indexed_content.as_ref().map_or(0, String::len))? != head.indexed_text_bytes
            || size_u64(folded_content.as_ref().map_or(0, String::len))? != head.folded_text_bytes
        {
            return Err(corrupt("source normalization lengths differ from head"));
        }
        let path_grams = derive_grams(folded_path.as_bytes())?;
        let path = size_u64(path_grams.len())?;
        *expected_path
            .get_mut(local)
            .ok_or_else(|| corrupt("path local outside segment"))? = path_grams;
        let content = match folded_content {
            Some(content) => {
                let grams = derive_grams(content.as_bytes())?;
                let count = size_u64(grams.len())?;
                *expected_content
                    .get_mut(local)
                    .ok_or_else(|| corrupt("content local outside segment"))? = grams;
                count
            }
            None => 0,
        };
        if path.checked_add(content) != Some(head.posting_memberships) {
            return Err(corrupt(
                "source head membership count differs from source oracle",
            ));
        }
        path_memberships = path_memberships
            .checked_add(path)
            .ok_or_else(|| corrupt("path membership sum overflow"))?;
        content_memberships = content_memberships
            .checked_add(content)
            .ok_or_else(|| corrupt("content membership sum overflow"))?;
    }
    if path_memberships != segment.path_memberships
        || content_memberships != segment.content_memberships
    {
        return Err(corrupt(
            "segment membership count differs from source oracle",
        ));
    }
    verify_term_map(
        store,
        digest,
        1,
        &segment.path_terms,
        &expected_path,
        segment.doc_ids.len(),
    )?;
    verify_term_map(
        store,
        digest,
        2,
        &segment.content_terms,
        &expected_content,
        segment.doc_ids.len(),
    )?;
    Ok(())
}

pub(crate) fn verify_source_absent(
    store: &ObjectStore,
    root: &AuthorityRoot,
    key: &SourceFileKey,
) -> Result<(), CoreError> {
    if store
        .map_get(&root.source_heads, &source_key_digest(key)?, None)?
        .is_some()
    {
        return Err(corrupt("deleted source remains current"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct LiveTermEvidence<'a> {
    doc_ids: &'a [u64],
    active: &'a [bool],
    expected_sources: &'a [Vec<[u8; 3]>],
}

fn verify_live_term_map(
    store: &ObjectStore,
    root: &AuthorityRoot,
    segment_ref: &BlobRef,
    terms: &MapRoot,
    surface: u8,
    evidence: &LiveTermEvidence<'_>,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    let doc_ids = evidence.doc_ids;
    let active = evidence.active;
    let expected_sources = evidence.expected_sources;
    if doc_ids.len() != active.len() {
        return Err(corrupt("scrub segment local census differs"));
    }
    let run_id = root
        .run_registry
        .owner_run_id(store, segment_ref.root.sha256, budget)?;
    let run_ref = store
        .map_get(&root.run_postings, &run_id.to_be_bytes(), budget)?
        .ok_or_else(|| corrupt("scrub physical leaf has no run postings"))?;
    let run = RunPostingRoot::load(store, &run_ref, budget)?;
    if run.run_id != run_id {
        return Err(corrupt("scrub physical leaf run ID differs"));
    }
    let mut cursor = store.map_cursor(terms, &[], None, budget)?;
    let mut page_cache: Option<(BlobRef, PostingPage, usize)> = None;
    let mut seen_pages = BTreeSet::new();
    let mut expected = ExpectedTerms::new(expected_sources)?;
    let mut expected_next = expected.next_term()?;
    let active_mask =
        active.iter().enumerate().fold(
            0_u64,
            |mask, (local, live)| {
                if *live { mask | (1_u64 << local) } else { mask }
            },
        );
    while let Some((key, page_ref)) = cursor.next()? {
        checkpoint(budget, "lexical:file-scrub-term")?;
        let gram: [u8; 3] = key
            .as_slice()
            .try_into()
            .map_err(|_conversion_error| corrupt("scrub term key is not a gram"))?;
        if page_ref.root.kind != ObjectKind::FilePostings {
            return Err(corrupt("scrub posting page kind differs"));
        }
        if page_cache
            .as_ref()
            .is_none_or(|(previous, _, _)| previous != &page_ref)
        {
            if let Some((_, page, consumed)) = page_cache.take()
                && consumed != page.0.len()
            {
                return Err(corrupt("scrub posting page has an unrouted term"));
            }
            if !seen_pages.insert(page_ref.root.sha256) {
                return Err(corrupt("scrub posting page appears nonconsecutively"));
            }
            let bytes =
                store.read_blob(&page_ref, size_u64(super::MAX_POSTING_PAGE_BYTES)?, budget)?;
            page_cache = Some((page_ref, PostingPage::decode(&bytes, active.len())?, 0));
        }
        let (_, page, consumed) = page_cache
            .as_mut()
            .ok_or_else(|| corrupt("scrub page cache is absent"))?;
        let (page_gram, page_mask) = page
            .0
            .get(*consumed)
            .ok_or_else(|| corrupt("scrub page omits routed term"))?;
        if *page_gram != gram {
            return Err(corrupt("scrub term map differs from posting page"));
        }
        *consumed = consumed
            .checked_add(1)
            .ok_or_else(|| corrupt("scrub posting page cursor overflows"))?;
        if expected_next.as_ref().is_some_and(|(next, _)| *next < gram) {
            return Err(corrupt("scrub segment omits a live source gram"));
        }
        let expected_mask = if expected_next
            .as_ref()
            .is_some_and(|(next, _)| *next == gram)
        {
            let (_, mask) = expected_next
                .take()
                .ok_or_else(|| corrupt("scrub oracle cursor disappeared"))?;
            expected_next = expected.next_term()?;
            mask
        } else {
            0
        };
        if (*page_mask & active_mask) != expected_mask {
            return Err(corrupt("live posting differs from source-body oracle"));
        }
        for (local, id) in doc_ids.iter().enumerate() {
            if *page_mask & (1_u64 << local) == 0 {
                continue;
            }
            let bucket = run_page_key(surface, gram, *id)?;
            let run_page_ref = store
                .map_get(&run.pages, &bucket, budget)?
                .ok_or_else(|| corrupt("physical leaf posting omitted from run page"))?;
            let run_page = RunPostingPage::decode(&store.read_blob(
                &run_page_ref,
                size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                budget,
            )?)?;
            let matching_member = run_page
                .rows
                .binary_search_by_key(id, |row| row.logical_id)
                .is_ok_and(|index| {
                    run_page
                        .rows
                        .get(index)
                        .is_some_and(|row| row.segment_sha256 == segment_ref.root.sha256)
                });
            if run_page.key()? != bucket || !matching_member {
                return Err(corrupt("physical leaf posting differs from run page"));
            }
        }
    }
    if expected_next.is_some() {
        return Err(corrupt("scrub segment omits a live source gram"));
    }
    if let Some((_, page, consumed)) = page_cache
        && consumed != page.0.len()
    {
        return Err(corrupt("scrub last page contains an unrouted term"));
    }
    Ok(())
}

fn verify_scrub_segment_row(
    store: &ObjectStore,
    root: &AuthorityRoot,
    key: &[u8],
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<u64, CoreError> {
    checkpoint(budget, "lexical:file-scrub-segment")?;
    let mut active_count = 0_u64;
    let digest: [u8; 32] = key
        .try_into()
        .map_err(|_conversion_error| corrupt("scrub segment key width"))?;
    if reference.root.kind != ObjectKind::Segment || reference.root.sha256 != digest {
        return Err(corrupt("scrub segment reference differs from key"));
    }
    let physical_index_bytes = segment_index_encoded_bytes(
        store,
        std::slice::from_ref(&reference.root),
        budget,
        size_u64(crate::lexical_store::MAX_CLOSURE_REFS)?,
    )?;
    root.run_registry
        .verify_segment(store, key, reference, physical_index_bytes, budget)?;
    let segment = Segment::decode(&store.read_blob(
        reference,
        size_u64(super::MAX_RECORD_BYTES)?,
        budget,
    )?)?;
    let mut dependencies = Vec::new();
    for map in [&segment.path_terms, &segment.content_terms] {
        if let Some(page) = &map.page {
            dependencies.push(page.object.clone());
        }
    }
    dependencies.sort();
    dependencies.dedup();
    if store.object_dependencies(&reference.root, budget)? != dependencies {
        return Err(corrupt("scrub segment dependency set differs"));
    }
    let mut active = vec![false; segment.doc_ids.len()];
    let mut expected_path = vec![Vec::new(); segment.doc_ids.len()];
    let mut expected_content = vec![Vec::new(); segment.doc_ids.len()];
    let mut has_stale = false;
    for (local, id) in segment.doc_ids.iter().enumerate() {
        let Some(current_ref) = store.map_get(&root.current_docs, &doc_key(*id), budget)? else {
            has_stale = true;
            continue;
        };
        let locator = LiveDoc::decode(&store.read_blob(
            &current_ref,
            size_u64(super::MAX_RECORD_BYTES)?,
            budget,
        )?)?;
        if locator.segment_sha256 != digest || index_usize(locator.local_id)? != local {
            has_stale = true;
            continue;
        }
        *active
            .get_mut(local)
            .ok_or_else(|| corrupt("scrub active local outside segment"))? = true;
        active_count = active_count
            .checked_add(1)
            .ok_or_else(|| corrupt("scrub active count overflow"))?;
        let (_, head) = source_head(store, root, locator.source_key_sha256, budget)?;
        if head.logical_id != *id || head.body.root.kind != ObjectKind::FileBody {
            return Err(corrupt("scrub live source/body differs from locator"));
        }
        let source =
            store.read_blob(&head.body, size_u64(super::super::MAX_FILE_BYTES)?, budget)?;
        if size_u64(source.len())? != head.source_bytes
            || <[u8; 32]>::from(Sha256::digest(&source)) != head.coverage.source.source_sha256
        {
            return Err(corrupt("scrub live source body differs from revision"));
        }
        let raw = if head.coverage.text_admitted {
            Some(
                std::str::from_utf8(&source)
                    .map_err(|_conversion_error| corrupt("scrub text source is not UTF-8"))?,
            )
        } else {
            None
        };
        let scratch = usize::try_from(super::super::policy().bucket_scratch_bytes)
            .map_err(|error| corrupt(&format!("scrub scratch width: {error}")))?;
        let (_, folded_path, indexed_content, folded_content) = super::super::normalized_surfaces(
            head.coverage.source.file.repo_relative_path.as_str(),
            raw,
            0,
            scratch,
        )?;
        if size_u64(indexed_content.as_ref().map_or(0, String::len))? != head.indexed_text_bytes
            || size_u64(folded_content.as_ref().map_or(0, String::len))? != head.folded_text_bytes
        {
            return Err(corrupt("scrub normalized source lengths differ"));
        }
        let path_grams = derive_grams(folded_path.as_bytes())?;
        let path = size_u64(path_grams.len())?;
        *expected_path
            .get_mut(local)
            .ok_or_else(|| corrupt("scrub path local outside segment"))? = path_grams;
        let content = match folded_content {
            Some(text) => {
                let grams = derive_grams(text.as_bytes())?;
                let count = size_u64(grams.len())?;
                *expected_content
                    .get_mut(local)
                    .ok_or_else(|| corrupt("scrub content local outside segment"))? = grams;
                count
            }
            None => 0,
        };
        if path.checked_add(content) != Some(head.posting_memberships) {
            return Err(corrupt("scrub source membership count differs"));
        }
    }
    if has_stale
        && store
            .map_get(&root.compaction_debt, &digest, budget)?
            .as_ref()
            != Some(reference)
    {
        return Err(corrupt("stale segment lacks compaction debt"));
    }
    verify_live_term_map(
        store,
        root,
        reference,
        &segment.path_terms,
        1,
        &LiveTermEvidence {
            doc_ids: &segment.doc_ids,
            active: &active,
            expected_sources: &expected_path,
        },
        budget,
    )?;
    verify_live_term_map(
        store,
        root,
        reference,
        &segment.content_terms,
        2,
        &LiveTermEvidence {
            doc_ids: &segment.doc_ids,
            active: &active,
            expected_sources: &expected_content,
        },
        budget,
    )?;
    Ok(active_count)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RunScrubPhase {
    Pages,
    Counts,
}

struct ActiveRunScrub {
    id: u64,
    reference: BlobRef,
    root: RunPostingRoot,
    phase: RunScrubPhase,
    after_key: Option<Vec<u8>>,
    current_gram: Option<[u8; 4]>,
    current_gram_rows: u64,
    page_rows: [u64; 2],
    page_bytes: u64,
    count_bytes: u64,
    seen_pages: u64,
    seen_grams: u64,
    seen_counts: u64,
}

#[derive(Default)]
struct RunPostingScrubCursor {
    after_run: Option<Vec<u8>>,
    active: Option<ActiveRunScrub>,
    seen_runs: u64,
    encoded_bytes: u64,
}

fn scrub_lower(after: Option<&Vec<u8>>) -> Vec<u8> {
    after.map_or_else(Vec::new, |key| {
        let mut lower = key.clone();
        lower.push(0);
        lower
    })
}

fn scrub_run_page_member(
    store: &ObjectStore,
    root: &AuthorityRoot,
    run_id: u64,
    page: &RunPostingPage,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    for row in &page.rows {
        checkpoint(budget, "lexical:file-run-page-member")?;
        if root
            .run_registry
            .owner_run_id(store, row.segment_sha256, budget)?
            != run_id
        {
            return Err(corrupt("run posting row belongs to another run"));
        }
        let segment_ref = store
            .map_get(&root.segments, &row.segment_sha256, budget)?
            .ok_or_else(|| corrupt("run posting row names absent segment"))?;
        if segment_ref.root.kind != ObjectKind::Segment
            || segment_ref.root.sha256 != row.segment_sha256
        {
            return Err(corrupt("run posting row segment reference differs"));
        }
        let segment = Segment::decode(&store.read_blob(
            &segment_ref,
            size_u64(super::MAX_RECORD_BYTES)?,
            budget,
        )?)?;
        let local = segment
            .doc_ids
            .binary_search(&row.logical_id)
            .map_err(|_absent| corrupt("run posting ID absent from physical leaf"))?;
        let terms = if page.surface == 1 {
            &segment.path_terms
        } else {
            &segment.content_terms
        };
        let page_ref = store
            .map_get(terms, &page.gram, budget)?
            .ok_or_else(|| corrupt("run posting gram absent from physical leaf"))?;
        if page_ref.root.kind != ObjectKind::FilePostings {
            return Err(corrupt("physical leaf posting kind differs"));
        }
        let leaf = PostingPage::decode(
            &store.read_blob(&page_ref, size_u64(super::MAX_POSTING_PAGE_BYTES)?, budget)?,
            segment.doc_ids.len(),
        )?;
        let gram_index = leaf
            .0
            .binary_search_by_key(&page.gram, |entry| entry.0)
            .map_err(|_absent| corrupt("physical leaf posting page omits run gram"))?;
        let mask = leaf
            .0
            .get(gram_index)
            .ok_or_else(|| corrupt("physical posting index disappeared"))?
            .1;
        if mask & (1_u64 << local) == 0 {
            return Err(corrupt("physical leaf lacks run posting member"));
        }
    }
    Ok(())
}

impl ActiveRunScrub {
    fn flush_gram(
        &mut self,
        store: &ObjectStore,
        authority: &AuthorityRoot,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let Some(key) = self.current_gram else {
            return Ok(());
        };
        let reference = store
            .map_get(&self.root.gram_counts, &key, budget)?
            .ok_or_else(|| corrupt("run pages have no gram count"))?;
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("run gram count kind differs"));
        }
        let record = RunGramCount::decode(&store.read_blob(
            &reference,
            size_u64(super::MAX_RECORD_BYTES)?,
            budget,
        )?)?;
        if record.key()? != key || record.count != self.current_gram_rows {
            return Err(corrupt("run page rows differ from gram count"));
        }
        let [surface, first, second, third] = key;
        let route = run_route_key(surface, [first, second, third], record.count, self.id)?;
        let routes = match surface {
            1 => &authority.path_routes,
            2 => &authority.content_routes,
            _ => return Err(corrupt("run gram surface differs")),
        };
        if store.map_get(routes, &route, budget)?.as_ref() != Some(&reference) {
            return Err(corrupt("run gram count has no exact global route"));
        }
        let seen_grams = self
            .seen_grams
            .checked_add(1)
            .ok_or_else(|| corrupt("run gram census overflows"))?;
        // Commit only after every authenticated read and route check succeeds.
        // An I/O or budget error must leave this gram available for retry.
        self.seen_grams = seen_grams;
        self.current_gram = None;
        self.current_gram_rows = 0;
        Ok(())
    }
}

impl RunPostingScrubCursor {
    /// One outer header, one posting page, or one count row per call. The
    /// page has at most 256 members, so physical leaf checks remain bounded.
    fn step(
        &mut self,
        store: &ObjectStore,
        authority: &AuthorityRoot,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<bool, CoreError> {
        if self.active.is_none() {
            let mut rows = store.map_cursor(
                &authority.run_postings,
                &scrub_lower(self.after_run.as_ref()),
                None,
                budget,
            )?;
            let Some((key, reference)) = rows.next()? else {
                if self.seen_runs != authority.run_postings.entries
                    || self.encoded_bytes != authority.run_index_encoded_bytes
                {
                    return Err(corrupt("run posting scrub census or byte ledger differs"));
                }
                return Ok(true);
            };
            let raw: [u8; 8] = key
                .as_slice()
                .try_into()
                .map_err(|_width| corrupt("run posting outer key width differs"))?;
            let id = u64::from_be_bytes(raw);
            let root = RunPostingRoot::load(store, &reference, budget)?;
            if root.run_id != id
                || authority
                    .run_registry
                    .candidate_by_id(store, id, budget)?
                    .is_none()
            {
                return Err(corrupt("run posting root has no live registry owner"));
            }
            let mut deps = root.object_roots();
            deps.sort();
            deps.dedup();
            if store.object_dependencies(&reference.root, budget)? != deps {
                return Err(corrupt("run posting root dependencies differ"));
            }
            self.active = Some(ActiveRunScrub {
                id,
                reference,
                root,
                phase: RunScrubPhase::Pages,
                after_key: None,
                current_gram: None,
                current_gram_rows: 0,
                page_rows: [0, 0],
                page_bytes: 0,
                count_bytes: 0,
                seen_pages: 0,
                seen_grams: 0,
                seen_counts: 0,
            });
            return Ok(false);
        }
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| corrupt("run posting scrub state disappeared"))?;
        let map = if active.phase == RunScrubPhase::Pages {
            &active.root.pages
        } else {
            &active.root.gram_counts
        };
        let mut rows =
            store.map_cursor(map, &scrub_lower(active.after_key.as_ref()), None, budget)?;
        if let Some((key, reference)) = rows.next()? {
            if active.phase == RunScrubPhase::Pages {
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("run posting page kind differs"));
                }
                let page = RunPostingPage::decode(&store.read_blob(
                    &reference,
                    size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                    budget,
                )?)?;
                if page.key()?.as_slice() != key
                    || !store
                        .object_dependencies(&reference.root, budget)?
                        .is_empty()
                {
                    return Err(corrupt("run posting page key or dependencies differ"));
                }
                let [first, second, third] = page.gram;
                let gram_key = [page.surface, first, second, third];
                if active.current_gram.is_some_and(|prior| prior != gram_key) {
                    active.flush_gram(store, authority, budget)?;
                }
                active.current_gram = Some(gram_key);
                scrub_run_page_member(store, authority, active.id, &page, budget)?;
                let row_count = size_u64(page.rows.len())?;
                active.current_gram_rows = active
                    .current_gram_rows
                    .checked_add(row_count)
                    .ok_or_else(|| corrupt("run gram row count overflows"))?;
                let index = match page.surface {
                    1 => 0,
                    2 => 1,
                    _ => return Err(corrupt("run surface differs")),
                };
                let slot = active
                    .page_rows
                    .get_mut(index)
                    .ok_or_else(|| corrupt("run surface slot absent"))?;
                *slot = slot
                    .checked_add(row_count)
                    .ok_or_else(|| corrupt("run surface row count overflows"))?;
                active.page_bytes = active
                    .page_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| corrupt("run page byte ledger overflows"))?;
                active.seen_pages = active
                    .seen_pages
                    .checked_add(1)
                    .ok_or_else(|| corrupt("run page census overflows"))?;
            } else {
                if key.len() != 4 || reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("run count key or kind differs"));
                }
                let count = RunGramCount::decode(&store.read_blob(
                    &reference,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    budget,
                )?)?;
                if count.key()?.as_slice() != key
                    || !store
                        .object_dependencies(&reference.root, budget)?
                        .is_empty()
                {
                    return Err(corrupt("run count key or dependencies differ"));
                }
                active.count_bytes = active
                    .count_bytes
                    .checked_add(reference.root.bytes)
                    .ok_or_else(|| corrupt("run count byte ledger overflows"))?;
                active.seen_counts = active
                    .seen_counts
                    .checked_add(1)
                    .ok_or_else(|| corrupt("run count census overflows"))?;
            }
            active.after_key = Some(key);
            return Ok(false);
        }
        if active.phase == RunScrubPhase::Pages {
            active.flush_gram(store, authority, budget)?;
            if active.seen_pages != active.root.pages.entries
                || active.page_rows != [active.root.path_rows, active.root.content_rows]
                || active.page_bytes != active.root.page_record_encoded_bytes
            {
                return Err(corrupt("run page census or byte ledger differs"));
            }
            active.phase = RunScrubPhase::Counts;
            active.after_key = None;
            return Ok(false);
        }
        if active.seen_counts != active.root.gram_counts.entries
            || active.seen_counts != active.seen_grams
            || active.count_bytes != active.root.gram_count_record_encoded_bytes
        {
            return Err(corrupt("run count census or byte ledger differs"));
        }
        self.encoded_bytes = self
            .encoded_bytes
            .checked_add(active.root.bundle_encoded_bytes(&active.reference, store)?)
            .ok_or_else(|| corrupt("run bundle aggregate overflows"))?;
        self.seen_runs = self
            .seen_runs
            .checked_add(1)
            .ok_or_else(|| corrupt("run root census overflows"))?;
        self.after_run = Some(active.id.to_be_bytes().to_vec());
        self.active = None;
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FileScrubPhase {
    #[default]
    Source,
    Current,
    Membership,
    Segment,
    PathRoute,
    ContentRoute,
    Debt,
    RunPostings,
    Registry,
    Done,
}

/// In-process progress across ordered map rows; no corpus-sized source census.
#[derive(Default)]
pub(crate) struct FileScrubCursor {
    root_sha256: Option<[u8; 32]>,
    phase: FileScrubPhase,
    after_key: Option<Vec<u8>>,
    sources: u64,
    source_bytes: u64,
    memberships: u64,
    current_rows: u64,
    membership_rows: u64,
    active_rows: u64,
    posting_cursor: RunPostingScrubCursor,
    run_cursor: Option<RunScrubCursor>,
}

pub(crate) struct FileScrubStep {
    pub(crate) done: bool,
    pub(crate) bytes_read: u64,
    pub(crate) read_count: u64,
}

/// Verify one map row, or advance across empty phases. The caller passes a
/// metered store clone if physical authenticated read accounting is required.
pub(crate) fn verify_scrub_step(
    store: &ObjectStore,
    root: &AuthorityRoot,
    cursor: &mut FileScrubCursor,
    budget: Option<&RequestBudgetV1>,
) -> Result<FileScrubStep, CoreError> {
    let root_bytes = root.encode()?;
    let root_sha256: [u8; 32] = Sha256::digest(&root_bytes).into();
    if cursor.root_sha256.is_some_and(|prior| prior != root_sha256) {
        return Err(corrupt("scrub cursor belongs to another file root"));
    }
    cursor.root_sha256 = Some(root_sha256);
    let before = store.read_meter_snapshot();
    loop {
        checkpoint(budget, "lexical:file-scrub-step")?;
        if cursor.phase == FileScrubPhase::RunPostings {
            if cursor.posting_cursor.step(store, root, budget)? {
                cursor.phase = FileScrubPhase::Registry;
            }
            let after = store.read_meter_snapshot();
            return Ok(FileScrubStep {
                done: false,
                bytes_read: after
                    .0
                    .checked_sub(before.0)
                    .ok_or_else(|| corrupt("scrub byte meter regressed"))?,
                read_count: after
                    .1
                    .checked_sub(before.1)
                    .ok_or_else(|| corrupt("scrub read meter regressed"))?,
            });
        }
        if cursor.phase == FileScrubPhase::Registry {
            if cursor.run_cursor.is_none() {
                cursor.run_cursor = Some(RunScrubCursor::new(&root.run_registry)?);
            }
            let registry_done = cursor
                .run_cursor
                .as_mut()
                .ok_or_else(|| corrupt("registry scrub cursor is absent"))?
                .step(store, budget)?;
            if registry_done {
                cursor.phase = FileScrubPhase::Done;
            }
            let after = store.read_meter_snapshot();
            return Ok(FileScrubStep {
                done: false,
                bytes_read: after
                    .0
                    .checked_sub(before.0)
                    .ok_or_else(|| corrupt("scrub byte meter regressed"))?,
                read_count: after
                    .1
                    .checked_sub(before.1)
                    .ok_or_else(|| corrupt("scrub read meter regressed"))?,
            });
        }
        let map = match cursor.phase {
            FileScrubPhase::Source => &root.source_heads,
            FileScrubPhase::Current => &root.current_docs,
            FileScrubPhase::Membership => &root.memberships,
            FileScrubPhase::Segment => &root.segments,
            FileScrubPhase::PathRoute => &root.path_routes,
            FileScrubPhase::ContentRoute => &root.content_routes,
            FileScrubPhase::Debt => &root.compaction_debt,
            FileScrubPhase::RunPostings => {
                return Err(corrupt("run posting phase bypassed bounded scrub cursor"));
            }
            FileScrubPhase::Registry => {
                return Err(corrupt("registry phase bypassed bounded scrub cursor"));
            }
            FileScrubPhase::Done => {
                let after = store.read_meter_snapshot();
                return Ok(FileScrubStep {
                    done: true,
                    bytes_read: after
                        .0
                        .checked_sub(before.0)
                        .ok_or_else(|| corrupt("scrub byte meter regressed"))?,
                    read_count: after
                        .1
                        .checked_sub(before.1)
                        .ok_or_else(|| corrupt("scrub read meter regressed"))?,
                });
            }
        };
        // Keys are fixed-width within each map. Appending zero is the lowest
        // possible lexicographic key strictly greater than the last row.
        let lower = cursor.after_key.as_ref().map_or_else(Vec::new, |key| {
            let mut lower = key.clone();
            lower.push(0);
            lower
        });
        let mut rows = store.map_cursor(map, &lower, None, budget)?;
        if let Some((key, reference)) = rows.next()? {
            match cursor.phase {
                FileScrubPhase::Source => {
                    let digest: [u8; 32] = key
                        .as_slice()
                        .try_into()
                        .map_err(|_conversion_error| corrupt("scrub source key is not a digest"))?;
                    let (committed, head) = source_head(store, root, digest, budget)?;
                    if committed != reference
                        || live_doc(store, root, head.logical_id, budget)?.source_key_sha256
                            != digest
                        || store
                            .map_get(
                                &root.memberships,
                                &membership_key(digest, head.logical_id),
                                budget,
                            )?
                            .as_ref()
                            != Some(&reference)
                    {
                        return Err(corrupt("scrub source/current/membership relation differs"));
                    }
                    cursor.sources = cursor
                        .sources
                        .checked_add(1)
                        .ok_or_else(|| corrupt("scrub source count overflow"))?;
                    cursor.source_bytes = cursor
                        .source_bytes
                        .checked_add(head.source_bytes)
                        .ok_or_else(|| corrupt("scrub source byte count overflow"))?;
                    cursor.memberships =
                        cursor
                            .memberships
                            .checked_add(head.posting_memberships)
                            .ok_or_else(|| corrupt("scrub membership count overflow"))?;
                }
                FileScrubPhase::Current => {
                    let raw: [u8; 8] = key
                        .as_slice()
                        .try_into()
                        .map_err(|_conversion_error| corrupt("scrub current key is not a u64"))?;
                    let id = u64::from_be_bytes(raw);
                    if reference.root.kind != ObjectKind::Record {
                        return Err(corrupt("scrub current locator kind differs"));
                    }
                    let locator = live_doc(store, root, id, budget)?;
                    let (_, head) = source_head(store, root, locator.source_key_sha256, budget)?;
                    let segment_ref = store
                        .map_get(&root.segments, &locator.segment_sha256, budget)?
                        .ok_or_else(|| corrupt("scrub current segment is absent"))?;
                    let segment = Segment::decode(&store.read_blob(
                        &segment_ref,
                        size_u64(super::MAX_RECORD_BYTES)?,
                        budget,
                    )?)?;
                    if head.logical_id != id
                        || segment.doc_ids.get(index_usize(locator.local_id)?) != Some(&id)
                    {
                        return Err(corrupt(
                            "scrub current locator differs from source or segment",
                        ));
                    }
                    cursor.current_rows = cursor
                        .current_rows
                        .checked_add(1)
                        .ok_or_else(|| corrupt("scrub current count overflow"))?;
                }
                FileScrubPhase::Membership => {
                    if key.len() != 40 {
                        return Err(corrupt("scrub membership key width differs"));
                    }
                    let digest: [u8; 32] = key
                        .get(..32)
                        .ok_or_else(|| corrupt("scrub membership digest absent"))?
                        .try_into()
                        .map_err(|_conversion_error| corrupt("scrub membership digest width"))?;
                    let raw: [u8; 8] = key
                        .get(32..)
                        .ok_or_else(|| corrupt("scrub membership ID absent"))?
                        .try_into()
                        .map_err(|_conversion_error| corrupt("scrub membership ID width"))?;
                    let (head_ref, head) = source_head(store, root, digest, budget)?;
                    if head.logical_id != u64::from_be_bytes(raw) || head_ref != reference {
                        return Err(corrupt("scrub membership has no matching source"));
                    }
                    cursor.membership_rows = cursor
                        .membership_rows
                        .checked_add(1)
                        .ok_or_else(|| corrupt("scrub membership row overflow"))?;
                }
                FileScrubPhase::Segment => {
                    cursor.active_rows = cursor
                        .active_rows
                        .checked_add(verify_scrub_segment_row(
                            store, root, &key, &reference, budget,
                        )?)
                        .ok_or_else(|| corrupt("scrub active count overflow"))?;
                }
                FileScrubPhase::PathRoute | FileScrubPhase::ContentRoute => {
                    let surface = if cursor.phase == FileScrubPhase::PathRoute {
                        1
                    } else {
                        2
                    };
                    let (gram, count, run_id) = route_parts(&key, surface)?;
                    let run_ref = store
                        .map_get(&root.run_postings, &run_id.to_be_bytes(), budget)?
                        .ok_or_else(|| corrupt("scrub route names absent run"))?;
                    let run = RunPostingRoot::load(store, &run_ref, budget)?;
                    if run.run_id != run_id {
                        return Err(corrupt("scrub route run ID differs"));
                    }
                    let gram_key = [surface, gram[0], gram[1], gram[2]];
                    let expected_ref = store
                        .map_get(&run.gram_counts, &gram_key, budget)?
                        .ok_or_else(|| corrupt("scrub route gram count is absent"))?;
                    if expected_ref != reference || reference.root.kind != ObjectKind::Record {
                        return Err(corrupt("scrub route value differs from run count"));
                    }
                    let record = RunGramCount::decode(&store.read_blob(
                        &reference,
                        size_u64(super::MAX_RECORD_BYTES)?,
                        budget,
                    )?)?;
                    if record.key()? != gram_key || record.count != count {
                        return Err(corrupt("scrub route count differs"));
                    }
                }
                FileScrubPhase::Debt => {
                    let digest: [u8; 32] = key
                        .as_slice()
                        .try_into()
                        .map_err(|_conversion_error| corrupt("scrub debt key width"))?;
                    if store.map_get(&root.segments, &digest, budget)?.as_ref() != Some(&reference)
                    {
                        return Err(corrupt("scrub debt references unknown segment"));
                    }
                }
                FileScrubPhase::RunPostings => {
                    return Err(corrupt("run posting phase selected a file map"));
                }
                FileScrubPhase::Registry => {
                    return Err(corrupt("registry phase selected a file map"));
                }
                FileScrubPhase::Done => {
                    return Err(corrupt("scrub phase changed after map selection"));
                }
            }
            cursor.after_key = Some(key);
            let after = store.read_meter_snapshot();
            return Ok(FileScrubStep {
                done: false,
                bytes_read: after
                    .0
                    .checked_sub(before.0)
                    .ok_or_else(|| corrupt("scrub byte meter regressed"))?,
                read_count: after
                    .1
                    .checked_sub(before.1)
                    .ok_or_else(|| corrupt("scrub read meter regressed"))?,
            });
        }
        match cursor.phase {
            FileScrubPhase::Source => {
                let source_count_differs = cursor.sources != root.live_sources;
                let source_bytes_differ = cursor.source_bytes != root.source_bytes;
                let posting_memberships_differ = cursor.memberships != root.membership_count;
                if source_count_differs || source_bytes_differ || posting_memberships_differ {
                    return Err(corrupt("scrub source summary differs"));
                }
                cursor.phase = FileScrubPhase::Current;
            }
            FileScrubPhase::Current => {
                if cursor.current_rows != cursor.sources {
                    return Err(corrupt("scrub current census differs"));
                }
                cursor.phase = FileScrubPhase::Membership;
            }
            FileScrubPhase::Membership => {
                if cursor.membership_rows != cursor.sources {
                    return Err(corrupt("scrub membership census differs"));
                }
                cursor.phase = FileScrubPhase::Segment;
            }
            FileScrubPhase::Segment => {
                if cursor.active_rows != cursor.sources {
                    return Err(corrupt("scrub segment live-document census differs"));
                }
                cursor.phase = FileScrubPhase::PathRoute;
            }
            FileScrubPhase::PathRoute => cursor.phase = FileScrubPhase::ContentRoute,
            FileScrubPhase::ContentRoute => cursor.phase = FileScrubPhase::Debt,
            FileScrubPhase::Debt => cursor.phase = FileScrubPhase::RunPostings,
            FileScrubPhase::RunPostings => {
                return Err(corrupt("run posting phase advanced outside its cursor"));
            }
            FileScrubPhase::Registry => {
                return Err(corrupt("registry phase advanced outside its cursor"));
            }
            FileScrubPhase::Done => return Err(corrupt("scrub terminal phase advanced")),
        }
        cursor.after_key = None;
    }
}

/// Full semantic oracle, using the same bounded-row path as resumable scrub.
/// Retired local IDs are allowed only when the current locator changed.
pub(crate) fn verify_all_for_scrub(
    store: &ObjectStore,
    root: &AuthorityRoot,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    let mut cursor = FileScrubCursor::default();
    loop {
        if verify_scrub_step(store, root, &mut cursor, budget)?.done {
            return Ok(());
        }
    }
}

/// Independently select old run cohorts for a clean physical merge.
///
/// The verifier reads immutable segment descriptors and live locators; it
/// never calls the posting producer's selection implementation.
struct CleanRunSelection {
    run_ids: Vec<u64>,
    progress: RunSelectionProgress,
}

/// Independent read-only File witness over one frozen old-run window. This
/// never calls producer eligibility or accepts its touched segment set.
pub(crate) fn replay_hard_scan_window(
    store: &ObjectStore,
    base: &AuthorityRoot,
    mutation: &FileAuthorityMutation,
    policy: &LexicalMaintenancePolicy,
    progress: RunSelectionProgress,
    budget: Option<&RequestBudgetV1>,
) -> Result<WindowWitness, CoreError> {
    if mutation.clear_all || progress.pending_run.is_some() {
        return Err(corrupt("File hard scan requires a non-clear frozen base"));
    }
    let high = base
        .run_registry
        .next_run_id
        .checked_sub(1)
        .ok_or_else(|| corrupt("File hard scan frontier underflows"))?;
    if high == 0
        || (progress.scan_high_water_run_id != 0 && progress.scan_high_water_run_id != high)
    {
        return Err(corrupt("File hard scan frontier differs"));
    }
    let mut touched = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for edit in &mutation.edits {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:file:verify-hard-source")?;
        }
        let key = edit.key();
        let digest = source_key_digest(key)?;
        if !seen.insert(digest) {
            return Err(corrupt("File hard scan source digest repeats"));
        }
        let Some((_, previous)) = optional_source_head(store, base, key)? else {
            continue;
        };
        let locator = live_doc(store, base, previous.logical_id, budget)?;
        if locator.source_key_sha256 != digest || locator.logical_id != previous.logical_id {
            return Err(corrupt("File hard scan prior source locator differs"));
        }
        let segment = store
            .map_get(&base.segments, &locator.segment_sha256, budget)?
            .ok_or_else(|| corrupt("File hard scan prior segment absent"))?;
        if segment.root.kind != ObjectKind::Segment || segment.root.sha256 != locator.segment_sha256
        {
            return Err(corrupt("File hard scan prior segment differs"));
        }
        let changed = match edit {
            FileSourceEdit::Delete { .. } => true,
            FileSourceEdit::Upsert { coverage, bytes } => {
                let encoded = crate::channel_payloads::encode_cbor(
                    &(super::policy_digest(), coverage),
                    "file hard scan derived input",
                )?;
                let mut hash = Sha256::new();
                hash.update(b"quanta-file-derived-input-v17\0");
                hash.update(encoded);
                previous.derived_input_sha256 != <[u8; 32]>::from(hash.finalize())
                    || previous.coverage != *coverage
                    || previous.body.sha256 != <[u8; 32]>::from(Sha256::digest(bytes))
            }
        };
        if changed {
            let _inserted = touched.insert(locator.segment_sha256);
        }
    }
    let window = base
        .run_registry
        .clone()
        .with_selection_progress(progress)?
        .scan_hard_window(store, 32, budget)?;
    if window.scan_high_water_run_id != high {
        return Err(corrupt("File hard scan window frontier differs"));
    }
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(window.candidates.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "reserve independent File hard scan window: {error}"
            ))
        })?;
    for candidate in &window.candidates {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:file:verify-hard-candidate")?;
        }
        let key = candidate.run_id.to_be_bytes();
        let header = store
            .map_get(&base.run_registry.runs, &key, budget)?
            .ok_or_else(|| corrupt("File hard scan run header absent"))?;
        if header.root.kind != ObjectKind::Record {
            return Err(corrupt("File hard scan run header kind differs"));
        }
        let mut header_hash = Sha256::new();
        header_hash.update(b"lexical-file-hard-scan-header-v1\0");
        header_hash.update(key);
        header_hash.update(header.root.sha256);
        let eligibility = if candidate.index_encoded_bytes > policy.max_merge_input_encoded_bytes()
        {
            EligibilityWitness::Ineligible(IneligibleReason::InputCap)
        } else {
            let reference = store
                .map_get(&base.run_postings, &key, budget)?
                .ok_or_else(|| corrupt("File hard scan posting root absent"))?;
            let posting = RunPostingRoot::load(store, &reference, budget)?;
            if posting.run_id != candidate.run_id {
                return Err(corrupt("File hard scan posting ID differs"));
            }
            let bytes = candidate
                .index_encoded_bytes
                .checked_add(posting.bundle_encoded_bytes(&reference, store)?)
                .ok_or_else(|| corrupt("File hard scan input bytes overflow"))?;
            if bytes > policy.max_merge_input_encoded_bytes() {
                EligibilityWitness::Ineligible(IneligibleReason::InputCap)
            } else {
                let mut eligible = true;
                base.run_registry.visit_run_members(
                    store,
                    candidate,
                    budget,
                    &mut |member_key, member_ref, _bytes| {
                        let digest: [u8; 32] = member_key
                            .try_into()
                            .map_err(|_width| corrupt("File hard scan member key width differs"))?;
                        if touched.contains(&digest)
                            || store
                                .map_get(&base.compaction_debt, member_key, budget)?
                                .is_some()
                        {
                            eligible = false;
                        }
                        if store.map_get(&base.segments, member_key, budget)?.as_ref()
                            != Some(member_ref)
                        {
                            return Err(corrupt("File hard scan member differs from segment map"));
                        }
                        Ok(())
                    },
                )?;
                if eligible {
                    EligibilityWitness::FileEligible {
                        effective_input_encoded_bytes: bytes,
                    }
                } else {
                    EligibilityWitness::Ineligible(IneligibleReason::TouchedOrDebt)
                }
            }
        };
        candidates.push(CandidateWitness {
            run_id: candidate.run_id,
            index_encoded_bytes: candidate.index_encoded_bytes,
            eligibility,
            header_sha256: header_hash.finalize().into(),
        });
    }
    Ok(WindowWitness {
        ordinal: 0,
        prior_chain_sha256: [0; 32],
        start_run_id: progress.next_scan_run_id,
        next_run_id: window.next_scan_run_id,
        frozen_high_water_run_id: high,
        round_complete: window.round_complete,
        candidates,
    })
}

fn verify_run_posting_transition(
    store: &ObjectStore,
    old: &AuthorityRoot,
    target: &AuthorityRoot,
    premerge: &RunRegistry,
    removed: &[SegmentRemoval],
    added: &[SegmentInput],
    selected: &[u64],
) -> Result<FileRunMergeFacts, CoreError> {
    let mut changed = BTreeMap::<u64, (Vec<SegmentRemoval>, Vec<SegmentInput>)>::new();
    for row in removed {
        let digest: [u8; 32] = row
            .key
            .as_slice()
            .try_into()
            .map_err(|_width| corrupt("removed run leaf digest width differs"))?;
        let id = old.run_registry.owner_run_id(store, digest, None)?;
        changed.entry(id).or_default().0.push(row.clone());
    }
    for row in added {
        let digest: [u8; 32] = row
            .key
            .as_slice()
            .try_into()
            .map_err(|_width| corrupt("added run leaf digest width differs"))?;
        let id = premerge.owner_run_id(store, digest, None)?;
        changed.entry(id).or_default().1.push(row.clone());
    }
    let mut allowed = BTreeSet::new();
    for (id, (removed, added)) in &changed {
        if selected.binary_search(id).is_ok() {
            return Err(corrupt("changed physical run selected for clean merge"));
        }
        let key = id.to_be_bytes();
        let old_ref = store.map_get(&old.run_postings, &key, None)?;
        let target_ref = store.map_get(&target.run_postings, &key, None)?;
        let old_run = old_ref
            .as_ref()
            .map(|reference| RunPostingRoot::load(store, reference, None))
            .transpose()?;
        let target_run = target_ref
            .as_ref()
            .map(|reference| RunPostingRoot::load(store, reference, None))
            .transpose()?;
        verify_changed_run(
            store,
            *id,
            old_run.as_ref().zip(old_ref.as_ref()),
            target_run.as_ref().zip(target_ref.as_ref()),
            removed,
            added,
            None,
        )?;
        if !allowed.insert(*id) {
            return Err(corrupt("changed run ID appears twice"));
        }
    }
    if !selected.is_empty() {
        if selected.len() < 2
            || selected.windows(2).any(|pair| {
                let [left, right] = pair else { return true };
                left >= right
            })
        {
            return Err(corrupt("selected clean run IDs are not a distinct cohort"));
        }
        let mut old_runs = Vec::new();
        for id in selected {
            let reference = store
                .map_get(&old.run_postings, &id.to_be_bytes(), None)?
                .ok_or_else(|| corrupt("selected clean run posting root absent"))?;
            let run = RunPostingRoot::load(store, &reference, None)?;
            old_runs.push((run, reference));
            if !allowed.insert(*id) {
                return Err(corrupt("clean run overlaps changed run"));
            }
        }
        let new_id = premerge.next_run_id;
        let new_ref = store
            .map_get(&target.run_postings, &new_id.to_be_bytes(), None)?
            .ok_or_else(|| corrupt("clean merged run posting root absent"))?;
        let new_run = RunPostingRoot::load(store, &new_ref, None)?;
        verify_clean_run_union(store, &old_runs, (&new_run, &new_ref), None)?;
        if !allowed.insert(new_id) {
            return Err(corrupt("clean output run ID overlaps an input"));
        }
    }
    let mut expected_bytes = old.run_index_encoded_bytes;
    let mut observed = BTreeSet::new();
    let mut retired_selected_posting_bytes = 0_u64;
    let mut created_selected_posting_bytes = 0_u64;
    store.map_changed_keys(
        &old.run_postings,
        &target.run_postings,
        None,
        &mut |key, before, after| {
            let raw: [u8; 8] = key
                .try_into()
                .map_err(|_width| corrupt("changed run posting key width differs"))?;
            let id = u64::from_be_bytes(raw);
            if !allowed.contains(&id) || !observed.insert(id) {
                return Err(corrupt("unrelated run posting root changed"));
            }
            if let Some(reference) = before {
                let run = RunPostingRoot::load(store, reference, None)?;
                if run.run_id != id {
                    return Err(corrupt("old run posting ID differs"));
                }
                let bytes = run.bundle_encoded_bytes(reference, store)?;
                if selected.binary_search(&id).is_ok() {
                    retired_selected_posting_bytes = retired_selected_posting_bytes
                        .checked_add(bytes)
                        .ok_or_else(|| corrupt("selected input posting bytes overflow"))?;
                }
                expected_bytes = expected_bytes
                    .checked_sub(bytes)
                    .ok_or_else(|| corrupt("run index byte ledger underflows"))?;
            }
            if let Some(reference) = after {
                let run = RunPostingRoot::load(store, reference, None)?;
                if run.run_id != id {
                    return Err(corrupt("target run posting ID differs"));
                }
                let bytes = run.bundle_encoded_bytes(reference, store)?;
                if !selected.is_empty() && id == premerge.next_run_id {
                    created_selected_posting_bytes = created_selected_posting_bytes
                        .checked_add(bytes)
                        .ok_or_else(|| corrupt("selected output posting bytes overflow"))?;
                }
                expected_bytes = expected_bytes
                    .checked_add(bytes)
                    .ok_or_else(|| corrupt("run index byte ledger overflows"))?;
            }
            Ok(())
        },
    )?;
    if selected.iter().any(|id| !observed.contains(id))
        || (!selected.is_empty() && !observed.contains(&premerge.next_run_id))
        || expected_bytes != target.run_index_encoded_bytes
    {
        return Err(corrupt("run posting edits or aggregate byte ledger differ"));
    }
    verify_global_run_routes(
        store,
        old,
        target,
        &allowed.into_iter().collect::<Vec<_>>(),
        None,
    )?;
    if selected.is_empty() {
        return Ok(FileRunMergeFacts::default());
    }
    // The already-proved cohort transfer retains every segment reference and
    // byte charge. Only run-posting bundles change logical ownership.
    let new_run = target
        .run_registry
        .candidate_by_id(store, premerge.next_run_id, None)?
        .ok_or_else(|| corrupt("selected merge output has no run header"))?;
    FileRunMergeFacts::merged(
        size_u64(selected.len())?,
        new_run.members.entries,
        new_run.index_encoded_bytes,
        retired_selected_posting_bytes,
        created_selected_posting_bytes,
    )
}

fn selected_clean_merge_runs(
    store: &ObjectStore,
    old: &AuthorityRoot,
    touched: &BTreeSet<[u8; 32]>,
    policy: &LexicalMaintenancePolicy,
    mutation: &FileAuthorityMutation,
    hard_scan: Option<&ScanLane>,
) -> Result<CleanRunSelection, CoreError> {
    let prior = old.run_registry.selection_progress();
    let before = old.maintenance_snapshot(store)?;
    if !policy.merge_due(before) || run_merge_stage_bytes(2)? > policy.max_staging_bytes() {
        return Ok(CleanRunSelection {
            run_ids: Vec::new(),
            progress: prior,
        });
    }
    let mut selected = Vec::new();
    let mut input_bytes = 0_u64;
    let max_fan_in = usize::try_from(policy.max_merge_fan_in())
        .map_err(|_width| corrupt("merge fan-in width differs"))?;
    let window = if policy.hard_pressure(before) {
        let lane = hard_scan.ok_or_else(|| corrupt("File hard recovery checkpoint is absent"))?;
        let root_sha256: [u8; 32] = Sha256::digest(old.encode()?).into();
        let high = old
            .run_registry
            .next_run_id
            .checked_sub(1)
            .ok_or_else(|| corrupt("File hard recovery frontier underflows"))?;
        if lane.authority != ScanAuthority::File
            || lane.base_authority_root_sha256 != root_sha256
            || lane.frozen_high_water_run_id != high
            || lane.windows.is_empty()
            || lane.windows.len() > 129
            || !lane
                .windows
                .last()
                .is_some_and(|window| window.round_complete)
        {
            return Err(corrupt(
                "File hard recovery checkpoint base or completion differs",
            ));
        }
        let mut earlier = Vec::<(u64, u64)>::new();
        let mut hard_selected = Vec::new();
        let mut hard_bytes = 0_u64;
        let mut next = 1_u64;
        let mut seen = 0_u64;
        for (ordinal, claimed) in lane.windows.iter().enumerate() {
            if usize::from(claimed.ordinal) != ordinal
                || claimed.start_run_id != next
                || claimed.frozen_high_water_run_id != high
                || (claimed.round_complete && ordinal.checked_add(1) != Some(lane.windows.len()))
            {
                return Err(corrupt("File hard recovery checkpoint cursor differs"));
            }
            let progress = RunSelectionProgress {
                next_scan_run_id: next,
                scan_high_water_run_id: high,
                pending_run: None,
            };
            let actual = replay_hard_scan_window(store, old, mutation, policy, progress, None)?;
            if actual.start_run_id != claimed.start_run_id
                || actual.next_run_id != claimed.next_run_id
                || actual.frozen_high_water_run_id != claimed.frozen_high_water_run_id
                || actual.round_complete != claimed.round_complete
                || actual.candidates != claimed.candidates
            {
                return Err(corrupt(
                    "File hard recovery checkpoint window differs from base",
                ));
            }
            seen = seen
                .checked_add(
                    u64::try_from(actual.candidates.len())
                        .map_err(|_width| corrupt("File hard recovery window count differs"))?,
                )
                .ok_or_else(|| corrupt("File hard recovery run count overflows"))?;
            if seen > quanta_index_core::lexical_maintenance::MAX_HARD_SCAN_RUNS {
                return Err(corrupt("File hard recovery run bound differs"));
            }
            for candidate in &actual.candidates {
                let bytes = match candidate.eligibility {
                    EligibilityWitness::FileEligible {
                        effective_input_encoded_bytes,
                    } => effective_input_encoded_bytes,
                    EligibilityWitness::Ineligible(_) => continue,
                    EligibilityWitness::NativeEligible { .. } => {
                        return Err(corrupt("Native eligibility in File checkpoint"));
                    }
                };
                if bytes < candidate.index_encoded_bytes
                    || bytes > policy.max_merge_input_encoded_bytes()
                {
                    return Err(corrupt("File hard recovery candidate cost differs"));
                }
                if hard_selected.len() >= max_fan_in {
                    continue;
                }
                if !hard_selected.is_empty() {
                    if let Some(total) = hard_bytes.checked_add(bytes)
                        && total <= policy.max_merge_input_encoded_bytes()
                        && run_merge_stage_bytes(
                            hard_selected
                                .len()
                                .checked_add(1)
                                .ok_or_else(|| corrupt("File hard recovery fan-in overflows"))?,
                        )? <= policy.max_staging_bytes()
                    {
                        hard_bytes = total;
                        hard_selected.push(candidate.run_id);
                    }
                    continue;
                }
                for &(run_id, earlier_bytes) in &earlier {
                    if let Some(total) = earlier_bytes.checked_add(bytes)
                        && total <= policy.max_merge_input_encoded_bytes()
                    {
                        hard_bytes = total;
                        hard_selected.extend([run_id, candidate.run_id]);
                        break;
                    }
                }
                if hard_selected.is_empty() {
                    earlier.push((candidate.run_id, bytes));
                }
            }
            next = claimed.next_run_id;
        }
        selected = hard_selected;
        let last = lane
            .windows
            .last()
            .ok_or_else(|| corrupt("File hard recovery checkpoint ended without window"))?;
        RunScanWindow {
            candidates: Vec::new(),
            next_scan_run_id: last.next_run_id,
            scan_high_water_run_id: high,
            round_complete: last.round_complete,
        }
    } else {
        let mut consider = |candidate: &RunCandidate| -> Result<bool, CoreError> {
            if selected.contains(&candidate.run_id) || selected.len() >= max_fan_in {
                return Ok(selected.len() >= max_fan_in);
            }
            if candidate.index_encoded_bytes > policy.max_merge_input_encoded_bytes() {
                return Ok(false);
            }
            let key = candidate.run_id.to_be_bytes();
            let reference = store
                .map_get(&old.run_postings, &key, None)?
                .ok_or_else(|| corrupt("eligible run has no posting root"))?;
            let posting = RunPostingRoot::load(store, &reference, None)?;
            if posting.run_id != candidate.run_id {
                return Err(corrupt("candidate posting root ID differs"));
            }
            let bytes = candidate
                .index_encoded_bytes
                .checked_add(posting.bundle_encoded_bytes(&reference, store)?)
                .ok_or_else(|| corrupt("merge input bytes overflow"))?;
            if bytes > policy.max_merge_input_encoded_bytes() {
                return Ok(false);
            }
            let mut eligible = true;
            old.run_registry.visit_run_members(
                store,
                candidate,
                None,
                &mut |key, reference, _| {
                    let digest: [u8; 32] = key
                        .try_into()
                        .map_err(|_width| corrupt("clean cohort digest width differs"))?;
                    if touched.contains(&digest)
                        || store.map_get(&old.compaction_debt, key, None)?.is_some()
                    {
                        eligible = false;
                    }
                    if store.map_get(&old.segments, key, None)?.as_ref() != Some(reference) {
                        return Err(corrupt("clean cohort member differs from segment map"));
                    }
                    Ok(())
                },
            )?;
            if !eligible {
                return Ok(false);
            }
            let Some(next_bytes) = input_bytes.checked_add(bytes) else {
                return Ok(false);
            };
            if next_bytes > policy.max_merge_input_encoded_bytes()
                || run_merge_stage_bytes(
                    selected
                        .len()
                        .checked_add(1)
                        .ok_or_else(|| corrupt("selected run fan-in overflows"))?,
                )? > policy.max_staging_bytes()
            {
                return Ok(false);
            }
            input_bytes = next_bytes;
            selected.push(candidate.run_id);
            Ok(selected.len() >= max_fan_in)
        };
        if let Some(anchor) = prior.pending_run
            && let Some(candidate) = old.run_registry.candidate_by_id(store, anchor, None)?
        {
            let _full = consider(&candidate)?;
        }
        let window = old.run_registry.scan_window(
            store,
            bounded_candidate_scan_limit(policy.max_merge_fan_in()),
            None,
        )?;
        for candidate in &window.candidates {
            let _full = consider(candidate)?;
        }
        window
    };
    let mut progress = RunSelectionProgress {
        next_scan_run_id: window.next_scan_run_id,
        scan_high_water_run_id: window.scan_high_water_run_id,
        pending_run: prior.pending_run,
    };
    if selected.len() >= 2 {
        selected.sort_unstable();
        progress.pending_run = None;
        return Ok(CleanRunSelection {
            run_ids: selected,
            progress,
        });
    }
    let anchor = selected.first().copied();
    progress.pending_run = if window.round_complete && prior.pending_run == anchor {
        None
    } else {
        anchor
    };
    Ok(CleanRunSelection {
        run_ids: Vec::new(),
        progress,
    })
}

#[cfg(test)]
pub(crate) fn verify_for_seal(
    store: &ObjectStore,
    root: &AuthorityRoot,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
) -> Result<(), CoreError> {
    verify_for_seal_with_policy(store, root, base, reservation, mutation, None)
}

#[cfg(test)]
pub(crate) fn verify_for_seal_with_policy(
    store: &ObjectStore,
    root: &AuthorityRoot,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
    maintenance_policy: Option<&LexicalMaintenancePolicy>,
) -> Result<(), CoreError> {
    let _facts = verify_for_seal_with_merge_facts(
        store,
        root,
        base,
        reservation,
        mutation,
        maintenance_policy,
        None,
    )?;
    Ok(())
}

/// Return merge-only attribution from the independent seal's existing
/// run-posting byte calculation. No producer receipt or extra posting walk.
pub(crate) fn verify_for_seal_with_merge_facts(
    store: &ObjectStore,
    root: &AuthorityRoot,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
    maintenance_policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<FileRunMergeFacts, CoreError> {
    reservation.validate()?;
    // Reauthenticate in this independent seal pass, then reuse at most the
    // store's fixed map-page cache. Never inherit the producer's read cache.
    let verification_store = store.metered_clone();
    let store = &verification_store;
    let _validated_root = root.encode()?;
    if root.allocation_scope_sha256 != reservation.scope_sha256
        || mutation.edits.windows(2).any(|pair| {
            let [left, right] = pair else { return true };
            left.key() >= right.key()
        })
    {
        return Err(corrupt("reservation scope or mutation sort order differs"));
    }
    if let Some(base) = base
        && base.allocation_scope_sha256 != reservation.scope_sha256
    {
        return Err(corrupt("sealed base allocation scope differs"));
    }
    let empty = AuthorityRoot {
        policy_sha256: super::policy_digest(),
        allocation_scope_sha256: reservation.scope_sha256,
        source_heads: MapRoot::default(),
        current_docs: MapRoot::default(),
        memberships: MapRoot::default(),
        segments: MapRoot::default(),
        run_registry: RunRegistry::empty(),
        premerge_run_registry: None,
        run_postings: MapRoot::default(),
        run_index_encoded_bytes: 0,
        compaction_debt: MapRoot::default(),
        path_routes: MapRoot::default(),
        content_routes: MapRoot::default(),
        live_sources: 0,
        live_docs: 0,
        source_bytes: 0,
        membership_count: 0,
    };
    let old = if mutation.clear_all {
        &empty
    } else {
        base.unwrap_or(&empty)
    };
    let mut allowed_sources = BTreeSet::new();
    let mut edited_ids = BTreeSet::new();
    let mut allowed_memberships = BTreeSet::new();
    let mut retired_segments = BTreeSet::new();
    let mut created_segments = BTreeSet::new();
    let mut source_count = old.live_sources;
    let mut source_bytes = old.source_bytes;
    let mut memberships = old.membership_count;
    let range_last = if reservation.count == 0 {
        0
    } else {
        reservation
            .first
            .checked_add(
                reservation
                    .count
                    .checked_sub(1)
                    .ok_or_else(|| corrupt("nonempty reservation count underflow"))?,
            )
            .ok_or_else(|| corrupt("reservation range overflow"))?
    };
    for edit in &mutation.edits {
        let key = edit.key();
        let digest = source_key_digest(key)?;
        if !allowed_sources.insert(digest) {
            return Err(corrupt("changed source digest repeats or collides"));
        }
        let prior = optional_source_head(store, old, key)?;
        let prior_segment = if let Some((_, old_head)) = &prior {
            source_count = source_count
                .checked_sub(1)
                .ok_or_else(|| corrupt("source count underflow"))?;
            source_bytes = source_bytes
                .checked_sub(old_head.source_bytes)
                .ok_or_else(|| corrupt("source byte count underflow"))?;
            memberships = memberships
                .checked_sub(old_head.posting_memberships)
                .ok_or_else(|| corrupt("membership count underflow"))?;
            let _inserted = edited_ids.insert(old_head.logical_id);
            let _inserted =
                allowed_memberships.insert(membership_key(digest, old_head.logical_id).to_vec());
            let locator = live_doc(store, old, old_head.logical_id, None)?;
            if locator.source_key_sha256 != digest {
                return Err(corrupt("base source and current locator differ"));
            }
            Some(locator.segment_sha256)
        } else {
            None
        };
        match edit {
            FileSourceEdit::Delete { .. } => {
                if let Some(segment) = prior_segment {
                    let _inserted = retired_segments.insert(segment);
                }
                verify_source_absent(store, root, key)?;
                if let Some((_, previous)) = &prior
                    && (store
                        .map_get(&root.current_docs, &doc_key(previous.logical_id), None)?
                        .is_some()
                        || store
                            .map_get(
                                &root.memberships,
                                &membership_key(digest, previous.logical_id),
                                None,
                            )?
                            .is_some())
                {
                    return Err(corrupt("deleted source retains current ID or membership"));
                }
            }
            FileSourceEdit::Upsert { coverage, bytes } => {
                if <[u8; 32]>::from(Sha256::digest(bytes)) != coverage.source.source_sha256
                    || bytes.len() > super::super::MAX_FILE_BYTES
                {
                    return Err(corrupt("mutation bytes differ from source coverage"));
                }
                let (reference, head) = optional_source_head(store, root, key)?
                    .ok_or_else(|| corrupt("upserted source head is absent"))?;
                if head.coverage != *coverage
                    || head.source_bytes != size_u64(bytes.len())?
                    || head.body.sha256 != coverage.source.source_sha256
                    || store.read_blob(&head.body, size_u64(super::super::MAX_FILE_BYTES)?, None)?
                        != *bytes
                {
                    return Err(corrupt("upserted source head/body differs from mutation"));
                }
                let encoded = crate::channel_payloads::encode_cbor(
                    &(super::policy_digest(), coverage),
                    "file derived oracle",
                )?;
                let mut hash = Sha256::new();
                hash.update(b"quanta-file-derived-input-v17\0");
                hash.update(encoded);
                if <[u8; 32]>::from(hash.finalize()) != head.derived_input_sha256 {
                    return Err(corrupt("source derived-input digest differs"));
                }
                let exact_inheritance = prior.as_ref().is_some_and(|(_, previous)| {
                    previous.coverage == *coverage
                        && previous.body == head.body
                        && previous.logical_id == head.logical_id
                });
                if exact_inheritance {
                    let (old_ref, old_head) = prior
                        .as_ref()
                        .ok_or_else(|| corrupt("exact inherited source lacks base"))?;
                    if *old_ref != reference
                        || live_doc(store, old, old_head.logical_id, None)?
                            != live_doc(store, root, head.logical_id, None)?
                    {
                        return Err(corrupt("exact source inheritance changed references"));
                    }
                } else {
                    if let Some(segment) = prior_segment {
                        let _inserted = retired_segments.insert(segment);
                    }
                    if reservation.count == 0
                        || head.logical_id < reservation.first
                        || head.logical_id > range_last
                        || prior
                            .as_ref()
                            .is_some_and(|(_, previous)| previous.logical_id == head.logical_id)
                    {
                        return Err(corrupt("changed source did not use a fresh reserved ID"));
                    }
                    let _inserted = created_segments
                        .insert(live_doc(store, root, head.logical_id, None)?.segment_sha256);
                    if let Some((_, previous)) = &prior
                        && (store
                            .map_get(&root.current_docs, &doc_key(previous.logical_id), None)?
                            .is_some()
                            || store
                                .map_get(
                                    &root.memberships,
                                    &membership_key(digest, previous.logical_id),
                                    None,
                                )?
                                .is_some())
                    {
                        return Err(corrupt("replaced source retains retired ID or membership"));
                    }
                }
                source_count = source_count
                    .checked_add(1)
                    .ok_or_else(|| corrupt("source count overflow"))?;
                source_bytes = source_bytes
                    .checked_add(head.source_bytes)
                    .ok_or_else(|| corrupt("source byte count overflow"))?;
                memberships = memberships
                    .checked_add(head.posting_memberships)
                    .ok_or_else(|| corrupt("membership count overflow"))?;
                let _inserted = edited_ids.insert(head.logical_id);
                let _inserted =
                    allowed_memberships.insert(membership_key(digest, head.logical_id).to_vec());
                let committed_membership = store
                    .map_get(
                        &root.memberships,
                        &membership_key(digest, head.logical_id),
                        None,
                    )?
                    .ok_or_else(|| corrupt("upserted membership is absent"))?;
                if committed_membership != reference {
                    return Err(corrupt("upserted membership differs from source head"));
                }
            }
        }
    }
    if root.live_sources != source_count
        || root.live_docs != source_count
        || root.source_bytes != source_bytes
        || root.membership_count != memberships
    {
        return Err(corrupt(
            "root summaries differ from base plus changed sources",
        ));
    }
    store.map_changed_keys(
        &old.source_heads,
        &root.source_heads,
        None,
        &mut |key, _, _| {
            let digest: [u8; 32] = key
                .try_into()
                .map_err(|_conversion_error| corrupt("changed source map key is not a digest"))?;
            if !allowed_sources.contains(&digest) {
                return Err(corrupt("unrelated source head changed"));
            }
            Ok(())
        },
    )?;
    let due_segment = {
        let mut cursor = store.map_cursor(&old.compaction_debt, &[], None, None)?;
        let mut next = None;
        while let Some((key, _)) = cursor.next()? {
            let digest: [u8; 32] = key
                .as_slice()
                .try_into()
                .map_err(|_conversion_error| corrupt("base compaction debt key is not a digest"))?;
            if !retired_segments.contains(&digest) {
                next = Some(digest);
                break;
            }
        }
        next
    };
    let clean_selection = match maintenance_policy {
        Some(policy) if !mutation.clear_all => {
            selected_clean_merge_runs(store, old, &retired_segments, policy, mutation, hard_scan)?
        }
        _ => CleanRunSelection {
            run_ids: Vec::new(),
            progress: old.run_registry.selection_progress(),
        },
    };
    let mut compacted_allowed = retired_segments.clone();
    if let Some(digest) = due_segment {
        let _inserted = compacted_allowed.insert(digest);
    }
    store.map_changed_keys(
        &old.current_docs,
        &root.current_docs,
        None,
        &mut |key, before, after| {
            let raw: [u8; 8] = key
                .try_into()
                .map_err(|_conversion_error| corrupt("changed current key is not a u64"))?;
            let id = u64::from_be_bytes(raw);
            if edited_ids.contains(&id) {
                return Ok(());
            }
            let (before, after) = before
                .zip(after)
                .ok_or_else(|| corrupt("unrelated current logical ID added or retired"))?;
            let old_locator = LiveDoc::decode(&store.read_blob(
                before,
                size_u64(super::MAX_RECORD_BYTES)?,
                None,
            )?)?;
            let new_locator = LiveDoc::decode(&store.read_blob(
                after,
                size_u64(super::MAX_RECORD_BYTES)?,
                None,
            )?)?;
            if old_locator.logical_id != id
                || new_locator.logical_id != id
                || old_locator.source_key_sha256 != new_locator.source_key_sha256
                || !compacted_allowed.contains(&old_locator.segment_sha256)
            {
                return Err(corrupt(
                    "unrelated current locator changed outside compaction debt",
                ));
            }
            let _inserted = created_segments.insert(new_locator.segment_sha256);
            Ok(())
        },
    )?;
    store.map_changed_keys(
        &old.memberships,
        &root.memberships,
        None,
        &mut |key, _, _| {
            if !allowed_memberships.contains(key) {
                return Err(corrupt("unrelated source membership changed"));
            }
            Ok(())
        },
    )?;
    let mut removed_registry_segments = Vec::new();
    let mut added_registry_segments = Vec::new();
    store.map_changed_keys(
        &old.segments,
        &root.segments,
        None,
        &mut |key, before, after| {
            let digest: [u8; 32] = key
                .try_into()
                .map_err(|_conversion_error| corrupt("changed segment key is not a digest"))?;
            if (after.is_some() && !created_segments.contains(&digest))
                || (before.is_some() && after.is_none() && !compacted_allowed.contains(&digest))
            {
                return Err(corrupt("unrelated segment descriptor changed"));
            }
            if let Some(reference) = after
                && (reference.root.sha256 != digest || reference.root.kind != ObjectKind::Segment)
            {
                return Err(corrupt("segment key and reference differ"));
            }
            if let Some(reference) = before {
                removed_registry_segments.push(SegmentRemoval {
                    key: key.to_vec(),
                    reference: reference.clone(),
                });
            }
            if let Some(reference) = after {
                let index_encoded_bytes = segment_index_encoded_bytes(
                    store,
                    std::slice::from_ref(&reference.root),
                    None,
                    size_u64(crate::lexical_store::MAX_CLOSURE_REFS)?,
                )?;
                added_registry_segments.push(SegmentInput {
                    key: key.to_vec(),
                    reference: reference.clone(),
                    index_encoded_bytes,
                });
            }
            Ok(())
        },
    )?;
    let premerge = if clean_selection.run_ids.is_empty() {
        if root.premerge_run_registry.is_some() {
            return Err(corrupt(
                "unselected cohort merge carried an intermediate registry",
            ));
        }
        &root.run_registry
    } else {
        root.premerge_run_registry
            .as_ref()
            .ok_or_else(|| corrupt("cohort merge omitted physical-delta registry witness"))?
    };
    let physical_progress = if clean_selection.run_ids.is_empty() {
        clean_selection.progress
    } else {
        old.run_registry.selection_progress()
    };
    old.run_registry.verify_transition_with_progress(
        premerge,
        store,
        &removed_registry_segments,
        &added_registry_segments,
        None,
        physical_progress,
    )?;
    if !clean_selection.run_ids.is_empty() {
        premerge.verify_cohort_merge_transition(
            &root.run_registry,
            store,
            &clean_selection.run_ids,
            clean_selection.progress,
            None,
        )?;
    }
    store.map_changed_keys(
        &old.compaction_debt,
        &root.compaction_debt,
        None,
        &mut |key, before, after| {
            let digest: [u8; 32] = key.try_into().map_err(|_conversion_error| {
                corrupt("changed compaction debt key is not a digest")
            })?;
            if (after.is_some() && !retired_segments.contains(&digest))
                || (before.is_some() && after.is_none() && !compacted_allowed.contains(&digest))
            {
                return Err(corrupt("unrelated compaction debt changed"));
            }
            if let Some(reference) = after {
                let inherited = store
                    .map_get(&old.segments, &digest, None)?
                    .ok_or_else(|| corrupt("debt references unknown base segment"))?;
                if *reference != inherited {
                    return Err(corrupt("debt reference differs from base segment"));
                }
            }
            Ok(())
        },
    )?;
    let mut removed_segments = BTreeSet::new();
    for digest in compacted_allowed {
        let reference = store
            .map_get(&old.segments, &digest, None)?
            .ok_or_else(|| corrupt("compaction debt segment absent in base"))?;
        let segment = Segment::decode(&store.read_blob(
            &reference,
            size_u64(super::MAX_RECORD_BYTES)?,
            None,
        )?)?;
        let removed = store.map_get(&root.segments, &digest, None)?.is_none();
        if retired_segments.contains(&digest) && !removed {
            return Err(corrupt(
                "selected segment was not retired in changed publication",
            ));
        }
        let mut still_current = 0_usize;
        for (local, id) in segment.doc_ids.iter().enumerate() {
            if let Some(current) = store.map_get(&root.current_docs, &doc_key(*id), None)? {
                let locator = LiveDoc::decode(&store.read_blob(
                    &current,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    None,
                )?)?;
                if locator.segment_sha256 == digest && index_usize(locator.local_id)? == local {
                    still_current = still_current
                        .checked_add(1)
                        .ok_or_else(|| corrupt("still-current count overflows"))?;
                }
            }
        }
        if removed && still_current != 0 {
            return Err(corrupt(
                "compaction removed a segment with live current IDs",
            ));
        }
        if !removed
            && still_current != segment.doc_ids.len()
            && store
                .map_get(&root.compaction_debt, &digest, None)?
                .is_none()
        {
            return Err(corrupt("partially retired segment lost compaction debt"));
        }
        if removed {
            let _inserted = removed_segments.insert(digest);
        }
    }
    for digest in created_segments {
        if removed_segments.contains(&digest) {
            return Err(corrupt("segment is both created and removed"));
        }
        let reference = store
            .map_get(&root.segments, &digest, None)?
            .ok_or_else(|| corrupt("new current locator segment absent"))?;
        verify_segment(store, root, &reference)?;
    }
    let facts = verify_run_posting_transition(
        store,
        old,
        root,
        premerge,
        &removed_registry_segments,
        &added_registry_segments,
        &clean_selection.run_ids,
    )?;
    facts.validate()?;
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_authority::paged::{
        FileAuthorityMutation, FileSourceEdit, PagedFileAuthority, build_for_seal,
        build_for_seal_prepared_with_policy,
    };
    use crate::file_authority::{PostingSurface, QueryWork};
    use crate::lexical_store::{AttemptBinding, MapEdit};
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileCoverage, SourceFileRevision,
        SymbolCoverage, SymbolNameSourcePolicyV1,
    };
    use quanta_index_lq_trigram::NecessaryGramExpr;

    fn coverage(path: &str, body: &[u8]) -> SourceFileCoverage {
        SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("repo").expect("repo"),
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("revision").expect("revision"),
                source_sha256: Sha256::digest(body).into(),
            },
            language: LanguageCode::new("rust").expect("language"),
            producer_policy_sha256: [6; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: [7; 32],
            text_admitted: true,
            symbols: SymbolCoverage::NotRequested,
        }
    }

    fn binding(target_generation: u64) -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [8; 32],
            source_event_sha256: [u8::try_from(target_generation)
                .expect("test generation fits in one byte"); 32],
            target_generation,
            base_root_sha256: None,
            operations_sha256: [4; 32],
            format: 17,
        }
    }

    #[test]
    fn hard_pressure_append_finds_pair_after_full_soft_window() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let mut large = String::new();
        for value in 0_u16..1_024 {
            std::fmt::Write::write_fmt(&mut large, format_args!("{value:04x}"))
                .expect("format fixed u16 key");
        }
        let large = large.into_bytes();
        let mut medium = String::new();
        for value in 0_u16..128 {
            std::fmt::Write::write_fmt(&mut medium, format_args!("{value:04x}"))
                .expect("format fixed u16 key");
        }
        let medium = medium.into_bytes();
        let mut base = None;
        for id in 1_u64..=34 {
            let body = if id == 1 {
                medium.clone()
            } else if id <= 32 {
                large.clone()
            } else {
                format!("tail-{id}").into_bytes()
            };
            let mutation = FileAuthorityMutation {
                edits: vec![FileSourceEdit::Upsert {
                    coverage: coverage(&format!("file-{id}.rs"), &body),
                    bytes: body,
                }],
                clear_all: false,
                operations_sha256: [4; 32],
            };
            let inherited = base
                .as_ref()
                .map_or_else(Vec::new, AuthorityRoot::object_roots);
            let lease = store
                .begin_attempt(&binding(id), &inherited)
                .expect("append attempt");
            let reservation = DocVersionReservation {
                first: id,
                count: 1,
                scope_sha256: [5; 32],
            };
            let built = build_for_seal(&store, &lease, base.as_ref(), &reservation, &mutation)
                .expect("append one run");
            lease
                .finish_published(&built.root.object_roots())
                .expect("publish append");
            base = Some(built.root);
        }
        let base = base.expect("34 run base");
        assert_eq!(base.run_registry.runs.entries, 34);
        let cost = |id: u64| {
            let candidate = base
                .run_registry
                .candidate_by_id(&store, id, None)
                .expect("candidate read")
                .expect("candidate exists");
            let reference = store
                .map_get(&base.run_postings, &id.to_be_bytes(), None)
                .expect("posting lookup")
                .expect("posting exists");
            let posting = RunPostingRoot::load(&store, &reference, None).expect("posting root");
            candidate.index_encoded_bytes
                + posting
                    .bundle_encoded_bytes(&reference, &store)
                    .expect("posting cost")
        };
        let pair_cap = cost(1).max(cost(33) + cost(34));
        assert!(cost(1) <= pair_cap, "first anchor is individually eligible");
        assert!(cost(1) + cost(33) > pair_cap, "first anchor blocks tail");
        assert!(cost(33) + cost(34) <= pair_cap, "tail pair fits");
        for id in 2_u64..=32 {
            assert!(cost(id) > pair_cap, "prefix run {id} must be ineligible");
        }
        let policy = LexicalMaintenancePolicy::new(
            2,
            34,
            1,
            u64::MAX,
            8, // the soft scan covers exactly 32 old headers
            pair_cap,
            64 * 1024 * 1024,
            60_000,
        )
        .expect("policy");
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("file-35.rs", b"tail-35"),
                bytes: b"tail-35".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let reservation = DocVersionReservation {
            first: 35,
            count: 1,
            scope_sha256: [5; 32],
        };
        let hard_scan =
            super::super::producer::test_hard_scan_lane(&store, &base, &mutation, &policy)
                .expect("File hard scan fixture");
        let lease = store
            .begin_attempt(&binding(35), &base.object_roots())
            .expect("ordinary append attempt");
        let target = build_for_seal_prepared_with_policy(
            &store,
            &lease,
            Some(&base),
            &reservation,
            &mutation,
            None,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("ordinary append recovers hard pressure");
        let _seal_facts = verify_for_seal_with_merge_facts(
            &store,
            &target.root,
            Some(&base),
            &reservation,
            &mutation,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("independent seal");
        assert!(
            verify_for_seal_with_merge_facts(
                &store,
                &target.root,
                Some(&base),
                &reservation,
                &mutation,
                Some(&policy),
                None,
            )
            .is_err(),
            "hard seal cannot infer an absent checkpoint"
        );
        let mut forged = hard_scan.clone().expect("hard scan lane");
        forged
            .windows
            .first_mut()
            .expect("first window")
            .candidates
            .first_mut()
            .expect("first candidate")
            .eligibility = EligibilityWitness::Ineligible(IneligibleReason::InputCap);
        assert!(
            verify_for_seal_with_merge_facts(
                &store,
                &target.root,
                Some(&base),
                &reservation,
                &mutation,
                Some(&policy),
                Some(&forged),
            )
            .is_err(),
            "independent old-base oracle rejects forged eligibility"
        );
        let mut skipped = hard_scan.expect("hard scan lane");
        let later_window = skipped.windows.get_mut(1).expect("later scan window");
        let _omitted = later_window.candidates.remove(0);
        assert!(
            verify_for_seal_with_merge_facts(
                &store,
                &target.root,
                Some(&base),
                &reservation,
                &mutation,
                Some(&policy),
                Some(&skipped),
            )
            .is_err(),
            "independent old-base oracle rejects a skipped later candidate"
        );
        policy
            .admit_target(
                base.maintenance_snapshot(&store).expect("base counters"),
                target
                    .root
                    .maintenance_snapshot(&store)
                    .expect("target counters"),
            )
            .expect("ordinary target admission");
        assert_eq!(target.root.run_registry.runs.entries, 34);
        assert_eq!(target.root.live_docs, 35);
        assert!(
            target
                .root
                .run_registry
                .candidate_by_id(&store, 33, None)
                .expect("retired run")
                .is_none()
        );
        assert!(
            target
                .root
                .run_registry
                .candidate_by_id(&store, 34, None)
                .expect("retired run")
                .is_none()
        );
        let reader =
            PagedFileAuthority::open(&target.root_bytes, store.clone()).expect("target reader");
        for id in 1_u64..=35 {
            let body = if id == 1 {
                medium.clone()
            } else if id <= 32 {
                large.clone()
            } else {
                format!("tail-{id}").into_bytes()
            };
            let key = coverage(&format!("file-{id}.rs"), &body).source.file;
            assert_eq!(reader.id_for_key(&key, None).expect("source ID"), Some(id));
        }
        verify_all_for_scrub(&store, &target.root, None).expect("target scrub");
    }

    #[test]
    fn over_hard_ordinary_append_uses_three_way_tail_merge() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let mut large = String::new();
        for value in 0_u16..1_024 {
            std::fmt::Write::write_fmt(&mut large, format_args!("{value:04x}"))
                .expect("format fixed u16 key");
        }
        let large = large.into_bytes();
        let mut medium = String::new();
        for value in 0_u16..128 {
            std::fmt::Write::write_fmt(&mut medium, format_args!("{value:04x}"))
                .expect("format fixed u16 key");
        }
        let medium = medium.into_bytes();
        let mut base = None;
        for id in 1_u64..=20 {
            let body = if id == 1 {
                medium.clone()
            } else if id <= 17 {
                large.clone()
            } else {
                format!("tail-{id}").into_bytes()
            };
            let mutation = FileAuthorityMutation {
                edits: vec![FileSourceEdit::Upsert {
                    coverage: coverage(&format!("file-{id}.rs"), &body),
                    bytes: body,
                }],
                clear_all: false,
                operations_sha256: [4; 32],
            };
            let inherited = base
                .as_ref()
                .map_or_else(Vec::new, AuthorityRoot::object_roots);
            let lease = store
                .begin_attempt(&binding(id), &inherited)
                .expect("append attempt");
            let reservation = DocVersionReservation {
                first: id,
                count: 1,
                scope_sha256: [5; 32],
            };
            let built = build_for_seal(&store, &lease, base.as_ref(), &reservation, &mutation)
                .expect("append one run");
            lease
                .finish_published(&built.root.object_roots())
                .expect("publish append");
            base = Some(built.root);
        }
        let base = base.expect("20 run base");
        assert_eq!(base.run_registry.runs.entries, 20);
        let cost = |id: u64| {
            let candidate = base
                .run_registry
                .candidate_by_id(&store, id, None)
                .expect("candidate read")
                .expect("candidate exists");
            let reference = store
                .map_get(&base.run_postings, &id.to_be_bytes(), None)
                .expect("posting lookup")
                .expect("posting exists");
            let posting = RunPostingRoot::load(&store, &reference, None).expect("posting root");
            candidate.index_encoded_bytes
                + posting
                    .bundle_encoded_bytes(&reference, &store)
                    .expect("posting cost")
        };
        let tail_three = cost(18) + cost(19) + cost(20);
        let cap = cost(1).max(tail_three);
        assert!(cost(1) + cost(18) > cap, "first anchor blocks tail");
        for id in 2_u64..=17 {
            assert!(cost(id) > cap, "prefix run {id} must be ineligible");
        }
        let policy = LexicalMaintenancePolicy::new(
            2,
            19, // the existing 20-run root is already over hard
            1,
            u64::MAX,
            3, // the soft scan covers only 12 old headers
            cap,
            64 * 1024 * 1024,
            60_000,
        )
        .expect("policy");
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("file-21.rs", b"tail-21"),
                bytes: b"tail-21".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let reservation = DocVersionReservation {
            first: 21,
            count: 1,
            scope_sha256: [5; 32],
        };
        let hard_scan =
            super::super::producer::test_hard_scan_lane(&store, &base, &mutation, &policy)
                .expect("File hard scan fixture");
        let lease = store
            .begin_attempt(&binding(21), &base.object_roots())
            .expect("ordinary append attempt");
        let target = build_for_seal_prepared_with_policy(
            &store,
            &lease,
            Some(&base),
            &reservation,
            &mutation,
            None,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("ordinary append strictly improves run count");
        let _seal_facts = verify_for_seal_with_merge_facts(
            &store,
            &target.root,
            Some(&base),
            &reservation,
            &mutation,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("independent seal");
        policy
            .admit_target(
                base.maintenance_snapshot(&store).expect("base counters"),
                target
                    .root
                    .maintenance_snapshot(&store)
                    .expect("target counters"),
            )
            .expect("over-hard strict improvement");
        assert_eq!(target.root.run_registry.runs.entries, 19);
        assert_eq!(target.root.live_docs, 21);
        for id in 18_u64..=20 {
            assert!(
                target
                    .root
                    .run_registry
                    .candidate_by_id(&store, id, None)
                    .expect("retired run")
                    .is_none()
            );
        }
        let reader =
            PagedFileAuthority::open(&target.root_bytes, store.clone()).expect("target reader");
        for id in 1_u64..=21 {
            let body = if id == 1 {
                medium.clone()
            } else if id <= 17 {
                large.clone()
            } else {
                format!("tail-{id}").into_bytes()
            };
            let key = coverage(&format!("file-{id}.rs"), &body).source.file;
            assert_eq!(reader.id_for_key(&key, None).expect("source ID"), Some(id));
        }
        verify_all_for_scrub(&store, &target.root, None).expect("target scrub");
    }

    #[test]
    fn clean_append_only_runs_merge_serving_dictionary_and_require_declared_policy() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let scope = [5; 32];
        let first_mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("a.rs", b"alpha-one"),
                bytes: b"alpha-one".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let first_lease = store.begin_attempt(&binding(1), &[]).expect("first lease");
        let first_range = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: scope,
        };
        let first = build_for_seal(&store, &first_lease, None, &first_range, &first_mutation)
            .expect("first run");
        let first_facts = verify_for_seal_with_merge_facts(
            &store,
            &first.root,
            None,
            &first_range,
            &first_mutation,
            None,
            None,
        )
        .expect("first seal");
        assert_eq!(first_facts, FileRunMergeFacts::default());
        let second_mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("b.rs", b"beta-two"),
                bytes: b"beta-two".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let second_lease = store
            .begin_attempt(&binding(2), &first.root.object_roots())
            .expect("second lease");
        let second_range = DocVersionReservation {
            first: 2,
            count: 1,
            scope_sha256: scope,
        };
        let second = build_for_seal(
            &store,
            &second_lease,
            Some(&first.root),
            &second_range,
            &second_mutation,
        )
        .expect("second run");
        let append_facts = verify_for_seal_with_merge_facts(
            &store,
            &second.root,
            Some(&first.root),
            &second_range,
            &second_mutation,
            None,
            None,
        )
        .expect("second seal");
        assert_eq!(append_facts, FileRunMergeFacts::default());
        assert_eq!(second.root.run_registry.runs.entries, 2);
        assert_eq!(second.root.segments.entries, 2);
        let prior_segments = {
            let mut old_rows = store
                .map_cursor(&second.root.segments, &[], None, None)
                .expect("old segments");
            let mut digests = BTreeSet::new();
            while let Some((_, reference)) = old_rows.next().expect("old segment row") {
                let _inserted = digests.insert(reference.root.sha256);
            }
            digests
        };

        let maintenance =
            LexicalMaintenancePolicy::new(2, 4, 1_000_000, 2_000_000, 2, 10_000_000, 1_000_000, 1)
                .expect("maintenance policy");
        let no_changes = FileAuthorityMutation {
            edits: Vec::new(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let third_lease = store
            .begin_attempt(&binding(3), &second.root.object_roots())
            .expect("merge lease");
        let zero_range = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: scope,
        };
        let hard_scan = super::super::producer::test_hard_scan_lane(
            &store,
            &second.root,
            &no_changes,
            &maintenance,
        )
        .expect("File hard scan fixture");
        let merged = build_for_seal_prepared_with_policy(
            &store,
            &third_lease,
            Some(&second.root),
            &zero_range,
            &no_changes,
            None,
            Some(&maintenance),
            hard_scan.as_ref(),
        )
        .expect("run dictionary merge");
        let merge_facts = verify_for_seal_with_merge_facts(
            &store,
            &merged.root,
            Some(&second.root),
            &zero_range,
            &no_changes,
            Some(&maintenance),
            hard_scan.as_ref(),
        )
        .expect("independent merge seal");
        assert_eq!(merge_facts.input_runs, 2);
        assert_eq!(merge_facts.output_runs, 1);
        assert_eq!(merge_facts.transferred_segments, 2);
        assert_eq!(
            merge_facts.transferred_segment_attributed_bytes,
            second
                .root
                .run_registry
                .snapshot()
                .expect("old segment attribution")
                .active_encoded_bytes()
        );
        assert!(merge_facts.retired_run_posting_attributed_bytes > 0);
        assert!(merge_facts.created_run_posting_attributed_bytes > 0);
        assert_eq!(merged.root.run_registry.runs.entries, 1);
        assert_eq!(merged.root.run_postings.entries, 1);
        assert_eq!(merged.root.segments, second.root.segments);
        assert_eq!(merged.root.live_docs, 2);
        let mut merged_segments = store
            .map_cursor(&merged.root.segments, &[], None, None)
            .expect("merged segments");
        let mut inherited = BTreeSet::new();
        while let Some((_, reference)) = merged_segments.next().expect("merged row") {
            let _inserted = inherited.insert(reference.root.sha256);
        }
        assert_eq!(inherited, prior_segments);
        assert!(
            verify_for_seal(
                &store,
                &merged.root,
                Some(&second.root),
                &zero_range,
                &no_changes,
            )
            .is_err(),
            "an undeclared clean-run removal must be rejected"
        );
        verify_all_for_scrub(&store, &merged.root, None).expect("merged scrub");
        let authority = PagedFileAuthority::open(&merged.root_bytes, store).expect("merged reader");
        for (gram, id) in [(*b"alp", 1_u64), (*b"bet", 2_u64)] {
            let mut work = QueryWork::default();
            assert_eq!(
                authority
                    .necessary_candidates(
                        PostingSurface::Content,
                        &NecessaryGramExpr::Gram(gram),
                        &mut work,
                        &RequestBudgetV1::unbounded(),
                    )
                    .expect("merged query"),
                Some(vec![id])
            );
        }
    }

    #[test]
    fn seal_rejects_metadata_only_run_merge_that_omits_one_cohort() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let scope = [5; 32];
        let append = |base: Option<&AuthorityRoot>, generation, id, path, body: &[u8]| {
            let mutation = FileAuthorityMutation {
                edits: vec![FileSourceEdit::Upsert {
                    coverage: coverage(path, body),
                    bytes: body.to_vec(),
                }],
                clear_all: false,
                operations_sha256: [4; 32],
            };
            let lease = store
                .begin_attempt(
                    &binding(generation),
                    &base.map_or_else(Vec::new, AuthorityRoot::object_roots),
                )
                .expect("append lease");
            build_for_seal(
                &store,
                &lease,
                base,
                &DocVersionReservation {
                    first: id,
                    count: 1,
                    scope_sha256: scope,
                },
                &mutation,
            )
            .expect("append source")
            .root
        };
        let first = append(None, 1, 1, "a.rs", b"alpha-one");
        let base = append(Some(&first), 2, 2, "b.rs", b"beta-two");
        let policy =
            LexicalMaintenancePolicy::new(2, 8, 1_000_000, 2_000_000, 2, 10_000_000, 1_000_000, 1)
                .expect("pair policy");
        let no_changes = FileAuthorityMutation {
            edits: Vec::new(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let hard_scan =
            super::super::producer::test_hard_scan_lane(&store, &base, &no_changes, &policy)
                .expect("File hard scan fixture");
        let selection = selected_clean_merge_runs(
            &store,
            &base,
            &BTreeSet::new(),
            &policy,
            &no_changes,
            hard_scan.as_ref(),
        )
        .expect("eligible full cohorts");
        assert_eq!(selection.run_ids.len(), 2);
        let new_id = base.run_registry.next_run_id;
        let lease = store
            .begin_attempt(&binding(3), &base.object_roots())
            .expect("forgery lease");
        let first_ref = store
            .map_get(
                &base.run_postings,
                &selection
                    .run_ids
                    .first()
                    .expect("first selected run")
                    .to_be_bytes(),
                None,
            )
            .expect("first run lookup")
            .expect("first run root");
        let mut omitted =
            RunPostingRoot::load(&store, &first_ref, None).expect("first run posting");
        omitted.run_id = new_id;
        let omitted_ref = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &omitted.object_roots(),
                &omitted.encode().expect("omitted root encode"),
            )
            .expect("omitted root");
        let mut forged = base.clone();
        forged.premerge_run_registry = Some(base.run_registry.clone());
        forged.run_registry = base
            .run_registry
            .merge_cohorts(&store, &lease, &selection.run_ids, selection.progress)
            .expect("forged cohort ownership");
        let mut edits = selection
            .run_ids
            .iter()
            .map(|id| MapEdit {
                key: id.to_be_bytes().to_vec(),
                value: None,
            })
            .collect::<Vec<_>>();
        edits.push(MapEdit {
            key: new_id.to_be_bytes().to_vec(),
            value: Some(omitted_ref.clone()),
        });
        edits.sort_by(|left, right| left.key.cmp(&right.key));
        forged.run_postings = store
            .map_apply(&lease, &base.run_postings, edits.into_iter().map(Ok))
            .expect("forged run posting map");
        forged.run_index_encoded_bytes = omitted
            .bundle_encoded_bytes(&omitted_ref, &store)
            .expect("forged ledger");

        let reservation = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: scope,
        };
        assert!(
            verify_for_seal_with_merge_facts(
                &store,
                &forged,
                Some(&base),
                &reservation,
                &no_changes,
                Some(&policy),
                hard_scan.as_ref(),
            )
            .is_err(),
            "independent seal must reject omitted cohort postings"
        );
    }

    #[test]
    fn source_gram_oracle_has_fixed_small_and_large_goldens() {
        assert_eq!(
            derive_grams(&[0xff, 0, 0xff, 0, 0xff]).expect("short oracle"),
            vec![[0, 0xff, 0], [0xff, 0, 0xff]]
        );
        let repeated = vec![b'a'; 700_000];
        assert_eq!(
            derive_grams(&repeated).expect("large oracle"),
            vec![*b"aaa"]
        );
    }

    #[test]
    fn changed_source_retains_stale_posting_only_behind_current_filter() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let initial = FileAuthorityMutation {
            edits: vec![
                FileSourceEdit::Upsert {
                    coverage: coverage("a.rs", b"old-value"),
                    bytes: b"old-value".to_vec(),
                },
                FileSourceEdit::Upsert {
                    coverage: coverage("b.rs", b"stable-value"),
                    bytes: b"stable-value".to_vec(),
                },
            ],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let lease = store
            .begin_attempt(&binding(1), &[])
            .expect("first attempt");
        let first_range = DocVersionReservation {
            first: 1,
            count: 2,
            scope_sha256: [5; 32],
        };
        let first =
            build_for_seal(&store, &lease, None, &first_range, &initial).expect("first build");
        verify_for_seal(&store, &first.root, None, &first_range, &initial)
            .expect("first seal proof");
        let update = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("a.rs", b"new-value"),
                bytes: b"new-value".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let lease = store
            .begin_attempt(&binding(2), &first.root.object_roots())
            .expect("second attempt");
        let second_range = DocVersionReservation {
            first: 3,
            count: 1,
            scope_sha256: [5; 32],
        };
        let second = build_for_seal(&store, &lease, Some(&first.root), &second_range, &update)
            .expect("second build");
        verify_for_seal(
            &store,
            &second.root,
            Some(&first.root),
            &second_range,
            &update,
        )
        .expect("second seal proof");
        let delete = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Delete {
                key: coverage("b.rs", b"stable-value").source.file,
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let delete_lease = store
            .begin_attempt(&binding(3), &second.root.object_roots())
            .expect("delete attempt");
        let delete_range = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [5; 32],
        };
        let third = build_for_seal(
            &store,
            &delete_lease,
            Some(&second.root),
            &delete_range,
            &delete,
        )
        .expect("delete build");
        verify_for_seal(
            &store,
            &third.root,
            Some(&second.root),
            &delete_range,
            &delete,
        )
        .expect("delete seal proof");
        verify_all_for_scrub(&store, &second.root, None).expect("scrub");
        let metered = store.metered_clone();
        let mut cursor = FileScrubCursor::default();
        let mut rows = 0_u64;
        let mut reads = 0_u64;
        loop {
            let step = verify_scrub_step(&metered, &second.root, &mut cursor, None)
                .expect("resumable scrub step");
            reads = reads.checked_add(step.read_count).expect("read count");
            if step.done {
                break;
            }
            rows = rows.checked_add(1).expect("row count");
        }
        assert!(rows >= 4);
        assert!(reads > 0);
        assert!(verify_scrub_step(&metered, &first.root, &mut cursor, None).is_err());
        let authority =
            PagedFileAuthority::open(&second.root_bytes, store.clone()).expect("reader");
        let mut work = QueryWork::default();
        let old = authority
            .necessary_candidates(
                PostingSurface::Content,
                &NecessaryGramExpr::Gram(*b"old"),
                &mut work,
                &RequestBudgetV1::unbounded(),
            )
            .expect("old candidates");
        assert_eq!(old, Some(Vec::new()));
        let mut work = QueryWork::default();
        let new = authority
            .necessary_candidates(
                PostingSurface::Content,
                &NecessaryGramExpr::Gram(*b"new"),
                &mut work,
                &RequestBudgetV1::unbounded(),
            )
            .expect("new candidates");
        assert_eq!(new, Some(vec![3]));
        let mut work = QueryWork::default();
        assert_eq!(
            authority
                .necessary_candidates(
                    PostingSurface::Content,
                    &NecessaryGramExpr::And(vec![
                        NecessaryGramExpr::Gram(*b"old"),
                        NecessaryGramExpr::Gram(*b"val"),
                    ]),
                    &mut work,
                    &RequestBudgetV1::unbounded(),
                )
                .expect("stale conjunction"),
            Some(Vec::new())
        );
        let mut work = QueryWork::default();
        assert_eq!(
            authority
                .necessary_candidates(
                    PostingSurface::Path,
                    &NecessaryGramExpr::And(vec![
                        NecessaryGramExpr::Gram(*b"a.r"),
                        NecessaryGramExpr::Gram(*b".rs"),
                    ]),
                    &mut work,
                    &RequestBudgetV1::unbounded(),
                )
                .expect("updated path conjunction"),
            Some(vec![3])
        );
        let deleted = PagedFileAuthority::open(&third.root_bytes, store).expect("deleted reader");
        let mut work = QueryWork::default();
        assert_eq!(
            deleted
                .necessary_candidates(
                    PostingSurface::Path,
                    &NecessaryGramExpr::And(vec![
                        NecessaryGramExpr::Gram(*b"b.r"),
                        NecessaryGramExpr::Gram(*b".rs"),
                    ]),
                    &mut work,
                    &RequestBudgetV1::unbounded(),
                )
                .expect("deleted path conjunction"),
            Some(Vec::new())
        );
        for expression in [
            NecessaryGramExpr::And(vec![
                NecessaryGramExpr::Or(vec![
                    NecessaryGramExpr::Gram(*b"val"),
                    NecessaryGramExpr::Gram(*b"old"),
                ]),
                NecessaryGramExpr::Gram(*b"new"),
            ]),
            NecessaryGramExpr::And(vec![
                NecessaryGramExpr::Gram(*b"new"),
                NecessaryGramExpr::Or(vec![
                    NecessaryGramExpr::AllDocs,
                    NecessaryGramExpr::Gram(*b"old"),
                ]),
            ]),
        ] {
            let mut work = QueryWork::default();
            let candidates = authority
                .necessary_candidates(
                    PostingSurface::Content,
                    &expression,
                    &mut work,
                    &RequestBudgetV1::unbounded(),
                )
                .expect("nested filtered candidates");
            assert_eq!(candidates, Some(vec![3]));
        }
        let mut alternatives = Vec::new();
        alternatives.push(NecessaryGramExpr::Gram(*b"new"));
        for prefix in 0_u8..64 {
            alternatives.push(NecessaryGramExpr::Gram([prefix, 0, 0]));
        }
        let scoped = authority.query_scope();
        let mut work = QueryWork::default();
        let candidates = scoped
            .necessary_candidates(
                PostingSurface::Content,
                &NecessaryGramExpr::Or(alternatives),
                &mut work,
                &RequestBudgetV1::unbounded(),
            )
            .expect("wide disjunction candidates");
        assert_eq!(candidates, Some(vec![3]));
        let (charged_bytes, charged_reads) = work.authenticated_io();
        assert!(charged_bytes > 0 && charged_reads > 0);
        let before_body = scoped.query_io_snapshot();
        let selected = scoped
            .load_file_for_id(3, &RequestBudgetV1::unbounded())
            .expect("selected body");
        assert_eq!(&*selected.bytes, b"new-value");
        let after_body = scoped.query_io_snapshot();
        assert!(after_body.0 > before_body.0 && after_body.1 > before_body.1);
    }

    #[test]
    fn segment_major_conjunction_preserves_broad_regex_and_long_gram_goldens() {
        // N=128 documents across two 64-document segments. The old
        // gram-at-a-time path re-authenticated at least N current records for
        // each of K=19 common grams: 2,432 record reads before final scoring.
        // The bounds below were fixed from that independent lower bound;
        // selected map/segment/posting reads have room within them.
        const FILES: usize = 128;
        const REGEX_READ_BOUND: u64 = 224;
        const LONG_AND_READ_BOUND: u64 = 512;
        let target = "scalefileneedle001file00000";
        let near_miss = "scalefileneedle001file00001";
        let common = "abcdefghijklmnopqrstu";
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let edits = (0..FILES)
            .map(|index| {
                let anchor = if index == 0 { target } else { near_miss };
                let bytes = format!("{anchor} {common}").into_bytes();
                FileSourceEdit::Upsert {
                    coverage: coverage(&format!("src/f{index:03}.rs"), &bytes),
                    bytes,
                }
            })
            .collect();
        let mutation = FileAuthorityMutation {
            edits,
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
        let reservation = DocVersionReservation {
            first: 1,
            count: u64::try_from(FILES).expect("fixed file count"),
            scope_sha256: [5; 32],
        };
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation)
            .expect("two-segment build");
        verify_for_seal(&store, &built.root, None, &reservation, &mutation)
            .expect("independent source oracle");
        let authority = PagedFileAuthority::open(&built.root_bytes, store).expect("reader");
        let budget = RequestBudgetV1::unbounded();
        let expected: Vec<u64> = (1..=u64::try_from(FILES).expect("fixed file count")).collect();

        // CodeSearch's default case:no prepends (?i). Its current bounded
        // HIR condition has broad numeric grams; the exact regex still has
        // one match. Keep this distinct from the explicit 19-gram AND below.
        let pattern = format!("(?i){target}");
        let (plan, prepared) = crate::regex::plan_regex_prepared(
            &pattern,
            &quanta_index_contract::LqOptions::defaults(),
            &crate::regex::RegexPolicy::defaults(),
        )
        .expect("default-case regex plan");
        let exact = quanta_index_lq_regex::RegexExecutor::compile_prepared(prepared)
            .expect("exact matcher");
        assert!(exact.verify(target.as_bytes()));
        assert!(!exact.verify(near_miss.as_bytes()));
        let regex_scope = authority.query_scope();
        let mut regex_work = QueryWork::default();
        let regex_ids = regex_scope
            .necessary_candidates(
                PostingSurface::Content,
                plan.necessary_gram_expr(),
                &mut regex_work,
                &budget,
            )
            .expect("default-case necessary candidates");
        assert_eq!(regex_ids, Some(expected.clone()));
        let (_, regex_reads) = regex_work.authenticated_io();
        assert!(
            regex_reads > 0 && regex_reads <= REGEX_READ_BOUND,
            "{regex_reads}"
        );

        let grams: Vec<_> = common
            .as_bytes()
            .windows(3)
            .map(|window| NecessaryGramExpr::Gram(window.try_into().expect("three-byte gram")))
            .collect();
        assert_eq!(grams.len(), 19);
        let long_scope = authority.query_scope();
        let mut long_work = QueryWork::default();
        let long_ids = long_scope
            .necessary_candidates(
                PostingSurface::Content,
                &NecessaryGramExpr::And(grams),
                &mut long_work,
                &budget,
            )
            .expect("19-gram conjunction");
        assert_eq!(long_ids, Some(expected));
        let (_, long_reads) = long_work.authenticated_io();
        assert!(
            long_reads > 0 && long_reads <= LONG_AND_READ_BOUND,
            "{long_reads}"
        );

        let mut work = QueryWork::default();
        assert_eq!(
            authority
                .necessary_candidates(
                    PostingSurface::Content,
                    &NecessaryGramExpr::And(vec![
                        NecessaryGramExpr::Gram(*b"abc"),
                        NecessaryGramExpr::Gram(*b"zzz"),
                    ]),
                    &mut work,
                    &budget,
                )
                .expect("missing gram"),
            Some(Vec::new())
        );
    }

    #[test]
    fn run_scrub_retries_final_gram_after_transient_count_read_failure() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("retry.rs", b"abcabc"),
                bytes: b"abcabc".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
        let reservation = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: [5; 32],
        };
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation)
            .expect("valid run fixture");
        let mut cursor = RunPostingScrubCursor::default();
        let mut at_final_flush = false;
        for _ in 0..1024 {
            if cursor.active.as_ref().is_some_and(|run| {
                run.phase == RunScrubPhase::Pages
                    && run.seen_pages == run.root.pages.entries
                    && run.current_gram.is_some()
            }) {
                at_final_flush = true;
                break;
            }
            assert!(
                !cursor
                    .step(&store, &built.root, None)
                    .expect("prefix scrub")
            );
        }
        assert!(at_final_flush, "fixture did not reach final gram flush");
        let run = cursor.active.as_ref().expect("active run");
        let gram = run.current_gram.expect("final gram");
        let count_ref = store
            .map_get(&run.root.gram_counts, &gram, None)
            .expect("count lookup")
            .expect("final count");
        let hex = count_ref
            .root
            .sha256
            .iter()
            .fold(String::new(), |mut text, byte| {
                use std::fmt::Write as _;
                write!(&mut text, "{byte:02x}").expect("digest hex");
                text
            });
        let count_path = directory
            .path()
            .join("lexical-object-pool")
            .join("objects")
            .join(hex.get(..2).expect("shard name"))
            .join(&hex);
        let hidden_path = directory.path().join("temporarily-hidden-count");
        let original_bytes = std::fs::read(&count_path).expect("count object bytes");
        std::fs::rename(&count_path, &hidden_path).expect("inject missing count object");
        assert!(cursor.step(&store, &built.root, None).is_err());
        std::fs::rename(&hidden_path, &count_path).expect("restore count object");
        assert_eq!(
            std::fs::read(&count_path).expect("restored bytes"),
            original_bytes
        );
        let mut completed = false;
        for _ in 0..1024 {
            if cursor
                .step(&store, &built.root, None)
                .expect("resume scrub")
            {
                completed = true;
                break;
            }
        }
        assert!(completed, "resumed scrub did not complete");
        assert_eq!(cursor.seen_runs, built.root.run_postings.entries);
        assert_eq!(cursor.encoded_bytes, built.root.run_index_encoded_bytes);
    }

    #[test]
    fn seal_and_full_scrub_reject_forged_run_root_accounting() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("accounting.rs", b"abcabc"),
                bytes: b"abcabc".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let reservation = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: [5; 32],
        };
        let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation)
            .expect("valid one-source fixture");
        verify_for_seal(&store, &built.root, None, &reservation, &mutation)
            .expect("unmodified transition seal");
        verify_all_for_scrub(&store, &built.root, None).expect("unmodified full scrub");

        let mut forged_run_bytes = built.root.clone();
        forged_run_bytes.run_index_encoded_bytes = forged_run_bytes
            .run_index_encoded_bytes
            .checked_add(1)
            .expect("fixture byte headroom");
        let mut forged_outer_entries = built.root;
        forged_outer_entries.run_postings.entries = forged_outer_entries
            .run_postings
            .entries
            .checked_add(1)
            .expect("fixture entry headroom");
        for (name, forged) in [
            ("run byte ledger", forged_run_bytes),
            ("outer run count", forged_outer_entries),
        ] {
            assert!(
                verify_for_seal(&store, &forged, None, &reservation, &mutation).is_err(),
                "transition seal accepted forged {name}"
            );
            assert!(
                verify_all_for_scrub(&store, &forged, None).is_err(),
                "full scrub accepted forged {name}"
            );
        }
    }

    #[test]
    fn full_scrub_rejects_self_consistent_forged_run_member_width() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("width.rs", b"abcabc"),
                bytes: b"abcabc".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let reservation = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: [5; 32],
        };
        let lease = store.begin_attempt(&binding(1), &[]).expect("attempt");
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation)
            .expect("valid one-source fixture");
        verify_all_for_scrub(&store, &built.root, None).expect("unmodified full scrub");

        let (segment_key, segment_reference) = {
            let mut rows = store
                .map_cursor(&built.root.segments, &[], None, None)
                .expect("segment cursor");
            let first = rows.next().expect("segment row read").expect("one segment");
            assert!(rows.next().expect("segment cursor end").is_none());
            first
        };
        let false_width = built
            .root
            .run_registry
            .active_index_encoded_bytes
            .checked_add(1)
            .expect("fixture byte headroom");
        let mut forged = built.root.clone();
        forged.run_registry = RunRegistry::empty()
            .apply(
                &store,
                &lease,
                &[],
                &[SegmentInput {
                    key: segment_key,
                    reference: segment_reference,
                    index_encoded_bytes: false_width,
                }],
            )
            .expect("internally consistent forged run");
        forged
            .run_registry
            .validate()
            .expect("valid registry shape");
        forged
            .run_registry
            .verify_all(&store, None)
            .expect("member, header, and total agree");
        let _encoded_root = forged.encode().expect("valid root control shape");
        let mut restored = forged.clone();
        restored.run_registry = built.root.run_registry.clone();
        assert_eq!(restored, built.root, "only run registry was forged");
        let error = verify_all_for_scrub(&store, &forged, None)
            .expect_err("physical segment index width must reject the forged registry");
        assert!(
            matches!(&error, CoreError::Typed { code, message }
                if *code == quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE
                    && message == "lexical run registry corrupt: segment index byte witness differs"),
            "unexpected scrub refusal: {error:?}"
        );
    }

    #[test]
    fn seal_route_inventory_rejects_missing_extra_count_and_value() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let initial = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("route.rs", b"abcabc"),
                bytes: b"abcabc".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let lease = store
            .begin_attempt(&binding(1), &[])
            .expect("route attempt");
        let reservation = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: [5; 32],
        };
        let built = build_for_seal(&store, &lease, None, &reservation, &initial)
            .expect("valid route build");
        verify_for_seal(&store, &built.root, None, &reservation, &initial)
            .expect("valid route seal");
        let mut forged_run_bytes = built.root.clone();
        forged_run_bytes.run_registry.active_index_encoded_bytes = forged_run_bytes
            .run_registry
            .active_index_encoded_bytes
            .checked_add(1)
            .expect("fixture byte headroom");
        assert!(
            verify_for_seal(&store, &forged_run_bytes, None, &reservation, &initial).is_err(),
            "seal oracle must reject a forged run byte total"
        );
        assert!(
            verify_all_for_scrub(&store, &forged_run_bytes, None).is_err(),
            "bounded scrub must reject a forged run byte total"
        );

        let (correct_key, count_ref) = store
            .map_cursor(&built.root.content_routes, &[], None, None)
            .expect("content route cursor")
            .next()
            .expect("content route read")
            .expect("content route exists");
        let (gram, count, run_id) = route_parts(&correct_key, 2).expect("run route shape");
        let source_digest = source_key_digest(initial.edits.first().expect("one edit").key())
            .expect("source digest");
        let (head_ref, _) =
            source_head(&store, &built.root, source_digest, None).expect("source head");
        let extra_key = run_route_key(2, [0, 0, 0], 1, run_id).expect("extra route key");
        let wrong_count_key =
            run_route_key(2, gram, count.checked_add(1).expect("count room"), run_id)
                .expect("wrong count route key");
        for (name, edits) in [
            (
                "missing",
                vec![MapEdit {
                    key: correct_key.clone(),
                    value: None,
                }],
            ),
            (
                "extra",
                vec![MapEdit {
                    key: extra_key.to_vec(),
                    value: Some(count_ref.clone()),
                }],
            ),
            (
                "wrong count",
                vec![
                    MapEdit {
                        key: correct_key.clone(),
                        value: None,
                    },
                    MapEdit {
                        key: wrong_count_key.to_vec(),
                        value: Some(count_ref),
                    },
                ],
            ),
            (
                "wrong value",
                vec![MapEdit {
                    key: correct_key,
                    value: Some(head_ref),
                }],
            ),
        ] {
            let mut edits = edits;
            edits.sort_by(|left, right| left.key.cmp(&right.key));
            let mut forged = built.root.clone();
            forged.content_routes = store
                .map_apply(
                    &lease,
                    &built.root.content_routes,
                    edits.into_iter().map(Ok),
                )
                .expect("forge route map");
            assert!(
                matches!(
                    verify_for_seal(&store, &forged, None, &reservation, &initial),
                    Err(CoreError::Typed {
                        code:
                            quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                        ..
                    })
                ),
                "{name} route must be refused"
            );
        }
    }

    #[test]
    fn seal_rejects_coherently_rehashed_source_against_fixed_mutation() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let original = b"abcabc";
        let forged = b"bcabca";
        let fixed = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("fixed.rs", original),
                bytes: original.to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let reservation = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: [5; 32],
        };
        let lease = store
            .begin_attempt(&binding(1), &[])
            .expect("fixed attempt");
        let built =
            build_for_seal(&store, &lease, None, &reservation, &fixed).expect("fixed build");
        verify_for_seal(&store, &built.root, None, &reservation, &fixed).expect("fixed seal");
        let key = fixed.edits.first().expect("fixture has an edit").key();
        let digest = source_key_digest(key).expect("source key");
        let (_, mut head) = source_head(&store, &built.root, digest, None).expect("fixed head");
        let forged_body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], forged)
            .expect("forged body");
        head.coverage.source.source_sha256 = Sha256::digest(forged).into();
        head.body = forged_body.clone();
        let encoded = crate::channel_payloads::encode_cbor(
            &(super::super::policy_digest(), &head.coverage),
            "forged derived oracle",
        )
        .expect("forged derived input");
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-file-derived-input-v17\0");
        hasher.update(encoded);
        head.derived_input_sha256 = hasher.finalize().into();
        let forged_head = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[forged_body.root],
                &head.encode().expect("forged head encoding"),
            )
            .expect("forged head");
        let mut forged_root = built.root.clone();
        forged_root.source_heads = store
            .map_apply(
                &lease,
                &built.root.source_heads,
                std::iter::once(Ok(MapEdit {
                    key: digest.to_vec(),
                    value: Some(forged_head.clone()),
                })),
            )
            .expect("forged source map");
        forged_root.memberships = store
            .map_apply(
                &lease,
                &built.root.memberships,
                std::iter::once(Ok(MapEdit {
                    key: membership_key(digest, head.logical_id).to_vec(),
                    value: Some(forged_head),
                })),
            )
            .expect("forged membership map");
        assert!(matches!(
            verify_for_seal(&store, &forged_root, None, &reservation, &fixed),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }
}
