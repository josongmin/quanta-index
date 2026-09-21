//! E2E regression rail for boolean composition across persisted predicate authorities.
//!
//! Every query runs through the UDS frontdoor after the lexical corpus and all
//! predicate authority batches have been sealed and activated. The fixture
//! intentionally gives two source repositories the same path: a boolean
//! implementation must correlate owner/contributor facts by both source repo
//! and path instead of intersecting independent global sets.

#![forbid(unsafe_code)]

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, FileContributorEntry,
    FileContributorIdentityEntry, FileContributorIngestBatch, FileOwnershipEntry,
    FileOwnershipIngestBatch, OwnerDocKind, RepoDescriptionEntry, RepoDescriptionIngestBatch,
    RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath, RepoTopicEntry,
    RepoTopicIngestBatch, SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchScopeKey,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceRoleV1, TextQuerySyntax,
};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

struct FixtureIds {
    alpha_shared: String,
    beta_shared: String,
    gamma_target: String,
}

fn file_scope(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn chunk(id: &str, path: &str, text: &str, source_repo_id: &str) -> AnyResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|error| anyhow::anyhow!("invalid fixture language: {error}"))?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())
            .map_err(|error| anyhow::anyhow!("fixture text length overflow: {error}"))?,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(
            RepoId::new(source_repo_id)
                .map_err(|error| anyhow::anyhow!("invalid fixture repo id: {error}"))?,
        ),
    })
}

fn semantic_scope(path: &str) -> SemanticSourceReplaceScopeV1 {
    let owner_id = format!("predicate-boolean:{path}");
    SemanticSourceReplaceScopeV1 {
        scope: SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id: owner_id.clone(),
        },
        scope_digest: format!("predicate-boolean:semantic-scope:{path}"),
        sources: vec![SemanticSourceRecordV1 {
            record_id: format!("predicate-boolean:semantic-record:{path}"),
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id,
            source_doc_id: format!("predicate-boolean:semantic-doc:{path}"),
            parent_owner_id: None,
            repo_relative_path: RepoRelativePath::new(path),
            language: Some("rust".to_string()),
            package: Some("predicate-boolean".to_string()),
            symbol_kind: Some("function".to_string()),
            visibility: Some("private".to_string()),
            source_role: SourceRoleV1::CardText,
            generated: false,
            capability_status: CapabilityStatusV1::Full,
            raw_fallback_reason: None,
            authority_digest: format!("predicate-boolean:authority:{path}"),
            render_policy_digest: "predicate-boolean:render:v1".to_string(),
            card_schema_version: 1,
            text: format!("semantic authority fixture {path}"),
        }],
        cluster_memberships: Vec::new(),
    }
}

fn sorted_ids(result: &e2e_harness::E2eQueryResult) -> Vec<String> {
    let mut ids = result.candidate_ids.clone();
    ids.sort();
    ids
}

