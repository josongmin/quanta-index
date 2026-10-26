//! Changed-source-only V17 publication. The caller owns durable attempt and
//! generation publication; this module only prepares immutable store roots.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use quanta_index_contract::{SourceFileCoverage, SourceFileKey};
use quanta_index_core::lexical_maintenance::{
    LexicalMaintenancePolicy, StagedPacketBounds, StagedPacketIdentity,
};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::trigrams_of;
use sha2::{Digest as _, Sha256};

use crate::lexical_allocation::DocVersionReservation;
use crate::lexical_runs::{
    RunCandidate, RunRegistry, RunScanWindow, RunSelectionProgress, SegmentInput, SegmentRemoval,
    bounded_candidate_scan_limit, segment_index_encoded_bytes,
};
#[cfg(test)]
use crate::lexical_store::MapCursor;
use crate::lexical_store::{
    AttemptLease, Authority as ScanAuthority, BlobRef, CandidateWitness, EligibilityWitness,
    IneligibleReason, MapEdit, MapRoot, ObjectKind, ObjectRef, ObjectStore, ScanLane,
    WindowWitness,
};

#[cfg(test)]
use super::route_key;
use super::run_postings;
use super::run_postings::{
    RunGramCount, RunPostingPage, RunPostingPageEdit, RunPostingRoot, RunPostingRow,
    apply_page_delta, apply_run_page_edits, bootstrap_new_run_postings, merge_run_posting_roots,
    run_page_key, run_route_key,
};
use super::{
    AuthorityRoot, LiveDoc, MAX_POSTING_PAGE_BYTES, MAX_POSTING_PAGE_TERMS, MAX_SEGMENT_DOCS,
    PostingPage, Segment, SourceHead, doc_key, membership_key, policy_digest, source_key_digest,
};

// Leaves room within the existing 128 MiB scratch admission for the source
// vectors, physical page descriptors, and map builder's bounded page stack.
pub(super) const MAX_SEGMENT_GRAM_BYTES: usize = 32 * 1024 * 1024;
const MAX_SINGLE_SOURCE_GRAM_BYTES: usize = 80 * 1024 * 1024;
const ADOPTION_CPU_FLUSH_BYTES: usize = 2_097_152;
// Each run edit now holds both the authenticated prior page and its target.
// Two worst-case pages per key keep the batch within the former target-only
// 1024-page scratch envelope; physical leaf page width remains unchanged.
const MAX_RUN_EDIT_BATCH_PAGES: usize = MAX_POSTING_PAGE_TERMS.div_euclid(2);
const _: () = assert!(MAX_RUN_EDIT_BATCH_PAGES > 0);

#[derive(Debug)]
pub(crate) enum FileSourceEdit {
    Upsert {
        coverage: SourceFileCoverage,
        bytes: Vec<u8>,
    },
    Delete {
        key: SourceFileKey,
    },
}

impl FileSourceEdit {
    pub(super) fn key(&self) -> &SourceFileKey {
        match self {
            Self::Upsert { coverage, .. } => &coverage.source.file,
            Self::Delete { key } => key,
        }
    }
}

#[derive(Debug)]
pub(crate) struct FileAuthorityMutation {
    pub(crate) edits: Vec<FileSourceEdit>,
    pub(crate) clear_all: bool,
    pub(crate) operations_sha256: [u8; 32],
}

pub(crate) struct FileAuthorityBuild {
    pub(crate) root: AuthorityRoot,
    pub(crate) root_bytes: Vec<u8>,
}

struct Prepared {
    key_digest: [u8; 32],
    head: SourceHead,
    head_blob: BlobRef,
    path_grams: Vec<[u8; 3]>,
    content_grams: Vec<[u8; 3]>,
}

#[cfg(test)]
struct CreatedSegment {
    segment: Segment,
    reference: BlobRef,
}

struct CleanRunSelection {
    run_ids: Vec<u64>,
    progress: RunSelectionProgress,
}

#[cfg(test)]
struct RouteStream<'a> {
    store: &'a ObjectStore,
    surface: u8,
    delete: bool,
    cursors: Vec<MapCursor<'a>>,
    references: Vec<BlobRef>,
    doc_counts: Vec<usize>,
    page_cache: Vec<Option<(BlobRef, PostingPage)>>,
    // route_key orders equal grams by posting count before segment digest.
    ready: BTreeSet<([u8; 3], u32, [u8; 32], usize)>,
    failed: bool,
}

#[cfg(test)]
impl<'a> RouteStream<'a> {
    fn new(
        store: &'a ObjectStore,
        surface: u8,
        segments: &[CreatedSegment],
        delete: bool,
    ) -> Result<Self, CoreError> {
        let mut stream = Self {
            store,
            surface,
            delete,
            cursors: Vec::new(),
            references: Vec::new(),
            doc_counts: Vec::new(),
            page_cache: Vec::new(),
            ready: BTreeSet::new(),
            failed: false,
        };
        stream
            .cursors
            .try_reserve(segments.len())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "file authority v17: reserve route cursors: {error}"
                ))
            })?;
        stream
            .references
            .try_reserve(segments.len())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "file authority v17: reserve route references: {error}"
                ))
            })?;
        for segment in segments {
            let terms = if surface == 1 {
                &segment.segment.path_terms
            } else {
                &segment.segment.content_terms
            };
            let mut cursor = store.map_cursor(terms, &[], None, None)?;
            let index = stream.cursors.len();
            let first = cursor.next()?;
            stream.cursors.push(cursor);
            stream.references.push(segment.reference.clone());
            stream.doc_counts.push(segment.segment.doc_ids.len());
            stream.page_cache.push(None);
            if let Some((key, page)) = first {
                let gram: [u8; 3] = key
                    .as_slice()
                    .try_into()
                    .map_err(|_width| corrupt("segment term map key is not a gram"))?;
                let count = stream.posting_count(index, gram, &page)?;
                let _inserted =
                    stream
                        .ready
                        .insert((gram, count, segment.reference.root.sha256, index));
            }
        }
        Ok(stream)
    }

    fn posting_count(
        &mut self,
        index: usize,
        gram: [u8; 3],
        page_ref: &BlobRef,
    ) -> Result<u32, CoreError> {
        let cache = self
            .page_cache
            .get_mut(index)
            .ok_or_else(|| corrupt("route posting cache absent"))?;
        if cache
            .as_ref()
            .is_none_or(|(reference, _)| reference != page_ref)
        {
            if page_ref.root.kind != ObjectKind::FilePostings {
                return Err(corrupt("route posting page kind differs"));
            }
            let doc_count = *self
                .doc_counts
                .get(index)
                .ok_or_else(|| corrupt("route segment size absent"))?;
            let bytes = self
                .store
                .read_blob(page_ref, size_u64(MAX_POSTING_PAGE_BYTES)?, None)?;
            *cache = Some((page_ref.clone(), PostingPage::decode(&bytes, doc_count)?));
        }
        let (_, page) = cache
            .as_ref()
            .ok_or_else(|| corrupt("route posting page absent"))?;
        let position = page
            .0
            .binary_search_by_key(&gram, |row| row.0)
            .map_err(|_position| corrupt("route gram absent from posting page"))?;
        Ok(page
            .0
            .get(position)
            .ok_or_else(|| corrupt("route gram position is absent"))?
            .1
            .count_ones())
    }
}

#[cfg(test)]
impl Iterator for RouteStream<'_> {
    type Item = Result<MapEdit, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let (gram, count, digest, index) = self.ready.pop_first()?;
        let Some(reference) = self.references.get(index).cloned() else {
            self.failed = true;
            return Some(Err(corrupt("route cursor index differs")));
        };
        let next = self
            .cursors
            .get_mut(index)
            .map_or_else(|| Err(corrupt("route cursor is absent")), MapCursor::next);
        match next {
            Ok(Some((key, page))) => {
                let Ok(next_gram) = key.as_slice().try_into() else {
                    self.failed = true;
                    return Some(Err(corrupt("segment term key is not a gram")));
                };
                if next_gram <= gram {
                    self.failed = true;
                    return Some(Err(corrupt("segment terms are not strictly ordered")));
                }
                let next_count = match self.posting_count(index, next_gram, &page) {
                    Ok(count) => count,
                    Err(error) => {
                        self.failed = true;
                        return Some(Err(error));
                    }
                };
                let _inserted = self.ready.insert((next_gram, next_count, digest, index));
            }
            Ok(None) => {}
            Err(error) => {
                self.failed = true;
                return Some(Err(error));
            }
        }
        Some(Ok(MapEdit {
            key: route_key(self.surface, gram, count, digest).to_vec(),
            value: if self.delete { None } else { Some(reference) },
        }))
    }
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("file authority v17 producer: {reason}"))
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file authority v17 producer: {reason}"),
    }
}

fn size_u64(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value).map_err(|error| invalid(&format!("byte length exceeds u64: {error}")))
}

fn encode_record(
    store: &ObjectStore,
    lease: &AttemptLease,
    bytes: &[u8],
    deps: &[ObjectRef],
) -> Result<BlobRef, CoreError> {
    store.put_blob(lease, ObjectKind::Record, deps, bytes)
}

fn read_head(
    store: &ObjectStore,
    root: &MapRoot,
    key: &SourceFileKey,
) -> Result<Option<(BlobRef, SourceHead)>, CoreError> {
    let digest = source_key_digest(key)?;
    let Some(reference) = store.map_get(root, &digest, None)? else {
        return Ok(None);
    };
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("source map value has wrong object kind"));
    }
    let bytes = store.read_blob(&reference, size_u64(super::MAX_RECORD_BYTES)?, None)?;
    let head = SourceHead::decode(&bytes)?;
    if head.coverage.source.file != *key {
        return Err(corrupt(
            "source digest key collides with a different identity",
        ));
    }
    Ok(Some((reference, head)))
}

fn read_live(store: &ObjectStore, root: &MapRoot, id: u64) -> Result<LiveDoc, CoreError> {
    let reference = store
        .map_get(root, &doc_key(id), None)?
        .ok_or_else(|| corrupt("current map omits source logical ID"))?;
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("current map value has wrong object kind"));
    }
    LiveDoc::decode(&store.read_blob(&reference, size_u64(super::MAX_RECORD_BYTES)?, None)?)
}

fn read_segment_ref(
    store: &ObjectStore,
    root: &MapRoot,
    digest: [u8; 32],
) -> Result<BlobRef, CoreError> {
    let reference = store
        .map_get(root, &digest, None)?
        .ok_or_else(|| corrupt("current locator segment is missing"))?;
    if reference.root.kind != ObjectKind::Segment || reference.root.sha256 != digest {
        return Err(corrupt("segment map key or object kind differs"));
    }
    Ok(reference)
}

fn checked_sub(total: &mut u64, amount: u64, field: &str) -> Result<(), CoreError> {
    *total = total
        .checked_sub(amount)
        .ok_or_else(|| corrupt(&format!("{field} summary underflow")))?;
    Ok(())
}

fn checked_add(total: &mut u64, amount: u64, limit: u64, field: &str) -> Result<(), CoreError> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| invalid(&format!("{field} summary overflow")))?;
    if *total > limit {
        return Err(invalid(&format!("{field} admission exceeded")));
    }
    Ok(())
}

fn unique_grams(bytes: &[u8]) -> Result<Vec<[u8; 3]>, CoreError> {
    // Small sources are cheaper to sort directly than to initialize a 2 MiB
    // universe bitmap for each path and body. The resulting vector stays
    // below the fixed bitmap size at this threshold.
    const DIRECT_SORT_BYTES: usize = 16 * 1024;
    if bytes.len() <= DIRECT_SORT_BYTES {
        let mut grams = Vec::new();
        grams
            .try_reserve_exact(bytes.len().saturating_sub(2))
            .map_err(|error| {
                CoreError::Storage(format!(
                    "file authority v17: reserve direct source grams: {error}"
                ))
            })?;
        grams.extend(trigrams_of(bytes));
        grams.sort_unstable();
        grams.dedup();
        return Ok(grams);
    }
    // A source may normalize to much more than its raw length. The fixed
    // 24-bit universe bounds scratch independently of normalized byte count.
    // Record only encountered grams; enumerating the whole universe per file
    // makes a tiny-file corpus pay the same scan cost as a large source.
    let mut bitmap = Vec::new();
    bitmap.try_reserve_exact(1 << 21).map_err(|error| {
        CoreError::Storage(format!("file authority v17: reserve gram bitmap: {error}"))
    })?;
    bitmap.resize(1 << 21, 0_u8);
    let mut grams = Vec::new();
    for [first, second, third] in trigrams_of(bytes) {
        let value = (usize::from(first) << 16) | (usize::from(second) << 8) | usize::from(third);
        let mask = 1_u8 << (value & 7);
        let slot = bitmap
            .get_mut(value >> 3)
            .ok_or_else(|| invalid("gram exceeds fixed bitmap universe"))?;
        if *slot & mask == 0 {
            grams.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!("file authority v17: reserve source grams: {error}"))
            })?;
            *slot |= mask;
            grams.push([first, second, third]);
        }
    }
    grams.sort_unstable();
    Ok(grams)
}

