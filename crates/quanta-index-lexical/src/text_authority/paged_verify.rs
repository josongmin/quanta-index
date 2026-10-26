//! Seal oracle for newly written Native segments, independent of posting builders.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_positions::MAX_POSITIONS_PER_CELL;
use sha2::{Digest as _, Sha256};

use crate::lexical_runs::RunScrubCursor;
use crate::lexical_store::{BlobCursor, BlobRef, ObjectKind, ObjectRef, ObjectStore};
use crate::normalize::{self, CaseMode};
use crate::text_authority::paged_manifest::NativeRoot;
use crate::text_authority::paged_merge_proof::NativeMergeProof;
use crate::text_authority::paged_posting_page::{PostingPage, route_posting_key};
use crate::text_authority::paged_segment::{
    GRAM_FOLDED, GRAM_RAW, TERM_FOLDED, TERM_RAW, decode_body,
};
use crate::text_authority::paged_wire::{
    CurrentRecord, MAX_BODY_BYTES, MAX_POSTING_BYTES, SegmentDescriptor, current_key,
    decode_source, gram_prefix, membership_key, prefix_upper, route_key, source_digest,
    term_prefix,
};
use crate::text_authority::paged_writer::{
    read_current_with_ref, read_route_inventory, read_route_inventory_budget, read_segment,
};

type TermDocs = BTreeMap<String, Vec<(u32, Vec<u32>)>>;
type PageRoutes = BTreeMap<ObjectRef, (BlobRef, BTreeSet<(u8, Vec<u8>)>)>;

/// In-process semantic scrub progress. A restart begins a new pass; no
/// partially checked state is persisted as a proof.
pub(crate) struct NativeScrubCursor {
    root: NativeRoot,
    phase: NativeScrubPhase,
    segment_after: Option<[u8; 8]>,
    source_after: Option<[u8; 32]>,
    segment: Option<ScrubSegment>,
    pending: Option<PendingBlob>,
    run_cursor: RunScrubCursor,
    live_count: u64,
    gram_routes: u64,
    term_routes: u64,
    due_count: u64,
    invalidated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeScrubPhase {
    Segment,
    Document,
    Inventory,
    Route,
    Page,
    Source,
    RunRegistry,
    Done,
}

struct ScrubSegment {
    descriptor: SegmentDescriptor,
    next_document: usize,
    raw_grams: BTreeMap<[u8; 3], Vec<u32>>,
    folded_grams: BTreeMap<[u8; 3], Vec<u32>>,
    raw_terms: TermDocs,
    folded_terms: TermDocs,
    routes: Vec<Vec<u8>>,
    next_route: usize,
    pages: PageRoutes,
}

struct PendingBlob {
    cursor: BlobCursor,
    bytes: Vec<u8>,
    purpose: PendingPurpose,
}

enum PendingPurpose {
    Body { local: u32, record: CurrentRecord },
    Inventory,
    Page { keys: BTreeSet<(u8, Vec<u8>)> },
}

pub(crate) struct NativeScrubStep {
    pub(crate) done: bool,
    pub(crate) charged_bytes: u64,
    pub(crate) read_count: u64,
}

impl NativeScrubCursor {
    pub(crate) fn new(root: &NativeRoot) -> Result<Self, CoreError> {
        root.validate()?;
        Ok(Self {
            root: root.clone(),
            phase: NativeScrubPhase::Segment,
            segment_after: None,
            source_after: None,
            segment: None,
            pending: None,
            run_cursor: RunScrubCursor::new(&root.run_registry)?,
            live_count: 0,
            gram_routes: 0,
            term_routes: 0,
            due_count: 0,
            invalidated: false,
        })
    }

