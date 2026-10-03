//! Opening a sealed generation for query: the walk every door shares and the searcher it builds.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::generation_dir::{is_writer_lock_entry, sync_generation_directory};
use crate::index_store::read_lexical_sealed_identity;
use crate::overlay_codec::OverlayFamily;
use crate::sealed_generation::{SealedGenerationVisitor, walk_sealed_generation};
use crate::text_authority::{
    ShardBody, ShardedTextAuthority, TEXT_AUTHORITY_DIR_NAME, TextAuthorityWriteReceipt,
};
use crate::text_authority_plan::plan_text_authority_delta;
use crate::text_docs::collect_text_authority_docs;
use crate::{
    GenKey, GenerationWriter, LexicalAdapter, LoadedGeneration, OverlaySnapshot, TantivySearcher,
    TextAuthorityPlan, TextAuthorityWrite, text_authority,
};
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    FileContributorIdentityEntry, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, LexicalArtifactIdentityV1, LexicalIndexOpenPort, LexicalSearcher,
    RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, RequestBudgetV1, TextNormalizerVersionV1,
    unique_inode_tree_bytes_below_track,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tantivy::Index;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LexicalMutationTimings {
    pub(crate) writer_mutation_ns: u64,
    pub(crate) text_authority_ns: u64,
    pub(crate) file_authority_ns: u64,
}

impl LexicalAdapter {
    /// The writer guard spans op-apply, commit, and the text-authority
    /// write so partial commits cannot interleave with sibling builds for
    /// the same generation; the text-authority write reads `guarded.index`
    /// after commit.
    pub(crate) fn commit_ops_under_lock(
        &self,
        handle: &Arc<Mutex<GenerationWriter>>,
        key: &GenKey,
        ops: &[LexicalChannelOp],
    ) -> Result<LexicalMutationTimings, CoreError> {
        let mut guarded = handle
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writer poisoned: {err}")))?;
        let writer_started = Instant::now();
        let generation_dir = self.index_path(key);
        let file_plan = crate::file_authority::plan_ops(&generation_dir, ops)?;
        // Planned before any op runs: the retired documents are only
        // nameable while the pre-mutation index still holds them, and the
        // doc ids the ops store come from the plan's watermark.
        let mut plan =
            plan_text_authority_delta(&guarded.index, &self.fields, ops, &generation_dir)?;
        let mut needs_commit = false;
        for op in ops {
            if self.apply_op(&guarded.writer, key, op, &mut plan.allocator)? {
                needs_commit = true;
            }
        }
        if !needs_commit {
            return Ok(LexicalMutationTimings {
                writer_mutation_ns: crate::adapter_ingest::elapsed_stage_ns(writer_started)?,
                ..LexicalMutationTimings::default()
            });
        }
        let _opstamp = guarded
            .writer
            .commit()
            .map_err(|err| CoreError::Storage(format!("lexical: commit: {err}")))?;
        let writer_mutation_ns = crate::adapter_ingest::elapsed_stage_ns(writer_started)?;
        // The text-authority write happens under the writer lock (it reads
        // the committed index); the accounting is folded in after the lock
        // is released so the stats lock is never nested inside the writer's.
        let text_authority_started = Instant::now();
        let written = self.write_text_authority(&generation_dir, key, &guarded.index, plan)?;
        let text_authority_ns = crate::adapter_ingest::elapsed_stage_ns(text_authority_started)?;
        let file_authority_started = Instant::now();
        crate::file_authority::apply_plan(&generation_dir, file_plan)?;
        let file_authority_ns = crate::adapter_ingest::elapsed_stage_ns(file_authority_started)?;
        drop(guarded);
        if let Some((rebuilt, receipt)) = written {
            self.record_text_authority_write(rebuilt, receipt)?;
        }
        Ok(LexicalMutationTimings {
            writer_mutation_ns,
            text_authority_ns,
            file_authority_ns,
        })
    }

