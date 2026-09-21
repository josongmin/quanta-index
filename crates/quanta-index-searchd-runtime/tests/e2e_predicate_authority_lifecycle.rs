//! UDS lifecycle coverage for source-repo and file authority predicates.
//!
//! This test keeps the authority data multi-repo and verifies that durable
//! predicate shards and file-owner projections survive a daemon reopen.

#![forbid(unsafe_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, FileContributorEntry,
    FileContributorIdentityEntry, FileContributorIngestBatch, FileOwnershipEntry,
    FileOwnershipIngestBatch, OwnerDocKind, RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch,
    RepoDescriptionEntry, RepoDescriptionIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch,
    RepoRelativePath, RepoTopicEntry, RepoTopicIngestBatch, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticSourceRecordV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceRoleV1,
    TextQuerySyntax, lex::LanguageCode,
};
use quanta_index_searchd_harness::{E2eQueryResult, E2eRuntime};

const NEEDLE: &str = "predicate_authority_lifecycle_needle";
const ALPHA_PATH: &str = "src/alpha.rs";
const BETA_PATH: &str = "src/beta.rs";

#[derive(Debug, Eq, PartialEq)]
struct AuthorityObservation {
    predicate_paths: Vec<Vec<String>>,
    owner_rows: Vec<(String, Vec<String>)>,
}

fn now_epoch_ms() -> AnyResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| anyhow::anyhow!("system time before unix epoch: {error}"))?
        .as_millis();
    u64::try_from(millis).map_err(|error| anyhow::anyhow!("epoch milliseconds overflow: {error}"))
}

fn sorted_paths(result: &E2eQueryResult) -> Vec<String> {
    let mut paths = result
        .candidates
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn sorted_owner_rows(result: &E2eQueryResult) -> Vec<(String, Vec<String>)> {
    let mut rows = result
        .file_owner_rows
        .iter()
        .map(|row| {
            let mut owners = row.owners.clone();
            owners.sort();
            (row.repo_relative_path.as_str().to_string(), owners)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

fn query_paths(rt: &mut E2eRuntime, query: &str) -> AnyResult<Vec<String>> {
    let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
    if let Some(error) = result.typed_error {
        anyhow::bail!(
            "query {query:?} returned typed error {}: {}",
            error.code,
            error.message
        );
    }
    Ok(sorted_paths(&result))
}

fn file_scope(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn replacement_chunk(id: &str, path: &str, source_repo_id: &str) -> AnyResult<ChunkRecord> {
    let text = format!("{NEEDLE} replacement {path}");
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|error| anyhow::anyhow!("invalid replacement language: {error}"))?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())
            .map_err(|error| anyhow::anyhow!("replacement text length overflow: {error}"))?,
        start_line: 1,
        end_line: 1,
        text: text.into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(
            RepoId::new(source_repo_id)
                .map_err(|error| anyhow::anyhow!("invalid fixture repo id: {error}"))?,
        ),
    })
}

fn semantic_source_scope(path: &str, generation: u64) -> SemanticSourceReplaceScopeV1 {
    let owner_id = format!("predicate-lifecycle:g{generation}:{path}");
    SemanticSourceReplaceScopeV1 {
        scope: SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id: owner_id.clone(),
        },
        scope_digest: format!("predicate-lifecycle:semantic-scope:g{generation}:{path}"),
        sources: vec![SemanticSourceRecordV1 {
            record_id: format!("predicate-lifecycle:semantic-record:g{generation}:{path}"),
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id,
            source_doc_id: format!("predicate-lifecycle:semantic-doc:g{generation}:{path}"),
            parent_owner_id: None,
            repo_relative_path: RepoRelativePath::new(path),
            language: Some("rust".to_string()),
            package: Some("predicate-lifecycle".to_string()),
            symbol_kind: Some("function".to_string()),
            visibility: Some("private".to_string()),
            source_role: SourceRoleV1::CardText,
            generated: false,
            capability_status: CapabilityStatusV1::Full,
            raw_fallback_reason: None,
            authority_digest: format!("predicate-lifecycle:authority:g{generation}:{path}"),
            render_policy_digest: "predicate-lifecycle:render:v1".to_string(),
            card_schema_version: 1,
            text: format!("{NEEDLE} semantic source {path}"),
        }],
        cluster_memberships: Vec::new(),
    }
}

