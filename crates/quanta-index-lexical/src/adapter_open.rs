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
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::domains::generation::unique_inode_tree_bytes;
use quanta_index_core::{
    CoreError, LexicalArtifactIdentityV1, LexicalIndexOpenPort, LexicalSearcher,
    RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, TextNormalizerVersionV1,
};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tantivy::{Index, IndexReader, ReloadPolicy};

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
    ) -> Result<(), CoreError> {
        let mut guarded = handle
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writer poisoned: {err}")))?;
        let generation_dir = self.index_path(key);
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
            return Ok(());
        }
        let _opstamp = guarded
            .writer
            .commit()
            .map_err(|err| CoreError::Storage(format!("lexical: commit: {err}")))?;
        // The text-authority write happens under the writer lock (it reads
        // the committed index); the accounting is folded in after the lock
        // is released so the stats lock is never nested inside the writer's.
        let written = self.write_text_authority(&generation_dir, key, &guarded.index, plan)?;
        drop(guarded);
        if let Some((rebuilt, receipt)) = written {
            self.record_text_authority_write(rebuilt, receipt)?;
        }
        Ok(())
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
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
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
        self.open_sealed(&path, &identity)
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "proven open")?;
        let searcher = self.open_sealed(&generation_dir, &observed)?;
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
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let mut loaded = LoadedGeneration::default();
        let verified = walk_sealed_generation(path, identity, &mut loaded)?;
        let reader: IndexReader = verified
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|err| CoreError::Storage(format!("lexical: reader: {err}")))?;
        reader
            .reload()
            .map_err(|err| CoreError::Storage(format!("lexical: reader reload: {err}")))?;
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
        let resident_bytes_estimate = resident_bytes_estimate(path, text_authority.as_ref())?
            .checked_add(coverage_bytes)
            .ok_or_else(|| CoreError::Storage("lexical resident byte estimate overflow".into()))?;
        let artifact_identity = LexicalArtifactIdentityV1 {
            manifest_digest: verified.manifest.manifest_digest.clone(),
            normalizer: TextNormalizerVersionV1 {
                major: verified.manifest.normalizer.major,
                minor: verified.manifest.normalizer.minor,
            },
            repo_metadata: materialized_authorities(&overlays),
        };
        Ok(Box::new(TantivySearcher {
            source_coverage: verified.coverage,
            source_publication_event: verified.source_publication,
            repo_id: identity.repo_id.clone(),
            revision_id: identity.revision_id.clone(),
            generation: identity.manifest_generation,
            fields: self.fields.clone(),
            reader,
            repo_metadata: loaded.repo_metadata,
            regex_match_cache: Arc::clone(&self.regex_match_cache),
            regex_policy: self.regex_policy,
            execution_budget: self.execution_budget,
            text_authority,
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
/// Every mapped or decoded file under the generation directory except the
/// text-authority sidecars (each inode counted once), plus the decoded text
/// authority's heap estimate in their place.
pub(crate) fn resident_bytes_estimate(
    generation_dir: &Path,
    text_authority: Option<&ShardedTextAuthority>,
) -> Result<u64, CoreError> {
    let skip = |name: &str| is_writer_lock_entry(name) || name == TEXT_AUTHORITY_DIR_NAME;
    let mapped =
        unique_inode_tree_bytes(&[generation_dir.to_path_buf()], &skip).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure resident bytes of {}: {err}",
                generation_dir.display()
            ))
        })?;
    let decoded = text_authority.map_or(0, ShardedTextAuthority::heap_bytes_estimate);
    Ok(mapped.saturating_add(decoded))
}