    /// Publish the text authority the plan decided on, after the commit.
    ///
    /// Returns whether it was a rebuild and what it did, or `None` when the
    /// batch touched no text.
    pub(crate) fn write_text_authority(
        &self,
        generation_dir: &Path,
        key: &GenKey,
        index: &Index,
        plan: TextAuthorityPlan,
    ) -> Result<Option<(bool, TextAuthorityWriteReceipt)>, CoreError> {
        let max_doc_id = plan.allocator.max_doc_id();
        match plan.write {
            TextAuthorityWrite::None => Ok(None),
            TextAuthorityWrite::Rebuild => {
                self.invalidate_regex_match_cache_generation(key)?;
                let docs = collect_text_authority_docs(index, &self.fields)?;
                let receipt = text_authority::rebuild(
                    generation_dir,
                    key.generation,
                    docs,
                    plan.prior.as_ref(),
                    max_doc_id,
                )?;
                Ok(Some((true, receipt)))
            }
            TextAuthorityWrite::Incremental {
                retired,
                touched_shards,
            } => {
                self.invalidate_regex_match_cache_generation(key)?;
                let Some(prior) = plan.prior.as_ref() else {
                    return Err(CoreError::InvalidContract(
                        "lexical: incremental text authority update planned without a prior manifest"
                            .to_string(),
                    ));
                };
                let receipt = text_authority::update(
                    generation_dir,
                    key.generation,
                    prior,
                    &retired,
                    &plan.allocator.added,
                    &touched_shards,
                    max_doc_id,
                )?;
                Ok(Some((false, receipt)))
            }
        }
    }
}

impl SealedGenerationVisitor for LoadedGeneration {
    fn text_authority_shard(&mut self, index: u64, body: ShardBody) -> Result<(), CoreError> {
        self.shards.push((index, body));
        Ok(())
    }

    fn file_authority(
        &mut self,
        files: Vec<crate::file_authority::SourceFile>,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        self.file_authority = Some(crate::file_authority::from_verified_files(files, budget)?);
        Ok(())
    }

    fn overlay(&mut self, snapshot: OverlaySnapshot) -> Result<(), CoreError> {
        match snapshot {
            OverlaySnapshot::RepoMetadata(payload) => self.repo_metadata = Some(payload),
            OverlaySnapshot::CommitRecency(shard) => self.repo_commit_recency = Some(shard),
            OverlaySnapshot::Meta(shard) => self.repo_meta = Some(shard),
            OverlaySnapshot::Topic(shard) => self.repo_topic = Some(shard),
            OverlaySnapshot::Description(shard) => self.repo_description = Some(shard),
            OverlaySnapshot::FileOwnership(shard) => self.file_ownership = Some(shard),
            OverlaySnapshot::Contributor(shard) => self.file_contributor = Some(shard),
        }
        Ok(())
    }
}

/// Heap owned by decoded overlays. This uses actual container element sizes
/// and string/vector capacities; allocator bookkeeping remains outside the
/// registry's documented estimate.
trait OverlayHeapBytes {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError>;
}

fn usize_bytes(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value).map_err(|error| {
        CoreError::Storage(format!("lexical: overlay heap estimate overflow: {error}"))
    })
}

fn add_heap_bytes(total: u64, value: u64) -> Result<u64, CoreError> {
    total
        .checked_add(value)
        .ok_or_else(|| CoreError::Storage("lexical: overlay heap estimate overflow".into()))
}

impl OverlayHeapBytes for String {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError> {
        usize_bytes(self.capacity())
    }
}

impl OverlayHeapBytes for u64 {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError> {
        Ok(0)
    }
}

impl<T: OverlayHeapBytes> OverlayHeapBytes for Vec<T> {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError> {
        let elements = usize_bytes(self.capacity())?
            .checked_mul(usize_bytes(std::mem::size_of::<T>())?)
            .ok_or_else(|| CoreError::Storage("lexical: overlay heap estimate overflow".into()))?;
        self.iter().try_fold(elements, |total, value| {
            add_heap_bytes(total, value.overlay_heap_bytes()?)
        })
    }
}

