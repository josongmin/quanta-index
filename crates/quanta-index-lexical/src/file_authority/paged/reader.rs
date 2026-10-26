//! Fallible V17 source and posting lookup. No source or term census is built
//! during open; selected map paths and authenticated blobs are read on demand.

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{SourceFileKey, SourceFileRevision};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_lq_trigram::NecessaryGramExpr;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::lexical_store::{BlobRef, ObjectKind, ObjectStore};

use super::run_postings::{
    RunGramCount, RunPostingPage, RunPostingRoot, run_page_key, run_page_prefix, run_route_key,
};
use super::{AuthorityRoot, LiveDoc, SourceHead, doc_key, policy_digest, source_key_digest};
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
        let lower = run_page_prefix(surface_code, gram)?;
        let upper = prefix_successor(&lower);
        let mut ids = Vec::new();
        self.store.map_scan(
            routes,
            &lower,
            upper.as_deref(),
            Some(budget),
            &mut |key, count_ref| {
                self.charge_query_io_since(work, &mut io_before)?;
                work.charge_paged_probe(super::super::policy())?;
                if key.len() != 20
                    || key.get(..4) != Some(lower.as_slice())
                    || count_ref.root.kind != ObjectKind::Record
                {
                    return Err(corrupt("run route key or count kind differs"));
                }
                let run_id = u64::from_be_bytes(
                    key.get(12..20)
                        .ok_or_else(|| corrupt("run route ID absent"))?
                        .try_into()
                        .map_err(|_width| corrupt("run route ID width differs"))?,
                );
                let count = RunGramCount::decode(&self.store.read_blob(
                    count_ref,
                    size_u64(super::MAX_RECORD_BYTES)?,
                    Some(budget),
                )?)?;
                self.charge_query_io_since(work, &mut io_before)?;
                if run_id == 0
                    || count.key()? != lower
                    || run_route_key(surface_code, gram, count.count, run_id)?.as_slice() != key
                {
                    return Err(corrupt("run route and gram count differ"));
                }
                let root_ref = self
                    .store
                    .map_get(&self.root.run_postings, &run_id.to_be_bytes(), Some(budget))?
                    .ok_or_else(|| corrupt("run route names absent posting root"))?;
                self.charge_query_io_since(work, &mut io_before)?;
                let run = RunPostingRoot::load(&self.store, &root_ref, Some(budget))?;
                self.charge_query_io_since(work, &mut io_before)?;
                if run.run_id != run_id {
                    return Err(corrupt("run route differs from posting root"));
                }
                let run_count_ref = self.store.map_get(&run.gram_counts, &lower, Some(budget))?;
                self.charge_query_io_since(work, &mut io_before)?;
                if run_count_ref.as_ref() != Some(count_ref) {
                    return Err(corrupt("run route differs from posting root"));
                }
                let mut observed = 0_u64;
                self.store.map_scan(
                    &run.pages,
                    &lower,
                    upper.as_deref(),
                    Some(budget),
                    &mut |page_key, page_ref| {
                        self.charge_query_io_since(work, &mut io_before)?;
                        work.charge_paged_probe(super::super::policy())?;
                        if page_ref.root.kind != ObjectKind::Record {
                            return Err(corrupt("run posting page kind differs"));
                        }
                        let page = RunPostingPage::decode(&self.store.read_blob(
                            page_ref,
                            size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                            Some(budget),
                        )?)?;
                        self.charge_query_io_since(work, &mut io_before)?;
                        if page.key()?.as_slice() != page_key {
                            return Err(corrupt("run posting map key differs"));
                        }
                        let rows = size_u64(page.rows.len())?;
                        observed = observed
                            .checked_add(rows)
                            .ok_or_else(|| corrupt("run route row count overflow"))?;
                        work.charge_paged_ids(super::super::policy(), rows)?;
                        for row in page.rows {
                            let current =
                                live_doc(&self.store, &self.root, row.logical_id, Some(budget))?;
                            self.charge_query_io_since(work, &mut io_before)?;
                            if current
                                .is_some_and(|locator| locator.segment_sha256 == row.segment_sha256)
                            {
                                ids.try_reserve(1).map_err(|error| {
                                    CoreError::Storage(format!("reserve run candidate ID: {error}"))
                                })?;
                                ids.push(row.logical_id);
                            }
                        }
                        self.charge_query_io_since(work, &mut io_before)?;
                        Ok(())
                    },
                )?;
                if observed != count.count {
                    return Err(corrupt("run route count differs from posting pages"));
                }
                self.charge_query_io_since(work, &mut io_before)?;
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
        let surface_code = if surface == PostingSurface::Path {
            1
        } else {
            2
        };
        let mut owner_cache = BTreeMap::<[u8; 32], u64>::new();
        let mut groups = BTreeMap::<(u64, [u8; 12]), Vec<(u64, [u8; 32])>>::new();
        for id in allowed {
            budget.checkpoint("lexical:file-filtered-run-candidate")?;
            let locator = live_doc(&self.store, &self.root, *id, Some(budget))?
                .ok_or_else(|| corrupt("candidate ID lost current locator"))?;
            self.charge_query_io_since(work, &mut io_before)?;
            let run_id = if let Some(run_id) = owner_cache.get(&locator.segment_sha256) {
                *run_id
            } else {
                let run_id = self.root.run_registry.owner_run_id(
                    &self.store,
                    locator.segment_sha256,
                    Some(budget),
                )?;
                let _previous = owner_cache.insert(locator.segment_sha256, run_id);
                run_id
            };
            let page_key = run_page_key(surface_code, gram, *id)?;
            groups
                .entry((run_id, page_key))
                .or_default()
                .push((*id, locator.segment_sha256));
            self.charge_query_io_since(work, &mut io_before)?;
        }
        let mut output = Vec::new();
        for ((run_id, page_key), candidates) in groups {
            work.charge_paged_probe(super::super::policy())?;
            let root_ref = self
                .store
                .map_get(&self.root.run_postings, &run_id.to_be_bytes(), Some(budget))?
                .ok_or_else(|| corrupt("candidate run posting root absent"))?;
            self.charge_query_io_since(work, &mut io_before)?;
            let run = RunPostingRoot::load(&self.store, &root_ref, Some(budget))?;
            self.charge_query_io_since(work, &mut io_before)?;
            if run.run_id != run_id {
                return Err(corrupt("candidate run posting ID differs"));
            }
            let Some(page_ref) = self.store.map_get(&run.pages, &page_key, Some(budget))? else {
                self.charge_query_io_since(work, &mut io_before)?;
                continue;
            };
            self.charge_query_io_since(work, &mut io_before)?;
            if page_ref.root.kind != ObjectKind::Record {
                return Err(corrupt("filtered run page kind differs"));
            }
            let page = RunPostingPage::decode(&self.store.read_blob(
                &page_ref,
                size_u64(super::MAX_POSTING_PAGE_BYTES)?,
                Some(budget),
            )?)?;
            self.charge_query_io_since(work, &mut io_before)?;
            if page.key()? != page_key {
                return Err(corrupt("filtered run page key differs"));
            }
            work.charge_paged_ids(super::super::policy(), size_u64(page.rows.len())?)?;
            for (id, digest) in candidates {
                if let Ok(index) = page.rows.binary_search_by_key(&id, |row| row.logical_id) {
                    if page
                        .rows
                        .get(index)
                        .ok_or_else(|| corrupt("filtered run row disappeared"))?
                        .segment_sha256
                        != digest
                    {
                        return Err(corrupt("filtered run locator differs from current"));
                    }
                    output.push(id);
                }
            }
            self.charge_query_io_since(work, &mut io_before)?;
        }
        output.sort_unstable();
        output.dedup();
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
        let mut ordered = grams.to_vec();
        ordered
            .sort_unstable_by_key(|gram| (estimates.get(gram).copied().unwrap_or(u64::MAX), *gram));
        let Some((&anchor, remaining)) = ordered.split_first() else {
            return Err(corrupt("empty conjunction reached posting evaluator"));
        };
        if estimates.get(&anchor) == Some(&0) {
            return Ok(Vec::new());
        }
        let mut ids = self.posting_ids(surface, anchor, work, budget)?;
        for gram in remaining {
            if ids.is_empty() {
                break;
            }
            ids = self.posting_ids_within(surface, *gram, &ids, work, budget)?;
        }
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
            let lower = run_page_prefix(surface_code, gram)?;
            let upper = prefix_successor(&lower);
            let mut count = 0_u64;
            self.store.map_scan(
                routes,
                &lower,
                upper.as_deref(),
                Some(budget),
                &mut |key, reference| {
                    self.charge_query_io_since(work, &mut io_before)?;
                    budget.checkpoint("lexical:file-run-gram-estimate")?;
                    if key.len() != 20
                        || key.get(..4) != Some(lower.as_slice())
                        || reference.root.kind != ObjectKind::Record
                    {
                        return Err(corrupt("gram run route shape differs"));
                    }
                    let run_id = u64::from_be_bytes(
                        key.get(12..20)
                            .ok_or_else(|| corrupt("gram route run ID absent"))?
                            .try_into()
                            .map_err(|_width| corrupt("gram route run ID width differs"))?,
                    );
                    let gram_count = RunGramCount::decode(&self.store.read_blob(
                        reference,
                        size_u64(super::MAX_RECORD_BYTES)?,
                        Some(budget),
                    )?)?;
                    self.charge_query_io_since(work, &mut io_before)?;
                    if gram_count.key()? != lower
                        || run_route_key(surface_code, gram, gram_count.count, run_id)?.as_slice()
                            != key
                    {
                        return Err(corrupt("gram run route count differs"));
                    }
                    count = count
                        .checked_add(gram_count.count)
                        .ok_or_else(|| plan_limit("gram run count overflow"))?;
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
    use crate::lexical_runs::RunRegistry;
    use crate::lexical_store::{AttemptBinding, MapEdit, MapRoot};
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
            first: 129,
            count: 128,
            scope_sha256: [5; 32],
        };
        let built = build_for_seal(&store, &lease, None, &reservation, &mutation).expect("build");
        let selected = live_doc(&store, &built.root, 129, None)
            .expect("selected")
            .expect("selected row");
        let unrelated = live_doc(&store, &built.root, 256, None)
            .expect("unrelated")
            .expect("unrelated row");
        assert_ne!(selected.segment_sha256, unrelated.segment_sha256);
        let selected_page = run_page_key(2, *b"alp", 129).expect("selected bucket");
        let unrelated_page = run_page_key(2, *b"alp", 256).expect("unrelated bucket");
        assert_ne!(selected_page, unrelated_page);
        let run_id = built
            .root
            .run_registry
            .owner_run_id(&store, unrelated.segment_sha256, None)
            .expect("unrelated owner run");
        let run_ref = store
            .map_get(&built.root.run_postings, &run_id.to_be_bytes(), None)
            .expect("run lookup")
            .expect("run posting root");
        let run = RunPostingRoot::load(&store, &run_ref, None).expect("run posting root read");
        let posting = store
            .map_get(&run.pages, &unrelated_page, None)
            .expect("posting lookup")
            .expect("posting");
        let page = RunPostingPage::decode(
            &store
                .read_blob(
                    &posting,
                    size_u64(super::super::MAX_POSTING_PAGE_BYTES).expect("page bound"),
                    None,
                )
                .expect("posting read"),
        )
        .expect("posting decode");
        assert!(page.rows.iter().any(|row| row.logical_id == 256));
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
        std::fs::write(path, bytes).expect("corrupt unrelated run posting");
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
            Some(vec![129])
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
    fn gram_estimate_refuses_after_count_blob_before_next_route_page() {
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
        let mut edits = Vec::new();
        for count in 1_u64..=97 {
            let record = RunGramCount {
                surface: 2,
                gram: *b"alp",
                count,
            };
            let reference = store
                .put_blob(
                    &lease,
                    ObjectKind::Record,
                    &[],
                    &record.encode().expect("count bytes"),
                )
                .expect("count record");
            edits.push(MapEdit {
                key: run_route_key(2, *b"alp", count, count)
                    .expect("route key")
                    .to_vec(),
                value: Some(reference),
            });
        }
        let routes = store
            .map_apply(&lease, &MapRoot::default(), edits.into_iter().map(Ok))
            .expect("route map");
        assert_eq!(routes.entries, 97);
        assert!(routes.page.as_ref().is_some_and(|page| page.height > 0));

        // Calibrate the leaf boundary through the map reader, independently
        // of the estimate evaluator and its query accounting.
        let lower = run_page_prefix(2, *b"alp").expect("gram prefix");
        let upper = prefix_successor(&lower);
        let calibration = store.metered_clone();
        let mut route_page_reads = Vec::new();
        calibration
            .map_scan(
                &routes,
                &lower,
                upper.as_deref(),
                None,
                &mut |_key, _ref| {
                    route_page_reads.push(calibration.read_meter_snapshot().1);
                    Ok(())
                },
            )
            .expect("calibrate route pages");
        assert_eq!(route_page_reads.len(), 97);
        let first_leaf_rows = route_page_reads
            .windows(2)
            .position(|pair| matches!(pair, [previous, next] if next > previous))
            .map(|index| index + 1)
            .expect("second leaf was read");
        let initial_map_reads = route_page_reads.first().copied().expect("first route");
        let expected_reads = initial_map_reads
            .checked_add(u64::try_from(first_leaf_rows).expect("first leaf width"))
            .expect("expected reads");

        // Every count has distinct encoded bytes, so each first-leaf record
        // consumes one authenticated read. The final one exceeds the work cap.
        let canonical = policy();
        let remaining = expected_reads.checked_sub(1).expect("nonempty route");
        let already_charged = canonical
            .query_list_reads
            .checked_sub(remaining)
            .expect("canonical limit admits this fixture");
        let mut work = QueryWork::default();
        work.charge_paged_io(canonical, (0, 0), (0, already_charged))
            .expect("prime one-read-short budget");
        let authority = PagedFileAuthority {
            root: AuthorityRoot {
                policy_sha256: policy_digest(),
                allocation_scope_sha256: [0; 32],
                source_heads: MapRoot::default(),
                current_docs: MapRoot::default(),
                memberships: MapRoot::default(),
                segments: MapRoot::default(),
                run_registry: RunRegistry::default(),
                premerge_run_registry: None,
                run_postings: MapRoot::default(),
                run_index_encoded_bytes: 0,
                compaction_debt: MapRoot::default(),
                path_routes: MapRoot::default(),
                content_routes: routes,
                live_sources: 0,
                live_docs: 0,
                source_bytes: 0,
                membership_count: 0,
            },
            store: store.metered_clone(),
        };
        let before = authority.query_io_snapshot().1;
        assert!(matches!(
            authority.gram_count_estimates(
                PostingSurface::Content,
                &NecessaryGramExpr::Gram(*b"alp"),
                &mut work,
                &RequestBudgetV1::unbounded(),
            ),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        let actual_reads = authority
            .query_io_snapshot()
            .1
            .checked_sub(before)
            .expect("meter advanced");
        assert_eq!(actual_reads, expected_reads, "next map leaf was not read");
        assert_eq!(
            work.authenticated_io().1,
            canonical
                .query_list_reads
                .checked_add(1)
                .expect("one over-limit read"),
            "first over-limit count is recorded before refusal"
        );
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
                    seen = seen.saturating_add(1);
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
