//! Opening a sealed generation for query: the walk every door shares and the searcher it builds.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::generation_dir::{is_writer_lock_entry, sync_generation_directory};
use crate::index_store::read_lexical_sealed_identity;
use crate::overlay_codec::OverlayFamily;
use crate::sealed_generation::{SealedGenerationVisitor, walk_sealed_generation};
use crate::text_authority::{NativeAuthority, TEXT_AUTHORITY_DIR_NAME, TextAuthorityWriteReceipt};
use crate::text_authority_plan::plan_text_authority_delta;
use crate::{
    GenKey, GenerationWriter, LexicalAdapter, LoadedGeneration, OverlaySnapshot, TantivySearcher,
    TextAuthorityWrite, text_authority,
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

#[derive(Clone, Copy, Debug, Default)]
#[expect(
    clippy::struct_field_names,
    reason = "these internal timing fields preserve explicit nanosecond units across the ingest/report boundary"
)]
pub(crate) struct LexicalMutationTimings {
    pub(crate) writer_mutation_ns: u64,
    pub(crate) text_authority_ns: u64,
    pub(crate) text_authority_collect_ns: Option<u64>,
    pub(crate) text_authority_shard_build_ns: Option<u64>,
    pub(crate) text_authority_publish_ns: Option<u64>,
    pub(crate) file_authority_ns: u64,
    pub(crate) file_authority_source_write_ns: Option<u64>,
}