fn require_ids(
    result: &e2e_harness::E2eQueryResult,
    expected: &[&str],
    context: &str,
) -> AnyResult<()> {
    if let Some(error) = &result.typed_error {
        anyhow::bail!(
            "{context}: unexpected typed error code={} message={}",
            error.code,
            error.message
        );
    }
    let mut expected = expected
        .iter()
        .map(|id| (*id).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    ensure!(
        sorted_ids(result) == expected,
        "{context}: candidate ids diverged: actual={:?} expected={expected:?}",
        sorted_ids(result),
    );
    Ok(())
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn boot_fixture() -> AnyResult<(E2eRuntime, FixtureIds)> {
    let mut rt = E2eRuntime::boot()?;
    let alpha_shared = "predicate-boolean-alpha".to_string();
    let beta_shared = "predicate-boolean-beta".to_string();
    let gamma_target = "predicate-boolean-gamma".to_string();
    let generation = rt.current_generation();
    rt.publish_search_corpus_batch(SearchCorpusIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation,
        base_generation: None,
        manifest_digest: "predicate-boolean:corpus".to_string(),
        batch_digest: "predicate-boolean:corpus-batch".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![
            SearchCorpusReplaceScope {
                scope: file_scope("src/shared.rs"),
                scope_digest: "predicate-boolean:shared".to_string(),
                chunks: vec![
                    chunk(
                        &alpha_shared,
                        "src/shared.rs",
                        "authority_boolean_needle alpha shared\n",
                        "corp-alpha",
                    )?,
                    chunk(
                        &beta_shared,
                        "src/shared.rs",
                        "authority_boolean_needle beta shared\n",
                        "corp-beta",
                    )?,
                ],
                symbols: Vec::new(),
            },
            SearchCorpusReplaceScope {
                scope: file_scope("src/target.rs"),
                scope_digest: "predicate-boolean:target".to_string(),
                chunks: vec![chunk(
                    &gamma_target,
                    "src/target.rs",
                    "authority_boolean_needle gamma target\n",
                    "corp-gamma",
                )?],
                symbols: Vec::new(),
            },
        ],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: vec![
            semantic_scope("src/shared.rs"),
            semantic_scope("src/target.rs"),
        ],
        semantic_tombstone_scopes: Vec::new(),
        seal: false,
    })?;

    rt.publish_repo_meta_batch(RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "predicate-boolean-meta".to_string(),
        entries: vec![
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-alpha")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "tier".to_string(),
                value: "prod".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-beta")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "tier".to_string(),
                value: "dev".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-gamma")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "tier".to_string(),
                value: "prod".to_string(),
            },
        ],
    })?;
    rt.publish_repo_topic_batch(RepoTopicIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "predicate-boolean-topics".to_string(),
        entries: vec![
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-alpha")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "platform".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-beta")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "security".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-gamma")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "security".to_string(),
            },
        ],
    })?;
    rt.publish_repo_description_batch(RepoDescriptionIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "predicate-boolean-description".to_string(),
        entries: vec![
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-alpha")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "platform ownership".to_string(),
            },
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-beta")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "security experiments".to_string(),
            },
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-gamma")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "security production service".to_string(),
            },
        ],
    })?;
    rt.publish_file_ownership_batch(FileOwnershipIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "predicate-boolean-owners".to_string(),
        entries: vec![
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-alpha")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/shared.rs"),
                owners: vec!["@alice".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-beta")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/shared.rs"),
                owners: vec!["@bob".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-gamma")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/target.rs"),
                owners: vec!["@alice".to_string()],
            },
        ],
    })?;
    rt.publish_file_contributor_batch(FileContributorIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "predicate-boolean-contributors".to_string(),
        entries: vec![
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-alpha")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/shared.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "carol".to_string(),
                    name: Some("Carol Example".to_string()),
                    email: Some("carol@example.com".to_string()),
                }],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-beta")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/shared.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "alice".to_string(),
                    name: Some("Alice Example".to_string()),
                    email: Some("alice@example.com".to_string()),
                }],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-gamma")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/target.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "alice".to_string(),
                    name: Some("Alice Example".to_string()),
                    email: Some("alice@example.com".to_string()),
                }],
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    Ok((
        rt,
        FixtureIds {
            alpha_shared,
            beta_shared,
            gamma_target,
        },
    ))
}

#[test]
fn repo_authority_boolean_intersection_and_complement_stay_source_repo_correlated() -> AnyResult<()>
{
    let (mut rt, ids) = boot_fixture()?;

    let intersection = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(tier:prod) AND repo:has.topic(security) authority_boolean_needle",
        10,
    );
    require_ids(
        &intersection,
        &[&ids.gamma_target],
        "repo meta/topic intersection",
    )?;

    let description_or_topic = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(platform) OR repo:has.topic(security)",
        10,
    );
    require_ids(
        &description_or_topic,
        &[&ids.alpha_shared, &ids.beta_shared, &ids.gamma_target],
        "repo description/topic union",
    )?;

    let complement = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "authority_boolean_needle NOT repo:has.topic(security)",
        10,
    );
    require_ids(&complement, &[&ids.alpha_shared], "repo topic complement")?;
    Ok(())
}

#[test]
fn file_authority_boolean_intersection_does_not_cross_join_same_path_repositories() -> AnyResult<()>
{
    let (mut rt, ids) = boot_fixture()?;

    let intersection = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@alice) AND file:has.contributor(alice) authority_boolean_needle",
        10,
    );
    require_ids(
        &intersection,
        &[&ids.gamma_target],
        "file owner/contributor intersection",
    )?;

    let selected = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "select:file.owners file:has.owner(@alice) AND file:has.contributor(alice) authority_boolean_needle",
        10,
    );
    require_ids(
        &selected,
        &[&ids.gamma_target],
        "owner projection behind correlated file predicates",
    )?;
    ensure!(
        matches!(
            selected.file_owner_rows.as_slice(),
            [only] if only.candidate_id == ids.gamma_target && only.owners == ["@alice"]
        ),
        "owner projection widened or lost correlated authority: {:?}",
        selected.file_owner_rows
    );
    Ok(())
}