#[cfg(test)]
#[test]
fn distinct_source_grams_are_sorted_without_full_universe_enumeration() {
    assert!(unique_grams(b"ab").expect("short source").is_empty());
    assert_eq!(
        unique_grams(&[0xff, 0, 0xff, 0, 0xff]).expect("binary grams"),
        vec![[0, 0xff, 0], [0xff, 0, 0xff]]
    );
    let repeated = b"abc".repeat(5_462);
    let expected = vec![*b"abc", *b"bca", *b"cab"];
    assert_eq!(
        unique_grams(repeated.get(..16 * 1024).expect("direct slice")).expect("direct threshold"),
        expected
    );
    assert_eq!(
        unique_grams(repeated.get(..=16 * 1024).expect("bitmap slice")).expect("bitmap threshold"),
        expected
    );
}

#[cfg(test)]
#[test]
fn route_stream_orders_real_segments_by_count_before_digest() {
    use crate::lexical_store::AttemptBinding;

    fn segment_with_gram(
        store: &ObjectStore,
        lease: &AttemptLease,
        gram: [u8; 3],
        doc_ids: Vec<u64>,
        mask: u64,
    ) -> Result<CreatedSegment, CoreError> {
        let page = PostingPage(vec![(gram, mask)]);
        let page_ref = store.put_blob(
            lease,
            ObjectKind::FilePostings,
            &[],
            &page.encode(doc_ids.len())?,
        )?;
        let path_terms = store.map_bootstrap(
            lease,
            std::iter::once(Ok(MapEdit {
                key: gram.to_vec(),
                value: Some(page_ref),
            })),
        )?;
        let segment = Segment {
            doc_ids,
            path_terms,
            content_terms: MapRoot::default(),
            path_memberships: u64::from(mask.count_ones()),
            content_memberships: 0,
        };
        let dependencies = segment
            .path_terms
            .page
            .iter()
            .map(|page| page.object.clone())
            .collect::<Vec<_>>();
        let reference = store.put_blob(
            lease,
            ObjectKind::Segment,
            &dependencies,
            &segment.encode()?,
        )?;
        Ok(CreatedSegment { segment, reference })
    }

    let directory = tempfile::tempdir().expect("state root");
    let store = ObjectStore::open(directory.path()).expect("store");
    let binding = AttemptBinding {
        root_incarnation: [1; 16],
        repository_sha256: [2; 32],
        revision_sha256: [8; 32],
        source_event_sha256: [3; 32],
        target_generation: 1,
        base_root_sha256: None,
        operations_sha256: [4; 32],
        format: 17,
    };
    let lease = store.begin_attempt(&binding, &[]).expect("attempt");
    let gram = *b"abc";
    let broad =
        segment_with_gram(&store, &lease, gram, vec![1, 2], 0b11).expect("two-document segment");
    // Select a fixed deterministic candidate whose digest order opposes the
    // canonical count order. The old digest-first stream then fails this test.
    let narrow = (3_u64..=66)
        .find_map(|id| {
            let candidate = segment_with_gram(&store, &lease, gram, vec![id], 0b1)
                .expect("one-document segment");
            (broad.reference.root.sha256 < candidate.reference.root.sha256).then_some(candidate)
        })
        .expect("fixed candidates contain an opposing digest order");
    let stream = RouteStream::new(&store, 1, &[broad, narrow], false).expect("route stream");
    let rows = stream.collect::<Result<Vec<_>, _>>().expect("route rows");
    assert_eq!(rows.len(), 2);
    assert!(rows.windows(2).all(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left.key < right.key)
    }));
    let counts = rows
        .iter()
        .map(|row| {
            u32::from_be_bytes(
                row.key
                    .get(4..8)
                    .expect("posting count bytes")
                    .try_into()
                    .expect("u32 posting count"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![1, 2]);
    let root = store
        .map_bootstrap(&lease, rows.into_iter().map(Ok))
        .expect("durable ordered routing map");
    assert_eq!(root.entries, 2);
}

fn gram_bytes(item: &Prepared) -> Result<usize, CoreError> {
    item.path_grams
        .capacity()
        .checked_add(item.content_grams.capacity())
        .and_then(|grams| grams.checked_mul(std::mem::size_of::<[u8; 3]>()))
        .ok_or_else(|| invalid("source gram scratch overflow"))
}

fn group_gram_bytes(group: &[Prepared]) -> Result<usize, CoreError> {
    group.iter().try_fold(0_usize, |total, item| {
        total
            .checked_add(gram_bytes(item)?)
            .ok_or_else(|| invalid("segment gram scratch overflow"))
    })
}

fn derived_digest(coverage: &SourceFileCoverage) -> Result<[u8; 32], CoreError> {
    let bytes =
        crate::channel_payloads::encode_cbor(&(policy_digest(), coverage), "file derived input")?;
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-derived-input-v17\0");
    hasher.update(bytes);
    Ok(hasher.finalize().into())
}

fn prepare_upsert(
    store: &ObjectStore,
    lease: &AttemptLease,
    coverage: &SourceFileCoverage,
    bytes: &[u8],
    logical_id: u64,
) -> Result<Prepared, CoreError> {
    coverage
        .validate()
        .map_err(|error| invalid(&error.to_string()))?;
    if bytes.len() > super::super::MAX_FILE_BYTES {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            message: "file authority: changed file exceeds 8 MiB admission".into(),
        });
    }
    if <[u8; 32]>::from(Sha256::digest(bytes)) != coverage.source.source_sha256 {
        return Err(invalid("source bytes differ from revision"));
    }
    let raw = if coverage.text_admitted {
        Some(
            std::str::from_utf8(bytes)
                .map_err(|error| invalid(&format!("text source UTF-8: {error}")))?,
        )
    } else {
        None
    };
    let scratch = usize::try_from(super::super::policy().bucket_scratch_bytes)
        .map_err(|error| invalid(&format!("normalization scratch width: {error}")))?;
    let (_, folded_path, indexed_text, folded_text) = super::super::normalized_surfaces(
        coverage.source.file.repo_relative_path.as_str(),
        raw,
        0,
        scratch,
    )?;
    let path_grams = unique_grams(folded_path.as_bytes())?;
    let content_grams = folded_text
        .as_ref()
        .map_or_else(|| Ok(Vec::new()), |text| unique_grams(text.as_bytes()))?;
    let path_memberships = size_u64(path_grams.len())?;
    let content_memberships = size_u64(content_grams.len())?;
    let memberships = path_memberships
        .checked_add(content_memberships)
        .ok_or_else(|| invalid("source membership count overflow"))?;
    if memberships > super::super::policy().total_memberships {
        return Err(invalid("source membership admission exceeded"));
    }
    let body = store.put_blob(lease, ObjectKind::FileBody, &[], bytes)?;
    let head = SourceHead {
        coverage: coverage.clone(),
        body: body.clone(),
        logical_id,
        source_bytes: size_u64(bytes.len())?,
        indexed_text_bytes: size_u64(indexed_text.as_ref().map_or(0, String::len))?,
        folded_text_bytes: size_u64(folded_text.as_ref().map_or(0, String::len))?,
        posting_memberships: memberships,
        derived_input_sha256: derived_digest(coverage)?,
    };
    let head_blob = encode_record(store, lease, &head.encode()?, &[body.root])?;
    Ok(Prepared {
        key_digest: source_key_digest(&coverage.source.file)?,
        head,
        head_blob,
        path_grams,
        content_grams,
    })
}

/// Merge each source's sorted unique grams without a corpus-sized term map.
/// The set contains at most one head per source in this physical segment.
struct GramMerge<'a> {
    group: &'a [Prepared],
    content: bool,
    offsets: Vec<usize>,
    ready: BTreeSet<([u8; 3], usize)>,
}

