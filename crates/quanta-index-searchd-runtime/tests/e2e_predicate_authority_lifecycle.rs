//! Pinned Legacy SDK lifecycle coverage for source-repo and file authority predicates.
//!
//! This test keeps the authority data multi-repo and verifies that durable
//! predicate shards and file-owner projections survive a daemon reopen.

#![forbid(unsafe_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::{
    CapabilityStatusV1, ChunkId, ChunkRecord, FileContributorIdentityEntry, GenerationPin,
    ManifestGeneration, OwnerDocKind, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceFileKey, SourcePublicationEvent, SourceRoleV1,
    TextQueryRequest, TextQueryResponse, TextQuerySyntax, lex::LanguageCode,
};
use quanta_index_sdk::{
    FileContributorBatch, FileOwnershipBatch, QuantaIndex, RepoCommitRecencyBatch,
    RepoDescriptionBatch, RepoMetaBatch, RepoTopicBatch, SearchCorpusBatch,
};
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, request_when_ready};
use quanta_index_searchd_harness::{fixture_source_scope_v1, private_tempdir};

const NEEDLE: &str = "predicate_authority_lifecycle_needle";
const ALPHA_PATH: &str = "src/alpha.rs";
const BETA_PATH: &str = "src/beta.rs";

#[derive(Debug, Eq, PartialEq)]
struct AuthorityObservation {
    predicate_paths: Vec<Vec<String>>,
    owner_rows: Vec<(String, Vec<String>)>,
}

fn pinned<T>(result: Result<T, Box<dyn std::error::Error>>) -> AnyResult<T> {
    result.map_err(|error| anyhow::anyhow!("{error}"))
}

fn now_epoch_ms() -> AnyResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| anyhow::anyhow!("system time before unix epoch: {error}"))?
        .as_millis();
    u64::try_from(millis).map_err(|error| anyhow::anyhow!("epoch milliseconds overflow: {error}"))
}