    /// Advance through at most the caller's byte allowance, except for one
    /// indivisible authenticated map/record read, one 32 KiB blob block, or
    /// one bounded run-registry row with its membership witnesses.
    pub(crate) fn step(
        &mut self,
        store: &ObjectStore,
        max_bytes: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<NativeScrubStep, CoreError> {
        if max_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "lexical: Native scrub byte allowance is zero".into(),
            ));
        }
        if self.invalidated {
            return Err(CoreError::InvalidContract(
                "lexical: Native scrub cursor must restart after a failed step".into(),
            ));
        }
        let before = store.read_meter_snapshot();
        for _ in 0..4096 {
            if let Err(error) = checkpoint(budget, "lexical:native:scrub-step") {
                self.invalidated = true;
                return Err(error);
            }
            if self.phase == NativeScrubPhase::Done {
                break;
            }
            if let Err(error) = self.advance_one(store, budget) {
                self.invalidated = true;
                return Err(error);
            }
            let now = store.read_meter_snapshot();
            if now
                .0
                .checked_sub(before.0)
                .ok_or_else(|| corrupt("scrub meter bytes regressed"))?
                >= max_bytes
            {
                break;
            }
        }
        let after = store.read_meter_snapshot();
        Ok(NativeScrubStep {
            done: self.phase == NativeScrubPhase::Done,
            charged_bytes: after
                .0
                .checked_sub(before.0)
                .ok_or_else(|| corrupt("scrub meter bytes regressed"))?,
            read_count: after
                .1
                .checked_sub(before.1)
                .ok_or_else(|| corrupt("scrub meter objects regressed"))?,
        })
    }

    fn advance_one(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        if let Some(mut pending) = self.pending.take() {
            match pending.cursor.next(store, budget)? {
                Some((chunk, _charged)) => {
                    pending.bytes.try_reserve(chunk.len()).map_err(|error| {
                        CoreError::Storage(format!("lexical: reserve Native scrub blob: {error}"))
                    })?;
                    pending.bytes.extend_from_slice(&chunk);
                    self.pending = Some(pending);
                }
                None => self.finish_blob(pending.purpose, &pending.bytes)?,
            }
            return Ok(());
        }
        match self.phase {
            NativeScrubPhase::Segment => self.open_next_segment(store, budget),
            NativeScrubPhase::Document => self.start_document(store, budget),
            NativeScrubPhase::Inventory => {
                let segment = self
                    .segment
                    .as_ref()
                    .ok_or_else(|| corrupt("scrub segment missing"))?;
                let reference = segment
                    .descriptor
                    .route_inventory
                    .as_ref()
                    .ok_or_else(|| corrupt("scrub route inventory missing"))?;
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("scrub route inventory kind differs"));
                }
                self.pending = Some(PendingBlob {
                    cursor: store.blob_cursor(reference, MAX_POSTING_BYTES)?,
                    bytes: Vec::new(),
                    purpose: PendingPurpose::Inventory,
                });
                Ok(())
            }
            NativeScrubPhase::Route => self.read_next_route(store, budget),
            NativeScrubPhase::Page => self.start_next_page(store),
            NativeScrubPhase::Source => self.read_next_source(store, budget),
            NativeScrubPhase::RunRegistry => {
                if self.run_cursor.step(store, budget)? {
                    self.phase = NativeScrubPhase::Done;
                }
                Ok(())
            }
            NativeScrubPhase::Done => Ok(()),
        }
    }

    fn open_next_segment(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let lower = match self.segment_after {
            None => vec![0],
            Some(key) => {
                let Some(next) = prefix_upper(&key) else {
                    self.phase = NativeScrubPhase::Source;
                    return Ok(());
                };
                next
            }
        };
        let mut cursor = store.map_cursor(&self.root.segments, &lower, None, budget)?;
        let Some((key, reference)) = cursor.next()? else {
            if self.live_count != self.root.live_docs
                || self.gram_routes != self.root.gram_routes.entries
                || self.term_routes != self.root.term_routes.entries
                || self.due_count != self.root.compaction_due.entries
            {
                return Err(corrupt("scrub segment totals differ from root"));
            }
            self.phase = NativeScrubPhase::Source;
            return Ok(());
        };
        let id_key: [u8; 8] = key
            .as_slice()
            .try_into()
            .map_err(|_width_error| corrupt("scrub segment key width differs"))?;
        let id = u64::from_be_bytes(id_key);
        if reference.root.kind != ObjectKind::Segment {
            return Err(corrupt("scrub segment kind differs"));
        }
        let descriptor =
            SegmentDescriptor::decode(&store.read_blob(&reference, 64 * 1024, budget)?)?;
        if descriptor.id != id {
            return Err(corrupt("scrub segment key/value differs"));
        }
        let _stored_index_bytes = self
            .root
            .run_registry
            .verify_segment_owner(store, &key, &reference, budget)?;
        let due = store.map_get(&self.root.compaction_due, &key, budget)?;
        if descriptor.live_locals.len() < descriptor.logical_ids.len() {
            if due.as_ref() != Some(&reference) {
                return Err(corrupt("scrub segment debt differs"));
            }
            self.due_count = self
                .due_count
                .checked_add(1)
                .ok_or_else(|| corrupt("scrub debt count overflows"))?;
        } else if due.is_some() {
            return Err(corrupt("scrub full segment has debt"));
        }
        self.segment_after = Some(id_key);
        self.segment = Some(ScrubSegment {
            descriptor,
            next_document: 0,
            raw_grams: BTreeMap::new(),
            folded_grams: BTreeMap::new(),
            raw_terms: TermDocs::new(),
            folded_terms: TermDocs::new(),
            routes: Vec::new(),
            next_route: 0,
            pages: BTreeMap::new(),
        });
        self.phase = NativeScrubPhase::Document;
        Ok(())
    }

    fn start_document(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let segment = self
            .segment
            .as_ref()
            .ok_or_else(|| corrupt("scrub segment missing"))?;
        let Some(local) = segment
            .descriptor
            .live_locals
            .get(segment.next_document)
            .copied()
        else {
            self.phase = NativeScrubPhase::Inventory;
            return Ok(());
        };
        let id = *segment
            .descriptor
            .logical_ids
            .get(usize::try_from(local).map_err(|_width_error| corrupt("scrub local width"))?)
            .ok_or_else(|| corrupt("scrub local outside logical table"))?;
        let current_ref = store
            .map_get(&self.root.current, &current_key(id), budget)?
            .ok_or_else(|| corrupt("scrub current record missing"))?;
        if current_ref.root.kind != ObjectKind::Record {
            return Err(corrupt("scrub current record kind differs"));
        }
        let record = CurrentRecord::decode(&store.read_blob(&current_ref, 32 * 1024, budget)?)?;
        if record.id != id
            || record.segment_key != segment.descriptor.id
            || record.local_id != local
        {
            return Err(corrupt("scrub current locator differs"));
        }
        let digest = source_digest(&record.source)?;
        if store.map_get(&self.root.membership, &membership_key(&digest, id), budget)?
            != Some(current_ref)
        {
            return Err(corrupt("scrub membership differs"));
        }
        let source_ref = store
            .map_get(&self.root.source, &digest, budget)?
            .ok_or_else(|| corrupt("scrub source binding missing"))?;
        if source_ref.root.kind != ObjectKind::Record {
            return Err(corrupt("scrub source kind differs"));
        }
        let coverage = decode_source(&store.read_blob(&source_ref, 64 * 1024, budget)?)?;
        if coverage.source.file != record.source {
            return Err(corrupt("scrub source identity differs"));
        }
        if record.body.root.kind != ObjectKind::NativeBody {
            return Err(corrupt("scrub body kind differs"));
        }
        self.pending = Some(PendingBlob {
            cursor: store.blob_cursor(&record.body, MAX_BODY_BYTES)?,
            bytes: Vec::new(),
            purpose: PendingPurpose::Body { local, record },
        });
        Ok(())
    }

    fn read_next_route(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let segment = self
            .segment
            .as_mut()
            .ok_or_else(|| corrupt("scrub segment missing"))?;
        let Some(route) = segment.routes.get(segment.next_route) else {
            self.phase = NativeScrubPhase::Page;
            return Ok(());
        };
        let (surface, posting_key) = route_posting_key(route, segment.descriptor.id)?;
        let (map, kind) = match surface {
            GRAM_RAW | GRAM_FOLDED => (&self.root.gram_routes, ObjectKind::NativeTrigrams),
            TERM_RAW | TERM_FOLDED => (&self.root.term_routes, ObjectKind::NativePositions),
            _ => return Err(corrupt("scrub route surface differs")),
        };
        let reference = store
            .map_get(map, route, budget)?
            .ok_or_else(|| corrupt("scrub route missing"))?;
        if reference.root.kind != kind {
            return Err(corrupt("scrub route kind differs"));
        }
        let group = segment
            .pages
            .entry(reference.root.clone())
            .or_insert_with(|| (reference.clone(), BTreeSet::new()));
        if group.0 != reference || !group.1.insert((surface, posting_key.to_vec())) {
            return Err(corrupt("scrub page route repeats or reference differs"));
        }
        segment.next_route = segment.next_route.saturating_add(1);
        Ok(())
    }

    fn start_next_page(&mut self, store: &ObjectStore) -> Result<(), CoreError> {
        let segment = self
            .segment
            .as_mut()
            .ok_or_else(|| corrupt("scrub segment missing"))?;
        let Some((_, (reference, keys))) = segment.pages.pop_first() else {
            if !segment.raw_grams.is_empty()
                || !segment.folded_grams.is_empty()
                || !segment.raw_terms.is_empty()
                || !segment.folded_terms.is_empty()
            {
                return Err(corrupt("scrub body term or gram lacks route"));
            }
            self.segment = None;
            self.phase = NativeScrubPhase::Segment;
            return Ok(());
        };
        self.pending = Some(PendingBlob {
            cursor: store.blob_cursor(&reference, MAX_POSTING_BYTES)?,
            bytes: Vec::new(),
            purpose: PendingPurpose::Page { keys },
        });
        Ok(())
    }

    fn read_next_source(
        &mut self,
        store: &ObjectStore,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        let lower = match self.source_after {
            None => vec![0],
            Some(key) => {
                let Some(next) = prefix_upper(&key) else {
                    self.phase = NativeScrubPhase::RunRegistry;
                    return Ok(());
                };
                next
            }
        };
        let mut cursor = store.map_cursor(&self.root.source, &lower, None, budget)?;
        let Some((key, reference)) = cursor.next()? else {
            self.phase = NativeScrubPhase::RunRegistry;
            return Ok(());
        };
        let digest: [u8; 32] = key
            .as_slice()
            .try_into()
            .map_err(|_width_error| corrupt("scrub source key width differs"))?;
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("scrub source value kind differs"));
        }
        let coverage = decode_source(&store.read_blob(&reference, 64 * 1024, budget)?)?;
        if source_digest(&coverage.source.file)? != digest {
            return Err(corrupt("scrub source key differs"));
        }
        self.source_after = Some(digest);
        Ok(())
    }

    fn finish_blob(&mut self, purpose: PendingPurpose, bytes: &[u8]) -> Result<(), CoreError> {
        let segment = self
            .segment
            .as_mut()
            .ok_or_else(|| corrupt("scrub segment missing"))?;
        match purpose {
            PendingPurpose::Body { local, record } => {
                let (candidate, indexed, folded) = decode_body(bytes)?;
                let sha: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
                if candidate != record.candidate_id || sha != record.indexed_sha256 {
                    return Err(corrupt("scrub body/current identity differs"));
                }
                add_grams(&mut segment.raw_grams, indexed.as_bytes(), local)?;
                add_grams(&mut segment.folded_grams, folded.as_bytes(), local)?;
                add_terms(&mut segment.raw_terms, &indexed, CaseMode::Sensitive, local)?;
                add_terms(&mut segment.folded_terms, &indexed, CaseMode::Folded, local)?;
                segment.next_document = segment.next_document.saturating_add(1);
                self.live_count = self
                    .live_count
                    .checked_add(1)
                    .ok_or_else(|| corrupt("scrub live count overflows"))?;
            }
            PendingPurpose::Inventory => {
                let (version, routes): (u16, Vec<Vec<u8>>) =
                    crate::channel_payloads::decode_cbor_exact(bytes).map_err(|error| {
                        corrupt(&format!("scrub route inventory decode: {error}"))
                    })?;
                if version != 1
                    || routes.windows(2).any(|pair| {
                        let [left, right] = pair else { return true };
                        left >= right
                    })
                    || routes
                        .iter()
                        .any(|route| route_posting_key(route, segment.descriptor.id).is_err())
                {
                    return Err(corrupt("scrub route inventory is not canonical"));
                }
                segment.routes = routes;
                self.phase = NativeScrubPhase::Route;
            }
            PendingPurpose::Page { keys } => {
                let page = PostingPage::decode(bytes, segment.descriptor.id)?;
                if page.rows.len() != keys.len() {
                    return Err(corrupt("scrub page inventory count differs"));
                }
                let live = segment
                    .descriptor
                    .live_locals
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>();
                for row in page.rows {
                    if !keys.contains(&(row.surface, row.key.to_vec())) {
                        return Err(corrupt("scrub page has unreferenced row"));
                    }
                    if matches!(row.surface, GRAM_RAW | GRAM_FOLDED) {
                        self.gram_routes = self
                            .gram_routes
                            .checked_add(1)
                            .ok_or_else(|| corrupt("scrub gram count overflows"))?;
                        let gram: [u8; 3] = row
                            .key
                            .try_into()
                            .map_err(|_width_error| corrupt("scrub gram width differs"))?;
                        let actual = crate::text_authority::paged_segment::decode_posting(
                            row.payload,
                            segment.descriptor.id,
                            row.surface,
                            segment.descriptor.logical_ids.len(),
                        )?
                        .into_iter()
                        .filter(|local| live.contains(local))
                        .collect::<Vec<_>>();
                        let expected = if row.surface == GRAM_RAW {
                            segment.raw_grams.remove(&gram)
                        } else {
                            segment.folded_grams.remove(&gram)
                        };
                        if actual != expected.unwrap_or_default() {
                            return Err(corrupt("scrub gram/body relation differs"));
                        }
                    } else {
                        self.term_routes = self
                            .term_routes
                            .checked_add(1)
                            .ok_or_else(|| corrupt("scrub term count overflows"))?;
                        let term = std::str::from_utf8(row.key)
                            .map_err(|_utf8_error| corrupt("scrub term UTF-8 differs"))?;
                        let actual = decode_term_posting_live(
                            row.payload,
                            segment.descriptor.id,
                            row.surface,
                            segment.descriptor.logical_ids.len(),
                            &live,
                        )?;
                        let expected = if row.surface == TERM_RAW {
                            segment.raw_terms.remove(term)
                        } else {
                            segment.folded_terms.remove(term)
                        };
                        if actual != expected.unwrap_or_default() {
                            return Err(corrupt("scrub term/body relation differs"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Recompute every posting from selected committed bodies.
///
/// Compare the complete route inventory and persisted posting bytes. Unchanged
/// segments are inherited through their sealed base root and are not rederived.
#[cfg(test)]
pub(crate) fn verify_changed_segments_for_seal(
    store: &ObjectStore,
    root: &NativeRoot,
    first_new_segment_id: u64,
) -> Result<(), CoreError> {
    verify_changed_segments_for_seal_inner(store, root, first_new_segment_id, None).map(|_facts| ())
}

/// Complete the independent transition's clean-merge attribution.
///
/// The existing changed-segment oracle reads each new descriptor and its postings.
/// No input or output facts escape unless both proofs complete.
pub(crate) fn verify_changed_segments_for_seal_with_merge_facts(
    store: &ObjectStore,
    root: &NativeRoot,
    first_new_segment_id: u64,
    proof: NativeMergeProof,
) -> Result<crate::maintenance_merge_facts::NativeRunMergeFacts, CoreError> {
    verify_changed_segments_for_seal_inner(store, root, first_new_segment_id, Some(proof))?
        .ok_or_else(|| corrupt("Native merge proof facts missing"))
}

fn verify_changed_segments_for_seal_inner(
    store: &ObjectStore,
    root: &NativeRoot,
    first_new_segment_id: u64,
    mut merge_proof: Option<NativeMergeProof>,
) -> Result<Option<crate::maintenance_merge_facts::NativeRunMergeFacts>, CoreError> {
    // Reauthenticate in this independent proof, with bounded map-page reuse.
    let verification_store = store.metered_clone();
    let store = &verification_store;
    root.validate()?;
    if first_new_segment_id == 0 || first_new_segment_id > root.next_segment_id {
        return Err(corrupt("new segment range differs"));
    }
    let mut output = crate::maintenance_merge_facts::NativeMergeOutputFacts::default();
    let mut output_attributable = true;
    for segment_key in first_new_segment_id..root.next_segment_id {
        let segment = read_segment(store, root, segment_key)?;
        if segment.live_locals.len() != segment.logical_ids.len()
            || segment
                .live_locals
                .iter()
                .enumerate()
                .any(|(ordinal, local)| !matches!(usize::try_from(*local), Ok(value) if value == ordinal))
        {
            return Err(corrupt("new segment has missing or retired local IDs"));
        }
        let mut raw_grams = BTreeMap::<[u8; 3], Vec<u32>>::new();
        let mut folded_grams = BTreeMap::<[u8; 3], Vec<u32>>::new();
        let mut raw_terms = TermDocs::new();
        let mut folded_terms = TermDocs::new();
        for (ordinal, id) in segment.logical_ids.iter().copied().enumerate() {
            let local =
                u32::try_from(ordinal).map_err(|_error| corrupt("local ID width differs"))?;
            let (current_ref, record) = read_current_with_ref(store, root, id)?
                .ok_or_else(|| corrupt("new segment current row missing"))?;
            if record.segment_key != segment_key || record.local_id != local {
                return Err(corrupt("new segment current locator differs"));
            }
            let digest = source_digest(&record.source)?;
            let source_ref = store
                .map_get(&root.source, &digest, None)?
                .ok_or_else(|| corrupt("new segment source binding missing"))?;
            let coverage = decode_source(&store.read_blob(&source_ref, 64 * 1024, None)?)?;
            if coverage.source.file != record.source {
                return Err(corrupt("new segment source identity differs"));
            }
            let membership_ref = store
                .map_get(&root.membership, &membership_key(&digest, id), None)?
                .ok_or_else(|| corrupt("new segment membership missing"))?;
            if current_ref != membership_ref {
                return Err(corrupt("new segment membership/current differ"));
            }
            if record.body.root.kind != ObjectKind::NativeBody {
                return Err(corrupt("new segment body kind differs"));
            }
            let body = store.read_blob(&record.body, MAX_BODY_BYTES, None)?;
            let (candidate, indexed, folded) = decode_body(&body)?;
            let indexed_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
            if candidate != record.candidate_id || indexed_sha256 != record.indexed_sha256 {
                return Err(corrupt("new segment body/current identity differs"));
            }
            add_grams(&mut raw_grams, indexed.as_bytes(), local)?;
            add_grams(&mut folded_grams, folded.as_bytes(), local)?;
            add_terms(&mut raw_terms, &indexed, CaseMode::Sensitive, local)?;
            add_terms(&mut folded_terms, &indexed, CaseMode::Folded, local)?;
        }
        let mut expected = Vec::<(Vec<u8>, ObjectKind, Vec<u8>)>::new();
        for (surface, grams) in [(GRAM_RAW, raw_grams), (GRAM_FOLDED, folded_grams)] {
            for (gram, locals) in grams {
                let key = route_key(&gram_prefix(surface, gram), segment_key);
                let bytes = encode_expected_posting(segment_key, surface, &locals, None)?;
                expected.push((key, ObjectKind::NativeTrigrams, bytes));
            }
        }
        for (surface, terms) in [(TERM_RAW, raw_terms), (TERM_FOLDED, folded_terms)] {
            for (term, docs) in terms {
                let locals = docs.iter().map(|(local, _)| *local).collect::<Vec<_>>();
                let positions = encode_positions(&docs)?;
                let key = route_key(&term_prefix(surface, &term)?, segment_key);
                let bytes =
                    encode_expected_posting(segment_key, surface, &locals, Some(&positions))?;
                expected.push((key, ObjectKind::NativePositions, bytes));
            }
        }
        expected.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let inventory = read_route_inventory(store, &segment)?;
        if inventory.len() != expected.len()
            || inventory
                .iter()
                .zip(&expected)
                .any(|(key, row)| key != &row.0)
        {
            return Err(corrupt(
                "new segment route inventory differs from raw documents",
            ));
        }
        let mut pages = BTreeMap::<ObjectRef, (BlobRef, BTreeMap<(u8, Vec<u8>), Vec<u8>>)>::new();
        for (key, kind, expected_bytes) in expected {
            let routes = if kind == ObjectKind::NativeTrigrams {
                &root.gram_routes
            } else {
                &root.term_routes
            };
            let reference = store
                .map_get(routes, &key, None)?
                .ok_or_else(|| corrupt("new segment route is missing"))?;
            if reference.root.kind != kind {
                return Err(corrupt("new segment posting kind differs"));
            }
            let (surface, posting_key) = route_posting_key(&key, segment_key)?;
            let group = pages
                .entry(reference.root.clone())
                .or_insert_with(|| (reference.clone(), BTreeMap::new()));
            if group.0 != reference
                || group
                    .1
                    .insert((surface, posting_key.to_vec()), expected_bytes)
                    .is_some()
            {
                return Err(corrupt(
                    "new segment posting page route repeats or reference differs",
                ));
            }
        }
        for (_, (reference, expected_rows)) in pages {
            let actual_bytes = store.read_blob(&reference, MAX_POSTING_BYTES, None)?;
            let actual = PostingPage::decode(&actual_bytes, segment_key)?;
            if actual.rows.len() != expected_rows.len()
                || actual.rows.iter().any(|row| {
                    expected_rows
                        .get(&(row.surface, row.key.to_vec()))
                        .map(Vec::as_slice)
                        != Some(row.payload)
                })
            {
                return Err(corrupt(
                    "new segment posting page differs from raw documents",
                ));
            }
        }
        if let Some(proof) = merge_proof.as_mut() {
            observe_verified_native_output(
                proof,
                segment_key,
                &segment.logical_ids,
                &mut output,
                &mut output_attributable,
            );
        }
    }
    let Some(proof) = merge_proof else {
        return Ok(None);
    };
    finish_native_output(proof, output, output_attributable).map(Some)
}

fn observe_verified_native_output(
    proof: &mut NativeMergeProof,
    segment_key: u64,
    logical_ids: &[u64],
    output: &mut crate::maintenance_merge_facts::NativeMergeOutputFacts,
    output_attributable: &mut bool,
) {
    if proof.input.input_runs == 0 {
        return;
    }
    let clean_here = logical_ids
        .iter()
        .filter(|id| proof.clean_doc_ids.remove(*id))
        .count();
    if clean_here == 0 {
        return;
    }
    if clean_here != logical_ids.len() {
        *output_attributable = false;
    } else if let Some(bytes) = proof.new_segment_index_bytes.get(&segment_key) {
        match output
            .segments
            .checked_add(1)
            .zip(output.index_encoded_bytes.checked_add(*bytes))
        {
            Some((segments, index_encoded_bytes)) => {
                output.segments = segments;
                output.index_encoded_bytes = index_encoded_bytes;
            }
            None => *output_attributable = false,
        }
    } else {
        *output_attributable = false;
    }
}

fn finish_native_output(
    mut proof: NativeMergeProof,
    output: crate::maintenance_merge_facts::NativeMergeOutputFacts,
    output_attributable: bool,
) -> Result<crate::maintenance_merge_facts::NativeRunMergeFacts, CoreError> {
    if proof.input.input_runs > 0 {
        proof.input.output = if output_attributable
            && proof.clean_doc_ids.is_empty()
            && output.segments > 0
            && output.index_encoded_bytes > 0
        {
            Some(output)
        } else {
            None
        };
    }
    proof.input.validate()?;
    Ok(proof.input)
}

/// Full semantic scrub of every live Native document and committed route.
///
/// Retired local postings remain immutable and are ignored only after their
/// full payload has been parsed and authenticated.
pub(crate) fn verify_all_for_scrub(
    store: &ObjectStore,
    root: &NativeRoot,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    root.validate()?;
    let mut live_count = 0_u64;
    let mut gram_routes = 0_u64;
    let mut term_routes = 0_u64;
    let mut due_count = 0_u64;
    let mut segments = store.map_cursor(&root.segments, &[0], None, budget)?;
    while let Some((key, reference)) = segments.next()? {
        checkpoint(budget, "lexical:native:scrub-segment")?;
        let segment_key = u64::from_be_bytes(
            key.as_slice()
                .try_into()
                .map_err(|_width_error| corrupt("segment map key width differs"))?,
        );
        if reference.root.kind != ObjectKind::Segment {
            return Err(corrupt("segment map value kind differs"));
        }
        let descriptor =
            SegmentDescriptor::decode(&store.read_blob(&reference, 64 * 1024, budget)?)?;
        if descriptor.id != segment_key {
            return Err(corrupt("segment map key and descriptor differ"));
        }
        let due = store.map_get(&root.compaction_due, &key, budget)?;
        if descriptor.live_locals.len() < descriptor.logical_ids.len() {
            if due.as_ref() != Some(&reference) {
                return Err(corrupt("partially retired segment debt differs"));
            }
            due_count = due_count
                .checked_add(1)
                .ok_or_else(|| corrupt("debt count overflow"))?;
        } else if due.is_some() {
            return Err(corrupt("full segment has compaction debt"));
        }
        let mut raw_grams = BTreeMap::<[u8; 3], Vec<u32>>::new();
        let mut folded_grams = BTreeMap::<[u8; 3], Vec<u32>>::new();
        let mut raw_terms = TermDocs::new();
        let mut folded_terms = TermDocs::new();
        for local in &descriptor.live_locals {
            checkpoint(budget, "lexical:native:scrub-body")?;
            let id = *descriptor
                .logical_ids
                .get(usize::try_from(*local).map_err(|_width_error| corrupt("live local width"))?)
                .ok_or_else(|| corrupt("live local outside segment table"))?;
            let current_ref = store
                .map_get(&root.current, &current_key(id), budget)?
                .ok_or_else(|| corrupt("live current record absent"))?;
            if current_ref.root.kind != ObjectKind::Record {
                return Err(corrupt("current record kind differs"));
            }
            let record =
                CurrentRecord::decode(&store.read_blob(&current_ref, 32 * 1024, budget)?)?;
            if record.id != id || record.segment_key != segment_key || record.local_id != *local {
                return Err(corrupt("live current locator differs"));
            }
            let digest = source_digest(&record.source)?;
            if store.map_get(&root.membership, &membership_key(&digest, id), budget)?
                != Some(current_ref)
            {
                return Err(corrupt("live membership differs from current"));
            }
            let source_ref = store
                .map_get(&root.source, &digest, budget)?
                .ok_or_else(|| corrupt("live source binding absent"))?;
            if source_ref.root.kind != ObjectKind::Record {
                return Err(corrupt("source binding kind differs"));
            }
            let coverage = decode_source(&store.read_blob(&source_ref, 64 * 1024, budget)?)?;
            if coverage.source.file != record.source {
                return Err(corrupt("live source binding identity differs"));
            }
            if record.body.root.kind != ObjectKind::NativeBody {
                return Err(corrupt("live body kind differs"));
            }
            let (candidate, indexed, folded) =
                decode_body(&store.read_blob(&record.body, MAX_BODY_BYTES, budget)?)?;
            let indexed_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
            if candidate != record.candidate_id || indexed_sha256 != record.indexed_sha256 {
                return Err(corrupt("live body identity differs"));
            }
            add_grams(&mut raw_grams, indexed.as_bytes(), *local)?;
            add_grams(&mut folded_grams, folded.as_bytes(), *local)?;
            add_terms(&mut raw_terms, &indexed, CaseMode::Sensitive, *local)?;
            add_terms(&mut folded_terms, &indexed, CaseMode::Folded, *local)?;
            live_count = live_count
                .checked_add(1)
                .ok_or_else(|| corrupt("live count overflow"))?;
        }
        let live = descriptor
            .live_locals
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut page_routes = BTreeMap::<ObjectRef, (BlobRef, BTreeSet<(u8, Vec<u8>)>)>::new();
        for route in read_route_inventory_budget(store, &descriptor, budget)? {
            checkpoint(budget, "lexical:native:scrub-posting-route")?;
            let (surface, posting_key) = route_posting_key(&route, segment_key)?;
            let (map, kind) = match surface {
                GRAM_RAW | GRAM_FOLDED => (&root.gram_routes, ObjectKind::NativeTrigrams),
                TERM_RAW | TERM_FOLDED => (&root.term_routes, ObjectKind::NativePositions),
                _ => return Err(corrupt("route surface differs")),
            };
            let reference = store
                .map_get(map, &route, budget)?
                .ok_or_else(|| corrupt("inventory route absent"))?;
            if reference.root.kind != kind {
                return Err(corrupt("posting route kind differs"));
            }
            let group = page_routes
                .entry(reference.root.clone())
                .or_insert_with(|| (reference.clone(), BTreeSet::new()));
            if group.0 != reference || !group.1.insert((surface, posting_key.to_vec())) {
                return Err(corrupt("posting page route repeats or reference differs"));
            }
        }
        for (_, (reference, expected_keys)) in page_routes {
            checkpoint(budget, "lexical:native:scrub-posting-page")?;
            let page_bytes = store.read_blob(&reference, MAX_POSTING_BYTES, budget)?;
            let page = PostingPage::decode(&page_bytes, segment_key)?;
            if page.rows.len() != expected_keys.len() {
                return Err(corrupt("posting page inventory count differs"));
            }
            for row in page.rows {
                checkpoint(budget, "lexical:native:scrub-posting-row")?;
                let surface = row.surface;
                if !expected_keys.contains(&(surface, row.key.to_vec())) {
                    return Err(corrupt("posting page contains unreferenced row"));
                }
                if matches!(surface, GRAM_RAW | GRAM_FOLDED) {
                    gram_routes = gram_routes
                        .checked_add(1)
                        .ok_or_else(|| corrupt("gram route count overflow"))?;
                    let gram: [u8; 3] = row
                        .key
                        .try_into()
                        .map_err(|_width_error| corrupt("gram key width"))?;
                    let actual = crate::text_authority::paged_segment::decode_posting(
                        row.payload,
                        segment_key,
                        surface,
                        descriptor.logical_ids.len(),
                    )?;
                    let actual_live = actual
                        .into_iter()
                        .filter(|local| live.contains(local))
                        .collect::<Vec<_>>();
                    let expected = if surface == GRAM_RAW {
                        raw_grams.remove(&gram)
                    } else {
                        folded_grams.remove(&gram)
                    };
                    if actual_live != expected.unwrap_or_default() {
                        return Err(corrupt("live gram membership differs from body"));
                    }
                } else {
                    term_routes = term_routes
                        .checked_add(1)
                        .ok_or_else(|| corrupt("term route count overflow"))?;
                    let term = std::str::from_utf8(row.key)
                        .map_err(|_utf8_error| corrupt("term route is not UTF-8"))?;
                    let actual = decode_term_posting_live(
                        row.payload,
                        segment_key,
                        surface,
                        descriptor.logical_ids.len(),
                        &live,
                    )?;
                    let expected = if surface == TERM_RAW {
                        raw_terms.remove(term)
                    } else {
                        folded_terms.remove(term)
                    };
                    if actual != expected.unwrap_or_default() {
                        return Err(corrupt("live term positions differ from body"));
                    }
                }
            }
        }
        if !raw_grams.is_empty()
            || !folded_grams.is_empty()
            || !raw_terms.is_empty()
            || !folded_terms.is_empty()
        {
            return Err(corrupt("body term or gram lacks posting route"));
        }
        let indexed = independently_count_segment_index(store, root, &key, &reference, budget)?;
        root.run_registry
            .verify_segment(store, &key, &reference, indexed, budget)?;
    }
    if live_count != root.live_docs
        || gram_routes != root.gram_routes.entries
        || term_routes != root.term_routes.entries
        || due_count != root.compaction_due.entries
    {
        return Err(corrupt("full scrub counts differ from roots"));
    }
    // Run metadata is a separate authenticated ownership graph. The loop
    // above checked each segment against its owner; this streams the run
    // headers and members to check aggregate custody and hidden extra rows.
    root.run_registry.verify_all(store, budget)?;
    let mut sources = store.map_cursor(&root.source, &[0], None, budget)?;
    while let Some((key, reference)) = sources.next()? {
        checkpoint(budget, "lexical:native:scrub-source")?;
        let digest: [u8; 32] = key
            .as_slice()
            .try_into()
            .map_err(|_width_error| corrupt("source key width differs"))?;
        if reference.root.kind != ObjectKind::Record {
            return Err(corrupt("source value kind differs"));
        }
        let coverage = decode_source(&store.read_blob(&reference, 64 * 1024, budget)?)?;
        if source_digest(&coverage.source.file)? != digest {
            return Err(corrupt("source key differs from coverage identity"));
        }
    }
    Ok(())
}

/// Independently enumerate one changed segment's authenticated index closure.
///
/// The writer's producer-side root selection is not reused by seal
/// or scrub. Body/current/source bytes are outside this posting-run counter
/// and must be accounted by the service disk authority.
pub(crate) fn independently_count_segment_index(
    store: &ObjectStore,
    root: &NativeRoot,
    key: &[u8],
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<u64, CoreError> {
    let id = u64::from_be_bytes(
        key.try_into()
            .map_err(|_error| corrupt("run verifier segment key width differs"))?,
    );
    if reference.root.kind != ObjectKind::Segment {
        return Err(corrupt("run verifier segment kind differs"));
    }
    let descriptor = SegmentDescriptor::decode(&store.read_blob(reference, 64 * 1024, budget)?)?;
    if descriptor.id != id {
        return Err(corrupt("run verifier segment key/value differs"));
    }
    let inventory = descriptor
        .route_inventory
        .as_ref()
        .ok_or_else(|| corrupt("run verifier route inventory absent"))?;
    let mut roots = BTreeSet::from([reference.root.clone(), inventory.root.clone()]);
    for route in read_route_inventory_budget(store, &descriptor, budget)? {
        let surface = route
            .first()
            .copied()
            .ok_or_else(|| corrupt("empty route"))?;
        let (map, kind) = match surface {
            GRAM_RAW | GRAM_FOLDED => (&root.gram_routes, ObjectKind::NativeTrigrams),
            TERM_RAW | TERM_FOLDED => (&root.term_routes, ObjectKind::NativePositions),
            _ => return Err(corrupt("run verifier route surface differs")),
        };
        let posting = store
            .map_get(map, &route, budget)?
            .ok_or_else(|| corrupt("run verifier posting route absent"))?;
        if posting.root.kind != kind {
            return Err(corrupt("run verifier posting kind differs"));
        }
        let _inserted = roots.insert(posting.root);
    }
    crate::lexical_runs::segment_index_encoded_bytes(
        store,
        &roots.into_iter().collect::<Vec<_>>(),
        budget,
        u64::try_from(crate::lexical_store::MAX_CLOSURE_REFS)
            .map_err(|error| corrupt(&format!("closure bound width: {error}")))?,
    )
}

fn decode_term_posting_live(
    bytes: &[u8],
    segment_key: u64,
    surface: u8,
    document_count: usize,
    live: &BTreeSet<u32>,
) -> Result<Vec<(u32, Vec<u32>)>, CoreError> {
    let (version, actual_segment, actual_surface, locals, positions): (
        u16,
        u64,
        u8,
        Vec<u32>,
        Option<Vec<u8>>,
    ) = crate::channel_payloads::decode_cbor_exact(bytes)
        .map_err(|error| corrupt(&format!("term posting decode: {error}")))?;
    if version != 1
        || actual_segment != segment_key
        || actual_surface != surface
        || locals.is_empty()
        || locals.len() > document_count
        || locals.windows(2).any(|pair| {
            let [left, right] = pair else { return true };
            left >= right
        })
        || locals
            .iter()
            .any(|local| usize::try_from(*local).map_or(true, |value| value >= document_count))
    {
        return Err(corrupt("term posting header or locals differ"));
    }
    let raw = positions.ok_or_else(|| corrupt("term posting positions absent"))?;
    let mut offset = 0_usize;
    let count = read_varint(&raw, &mut offset)?;
    if !matches!(usize::try_from(count), Ok(value) if value == locals.len()) {
        return Err(corrupt("term position document count differs"));
    }
    let mut result = Vec::new();
    let mut prior_doc = 0_u32;
    for (ordinal, local) in locals.into_iter().enumerate() {
        let gap = read_varint(&raw, &mut offset)?;
        let decoded_local = prior_doc
            .checked_add(gap)
            .ok_or_else(|| corrupt("term doc gap overflows"))?;
        if ordinal > 0 && gap == 0 || decoded_local != local {
            return Err(corrupt("term doc gap differs from local list"));
        }
        prior_doc = decoded_local;
        let position_count = read_varint(&raw, &mut offset)?;
        if position_count == 0
            || position_count > MAX_POSITIONS_PER_CELL
            || usize::try_from(position_count)
                .map_or(true, |count| count > raw.len().saturating_sub(offset))
        {
            return Err(corrupt("term position count is invalid"));
        }
        let mut positions = Vec::new();
        if live.contains(&local) {
            positions
                .try_reserve(
                    usize::try_from(position_count)
                        .map_err(|_width_error| corrupt("position width"))?,
                )
                .map_err(|error| {
                    CoreError::Storage(format!("lexical: reserve scrub positions: {error}"))
                })?;
        }
        let mut prior_position = 0_u32;
        for index in 0..position_count {
            let gap = read_varint(&raw, &mut offset)?;
            let position = prior_position
                .checked_add(gap)
                .ok_or_else(|| corrupt("term position overflows"))?;
            if index > 0 && gap == 0 {
                return Err(corrupt("term positions repeat"));
            }
            prior_position = position;
            if live.contains(&local) {
                positions.push(position);
            }
        }
        if live.contains(&local) {
            result.push((local, positions));
        }
    }
    if offset != raw.len() {
        return Err(corrupt("term posting has trailing bytes"));
    }
    Ok(result)
}

fn read_varint(bytes: &[u8], offset: &mut usize) -> Result<u32, CoreError> {
    let start = *offset;
    let mut value = 0_u32;
    for shift in [0, 7, 14, 21, 28] {
        let byte = *bytes
            .get(*offset)
            .ok_or_else(|| corrupt("truncated varint"))?;
        *offset = (*offset).saturating_add(1);
        if shift == 28 && byte > 15 {
            return Err(corrupt("varint exceeds u32"));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            if offset.saturating_sub(start) > 1 && byte == 0 {
                return Err(corrupt("noncanonical varint"));
            }
            return Ok(value);
        }
    }
    Err(corrupt("overlong varint"))
}

fn checkpoint(budget: Option<&RequestBudgetV1>, stage: &'static str) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint(stage)?;
    }
    Ok(())
}

fn add_grams(
    output: &mut BTreeMap<[u8; 3], Vec<u32>>,
    text: &[u8],
    local: u32,
) -> Result<(), CoreError> {
    let mut unique = BTreeSet::new();
    for window in text.windows(3) {
        let gram: [u8; 3] = window
            .try_into()
            .map_err(|_error| corrupt("gram width differs"))?;
        let _inserted = unique.insert(gram);
    }
    for gram in unique {
        let docs = output.entry(gram).or_default();
        docs.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native gram oracle: {error}"))
        })?;
        docs.push(local);
    }
    Ok(())
}

fn add_terms(
    output: &mut TermDocs,
    indexed: &str,
    mode: CaseMode,
    local: u32,
) -> Result<(), CoreError> {
    let mut per_doc = BTreeMap::<String, Vec<u32>>::new();
    for token in normalize::tokenize(indexed, mode).indexable() {
        let position = u32::try_from(token.position)
            .map_err(|_error| corrupt("token position exceeds u32"))?;
        let positions = per_doc.entry(token.text.clone()).or_default();
        if positions.len()
            >= usize::try_from(MAX_POSITIONS_PER_CELL).map_or(usize::MAX, |count| count)
        {
            return Err(corrupt("term cell exceeds position cap"));
        }
        positions.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native position oracle: {error}"))
        })?;
        positions.push(position);
    }
    for (term, positions) in per_doc {
        let docs = output.entry(term).or_default();
        docs.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native term oracle: {error}"))
        })?;
        docs.push((local, positions));
    }
    Ok(())
}

fn encode_positions(docs: &[(u32, Vec<u32>)]) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    put_varint(
        u32::try_from(docs.len()).map_err(|_error| corrupt("term doc count exceeds u32"))?,
        &mut bytes,
    )?;
    let mut prior_doc = 0_u32;
    for (local, positions) in docs {
        let gap = local
            .checked_sub(prior_doc)
            .ok_or_else(|| corrupt("term docs not ascending"))?;
        put_varint(gap, &mut bytes)?;
        prior_doc = *local;
        put_varint(
            u32::try_from(positions.len())
                .map_err(|_error| corrupt("position count exceeds u32"))?,
            &mut bytes,
        )?;
        let mut prior_position = 0_u32;
        for position in positions {
            let gap = position
                .checked_sub(prior_position)
                .ok_or_else(|| corrupt("positions not ascending"))?;
            put_varint(gap, &mut bytes)?;
            prior_position = *position;
        }
    }
    Ok(bytes)
}

fn put_varint(mut value: u32, bytes: &mut Vec<u8>) -> Result<(), CoreError> {
    bytes.try_reserve(5).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve Native oracle varint: {error}"))
    })?;
    while value >= 128 {
        bytes.push(
            u8::try_from(value & 0x7f).map_err(|_error| corrupt("varint byte width"))? | 0x80,
        );
        value >>= 7;
    }
    bytes.push(u8::try_from(value).map_err(|_error| corrupt("varint tail width"))?);
    Ok(())
}