impl<K: Ord + OverlayHeapBytes, V: OverlayHeapBytes> OverlayHeapBytes for BTreeMap<K, V> {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError> {
        self.iter().try_fold(0_u64, |total, (key, value)| {
            let total = add_heap_bytes(total, usize_bytes(std::mem::size_of::<(K, V)>())?)?;
            let total = add_heap_bytes(total, key.overlay_heap_bytes()?)?;
            add_heap_bytes(total, value.overlay_heap_bytes()?)
        })
    }
}

impl<T: Ord + OverlayHeapBytes> OverlayHeapBytes for BTreeSet<T> {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError> {
        self.iter().try_fold(0_u64, |total, value| {
            let total = add_heap_bytes(total, usize_bytes(std::mem::size_of::<T>())?)?;
            add_heap_bytes(total, value.overlay_heap_bytes()?)
        })
    }
}

impl OverlayHeapBytes for FileContributorIdentityEntry {
    fn overlay_heap_bytes(&self) -> Result<u64, CoreError> {
        let mut bytes = self.canonical.overlay_heap_bytes()?;
        if let Some(name) = &self.name {
            bytes = add_heap_bytes(bytes, name.overlay_heap_bytes()?)?;
        }
        if let Some(email) = &self.email {
            bytes = add_heap_bytes(bytes, email.overlay_heap_bytes()?)?;
        }
        Ok(bytes)
    }
}

impl LoadedGeneration {
    fn overlay_heap_bytes_estimate(&self) -> Result<u64, CoreError> {
        let mut bytes = match &self.repo_metadata {
            Some(metadata) => metadata.contexts.overlay_heap_bytes()?,
            None => 0,
        };
        for family in [
            self.repo_commit_recency.as_ref().map(|shard| {
                shard
                    .latest_committer_time_ms_by_repo_id
                    .overlay_heap_bytes()
            }),
            self.repo_meta
                .as_ref()
                .map(|shard| shard.meta_by_repo_id.overlay_heap_bytes()),
            self.repo_topic
                .as_ref()
                .map(|shard| shard.topics_by_repo_id.overlay_heap_bytes()),
            self.repo_description
                .as_ref()
                .map(|shard| shard.descriptions_by_repo_id.overlay_heap_bytes()),
            self.file_ownership
                .as_ref()
                .map(|shard| shard.owners_by_repo_id.overlay_heap_bytes()),
            self.file_contributor
                .as_ref()
                .map(|shard| shard.contributors_by_repo_id.overlay_heap_bytes()),
        ]
        .into_iter()
        .flatten()
        {
            bytes = add_heap_bytes(bytes, family?)?;
        }
        Ok(bytes)
    }
}

/// The repo-metadata authorities a sealed manifest says the generation
/// carries: the explicit capability set a query's typed refusals are
/// answered from.
pub(crate) fn materialized_authorities(overlays: &[OverlayFamily]) -> RepoMetadataAuthoritiesV1 {
    overlays.iter().fold(
        RepoMetadataAuthoritiesV1::NONE,
        |set, family| match family {
            OverlayFamily::RepoMetadata => set,
            OverlayFamily::CommitRecency => set.with(RepoMetadataAuthorityV1::CommitRecency),
            OverlayFamily::Meta => set.with(RepoMetadataAuthorityV1::Meta),
            OverlayFamily::Topic => set.with(RepoMetadataAuthorityV1::Topic),
            OverlayFamily::Description => set.with(RepoMetadataAuthorityV1::Description),
            OverlayFamily::FileOwnership => set.with(RepoMetadataAuthorityV1::FileOwnership),
            OverlayFamily::Contributor => set.with(RepoMetadataAuthorityV1::Contributor),
        },
    )
}

