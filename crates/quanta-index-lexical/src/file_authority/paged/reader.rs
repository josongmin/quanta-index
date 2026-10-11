//! Fallible V17 source and posting lookup. No source or term census is built
//! during open; selected map paths and authenticated blobs are read on demand.

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{SourceFileKey, SourceFileRevision};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::NecessaryGramExpr;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::lexical_store::{BlobRef, ObjectKind, ObjectStore};

use super::{
    AuthorityRoot, LiveDoc, PostingPage, Segment, SourceHead, doc_key, policy_digest, route_prefix,
    source_key_digest,
};
use crate::file_authority::query::PostingSurface;
use crate::file_authority::query::QueryWork;
use crate::file_authority::{SourceFile, share_verified_surfaces};

#[derive(Clone, Debug)]
pub(crate) struct FileMetadata {
    pub(crate) source: SourceFileRevision,
    pub(crate) text_admitted: bool,
    pub(crate) language: LanguageCode,
    pub(crate) indexed_path: String,
    pub(crate) folded_path: String,
    pub(crate) source_bytes: usize,
    pub(crate) indexed_text_bytes: usize,
    pub(crate) folded_text_bytes: usize,
    body: BlobRef,
    pub(crate) logical_id: u64,
}

pub(crate) struct PagedFileAuthority {
    pub(crate) root: AuthorityRoot,
    store: ObjectStore,
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("file authority v17 reader: {reason}"),
    }
}

fn plan_limit(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
        message: format!("file authority v17 reader: {reason}"),
    }
}

fn size_u64(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value).map_err(|error| corrupt(&format!("byte length exceeds u64: {error}")))
}

