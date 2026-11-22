//! QI-BB-020: auxiliary authority rows are durable in the catalog, survive
//! a pinned Legacy daemon restart, and follow generation retention.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::Path;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, ChunkId, ChunkRecord, GenerationPin, HistoryOrderV1,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1,
    SearchScopeKey, SearchScopeSurface, SourceFileKey, StructuralIngestBatch,
    StructuralReplaceScope, StructuralTreeRecord,
    lex::{DirtyRecord, LanguageCode},
};
use quanta_index_core::{AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_sdk::{DirtyBatch, PublishedBatchEvidence, QuantaIndex, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    history_fixture::HistoryFixtureSpec,
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, legacy_source_delta_from_chunks,
        legacy_source_replacement_from_chunks, publish_legacy_source_batch, request_when_ready,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
    structural_function_tree_record,
};

type TestResult = Result<(), Box<dyn Error>>;

const REPO: &str = "repo-e2e";
const REVISION: &str = "rev-e2e";

fn identity() -> Result<(RepoId, RevisionId), Box<dyn Error>> {
    Ok((RepoId::new(REPO)?, RevisionId::new(REVISION)?))
}

fn start(
    binary: &LegacySearchdBinary,
    root: &Path,
    history_max_generations: usize,
) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
    let command = binary.benchmark_command(root, history_max_generations)?;
    SearchdBinaryProcess::start_with_pinned_command(root, command)
}

fn require_durable(receipt: &BatchPublishReceipt, domain: &str) -> TestResult {
    if !receipt.applied || receipt.durable_sequence == 0 {
        return Err(format!("{domain} was not durably published").into());
    }
    Ok(())
}

/// Rows per (domain, generation), read only after the daemon has stopped.
fn rows_by_domain_and_generation(
    state_root: &Path,
) -> Result<BTreeMap<(AuxiliaryDomainV1, u64), u64>, Box<dyn Error>> {
    let catalog = SqliteCatalog::open(state_root, Duration::from_millis(500))?;
    let mut counts: BTreeMap<(AuxiliaryDomainV1, u64), u64> = BTreeMap::new();
    catalog.for_each_row(&mut |row| {
        let count = counts
            .entry((row.key.domain, row.key.generation.generation.get()))
            .or_insert(0);
        *count = count.saturating_add(1);
        Ok(())
    })?;
    Ok(counts)
}

fn source_batch(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
    chunk_number: u64,
    path: &str,
    content: &str,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-{chunk_number}-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(content.len())?,
        start_line: 1,
        end_line: 2,
        text: content.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    if generation == 1 {
        legacy_source_replacement_from_chunks(repo, revision, generation, vec![chunk])
    } else {
        // The original staged publisher carries earlier files forward.
        legacy_source_delta_from_chunks(repo, revision, generation, &[], &[], vec![chunk])
    }
}

fn history_spec<'a>(
    commit_sha: &'a str,
    path: &'a str,
    message: &'a str,
    tag_name: &'a str,
    added_text: &'a str,
    touched_text: &'a str,
) -> HistoryFixtureSpec<'a> {
    HistoryFixtureSpec {
        commit_sha,
        file_path: path,
        author: "alice",
        committer: "alice",
        message,
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        ref_name: "refs/heads/main",
        tag_name,
        added_text,
        removed_text: "",
        touched_text,
    }
}

fn publish_history(
    client: &QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    spec: &HistoryFixtureSpec<'_>,
) -> TestResult {
    let mut batch = spec.build_batch(repo.clone(), revision.clone(), generation)?;
    stamp_batch_digest_v1(&mut batch)?;
    let receipt = client.history().publish_history_borrowed_v1(&batch)?;
    require_durable(&receipt, "History")
}

fn publish_dirty(
    client: &QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    path: &str,
    chunk_number: u64,
    applied_at_ms: u64,
) -> TestResult {
    let batch = DirtyBatch::new(repo.clone(), revision.clone(), generation, applied_at_ms).upsert(
        DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new(format!("e2e-{chunk_number}-{path}")),
            applied_at_ms,
            payload_hash: [0x5a; 32],
        },
    );
    let receipt = client.runtime().publish_dirty(&batch)?;
    require_durable(&receipt, "Dirty")
}