impl LexicalAdapter {
    /// Commit the index under its writer mutex. The caller retains the target
    /// and base generation mutation guards through both authority publications.
    pub(crate) fn commit_ops_under_lock(
        &self,
        handle: &Arc<Mutex<GenerationWriter>>,
        key: &GenKey,
        ops: &[LexicalChannelOp],
        file_plan: Option<&mut crate::file_authority::FileAuthorityPlanCache>,
        publication: &mut crate::lexical_publication::PublicationContext,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<LexicalMutationTimings, CoreError> {
        // Preparation overlaps coverage/base cloning. Join outside the writer
        // mutex and keep the permit until both packets are consumed or dropped.
        let mut prepared = publication
            .compaction_worker
            .take()
            .map(|worker| {
                worker.finish_for(
                    publication.file_base.as_ref(),
                    publication.native_base.as_ref(),
                    publication.maintenance_policy.as_ref(),
                )
            })
            .transpose()?
            .unwrap_or_default();
        let mut guarded = handle
            .lock()
            .map_err(|error| CoreError::Storage(format!("lexical writer poisoned: {error}")))?;
        let writer_started = Instant::now();
        let generation_dir = self.index_path(key);
        let mut mutation = match file_plan {
            Some(cache) => cache.take(&generation_dir, ops)?,
            None => crate::file_authority::plan_ops(&generation_dir, ops)?,
        };
        mutation.operations_sha256 = publication.operations_sha256;
        let mut plan = if let Some(plan) = publication.native_plan.take() {
            plan
        } else {
            plan_text_authority_delta(
                &publication.store,
                publication.native_base.as_ref(),
                ops,
                &publication.reservation.native_text,
            )?
        };
        let mut needs_commit = false;
        let mut census = crate::doc_census::CensusBuildObservation::new(diagnostic);
        let applied = (|| {
            for op in ops {
                needs_commit |= self.apply_op(
                    &guarded.writer,
                    key,
                    op,
                    &mut plan.allocator,
                    &mut census,
                    publication.store.sync_totals(),
                )?;
            }
            plan.allocator.validate_fully_consumed()
        })();
        census.emit(applied.is_ok());
        applied?;
        if needs_commit {
            let _opstamp = guarded
                .writer
                .commit()
                .map_err(|error| CoreError::Storage(format!("lexical: commit: {error}")))?;
        }
        // No further operation reads or mutates the Tantivy writer. Keep the
        // generation fence, but release this mutex before authority I/O and
        // potentially large clean-run merges.
        drop(guarded);
        let writer_mutation_ns = crate::stage_timing::elapsed_stage_ns(writer_started)?;
        let text_started = Instant::now();
        let lease = publication.lease.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("lexical publication lost its adoption lease".into())
        })?;
        let (native, receipt) = match plan.write {
            TextAuthorityWrite::None => {
                let prior = plan.prior.unwrap_or_else(|| {
                    text_authority::NativeRoot::empty(
                        publication.reservation.native_text.scope_sha256,
                    )
                });
                // A real no-op source event may pay existing physical debt;
                // it consumes no fresh logical IDs or maintenance generation.
                let maintenance_due = match &publication.maintenance_policy {
                    Some(policy) => policy.merge_due(prior.run_registry.snapshot()?),
                    None => false,
                };
                let written = if prior.compaction_due.entries != 0 || maintenance_due {
                    text_authority::publish_delta_prepared_with_policy(
                        &publication.store,
                        lease,
                        Some(&prior),
                        &[],
                        &[],
                        &plan.allocator.added,
                        false,
                        key.generation,
                        publication.reservation.native_text.scope_sha256,
                        prepared.take_native_for(
                            publication.native_base.as_ref(),
                            publication.maintenance_policy.as_ref(),
                        )?,
                        publication.maintenance_policy.as_ref(),
                        crate::lexical_hard_scan::lane(
                            publication.hard_scan.as_ref(),
                            crate::lexical_store::Authority::Native,
                        ),
                    )?
                } else {
                    (prior, TextAuthorityWriteReceipt::default())
                };
                publication.native_mutation = Some(crate::lexical_publication::NativeMutation {
                    sources: Vec::new(),
                    retired: Vec::new(),
                    added: plan.allocator.added,
                    clear_all: false,
                });
                written
            }
            TextAuthorityWrite::Delta {
                retired,
                source_edits,
                clear_all,
            } => {
                self.invalidate_regex_match_cache_generation(key)?;
                let written = crate::causal_profile::observe_result(
                    diagnostic,
                    quanta_index_core::DiagnosticStageV1::NativePublish,
                    |_diagnostic| {
                        crate::causal_profile::timed_work("native_authority_publish", || {
                            text_authority::publish_delta_prepared_with_policy(
                                &publication.store,
                                lease,
                                plan.prior.as_ref(),
                                &source_edits,
                                &retired,
                                &plan.allocator.added,
                                clear_all,
                                key.generation,
                                publication.reservation.native_text.scope_sha256,
                                prepared.take_native_for(
                                    publication.native_base.as_ref(),
                                    publication.maintenance_policy.as_ref(),
                                )?,
                                publication.maintenance_policy.as_ref(),
                                crate::lexical_hard_scan::lane(
                                    publication.hard_scan.as_ref(),
                                    crate::lexical_store::Authority::Native,
                                ),
                            )
                        })
                    },
                )?;
                publication.native_mutation = Some(crate::lexical_publication::NativeMutation {
                    sources: source_edits,
                    retired,
                    added: plan.allocator.added,
                    clear_all,
                });
                written
            }
        };
        let native_bytes = crate::channel_payloads::encode_cbor(&native, "Native authority root")?;
        crate::lexical_publication::write_root_observed(
            &generation_dir,
            crate::lexical_publication::NATIVE_ROOT_NAME,
            &native_bytes,
            publication.store.sync_totals(),
        )?;
        publication.native_root = Some(native);
        let text_authority_ns = crate::stage_timing::elapsed_stage_ns(text_started)?;
        let file_started = Instant::now();
        let file = crate::causal_profile::observe_result(
            diagnostic,
            quanta_index_core::DiagnosticStageV1::FilePublish,
            |_diagnostic| {
                crate::causal_profile::timed_work("file_authority_publish", || {
                    crate::file_authority::paged::build_for_seal_prepared_with_policy(
                        &publication.store,
                        lease,
                        publication.file_base.as_ref(),
                        &publication.reservation.file,
                        &mutation,
                        prepared.take_file_for(
                            publication.file_base.as_ref(),
                            publication.maintenance_policy.as_ref(),
                        )?,
                        publication.maintenance_policy.as_ref(),
                        crate::lexical_hard_scan::lane(
                            publication.hard_scan.as_ref(),
                            crate::lexical_store::Authority::File,
                        ),
                    )
                })
            },
        )?;
        crate::lexical_publication::write_root_observed(
            &generation_dir,
            crate::lexical_publication::FILE_ROOT_NAME,
            &file.root_bytes,
            publication.store.sync_totals(),
        )?;
        publication.file_root = Some(file.root);
        publication.file_mutation = Some(mutation);
        let file_authority_ns = crate::stage_timing::elapsed_stage_ns(file_started)?;
        self.record_text_authority_write(publication.native_base.is_none(), receipt)?;
        Ok(LexicalMutationTimings {
            writer_mutation_ns,
            text_authority_ns,
            text_authority_collect_ns: None,
            // Paged publication builds segments and writes their pooled
            // objects in one call; only the enclosing stage is measured.
            text_authority_shard_build_ns: None,
            text_authority_publish_ns: None,
            file_authority_ns,
            // The File stage includes map and root publication, not just
            // source-body writes.
            file_authority_source_write_ns: None,
        })
    }
}

