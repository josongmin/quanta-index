//! Fallible selected-leaf Native reader. Ordinary open does no corpus walk.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::{
    DocId as TrigramDocId, MAX_CANDIDATE_PRE_VERIFY, NecessaryGramCandidates, NecessaryGramExpr,
    TrigramError, TrigramErrorCode, TrigramPostingSource, evaluate_necessary_gram_expr,
};
use sha2::{Digest as _, Sha256};

use crate::lexical_store::{BlobRef, MapRoot, ObjectKind, ObjectStore};
use crate::query_errors::map_trigram_error;
use crate::text_authority::paged_manifest::NativeRoot;
use crate::text_authority::paged_posting_page::PostingPage;
use crate::text_authority::paged_segment::{
    GRAM_FOLDED, GRAM_RAW, TERM_FOLDED, TERM_RAW, decode_body, decode_posting,
};
use crate::text_authority::paged_wire::{
    CurrentRecord, MAX_BODY_BYTES, MAX_POSTING_BYTES, SegmentDescriptor, current_key, gram_prefix,
    prefix_upper, route_key, segment_key, term_prefix,
};

const MAX_CURRENT_RECORD_BYTES: u64 = 32 * 1024;
const MAX_SEGMENT_BYTES: u64 = 64 * 1024;
const MAX_SELECTED_POSTING_READS: u64 = 1_000_000;
const MAX_SELECTED_PHYSICAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SELECTED_PHYSICAL_READS: u64 = 1_000_000;
const MAX_QUERY_SEGMENT_CACHE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Default)]
struct PostingReadLedger {
    bytes: Cell<u64>,
    reads: Cell<u64>,
}

#[derive(Clone)]
pub(crate) struct NativeAuthority {
    store: ObjectStore,
    root: NativeRoot,
    segment_cache: Option<Arc<Mutex<SegmentCache>>>,
}

#[derive(Default)]
struct SegmentCache {
    bytes: u64,
    descriptors: BTreeMap<u64, Arc<SegmentDescriptor>>,
}

pub(crate) struct NativeDocument {
    pub(crate) indexed_text: String,
    pub(crate) folded_text: String,
}

impl NativeAuthority {
    pub(crate) fn open(store: ObjectStore, root: NativeRoot) -> Result<Self, CoreError> {
        root.validate()?;
        Ok(Self {
            store,
            root,
            segment_cache: None,
        })
    }

    pub(crate) fn heap_bytes_estimate(&self) -> u64 {
        // Root descriptors are fixed-size controls. No corpus inventory is
        // materialized at open; selected reads own request scratch.
        2048
    }

    /// One request owns this meter across all leaves and selected rows.
    pub(crate) fn scoped_query(&self) -> Self {
        Self {
            store: self.store.metered_clone(),
            root: self.root.clone(),
            segment_cache: Some(Arc::new(Mutex::new(SegmentCache::default()))),
        }
    }

    fn check_query_io(&self) -> Result<(), CoreError> {
        self.check_query_io_against(MAX_SELECTED_PHYSICAL_BYTES, MAX_SELECTED_PHYSICAL_READS)
    }