impl LexicalIndexOpenPort for LexicalAdapter {
    fn preflight_query_primitives(
        &self,
        plan: &quanta_index_core::ValidatedLexicalPlan,
        budget: &quanta_index_core::RequestBudgetV1,
    ) -> Result<(), CoreError> {
        crate::planner::LexicalPlanner::validate_query_primitives(plan, &self.regex_policy, budget)
    }

    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        budget.checkpoint("lexical:cold-open")?;
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let path = self.index_path(&key);
        if !path.is_dir() {
            return Err(CoreError::NotFound(format!(
                "lexical: no index at {}",
                path.display()
            )));
        }
        let identity = read_lexical_sealed_identity(&path)?;
        if identity.repo_id != *repo
            || identity.revision_id != *revision
            || identity.track != SearchPlaneTrackKind::Lexical
            || identity.manifest_generation != generation
        {
            return Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch,
                message: format!(
                    "lexical: active generation identity scope disagrees with path {}",
                    path.display()
                ),
            });
        }
        self.open_sealed(&path, &identity, Some(budget))
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "proven open")?;
        let searcher = self.open_sealed(&generation_dir, &observed, None)?;
        sync_generation_directory(&generation_dir)?;
        Ok(searcher)
    }
}

impl LexicalAdapter {
    /// Open the sealed generation at `path` whose identity is `identity`:
    /// the walk every door shares, keeping what it decodes.
    ///
    /// Every file a query decodes is read once, proved and decoded here;
    /// the index is opened from the proved commit and its segment files are
    /// proved present at their committed length.
    pub(crate) fn open_sealed(
        &self,
        path: &Path,
        identity: &GenerationSnapshot,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let mut loaded = LoadedGeneration::default();
        let verified = walk_sealed_generation(path, identity, &mut loaded, budget)?;
        self.record_coverage_read(
            crate::sealed_generation::coverage::CoverageReadPhase::Open,
            verified.coverage_read_stats,
        )?;
        let reader = verified.reader;
        let ranked_keys = verified.ranked_keys;
        let overlay_bytes = loaded.overlay_heap_bytes_estimate()?;
        let text_authority = verified
            .manifest
            .text_authority
            .as_ref()
            .map(|_files| ShardedTextAuthority::from_proved_shards(loaded.shards))
            .transpose()?;
        let overlays: Vec<OverlayFamily> = verified
            .manifest
            .overlay_commitments()
            .map(|(family, _artifact)| family)
            .collect();
        let coverage_bytes = crate::sealed_generation::coverage::coverage_heap_bytes_estimate(
            verified.coverage.as_ref(),
            verified.source_publication.as_ref(),
        )?;
        let resident_bytes_estimate = resident_bytes_estimate(
            &self.state_root,
            path,
            text_authority.as_ref(),
            &ranked_keys,
        )?
        .checked_add(coverage_bytes)
        .and_then(|bytes| {
            bytes.checked_add(
                loaded
                    .file_authority
                    .as_ref()
                    .map_or(0, crate::file_authority::FileAuthority::heap_bytes_estimate),
            )
        })
        .and_then(|bytes| bytes.checked_add(overlay_bytes))
        .ok_or_else(|| CoreError::Storage("lexical resident byte estimate overflow".into()))?;
        let artifact_identity = LexicalArtifactIdentityV1 {
            manifest_digest: verified.manifest.manifest_digest.clone(),
            normalizer: TextNormalizerVersionV1 {
                major: verified.manifest.normalizer.major,
                minor: verified.manifest.normalizer.minor,
            },
            repo_metadata: materialized_authorities(&overlays),
        };
        if let Some(budget) = budget {
            budget.checkpoint("lexical:cold-open:publish")?;
        }
        Ok(Box::new(TantivySearcher {
            source_coverage: verified.coverage,
            source_publication_event: verified.source_publication,
            repo_id: identity.repo_id.clone(),
            revision_id: identity.revision_id.clone(),
            generation: identity.manifest_generation,
            fields: self.fields.clone(),
            reader,
            ranked_keys,
            repo_metadata: loaded.repo_metadata,
            regex_match_cache: Arc::clone(&self.regex_match_cache),
            regex_policy: self.regex_policy,
            execution_budget: self.execution_budget,
            text_authority,
            file_authority: loaded.file_authority,
            repo_commit_recency: loaded.repo_commit_recency,
            repo_meta: loaded.repo_meta,
            repo_topic: loaded.repo_topic,
            repo_description: loaded.repo_description,
            file_ownership: loaded.file_ownership,
            file_contributor: loaded.file_contributor,
            resident_bytes_estimate,
            artifact_identity,
        }))
    }
}