fn encode_expected_posting(
    segment_key: u64,
    surface: u8,
    locals: &[u32],
    positions: Option<&[u8]>,
) -> Result<Vec<u8>, CoreError> {
    crate::channel_payloads::encode_cbor(
        &(1_u16, segment_key, surface, locals, positions),
        "Native seal oracle posting",
    )
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!("lexical: Native seal oracle: {reason}"),
    }
}

#[cfg(test)]
mod cursor_tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;

    use super::{
        NativeScrubCursor, finish_native_output, independently_count_segment_index,
        observe_verified_native_output, verify_all_for_scrub, verify_changed_segments_for_seal,
        verify_changed_segments_for_seal_with_merge_facts,
    };
    use crate::lexical_allocation::DocVersionReservation;
    use crate::lexical_runs::SegmentRemoval;
    use crate::lexical_store::{AttemptBinding, MapEdit, ObjectStore};
    use crate::maintenance_merge_facts::{NativeMergeOutputFacts, NativeRunMergeFacts};
    use crate::text_authority::paged_merge_proof::NativeMergeProof;
    use crate::text_authority::paged_transition::{
        verify_transition_for_seal, verify_transition_for_seal_with_policy,
    };
    use crate::text_authority::paged_writer::publish_delta_prepared_with_policy;
    use crate::text_authority::{AddedTextDoc, NativeAuthority, NativeSourceEdit, publish_delta};
    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SourceFileCoverage,
        SourceFileKey, SourceFileRevision, SymbolCoverage, SymbolNameSourcePolicyV1,
    };
    use quanta_index_core::RequestBudgetV1;
    use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
    use sha2::{Digest as _, Sha256};

    #[test]
    fn mixed_clean_output_attribution_is_unavailable_without_refusing_valid_facts() {
        let proof = || NativeMergeProof {
            input: NativeRunMergeFacts {
                input_runs: 2,
                input_segments: 2,
                input_index_encoded_bytes: 47,
                output: Some(NativeMergeOutputFacts::default()),
            },
            clean_doc_ids: BTreeSet::from([11, 12]),
            new_segment_index_bytes: BTreeMap::from([(7, 31), (8, 29)]),
        };
        let mut pure = proof();
        let mut pure_output = NativeMergeOutputFacts::default();
        let mut pure_available = true;
        observe_verified_native_output(&mut pure, 7, &[11], &mut pure_output, &mut pure_available);
        observe_verified_native_output(&mut pure, 8, &[12], &mut pure_output, &mut pure_available);
        let pure_facts = finish_native_output(pure, pure_output, pure_available)
            .expect("fixed pure output facts");
        assert_eq!(
            pure_facts.output,
            Some(NativeMergeOutputFacts {
                segments: 2,
                index_encoded_bytes: 60,
            })
        );

        for unrelated_id in [13, 99] {
            let mut mixed = proof();
            let mut output = NativeMergeOutputFacts::default();
            let mut available = true;
            // 13 is a fresh document, 99 a queued-debt survivor. Neither has
            // clean input ownership, so this segment has no exact clean cost.
            observe_verified_native_output(
                &mut mixed,
                7,
                &[11, unrelated_id],
                &mut output,
                &mut available,
            );
            observe_verified_native_output(&mut mixed, 8, &[12], &mut output, &mut available);
            let facts = finish_native_output(mixed, output, available)
                .expect("mixed attribution must not refuse a publication");
            assert_eq!(facts.input_runs, 2);
            assert_eq!(facts.input_segments, 2);
            assert_eq!(facts.input_index_encoded_bytes, 47);
            assert_eq!(facts.output, None);
        }
    }

    #[test]
    fn bounded_cursor_proves_the_same_live_pages_as_full_scrub() {
        let run = || -> Result<(), Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let store = ObjectStore::open(directory.path())?;
            let source_file = SourceFileKey {
                source_repo_id: RepoId::new("source-repo")?,
                repo_relative_path: RepoRelativePath::new("src/example.rs"),
            };
            let coverage = SourceFileCoverage {
                source: SourceFileRevision {
                    file: source_file.clone(),
                    revision_id: RevisionId::new("source-r1")?,
                    source_sha256: Sha256::digest(b"fn alpha() {} fn beta() {}").into(),
                },
                language: quanta_index_contract::lex::LanguageCode::new("rust")?,
                producer_policy_sha256: [1; 32],
                symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: [2; 32],
                text_admitted: true,
                symbols: SymbolCoverage::Complete { symbol_count: 0 },
            };
            let generation = ManifestGeneration::new(1);
            let binding = AttemptBinding {
                root_incarnation: [1; 16],
                repository_sha256: [2; 32],
                revision_sha256: [5; 32],
                source_event_sha256: [3; 32],
                target_generation: generation.get(),
                base_root_sha256: None,
                operations_sha256: [4; 32],
                format: 1,
            };
            let lease = store.begin_attempt(&binding, &[])?;
            let added = [
                AddedTextDoc {
                    doc_id: 1,
                    source_file: source_file.clone(),
                    candidate_id: "alpha".into(),
                    text: "fn alpha() { beta(); }".into(),
                },
                AddedTextDoc {
                    doc_id: 2,
                    source_file,
                    candidate_id: "beta".into(),
                    text: "fn beta() { alpha(); }".into(),
                },
            ];
            let producer_scope = store.metered_clone();
            let (root, receipt) = publish_delta(
                &producer_scope,
                &lease,
                None,
                &[NativeSourceEdit::Upsert(coverage.clone())],
                &[],
                &added,
                false,
                generation,
                [9; 32],
            )?;
            assert!(receipt.posting_pages_written > 0);
            assert!(
                receipt.posting_pages_written < root.gram_routes.entries + root.term_routes.entries
            );
            assert_eq!(receipt.oversized_posting_pages, 0);
            assert_eq!(receipt.oversized_posting_bytes, 0);
            let query = NativeAuthority::open(store.clone(), root.clone())?.scoped_query();
            let selected_preview = query.clone();
            let budget = RequestBudgetV1::unbounded();
            let gram_ids = query.intersect_grams(&[[b'a', b'l', b'p']], false, &budget)?;
            assert_eq!(gram_ids, vec![1, 2]);
            let pre_preview = query.authenticated_io_for_test();
            assert!(selected_preview.read_document(1, &budget)?.is_some());
            let physical = query.authenticated_io_for_test();
            assert!(pre_preview.0 > 0 && pre_preview.1 > 0);
            assert!(physical.0 > pre_preview.0 && physical.1 > pre_preview.1);
            assert!(
                query
                    .check_query_io_for_test(physical.0.saturating_sub(1), physical.1)
                    .is_err()
            );
            verify_changed_segments_for_seal(&store, &root, 1)?;
            verify_all_for_scrub(&store, &root, None)?;

            // A producer-scoped authenticated page can remain cached after
            // the durable bytes change. Both seal proofs must use independent
            // scopes and reject the selected changed map page.
            let current_page = root.current.page.as_ref().ok_or("current page absent")?;
            assert!(
                producer_scope
                    .map_get(&root.current, &1_u64.to_be_bytes(), None)?
                    .is_some()
            );
            let mut page_digest = String::new();
            for byte in current_page.object.sha256 {
                write!(&mut page_digest, "{byte:02x}")?;
            }
            let current_object = directory
                .path()
                .join("lexical-object-pool")
                .join("objects")
                .join(page_digest.get(..2).ok_or("current digest prefix absent")?)
                .join(&page_digest);
            let original_page = std::fs::read(&current_object)?;
            let mut changed_page = original_page.clone();
            let last = changed_page.last_mut().ok_or("current page empty")?;
            *last ^= 1;
            std::fs::write(&current_object, changed_page)?;
            assert!(
                producer_scope
                    .map_get(&root.current, &1_u64.to_be_bytes(), None)?
                    .is_some()
            );
            let reservation = DocVersionReservation {
                first: 1,
                count: 2,
                scope_sha256: [9; 32],
            };
            assert!(matches!(
                verify_transition_for_seal(
                    &store,
                    &root,
                    None,
                    &reservation,
                    &[NativeSourceEdit::Upsert(coverage)],
                    &[],
                    &added,
                    false,
                    1,
                    None,
                ),
                Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));
            assert!(matches!(
                verify_changed_segments_for_seal(&store, &root, 1),
                Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));
            std::fs::write(&current_object, original_page)?;

            let metered = store.metered_clone();
            let mut cursor = NativeScrubCursor::new(&root)?;
            let mut steps = 0_u32;
            loop {
                // Two distinct bodies require separate authenticated object
                // reads. A one-byte grant admits one indivisible artifact per
                // step and proves that the cursor retains progress.
                let step = cursor.step(&metered, 1, None)?;
                steps = steps.saturating_add(1);
                assert!(steps < 100_000, "scrub did not converge");
                if step.done {
                    assert!(step.read_count > 0 || steps > 1);
                    break;
                }
            }
            assert!(
                steps > 1,
                "cursor must not claim a whole scrub after one bounded step"
            );
            lease.finish_published(&root.root_objects())?;
            let next_binding = AttemptBinding {
                target_generation: 2,
                base_root_sha256: Some([8; 32]),
                ..binding
            };
            let next_lease = store.begin_attempt(&next_binding, &root.root_objects())?;
            let no_op_store = store.metered_clone();
            let before = no_op_store.read_meter_snapshot();
            let (unchanged, no_op_receipt) = publish_delta(
                &no_op_store,
                &next_lease,
                Some(&root),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(2),
                [9; 32],
            )?;
            let after = no_op_store.read_meter_snapshot();
            let reads = after
                .1
                .checked_sub(before.1)
                .ok_or("read meter regressed")?;
            assert!(
                reads > 0,
                "no-op producer must authenticate inherited roots"
            );
            assert_eq!(root.compaction_due.entries, 0);
            let expected_noop_reads = u64::try_from(root.root_objects().len())?
                .checked_add(1)
                .ok_or("no-op read bound overflow")?;
            assert!(
                reads <= expected_noop_reads,
                "no-op delta read beyond inherited map roots and one adoption-manifest verification: observed {reads}, bound {expected_noop_reads}"
            );
            assert_eq!(unchanged.current, root.current);
            assert_eq!(unchanged.segments, root.segments);
            assert_eq!(no_op_receipt.docs_derived, 0);
            assert_eq!(no_op_receipt.compaction_rederived_docs, 0);
            assert_eq!(no_op_receipt.compaction_rederived_text_bytes, 0);
            assert_eq!(no_op_receipt.shards_written, 0);
            assert_eq!(no_op_receipt.posting_pages_written, 0);

            // The first selected document authenticated segment 1. A clone of
            // this request reuses that proved descriptor, while a new request
            // must authenticate the object again and detect its mutation.
            let segment = store
                .map_get(&root.segments, &1_u64.to_be_bytes(), None)?
                .ok_or("first segment descriptor absent")?;
            let mut digest = String::new();
            for byte in segment.root.sha256 {
                write!(&mut digest, "{byte:02x}")?;
            }
            let object = directory
                .path()
                .join("lexical-object-pool")
                .join("objects")
                .join(digest.get(..2).ok_or("digest prefix absent")?)
                .join(&digest);
            let mut bytes = std::fs::read(&object)?;
            let tail = bytes.last_mut().ok_or("segment object empty")?;
            *tail ^= 1;
            std::fs::write(&object, bytes)?;
            assert!(selected_preview.read_document(2, &budget)?.is_some());
            let cancelled = RequestBudgetV1::unbounded();
            cancelled.cancel_handle().cancel();
            assert!(matches!(
                query.read_document(2, &cancelled),
                Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled,
                    ..
                })
            ));
            let fresh = NativeAuthority::open(store, root)?.scoped_query();
            assert!(matches!(
                fresh.read_document(2, &budget),
                Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                })
            ));
            Ok(())
        };
        run().expect("bounded Native scrub oracle");
    }

    #[test]
    fn append_only_runs_merge_physically_and_unrelated_healthy_removal_is_rejected() {
        let run = || -> Result<(), Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let store = ObjectStore::open(directory.path())?;
            let mut base = None;
            // Body payload dominates a deliberately small posting/index bundle.
            let texts = [
                format!("fn alpha() {{}} {}", "a".repeat(16 * 1024)),
                format!("fn beta() {{}} {}", "b".repeat(16 * 1024)),
                format!("fn gamma() {{}} {}", "c".repeat(16 * 1024)),
            ];
            for (ordinal, text) in texts.iter().enumerate() {
                let id = u64::try_from(ordinal)? + 1;
                let source = SourceFileKey {
                    source_repo_id: RepoId::new("source-repo")?,
                    repo_relative_path: RepoRelativePath::new(format!("src/doc-{id}.rs")),
                };
                let coverage = SourceFileCoverage {
                    source: SourceFileRevision {
                        file: source.clone(),
                        revision_id: RevisionId::new(format!("source-r{id}"))?,
                        source_sha256: Sha256::digest(text.as_bytes()).into(),
                    },
                    language: quanta_index_contract::lex::LanguageCode::new("rust")?,
                    producer_policy_sha256: [1; 32],
                    symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                    unit_set_sha256: [2; 32],
                    text_admitted: true,
                    symbols: SymbolCoverage::Complete { symbol_count: 0 },
                };
                let binding = AttemptBinding {
                    root_incarnation: [1; 16],
                    repository_sha256: [2; 32],
                    revision_sha256: [u8::try_from(id)?; 32],
                    source_event_sha256: [u8::try_from(id)?; 32],
                    target_generation: id,
                    base_root_sha256: base.as_ref().map(|_| [u8::try_from(id - 1).unwrap(); 32]),
                    operations_sha256: [u8::try_from(id)?; 32],
                    format: 1,
                };
                let inherited = base
                    .as_ref()
                    .map_or_else(Vec::new, super::NativeRoot::root_objects);
                let lease = store.begin_attempt(&binding, &inherited)?;
                let (next, _) = publish_delta(
                    &store,
                    &lease,
                    base.as_ref(),
                    &[NativeSourceEdit::Upsert(coverage)],
                    &[],
                    &[AddedTextDoc {
                        doc_id: id,
                        source_file: source,
                        candidate_id: format!("doc-{id}"),
                        text: text.clone(),
                    }],
                    false,
                    ManifestGeneration::new(id),
                    [9; 32],
                )?;
                lease.finish_published(&next.root_objects())?;
                base = Some(next);
            }
            let base = base.expect("three append-only generations");
            assert_eq!(base.run_registry.runs.entries, 3);
            let budget = RequestBudgetV1::unbounded();
            let gold = texts.iter().cloned().map(Some).collect::<Vec<_>>();
            let prior = NativeAuthority::open(store.clone(), base.clone())?;
            assert_eq!(prior.intersect_terms(&["alpha"], true, &budget)?, vec![1]);
            assert_eq!(prior.intersect_terms(&["beta"], true, &budget)?, vec![2]);
            assert_eq!(prior.intersect_terms(&["gamma"], true, &budget)?, vec![3]);
            let prior_docs = (1..=3)
                .map(|id| {
                    prior
                        .read_document(id, &budget)
                        .map(|document| document.map(|document| document.indexed_text))
                })
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(prior_docs, gold);
            let body_limited = LexicalMaintenancePolicy::new(
                2,
                8,
                64 * 1024 * 1024,
                128 * 1024 * 1024,
                2,
                64 * 1024 * 1024,
                8 * 1024,
                60_000,
            )?;
            let no_op_binding = AttemptBinding {
                root_incarnation: [1; 16],
                repository_sha256: [2; 32],
                revision_sha256: [5; 32],
                source_event_sha256: [5; 32],
                target_generation: 5,
                base_root_sha256: Some([3; 32]),
                operations_sha256: [5; 32],
                format: 1,
            };
            let no_op_lease = store.begin_attempt(&no_op_binding, &base.root_objects())?;
            let (unmerged, _) = publish_delta_prepared_with_policy(
                &store,
                &no_op_lease,
                Some(&base),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(5),
                [9; 32],
                None,
                Some(&body_limited),
                None,
            )?;
            assert_eq!(unmerged.run_registry.runs.entries, 3);
            assert_eq!(unmerged.segments, base.segments);
            let empty_reservation = DocVersionReservation {
                first: 0,
                count: 0,
                scope_sha256: [9; 32],
            };
            let no_merge_proof = verify_transition_for_seal_with_policy(
                &store,
                &unmerged,
                Some(&base),
                &empty_reservation,
                &[],
                &[],
                &[],
                false,
                base.next_segment_id,
                None,
                Some(&body_limited),
                None,
            )?;
            let no_merge_facts = verify_changed_segments_for_seal_with_merge_facts(
                &store,
                &unmerged,
                base.next_segment_id,
                no_merge_proof,
            )?;
            assert_eq!(
                no_merge_facts,
                crate::maintenance_merge_facts::NativeRunMergeFacts::default()
            );
            no_op_lease.discard_unsealed()?;

            let delta_source = SourceFileKey {
                source_repo_id: RepoId::new("source-repo")?,
                repo_relative_path: RepoRelativePath::new("src/doc-4.rs"),
            };
            let delta_text = "fn delta() {}";
            let delta_coverage = SourceFileCoverage {
                source: SourceFileRevision {
                    file: delta_source.clone(),
                    revision_id: RevisionId::new("source-r4")?,
                    source_sha256: Sha256::digest(delta_text.as_bytes()).into(),
                },
                language: quanta_index_contract::lex::LanguageCode::new("rust")?,
                producer_policy_sha256: [1; 32],
                symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: [2; 32],
                text_admitted: true,
                symbols: SymbolCoverage::Complete { symbol_count: 0 },
            };
            let delta_doc = AddedTextDoc {
                doc_id: 4,
                source_file: delta_source,
                candidate_id: "doc-4".into(),
                text: delta_text.into(),
            };
            let delta_edit = NativeSourceEdit::Upsert(delta_coverage);
            let delta_binding = AttemptBinding {
                root_incarnation: [1; 16],
                repository_sha256: [2; 32],
                revision_sha256: [6; 32],
                source_event_sha256: [6; 32],
                target_generation: 6,
                base_root_sha256: Some([3; 32]),
                operations_sha256: [6; 32],
                format: 1,
            };
            let delta_lease = store.begin_attempt(&delta_binding, &base.root_objects())?;
            let (delta, _) = publish_delta_prepared_with_policy(
                &store,
                &delta_lease,
                Some(&base),
                std::slice::from_ref(&delta_edit),
                &[],
                std::slice::from_ref(&delta_doc),
                false,
                ManifestGeneration::new(6),
                [9; 32],
                None,
                Some(&body_limited),
                None,
            )?;
            assert_eq!(delta.run_registry.runs.entries, 4);
            let delta_reservation = DocVersionReservation {
                first: 4,
                count: 1,
                scope_sha256: [9; 32],
            };
            let append_proof = verify_transition_for_seal_with_policy(
                &store,
                &delta,
                Some(&base),
                &delta_reservation,
                std::slice::from_ref(&delta_edit),
                &[],
                std::slice::from_ref(&delta_doc),
                false,
                base.next_segment_id,
                None,
                Some(&body_limited),
                None,
            )?;
            let append_facts = verify_changed_segments_for_seal_with_merge_facts(
                &store,
                &delta,
                base.next_segment_id,
                append_proof,
            )?;
            assert_eq!(
                append_facts,
                crate::maintenance_merge_facts::NativeRunMergeFacts::default()
            );
            let delta_reader = NativeAuthority::open(store.clone(), delta)?;
            assert_eq!(
                delta_reader.intersect_terms(&["delta"], true, &budget)?,
                vec![4]
            );
            assert_eq!(
                delta_reader
                    .read_document(4, &budget)?
                    .map(|doc| doc.indexed_text),
                Some(delta_text.into())
            );
            delta_lease.discard_unsealed()?;
            let policy = LexicalMaintenancePolicy::new(
                2,
                4,
                64 * 1024 * 1024,
                128 * 1024 * 1024,
                2,
                64 * 1024 * 1024,
                64 * 1024 * 1024,
                60_000,
            )?;
            let binding = AttemptBinding {
                root_incarnation: [1; 16],
                repository_sha256: [2; 32],
                revision_sha256: [4; 32],
                source_event_sha256: [4; 32],
                target_generation: 4,
                base_root_sha256: Some([3; 32]),
                operations_sha256: [4; 32],
                format: 1,
            };
            let lease = store.begin_attempt(&binding, &base.root_objects())?;
            let (merged, receipt) = publish_delta_prepared_with_policy(
                &store,
                &lease,
                Some(&base),
                &[],
                &[],
                &[],
                false,
                ManifestGeneration::new(4),
                [9; 32],
                None,
                Some(&policy),
                None,
            )?;
            assert_eq!(merged.run_registry.runs.entries, 2);
            assert_eq!(merged.live_docs, base.live_docs);
            assert_eq!(receipt.compaction_rederived_docs, 2);
            assert!(receipt.shards_written > 0);
            assert!(
                store
                    .map_get(&merged.segments, &1_u64.to_be_bytes(), None)?
                    .is_none()
            );
            assert!(
                store
                    .map_get(&merged.segments, &2_u64.to_be_bytes(), None)?
                    .is_none()
            );
            assert!(
                store
                    .map_get(&merged.segments, &3_u64.to_be_bytes(), None)?
                    .is_some()
            );
            let reservation = DocVersionReservation {
                first: 0,
                count: 0,
                scope_sha256: [9; 32],
            };
            let merge_proof = verify_transition_for_seal_with_policy(
                &store,
                &merged,
                Some(&base),
                &reservation,
                &[],
                &[],
                &[],
                false,
                base.next_segment_id,
                None,
                Some(&policy),
                None,
            )?;
            let merge_facts = verify_changed_segments_for_seal_with_merge_facts(
                &store,
                &merged,
                base.next_segment_id,
                merge_proof,
            )?;
            assert_eq!(merge_facts.input_runs, 2);
            assert_eq!(merge_facts.input_segments, 2);
            let expected_input_bytes = [1_u64, 2].into_iter().try_fold(0_u64, |sum, id| {
                let key = id.to_be_bytes();
                let reference = store.map_get(&base.segments, &key, None)?.ok_or_else(|| {
                    quanta_index_core::CoreError::NotFound("fixed old segment absent".into())
                })?;
                let bytes =
                    independently_count_segment_index(&store, &base, &key, &reference, None)?;
                sum.checked_add(bytes).ok_or_else(|| {
                    quanta_index_core::CoreError::InvalidContract(
                        "fixed old index byte sum overflow".into(),
                    )
                })
            })?;
            assert_eq!(merge_facts.input_index_encoded_bytes, expected_input_bytes);
            let expected_output =
                std::iter::once(base.next_segment_id).try_fold(0_u64, |sum, id| {
                    let key = id.to_be_bytes();
                    let reference =
                        store
                            .map_get(&merged.segments, &key, None)?
                            .ok_or_else(|| {
                                quanta_index_core::CoreError::NotFound(
                                    "fixed merged segment absent".into(),
                                )
                            })?;
                    let bytes =
                        independently_count_segment_index(&store, &merged, &key, &reference, None)?;
                    sum.checked_add(bytes).ok_or_else(|| {
                        quanta_index_core::CoreError::InvalidContract(
                            "fixed merged index byte sum overflow".into(),
                        )
                    })
                })?;
            assert_eq!(
                merge_facts.output,
                Some(crate::maintenance_merge_facts::NativeMergeOutputFacts {
                    segments: 1,
                    index_encoded_bytes: expected_output,
                })
            );
            let mixed_binding = AttemptBinding {
                root_incarnation: [1; 16],
                repository_sha256: [2; 32],
                revision_sha256: [7; 32],
                source_event_sha256: [7; 32],
                target_generation: 7,
                base_root_sha256: Some([3; 32]),
                operations_sha256: [7; 32],
                format: 1,
            };
            let mixed_lease = store.begin_attempt(&mixed_binding, &base.root_objects())?;
            let (mixed, _) = publish_delta_prepared_with_policy(
                &store,
                &mixed_lease,
                Some(&base),
                std::slice::from_ref(&delta_edit),
                &[],
                std::slice::from_ref(&delta_doc),
                false,
                ManifestGeneration::new(7),
                [9; 32],
                None,
                Some(&policy),
                None,
            )?;
            let mixed_proof = verify_transition_for_seal_with_policy(
                &store,
                &mixed,
                Some(&base),
                &delta_reservation,
                std::slice::from_ref(&delta_edit),
                &[],
                std::slice::from_ref(&delta_doc),
                false,
                base.next_segment_id,
                None,
                Some(&policy),
                None,
            )?;
            let mixed_facts = verify_changed_segments_for_seal_with_merge_facts(
                &store,
                &mixed,
                base.next_segment_id,
                mixed_proof,
            )?;
            assert_eq!(mixed_facts.input_runs, 2);
            assert_eq!(mixed_facts.input_segments, 2);
            assert_eq!(mixed_facts.input_index_encoded_bytes, expected_input_bytes);
            let mut clean_outputs = Vec::new();
            let mut fresh_outputs = Vec::new();
            for id in base.next_segment_id..mixed.next_segment_id {
                let segment =
                    crate::text_authority::paged_writer::read_segment(&store, &mixed, id)?;
                if segment
                    .logical_ids
                    .iter()
                    .all(|logical| [1, 2].contains(logical))
                {
                    clean_outputs.push(id);
                } else {
                    assert_eq!(segment.logical_ids, vec![4]);
                    fresh_outputs.push(id);
                }
            }
            assert_eq!(clean_outputs.len(), 1);
            assert_eq!(fresh_outputs.len(), 1);
            let clean_key = clean_outputs
                .first()
                .ok_or("fixed mixed clean output absent")?
                .to_be_bytes();
            let clean_reference = store
                .map_get(&mixed.segments, &clean_key, None)?
                .ok_or("fixed mixed clean segment absent")?;
            let clean_output_bytes = independently_count_segment_index(
                &store,
                &mixed,
                &clean_key,
                &clean_reference,
                None,
            )?;
            assert_eq!(
                mixed_facts.output,
                Some(crate::maintenance_merge_facts::NativeMergeOutputFacts {
                    segments: 1,
                    index_encoded_bytes: clean_output_bytes,
                })
            );
            mixed_lease.discard_unsealed()?;
            verify_all_for_scrub(&store, &merged, None)?;
            let current = NativeAuthority::open(store.clone(), merged.clone())?;
            let actual = (1..=3)
                .map(|id| {
                    current
                        .read_document(id, &budget)
                        .map(|document| document.map(|document| document.indexed_text))
                })
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(actual, gold);

            let unrelated = 3_u64.to_be_bytes().to_vec();
            let reference = store
                .map_get(&merged.segments, &unrelated, None)?
                .expect("third run remains physical");
            let mut forged = merged;
            forged.segments = store.map_apply(
                &lease,
                &forged.segments,
                std::iter::once(Ok(MapEdit {
                    key: unrelated.clone(),
                    value: None,
                })),
            )?;
            forged.run_registry = forged.run_registry.apply(
                &store,
                &lease,
                &[SegmentRemoval {
                    key: unrelated,
                    reference,
                }],
                &[],
            )?;
            forged.validate()?;
            assert!(
                verify_transition_for_seal_with_policy(
                    &store,
                    &forged,
                    Some(&base),
                    &reservation,
                    &[],
                    &[],
                    &[],
                    false,
                    base.next_segment_id,
                    None,
                    Some(&policy),
                    None,
                )
                .is_err()
            );
            Ok(())
        };
        run().expect("clean Native run merge and independent refusal");
    }
}