impl<'a> GramMerge<'a> {
    fn new(group: &'a [Prepared], content: bool) -> Result<Self, CoreError> {
        let mut offsets = Vec::new();
        offsets.try_reserve_exact(group.len()).map_err(|error| {
            CoreError::Storage(format!("file authority v17: reserve gram cursors: {error}"))
        })?;
        let mut ready = BTreeSet::new();
        for (local, item) in group.iter().enumerate() {
            offsets.push(0);
            let grams = if content {
                &item.content_grams
            } else {
                &item.path_grams
            };
            if let Some(gram) = grams.first() {
                let _inserted = ready.insert((*gram, local));
            }
        }
        Ok(Self {
            group,
            content,
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
                .ok_or_else(|| corrupt("gram merge head disappeared"))?;
            self.advance(local, gram, &mut mask)?;
        }
        Ok(Some((gram, mask)))
    }

    fn advance(&mut self, local: usize, gram: [u8; 3], mask: &mut u64) -> Result<(), CoreError> {
        if local >= MAX_SEGMENT_DOCS {
            return Err(invalid("local ID exceeds bitmask width"));
        }
        *mask |= 1_u64 << local;
        let offset = self
            .offsets
            .get_mut(local)
            .ok_or_else(|| corrupt("gram cursor offset absent"))?;
        *offset = offset
            .checked_add(1)
            .ok_or_else(|| invalid("gram cursor overflow"))?;
        let item = self
            .group
            .get(local)
            .ok_or_else(|| corrupt("gram cursor source absent"))?;
        let grams = if self.content {
            &item.content_grams
        } else {
            &item.path_grams
        };
        if let Some(next) = grams.get(*offset) {
            if *next <= gram {
                return Err(corrupt("source grams are not strictly ordered"));
            }
            let _inserted = self.ready.insert((*next, local));
        }
        Ok(())
    }
}

fn write_term_map(
    store: &ObjectStore,
    lease: &AttemptLease,
    group: &[Prepared],
    content: bool,
) -> Result<(MapRoot, u64), CoreError> {
    let mut pages = Vec::new();
    let mut memberships = 0_u64;
    let mut page_rows = Vec::new();
    let mut merge = GramMerge::new(group, content)?;
    while let Some((gram, mask)) = merge.next_term()? {
        memberships = memberships
            .checked_add(u64::from(mask.count_ones()))
            .ok_or_else(|| invalid("segment membership overflow"))?;
        page_rows.push((gram, mask));
        if page_rows.len() == MAX_POSTING_PAGE_TERMS {
            emit_posting_page(store, lease, &mut pages, &mut page_rows, group.len())?;
        }
    }
    if !page_rows.is_empty() {
        emit_posting_page(store, lease, &mut pages, &mut page_rows, group.len())?;
    }
    let mut page_index = 0_usize;
    let mut merge = GramMerge::new(group, content)?;
    let rows = std::iter::from_fn(|| {
        let next = match merge.next_term() {
            Ok(Some((gram, _mask))) => gram,
            Ok(None) => return None,
            Err(error) => return Some(Err(error)),
        };
        while pages.get(page_index).is_some_and(|(last, _)| *last < next) {
            let Some(next_index) = page_index.checked_add(1) else {
                return Some(Err(invalid("posting page cursor overflow")));
            };
            page_index = next_index;
        }
        let row = pages
            .get(page_index)
            .ok_or_else(|| corrupt("posting page fence omits term"))
            .map(|(_, reference)| MapEdit {
                key: next.to_vec(),
                value: Some(reference.clone()),
            });
        Some(row)
    });
    let map = store.map_bootstrap(lease, rows)?;
    Ok((map, memberships))
}

fn emit_posting_page(
    store: &ObjectStore,
    lease: &AttemptLease,
    pages: &mut Vec<([u8; 3], BlobRef)>,
    pending: &mut Vec<([u8; 3], u64)>,
    doc_count: usize,
) -> Result<(), CoreError> {
    let page = PostingPage(std::mem::take(pending));
    let last = page
        .0
        .last()
        .ok_or_else(|| invalid("empty posting page"))?
        .0;
    let bytes = page.encode(doc_count)?;
    if bytes.len() > MAX_POSTING_PAGE_BYTES {
        return Err(invalid("posting page exceeds physical byte admission"));
    }
    let reference = store.put_blob(lease, ObjectKind::FilePostings, &[], &bytes)?;
    pages.try_reserve(1).map_err(|error| {
        CoreError::Storage(format!(
            "file authority v17: reserve posting page fence: {error}"
        ))
    })?;
    pages.push((last, reference));
    Ok(())
}

fn map_update(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: &MapRoot,
    edits: BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<MapRoot, CoreError> {
    if base.entries == 0 {
        store.map_bootstrap(
            lease,
            edits.into_iter().filter_map(|(key, value)| {
                value.map(|value| {
                    Ok(MapEdit {
                        key,
                        value: Some(value),
                    })
                })
            }),
        )
    } else {
        store.map_apply(
            lease,
            base,
            edits
                .into_iter()
                .map(|(key, value)| Ok(MapEdit { key, value })),
        )
    }
}

/// Replays one changed physical leaf into its owning run.
///
/// At most one physical posting page plus the existing map's bounded edit
/// batch is live, even for a large source.
fn apply_segment_to_run(
    store: &ObjectStore,
    lease: &AttemptLease,
    mut run: Option<RunPostingRoot>,
    run_id: u64,
    segment: &Segment,
    digest: [u8; 32],
    remove: bool,
) -> Result<RunPostingRoot, CoreError> {
    for (surface, terms) in [(1_u8, &segment.path_terms), (2_u8, &segment.content_terms)] {
        let mut cursor = store.map_cursor(terms, &[], None, None)?;
        let mut cached: Option<(BlobRef, PostingPage)> = None;
        let mut edits = BTreeMap::<Vec<u8>, RunPostingPageEdit>::new();
        while let Some((term_key, page_ref)) = cursor.next()? {
            let gram: [u8; 3] = term_key
                .as_slice()
                .try_into()
                .map_err(|_width| corrupt("run leaf term key width differs"))?;
            if cached
                .as_ref()
                .is_none_or(|(reference, _)| *reference != page_ref)
            {
                if page_ref.root.kind != ObjectKind::FilePostings {
                    return Err(corrupt("run leaf page object kind differs"));
                }
                cached = Some((
                    page_ref.clone(),
                    PostingPage::decode(
                        &store.read_blob(&page_ref, size_u64(MAX_POSTING_PAGE_BYTES)?, None)?,
                        segment.doc_ids.len(),
                    )?,
                ));
            }
            let page = &cached
                .as_ref()
                .ok_or_else(|| corrupt("run leaf page absent"))?
                .1;
            let position = page
                .0
                .binary_search_by_key(&gram, |entry| entry.0)
                .map_err(|_position| corrupt("run leaf page omits routed gram"))?;
            let mask = page
                .0
                .get(position)
                .ok_or_else(|| corrupt("run leaf page gram position disappeared"))?
                .1;
            let mut buckets = BTreeMap::<u64, Vec<RunPostingRow>>::new();
            for (local, logical_id) in segment.doc_ids.iter().copied().enumerate() {
                if mask & (1_u64 << local) != 0 {
                    buckets
                        .entry(logical_id >> 8)
                        .or_default()
                        .push(RunPostingRow {
                            logical_id,
                            segment_sha256: digest,
                        });
                }
            }
            if buckets.is_empty() {
                return Err(corrupt("run leaf term has no posting IDs"));
            }
            for (_bucket, rows) in buckets {
                let first_id = rows
                    .first()
                    .ok_or_else(|| corrupt("run leaf bucket has no posting rows"))?
                    .logical_id;
                let key = run_page_key(surface, gram, first_id)?.to_vec();
                let prior_ref = run
                    .as_ref()
                    .map(|root| store.map_get(&root.pages, &key, None))
                    .transpose()?
                    .flatten();
                let prior = prior_ref
                    .as_ref()
                    .map(|reference| {
                        if reference.root.kind != ObjectKind::Record {
                            return Err(corrupt("run page object kind differs"));
                        }
                        RunPostingPage::decode(&store.read_blob(
                            reference,
                            size_u64(MAX_POSTING_PAGE_BYTES)?,
                            None,
                        )?)
                    })
                    .transpose()?;
                let next = apply_page_delta(
                    surface,
                    gram,
                    first_id >> 8,
                    prior.as_ref(),
                    if remove { &rows } else { &[] },
                    if remove { &[] } else { &rows },
                )?;
                if edits
                    .insert(
                        key,
                        RunPostingPageEdit {
                            prior: prior_ref.zip(prior),
                            next,
                        },
                    )
                    .is_some()
                {
                    return Err(corrupt("run leaf emits duplicate bucket"));
                }
            }
            if edits.len() >= MAX_RUN_EDIT_BATCH_PAGES {
                let (next, _intermediate) = apply_run_page_edits(
                    store,
                    lease,
                    run.as_ref(),
                    run_id,
                    std::mem::take(&mut edits),
                )?;
                run = Some(next);
            }
        }
        if !edits.is_empty() {
            let (next, _intermediate) =
                apply_run_page_edits(store, lease, run.as_ref(), run_id, edits)?;
            run = Some(next);
        }
    }
    if let Some(run) = run {
        Ok(run)
    } else {
        let (empty, _reference) =
            apply_run_page_edits(store, lease, None, run_id, BTreeMap::new())?;
        Ok(empty)
    }
}

fn update_run_routes(
    store: &ObjectStore,
    lease: &AttemptLease,
    path_routes: &mut MapRoot,
    content_routes: &mut MapRoot,
    run_id: u64,
    prior: Option<&RunPostingRoot>,
    target: Option<&RunPostingRoot>,
) -> Result<(), CoreError> {
    let empty = MapRoot::default();
    let prior_counts = prior.map_or(&empty, |root| &root.gram_counts);
    let target_counts = target.map_or(&empty, |root| &root.gram_counts);
    let mut path_edits = BTreeMap::new();
    let mut content_edits = BTreeMap::new();
    store.map_changed_keys(
        prior_counts,
        target_counts,
        None,
        &mut |key, before, after| {
            let gram_key: [u8; 4] = key
                .try_into()
                .map_err(|_width| corrupt("run gram-count map key width differs"))?;
            let edits = match gram_key[0] {
                1 => &mut path_edits,
                2 => &mut content_edits,
                _ => return Err(corrupt("run gram-count surface differs")),
            };
            for (reference, addition) in [(before, false), (after, true)] {
                let Some(reference) = reference else { continue };
                if reference.root.kind != ObjectKind::Record {
                    return Err(corrupt("run gram-count object kind differs"));
                }
                let count = RunGramCount::decode(&store.read_blob(
                    reference,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    None,
                )?)?;
                if count.key()? != gram_key {
                    return Err(corrupt("run gram-count key differs"));
                }
                let route = run_route_key(count.surface, count.gram, count.count, run_id)?;
                if edits
                    .insert(route.to_vec(), addition.then(|| reference.clone()))
                    .is_some()
                {
                    return Err(corrupt("duplicate run route edit"));
                }
            }
            if path_edits.len() >= MAX_POSTING_PAGE_TERMS {
                *path_routes =
                    map_update(store, lease, path_routes, std::mem::take(&mut path_edits))?;
            }
            if content_edits.len() >= MAX_POSTING_PAGE_TERMS {
                *content_routes = map_update(
                    store,
                    lease,
                    content_routes,
                    std::mem::take(&mut content_edits),
                )?;
            }
            Ok(())
        },
    )?;
    if !path_edits.is_empty() {
        *path_routes = map_update(store, lease, path_routes, path_edits)?;
    }
    if !content_edits.is_empty() {
        *content_routes = map_update(store, lease, content_routes, content_edits)?;
    }
    Ok(())
}

fn flush_segment(
    store: &ObjectStore,
    lease: &AttemptLease,
    group: &[Prepared],
    segment_edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    current_edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<(), CoreError> {
    if group.is_empty() {
        return Ok(());
    }
    // Term derivation can take substantial CPU for an admitted 64-document
    // cohort. End any prior durable adoption group before that work.
    store.flush_adoption_batch()?;
    let scratch = group_gram_bytes(group)?;
    let ceiling = if group.len() == 1 {
        MAX_SINGLE_SOURCE_GRAM_BYTES
    } else {
        MAX_SEGMENT_GRAM_BYTES
    };
    if scratch > ceiling {
        return Err(invalid("physical segment gram scratch admission exceeded"));
    }
    let (path_terms, path_memberships) = write_term_map(store, lease, group, false)?;
    let (content_terms, content_memberships) = write_term_map(store, lease, group, true)?;
    let segment = Segment {
        doc_ids: group.iter().map(|item| item.head.logical_id).collect(),
        path_terms,
        content_terms,
        path_memberships,
        content_memberships,
    };
    let mut dependencies = Vec::new();
    for map in [&segment.path_terms, &segment.content_terms] {
        if let Some(page) = &map.page {
            dependencies.push(page.object.clone());
        }
    }
    let reference = store.put_blob(
        lease,
        ObjectKind::Segment,
        &dependencies,
        &segment.encode()?,
    )?;
    let digest = reference.root.sha256;
    let _previous = segment_edits.insert(digest.to_vec(), Some(reference));
    for (local, item) in group.iter().enumerate() {
        let locator = LiveDoc {
            source_key_sha256: item.key_digest,
            logical_id: item.head.logical_id,
            segment_sha256: digest,
            local_id: u32::try_from(local)
                .map_err(|error| invalid(&format!("local ID: {error}")))?,
        };
        let locator_blob = encode_record(store, lease, &locator.encode()?, &[])?;
        let _previous =
            current_edits.insert(doc_key(item.head.logical_id).to_vec(), Some(locator_blob));
    }
    Ok(())
}

fn inverted_grams(
    store: &ObjectStore,
    terms: &MapRoot,
    doc_count: usize,
    expected_memberships: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<Vec<[u8; 3]>>, CoreError> {
    compaction_checkpoint(budget, "file-compaction:invert-entry")?;
    let mut by_doc = vec![Vec::new(); doc_count];
    let mut cursor = store.map_cursor(terms, &[], None, budget)?;
    let mut cached: Option<(BlobRef, PostingPage)> = None;
    let mut scratch = 0_usize;
    let mut seen = 0_u64;
    while let Some((key, page_ref)) = cursor.next()? {
        compaction_checkpoint(budget, "file-compaction:invert-term")?;
        let gram: [u8; 3] = key
            .as_slice()
            .try_into()
            .map_err(|_width| corrupt("compaction term key is not a gram"))?;
        if cached
            .as_ref()
            .is_none_or(|(reference, _)| *reference != page_ref)
        {
            if page_ref.root.kind != ObjectKind::FilePostings {
                return Err(corrupt("compaction posting page kind differs"));
            }
            let bytes =
                store.read_blob(&page_ref, size_u64(super::MAX_POSTING_PAGE_BYTES)?, budget)?;
            cached = Some((page_ref, PostingPage::decode(&bytes, doc_count)?));
        }
        let (_, page) = cached
            .as_ref()
            .ok_or_else(|| corrupt("compaction page absent"))?;
        let index = page
            .0
            .binary_search_by_key(&gram, |row| row.0)
            .map_err(|_position| corrupt("compaction term map omits page gram"))?;
        let mask = page
            .0
            .get(index)
            .ok_or_else(|| corrupt("compaction term position is absent"))?
            .1;
        for local in 0..doc_count {
            compaction_checkpoint(budget, "file-compaction:invert-local")?;
            if mask & (1_u64 << local) == 0 {
                continue;
            }
            let grams = by_doc
                .get_mut(local)
                .ok_or_else(|| corrupt("compaction local ID exceeds segment"))?;
            seen = seen
                .checked_add(1)
                .ok_or_else(|| corrupt("compaction membership count overflow"))?;
            if seen > expected_memberships {
                return Err(corrupt("compaction terms exceed segment membership count"));
            }
            grams.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!(
                    "file authority v17: reserve compaction grams: {error}"
                ))
            })?;
            grams.push(gram);
            scratch = scratch
                .checked_add(3)
                .ok_or_else(|| invalid("compaction gram scratch overflow"))?;
            if size_u64(scratch)? > super::super::policy().bucket_scratch_bytes {
                return Err(invalid(
                    "compaction gram scratch exceeds existing admission",
                ));
            }
        }
    }
    if seen != expected_memberships {
        return Err(corrupt(
            "compaction terms differ from segment membership count",
        ));
    }
    for grams in &mut by_doc {
        compaction_checkpoint(budget, "file-compaction:invert-shrink")?;
        grams.shrink_to_fit();
    }
    Ok(by_doc)
}

fn compaction_checkpoint(
    budget: Option<&RequestBudgetV1>,
    stage: &'static str,
) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint(stage)?;
    }
    Ok(())
}

/// Owned, read-only work for the oldest queued File compaction. It is not an
/// adoption root and may be dropped without changing a generation or journal.
pub(crate) struct PreparedFileCompaction {
    digest: [u8; 32],
    reference: BlobRef,
    segment: Segment,
    current_refs: Vec<Option<BlobRef>>,
    survivors: Vec<Prepared>,
    policy_binding: Option<FilePacketBinding>,
}

struct FilePacketBinding {
    identity: StagedPacketIdentity,
    bounds: StagedPacketBounds,
    created_at: Instant,
}

