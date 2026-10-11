//! Independent V17 changed-segment oracle. It derives exact gram membership
//! from authenticated source bodies; the posting producer is never called.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::SourceFileKey;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::trigrams_of;
use sha2::{Digest as _, Sha256};

use crate::lexical_allocation::DocVersionReservation;
use crate::lexical_store::{BlobRef, MapCursor, MapRoot, ObjectKind, ObjectRef, ObjectStore};

use super::producer::{FileAuthorityMutation, FileSourceEdit};
use super::{
    AuthorityRoot, LiveDoc, PostingPage, Segment, SourceHead, doc_key, membership_key, route_key,
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

fn route_parts(key: &[u8], surface: u8) -> Result<([u8; 3], u32, [u8; 32]), CoreError> {
    if key.len() != 40 || key.first() != Some(&surface) {
        return Err(corrupt("route key shape differs"));
    }
    let gram = key
        .get(1..4)
        .ok_or_else(|| corrupt("route gram absent"))?
        .try_into()
        .map_err(|_width_error| corrupt("route gram width differs"))?;
    let count_bytes: [u8; 4] = key
        .get(4..8)
        .ok_or_else(|| corrupt("route count absent"))?
        .try_into()
        .map_err(|_width_error| corrupt("route count width differs"))?;
    let digest = key
        .get(8..)
        .ok_or_else(|| corrupt("route segment absent"))?
        .try_into()
        .map_err(|_width_error| corrupt("route segment width differs"))?;
    Ok((gram, u32::from_be_bytes(count_bytes), digest))
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

#[derive(Debug, Eq, PartialEq)]
struct RouteCommitment {
    count: u64,
    sha256: [u8; 32],
}

struct RouteHasher {
    hasher: Sha256,
    count: u64,
}

struct RouteWitness {
    reference: BlobRef,
    expected: [RouteCommitment; 2],
    actual: [RouteHasher; 2],
}

impl RouteWitness {
    fn new(reference: BlobRef, expected: [RouteCommitment; 2]) -> Self {
        let digest = reference.root.sha256;
        Self {
            reference,
            expected,
            actual: [RouteHasher::new(1, digest), RouteHasher::new(2, digest)],
        }
    }

    fn record(&mut self, surface: u8, key: &[u8], reference: &BlobRef) -> Result<(), CoreError> {
        if *reference != self.reference {
            return Err(corrupt("route value differs from segment descriptor"));
        }
        let index = usize::from(surface)
            .checked_sub(1)
            .ok_or_else(|| corrupt("route surface underflows"))?;
        self.actual
            .get_mut(index)
            .ok_or_else(|| corrupt("route surface exceeds two surfaces"))?
            .push(key)
    }

    fn verify(self) -> Result<(), CoreError> {
        for (expected, actual) in self.expected.into_iter().zip(self.actual) {
            if expected != actual.finish() {
                return Err(corrupt("segment route inventory differs from term pages"));
            }
        }
        Ok(())
    }
}

impl RouteHasher {
    fn new(surface: u8, segment: [u8; 32]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-file-route-stream-v17\0");
        hasher.update([surface]);
        hasher.update(segment);
        Self { hasher, count: 0 }
    }

    fn push(&mut self, key: &[u8]) -> Result<(), CoreError> {
        if key.len() != 40 {
            return Err(corrupt("route commitment key width differs"));
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| corrupt("route commitment count overflows"))?;
        self.hasher.update(key);
        Ok(())
    }

    fn finish(self) -> RouteCommitment {
        RouteCommitment {
            count: self.count,
            sha256: self.hasher.finalize().into(),
        }
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
    digest: [u8; 32],
    surface: u8,
    terms: &MapRoot,
    expected_sources: &[Vec<[u8; 3]>],
    doc_count: usize,
) -> Result<RouteCommitment, CoreError> {
    let mut stream = TermPageStream::new(store, terms, doc_count)?;
    let mut expected = ExpectedTerms::new(expected_sources)?;
    let mut routes = RouteHasher::new(surface, digest);
    while let Some((gram, page_mask)) = stream.next_term()? {
        let (expected_gram, expected_mask) = expected
            .next_term()?
            .ok_or_else(|| corrupt("term map contains an extra gram"))?;
        if gram != expected_gram || page_mask != expected_mask {
            return Err(corrupt("posting IDs differ from source-body gram oracle"));
        }
        routes.push(&route_key(surface, gram, page_mask.count_ones(), digest))?;
    }
    if expected.next_term()?.is_some() {
        return Err(corrupt("term map omits a source-body gram"));
    }
    Ok(routes.finish())
}

fn inherited_route_commitment(
    store: &ObjectStore,
    terms: &MapRoot,
    doc_count: usize,
    surface: u8,
    digest: [u8; 32],
) -> Result<RouteCommitment, CoreError> {
    let mut stream = TermPageStream::new(store, terms, doc_count)?;
    let mut routes = RouteHasher::new(surface, digest);
    while let Some((gram, mask)) = stream.next_term()? {
        routes.push(&route_key(surface, gram, mask.count_ones(), digest))?;
    }
    Ok(routes.finish())
}

/// Verify one newly produced or compacted segment against its current sources.
///
/// The outer sealed-generation owner authenticates the base. This function
/// independently checks every logical membership of the selected segment.
fn verify_segment(
    store: &ObjectStore,
    root: &AuthorityRoot,
    reference: &BlobRef,
) -> Result<[RouteCommitment; 2], CoreError> {
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
    let path_routes = verify_term_map(
        store,
        digest,
        1,
        &segment.path_terms,
        &expected_path,
        segment.doc_ids.len(),
    )?;
    let content_routes = verify_term_map(
        store,
        digest,
        2,
        &segment.content_terms,
        &expected_content,
        segment.doc_ids.len(),
    )?;
    Ok([path_routes, content_routes])
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

fn verify_live_term_map(
    store: &ObjectStore,
    root: &AuthorityRoot,
    segment_ref: &BlobRef,
    terms: &MapRoot,
    surface: u8,
    active: &[bool],
    expected_sources: &[Vec<[u8; 3]>],
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    let routes = if surface == 1 {
        &root.path_routes
    } else {
        &root.content_routes
    };
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
        if store
            .map_get(
                routes,
                &route_key(
                    surface,
                    gram,
                    page_mask.count_ones(),
                    segment_ref.root.sha256,
                ),
                budget,
            )?
            .as_ref()
            != Some(segment_ref)
        {
            return Err(corrupt("scrub segment term route is missing or different"));
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

// Scrub may select one route independently of a segment-wide seal proof.
fn posting_count_for_term(
    store: &ObjectStore,
    terms: &MapRoot,
    gram: [u8; 3],
    doc_count: usize,
    budget: Option<&RequestBudgetV1>,
) -> Result<u32, CoreError> {
    let page_ref = store
        .map_get(terms, &gram, budget)?
        .ok_or_else(|| corrupt("route gram absent from segment"))?;
    if page_ref.root.kind != ObjectKind::FilePostings {
        return Err(corrupt("route posting page kind differs"));
    }
    let page = PostingPage::decode(
        &store.read_blob(&page_ref, size_u64(super::MAX_POSTING_PAGE_BYTES)?, budget)?,
        doc_count,
    )?;
    let index = page
        .0
        .binary_search_by_key(&gram, |row| row.0)
        .map_err(|_conversion_error| corrupt("route gram absent from posting page"))?;
    Ok(page
        .0
        .get(index)
        .ok_or_else(|| corrupt("route posting index disappeared"))?
        .1
        .count_ones())
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
        &active,
        &expected_path,
        budget,
    )?;
    verify_live_term_map(
        store,
        root,
        reference,
        &segment.content_terms,
        2,
        &active,
        &expected_content,
        budget,
    )?;
    Ok(active_count)
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
    Done,
}

/// In-process progress across ordered map rows; no corpus-sized source census.
#[derive(Clone, Debug, Default)]
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
        let map = match cursor.phase {
            FileScrubPhase::Source => &root.source_heads,
            FileScrubPhase::Current => &root.current_docs,
            FileScrubPhase::Membership => &root.memberships,
            FileScrubPhase::Segment => &root.segments,
            FileScrubPhase::PathRoute => &root.path_routes,
            FileScrubPhase::ContentRoute => &root.content_routes,
            FileScrubPhase::Debt => &root.compaction_debt,
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
                    let (gram, count, digest) = route_parts(&key, surface)?;
                    if reference.root.sha256 != digest
                        || reference.root.kind != ObjectKind::Segment
                        || store.map_get(&root.segments, &digest, budget)?.as_ref()
                            != Some(&reference)
                    {
                        return Err(corrupt("scrub route references unknown segment"));
                    }
                    let segment = Segment::decode(&store.read_blob(
                        &reference,
                        size_u64(super::MAX_RECORD_BYTES)?,
                        budget,
                    )?)?;
                    let terms = if surface == 1 {
                        &segment.path_terms
                    } else {
                        &segment.content_terms
                    };
                    if posting_count_for_term(store, terms, gram, segment.doc_ids.len(), budget)?
                        != count
                    {
                        return Err(corrupt("scrub route count differs from posting page"));
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
            FileScrubPhase::Debt => cursor.phase = FileScrubPhase::Done,
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

pub(crate) fn verify_for_seal(
    store: &ObjectStore,
    root: &AuthorityRoot,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
) -> Result<(), CoreError> {
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
            Ok(())
        },
    )?;
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
    let mut removed_routes = BTreeMap::<[u8; 32], RouteWitness>::new();
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
                "touched segment was not retired in changed publication",
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
            let expected = [
                inherited_route_commitment(
                    store,
                    &segment.path_terms,
                    segment.doc_ids.len(),
                    1,
                    digest,
                )?,
                inherited_route_commitment(
                    store,
                    &segment.content_terms,
                    segment.doc_ids.len(),
                    2,
                    digest,
                )?,
            ];
            if removed_routes
                .insert(digest, RouteWitness::new(reference, expected))
                .is_some()
            {
                return Err(corrupt("removed segment route witness repeats"));
            }
        }
        if removed {
            let _inserted = removed_segments.insert(digest);
        }
    }
    let mut created_routes = BTreeMap::<[u8; 32], RouteWitness>::new();
    for digest in created_segments {
        if removed_segments.contains(&digest) {
            return Err(corrupt("segment is both created and removed"));
        }
        let reference = store
            .map_get(&root.segments, &digest, None)?
            .ok_or_else(|| corrupt("new current locator segment absent"))?;
        let expected = verify_segment(store, root, &reference)?;
        if created_routes
            .insert(digest, RouteWitness::new(reference, expected))
            .is_some()
        {
            return Err(corrupt("created segment route witness repeats"));
        }
    }
    // The authenticated map diff is ordered by full route key. Its projection
    // onto one segment and surface is therefore ordered by gram and count.
    // Fixed-width key commitments compare the complete expected and observed
    // inventories without retaining one route row per gram.
    for (surface, old_routes, new_routes) in [
        (1_u8, &old.path_routes, &root.path_routes),
        (2_u8, &old.content_routes, &root.content_routes),
    ] {
        store.map_changed_keys(old_routes, new_routes, None, &mut |key, before, after| {
            let (_, _, digest) = route_parts(key, surface)?;
            if let Some(reference) = before {
                removed_routes
                    .get_mut(&digest)
                    .ok_or_else(|| corrupt("route removed outside compacted segment"))?
                    .record(surface, key, reference)?;
            }
            if let Some(reference) = after {
                created_routes
                    .get_mut(&digest)
                    .ok_or_else(|| corrupt("route added for unrelated segment"))?
                    .record(surface, key, reference)?;
            }
            Ok(())
        })?;
    }
    for (_, witness) in removed_routes.into_iter().chain(created_routes) {
        witness.verify()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_authority::paged::{
        FileAuthorityMutation, FileSourceEdit, PagedFileAuthority, build_for_seal,
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

        let (correct_key, segment_ref) = store
            .map_cursor(&built.root.content_routes, &[], None, None)
            .expect("content route cursor")
            .next()
            .expect("content route read")
            .expect("content route exists");
        let (_, count, digest) = route_parts(&correct_key, 2).expect("route shape");
        let segment = Segment::decode(
            &store
                .read_blob(
                    &segment_ref,
                    size_u64(super::super::MAX_RECORD_BYTES).expect("record bound"),
                    None,
                )
                .expect("segment bytes"),
        )
        .expect("segment record");
        let source_grams = vec![derive_grams(b"abcabc").expect("fixed source grams")];
        let source_meter = store.metered_clone();
        let source_commitment = verify_term_map(
            &source_meter,
            digest,
            2,
            &segment.content_terms,
            &source_grams,
            1,
        )
        .expect("source-backed routes");
        assert_eq!(source_commitment.count, 3);
        assert_eq!(source_meter.read_meter_snapshot().1, 2);
        let inherited_meter = store.metered_clone();
        let inherited_commitment =
            inherited_route_commitment(&inherited_meter, &segment.content_terms, 1, 2, digest)
                .expect("inherited routes");
        assert_eq!(inherited_commitment, source_commitment);
        assert_eq!(inherited_meter.read_meter_snapshot().1, 2);
        let source_digest = source_key_digest(initial.edits.first().expect("one edit").key())
            .expect("source digest");
        let (head_ref, _) =
            source_head(&store, &built.root, source_digest, None).expect("source head");
        let extra_key = route_key(2, [0, 0, 0], 1, digest);
        let wrong_count_key = route_key(
            2,
            correct_key
                .get(1..4)
                .expect("gram bytes")
                .try_into()
                .expect("gram width"),
            count.checked_add(1).expect("count room"),
            digest,
        );
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
                    value: Some(segment_ref.clone()),
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
                        value: Some(segment_ref),
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