fn history_commits(
    client: &QuantaIndex,
    pin: &GenerationPin,
) -> Result<Vec<String>, Box<dyn Error>> {
    let answer = request_when_ready(|| {
        client
            .history()
            .query()
            .sourcegraph("type:commit fix")
            .pinned(pin.clone())
            .top_k(5)
            .order(HistoryOrderV1::Recency)
            .execute()
    })?;
    if answer.generation != *pin {
        return Err("History query selected a different generation".into());
    }
    Ok(answer
        .commits
        .iter()
        .map(|commit| commit.sha.to_hex())
        .collect())
}

fn publish_structural_after_source(
    client: &QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    path: &str,
    content: &str,
) -> TestResult {
    let tree = structural_function_tree_record(path, content, "aux_restart_alpha")?;
    // The old fixture's first Source seal consumes sequence 1, and its
    // structural_tree_batch consumes sequence 2. It is unsealed Delta with
    // no base, after Source seal but before activation.
    let batch = StructuralIngestBatch {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation,
        base_generation: None,
        manifest_digest: format!("struct:{path}:{}", generation.get()),
        batch_digest: String::new(),
        mode: BatchIngestMode::Delta,
        replace_scopes: vec![StructuralReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                source_file: SourceFileKey {
                    source_repo_id: repo.clone(),
                    repo_relative_path: RepoRelativePath::new(path),
                },
            },
            scope_digest: format!("struct-scope:{path}:2"),
            trees: vec![StructuralTreeRecord {
                chunk_id: ChunkId::new(format!("e2e-1-{path}")),
                record: tree,
            }],
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };
    let receipt = client.structural().publish_wire_batch(batch)?;
    require_durable(&receipt, "Structural")
}