fn compaction_preparation_budget(
    segment: &Segment,
    max_scratch_bytes: u64,
) -> Result<bool, CoreError> {
    let documents = size_u64(segment.doc_ids.len())?;
    let memberships = segment
        .path_memberships
        .checked_add(segment.content_memberships)
        .ok_or_else(|| invalid("compaction membership scratch overflow"))?;
    // Charge three simultaneous copies of gram storage: decoded inversion,
    // its possible shrink/reallocation, and the retained prepared packet.
    // Also charge transient authenticated record/page decodes and both row
    // vectors before any of the potentially large inversion allocations.
    // This is a codec/collection charge, not an allocator RSS guarantee.
    let row_width = std::mem::size_of::<Prepared>()
        .checked_add(std::mem::size_of::<Option<BlobRef>>())
        .and_then(|width| width.checked_add(std::mem::size_of::<(usize, LiveDoc)>()))
        .and_then(|width| width.checked_add(3 * super::MAX_RECORD_BYTES))
        .ok_or_else(|| invalid("compaction row scratch overflow"))?;
    let per_document = size_u64(row_width)?;
    let record_and_page = size_u64(
        super::MAX_RECORD_BYTES
            .checked_add(2 * super::MAX_POSTING_PAGE_BYTES)
            .ok_or_else(|| invalid("compaction record scratch overflow"))?,
    )?;
    let Some(required) = memberships
        .checked_mul(9)
        .and_then(|grams| {
            documents
                .checked_mul(per_document)
                .and_then(|rows| grams.checked_add(rows))
        })
        .and_then(|resident| resident.checked_add(record_and_page))
    else {
        return Ok(false);
    };
    // Preparation is optional. A cap miss keeps the exact foreground path;
    // it does not tighten the existing publication/codec admission.
    Ok(required <= max_scratch_bytes.min(super::super::policy().bucket_scratch_bytes))
}

fn compaction_survivors(
    store: &ObjectStore,
    original: &AuthorityRoot,
    digest: [u8; 32],
    segment: &Segment,
    current_edits: &BTreeMap<Vec<u8>, Option<BlobRef>>,
    force_rewrite: bool,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<Prepared>, CoreError> {
    compaction_checkpoint(budget, "file-compaction:survivors-entry")?;
    let mut survivors = Vec::new();
    survivors
        .try_reserve(segment.doc_ids.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "file authority v17: reserve compaction survivors: {error}"
            ))
        })?;
    for (local, id) in segment.doc_ids.iter().enumerate() {
        compaction_checkpoint(budget, "file-compaction:survivor-doc")?;
        if current_edits
            .get(&doc_key(*id).to_vec())
            .is_some_and(Option::is_none)
        {
            continue;
        }
        let Some(current_ref) = store.map_get(&original.current_docs, &doc_key(*id), budget)?
        else {
            // A queued debt can include a document retired in an earlier
            // publication; only currently mapped IDs belong in the output.
            continue;
        };
        if current_ref.root.kind != ObjectKind::Record {
            return Err(corrupt(
                "compaction current map value has wrong object kind",
            ));
        }
        let locator = LiveDoc::decode(&store.read_blob(
            &current_ref,
            size_u64(super::MAX_RECORD_BYTES)?,
            budget,
        )?)?;
        if locator.segment_sha256 == digest
            && usize::try_from(locator.local_id)
                .map_err(|error| corrupt(&format!("compaction local ID width: {error}")))?
                == local
        {
            survivors.push((local, locator));
        }
    }
    let rewrite = force_rewrite || survivors.len() != segment.doc_ids.len();
    let mut path = rewrite
        .then(|| {
            inverted_grams(
                store,
                &segment.path_terms,
                segment.doc_ids.len(),
                segment.path_memberships,
                budget,
            )
        })
        .transpose()?;
    let mut content = rewrite
        .then(|| {
            inverted_grams(
                store,
                &segment.content_terms,
                segment.doc_ids.len(),
                segment.content_memberships,
                budget,
            )
        })
        .transpose()?;
    let mut prepared = Vec::new();
    prepared.try_reserve(survivors.len()).map_err(|error| {
        CoreError::Storage(format!(
            "file authority v17: reserve compaction rows: {error}"
        ))
    })?;
    for (local, locator) in survivors {
        compaction_checkpoint(budget, "file-compaction:prepare-row")?;
        let head_ref = store
            .map_get(&original.source_heads, &locator.source_key_sha256, budget)?
            .ok_or_else(|| corrupt("compaction survivor source head absent"))?;
        if head_ref.root.kind != ObjectKind::Record {
            return Err(corrupt("compaction survivor head kind differs"));
        }
        let head = SourceHead::decode(&store.read_blob(
            &head_ref,
            size_u64(super::MAX_RECORD_BYTES)?,
            budget,
        )?)?;
        if head.logical_id != locator.logical_id
            || source_key_digest(&head.coverage.source.file)? != locator.source_key_sha256
        {
            return Err(corrupt("compaction survivor head and locator differ"));
        }
        let mut row = Prepared {
            key_digest: locator.source_key_sha256,
            head,
            head_blob: head_ref,
            path_grams: match path.as_mut() {
                Some(path) => std::mem::take(
                    path.get_mut(local)
                        .ok_or_else(|| corrupt("compaction path grams are absent"))?,
                ),
                None => Vec::new(),
            },
            content_grams: match content.as_mut() {
                Some(content) => std::mem::take(
                    content
                        .get_mut(local)
                        .ok_or_else(|| corrupt("compaction content grams are absent"))?,
                ),
                None => Vec::new(),
            },
        };
        if force_rewrite {
            // Keep the physical scratch charge identical to the authenticated
            // membership count used by clean-run selection and its verifier.
            row.path_grams = row.path_grams.into_boxed_slice().into_vec();
            row.content_grams = row.content_grams.into_boxed_slice().into_vec();
        }
        prepared.push(row);
    }
    Ok(prepared)
}

/// Read and derive the oldest debt without publishing or adopting an object.
/// The caller must retain the base against reclaim for this entire read.
pub(crate) fn prepare_compaction(
    store: &ObjectStore,
    base: &AuthorityRoot,
    max_scratch_bytes: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<PreparedFileCompaction>, CoreError> {
    compaction_checkpoint(budget, "file-compaction:prepare-entry")?;
    let mut cursor = store.map_cursor(&base.compaction_debt, &[], None, budget)?;
    let Some((key, reference)) = cursor.next()? else {
        return Ok(None);
    };
    let digest: [u8; 32] = key
        .as_slice()
        .try_into()
        .map_err(|_width| corrupt("compaction debt key is not a digest"))?;
    let committed = store
        .map_get(&base.segments, &digest, budget)?
        .ok_or_else(|| corrupt("compaction debt segment is absent"))?;
    if committed.root.kind != ObjectKind::Segment
        || committed.root.sha256 != digest
        || committed != reference
    {
        return Err(corrupt("compaction debt and segment references differ"));
    }
    let segment = Segment::decode(&store.read_blob(
        &reference,
        size_u64(super::MAX_RECORD_BYTES)?,
        budget,
    )?)?;
    compaction_checkpoint(budget, "file-compaction:before-cap")?;
    if !compaction_preparation_budget(&segment, max_scratch_bytes)? {
        return Ok(None);
    }
    let mut current_refs = Vec::new();
    current_refs
        .try_reserve(segment.doc_ids.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "file authority v17: reserve current witnesses: {error}"
            ))
        })?;
    for id in &segment.doc_ids {
        compaction_checkpoint(budget, "file-compaction:current-witness")?;
        current_refs.push(store.map_get(&base.current_docs, &doc_key(*id), budget)?);
    }
    let survivors = compaction_survivors(
        store,
        base,
        digest,
        &segment,
        &BTreeMap::new(),
        false,
        budget,
    )?;
    compaction_checkpoint(budget, "file-compaction:prepare-complete")?;
    Ok(Some(PreparedFileCompaction {
        digest,
        reference,
        segment,
        current_refs,
        survivors,
        policy_binding: None,
    }))
}

impl PreparedFileCompaction {
    pub(crate) fn bind_policy(
        mut self,
        base: &AuthorityRoot,
        policy: &LexicalMaintenancePolicy,
        encoded_bytes: u64,
    ) -> Result<Self, CoreError> {
        self.policy_binding = Some(FilePacketBinding {
            identity: StagedPacketIdentity::new(
                Sha256::digest(base.encode()?).into(),
                policy.policy_sha256(),
            )?,
            bounds: StagedPacketBounds {
                encoded_bytes,
                created_at_monotonic_millis: 0,
                input_runs: 1,
            },
            created_at: Instant::now(),
        });
        Ok(self)
    }

    pub(crate) fn policy_valid(
        &self,
        base: Option<&AuthorityRoot>,
        policy: Option<&LexicalMaintenancePolicy>,
    ) -> Result<bool, CoreError> {
        match (self.policy_binding.as_ref(), base, policy) {
            (None, Some(_), None) => Ok(true),
            (Some(binding), Some(base), Some(policy)) => {
                let age = u64::try_from(binding.created_at.elapsed().as_millis())
                    .map_err(|_width| invalid("File staged packet age exceeds u64"))?;
                Ok(policy
                    .check_staged_packet(
                        binding.identity,
                        Sha256::digest(base.encode()?).into(),
                        binding.bounds,
                        age,
                    )
                    .is_ok())
            }
            _ => Ok(false),
        }
    }