/// What an opened handle keeps resident.
///
/// Every mapped file under the generation directory except decoded sidecars
/// (each inode counted once), plus the text-authority and ranked-key heap.
/// The caller adds decoded file authority, coverage, and overlay heap before
/// reporting to the registry; their encoded files must not be counted again.
pub(crate) fn resident_bytes_estimate(
    track_root: &Path,
    generation_dir: &Path,
    text_authority: Option<&ShardedTextAuthority>,
    ranked_keys: &crate::ranked_keys::RankedKeyTables,
) -> Result<u64, CoreError> {
    let skip = |name: &str| {
        is_writer_lock_entry(name)
            || name == TEXT_AUTHORITY_DIR_NAME
            || name == crate::file_authority::DIR
            || crate::ranked_keys::is_ranked_key_entry(name)
            || OverlayFamily::from_file_name(name).is_some()
            || name == crate::sealed_generation::coverage::SOURCE_FILE_COVERAGE_FILE_NAME
            || crate::sealed_generation::coverage::is_coverage_page(name)
    };
    let mapped =
        unique_inode_tree_bytes_below_track(track_root, generation_dir, &skip).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure resident bytes of {}: {err}",
                generation_dir.display()
            ))
        })?;
    let decoded = text_authority.map_or(0, ShardedTextAuthority::heap_bytes_estimate);
    let ranked_key_heap = ranked_keys.heap_bytes_estimate()?;
    mapped
        .checked_add(decoded)
        .and_then(|bytes| bytes.checked_add(ranked_key_heap))
        .ok_or_else(|| CoreError::Storage("lexical resident byte estimate overflow".into()))
}

#[cfg(test)]
mod tests {
    use super::{OverlayFamily, resident_bytes_estimate};

    #[test]
    fn decoded_sidecar_files_do_not_consume_mapped_file_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = crate::test_support::generation_fixture()?;
        std::fs::write(directory.path().join("index.bin"), b"index")?;
        for family in OverlayFamily::ALL {
            std::fs::write(directory.path().join(family.file_name()), vec![0_u8; 4096])?;
        }
        std::fs::write(
            directory.path().join("source-file-coverage.cbor"),
            vec![0_u8; 4096],
        )?;
        std::fs::write(
            directory
                .path()
                .join("source-file-coverage-page-00-test.cbor"),
            vec![0_u8; 4096],
        )?;
        let file_authority_dir = directory.path().join(crate::file_authority::DIR);
        std::fs::create_dir_all(&file_authority_dir)?;
        std::fs::write(file_authority_dir.join("manifest.cbor"), vec![0_u8; 4096])?;
        std::fs::write(file_authority_dir.join("source.bin"), vec![0_u8; 4096])?;
        let ranked_keys = crate::ranked_keys::RankedKeyTables::bind(Vec::new(), &[])?;
        let estimate =
            resident_bytes_estimate(directory.track_path(), directory.path(), None, &ranked_keys)?;
        if estimate != 5 {
            return Err(format!("decoded sidecar files were charged: {estimate} bytes").into());
        }
        Ok(())
    }
}