fn record_bytes(
    store: &ObjectStore,
    reference: &BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<u8>, CoreError> {
    if reference.root.kind != ObjectKind::Record {
        return Err(corrupt("map value is not a record"));
    }
    store.read_blob(reference, size_u64(super::MAX_RECORD_BYTES)?, budget)
}

fn source_by_digest(
    store: &ObjectStore,
    root: &AuthorityRoot,
    digest: [u8; 32],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<SourceHead>, CoreError> {
    let Some(reference) = store.map_get(&root.source_heads, &digest, budget)? else {
        return Ok(None);
    };
    let head = SourceHead::decode(&record_bytes(store, &reference, budget)?)?;
    if source_key_digest(&head.coverage.source.file)? != digest {
        return Err(corrupt("source head differs from digest map key"));
    }
    Ok(Some(head))
}

fn live_doc(
    store: &ObjectStore,
    root: &AuthorityRoot,
    id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<LiveDoc>, CoreError> {
    let Some(reference) = store.map_get(&root.current_docs, &doc_key(id), budget)? else {
        return Ok(None);
    };
    let row = LiveDoc::decode(&record_bytes(store, &reference, budget)?)?;
    if row.logical_id != id {
        return Err(corrupt("current map key and logical ID differ"));
    }
    Ok(Some(row))
}

fn metadata(head: SourceHead) -> Result<FileMetadata, CoreError> {
    let scratch = usize::try_from(super::super::policy().bucket_scratch_bytes)
        .map_err(|error| corrupt(&format!("normalization scratch width: {error}")))?;
    let (indexed_path, folded_path, _, _) = super::super::normalized_surfaces(
        head.coverage.source.file.repo_relative_path.as_str(),
        None,
        0,
        scratch,
    )?;
    Ok(FileMetadata {
        source: head.coverage.source,
        text_admitted: head.coverage.text_admitted,
        language: head.coverage.language,
        indexed_path,
        folded_path,
        source_bytes: usize::try_from(head.source_bytes)
            .map_err(|error| corrupt(&format!("source byte width: {error}")))?,
        indexed_text_bytes: usize::try_from(head.indexed_text_bytes)
            .map_err(|error| corrupt(&format!("indexed byte width: {error}")))?,
        folded_text_bytes: usize::try_from(head.folded_text_bytes)
            .map_err(|error| corrupt(&format!("folded byte width: {error}")))?,
        body: head.body,
        logical_id: head.logical_id,
    })
}

impl PagedFileAuthority {
    pub(crate) fn open(root_bytes: &[u8], store: ObjectStore) -> Result<Self, CoreError> {
        let root = AuthorityRoot::decode(root_bytes)?;
        if root.policy_sha256 != policy_digest() {
            return Err(corrupt("policy digest differs"));
        }
        Ok(Self { root, store })
    }

    pub(crate) fn len(&self) -> u64 {
        self.root.live_sources
    }

    /// One `CodeSearch` request shares this meter across prefilter, exact body
    /// verification, and selected preview reads.
    pub(crate) fn query_scope(&self) -> Self {
        Self {
            root: self.root.clone(),
            store: self.store.metered_clone(),
        }
    }

    pub(crate) fn query_io_snapshot(&self) -> (u64, u64) {
        self.store.read_meter_snapshot()
    }

    fn check_query_io(&self) -> Result<(), CoreError> {
        self.check_query_io_with_policy(super::super::policy())
    }

    fn check_query_io_with_policy(
        &self,
        policy: super::super::FilePolicy,
    ) -> Result<(), CoreError> {
        if !self.store.has_read_meter() {
            return Ok(());
        }
        let (bytes, reads) = self.store.read_meter_snapshot();
        if reads > super::super::query_physical_read_limit(policy)?
            || bytes > policy.query_decoded_bytes
        {
            return Err(plan_limit("whole-request authenticated I/O exceeds policy"));
        }
        Ok(())
    }

    fn charge_query_io_since(
        &self,
        work: &mut QueryWork,
        snapshot: &mut (u64, u64),
    ) -> Result<(), CoreError> {
        let after = self.store.read_meter_snapshot();
        work.charge_paged_io(super::super::policy(), *snapshot, after)?;
        self.check_query_io()?;
        *snapshot = after;
        Ok(())
    }

    /// Retained authority metadata only. Selected source and map-page buffers
    /// are request-local and are accounted by their independent query limits.
    pub(crate) fn heap_bytes_estimate(&self) -> Result<u64, CoreError> {
        let encoded = self.root.encode()?;
        let retained = std::mem::size_of::<Self>()
            .checked_add(encoded.len())
            .and_then(|bytes| bytes.checked_add(16 * 1024))
            .ok_or_else(|| {
                CoreError::Storage("lexical: paged file authority heap estimate overflow".into())
            })?;
        u64::try_from(retained).map_err(|error| {
            CoreError::Storage(format!("lexical: paged file authority heap width: {error}"))
        })
    }

    pub(crate) fn metadata_for_key(
        &self,
        key: &SourceFileKey,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<FileMetadata>, CoreError> {
        key.validate()
            .map_err(|error| corrupt(&format!("source key: {error}")))?;
        let head = source_by_digest(&self.store, &self.root, source_key_digest(key)?, budget)?;
        self.check_query_io()?;
        let Some(head) = head else {
            return Ok(None);
        };
        if head.coverage.source.file != *key {
            return Err(corrupt(
                "source key hash collides with a different identity",
            ));
        }
        Ok(Some(metadata(head)?))
    }

    pub(crate) fn metadata_for_id(
        &self,
        id: u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<FileMetadata>, CoreError> {
        let locator = live_doc(&self.store, &self.root, id, budget)?;
        self.check_query_io()?;
        let Some(locator) = locator else {
            return Ok(None);
        };
        let head = source_by_digest(&self.store, &self.root, locator.source_key_sha256, budget)?
            .ok_or_else(|| corrupt("current document omits source head"))?;
        self.check_query_io()?;
        if head.logical_id != id {
            return Err(corrupt("current document and source head ID differ"));
        }
        Ok(Some(metadata(head)?))
    }

    #[cfg(test)]
    pub(crate) fn id_for_key(
        &self,
        key: &SourceFileKey,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Option<u64>, CoreError> {
        Ok(self
            .metadata_for_key(key, budget)?
            .map(|metadata| metadata.logical_id))
    }

    pub(crate) fn scan_sources(
        &self,
        budget: &RequestBudgetV1,
        visitor: &mut dyn FnMut(FileMetadata) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        self.scan_sources_with_policy(budget, visitor, super::super::policy())
    }

    fn scan_sources_with_policy(
        &self,
        budget: &RequestBudgetV1,
        visitor: &mut dyn FnMut(FileMetadata) -> Result<(), CoreError>,
        policy: super::super::FilePolicy,
    ) -> Result<(), CoreError> {
        self.store.map_scan(
            &self.root.source_heads,
            &[],
            None,
            Some(budget),
            &mut |key, reference| {
                budget.checkpoint("lexical:file-source-scan")?;
                let digest: [u8; 32] = key
                    .try_into()
                    .map_err(|_detail| corrupt("source map key is not a SHA-256 digest"))?;
                let head =
                    SourceHead::decode(&record_bytes(&self.store, reference, Some(budget))?)?;
                self.check_query_io_with_policy(policy)?;
                if source_key_digest(&head.coverage.source.file)? != digest {
                    return Err(corrupt("scanned source digest differs"));
                }
                visitor(metadata(head)?)
            },
        )?;
        self.check_query_io_with_policy(policy)
    }

    pub(crate) fn load_file(
        &self,
        file: &FileMetadata,
        budget: &RequestBudgetV1,
    ) -> Result<SourceFile, CoreError> {
        budget.checkpoint("lexical:file-body-read")?;
        if file.body.root.kind != ObjectKind::FileBody {
            return Err(corrupt("source body has wrong object kind"));
        }
        let bytes = self.store.read_blob(
            &file.body,
            size_u64(super::super::MAX_FILE_BYTES)?,
            Some(budget),
        )?;
        self.check_query_io()?;
        if bytes.len() != file.source_bytes
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != file.source.source_sha256
        {
            return Err(corrupt("selected source differs from source revision"));
        }
        let raw = if file.text_admitted {
            Some(
                std::str::from_utf8(&bytes)
                    .map_err(|_detail| corrupt("text source is not UTF-8"))?,
            )
        } else {
            None
        };
        let scratch = usize::try_from(super::super::policy().bucket_scratch_bytes)
            .map_err(|error| corrupt(&format!("normalization scratch width: {error}")))?;
        let (indexed_path, folded_path, indexed_text, folded_text) =
            super::super::normalized_surfaces(
                file.source.file.repo_relative_path.as_str(),
                raw,
                0,
                scratch,
            )?;
        if indexed_path != file.indexed_path
            || folded_path != file.folded_path
            || indexed_text.as_ref().map_or(0, String::len) != file.indexed_text_bytes
            || folded_text.as_ref().map_or(0, String::len) != file.folded_text_bytes
        {
            return Err(corrupt(
                "selected source normalization differs from source head",
            ));
        }
        let (bytes, indexed_text, folded_text) =
            share_verified_surfaces(&bytes, raw, indexed_text, folded_text);
        Ok(SourceFile {
            source: file.source.clone(),
            bytes,
            #[cfg(test)]
            text_admitted: file.text_admitted,
            #[cfg(test)]
            language: file.language.clone(),
            indexed_text,
            folded_text,
            indexed_path,
            folded_path,
        })
    }

    pub(crate) fn load_file_for_key(
        &self,
        key: &SourceFileKey,
        budget: &RequestBudgetV1,
    ) -> Result<SourceFile, CoreError> {
        let file = self
            .metadata_for_key(key, Some(budget))?
            .ok_or_else(|| corrupt("selected source key is absent"))?;
        self.load_file(&file, budget)
    }

    /// A selected preview owns one completed body decode even if later
    /// projection fails. Metadata and posting reads do not change this count.
    pub(crate) fn load_preview_file_for_key(
        &self,
        key: &SourceFileKey,
        budget: &RequestBudgetV1,
        files_decoded: &mut u64,
    ) -> Result<SourceFile, CoreError> {
        let file = self.load_file_for_key(key, budget)?;
        *files_decoded = files_decoded.saturating_add(1);
        Ok(file)
    }

    pub(crate) fn load_file_for_id(
        &self,
        id: u64,
        budget: &RequestBudgetV1,
    ) -> Result<SourceFile, CoreError> {
        let file = self
            .metadata_for_id(id, Some(budget))?
            .ok_or_else(|| corrupt("selected source ID is absent"))?;
        self.load_file(&file, budget)
    }

    fn posting_ids(
        &self,
        surface: PostingSurface,
        gram: [u8; 3],
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<u64>, CoreError> {
        let mut io_before = self.store.read_meter_snapshot();
        let surface_code = match surface {
            PostingSurface::Path => 1,
            PostingSurface::Content => 2,
        };
        let map = match surface {
            PostingSurface::Path => &self.root.path_routes,
            PostingSurface::Content => &self.root.content_routes,
        };
        let lower = route_prefix(surface_code, gram);
        let upper = prefix_successor(&lower);
        let mut ids = Vec::new();
        self.store.map_scan(
            map,
            &lower,
            upper.as_deref(),
            Some(budget),
            &mut |key, segment_ref| {
                self.charge_query_io_since(work, &mut io_before)?;
                work.charge_paged_probe(super::super::policy())?;
                if key.len() != 40
                    || key.get(..4) != Some(lower.as_slice())
                    || segment_ref.root.kind != ObjectKind::Segment
                    || key.get(8..) != Some(segment_ref.root.sha256.as_slice())
                {
                    return Err(corrupt("route key and segment reference differ"));
                }
                let segment = Segment::decode(&self.store.read_blob(
                    segment_ref,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    Some(budget),
                )?)?;
                self.charge_query_io_since(work, &mut io_before)?;
                let terms = match surface {
                    PostingSurface::Path => &segment.path_terms,
                    PostingSurface::Content => &segment.content_terms,
                };
                let page_ref = self
                    .store
                    .map_get(terms, &gram, Some(budget))?
                    .ok_or_else(|| corrupt("route omits segment posting term"))?;
                self.charge_query_io_since(work, &mut io_before)?;
                if page_ref.root.kind != ObjectKind::FilePostings {
                    return Err(corrupt("posting page has wrong object kind"));
                }
                let bytes = self.store.read_blob(
                    &page_ref,
                    size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                    Some(budget),
                )?;
                self.charge_query_io_since(work, &mut io_before)?;
                let page = PostingPage::decode(&bytes, segment.doc_ids.len())?;
                let index = page
                    .0
                    .binary_search_by_key(&gram, |row| row.0)
                    .map_err(|_detail| corrupt("selected posting page omits routed gram"))?;
                let local_mask = page
                    .0
                    .get(index)
                    .ok_or_else(|| corrupt("selected posting term position absent"))?
                    .1;
                let route_count = u32::from_be_bytes(
                    key.get(4..8)
                        .ok_or_else(|| corrupt("route posting count is absent"))?
                        .try_into()
                        .map_err(|_detail| corrupt("route posting count width differs"))?,
                );
                if route_count != local_mask.count_ones() {
                    return Err(corrupt("route posting count differs from selected page"));
                }
                work.charge_paged_ids(super::super::policy(), u64::from(local_mask.count_ones()))?;
                for local in 0..segment.doc_ids.len() {
                    if local_mask & (1_u64 << local) == 0 {
                        continue;
                    }
                    let logical_id = *segment
                        .doc_ids
                        .get(local)
                        .ok_or_else(|| corrupt("posting local ID outside segment"))?;
                    let current = live_doc(&self.store, &self.root, logical_id, Some(budget))?;
                    self.charge_query_io_since(work, &mut io_before)?;
                    if let Some(current) = current
                        && current.segment_sha256 == segment_ref.root.sha256
                        && usize::try_from(current.local_id)
                            .map_err(|error| corrupt(&format!("local ID width: {error}")))?
                            == local
                    {
                        ids.try_reserve(1).map_err(|error| {
                            CoreError::Storage(format!(
                                "file authority v17: reserve candidate ID: {error}"
                            ))
                        })?;
                        ids.push(logical_id);
                    }
                }
                Ok(())
            },
        )?;
        ids.sort_unstable();
        ids.dedup();
        self.charge_query_io_since(work, &mut io_before)?;
        Ok(ids)
    }

    fn posting_ids_within(
        &self,
        surface: PostingSurface,
        gram: [u8; 3],
        allowed: &[u64],
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<u64>, CoreError> {
        let mut io_before = self.store.read_meter_snapshot();
        let mut by_segment = BTreeMap::<[u8; 32], Vec<(u32, u64)>>::new();
        for id in allowed {
            budget.checkpoint("lexical:file-filtered-gram-candidate")?;
            let current = live_doc(&self.store, &self.root, *id, Some(budget))?
                .ok_or_else(|| corrupt("candidate ID lost current locator"))?;
            self.charge_query_io_since(work, &mut io_before)?;
            let selected = by_segment.entry(current.segment_sha256).or_default();
            selected.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!(
                    "file authority v17: reserve filtered candidates: {error}"
                ))
            })?;
            selected.push((current.local_id, *id));
        }
        let mut output = Vec::new();
        output.try_reserve(allowed.len()).map_err(|error| {
            CoreError::Storage(format!(
                "file authority v17: reserve filtered result: {error}"
            ))
        })?;
        let surface_code = if surface == PostingSurface::Path {
            1
        } else {
            2
        };
        let routes = if surface == PostingSurface::Path {
            &self.root.path_routes
        } else {
            &self.root.content_routes
        };
        for (digest, selected) in by_segment {
            work.charge_paged_probe(super::super::policy())?;
            let segment_ref = self
                .store
                .map_get(&self.root.segments, &digest, Some(budget))?
                .ok_or_else(|| corrupt("candidate segment is absent"))?;
            self.charge_query_io_since(work, &mut io_before)?;
            if segment_ref.root.kind != ObjectKind::Segment || segment_ref.root.sha256 != digest {
                return Err(corrupt("candidate segment key differs from descriptor"));
            }
            let segment = Segment::decode(&self.store.read_blob(
                &segment_ref,
                size_u64(super::MAX_RECORD_BYTES)?,
                Some(budget),
            )?)?;
            self.charge_query_io_since(work, &mut io_before)?;
            let terms = if surface == PostingSurface::Path {
                &segment.path_terms
            } else {
                &segment.content_terms
            };
            let Some(page_ref) = self.store.map_get(terms, &gram, Some(budget))? else {
                self.charge_query_io_since(work, &mut io_before)?;
                continue;
            };
            self.charge_query_io_since(work, &mut io_before)?;
            if page_ref.root.kind != ObjectKind::FilePostings {
                return Err(corrupt("filtered posting page kind differs"));
            }
            let bytes = self.store.read_blob(
                &page_ref,
                size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                Some(budget),
            )?;
            self.charge_query_io_since(work, &mut io_before)?;
            let page = PostingPage::decode(&bytes, segment.doc_ids.len())?;
            let index = page
                .0
                .binary_search_by_key(&gram, |row| row.0)
                .map_err(|_detail| corrupt("filtered posting page omits term"))?;
            let local_mask = page
                .0
                .get(index)
                .ok_or_else(|| corrupt("filtered posting term position absent"))?
                .1;
            let count = local_mask.count_ones();
            let route = self.store.map_get(
                routes,
                &super::route_key(surface_code, gram, count, digest),
                Some(budget),
            )?;
            self.charge_query_io_since(work, &mut io_before)?;
            if route.as_ref() != Some(&segment_ref) {
                return Err(corrupt("filtered gram route is absent or differs"));
            }
            // Only selected logical IDs are materialized. Physical page bytes
            // and decoded IDs remain charged to the shared query work budget.
            work.charge_paged_ids(super::super::policy(), u64::from(count))?;
            for (local, id) in selected {
                let local_index = usize::try_from(local)
                    .map_err(|error| corrupt(&format!("filtered local ID width: {error}")))?;
                if segment.doc_ids.get(local_index) != Some(&id) {
                    return Err(corrupt("candidate local ID differs from segment"));
                }
                if local_mask & (1_u64 << local) != 0 {
                    output.push(id);
                }
            }
        }
        output.sort_unstable();
        output.dedup();
        self.charge_query_io_since(work, &mut io_before)?;
        Ok(output)
    }

    fn conjunctive_posting_ids(
        &self,
        surface: PostingSurface,
        grams: &[[u8; 3]],
        estimates: &BTreeMap<[u8; 3], u64>,
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<u64>, CoreError> {
        let mut ordered = Vec::new();
        ordered.try_reserve_exact(grams.len()).map_err(|error| {
            CoreError::Storage(format!(
                "file authority v17: reserve conjunction grams: {error}"
            ))
        })?;
        ordered.extend_from_slice(grams);
        ordered
            .sort_unstable_by_key(|gram| (estimates.get(gram).copied().unwrap_or(u64::MAX), *gram));
        let Some((&anchor, _remaining)) = ordered.split_first() else {
            return Err(corrupt("empty conjunction reached posting evaluator"));
        };
        if estimates.get(&anchor) == Some(&0) {
            return Ok(Vec::new());
        }
        let surface_code = if surface == PostingSurface::Path {
            1
        } else {
            2
        };
        let routes = if surface == PostingSurface::Path {
            &self.root.path_routes
        } else {
            &self.root.content_routes
        };
        let lower = route_prefix(surface_code, anchor);
        let upper = prefix_successor(&lower);
        let mut io_before = self.store.read_meter_snapshot();
        let mut ids = Vec::new();
        let mut observed_anchor_count = 0_u64;
        self.store.map_scan(
            routes,
            &lower,
            upper.as_deref(),
            Some(budget),
            &mut |key, segment_ref| {
                self.charge_query_io_since(work, &mut io_before)?;
                work.charge_paged_probe(super::super::policy())?;
                if key.len() != 40
                    || key.get(..4) != Some(lower.as_slice())
                    || segment_ref.root.kind != ObjectKind::Segment
                    || key.get(8..) != Some(segment_ref.root.sha256.as_slice())
                {
                    return Err(corrupt("conjunction anchor route differs from segment"));
                }
                let anchor_count = u32::from_be_bytes(
                    key.get(4..8)
                        .ok_or_else(|| corrupt("conjunction anchor count is absent"))?
                        .try_into()
                        .map_err(|_detail| corrupt("conjunction anchor count width differs"))?,
                );
                observed_anchor_count = observed_anchor_count
                    .checked_add(u64::from(anchor_count))
                    .ok_or_else(|| corrupt("conjunction anchor count overflows"))?;
                let segment = Segment::decode(&self.store.read_blob(
                    segment_ref,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    Some(budget),
                )?)?;
                self.charge_query_io_since(work, &mut io_before)?;
                let terms = if surface == PostingSurface::Path {
                    &segment.path_terms
                } else {
                    &segment.content_terms
                };
                let mut page_cache = Vec::<(BlobRef, PostingPage)>::new();
                page_cache
                    .try_reserve_exact(ordered.len())
                    .map_err(|error| {
                        CoreError::Storage(format!(
                            "file authority v17: reserve selected posting pages: {error}"
                        ))
                    })?;
                let mut intersection = u64::MAX;
                for (index, gram) in ordered.iter().copied().enumerate() {
                    budget.checkpoint("lexical:file-conjunction-segment")?;
                    let Some(page_ref) = self.store.map_get(terms, &gram, Some(budget))? else {
                        self.charge_query_io_since(work, &mut io_before)?;
                        if index == 0 {
                            return Err(corrupt("conjunction anchor omits segment posting term"));
                        }
                        intersection = 0;
                        break;
                    };
                    self.charge_query_io_since(work, &mut io_before)?;
                    if page_ref.root.kind != ObjectKind::FilePostings {
                        return Err(corrupt("conjunction posting page kind differs"));
                    }
                    if !page_cache
                        .iter()
                        .any(|(reference, _)| reference == &page_ref)
                    {
                        let bytes = self.store.read_blob(
                            &page_ref,
                            size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                            Some(budget),
                        )?;
                        self.charge_query_io_since(work, &mut io_before)?;
                        page_cache.push((
                            page_ref.clone(),
                            PostingPage::decode(&bytes, segment.doc_ids.len())?,
                        ));
                    }
                    let page = page_cache
                        .iter()
                        .find(|(reference, _)| reference == &page_ref)
                        .map(|(_, page)| page)
                        .ok_or_else(|| corrupt("selected posting page cache lost reference"))?;
                    let position = page
                        .0
                        .binary_search_by_key(&gram, |row| row.0)
                        .map_err(|_detail| corrupt("conjunction posting page omits gram"))?;
                    let mask = page
                        .0
                        .get(position)
                        .ok_or_else(|| corrupt("conjunction gram position absent"))?
                        .1;
                    let count = mask.count_ones();
                    work.charge_paged_ids(super::super::policy(), u64::from(count))?;
                    if index == 0 {
                        if count != anchor_count {
                            return Err(corrupt("conjunction anchor route count differs"));
                        }
                    } else {
                        work.charge_paged_probe(super::super::policy())?;
                        let route = self.store.map_get(
                            routes,
                            &super::route_key(surface_code, gram, count, segment_ref.root.sha256),
                            Some(budget),
                        )?;
                        self.charge_query_io_since(work, &mut io_before)?;
                        if route.as_ref() != Some(segment_ref) {
                            return Err(corrupt("conjunction gram route is absent or differs"));
                        }
                    }
                    intersection &= mask;
                    if intersection == 0 {
                        break;
                    }
                }
                for local in 0..segment.doc_ids.len() {
                    if intersection & (1_u64 << local) == 0 {
                        continue;
                    }
                    let id = *segment
                        .doc_ids
                        .get(local)
                        .ok_or_else(|| corrupt("conjunction local ID outside segment"))?;
                    let current = live_doc(&self.store, &self.root, id, Some(budget))?;
                    self.charge_query_io_since(work, &mut io_before)?;
                    if let Some(current) = current
                        && current.segment_sha256 == segment_ref.root.sha256
                        && usize::try_from(current.local_id)
                            .map_err(|error| corrupt(&format!("local ID width: {error}")))?
                            == local
                    {
                        ids.try_reserve(1).map_err(|error| {
                            CoreError::Storage(format!(
                                "file authority v17: reserve conjunction ID: {error}"
                            ))
                        })?;
                        ids.push(id);
                    }
                }
                Ok(())
            },
        )?;
        if estimates.get(&anchor) != Some(&observed_anchor_count) {
            return Err(corrupt("conjunction anchor route inventory changed"));
        }
        ids.sort_unstable();
        ids.dedup();
        self.charge_query_io_since(work, &mut io_before)?;
        Ok(ids)
    }

    fn gram_count_estimates(
        &self,
        surface: PostingSurface,
        expression: &NecessaryGramExpr,
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeMap<[u8; 3], u64>, CoreError> {
        let mut io_before = self.store.read_meter_snapshot();
        let mut grams = BTreeSet::new();
        collect_grams(expression, &mut grams);
        let routes = if surface == PostingSurface::Path {
            &self.root.path_routes
        } else {
            &self.root.content_routes
        };
        let surface_code = if surface == PostingSurface::Path {
            1
        } else {
            2
        };
        let mut counts = BTreeMap::new();
        for gram in grams {
            let lower = route_prefix(surface_code, gram);
            let upper = prefix_successor(&lower);
            let mut count = 0_u64;
            self.store.map_scan(
                routes,
                &lower,
                upper.as_deref(),
                Some(budget),
                &mut |key, reference| {
                    self.charge_query_io_since(work, &mut io_before)?;
                    budget.checkpoint("lexical:file-gram-descriptor-estimate")?;
                    if key.len() != 40
                        || key.get(..4) != Some(lower.as_slice())
                        || reference.root.kind != ObjectKind::Segment
                        || key.get(8..) != Some(reference.root.sha256.as_slice())
                    {
                        return Err(corrupt("gram route descriptor shape differs"));
                    }
                    let local_count = u32::from_be_bytes(
                        key.get(4..8)
                            .ok_or_else(|| corrupt("gram descriptor count absent"))?
                            .try_into()
                            .map_err(|_detail| corrupt("gram descriptor count width"))?,
                    );
                    if local_count == 0
                        || usize::try_from(local_count)
                            .map_err(|error| corrupt(&format!("gram count width: {error}")))?
                            > super::MAX_SEGMENT_DOCS
                    {
                        return Err(corrupt(
                            "gram route descriptor count exceeds segment policy",
                        ));
                    }
                    count = count
                        .checked_add(u64::from(local_count))
                        .ok_or_else(|| plan_limit("gram descriptor count overflow"))?;
                    Ok(())
                },
            )?;
            self.charge_query_io_since(work, &mut io_before)?;
            let _previous = counts.insert(gram, count);
        }
        Ok(counts)
    }

    pub(crate) fn posting_lists(
        &self,
        surface: PostingSurface,
        grams: &[[u8; 3]],
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeMap<[u8; 3], Vec<u64>>, CoreError> {
        let metered = if self.store.has_read_meter() {
            Self {
                root: self.root.clone(),
                store: self.store.clone(),
            }
        } else {
            self.query_scope()
        };
        let mut output = BTreeMap::new();
        for gram in grams {
            budget.checkpoint("lexical:file-paged-posting-lists")?;
            if output.contains_key(gram) {
                continue;
            }
            let ids = metered.posting_ids(surface, *gram, work, budget)?;
            let _previous = output.insert(*gram, ids);
        }
        Ok(output)
    }

    pub(crate) fn necessary_candidates(
        &self,
        surface: PostingSurface,
        expression: &NecessaryGramExpr,
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<Option<Vec<u64>>, CoreError> {
        crate::file_authority::query::validate_expression(expression)?;
        if crate::file_authority::query::is_universal(expression) {
            return Ok(None);
        }
        let metered = if self.store.has_read_meter() {
            Self {
                root: self.root.clone(),
                store: self.store.clone(),
            }
        } else {
            self.query_scope()
        };
        let estimates = metered.gram_count_estimates(surface, expression, work, budget)?;
        let result =
            metered.evaluate_candidates(surface, expression, None, &estimates, work, budget)?;
        if result
            .as_ref()
            .map(|ids| size_u64(ids.len()))
            .transpose()?
            .is_some_and(|count| count > super::super::policy().query_posting_ids)
        {
            return Err(plan_limit("final candidate count exceeds policy"));
        }
        Ok(result)
    }

    fn evaluate_candidates(
        &self,
        surface: PostingSurface,
        expression: &NecessaryGramExpr,
        allowed: Option<&[u64]>,
        estimates: &BTreeMap<[u8; 3], u64>,
        work: &mut QueryWork,
        budget: &RequestBudgetV1,
    ) -> Result<Option<Vec<u64>>, CoreError> {
        // A parent may already have reduced the candidate set. Preserve that
        // bounded locator path instead of scanning every anchor segment again.
        if allowed.is_none()
            && let Some(grams) = pure_conjunction_grams(expression)?
        {
            if grams.is_empty() {
                return Ok(None);
            }
            if grams.len() >= 2 {
                return self
                    .conjunctive_posting_ids(surface, &grams, estimates, work, budget)
                    .map(Some);
            }
        }
        let result: Result<Option<Vec<u64>>, CoreError> = match expression {
            NecessaryGramExpr::AllDocs => Ok(None),
            NecessaryGramExpr::Gram(gram) => Ok(Some(match allowed {
                Some(ids) => self.posting_ids_within(surface, *gram, ids, work, budget)?,
                None => self.posting_ids(surface, *gram, work, budget)?,
            })),
            NecessaryGramExpr::And(children) => {
                let mut output: Option<Vec<u64>> = None;
                let mut ordered = Vec::new();
                ordered.try_reserve(children.len()).map_err(|error| {
                    CoreError::Storage(format!(
                        "file authority v17: reserve conjunction order: {error}"
                    ))
                })?;
                ordered.extend(children.iter());
                ordered.sort_by_key(|child| estimated_count(child, estimates));
                for child in ordered {
                    let within = output.as_deref().or(allowed);
                    if let Some(ids) =
                        self.evaluate_candidates(surface, child, within, estimates, work, budget)?
                    {
                        output = Some(ids);
                        if output.as_ref().is_some_and(Vec::is_empty) {
                            break;
                        }
                    }
                }
                Ok(output)
            }
            NecessaryGramExpr::Or(children) => {
                let mut output = Vec::new();
                for child in children {
                    let Some(ids) =
                        self.evaluate_candidates(surface, child, allowed, estimates, work, budget)?
                    else {
                        return Ok(allowed.map(<[u64]>::to_vec));
                    };
                    output = union_sorted(&output, &ids)?;
                }
                Ok(Some(output))
            }
        };
        result
    }
}

fn pure_conjunction_grams(
    expression: &NecessaryGramExpr,
) -> Result<Option<Vec<[u8; 3]>>, CoreError> {
    let mut grams = Vec::new();
    let mut pending = Vec::new();
    pending.try_reserve(1).map_err(|error| {
        CoreError::Storage(format!(
            "file authority v17: reserve conjunction traversal: {error}"
        ))
    })?;
    pending.push(expression);
    while let Some(node) = pending.pop() {
        match node {
            NecessaryGramExpr::AllDocs => {}
            NecessaryGramExpr::Gram(gram) => {
                grams.try_reserve(1).map_err(|error| {
                    CoreError::Storage(format!(
                        "file authority v17: reserve conjunction gram: {error}"
                    ))
                })?;
                grams.push(*gram);
            }
            NecessaryGramExpr::And(children) => {
                pending.try_reserve(children.len()).map_err(|error| {
                    CoreError::Storage(format!(
                        "file authority v17: reserve conjunction children: {error}"
                    ))
                })?;
                pending.extend(children);
            }
            NecessaryGramExpr::Or(_) => return Ok(None),
        }
    }
    grams.sort_unstable();
    grams.dedup();
    Ok(Some(grams))
}

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut upper = prefix.to_vec();
    for index in (0..upper.len()).rev() {
        let byte = upper.get_mut(index)?;
        if *byte != u8::MAX {
            *byte = byte.checked_add(1)?;
            upper.truncate(index.checked_add(1)?);
            return Some(upper);
        }
    }
    None
}

fn collect_grams(expression: &NecessaryGramExpr, grams: &mut BTreeSet<[u8; 3]>) {
    match expression {
        NecessaryGramExpr::AllDocs => {}
        NecessaryGramExpr::Gram(gram) => {
            let _inserted = grams.insert(*gram);
        }
        NecessaryGramExpr::And(children) | NecessaryGramExpr::Or(children) => {
            for child in children {
                collect_grams(child, grams);
            }
        }
    }
}

fn estimated_count(expression: &NecessaryGramExpr, estimates: &BTreeMap<[u8; 3], u64>) -> u64 {
    match expression {
        NecessaryGramExpr::AllDocs => u64::MAX,
        NecessaryGramExpr::Gram(gram) => estimates.get(gram).copied().unwrap_or(u64::MAX),
        NecessaryGramExpr::And(children) => children
            .iter()
            .map(|child| estimated_count(child, estimates))
            .min()
            .unwrap_or(u64::MAX),
        NecessaryGramExpr::Or(children) => children
            .iter()
            .map(|child| estimated_count(child, estimates))
            .fold(0_u64, u64::saturating_add),
    }
}

fn union_sorted(left: &[u64], right: &[u64]) -> Result<Vec<u64>, CoreError> {
    let capacity = left
        .len()
        .checked_add(right.len())
        .ok_or_else(|| plan_limit("candidate union length overflow"))?;
    let mut output = Vec::new();
    output.try_reserve(capacity).map_err(|error| {
        CoreError::Storage(format!("file authority v17: reserve union: {error}"))
    })?;
    let mut left = left.iter().copied().peekable();
    let mut right = right.iter().copied().peekable();
    loop {
        let next = match (left.peek().copied(), right.peek().copied()) {
            (Some(x), Some(y)) if x < y => left.next(),
            (Some(x), Some(y)) if x > y => right.next(),
            (Some(_), Some(_)) => {
                let _same = right.next();
                left.next()
            }
            (Some(_), None) => left.next(),
            (None, Some(_)) => right.next(),
            (None, None) => break,
        };
        output.push(next.ok_or_else(|| plan_limit("candidate union iterator regressed"))?);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_authority::paged::{FileAuthorityMutation, FileSourceEdit, build_for_seal};
    use crate::file_authority::{QueryWork, policy, query_physical_read_limit};
    use crate::lexical_allocation::DocVersionReservation;
    use crate::lexical_store::AttemptBinding;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileCoverage, SymbolCoverage,
        SymbolNameSourcePolicyV1,
    };

    #[test]
    fn restricted_nested_conjunction_does_not_read_unselected_corrupt_postings() {
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
        let edits = (0..128)
            .map(|index| {
                let bytes = if index == 0 {
                    b"rare alpha beta file000".to_vec()
                } else {
                    format!("alpha beta file{index:03}").into_bytes()
                };
                FileSourceEdit::Upsert {
                    coverage: SourceFileCoverage {
                        source: SourceFileRevision {
                            file: SourceFileKey {
                                source_repo_id: RepoId::new("repo").expect("repo"),
                                repo_relative_path: RepoRelativePath::new(format!("{index:03}.rs")),
                            },
                            revision_id: RevisionId::new("revision").expect("revision"),
                            source_sha256: Sha256::digest(&bytes).into(),
                        },
                        language: LanguageCode::new("rust").expect("language"),
                        producer_policy_sha256: [6; 32],
                        symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                        unit_set_sha256: [7; 32],
                        text_admitted: true,
                        symbols: SymbolCoverage::NotRequested,
                    },
                    bytes,
                }
            })
            .collect();
        let mutation = FileAuthorityMutation {
            edits,
            clear_all: false,
            operations_sha256: binding.operations_sha256,
        };
        let reservation = DocVersionReservation {
            first: 1,
            count: 128,
            scope_sha256: [5; 32],
        };
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation).expect("build");
        let selected = live_doc(&store, &built.root, 1, None)
            .expect("selected")
            .expect("selected row");
        let unrelated = live_doc(&store, &built.root, 128, None)
            .expect("unrelated")
            .expect("unrelated row");
        assert_ne!(selected.segment_sha256, unrelated.segment_sha256);
        let segment_ref = store
            .map_get(&built.root.segments, &unrelated.segment_sha256, None)
            .expect("segment lookup")
            .expect("segment");
        let segment = Segment::decode(
            &store
                .read_blob(
                    &segment_ref,
                    size_u64(super::super::MAX_RECORD_BYTES).expect("record bound"),
                    None,
                )
                .expect("segment read"),
        )
        .expect("segment decode");
        let posting = store
            .map_get(&segment.content_terms, b"alp", None)
            .expect("posting lookup")
            .expect("posting");
        let mut hex = String::with_capacity(64);
        for byte in &posting.root.sha256 {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").expect("digest hex");
        }
        let path = directory
            .path()
            .join("lexical-object-pool/objects")
            .join(hex.get(..2).expect("digest prefix"))
            .join(hex);
        let mut bytes = std::fs::read(&path).expect("posting bytes");
        *bytes.last_mut().expect("nonempty posting") ^= 1;
        std::fs::write(path, bytes).expect("corrupt unrelated posting");
        let authority = PagedFileAuthority::open(&built.root_bytes, store).expect("authority");
        let common = NecessaryGramExpr::And(vec![
            NecessaryGramExpr::Gram(*b"alp"),
            NecessaryGramExpr::Gram(*b"bet"),
        ]);
        let restricted = NecessaryGramExpr::And(vec![
            NecessaryGramExpr::Gram(*b"rar"),
            NecessaryGramExpr::Or(vec![common.clone(), NecessaryGramExpr::Gram(*b"zzz")]),
        ]);
        assert_eq!(
            authority
                .necessary_candidates(
                    PostingSurface::Content,
                    &restricted,
                    &mut QueryWork::default(),
                    &RequestBudgetV1::unbounded()
                )
                .expect("restricted query"),
            Some(vec![1])
        );
        assert!(matches!(
            authority.necessary_candidates(
                PostingSurface::Content,
                &common,
                &mut QueryWork::default(),
                &RequestBudgetV1::unbounded()
            ),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }

    #[test]
    fn metadata_walk_has_bounded_physical_credit_without_spending_posting_credit() {
        let directory = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(directory.path()).expect("store");
        let bytes = b"x";
        let key = SourceFileKey {
            source_repo_id: RepoId::new("repo").expect("repo"),
            repo_relative_path: RepoRelativePath::new("a.rs"),
        };
        let coverage = SourceFileCoverage {
            source: SourceFileRevision {
                file: key,
                revision_id: RevisionId::new("revision").expect("revision"),
                source_sha256: Sha256::digest(bytes).into(),
            },
            language: LanguageCode::new("rust").expect("language"),
            producer_policy_sha256: [6; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: [7; 32],
            text_admitted: true,
            symbols: SymbolCoverage::NotRequested,
        };
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
        let mutation = FileAuthorityMutation {
            edits: vec![FileSourceEdit::Upsert {
                coverage,
                bytes: bytes.to_vec(),
            }],
            clear_all: false,
            operations_sha256: binding.operations_sha256,
        };
        let reservation = DocVersionReservation {
            first: 1,
            count: 1,
            scope_sha256: [5; 32],
        };
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation).expect("build");
        // Body reads bypass the bounded request metadata cache, so repeated
        // reads exercise the actual physical-I/O ceiling rather than cache hits.
        let uncached_body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"physical read probe")
            .expect("uncached body");
        let authority = PagedFileAuthority::open(&built.root_bytes, store)
            .expect("authority")
            .query_scope();
        let mut low = policy();
        low.source_files = 1;
        low.query_list_reads = 1;
        let budget = RequestBudgetV1::unbounded();
        let mut seen = 0_u64;
        authority
            .scan_sources_with_policy(
                &budget,
                &mut |_metadata| {
                    seen += 1;
                    Ok(())
                },
                low,
            )
            .expect("metadata walk admitted");
        assert_eq!(seen, 1);
        let observed = authority.store.read_meter_snapshot();
        assert!(observed.1 > low.query_list_reads);
        let mut posting_work = QueryWork::default();
        assert!(matches!(
            posting_work.charge_paged_io(low, (0, 0), observed),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        let canonical = policy();
        let mut nearly_exhausted = QueryWork::default();
        nearly_exhausted
            .charge_paged_io(
                canonical,
                (0, 0),
                (0, canonical.query_list_reads.saturating_sub(1)),
            )
            .expect("one posting read remains");
        let before_posting = authority.store.read_meter_snapshot();
        assert!(matches!(
            authority.posting_ids(
                PostingSurface::Path,
                *b"a.r",
                &mut nearly_exhausted,
                &budget,
            ),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        let posting_reads = authority
            .store
            .read_meter_snapshot()
            .1
            .checked_sub(before_posting.1)
            .expect("meter advanced");
        assert!(
            (2..=3).contains(&posting_reads),
            "posting refusal read {posting_reads} objects"
        );
        let ceiling = query_physical_read_limit(low).expect("physical bound");
        let record_ceiling =
            u64::try_from(super::super::MAX_RECORD_BYTES).expect("record ceiling fits u64");
        while authority.store.read_meter_snapshot().1 < ceiling {
            let _record = authority
                .store
                .read_blob(&uncached_body, record_ceiling, None)
                .expect("authenticated record");
        }
        authority
            .check_query_io_with_policy(low)
            .expect("at physical bound");
        let _record = authority
            .store
            .read_blob(&uncached_body, record_ceiling, None)
            .expect("authenticated excess record");
        assert!(matches!(
            authority.check_query_io_with_policy(low),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
    }
}