    fn still_matches(
        &self,
        store: &ObjectStore,
        base: &AuthorityRoot,
        current_edits: &BTreeMap<Vec<u8>, Option<BlobRef>>,
        digest: [u8; 32],
        reference: &BlobRef,
    ) -> Result<bool, CoreError> {
        if self.digest != digest || &self.reference != reference {
            return Ok(false);
        }
        for (id, expected) in self.segment.doc_ids.iter().zip(&self.current_refs) {
            let key = doc_key(*id);
            if current_edits.contains_key(key.as_slice())
                || store.map_get(&base.current_docs, &key, None)? != *expected
            {
                return Ok(false);
            }
        }
        for item in &self.survivors {
            if store.map_get(&base.source_heads, &item.key_digest, None)?
                != Some(item.head_blob.clone())
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Rewrite one touched or previously queued segment. A retired logical ID is
/// skipped; survivors retain IDs and acquire a new immutable segment locator.
struct CompactionWork<'a> {
    store: &'a ObjectStore,
    lease: &'a AttemptLease,
    original: &'a AuthorityRoot,
    current_edits: &'a mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    segment_edits: &'a mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    debt_edits: &'a mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    pending: &'a mut Vec<Prepared>,
}

fn compact_segment(
    work: CompactionWork<'_>,
    digest: [u8; 32],
    reference: &BlobRef,
    prepared_compaction: Option<PreparedFileCompaction>,
    maintenance_policy: Option<&LexicalMaintenancePolicy>,
) -> Result<(), CoreError> {
    let CompactionWork {
        store,
        lease,
        original,
        current_edits,
        segment_edits,
        debt_edits,
        pending,
    } = work;
    // Inversion and survivor validation are bounded, but can be much more
    // expensive than an object write. They do not require the adoption gate.
    store.flush_adoption_batch()?;
    let committed = read_segment_ref(store, &original.segments, digest)?;
    if &committed != reference {
        return Err(corrupt("compaction debt and segment references differ"));
    }
    let selected = match prepared_compaction {
        Some(plan)
            if plan.policy_valid(Some(original), maintenance_policy)?
                && plan.still_matches(store, original, current_edits, digest, reference)? =>
        {
            Some(plan)
        }
        _ => None,
    };
    let (segment, mut prepared) = if let Some(plan) = selected {
        (plan.segment, plan.survivors)
    } else {
        let segment = Segment::decode(&store.read_blob(
            reference,
            size_u64(super::MAX_RECORD_BYTES)?,
            None,
        )?)?;
        let prepared = compaction_survivors(
            store,
            original,
            digest,
            &segment,
            current_edits,
            false,
            None,
        )?;
        (segment, prepared)
    };
    let _previous = debt_edits.insert(digest.to_vec(), None);
    if prepared.len() == segment.doc_ids.len() {
        return Ok(());
    }
    let compacted_documents = size_u64(prepared.len())?;
    let compacted_bytes = prepared.iter().try_fold(0_u64, |total, item| {
        total
            .checked_add(item.head.source_bytes)
            .ok_or_else(|| invalid("compacted source byte count overflow"))
    })?;
    store.record_compaction(1, compacted_documents, compacted_bytes);
    let _previous = segment_edits.insert(digest.to_vec(), None);
    // New versions may fill the retired segment's vacant slots. This keeps
    // repeated one-file replacements from creating an extra run each time.
    let free = MAX_SEGMENT_DOCS
        .checked_sub(prepared.len())
        .ok_or_else(|| corrupt("compaction survivor count exceeds segment admission"))?;
    let mut scratch = group_gram_bytes(&prepared)?;
    let mut take = 0_usize;
    for item in pending.iter().take(free) {
        let next = scratch
            .checked_add(gram_bytes(item)?)
            .ok_or_else(|| invalid("compaction segment gram scratch overflow"))?;
        if next > MAX_SEGMENT_GRAM_BYTES {
            break;
        }
        scratch = next;
        take = take
            .checked_add(1)
            .ok_or_else(|| invalid("compaction pending count overflow"))?;
    }
    prepared.extend(pending.drain(..take));
    prepared.sort_by_key(|item| item.head.logical_id);
    flush_segment(store, lease, &prepared, segment_edits, current_edits)?;
    Ok(())
}

/// Authenticate one complete old cohort before selecting it for run-level
/// posting merge. A candidate touching an edit or inherited debt is skipped;
/// no segment is reclassified as a run.
fn eligible_clean_run(
    store: &ObjectStore,
    original: &AuthorityRoot,
    touched: &BTreeMap<[u8; 32], BlobRef>,
    candidate: &RunCandidate,
    policy: &LexicalMaintenancePolicy,
) -> Result<Option<u64>, CoreError> {
    match probe_clean_run(store, original, touched, candidate, policy, None)? {
        EligibilityWitness::FileEligible {
            effective_input_encoded_bytes,
        } => Ok(Some(effective_input_encoded_bytes)),
        EligibilityWitness::Ineligible(_) => Ok(None),
        EligibilityWitness::NativeEligible { .. } => Err(corrupt("Native eligibility in File run")),
    }
}

fn probe_clean_run(
    store: &ObjectStore,
    original: &AuthorityRoot,
    touched: &BTreeMap<[u8; 32], BlobRef>,
    candidate: &RunCandidate,
    policy: &LexicalMaintenancePolicy,
    budget: Option<&RequestBudgetV1>,
) -> Result<EligibilityWitness, CoreError> {
    if candidate.index_encoded_bytes > policy.max_merge_input_encoded_bytes() {
        return Ok(EligibilityWitness::Ineligible(IneligibleReason::InputCap));
    }
    let run_key = candidate.run_id.to_be_bytes();
    let reference = store
        .map_get(&original.run_postings, &run_key, budget)?
        .ok_or_else(|| corrupt("clean run has no posting root"))?;
    let postings = RunPostingRoot::load(store, &reference, budget)?;
    if postings.run_id != candidate.run_id {
        return Err(corrupt("clean run posting ID differs"));
    }
    let input_bytes = candidate
        .index_encoded_bytes
        .checked_add(postings.bundle_encoded_bytes(&reference, store)?)
        .ok_or_else(|| invalid("clean run input bytes overflow"))?;
    if input_bytes > policy.max_merge_input_encoded_bytes() {
        return Ok(EligibilityWitness::Ineligible(IneligibleReason::InputCap));
    }
    let mut eligible = true;
    original.run_registry.visit_run_members(
        store,
        candidate,
        budget,
        &mut |key, segment_ref, _bytes| {
            if let Some(budget) = budget {
                budget.checkpoint("lexical:file:hard-scan-member")?;
            }
            let digest: [u8; 32] = key
                .try_into()
                .map_err(|_width| corrupt("clean run member digest width differs"))?;
            if touched.contains_key(&digest)
                || store
                    .map_get(&original.compaction_debt, key, budget)?
                    .is_some()
            {
                eligible = false;
            }
            if store.map_get(&original.segments, key, budget)?.as_ref() != Some(segment_ref) {
                return Err(corrupt("clean run member differs from segment map"));
            }
            Ok(())
        },
    )?;
    Ok(if eligible {
        EligibilityWitness::FileEligible {
            effective_input_encoded_bytes: input_bytes,
        }
    } else {
        EligibilityWitness::Ineligible(IneligibleReason::TouchedOrDebt)
    })
}

/// Derive the old segment exclusions before any target/source object write.
///
/// This must agree with `build_for_seal_inner`'s touched set; the build checks
/// that equality again before using a completed hard-scan selection.
pub(crate) fn preflight_touched_segments(
    store: &ObjectStore,
    original: &AuthorityRoot,
    mutation: &FileAuthorityMutation,
    budget: Option<&RequestBudgetV1>,
) -> Result<BTreeMap<[u8; 32], BlobRef>, CoreError> {
    if mutation.clear_all {
        return Ok(BTreeMap::new());
    }
    let mut touched = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for edit in &mutation.edits {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:file:hard-scan:changed-source")?;
        }
        let key = edit.key();
        key.validate().map_err(invalid)?;
        let source_digest = source_key_digest(key)?;
        if !seen.insert(source_digest) {
            return Err(invalid("duplicate or colliding changed source digest"));
        }
        let Some((_, old)) = read_head(store, &original.source_heads, key)? else {
            continue;
        };
        let locator = read_live(store, &original.current_docs, old.logical_id)?;
        if locator.source_key_sha256 != source_digest || locator.logical_id != old.logical_id {
            return Err(corrupt("source and current locator differ"));
        }
        let reference = read_segment_ref(store, &original.segments, locator.segment_sha256)?;
        let changed = match edit {
            FileSourceEdit::Delete { .. } => true,
            FileSourceEdit::Upsert { coverage, bytes } => {
                old.derived_input_sha256 != derived_digest(coverage)?
                    || old.coverage != *coverage
                    || old.body.sha256 != <[u8; 32]>::from(Sha256::digest(bytes))
            }
        };
        if changed {
            let _previous = touched.insert(locator.segment_sha256, reference);
        }
    }
    Ok(touched)
}

fn file_header_sha256(
    store: &ObjectStore,
    original: &AuthorityRoot,
    candidate: &RunCandidate,
    budget: Option<&RequestBudgetV1>,
) -> Result<[u8; 32], CoreError> {
    let key = candidate.run_id.to_be_bytes();
    let reference = store
        .map_get(&original.run_registry.runs, &key, budget)?
        .ok_or_else(|| corrupt("hard scan run header disappeared"))?;
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("hard scan run header kind differs"));
    }
    let mut hash = Sha256::new();
    hash.update(b"lexical-file-hard-scan-header-v1\0");
    hash.update(key);
    hash.update(reference.root.sha256);
    Ok(hash.finalize().into())
}