fn sorted_paths(result: &TextQueryResponse) -> Vec<String> {
    let mut paths = result
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn sorted_owner_rows(result: &TextQueryResponse) -> Vec<(String, Vec<String>)> {
    let mut rows = result
        .file_owner_rows
        .as_deref()
        .unwrap_or_default()
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

fn query(client: &QuantaIndex, pin: &GenerationPin, text: &str) -> AnyResult<TextQueryResponse> {
    Ok(request_when_ready(|| {
        client.lexical().query_request(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: text.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: 10,
            cursor: None,
        })
    })?)
}

fn query_paths(client: &QuantaIndex, pin: &GenerationPin, text: &str) -> AnyResult<Vec<String>> {
    Ok(sorted_paths(&query(client, pin, text)?))
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

fn semantic_source_scope(
    path: &str,
    generation: u64,
    source_repo_id: &str,
) -> AnyResult<SemanticSourceReplaceScopeV1> {
    let owner_id = format!("predicate-lifecycle:g{generation}:{path}");
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
        scope_digest: format!("predicate-lifecycle:semantic-scope:g{generation}:{path}"),
        sources: vec![SemanticSourceRecordV1 {
            record_id: format!("predicate-lifecycle:semantic-record:g{generation}:{path}"),
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            owner_kind: OwnerDocKind::Symbol,
            owner_id,
            source_doc_id: format!("predicate-lifecycle:semantic-doc:g{generation}:{path}"),
            parent_owner_id: None,
            source_file,
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
    })
}

fn fixture_batch(
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> AnyResult<SearchCorpusBatch> {
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
    for record in [
        replacement_chunk(
            &format!("predicate-lifecycle-g{}-alpha", generation.get()),
            ALPHA_PATH,
            "authority-alpha",
        )?,
        replacement_chunk(
            &format!("predicate-lifecycle-g{}-beta", generation.get()),
            BETA_PATH,
            "authority-beta",
        )?,
    ] {
        let file = SourceFileKey {
            source_repo_id: record
                .source_repo_id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("fixture chunk has no source repository"))?,
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
    for scope in [
        semantic_source_scope(ALPHA_PATH, generation.get(), "authority-alpha")?,
        semantic_source_scope(BETA_PATH, generation.get(), "authority-beta")?,
    ] {
        batch = batch.replace_semantic_scope(
            scope.scope,
            scope.scope_digest,
            scope.sources,
            scope.cluster_memberships,
        );
    }
    Ok(batch)
}

fn observe_generation(
    client: &QuantaIndex,
    pin: &GenerationPin,
    label: &str,
) -> AnyResult<AuthorityObservation> {
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
        let paths = query_paths(client, pin, query)?;
        ensure!(
            paths == expected_alpha,
            "{query:?} must select only {ALPHA_PATH}, got {paths:?}"
        );
        predicate_paths.push(paths);
    }

    let select = query(client, pin, &format!("select:file.owners {NEEDLE}"))?;
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

fn publish_generation(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    label: &str,
    recent_at_ms: u64,
) -> AnyResult<GenerationPin> {
    let alpha = RepoId::new("authority-alpha")?;
    let beta = RepoId::new("authority-beta")?;

    let reservation_request = batch.generation_reservation_request()?;
    let reserved = client
        .search_corpus()
        .reserve_generation(&reservation_request)?;
    ensure!(
        reserved.repo_id == *repo
            && reserved.revision_id == *revision
            && reserved.generation == generation
            && reserved.source_event == reservation_request.source_event,
        "Source reservation differs from fixture: {reserved:?}"
    );
    let recency = client.history().publish_repo_commit_recency(
        &RepoCommitRecencyBatch::new(repo.clone(), revision.clone(), generation)
            .entry(alpha.clone(), recent_at_ms)
            .entry(beta.clone(), 1_600_000_000_000),
    )?;
    let metadata = client.history().publish_repo_meta(
        &RepoMetaBatch::new(repo.clone(), revision.clone(), generation)
            .entry(alpha.clone(), "lifecycle", label)
            .entry(beta.clone(), "lifecycle", "beta"),
    )?;
    let topics = client.history().publish_repo_topic(
        &RepoTopicBatch::new(repo.clone(), revision.clone(), generation)
            .entry(alpha.clone(), format!("lifecycle-{label}"))
            .entry(beta.clone(), "lifecycle-beta"),
    )?;
    let descriptions = client.history().publish_repo_description(
        &RepoDescriptionBatch::new(repo.clone(), revision.clone(), generation)
            .entry(alpha.clone(), format!("{label} authority description"))
            .entry(beta.clone(), "beta authority description"),
    )?;
    let ownership = client.history().publish_file_ownership(
        &FileOwnershipBatch::new(repo.clone(), revision.clone(), generation)
            .entry(
                alpha.clone(),
                RepoRelativePath::new(ALPHA_PATH),
                vec![format!("@{label}-owner")],
            )
            .entry(
                beta.clone(),
                RepoRelativePath::new(BETA_PATH),
                vec!["@beta-owner".to_string()],
            ),
    )?;
    let contributors = client.history().publish_file_contributor(
        &FileContributorBatch::new(repo.clone(), revision.clone(), generation)
            .entry_identities(
                alpha,
                RepoRelativePath::new(ALPHA_PATH),
                vec![FileContributorIdentityEntry {
                    canonical: format!("{label}-contributor"),
                    name: Some(format!("{label} contributor")),
                    email: Some(format!("{label}@example.test")),
                }],
            )
            .entry_identities(
                beta,
                RepoRelativePath::new(BETA_PATH),
                vec![FileContributorIdentityEntry {
                    canonical: "beta-contributor".to_string(),
                    name: Some("Beta Contributor".to_string()),
                    email: Some("beta@example.test".to_string()),
                }],
            ),
    )?;
    for receipt in [
        recency,
        metadata,
        topics,
        descriptions,
        ownership,
        contributors,
    ] {
        ensure!(
            receipt.generation == generation && receipt.applied && receipt.durable_sequence != 0,
            "predicate authority receipt differs from fixture: {receipt:?}"
        );
    }
    let (receipt, activation) = client.search_corpus().publish_and_activate(batch, None)?;
    ensure!(
        receipt.generation == generation
            && receipt.applied
            && receipt.durable_sequence != 0
            && receipt.accepted_replace_scopes == 2
            && receipt.accepted_semantic_replace_scopes == 2
            && activation.active.generation.lexical.manifest_generation == generation,
        "Source receipt or activation differs from fixture: {receipt:?} {activation:?}"
    );
    let observed = client
        .generations()
        .active_head(repo.clone(), revision.clone())?
        .ok_or_else(|| anyhow::anyhow!("Source activation has no observed active head"))?;
    ensure!(
        observed == activation.active,
        "observed Source head differs from activation"
    );
    Ok(GenerationPin::new(
        repo.clone(),
        revision.clone(),
        generation,
    ))
}

#[test]
fn uds_predicate_authority_lifecycle_survives_reopen() -> AnyResult<()> {
    let parent = private_tempdir()?;
    let binary = pinned(LegacySearchdBinary::from_environment())?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let generation = ManifestGeneration::new(1);
    let now_ms = now_epoch_ms()?;
    let batch = fixture_batch(&repo, &revision, generation)?;

    let process = pinned(binary.start(parent.path(), 8))?;
    let before = (|| -> AnyResult<_> {
        let client = pinned(process.connect())?;
        let pin = publish_generation(
            &client,
            &batch,
            &repo,
            &revision,
            generation,
            "g1",
            now_ms.saturating_sub(1_000),
        )?;
        let observed = observe_generation(&client, &pin, "g1")?;
        Ok((pin, observed))
    })();
    let stopped = pinned(process.stop());
    let (pin, g1_before_reopen) = before?;
    stopped?;

    let process = pinned(binary.start(parent.path(), 8))?;
    let after = (|| -> AnyResult<_> {
        let client = pinned(process.connect())?;
        let active = client
            .generations()
            .active_head(repo.clone(), revision.clone())?
            .ok_or_else(|| anyhow::anyhow!("reopen lost the active Source head"))?;
        ensure!(
            active.generation.lexical.manifest_generation == generation,
            "reopen changed the active generation: {active:?}"
        );
        observe_generation(&client, &pin, "g1")
    })();
    let stopped = pinned(process.stop());
    let g1_after_reopen = after?;
    stopped?;
    ensure!(
        g1_after_reopen == g1_before_reopen,
        "reopen changed generation-one predicate or select behavior: before={g1_before_reopen:?} after={g1_after_reopen:?}"
    );
    pinned(binary.verify())?;
    Ok(())
}
