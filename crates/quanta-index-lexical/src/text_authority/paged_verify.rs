//! Seal oracle for newly written Native segments, independent of posting builders.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_positions::MAX_POSITIONS_PER_CELL;
use sha2::{Digest as _, Sha256};

use crate::lexical_store::{BlobCursor, BlobRef, ObjectKind, ObjectRef, ObjectStore};
use crate::normalize::{self, CaseMode};
use crate::text_authority::paged_manifest::NativeRoot;
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
            live_count: 0,
            gram_routes: 0,
            term_routes: 0,
            due_count: 0,
            invalidated: false,
        })
    }

    /// Advance through at most the caller's byte allowance, except for one
    /// indivisible authenticated map/record read or one 32 KiB blob block.
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
        let due = store.map_get(&self.root.compaction_due, &key, budget)?;
        if descriptor.live_locals.len() < descriptor.logical_ids.len() {
            if due != Some(reference) {
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
                    self.phase = NativeScrubPhase::Done;
                    return Ok(());
                };
                next
            }
        };
        let mut cursor = store.map_cursor(&self.root.source, &lower, None, budget)?;
        let Some((key, reference)) = cursor.next()? else {
            self.phase = NativeScrubPhase::Done;
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
pub(crate) fn verify_changed_segments_for_seal(
    store: &ObjectStore,
    root: &NativeRoot,
    first_new_segment_id: u64,
) -> Result<(), CoreError> {
    // Reauthenticate in this independent proof, with bounded map-page reuse.
    let verification_store = store.metered_clone();
    let store = &verification_store;
    root.validate()?;
    if first_new_segment_id == 0 || first_new_segment_id > root.next_segment_id {
        return Err(corrupt("new segment range differs"));
    }
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
    }
    Ok(())
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
            if due != Some(reference) {
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
    }
    if live_count != root.live_docs
        || gram_routes != root.gram_routes.entries
        || term_routes != root.term_routes.entries
        || due_count != root.compaction_due.entries
    {
        return Err(corrupt("full scrub counts differ from roots"));
    }
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
    use std::fmt::Write as _;

    use super::{NativeScrubCursor, verify_all_for_scrub, verify_changed_segments_for_seal};
    use crate::lexical_allocation::DocVersionReservation;
    use crate::lexical_store::{AttemptBinding, ObjectStore};
    use crate::text_authority::paged_transition::verify_transition_for_seal;
    use crate::text_authority::{AddedTextDoc, NativeAuthority, NativeSourceEdit, publish_delta};
    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SourceFileCoverage,
        SourceFileKey, SourceFileRevision, SymbolCoverage, SymbolNameSourcePolicyV1,
    };
    use quanta_index_core::RequestBudgetV1;
    use sha2::{Digest as _, Sha256};

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
}