/// Probe at most 32 immutable old headers and persist no target mutation.
/// The caller owns checkpoint append and independent replay.
pub(crate) fn prepare_hard_scan_window(
    store: &ObjectStore,
    base: &AuthorityRoot,
    mutation: &FileAuthorityMutation,
    policy: &LexicalMaintenancePolicy,
    progress: RunSelectionProgress,
    budget: Option<&RequestBudgetV1>,
) -> Result<WindowWitness, CoreError> {
    if mutation.clear_all || progress.pending_run.is_some() {
        return Err(invalid("hard scan requires a non-clear frozen base"));
    }
    let high = base
        .run_registry
        .next_run_id
        .checked_sub(1)
        .ok_or_else(|| corrupt("hard scan frontier underflows"))?;
    if high == 0
        || (progress.scan_high_water_run_id != 0 && progress.scan_high_water_run_id != high)
    {
        return Err(invalid("hard scan frontier differs from immutable base"));
    }
    let touched = preflight_touched_segments(store, base, mutation, budget)?;
    let window = base
        .run_registry
        .clone()
        .with_selection_progress(progress)?
        .scan_hard_window(store, 32, budget)?;
    if window.scan_high_water_run_id != high {
        return Err(corrupt("hard scan window changed frozen frontier"));
    }
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(window.candidates.len())
        .map_err(|error| CoreError::Storage(format!("reserve File hard scan window: {error}")))?;
    for candidate in &window.candidates {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:file:hard-scan-candidate")?;
        }
        candidates.push(CandidateWitness {
            run_id: candidate.run_id,
            index_encoded_bytes: candidate.index_encoded_bytes,
            eligibility: probe_clean_run(store, base, &touched, candidate, policy, budget)?,
            header_sha256: file_header_sha256(store, base, candidate, budget)?,
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

#[cfg(test)]
pub(crate) fn test_hard_scan_lane(
    store: &ObjectStore,
    base: &AuthorityRoot,
    mutation: &FileAuthorityMutation,
    policy: &LexicalMaintenancePolicy,
) -> Result<Option<ScanLane>, CoreError> {
    let before = base.maintenance_snapshot(store)?;
    if !policy.merge_due(before) || !policy.hard_pressure(before) {
        return Ok(None);
    }
    let high = base
        .run_registry
        .next_run_id
        .checked_sub(1)
        .ok_or_else(|| corrupt("test File hard scan frontier underflows"))?;
    let mut progress = RunSelectionProgress::default();
    let mut windows = Vec::new();
    loop {
        let mut window = prepare_hard_scan_window(store, base, mutation, policy, progress, None)?;
        window.ordinal = u16::try_from(windows.len())
            .map_err(|_width| corrupt("test File hard scan window count differs"))?;
        progress = RunSelectionProgress {
            next_scan_run_id: window.next_run_id,
            scan_high_water_run_id: high,
            pending_run: None,
        };
        let done = window.round_complete;
        windows.push(window);
        if done {
            break;
        }
    }
    Ok(Some(ScanLane {
        authority: ScanAuthority::File,
        base_authority_root_sha256: Sha256::digest(base.encode()?).into(),
        frozen_high_water_run_id: high,
        windows,
    }))
}

fn select_clean_runs(
    store: &ObjectStore,
    original: &AuthorityRoot,
    touched: &BTreeMap<[u8; 32], BlobRef>,
    policy: &LexicalMaintenancePolicy,
    hard_scan: Option<&ScanLane>,
) -> Result<CleanRunSelection, CoreError> {
    let prior = original.run_registry.selection_progress();
    let before = original.maintenance_snapshot(store)?;
    if !policy.merge_due(before)
        || run_postings::run_merge_stage_bytes(2)? > policy.max_staging_bytes()
    {
        return Ok(CleanRunSelection {
            run_ids: Vec::new(),
            progress: prior,
        });
    }
    let mut selected = Vec::new();
    let mut input_bytes = 0_u64;
    let max_fan_in = usize::try_from(policy.max_merge_fan_in())
        .map_err(|_width| invalid("run fan-in width differs"))?;
    let hard = policy.hard_pressure(before);
    let window = if hard {
        let lane = hard_scan.ok_or_else(|| invalid("hard recovery checkpoint is absent"))?;
        let root_sha256: [u8; 32] = Sha256::digest(original.encode()?).into();
        let high = original
            .run_registry
            .next_run_id
            .checked_sub(1)
            .ok_or_else(|| corrupt("hard recovery frontier underflows"))?;
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
            return Err(invalid(
                "hard recovery checkpoint base or completion differs",
            ));
        }
        // The checkpoint's independent oracle has authenticated each old
        // candidate. Derive the same first feasible pair and subsequent
        // followers from its bounded scalar witnesses, without rescanning
        // every old posting body after source mutation.
        let mut earlier = Vec::<(u64, u64)>::new();
        let mut hard_selected = Vec::new();
        let mut hard_bytes = 0_u64;
        let mut next = 1_u64;
        let mut seen = 0_u64;
        let mut costs = BTreeMap::new();
        for (ordinal, witnessed) in lane.windows.iter().enumerate() {
            if usize::from(witnessed.ordinal) != ordinal
                || witnessed.start_run_id != next
                || witnessed.frozen_high_water_run_id != high
                || witnessed.candidates.len() > 32
                || (witnessed.round_complete && ordinal.checked_add(1) != Some(lane.windows.len()))
                || (!witnessed.round_complete && witnessed.next_run_id <= next)
                || (witnessed.round_complete && witnessed.next_run_id != 1)
            {
                return Err(corrupt("hard recovery checkpoint cursor differs"));
            }
            let mut last_id = next.saturating_sub(1);
            for candidate in &witnessed.candidates {
                if candidate.run_id <= last_id
                    || candidate.run_id > high
                    || (!witnessed.round_complete && candidate.run_id >= witnessed.next_run_id)
                    || candidate.header_sha256 == [0; 32]
                {
                    return Err(corrupt("hard recovery checkpoint candidate order differs"));
                }
                last_id = candidate.run_id;
                seen = seen
                    .checked_add(1)
                    .ok_or_else(|| corrupt("hard recovery count overflows"))?;
                if seen > quanta_index_core::lexical_maintenance::MAX_HARD_SCAN_RUNS {
                    return Err(corrupt("hard recovery candidate bound differs"));
                }
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
                    return Err(corrupt("hard recovery candidate cost differs"));
                }
                let _prior = costs.insert(candidate.run_id, bytes);
                if hard_selected.len() >= max_fan_in {
                    continue;
                }
                if !hard_selected.is_empty() {
                    if let Some(total) = hard_bytes.checked_add(bytes)
                        && total <= policy.max_merge_input_encoded_bytes()
                        && run_postings::run_merge_stage_bytes(
                            hard_selected.len().checked_add(1).ok_or_else(|| {
                                invalid("hard recovery selected run fan-in overflows")
                            })?,
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
            next = witnessed.next_run_id;
        }
        for &id in &hard_selected {
            let candidate = original
                .run_registry
                .candidate_by_id(store, id, None)?
                .ok_or_else(|| corrupt("selected hard run disappeared"))?;
            if eligible_clean_run(store, original, touched, &candidate, policy)?
                != costs.get(&id).copied()
            {
                return Err(corrupt("selected hard run cost or eligibility differs"));
            }
        }
        selected = hard_selected;
        let last = lane
            .windows
            .last()
            .ok_or_else(|| corrupt("hard recovery checkpoint ended without window"))?;
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
            let Some(bytes) = eligible_clean_run(store, original, touched, candidate, policy)?
            else {
                return Ok(false);
            };
            let Some(next_bytes) = input_bytes.checked_add(bytes) else {
                return Ok(false);
            };
            if next_bytes > policy.max_merge_input_encoded_bytes()
                || run_postings::run_merge_stage_bytes(
                    selected
                        .len()
                        .checked_add(1)
                        .ok_or_else(|| invalid("selected run fan-in overflows"))?,
                )? > policy.max_staging_bytes()
            {
                return Ok(false);
            }
            input_bytes = next_bytes;
            selected.push(candidate.run_id);
            Ok(selected.len() >= max_fan_in)
        };
        if let Some(pending) = prior.pending_run
            && let Some(candidate) = original
                .run_registry
                .candidate_by_id(store, pending, None)?
        {
            let _full = consider(&candidate)?;
        }
        let window = original.run_registry.scan_window(
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

fn publish_clean_run_merge(
    store: &ObjectStore,
    lease: &AttemptLease,
    output: &mut AuthorityRoot,
    selection: &CleanRunSelection,
    policy: &LexicalMaintenancePolicy,
) -> Result<(), CoreError> {
    if selection.run_ids.len() < 2 {
        output.run_registry = output
            .run_registry
            .clone()
            .with_selection_progress(selection.progress)?;
        return Ok(());
    }
    let new_id = output.run_registry.next_run_id;
    let mut inputs = Vec::new();
    let mut edits = BTreeMap::new();
    for id in &selection.run_ids {
        let key = id.to_be_bytes().to_vec();
        let reference = store
            .map_get(&output.run_postings, &key, None)?
            .ok_or_else(|| corrupt("selected run has no posting root"))?;
        let run = RunPostingRoot::load(store, &reference, None)?;
        if run.run_id != *id {
            return Err(corrupt("selected run posting ID differs"));
        }
        output.run_index_encoded_bytes = output
            .run_index_encoded_bytes
            .checked_sub(run.bundle_encoded_bytes(&reference, store)?)
            .ok_or_else(|| corrupt("clean run byte ledger underflows"))?;
        update_run_routes(
            store,
            lease,
            &mut output.path_routes,
            &mut output.content_routes,
            *id,
            Some(&run),
            None,
        )?;
        let _previous = edits.insert(key, None);
        inputs.push(run);
    }
    let (merged, merged_ref) =
        merge_run_posting_roots(store, lease, new_id, &inputs, policy.max_staging_bytes())?;
    output.run_index_encoded_bytes = output
        .run_index_encoded_bytes
        .checked_add(merged.bundle_encoded_bytes(&merged_ref, store)?)
        .ok_or_else(|| invalid("merged run byte ledger overflows"))?;
    update_run_routes(
        store,
        lease,
        &mut output.path_routes,
        &mut output.content_routes,
        new_id,
        None,
        Some(&merged),
    )?;
    let _previous = edits.insert(new_id.to_be_bytes().to_vec(), Some(merged_ref));
    output.run_postings = map_update(store, lease, &output.run_postings, edits)?;
    output.premerge_run_registry = Some(output.run_registry.clone());
    output.run_registry =
        output
            .run_registry
            .merge_cohorts(store, lease, &selection.run_ids, selection.progress)?;
    Ok(())
}

/// Only touched source heads are decoded. Inherited map subtrees are retained
/// by authenticated references, never reconstructed as a full source vector.
#[cfg(test)]
pub(crate) fn build_for_seal(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
) -> Result<FileAuthorityBuild, CoreError> {
    store.with_adoption_batch(lease, |batched_store| {
        build_for_seal_inner(
            batched_store,
            lease,
            base,
            reservation,
            mutation,
            None,
            None,
            None,
        )
    })
}

#[cfg(test)]
pub(crate) fn build_for_seal_prepared(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
    prepared: Option<PreparedFileCompaction>,
) -> Result<FileAuthorityBuild, CoreError> {
    build_for_seal_prepared_with_policy(
        store,
        lease,
        base,
        reservation,
        mutation,
        prepared,
        None,
        None,
    )
}

pub(crate) fn build_for_seal_prepared_with_policy(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
    prepared: Option<PreparedFileCompaction>,
    maintenance_policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<FileAuthorityBuild, CoreError> {
    store.with_adoption_batch(lease, |batched_store| {
        build_for_seal_inner(
            batched_store,
            lease,
            base,
            reservation,
            mutation,
            prepared,
            maintenance_policy,
            hard_scan,
        )
    })
}

fn build_for_seal_inner(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&AuthorityRoot>,
    reservation: &DocVersionReservation,
    mutation: &FileAuthorityMutation,
    mut prepared_compaction: Option<PreparedFileCompaction>,
    maintenance_policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<FileAuthorityBuild, CoreError> {
    reservation.validate()?;
    if mutation.operations_sha256 != lease.binding().operations_sha256
        || mutation.edits.windows(2).any(|pair| {
            pair.first().map(FileSourceEdit::key) >= pair.get(1).map(FileSourceEdit::key)
        })
    {
        return Err(invalid(
            "mutation operation, sort order or reservation differs",
        ));
    }
    if let Some(base) = base
        && base.allocation_scope_sha256 != reservation.scope_sha256
    {
        return Err(invalid("base and reservation allocation scopes differ"));
    }
    let empty = AuthorityRoot {
        policy_sha256: policy_digest(),
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
    let original = if mutation.clear_all {
        &empty
    } else {
        base.unwrap_or(&empty)
    };
    let expected_touched = hard_scan
        .map(|_lane| preflight_touched_segments(store, original, mutation, None))
        .transpose()?;
    let mut output = original.clone();
    // The intermediate registry witnesses this publication only. A later
    // generation must not inherit an earlier merge's proof state.
    output.premerge_run_registry = None;
    let mut source_edits = BTreeMap::new();
    let mut current_edits = BTreeMap::new();
    let mut member_edits = BTreeMap::new();
    let mut segment_edits = BTreeMap::new();
    let mut debt_edits = BTreeMap::new();
    let mut touched_segments = BTreeMap::<[u8; 32], BlobRef>::new();
    let mut prepared = Vec::new();
    let mut digests = BTreeSet::new();
    let mut allocated = 0_u64;
    for cohort in mutation.edits.chunks(MAX_SEGMENT_DOCS) {
        // Prior writes must not hold the publication gate through a long
        // inherited/no-op scan. Tiny files still share a bounded cohort.
        store.flush_adoption_batch()?;
        for edit in cohort {
            let key = edit.key();
            key.validate().map_err(invalid)?;
            let digest = source_key_digest(key)?;
            if !digests.insert(digest) {
                return Err(invalid("duplicate or colliding changed source digest"));
            }
            let prior = read_head(store, &original.source_heads, key)?;
            let mut retired_segment = None;
            if let Some((_, old)) = &prior {
                let locator = read_live(store, &original.current_docs, old.logical_id)?;
                if locator.source_key_sha256 != digest || locator.logical_id != old.logical_id {
                    return Err(corrupt("source and current locator differ"));
                }
                let segment = read_segment_ref(store, &original.segments, locator.segment_sha256)?;
                retired_segment = Some((locator.segment_sha256, segment));
                checked_sub(&mut output.live_sources, 1, "live source")?;
                checked_sub(&mut output.live_docs, 1, "live document")?;
                checked_sub(&mut output.source_bytes, old.source_bytes, "source bytes")?;
                checked_sub(
                    &mut output.membership_count,
                    old.posting_memberships,
                    "memberships",
                )?;
                let _previous = current_edits.insert(doc_key(old.logical_id).to_vec(), None);
                let _previous =
                    member_edits.insert(membership_key(digest, old.logical_id).to_vec(), None);
            }
            match edit {
                FileSourceEdit::Delete { .. } => {
                    if let Some((digest, reference)) = retired_segment {
                        let _previous = touched_segments.insert(digest, reference.clone());
                        let _previous = debt_edits.insert(digest.to_vec(), Some(reference));
                    }
                    let _previous = source_edits.insert(digest.to_vec(), None);
                }
                FileSourceEdit::Upsert { coverage, bytes } => {
                    let expected_derived = derived_digest(coverage)?;
                    if let Some((_, old)) = &prior
                        && old.derived_input_sha256 == expected_derived
                        && old.coverage == *coverage
                        && old.body.sha256 == <[u8; 32]>::from(Sha256::digest(bytes))
                    {
                        // Exact source and producer contract inheritance consumes
                        // no new logical ID or object. Restore summary decrements.
                        checked_add(
                            &mut output.live_sources,
                            1,
                            super::super::policy().source_files,
                            "live source",
                        )?;
                        checked_add(
                            &mut output.live_docs,
                            1,
                            super::super::policy().source_files,
                            "live document",
                        )?;
                        checked_add(
                            &mut output.source_bytes,
                            old.source_bytes,
                            super::super::policy().source_bytes,
                            "source bytes",
                        )?;
                        checked_add(
                            &mut output.membership_count,
                            old.posting_memberships,
                            super::super::policy().total_memberships,
                            "memberships",
                        )?;
                        let _previous = current_edits.remove(doc_key(old.logical_id).as_slice());
                        let _previous =
                            member_edits.remove(membership_key(digest, old.logical_id).as_slice());
                        continue;
                    }
                    if allocated >= reservation.count {
                        return Err(invalid("document-version reservation exhausted"));
                    }
                    let id = reservation
                        .first
                        .checked_add(allocated)
                        .ok_or_else(|| invalid("document ID overflow"))?;
                    allocated = allocated
                        .checked_add(1)
                        .ok_or_else(|| invalid("reservation use overflow"))?;
                    if bytes.len() >= ADOPTION_CPU_FLUSH_BYTES {
                        store.flush_adoption_batch()?;
                    }
                    let item = prepare_upsert(store, lease, coverage, bytes, id)?;
                    if let Some((digest, reference)) = retired_segment {
                        let _previous = touched_segments.insert(digest, reference.clone());
                        let _previous = debt_edits.insert(digest.to_vec(), Some(reference));
                    }
                    checked_add(
                        &mut output.live_sources,
                        1,
                        super::super::policy().source_files,
                        "live source",
                    )?;
                    checked_add(
                        &mut output.live_docs,
                        1,
                        super::super::policy().source_files,
                        "live document",
                    )?;
                    checked_add(
                        &mut output.source_bytes,
                        item.head.source_bytes,
                        super::super::policy().source_bytes,
                        "source bytes",
                    )?;
                    checked_add(
                        &mut output.membership_count,
                        item.head.posting_memberships,
                        super::super::policy().total_memberships,
                        "memberships",
                    )?;
                    let _previous =
                        source_edits.insert(digest.to_vec(), Some(item.head_blob.clone()));
                    let _previous = member_edits.insert(
                        membership_key(digest, id).to_vec(),
                        Some(item.head_blob.clone()),
                    );
                    let item_scratch = gram_bytes(&item)?;
                    if !prepared.is_empty()
                        && group_gram_bytes(&prepared)?
                            .checked_add(item_scratch)
                            .is_none_or(|total| total > MAX_SEGMENT_GRAM_BYTES)
                    {
                        flush_segment(
                            store,
                            lease,
                            &prepared,
                            &mut segment_edits,
                            &mut current_edits,
                        )?;
                        prepared.clear();
                    }
                    prepared.push(item);
                    if prepared.len() == MAX_SEGMENT_DOCS || item_scratch > MAX_SEGMENT_GRAM_BYTES {
                        flush_segment(
                            store,
                            lease,
                            &prepared,
                            &mut segment_edits,
                            &mut current_edits,
                        )?;
                        prepared.clear();
                    }
                }
            }
        }
    }
    store.flush_adoption_batch()?;
    for (digest, reference) in &touched_segments {
        compact_segment(
            CompactionWork {
                store,
                lease,
                original,
                current_edits: &mut current_edits,
                segment_edits: &mut segment_edits,
                debt_edits: &mut debt_edits,
                pending: &mut prepared,
            },
            *digest,
            reference,
            None,
            maintenance_policy,
        )?;
    }
    if !mutation.clear_all {
        store.flush_adoption_batch()?;
        let mut cursor = store.map_cursor(&original.compaction_debt, &[], None, None)?;
        while let Some((key, reference)) = cursor.next()? {
            let digest: [u8; 32] = key
                .as_slice()
                .try_into()
                .map_err(|_width| corrupt("compaction debt key is not a digest"))?;
            if touched_segments.contains_key(&digest) {
                continue;
            }
            compact_segment(
                CompactionWork {
                    store,
                    lease,
                    original,
                    current_edits: &mut current_edits,
                    segment_edits: &mut segment_edits,
                    debt_edits: &mut debt_edits,
                    pending: &mut prepared,
                },
                digest,
                &reference,
                prepared_compaction.take(),
                maintenance_policy,
            )?;
            break;
        }
    }
    flush_segment(
        store,
        lease,
        &prepared,
        &mut segment_edits,
        &mut current_edits,
    )?;
    prepared.clear();
    if expected_touched
        .as_ref()
        .is_some_and(|expected| expected != &touched_segments)
    {
        return Err(corrupt("hard scan touched source set differs from build"));
    }
    let clean_selection = if let Some(policy) = maintenance_policy
        && !mutation.clear_all
    {
        Some(select_clean_runs(
            store,
            original,
            &touched_segments,
            policy,
            hard_scan,
        )?)
    } else {
        None
    };
    output.source_heads = map_update(store, lease, &original.source_heads, source_edits)?;
    output.current_docs = map_update(store, lease, &original.current_docs, current_edits)?;
    output.memberships = map_update(store, lease, &original.memberships, member_edits)?;
    output.segments = map_update(store, lease, &original.segments, segment_edits)?;
    output.compaction_debt = map_update(store, lease, &original.compaction_debt, debt_edits)?;
    store.flush_adoption_batch()?;
    // Derive the exact changed segment set from authenticated map roots. A
    // touched segment whose descriptor remains identical does not create a
    // new logical run or change its attributed index bytes.
    let mut removed = Vec::new();
    let mut added = Vec::new();
    store.map_changed_keys(
        &original.segments,
        &output.segments,
        None,
        &mut |key, before, after| {
            if let Some(reference) = before {
                removed.push(SegmentRemoval {
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
                added.push(SegmentInput {
                    key: key.to_vec(),
                    reference: reference.clone(),
                    index_encoded_bytes,
                });
            }
            Ok(())
        },
    )?;
    output.run_registry = original
        .run_registry
        .apply(store, lease, &removed, &added)?;
    // A changed physical leaf affects only its old/new owning run. Apply
    // leaf term deltas in bounded gram batches, then rewrite only changed
    // run route counts; inherited run posting dictionaries are reused.
    let mut per_run = BTreeMap::<u64, Vec<(bool, BlobRef)>>::new();
    for segment in &removed {
        let digest: [u8; 32] = segment
            .key
            .as_slice()
            .try_into()
            .map_err(|_width| corrupt("removed run leaf digest width differs"))?;
        let run_id = original.run_registry.owner_run_id(store, digest, None)?;
        per_run
            .entry(run_id)
            .or_default()
            .push((true, segment.reference.clone()));
    }
    for segment in &added {
        let digest: [u8; 32] = segment
            .key
            .as_slice()
            .try_into()
            .map_err(|_width| corrupt("added run leaf digest width differs"))?;
        let run_id = output.run_registry.owner_run_id(store, digest, None)?;
        per_run
            .entry(run_id)
            .or_default()
            .push((false, segment.reference.clone()));
    }
    let mut run_edits = BTreeMap::new();
    for (run_id, segments) in per_run {
        let key = run_id.to_be_bytes().to_vec();
        let old_ref = store.map_get(&original.run_postings, &key, None)?;
        let old_run = old_ref
            .as_ref()
            .map(|reference| RunPostingRoot::load(store, reference, None))
            .transpose()?;
        if old_run.as_ref().is_some_and(|root| root.run_id != run_id) {
            return Err(corrupt("base run posting root ID differs"));
        }
        if let (Some(root), Some(reference)) = (&old_run, &old_ref) {
            output.run_index_encoded_bytes = output
                .run_index_encoded_bytes
                .checked_sub(root.bundle_encoded_bytes(reference, store)?)
                .ok_or_else(|| corrupt("run bundle ledger underflows"))?;
        }
        // Only a genuinely new, addition-only run can use the streaming
        // bootstrap. Existing runs keep the exact prior subtraction path.
        // IDs at or above the original next-run boundary cannot belong to
        // the original registry. Its old-candidate API deliberately rejects
        // such newly issued IDs instead of returning an absent candidate.
        let fresh = old_ref.is_none()
            && run_id >= original.run_registry.next_run_id
            && segments.iter().all(|(remove, _)| !remove);
        let (next, prebuilt_ref) = if fresh {
            let mut references = Vec::new();
            references
                .try_reserve_exact(segments.len())
                .map_err(|error| CoreError::Storage(format!("reserve new run leaves: {error}")))?;
            references.extend(segments.iter().map(|(_, reference)| reference));
            let (root, reference) = bootstrap_new_run_postings(
                store,
                lease,
                run_id,
                &references,
                super::super::policy().bucket_scratch_bytes,
            )?;
            (Some(root), Some(reference))
        } else {
            let mut next = old_run.clone();
            for (remove, reference) in segments {
                if reference.root.kind != ObjectKind::Segment {
                    return Err(corrupt("run leaf has wrong object kind"));
                }
                let segment = Segment::decode(&store.read_blob(
                    &reference,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    None,
                )?)?;
                next = Some(apply_segment_to_run(
                    store,
                    lease,
                    next,
                    run_id,
                    &segment,
                    reference.root.sha256,
                    remove,
                )?);
            }
            (next, None)
        };
        let active = output
            .run_registry
            .candidate_by_id(store, run_id, None)?
            .is_some();
        if prebuilt_ref.is_some() && !active {
            return Err(corrupt("new run leaves have no live registry owner"));
        }
        let new_ref = if active {
            let root = next
                .as_ref()
                .ok_or_else(|| corrupt("live run has no posting root"))?;
            let reference = if let Some(reference) = prebuilt_ref {
                reference
            } else {
                store.put_blob(
                    lease,
                    ObjectKind::Record,
                    &root.object_roots(),
                    &root.encode()?,
                )?
            };
            output.run_index_encoded_bytes = output
                .run_index_encoded_bytes
                .checked_add(root.bundle_encoded_bytes(&reference, store)?)
                .ok_or_else(|| invalid("run bundle ledger overflows"))?;
            Some(reference)
        } else {
            None
        };
        update_run_routes(
            store,
            lease,
            &mut output.path_routes,
            &mut output.content_routes,
            run_id,
            old_run.as_ref(),
            if active { next.as_ref() } else { None },
        )?;
        let _previous = run_edits.insert(key, new_ref);
    }
    output.run_postings = map_update(store, lease, &original.run_postings, run_edits)?;
    if let (Some(policy), Some(selection)) = (maintenance_policy, clean_selection) {
        publish_clean_run_merge(store, lease, &mut output, &selection, policy)?;
    }
    let root_bytes = output.encode()?;
    Ok(FileAuthorityBuild {
        root: output,
        root_bytes,
    })
}

#[cfg(test)]
mod prepared_compaction_tests {
    use super::*;
    use crate::file_authority::paged::{PagedFileAuthority, verify_all_for_scrub, verify_for_seal};
    use crate::file_authority::{PostingSurface, QueryWork};
    use crate::lexical_store::AttemptBinding;
    use crate::text_authority::NativeRoot;
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileRevision, SymbolCoverage,
        SymbolNameSourcePolicyV1,
    };
    use quanta_index_core::RequestBudgetV1;
    use quanta_index_core::lexical_maintenance::TargetCounters;
    use quanta_index_lq_trigram::NecessaryGramExpr;

    fn coverage(path: &str, body: &[u8]) -> SourceFileCoverage {
        SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("repo").expect("repo ID"),
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("revision").expect("revision ID"),
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
            source_event_sha256: [u8::try_from(target_generation).expect("test generation"); 32],
            target_generation,
            base_root_sha256: None,
            operations_sha256: [4; 32],
            format: 17,
        }
    }

    fn one_edit(
        store: &ObjectStore,
        lease: &AttemptLease,
        root: &MapRoot,
        key: Vec<u8>,
        value: Option<BlobRef>,
    ) -> MapRoot {
        store
            .map_apply(lease, root, std::iter::once(Ok(MapEdit { key, value })))
            .expect("one map edit")
    }

    // Construct a coherent, previously published retirement debt state. The
    // foreground writer normally drains touched debt in the same publication,
    // so this fixture isolates the inherited-debt contract exercised here.
    fn retire_into_debt(
        store: &ObjectStore,
        lease: &AttemptLease,
        root: &AuthorityRoot,
        key: &SourceFileKey,
    ) -> AuthorityRoot {
        let (head_ref, head) = read_head(store, &root.source_heads, key)
            .expect("head read")
            .expect("retired source head");
        let locator = read_live(store, &root.current_docs, head.logical_id).expect("locator");
        let segment_ref = read_segment_ref(store, &root.segments, locator.segment_sha256)
            .expect("segment reference");
        let digest = source_key_digest(key).expect("source digest");
        let mut debt = root.clone();
        debt.source_heads = one_edit(store, lease, &root.source_heads, digest.to_vec(), None);
        debt.current_docs = one_edit(
            store,
            lease,
            &root.current_docs,
            doc_key(head.logical_id).to_vec(),
            None,
        );
        debt.memberships = one_edit(
            store,
            lease,
            &root.memberships,
            membership_key(digest, head.logical_id).to_vec(),
            None,
        );
        debt.compaction_debt = one_edit(
            store,
            lease,
            &root.compaction_debt,
            locator.segment_sha256.to_vec(),
            Some(segment_ref),
        );
        assert_eq!(head_ref.root.kind, ObjectKind::Record);
        debt.live_sources = debt.live_sources.checked_sub(1).expect("source count");
        debt.live_docs = debt.live_docs.checked_sub(1).expect("document count");
        debt.source_bytes = debt
            .source_bytes
            .checked_sub(head.source_bytes)
            .expect("source bytes");
        debt.membership_count = debt
            .membership_count
            .checked_sub(head.posting_memberships)
            .expect("membership count");
        verify_all_for_scrub(store, &debt, None).expect("retirement debt is independently valid");
        debt
    }

    fn assert_query_ids(store: &ObjectStore, root_bytes: &[u8], expected: &BTreeSet<u64>) {
        let authority = PagedFileAuthority::open(root_bytes, store.clone()).expect("reader");
        assert_eq!(
            authority.len(),
            u64::try_from(expected.len()).expect("count")
        );
        let mut work = QueryWork::default();
        let candidates = authority
            .necessary_candidates(
                PostingSurface::Content,
                &NecessaryGramExpr::Gram(*b"abc"),
                &mut work,
                &RequestBudgetV1::unbounded(),
            )
            .expect("content candidates");
        assert_eq!(candidates, Some(expected.iter().copied().collect()));
    }

    #[test]
    fn new_run_bootstrap_preserves_routes_ledger_and_pinned_old_views_through_delta() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let initial_mutation = FileAuthorityMutation {
            edits: (0_u64..2)
                .map(|index| FileSourceEdit::Upsert {
                    coverage: coverage(&format!("file-{index}.rs"), b"abcxyz"),
                    bytes: b"abcxyz".to_vec(),
                })
                .collect(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let initial_reservation = DocVersionReservation {
            first: 1,
            count: 2,
            scope_sha256: [5; 32],
        };
        let initial = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(1), &[])
                .expect("initial attempt"),
            None,
            &initial_reservation,
            &initial_mutation,
        )
        .expect("initial build");
        verify_for_seal(
            &store,
            &initial.root,
            None,
            &initial_reservation,
            &initial_mutation,
        )
        .expect("initial independent seal");
        verify_all_for_scrub(&store, &initial.root, None).expect("initial scrub");
        assert!(initial.root.path_routes.entries > 0);
        assert!(initial.root.content_routes.entries > 0);
        assert!(initial.root.run_index_encoded_bytes > 0);
        assert_query_ids(
            &store,
            &initial.root_bytes,
            &[1_u64, 2].into_iter().collect(),
        );

        let append_mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("file-2.rs", b"abcxyz"),
                bytes: b"abcxyz".to_vec(),
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let append_reservation = DocVersionReservation {
            first: 3,
            count: 1,
            scope_sha256: [5; 32],
        };
        let appended = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(2), &[])
                .expect("append attempt"),
            Some(&initial.root),
            &append_reservation,
            &append_mutation,
        )
        .expect("new-run append");
        verify_for_seal(
            &store,
            &appended.root,
            Some(&initial.root),
            &append_reservation,
            &append_mutation,
        )
        .expect("append independent seal");
        verify_all_for_scrub(&store, &appended.root, None).expect("append scrub");
        assert_query_ids(
            &store,
            &appended.root_bytes,
            &[1_u64, 2, 3].into_iter().collect(),
        );

        let delete_mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Delete {
                key: coverage("file-0.rs", b"abcxyz").source.file,
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let delete_reservation = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [5; 32],
        };
        let deleted = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(3), &[])
                .expect("delete attempt"),
            Some(&appended.root),
            &delete_reservation,
            &delete_mutation,
        )
        .expect("old-run deletion");
        verify_for_seal(
            &store,
            &deleted.root,
            Some(&appended.root),
            &delete_reservation,
            &delete_mutation,
        )
        .expect("delete independent seal");
        verify_all_for_scrub(&store, &deleted.root, None).expect("delete scrub");
        assert_query_ids(
            &store,
            &deleted.root_bytes,
            &[2_u64, 3].into_iter().collect(),
        );
        assert_query_ids(
            &store,
            &initial.root_bytes,
            &[1_u64, 2].into_iter().collect(),
        );
        assert_query_ids(
            &store,
            &appended.root_bytes,
            &[1_u64, 2, 3].into_iter().collect(),
        );
        let clear_mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage: coverage("file-fresh.rs", b"abcxyz"),
                bytes: b"abcxyz".to_vec(),
            }],
            clear_all: true,
            operations_sha256: [4; 32],
        };
        let clear_reservation = DocVersionReservation {
            first: 4,
            count: 1,
            scope_sha256: [5; 32],
        };
        let cleared = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(4), &[])
                .expect("clear attempt"),
            Some(&deleted.root),
            &clear_reservation,
            &clear_mutation,
        )
        .expect("new-run clear replacement");
        verify_for_seal(
            &store,
            &cleared.root,
            Some(&deleted.root),
            &clear_reservation,
            &clear_mutation,
        )
        .expect("clear independent seal");
        verify_all_for_scrub(&store, &cleared.root, None).expect("clear scrub");
        assert_query_ids(
            &store,
            &cleared.root_bytes,
            &std::iter::once(4_u64).collect(),
        );
        assert_query_ids(
            &store,
            &initial.root_bytes,
            &[1_u64, 2].into_iter().collect(),
        );
        assert_query_ids(
            &store,
            &appended.root_bytes,
            &[1_u64, 2, 3].into_iter().collect(),
        );
        assert_query_ids(
            &store,
            &deleted.root_bytes,
            &[2_u64, 3].into_iter().collect(),
        );
    }

    #[test]
    fn three_full_segment_preparations_match_foreground_and_preserve_unrelated_edits() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let initial = FileAuthorityMutation {
            edits: (0_u64..64)
                .map(|index| FileSourceEdit::Upsert {
                    coverage: coverage(&format!("file-{index:02}.rs"), b"abcxyz"),
                    bytes: b"abcxyz".to_vec(),
                })
                .collect(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let first = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(1), &[])
                .expect("initial attempt"),
            None,
            &DocVersionReservation {
                first: 1,
                count: 64,
                scope_sha256: [5; 32],
            },
            &initial,
        )
        .expect("initial full segment");
        let mut root = first.root;
        let mut expected = (1_u64..=64).collect::<BTreeSet<_>>();
        for cycle in 0_u64..3 {
            let retiring = coverage(&format!("file-{cycle:02}.rs"), b"abcxyz")
                .source
                .file;
            let debt = retire_into_debt(
                &store,
                &store
                    .begin_attempt(&binding(10 + 3 * cycle), &[])
                    .expect("debt attempt"),
                &root,
                &retiring,
            );
            assert!(
                prepare_compaction(&store, &debt, 1, None)
                    .expect("small optional cap")
                    .is_none()
            );
            if cycle == 0 {
                let cancelled = RequestBudgetV1::unbounded();
                cancelled.cancel_handle().cancel();
                assert!(matches!(
                    prepare_compaction(&store, &debt, 1, Some(&cancelled)),
                    Err(CoreError::Typed { code, .. })
                        if code == quanta_index_core::REQUEST_CANCELLED_CODE
                ));
            }
            let plan = prepare_compaction(&store, &debt, 62 * 1024 * 1024, None)
                .expect("bounded preparation")
                .expect("full segment fits optional cap");
            assert_eq!(plan.segment.doc_ids.len(), 64);
            assert_eq!(plan.survivors.len(), 63);
            let replacement_id = 65 + cycle;
            let mutation = FileAuthorityMutation {
                edits: vec![FileSourceEdit::Upsert {
                    coverage: coverage(&format!("replacement-{cycle:02}.rs"), b"abcxyz"),
                    bytes: b"abcxyz".to_vec(),
                }],
                clear_all: false,
                operations_sha256: [4; 32],
            };
            let reservation = DocVersionReservation {
                first: replacement_id,
                count: 1,
                scope_sha256: [5; 32],
            };
            let baseline = build_for_seal(
                &store,
                &store
                    .begin_attempt(&binding(11 + 3 * cycle), &[])
                    .expect("baseline attempt"),
                Some(&debt),
                &reservation,
                &mutation,
            )
            .expect("foreground compaction");
            let prepared = build_for_seal_prepared(
                &store,
                &store
                    .begin_attempt(&binding(12 + 3 * cycle), &[])
                    .expect("prepared attempt"),
                Some(&debt),
                &reservation,
                &mutation,
                Some(plan),
            )
            .expect("prepared compaction");
            assert_eq!(prepared.root_bytes, baseline.root_bytes);
            verify_for_seal(&store, &prepared.root, Some(&debt), &reservation, &mutation)
                .expect("independent seal proof");
            verify_all_for_scrub(&store, &prepared.root, None).expect("independent scrub proof");
            assert!(expected.remove(&(cycle + 1)));
            assert!(expected.insert(replacement_id));
            assert_query_ids(&store, &baseline.root_bytes, &expected);
            assert_query_ids(&store, &prepared.root_bytes, &expected);
            let retired_reader = PagedFileAuthority::open(&prepared.root_bytes, store.clone())
                .expect("prepared reader");
            assert_eq!(
                retired_reader
                    .id_for_key(&retiring, None)
                    .expect("retired source identity"),
                None
            );
            let replacement = coverage(&format!("replacement-{cycle:02}.rs"), b"abcxyz")
                .source
                .file;
            assert_eq!(
                retired_reader
                    .id_for_key(&replacement, None)
                    .expect("replacement source identity"),
                Some(replacement_id)
            );
            root = prepared.root;
        }
    }

    #[test]
    fn touched_debt_discards_preparation_and_preserves_delete() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let initial = FileAuthorityMutation {
            edits: (0_u64..3)
                .map(|index| FileSourceEdit::Upsert {
                    coverage: coverage(&format!("file-{index}.rs"), b"abcxyz"),
                    bytes: b"abcxyz".to_vec(),
                })
                .collect(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let initial_root = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(1), &[])
                .expect("initial attempt"),
            None,
            &DocVersionReservation {
                first: 1,
                count: 3,
                scope_sha256: [5; 32],
            },
            &initial,
        )
        .expect("initial build")
        .root;
        let first_key = coverage("file-0.rs", b"abcxyz").source.file;
        let debt = retire_into_debt(
            &store,
            &store.begin_attempt(&binding(2), &[]).expect("debt attempt"),
            &initial_root,
            &first_key,
        );
        let plan = prepare_compaction(&store, &debt, 62 * 1024 * 1024, None)
            .expect("preparation")
            .expect("debt plan");
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Delete {
                key: coverage("file-1.rs", b"abcxyz").source.file,
            }],
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let reservation = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [5; 32],
        };
        let baseline = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(3), &[])
                .expect("baseline attempt"),
            Some(&debt),
            &reservation,
            &mutation,
        )
        .expect("baseline delete");
        let prepared = build_for_seal_prepared(
            &store,
            &store
                .begin_attempt(&binding(4), &[])
                .expect("prepared attempt"),
            Some(&debt),
            &reservation,
            &mutation,
            Some(plan),
        )
        .expect("prepared delete");
        assert_eq!(prepared.root_bytes, baseline.root_bytes);
        verify_for_seal(&store, &prepared.root, Some(&debt), &reservation, &mutation)
            .expect("independent seal proof");
        verify_all_for_scrub(&store, &prepared.root, None).expect("independent scrub proof");
        assert_query_ids(
            &store,
            &prepared.root_bytes,
            &std::iter::once(3_u64).collect(),
        );
    }

    #[test]
    fn two_full_leaf_runs_merge_one_serving_dictionary_without_repacking_leaves() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let make_batch = |start: u64| FileAuthorityMutation {
            edits: (start..start + 64)
                .map(|index| FileSourceEdit::Upsert {
                    coverage: coverage(&format!("file-{index:03}.rs"), b"abcxyz"),
                    bytes: b"abcxyz".to_vec(),
                })
                .collect(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let first = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(1), &[])
                .expect("first attempt"),
            None,
            &DocVersionReservation {
                first: 1,
                count: 64,
                scope_sha256: [5; 32],
            },
            &make_batch(0),
        )
        .expect("first full leaf run");
        let second = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(2), &first.root.object_roots())
                .expect("second attempt"),
            Some(&first.root),
            &DocVersionReservation {
                first: 65,
                count: 64,
                scope_sha256: [5; 32],
            },
            &make_batch(64),
        )
        .expect("second full leaf run");
        assert_eq!(second.root.run_registry.runs.entries, 2);
        assert_eq!(second.root.segments.entries, 2);
        let held = second.root_bytes.clone();
        let no_changes = FileAuthorityMutation {
            edits: Vec::new(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let zero = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [5; 32],
        };
        let policy = LexicalMaintenancePolicy::new(
            2,
            4,
            10_000_000,
            20_000_000,
            2,
            100_000_000,
            1_000_000,
            1,
        )
        .expect("test policy");
        let hard_scan = test_hard_scan_lane(&store, &second.root, &no_changes, &policy)
            .expect("File hard scan fixture");
        let merged = build_for_seal_prepared_with_policy(
            &store,
            &store
                .begin_attempt(&binding(3), &second.root.object_roots())
                .expect("merge attempt"),
            Some(&second.root),
            &zero,
            &no_changes,
            None,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("run posting merge");
        assert_eq!(merged.root.run_registry.runs.entries, 1);
        assert_eq!(merged.root.run_postings.entries, 1);
        assert_eq!(merged.root.segments, second.root.segments);
        let expected = (1_u64..=128).collect::<BTreeSet<_>>();
        assert_query_ids(&store, &held, &expected);
        assert_query_ids(&store, &merged.root_bytes, &expected);
        let _merge_facts = crate::file_authority::paged::verify_for_seal_with_merge_facts(
            &store,
            &merged.root,
            Some(&second.root),
            &zero,
            &no_changes,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("independent run merge seal");
        verify_all_for_scrub(&store, &merged.root, None).expect("merged run scrub");
        let leaf_only = merged.root.run_registry.active_index_encoded_bytes;
        let strict_bytes =
            LexicalMaintenancePolicy::new(1, 2, leaf_only, leaf_only, 2, 100_000_000, 1_000_000, 1)
                .expect("leaf-only comparison policy");
        let empty = TargetCounters::new(0, 0, 0).expect("empty counters");
        strict_bytes
            .admit_target(
                empty,
                TargetCounters::new(1, 2, leaf_only).expect("leaf-only counters"),
            )
            .expect("old subset accounting would admit exact boundary");
        let empty_native = NativeRoot::empty([5; 32]);
        assert!(
            crate::lexical_publication::admit_maintenance_roots(
                &store,
                &strict_bytes,
                (None, &merged.root),
                (None, &empty_native),
                false,
            )
            .is_err(),
            "actual run dictionary and outer map bytes exceed leaf-only admission"
        );
    }

    #[test]
    fn expired_file_preparation_falls_back_at_compaction_consumption() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let initial = FileAuthorityMutation {
            edits: (0..3)
                .map(|index| FileSourceEdit::Upsert {
                    coverage: coverage(&format!("packet-{index}.rs"), b"abcxyz"),
                    bytes: b"abcxyz".to_vec(),
                })
                .collect(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let initial_root = build_for_seal(
            &store,
            &store
                .begin_attempt(&binding(1), &[])
                .expect("initial attempt"),
            None,
            &DocVersionReservation {
                first: 1,
                count: 3,
                scope_sha256: [5; 32],
            },
            &initial,
        )
        .expect("initial build")
        .root;
        let retiring = coverage("packet-0.rs", b"abcxyz").source.file;
        let debt = retire_into_debt(
            &store,
            &store.begin_attempt(&binding(2), &[]).expect("debt attempt"),
            &initial_root,
            &retiring,
        );
        let policy = LexicalMaintenancePolicy::new(
            2,
            4,
            1_000_000,
            2_000_000,
            2,
            10_000_000,
            62 * 1024 * 1024,
            1,
        )
        .expect("packet policy");
        let mut packet = prepare_compaction(&store, &debt, 62 * 1024 * 1024, None)
            .expect("bounded preparation")
            .expect("debt packet")
            .bind_policy(&debt, &policy, 62 * 1024 * 1024)
            .expect("packet binding");
        packet.policy_binding.as_mut().expect("binding").created_at = Instant::now()
            .checked_sub(std::time::Duration::from_millis(3))
            .expect("monotonic instant headroom");
        assert!(
            !packet
                .policy_valid(Some(&debt), Some(&policy))
                .expect("expired packet check")
        );
        let no_changes = FileAuthorityMutation {
            edits: Vec::new(),
            clear_all: false,
            operations_sha256: [4; 32],
        };
        let zero = DocVersionReservation {
            first: 0,
            count: 0,
            scope_sha256: [5; 32],
        };
        let hard_scan = test_hard_scan_lane(&store, &debt, &no_changes, &policy)
            .expect("File hard scan fixture");
        let baseline = build_for_seal_prepared_with_policy(
            &store,
            &store
                .begin_attempt(&binding(3), &debt.object_roots())
                .expect("baseline attempt"),
            Some(&debt),
            &zero,
            &no_changes,
            None,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("foreground fallback");
        let expired = build_for_seal_prepared_with_policy(
            &store,
            &store
                .begin_attempt(&binding(4), &debt.object_roots())
                .expect("expired attempt"),
            Some(&debt),
            &zero,
            &no_changes,
            Some(packet),
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("expired packet fallback");
        assert_eq!(expired.root_bytes, baseline.root_bytes);
        let _merge_facts = crate::file_authority::paged::verify_for_seal_with_merge_facts(
            &store,
            &expired.root,
            Some(&debt),
            &zero,
            &no_changes,
            Some(&policy),
            hard_scan.as_ref(),
        )
        .expect("independent fallback seal");
    }
}