fn publish_initial_text_generation(rt: &mut E2eRuntime) -> AnyResult<()> {
    let generation = rt.current_generation();
    rt.publish_search_corpus_batch(SearchCorpusIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("predicate-lifecycle:corpus:g{}", generation.get()),
        batch_digest: format!("predicate-lifecycle:corpus-batch:g{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![
            SearchCorpusReplaceScope {
                scope: file_scope(ALPHA_PATH),
                scope_digest: format!("predicate-lifecycle:corpus:g{}:alpha", generation.get()),
                chunks: vec![replacement_chunk(
                    &format!("predicate-lifecycle-g{}-alpha", generation.get()),
                    ALPHA_PATH,
                    "authority-alpha",
                )?],
                symbols: Vec::new(),
            },
            SearchCorpusReplaceScope {
                scope: file_scope(BETA_PATH),
                scope_digest: format!("predicate-lifecycle:corpus:g{}:beta", generation.get()),
                chunks: vec![replacement_chunk(
                    &format!("predicate-lifecycle-g{}-beta", generation.get()),
                    BETA_PATH,
                    "authority-beta",
                )?],
                symbols: Vec::new(),
            },
        ],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: vec![
            semantic_source_scope(ALPHA_PATH, generation.get()),
            semantic_source_scope(BETA_PATH, generation.get()),
        ],
        semantic_tombstone_scopes: Vec::new(),
        seal: false,
    })
}

fn observe_generation(rt: &mut E2eRuntime, label: &str) -> AnyResult<AuthorityObservation> {
    let expected_alpha = vec![ALPHA_PATH.to_string()];
    let predicates = [
        format!("repo:has.meta(lifecycle:{label}) {NEEDLE}"),
        format!("repo:has.topic(lifecycle-{label}) {NEEDLE}"),
        format!("repo:has.description(\"{label} authority\") {NEEDLE}"),
        format!("repo:has.commit.after(1 year ago) {NEEDLE}"),
        format!("file:has.owner(@{label}-owner) {NEEDLE}"),
        format!("file:has.contributor({label}-contributor) {NEEDLE}"),
    ];
    let mut predicate_paths = Vec::with_capacity(predicates.len());
    for query in &predicates {
        let paths = query_paths(rt, query)?;
        ensure!(
            paths == expected_alpha,
            "{query:?} must select only {ALPHA_PATH}, got {paths:?}"
        );
        predicate_paths.push(paths);
    }

    let select = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        &format!("select:file.owners {NEEDLE}"),
        10,
    );
    if let Some(error) = select.typed_error {
        anyhow::bail!(
            "select:file.owners returned typed error {}: {}",
            error.code,
            error.message
        );
    }
    let select_paths = sorted_paths(&select);
    ensure!(
        select_paths == [ALPHA_PATH.to_string(), BETA_PATH.to_string()],
        "select:file.owners must preserve both lexical candidates, got {select_paths:?}"
    );
    let owner_rows = sorted_owner_rows(&select);
    ensure!(
        owner_rows
            == [
                (ALPHA_PATH.to_string(), vec![format!("@{label}-owner")]),
                (BETA_PATH.to_string(), vec!["@beta-owner".to_string()]),
            ],
        "select:file.owners projection differs from the active authority: {owner_rows:?}"
    );

    Ok(AuthorityObservation {
        predicate_paths,
        owner_rows,
    })
}