/// History, dirty and structural rows are durable before their receipts and
/// serve again after a process restart from the catalog alone.
#[test]
fn auxiliary_rows_survive_a_restart_from_the_catalog() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = identity()?;
    let generation = ManifestGeneration::new(1);
    let pin = GenerationPin::new(repo.clone(), revision.clone(), generation);
    let path = "src/aux_restart.rs";
    let content = "fn aux_restart_alpha() { aux_restart_needle }";
    let first = start(&binary, root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let first_result = (|| -> Result<Vec<String>, Box<dyn Error>> {
        let client = first.connect()?;
        let spec = history_spec(
            "0123456789abcdef0123456789abcdef01234567",
            path,
            "fix: sample history alpha_content_needle",
            "v1.0.0",
            "history added line",
            "history touched line",
        );
        publish_history(&client, &repo, &revision, generation, &spec)?;
        publish_dirty(&client, &repo, &revision, generation, path, 1, 77)?;

        let source = source_batch(&repo, &revision, 1, 1, path, content)?;
        let outcome = client.search_corpus().publish_outcome(&source)?;
        require_durable(&outcome.receipt, "Source")?;
        if outcome.publication.target.manifest_generation != generation {
            return Err("Source seal selected a different generation".into());
        }
        publish_structural_after_source(&client, &repo, &revision, generation, path, content)?;
        let activation = client
            .search_corpus()
            .activate_published(&PublishedBatchEvidence::from(&outcome), None)?;
        if activation.active.generation.lexical.manifest_generation != generation
            || client
                .generations()
                .active_head(repo.clone(), revision.clone())?
                != Some(activation.active)
        {
            return Err("Source activation selected a different generation".into());
        }

        let before = history_commits(&client, &pin)?;
        if before.len() != 1 {
            return Err(
                format!("history must serve its one commit, served {}", before.len()).into(),
            );
        }
        Ok(before)
    })();
    let stopped = first.stop();
    let before_restart = first_result?;
    stopped?;
    binary.verify()?;

    let counts = rows_by_domain_and_generation(root.path())?;
    for domain in AuxiliaryDomainV1::ALL {
        let rows = counts
            .get(&(domain, generation.get()))
            .copied()
            .unwrap_or(0);
        if rows == 0 {
            return Err(format!(
                "{domain} rows for generation {} must be durable, found none in {counts:?}",
                generation.get()
            )
            .into());
        }
    }

    let second = start(&binary, root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let second_result = (|| -> TestResult {
        let client = second.connect()?;
        let after_restart = history_commits(&client, &pin)?;
        if after_restart != before_restart {
            return Err(format!(
                "history restored from rows must serve the same commits: before={before_restart:?} after={after_restart:?}"
            ).into());
        }
        let structural = request_when_ready(|| {
            client
                .structural()
                .query()
                .native("aux_restart_alpha AND match { function_item }")
                .pinned(pin.clone())
                .top_k(5)
                .execute()
        })?;
        if structural.generation != pin || structural.results.len() != 1 {
            return Err(format!(
                "structural restored from rows must serve its one tree, served {} at {:?}",
                structural.results.len(),
                structural.generation
            )
            .into());
        }
        Ok(())
    })();
    let stopped = second.stop();
    second_result?;
    stopped?;
    binary.verify()?;
    Ok(())
}

/// Generation retention forgets every reaped generation's auxiliary rows
/// and keeps all domains of each retained generation.
#[test]
fn retention_forgets_the_reaped_generations_auxiliary_rows() -> TestResult {
    const KEEP: usize = 2;
    const SEALED: u32 = 4;
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = identity()?;
    let process = start(&binary, root.path(), KEEP)?;
    let run_result = (|| -> Result<Vec<ManifestGeneration>, Box<dyn Error>> {
        let client = process.connect()?;
        let mut active: Option<SearchCorpusActiveHeadV1> = None;
        let mut sealed = Vec::new();
        for index in 0..SEALED {
            let generation_number = u64::from(index) + 1;
            let generation = ManifestGeneration::new(generation_number);
            let path = format!("src/aux_gc_{index}.rs");
            let content = format!("fn aux_gc_{index}() {{ aux_gc_needle_{index} }}");
            // Each old harness loop consumes six request IDs: text chunk,
            // History, Dirty, Source seal, active-head read, activation CAS.
            let chunk_number = 1 + u64::from(index) * 6;
            let commit_sha = format!("{index:0>40}");
            let message = format!("gc generation {index}");
            let tag_name = format!("v{index}");
            let spec = history_spec(&commit_sha, &path, &message, &tag_name, "added", "touched");
            publish_history(&client, &repo, &revision, generation, &spec).map_err(|error| {
                format!("auxiliary retention generation {index}: history ingest: {error}")
            })?;
            publish_dirty(
                &client,
                &repo,
                &revision,
                generation,
                &path,
                chunk_number,
                5,
            )
            .map_err(|error| {
                format!("auxiliary retention generation {index}: dirty ingest: {error}")
            })?;
            let batch = source_batch(
                &repo,
                &revision,
                generation_number,
                chunk_number,
                &path,
                &content,
            )
            .map_err(|error| {
                format!("auxiliary retention generation {index}: text ingest: {error}")
            })?;
            let (head, _) =
                publish_legacy_source_batch(&client, &batch, active).map_err(|error| {
                    format!("auxiliary retention generation {index}: seal/activate: {error}")
                })?;
            if head.generation.lexical.manifest_generation != generation
                || head.generation.semantic.manifest_generation != generation
            {
                return Err(format!(
                    "auxiliary retention generation {index}: active identity differs"
                )
                .into());
            }
            active = Some(head);
            sealed.push(generation);
        }
        Ok(sealed)
    })();
    let stopped = process.stop();
    let sealed = run_result?;
    stopped?;
    binary.verify()?;

    let retained: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .take(KEEP)
        .map(|generation| generation.get())
        .collect();
    let counts = rows_by_domain_and_generation(root.path())?;
    let generations: BTreeSet<u64> = counts
        .keys()
        .map(|(_domain, generation)| *generation)
        .collect();
    if generations != retained {
        return Err(format!(
            "only the retained generations {retained:?} may keep auxiliary rows, the catalog holds {generations:?}"
        ).into());
    }
    for generation in &retained {
        for domain in AuxiliaryDomainV1::ALL {
            if counts.get(&(domain, *generation)).copied().unwrap_or(0) == 0 {
                return Err(format!(
                    "retained generation {generation} must keep its {domain} rows"
                )
                .into());
            }
        }
    }
    Ok(())
}