    fn check_query_io_against(&self, max_bytes: u64, max_reads: u64) -> Result<(), CoreError> {
        let (bytes, reads) = self.store.read_meter_snapshot();
        if bytes > max_bytes || reads > max_reads {
            return Err(physical_io_cap());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn authenticated_io_for_test(&self) -> (u64, u64) {
        self.store.read_meter_snapshot()
    }

    #[cfg(test)]
    pub(crate) fn check_query_io_for_test(
        &self,
        max_bytes: u64,
        max_reads: u64,
    ) -> Result<(), CoreError> {
        self.check_query_io_against(max_bytes, max_reads)
    }

    pub(crate) fn current_record(
        &self,
        id: u64,
        budget: &RequestBudgetV1,
    ) -> Result<Option<CurrentRecord>, CoreError> {
        let reference = self
            .store
            .map_get(&self.root.current, &current_key(id), Some(budget))?;
        self.check_query_io()?;
        let Some(reference) = reference else {
            return Ok(None);
        };
        let record = self.decode_current(&reference, Some(budget))?;
        self.check_query_io()?;
        if record.id != id {
            return Err(corrupt("current key/value ID differs"));
        }
        Ok(Some(record))
    }

    fn decode_current(
        &self,
        reference: &BlobRef,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<CurrentRecord, CoreError> {
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("current value kind differs"));
        }
        let bytes = self
            .store
            .read_blob(reference, MAX_CURRENT_RECORD_BYTES, budget)?;
        CurrentRecord::decode(&bytes).map_err(|error| corrupt(&format!("current row: {error}")))
    }

    pub(crate) fn matches_document(
        &self,
        id: u64,
        candidate_id: &str,
        indexed: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        let Some(row) = self.current_record(id, budget)? else {
            return Ok(false);
        };
        let indexed_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
        Ok(row.candidate_id == candidate_id && row.indexed_sha256 == indexed_sha256)
    }

    pub(crate) fn read_document(
        &self,
        id: u64,
        budget: &RequestBudgetV1,
    ) -> Result<Option<NativeDocument>, CoreError> {
        let Some(record) = self.current_record(id, budget)? else {
            return Ok(None);
        };
        let segment = self.read_segment(record.segment_key, budget)?;
        let local =
            usize::try_from(record.local_id).map_err(|_error| corrupt("local ID width differs"))?;
        if segment.logical_ids.get(local) != Some(&id)
            || segment.live_locals.binary_search(&record.local_id).is_err()
        {
            return Err(corrupt("current locator differs from live segment"));
        }
        if record.body.root.kind != ObjectKind::NativeBody {
            return Err(corrupt("body object kind differs"));
        }
        let bytes = self
            .store
            .read_blob(&record.body, MAX_BODY_BYTES, Some(budget))?;
        self.check_query_io()?;
        let (candidate_id, indexed_text, folded_text) =
            decode_body(&bytes).map_err(|error| corrupt(&format!("body: {error}")))?;
        let indexed_sha256: [u8; 32] = Sha256::digest(indexed_text.as_bytes()).into();
        if candidate_id != record.candidate_id || indexed_sha256 != record.indexed_sha256 {
            return Err(corrupt(
                "body identity differs from committed current record",
            ));
        }
        Ok(Some(NativeDocument {
            indexed_text,
            folded_text,
        }))
    }

    fn read_segment(
        &self,
        id: u64,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<SegmentDescriptor>, CoreError> {
        budget.checkpoint("lexical:native:segment")?;
        if let Some(cache) = &self.segment_cache {
            let locked = cache.lock().map_err(|error| {
                CoreError::Storage(format!("lexical: Native segment cache poisoned: {error}"))
            })?;
            if let Some(descriptor) = locked.descriptors.get(&id) {
                return Ok(Arc::clone(descriptor));
            }
        }
        let reference = self
            .store
            .map_get(&self.root.segments, &segment_key(id), Some(budget))?
            .ok_or_else(|| corrupt("referenced segment is absent"))?;
        self.check_query_io()?;
        if reference.root.kind != ObjectKind::Segment {
            return Err(corrupt("segment object kind differs"));
        }
        let descriptor_bytes = self
            .store
            .read_blob(&reference, MAX_SEGMENT_BYTES, Some(budget))?;
        self.check_query_io()?;
        let descriptor = SegmentDescriptor::decode(&descriptor_bytes)
            .map_err(|error| corrupt(&format!("segment row: {error}")))?;
        if descriptor.id != id {
            return Err(corrupt("segment key/value ID differs"));
        }
        let descriptor = Arc::new(descriptor);
        if let Some(cache) = &self.segment_cache {
            let mut locked = cache.lock().map_err(|error| {
                CoreError::Storage(format!("lexical: Native segment cache poisoned: {error}"))
            })?;
            if !locked.descriptors.contains_key(&id) {
                let decoded_bytes = descriptor
                    .logical_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u64>())
                    .saturating_add(
                        descriptor
                            .live_locals
                            .capacity()
                            .saturating_mul(std::mem::size_of::<u32>()),
                    )
                    .saturating_add(std::mem::size_of::<SegmentDescriptor>())
                    .saturating_add(std::mem::size_of::<(u64, Arc<SegmentDescriptor>)>())
                    // An ordered-map entry owns a node allocation in addition
                    // to its key and Arc. Keep the request cap conservative.
                    .saturating_add(256);
                let resident_bytes = u64::try_from(decoded_bytes).map_err(|error| {
                    CoreError::Storage(format!("lexical: Native segment size: {error}"))
                })?;
                if locked.bytes.saturating_add(resident_bytes) <= MAX_QUERY_SEGMENT_CACHE_BYTES {
                    locked.bytes = locked.bytes.saturating_add(resident_bytes);
                    let _previous = locked.descriptors.insert(id, Arc::clone(&descriptor));
                }
            }
        }
        Ok(descriptor)
    }

    pub(crate) fn all_live_ids(&self, budget: &RequestBudgetV1) -> Result<Vec<u64>, CoreError> {
        let mut ids = BTreeSet::new();
        self.store.map_scan(
            &self.root.segments,
            &[0],
            None,
            Some(budget),
            &mut |key, reference| {
                budget.checkpoint("lexical:native:all-live")?;
                self.check_query_io()?;
                if key.len() != 8 || reference.root.kind != ObjectKind::Segment {
                    return Err(corrupt("segment map key or value kind differs"));
                }
                let id = u64::from_be_bytes(
                    key.try_into()
                        .map_err(|_error| corrupt("segment ID width differs"))?,
                );
                let descriptor = self.read_segment(id, budget)?;
                for &local in &descriptor.live_locals {
                    let logical = *descriptor
                        .logical_ids
                        .get(
                            usize::try_from(local)
                                .map_err(|_error| corrupt("segment local width differs"))?,
                        )
                        .ok_or_else(|| corrupt("segment live local is outside logical table"))?;
                    if !ids.insert(logical) {
                        return Err(corrupt("duplicate live Native ID"));
                    }
                    if ids.len() > MAX_CANDIDATE_PRE_VERIFY {
                        return Err(candidate_cap());
                    }
                }
                Ok(())
            },
        )?;
        self.check_query_io()?;
        Ok(ids.into_iter().collect())
    }

    pub(crate) fn intersect_grams(
        &self,
        grams: &[[u8; 3]],
        folded: bool,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<u64>, CoreError> {
        self.intersect_grams_with_ledger(grams, folded, budget, &PostingReadLedger::default())
    }

    fn intersect_grams_with_ledger(
        &self,
        grams: &[[u8; 3]],
        folded: bool,
        budget: &RequestBudgetV1,
        ledger: &PostingReadLedger,
    ) -> Result<Vec<u64>, CoreError> {
        if grams.is_empty() {
            return Ok(Vec::new());
        }
        let surface = if folded { GRAM_FOLDED } else { GRAM_RAW };
        let prefixes = grams
            .iter()
            .map(|gram| gram_prefix(surface, *gram).to_vec())
            .collect::<Vec<_>>();
        self.intersect_routes(
            &self.root.gram_routes,
            ObjectKind::NativeTrigrams,
            surface,
            &prefixes,
            budget,
            ledger,
        )
    }

    pub(crate) fn intersect_terms(
        &self,
        terms: &[&str],
        case_sensitive: bool,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<u64>, CoreError> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let surface = if case_sensitive {
            TERM_RAW
        } else {
            TERM_FOLDED
        };
        let prefixes = terms
            .iter()
            .map(|term| term_prefix(surface, term))
            .collect::<Result<Vec<_>, _>>()?;
        self.intersect_routes(
            &self.root.term_routes,
            ObjectKind::NativePositions,
            surface,
            &prefixes,
            budget,
            &PostingReadLedger::default(),
        )
    }

    fn intersect_routes(
        &self,
        map: &MapRoot,
        kind: ObjectKind,
        surface: u8,
        prefixes: &[Vec<u8>],
        budget: &RequestBudgetV1,
        ledger: &PostingReadLedger,
    ) -> Result<Vec<u64>, CoreError> {
        let mut result = BTreeSet::new();
        let first = prefixes
            .first()
            .ok_or_else(|| corrupt("empty route intersection"))?;
        let upper = prefix_upper(first);
        self.store.map_scan(
            map,
            first,
            upper.as_deref(),
            Some(budget),
            &mut |key, reference| {
                budget.checkpoint("lexical:native:posting-route")?;
                self.check_query_io()?;
                if key.len() != first.len().saturating_add(8) || !key.starts_with(first) {
                    return Err(corrupt("posting route key differs"));
                }
                let segment_key = u64::from_be_bytes(
                    key.get(first.len()..)
                        .ok_or_else(|| corrupt("posting route segment suffix missing"))?
                        .try_into()
                        .map_err(|error| {
                            corrupt(&format!("posting route segment width: {error}"))
                        })?,
                );
                let mut refs = Vec::new();
                refs.try_reserve(prefixes.len()).map_err(|error| {
                    CoreError::Storage(format!("lexical: reserve Native posting refs: {error}"))
                })?;
                refs.push(reference.clone());
                for prefix in prefixes.iter().skip(1) {
                    let selected =
                        self.store
                            .map_get(map, &route_key(prefix, segment_key), Some(budget))?;
                    self.check_query_io()?;
                    let Some(reference) = selected else {
                        return Ok(());
                    };
                    refs.push(reference);
                }
                let descriptor = self.read_segment(segment_key, budget)?;
                let mut local_match: Option<BTreeSet<u32>> = None;
                for (reference, prefix) in refs.iter().zip(prefixes) {
                    if reference.root.kind != kind {
                        return Err(corrupt("posting object kind differs"));
                    }
                    let next_reads = ledger
                        .reads
                        .get()
                        .checked_add(1)
                        .ok_or_else(posting_work_cap)?;
                    if next_reads > MAX_SELECTED_POSTING_READS {
                        return Err(posting_work_cap());
                    }
                    ledger.reads.set(next_reads);
                    let next_bytes = ledger
                        .bytes
                        .get()
                        .checked_add(reference.bytes)
                        .ok_or_else(posting_bytes_cap)?;
                    if next_bytes > MAX_POSTING_BYTES {
                        return Err(posting_bytes_cap());
                    }
                    ledger.bytes.set(next_bytes);
                    let bytes = self
                        .store
                        .read_blob(reference, MAX_POSTING_BYTES, Some(budget))?;
                    self.check_query_io()?;
                    let page = PostingPage::decode(&bytes, segment_key)?;
                    let posting_key = if matches!(surface, GRAM_RAW | GRAM_FOLDED) {
                        prefix
                            .get(1..4)
                            .ok_or_else(|| corrupt("gram prefix width differs"))?
                    } else {
                        prefix
                            .get(3..)
                            .ok_or_else(|| corrupt("term prefix width differs"))?
                    };
                    let posting = page
                        .posting(surface, posting_key)
                        .ok_or_else(|| corrupt("selected posting absent from page"))?;
                    let locals =
                        decode_posting(posting, segment_key, surface, descriptor.logical_ids.len())
                            .map_err(|error| corrupt(&format!("posting row: {error}")))?;
                    let local_set: BTreeSet<u32> = locals.into_iter().collect();
                    local_match = Some(match local_match {
                        Some(prior) => prior.intersection(&local_set).copied().collect(),
                        None => local_set,
                    });
                    if local_match.as_ref().is_some_and(BTreeSet::is_empty) {
                        break;
                    }
                }
                for local in local_match.into_iter().flatten() {
                    if descriptor.live_locals.binary_search(&local).is_err() {
                        continue;
                    }
                    let id = *descriptor
                        .logical_ids
                        .get(
                            usize::try_from(local)
                                .map_err(|_error| corrupt("posting local width differs"))?,
                        )
                        .ok_or_else(|| corrupt("posting local outside segment"))?;
                    if !result.insert(id) {
                        return Err(corrupt("duplicate Native result ID"));
                    }
                    if result.len() > MAX_CANDIDATE_PRE_VERIFY {
                        return Err(candidate_cap());
                    }
                }
                Ok(())
            },
        )?;
        self.check_query_io()?;
        Ok(result.into_iter().collect())
    }

    /// Reuse the canonical regex necessary-condition evaluator. Storage and
    /// cancellation failures are preserved as `CoreError`, not weakened to
    /// `AllDocs` or an empty posting.
    pub(crate) fn regex_candidates(
        &self,
        expr: &NecessaryGramExpr,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<u64>, CoreError> {
        let source = PagedGramSource {
            authority: self,
            budget,
            failure: RefCell::new(None),
            ledger: PostingReadLedger::default(),
        };
        let result = evaluate_necessary_gram_expr(&source, expr);
        if let Some(error) = source.failure.into_inner() {
            return Err(error);
        }
        match result.map_err(|error| map_trigram_error("Native regex prefilter", &error))? {
            NecessaryGramCandidates::AllDocs => self.all_live_ids(budget),
            NecessaryGramCandidates::Docs(ids) => Ok(ids.into_iter().map(|id| id.0).collect()),
        }
    }
}

struct PagedGramSource<'a> {
    authority: &'a NativeAuthority,
    budget: &'a RequestBudgetV1,
    failure: RefCell<Option<CoreError>>,
    ledger: PostingReadLedger,
}

impl TrigramPostingSource for PagedGramSource<'_> {
    fn intersect_trigrams(&self, grams: &[[u8; 3]]) -> Result<Vec<TrigramDocId>, TrigramError> {
        match self
            .authority
            .intersect_grams_with_ledger(grams, false, self.budget, &self.ledger)
        {
            Ok(ids) => Ok(ids.into_iter().map(TrigramDocId).collect()),
            Err(error) => {
                *self.failure.borrow_mut() = Some(error);
                Err(TrigramError::new(
                    TrigramErrorCode::IndexCorrupted,
                    "Native posting read failed",
                ))
            }
        }
    }
}

fn posting_bytes_cap() -> CoreError {
    map_trigram_error(
        "Native selected postings",
        &TrigramError::plan_limit(
            quanta_index_lq_trigram::LimitDimension::InputBytes,
            format!("Native selected posting bytes exceed {MAX_POSTING_BYTES}"),
        ),
    )
}

fn posting_work_cap() -> CoreError {
    map_trigram_error(
        "Native selected postings",
        &TrigramError::plan_limit(
            quanta_index_lq_trigram::LimitDimension::CandidateSet,
            format!("Native selected posting reads exceed {MAX_SELECTED_POSTING_READS}"),
        ),
    )
}

fn physical_io_cap() -> CoreError {
    map_trigram_error(
        "Native authenticated query I/O",
        &TrigramError::plan_limit(
            quanta_index_lq_trigram::LimitDimension::InputBytes,
            format!(
                "Native selected reads exceed {MAX_SELECTED_PHYSICAL_BYTES} physical bytes or {MAX_SELECTED_PHYSICAL_READS} objects"
            ),
        ),
    )
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!("lexical: Native authority: {reason}"),
    }
}

fn candidate_cap() -> CoreError {
    map_trigram_error(
        "Native candidate set",
        &TrigramError::plan_limit(
            quanta_index_lq_trigram::LimitDimension::CandidateSet,
            format!("Native candidate set exceeds {MAX_CANDIDATE_PRE_VERIFY}"),
        ),
    )
}