fn publish_generation(rt: &mut E2eRuntime, label: &str, recent_at_ms: u64) -> AnyResult<()> {
    let source_repo_alpha = RepoId::new("authority-alpha")
        .map_err(|error| anyhow::anyhow!("static fixture repo id: {error}"))?;
    let source_repo_beta = RepoId::new("authority-beta")
        .map_err(|error| anyhow::anyhow!("static fixture repo id: {error}"))?;

    rt.publish_repo_commit_recency_batch(RepoCommitRecencyIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: format!("predicate-lifecycle:commit-recency:{label}"),
        entries: vec![
            RepoCommitRecencyEntry {
                source_repo_id: source_repo_alpha.clone(),
                latest_committer_time_ms: recent_at_ms,
            },
            RepoCommitRecencyEntry {
                source_repo_id: source_repo_beta.clone(),
                latest_committer_time_ms: 1_600_000_000_000,
            },
        ],
    })?;
    rt.publish_repo_meta_batch(RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: format!("predicate-lifecycle:meta:{label}"),
        entries: vec![
            RepoMetaEntry {
                source_repo_id: source_repo_alpha.clone(),
                key: "lifecycle".to_string(),
                value: label.to_string(),
            },
            RepoMetaEntry {
                source_repo_id: source_repo_beta.clone(),
                key: "lifecycle".to_string(),
                value: "beta".to_string(),
            },
        ],
    })?;
    rt.publish_repo_topic_batch(RepoTopicIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: format!("predicate-lifecycle:topic:{label}"),
        entries: vec![
            RepoTopicEntry {
                source_repo_id: source_repo_alpha.clone(),
                topic: format!("lifecycle-{label}"),
            },
            RepoTopicEntry {
                source_repo_id: source_repo_beta.clone(),
                topic: "lifecycle-beta".to_string(),
            },
        ],
    })?;
    rt.publish_repo_description_batch(RepoDescriptionIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: format!("predicate-lifecycle:description:{label}"),
        entries: vec![
            RepoDescriptionEntry {
                source_repo_id: source_repo_alpha.clone(),
                description: format!("{label} authority description"),
            },
            RepoDescriptionEntry {
                source_repo_id: source_repo_beta.clone(),
                description: "beta authority description".to_string(),
            },
        ],
    })?;
    rt.publish_file_ownership_batch(FileOwnershipIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: format!("predicate-lifecycle:ownership:{label}"),
        entries: vec![
            FileOwnershipEntry {
                source_repo_id: source_repo_alpha.clone(),
                repo_relative_path: RepoRelativePath::new(ALPHA_PATH),
                owners: vec![format!("@{label}-owner")],
            },
            FileOwnershipEntry {
                source_repo_id: source_repo_beta.clone(),
                repo_relative_path: RepoRelativePath::new(BETA_PATH),
                owners: vec!["@beta-owner".to_string()],
            },
        ],
    })?;
    rt.publish_file_contributor_batch(FileContributorIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: format!("predicate-lifecycle:contributor:{label}"),
        entries: vec![
            FileContributorEntry {
                source_repo_id: source_repo_alpha,
                repo_relative_path: RepoRelativePath::new(ALPHA_PATH),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: format!("{label}-contributor"),
                    name: Some(format!("{label} contributor")),
                    email: Some(format!("{label}@example.test")),
                }],
            },
            FileContributorEntry {
                source_repo_id: source_repo_beta,
                repo_relative_path: RepoRelativePath::new(BETA_PATH),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "beta-contributor".to_string(),
                    name: Some("Beta Contributor".to_string()),
                    email: Some("beta@example.test".to_string()),
                }],
            },
        ],
    })?;
    Ok(())
}

#[test]
fn uds_predicate_authority_lifecycle_survives_reopen() -> AnyResult<()> {
    let mut runtime = E2eRuntime::boot()?;
    let now_ms = now_epoch_ms()?;

    publish_initial_text_generation(&mut runtime)?;
    publish_generation(&mut runtime, "g1", now_ms.saturating_sub(1_000))?;
    let sealed_g1 = runtime.seal()?;
    ensure!(
        sealed_g1.get() == 1,
        "expected initial generation 1, got {sealed_g1:?}"
    );
    runtime.activate_last_sealed_generation()?;
    let g1_before_reopen = observe_generation(&mut runtime, "g1")?;

    let mut runtime = runtime.reopen();
    let g1_after_reopen = observe_generation(&mut runtime, "g1")?;
    ensure!(
        g1_after_reopen == g1_before_reopen,
        "reopen changed generation-one predicate or select behavior: before={g1_before_reopen:?} after={g1_after_reopen:?}"
    );

    Ok(())
}