impl SealedGenerationVisitor for LoadedGeneration {
    fn native_authority(
        &mut self,
        store: crate::lexical_store::ObjectStore,
        root: text_authority::NativeRoot,
    ) -> Result<(), CoreError> {
        self.text_authority = Some(text_authority::NativeAuthority::open(store, root)?);
        Ok(())
    }

    fn file_authority(
        &mut self,
        root_bytes: &[u8],
        store: crate::lexical_store::ObjectStore,
    ) -> Result<(), CoreError> {
        self.file_authority = Some(crate::file_authority::paged::PagedFileAuthority::open(
            root_bytes, store,
        )?);
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
        self.open_sealed(&path, &identity, Some(budget), budget.diagnostic_context())
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "proven open")?;
        let searcher = self.open_sealed(&generation_dir, &observed, None, None)?;
        sync_generation_directory(&generation_dir)?;
        Ok(searcher)
    }
}

impl LexicalAdapter {
    /// Open the sealed generation at `path` whose identity is `identity`:
    /// the walk every door shares, keeping what it decodes.
    ///
    /// Control metadata is authenticated here. Immutable source, posting and
    /// text leaves are pinned at their committed length and authenticated on use.
    /// Their semantic relationships were independently checked before sealing.
    pub(crate) fn open_sealed(
        &self,
        path: &Path,
        identity: &GenerationSnapshot,
        budget: Option<&RequestBudgetV1>,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let mut loaded = LoadedGeneration::default();
        let verified = walk_sealed_generation(
            path,
            identity,
            &mut loaded,
            budget,
            &self.open_existing_store()?,
        )?;
        self.record_coverage_read(
            crate::sealed_generation::coverage::CoverageReadPhase::Open,
            verified.coverage_read_stats,
        )?;
        let reader = verified.reader;
        let ranked_keys = verified.ranked_keys;
        let live_bm25 = verified.live_bm25;
        let overlay_bytes = loaded.overlay_heap_bytes_estimate()?;
        let text_authority = loaded.text_authority;
        let overlays: Vec<OverlayFamily> = verified
            .manifest
            .overlay_commitments()
            .map(|(family, _artifact)| family)
            .collect();
        let coverage_bytes = crate::sealed_generation::coverage::coverage_heap_bytes_estimate(
            verified.coverage.as_ref(),
            verified.source_publication.as_ref(),
        )?;
        let live_bm25_bytes = live_bm25.heap_bytes_estimate()?;
        let file_authority_bytes = loaded.file_authority.as_ref().map_or(
            Ok(0),
            crate::file_authority::paged::PagedFileAuthority::heap_bytes_estimate,
        )?;
        let resident_bytes_estimate = resident_bytes_estimate(
            &self.state_root,
            path,
            text_authority.as_ref(),
            &ranked_keys,
            diagnostic,
        )?
        .checked_add(coverage_bytes)
        .and_then(|bytes| bytes.checked_add(file_authority_bytes))
        .and_then(|bytes| bytes.checked_add(overlay_bytes))
        .and_then(|bytes| bytes.checked_add(live_bm25_bytes))
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
            live_bm25,
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
    text_authority: Option<&NativeAuthority>,
    ranked_keys: &crate::ranked_keys::RankedKeyTables,
    diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
) -> Result<u64, CoreError> {
    let skip = |name: &str| {
        is_writer_lock_entry(name)
            || name == TEXT_AUTHORITY_DIR_NAME
            || name == crate::file_authority::DIR
            || crate::ranked_keys::is_ranked_key_entry(name)
            || name == crate::sealed_generation::live_bm25::FILE_NAME
            || OverlayFamily::from_file_name(name).is_some()
            || name == crate::sealed_generation::coverage::SOURCE_FILE_COVERAGE_FILE_NAME
            || crate::sealed_generation::coverage::is_coverage_page(name)
    };
    let mapped = unique_inode_tree_bytes_below_track(track_root, generation_dir, &skip, diagnostic)
        .map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure resident bytes of {}: {err}",
                generation_dir.display()
            ))
        })?;
    let decoded = text_authority.map_or(0, NativeAuthority::heap_bytes_estimate);
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
        let estimate = resident_bytes_estimate(
            directory.track_path(),
            directory.path(),
            None,
            &ranked_keys,
            None,
        )?;
        if estimate != 5 {
            return Err(format!("decoded sidecar files were charged: {estimate} bytes").into());
        }
        Ok(())
    }
}
