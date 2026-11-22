//! Boolean predicates must correlate auxiliary authority by source repository
//! and path. The fixture gives two source repositories the same path and
//! publishes through the pinned Legacy SDK before querying its real daemon.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    CapabilityStatusV1, ChunkId, ChunkRecord, FileContributorIdentityEntry, GenerationPin,
    ManifestGeneration, OwnerDocKind, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceFileKey, SourcePublicationEvent, SourceRoleV1,
    TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_sdk::{
    FileContributorBatch, FileOwnershipBatch, QuantaIndex, RepoDescriptionBatch, RepoMetaBatch,
    RepoTopicBatch, SearchCorpusBatch,
};
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, request_when_ready};
use quanta_index_searchd_harness::{fixture_source_scope_v1, private_tempdir};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const QUERY_TOP_K: u32 = 10;

struct FixtureIds {
    alpha_shared: String,
    beta_shared: String,
    gamma_target: String,
}

fn chunk(id: &str, path: &str, text: &str, source_repo_id: &str) -> TestResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(RepoId::new(source_repo_id)?),
    })
}

fn semantic_scope(path: &str, source_repo_id: &str) -> TestResult<SemanticSourceReplaceScopeV1> {
    let owner_id = format!("predicate-boolean:{path}");
    let source_file = SourceFileKey {
        source_repo_id: RepoId::new(source_repo_id)?,
        repo_relative_path: RepoRelativePath::new(path),
    };
    Ok(SemanticSourceReplaceScopeV1 {
        scope: SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id: owner_id.clone(),
            source_file: source_file.clone(),
        },
        scope_digest: format!("predicate-boolean:semantic-scope:{path}"),
        sources: vec![SemanticSourceRecordV1 {
            record_id: format!("predicate-boolean:semantic-record:{path}"),
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id,
            source_doc_id: format!("predicate-boolean:semantic-doc:{path}"),
            parent_owner_id: None,
            source_file,
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
    })
}

fn fixture_batch(
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> TestResult<(SearchCorpusBatch, FixtureIds)> {
    let ids = FixtureIds {
        alpha_shared: "predicate-boolean-alpha".to_string(),
        beta_shared: "predicate-boolean-beta".to_string(),
        gamma_target: "predicate-boolean-gamma".to_string(),
    };
    let chunks = [
        chunk(
            &ids.alpha_shared,
            "src/shared.rs",
            "authority_boolean_needle alpha shared\n",
            "corp-alpha",
        )?,
        chunk(
            &ids.beta_shared,
            "src/shared.rs",
            "authority_boolean_needle beta shared\n",
            "corp-beta",
        )?,
        chunk(
            &ids.gamma_target,
            "src/target.rs",
            "authority_boolean_needle gamma target\n",
            "corp-gamma",
        )?,
    ];
    let mut batch = SearchCorpusBatch::replace_generation(
        repo.clone(),
        revision.clone(),
        generation,
        format!("lex-seal:{}", generation.get()),
    )
    .source_event(SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: "e2e-source-event-1".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    for record in chunks {
        let file = SourceFileKey {
            source_repo_id: record
                .source_repo_id
                .clone()
                .ok_or("fixture chunk has no source repository")?,
            repo_relative_path: record.repo_relative_path.clone(),
        };
        let scope = fixture_source_scope_v1(file, revision.clone(), vec![record], Vec::new())?;
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    // Preserve the original fixture: only alpha and gamma declare semantic
    // source authority. The beta path must not acquire a synthetic scope.
    for scope in [
        semantic_scope("src/shared.rs", "corp-alpha")?,
        semantic_scope("src/target.rs", "corp-gamma")?,
    ] {
        batch = batch.replace_semantic_scope(
            scope.scope,
            scope.scope_digest,
            scope.sources,
            scope.cluster_memberships,
        );
    }
    Ok((batch, ids))
}

fn publish_fixture(client: &QuantaIndex) -> TestResult<(GenerationPin, FixtureIds)> {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let generation = ManifestGeneration::new(1);
    let (batch, ids) = fixture_batch(&repo, &revision, generation)?;
    let reservation_request = batch.generation_reservation_request()?;
    let reserved = client
        .search_corpus()
        .reserve_generation(&reservation_request)?;
    if reserved.repo_id != repo
        || reserved.revision_id != revision
        || reserved.generation != generation
        || reserved.source_event != reservation_request.source_event
    {
        return Err(format!("Source reservation differs from fixture: {reserved:?}").into());
    }

    let meta = RepoMetaBatch::new(repo.clone(), revision.clone(), generation)
        .entry(RepoId::new("corp-alpha")?, "tier", "prod")
        .entry(RepoId::new("corp-beta")?, "tier", "dev")
        .entry(RepoId::new("corp-gamma")?, "tier", "prod");
    let _meta_receipt = client.history().publish_repo_meta(&meta)?;
    let topics = RepoTopicBatch::new(repo.clone(), revision.clone(), generation)
        .entry(RepoId::new("corp-alpha")?, "platform")
        .entry(RepoId::new("corp-beta")?, "security")
        .entry(RepoId::new("corp-gamma")?, "security");
    let _topic_receipt = client.history().publish_repo_topic(&topics)?;
    let descriptions = RepoDescriptionBatch::new(repo.clone(), revision.clone(), generation)
        .entry(RepoId::new("corp-alpha")?, "platform ownership")
        .entry(RepoId::new("corp-beta")?, "security experiments")
        .entry(RepoId::new("corp-gamma")?, "security production service");
    let _description_receipt = client.history().publish_repo_description(&descriptions)?;
    let owners = FileOwnershipBatch::new(repo.clone(), revision.clone(), generation)
        .entry(
            RepoId::new("corp-alpha")?,
            RepoRelativePath::new("src/shared.rs"),
            vec!["@alice".to_string()],
        )
        .entry(
            RepoId::new("corp-beta")?,
            RepoRelativePath::new("src/shared.rs"),
            vec!["@bob".to_string()],
        )
        .entry(
            RepoId::new("corp-gamma")?,
            RepoRelativePath::new("src/target.rs"),
            vec!["@alice".to_string()],
        );
    let _ownership_receipt = client.history().publish_file_ownership(&owners)?;
    let contributors = FileContributorBatch::new(repo.clone(), revision.clone(), generation)
        .entry_identities(
            RepoId::new("corp-alpha")?,
            RepoRelativePath::new("src/shared.rs"),
            vec![FileContributorIdentityEntry {
                canonical: "carol".to_string(),
                name: Some("Carol Example".to_string()),
                email: Some("carol@example.com".to_string()),
            }],
        )
        .entry_identities(
            RepoId::new("corp-beta")?,
            RepoRelativePath::new("src/shared.rs"),
            vec![FileContributorIdentityEntry {
                canonical: "alice".to_string(),
                name: Some("Alice Example".to_string()),
                email: Some("alice@example.com".to_string()),
            }],
        )
        .entry_identities(
            RepoId::new("corp-gamma")?,
            RepoRelativePath::new("src/target.rs"),
            vec![FileContributorIdentityEntry {
                canonical: "alice".to_string(),
                name: Some("Alice Example".to_string()),
                email: Some("alice@example.com".to_string()),
            }],
        );
    let _contributor_receipt = client.history().publish_file_contributor(&contributors)?;

    let (receipt, activation) = client.search_corpus().publish_and_activate(&batch, None)?;
    if receipt.generation != generation
        || !receipt.applied
        || receipt.durable_sequence == 0
        || receipt.accepted_replace_scopes != 3
        || receipt.accepted_semantic_replace_scopes != 2
        || activation.active.generation.lexical.manifest_generation != generation
    {
        return Err(format!(
            "Source receipt or activation differs from fixture: {receipt:?} {activation:?}"
        )
        .into());
    }
    let observed = client
        .generations()
        .active_head(repo.clone(), revision.clone())?
        .ok_or("Source activation has no observed active head")?;
    if observed != activation.active {
        return Err("observed Source head differs from activation".into());
    }
    Ok((GenerationPin::new(repo, revision, generation), ids))
}

fn query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query_text: &str,
) -> TestResult<TextQueryResponse> {
    Ok(request_when_ready(|| {
        client.lexical().query_request(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: query_text.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: QUERY_TOP_K,
            cursor: None,
        })
    })?)
}

fn require_ids(result: &TextQueryResponse, expected: &[&str], context: &str) -> TestResult {
    let mut actual = result
        .results
        .iter()
        .map(|candidate| candidate.candidate_id.as_str())
        .collect::<Vec<_>>();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    if actual != expected {
        return Err(format!(
            "{context}: candidate IDs diverged: actual={actual:?} expected={expected:?}"
        )
        .into());
    }
    Ok(())
}

fn with_fixture(
    test: impl FnOnce(&QuantaIndex, &GenerationPin, &FixtureIds) -> TestResult,
) -> TestResult {
    let parent = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(parent.path(), 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let (pin, ids) = publish_fixture(&client)?;
        test(&client, &pin, &ids)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

#[test]
fn repo_authority_boolean_intersection_and_complement_stay_source_repo_correlated() -> TestResult {
    with_fixture(|client, pin, ids| {
        let intersection = query(
            client,
            pin,
            "repo:has.meta(tier:prod) AND repo:has.topic(security) authority_boolean_needle",
        )?;
        require_ids(
            &intersection,
            &[&ids.gamma_target],
            "repo meta/topic intersection",
        )?;

        let description_or_topic = query(
            client,
            pin,
            "repo:has.description(platform) OR repo:has.topic(security)",
        )?;
        require_ids(
            &description_or_topic,
            &[&ids.alpha_shared, &ids.beta_shared, &ids.gamma_target],
            "repo description/topic union",
        )?;

        let complement = query(
            client,
            pin,
            "authority_boolean_needle NOT repo:has.topic(security)",
        )?;
        require_ids(&complement, &[&ids.alpha_shared], "repo topic complement")
    })
}

#[test]
fn file_authority_boolean_intersection_does_not_cross_join_same_path_repositories() -> TestResult {
    with_fixture(|client, pin, ids| {
        let intersection = query(
            client,
            pin,
            "file:has.owner(@alice) AND file:has.contributor(alice) authority_boolean_needle",
        )?;
        require_ids(
            &intersection,
            &[&ids.gamma_target],
            "file owner/contributor intersection",
        )?;

        let selected = query(
            client,
            pin,
            "select:file.owners file:has.owner(@alice) AND file:has.contributor(alice) authority_boolean_needle",
        )?;
        require_ids(
            &selected,
            &[&ids.gamma_target],
            "owner projection behind correlated file predicates",
        )?;
        match selected.file_owner_rows.as_deref() {
            Some([only]) if only.candidate_id == ids.gamma_target && only.owners == ["@alice"] => {
                Ok(())
            }
            other => Err(format!(
                "owner projection widened or lost correlated authority: {other:?}"
            )
            .into()),
        }
    })
}
