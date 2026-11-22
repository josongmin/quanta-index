#![forbid(unsafe_code)]

use std::time::Duration;

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, ChunkRecord, FileContributorIdentityEntry,
    FileOwnerProjectionRow, GenerationPin, LexicalCandidate, LqVisibility, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SourceFileKey,
    SourcePublicationEvent, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_sdk::{
    FileContributorBatch, FileOwnershipBatch, QuantaIndex, RepoCommitRecencyBatch,
    RepoDescriptionBatch, RepoMetaBatch, RepoTopicBatch, SdkError, SearchCorpusBatch,
};
use quanta_index_searchd_harness::{
    SourceCorpusFixture, fixture_source_scope_v1,
    legacy_reference::{LegacySearchdBinary, daemon_socket_paths},
    private_tempdir, semantic_source_scopes_for_chunk_records,
};

use crate::fail_closed_wait::{RealTicker, WaitError, wait_for};

const SOURCE_EVENT: &str = "e2e-source-event-1";

#[derive(Debug)]
struct RouteError {
    code: String,
    message: String,
}

struct RouteResult {
    candidates: Vec<LexicalCandidate>,
    candidate_ids: Vec<String>,
    file_owner_rows: Vec<FileOwnerProjectionRow>,
    typed_error: Option<RouteError>,
}

struct SdkRoute {
    client: QuantaIndex,
    pin: GenerationPin,
}

impl SdkRoute {
    fn query_text(
        &mut self,
        syntax: TextQuerySyntax,
        query: &str,
        top_k: u32,
    ) -> AnyResult<RouteResult> {
        let result = self.query(syntax, query, top_k)?;
        if let Some(error) = result.typed_error.as_ref() {
            anyhow::bail!("query {query:?} returned {}: {}", error.code, error.message);
        }
        Ok(result)
    }

    fn query(&self, syntax: TextQuerySyntax, query: &str, top_k: u32) -> AnyResult<RouteResult> {
        let request = TextQueryRequest {
            syntax,
            query_text: query.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(self.pin.clone()),
            generation_selector: None,
            top_k,
            cursor: None,
        };
        let response = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            &format!("old-Q Text route query {query}"),
            || self.client.lexical().query_request(request.clone()),
            |_| true,
            |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
        );
        match response {
            Ok(response) => {
                ensure!(
                    response.generation == self.pin,
                    "Text route SDK generation changed"
                );
                Ok(RouteResult {
                    candidate_ids: response
                        .results
                        .iter()
                        .map(|row| row.candidate_id.clone())
                        .collect(),
                    candidates: response.results,
                    file_owner_rows: response.file_owner_rows.unwrap_or_default(),
                    typed_error: None,
                })
            }
            Err(WaitError::Terminal(SdkError::Remote { code, message, .. })) => Ok(RouteResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                typed_error: Some(RouteError {
                    code: code.as_wire_str().to_string(),
                    message,
                }),
            }),
            Err(error) => Err(anyhow::anyhow!("Text route SDK request failed: {error}")),
        }
    }
}

fn sorted_candidate_paths(result: &RouteResult) -> Vec<String> {
    let mut paths = result
        .candidates
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn sorted_candidate_ids(result: &RouteResult) -> Vec<String> {
    let mut ids = result.candidate_ids.clone();
    ids.sort();
    ids
}

fn sorted_file_owner_projection(result: &RouteResult) -> Vec<(String, Vec<String>)> {
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

fn pin() -> AnyResult<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn source_batch(pin: &GenerationPin) -> AnyResult<SearchCorpusBatch> {
    // The candidate IDs remain in the original ingest order. Source scopes
    // are then published in canonical source-repository/path order.
    let specs = [
        (
            "corp-a",
            "src/corp-a.rs",
            "shared_oracle_needle corp-a branch\n",
            "rust",
        ),
        (
            "corp-b",
            "src/corp-b.rs",
            "shared_oracle_needle corp-b branch\n",
            "rust",
        ),
        (
            "corp-a",
            "src/gate-a.rs",
            "shared_oracle_needle gate-a only 123\n",
            "rust",
        ),
        (
            "corp-b",
            "lib/gate-a.rs",
            "shared_oracle_needle corp-b gate-a branch\n",
            "rust",
        ),
        (
            "corp-b",
            "src/gate-b.py",
            "shared_oracle_needle corp-b python branch\n",
            "python",
        ),
        (
            "corp-a",
            "docs/colors.md",
            "the lemon yellow banana ripens\n",
            "markdown",
        ),
    ];
    let ranking = [
        ("src/zeta.rs", "repo_tie_needle", "rust", "zeta"),
        ("src/alpha.rs", "repo_tie_needle", "rust", "alpha"),
        ("config/path_only_needle.toml", "value = 1", "toml", "beta"),
        (
            "scripts/helper.py",
            "def alpha_content_needle(): pass  # the config path_only_needle key is read here",
            "python",
            "gamma",
        ),
        (
            "src/bait.rs",
            "let needle_xx = 1; // path_only_needle",
            "rust",
            "eta",
        ),
    ];
    let regular = specs
        .into_iter()
        .enumerate()
        .map(|(offset, (repo, path, body, language))| {
            (
                repo,
                path,
                body,
                language,
                format!("e2e-{}-{path}", offset + 1),
            )
        });
    let ranked = ranking
        .into_iter()
        .map(|(path, body, language, id)| ("ranking", path, body, language, id.to_owned()));
    let mut scopes = Vec::new();
    for (source_repo, path, body, language, id) in regular.chain(ranked) {
        let chunk = ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new(language).map_err(anyhow::Error::msg)?,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 2,
            text: body.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(RepoId::new(source_repo)?),
        };
        scopes.push(fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id: RepoId::new(source_repo)?,
                repo_relative_path: RepoRelativePath::new(path),
            },
            pin.revision_id.clone(),
            vec![chunk],
            Vec::new(),
        )?);
    }
    scopes.sort_unstable_by(|a, b| {
        (
            &a.coverage.source.file.source_repo_id,
            &a.coverage.source.file.repo_relative_path,
        )
            .cmp(&(
                &b.coverage.source.file.source_repo_id,
                &b.coverage.source.file.repo_relative_path,
            ))
    });
    let mut batch = SearchCorpusBatch::replace_generation(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        "lex-seal:1",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".to_string(),
        event_id: SOURCE_EVENT.to_string(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    for scope in scopes {
        for semantic in semantic_source_scopes_for_chunk_records(&pin.repo_id, &scope.chunks) {
            batch = batch.replace_semantic_scope(
                semantic.scope,
                semantic.scope_digest,
                semantic.sources,
                semantic.cluster_memberships,
            );
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

fn reserve_source_generation(client: &QuantaIndex, batch: &SearchCorpusBatch) -> AnyResult<()> {
    // The SDK derives this request from the exact sealed body it will publish.
    let request = batch.generation_reservation_request()?;
    let reservation = client.search_corpus().reserve_generation(&request)?;
    ensure!(
        reservation.repo_id == request.repo_id
            && reservation.revision_id == request.revision_id
            && reservation.source_event == request.source_event
            && reservation.generation == batch.generation(),
        "old-Q Source reservation differs from published identity",
    );
    Ok(())
}

fn check_aux(
    name: &str,
    receipt: BatchPublishReceipt,
    generation: ManifestGeneration,
    scopes: u32,
) -> AnyResult<()> {
    ensure!(
        receipt.generation == generation
            && receipt.applied
            && receipt.durable_sequence > 0
            && receipt.accepted_replace_scopes == scopes
            && receipt.accepted_tombstone_scopes == 0,
        "{name} auxiliary receipt diverged: {receipt:?}",
    );
    Ok(())
}

fn boot_with_text_route_authorities(client: &QuantaIndex) -> AnyResult<GenerationPin> {
    let pin = pin()?;
    let source = source_batch(&pin)?;
    reserve_source_generation(client, &source)?;
    let a = RepoId::new("corp-a")?;
    let b = RepoId::new("corp-b")?;
    let now_ms = now_epoch_ms()?;
    let history = client.history();
    check_aux(
        "RepoMeta",
        history.publish_repo_meta(
            &RepoMetaBatch::new(
                pin.repo_id.clone(),
                pin.revision_id.clone(),
                pin.manifest_generation,
            )
            .entry(a.clone(), "license", "apache-2.0")
            .entry(b.clone(), "license", "gpl-3.0")
            .entry(a.clone(), "tier", "prod"),
        )?,
        pin.manifest_generation,
        3,
    )?;
    check_aux(
        "RepoTopic",
        history.publish_repo_topic(
            &RepoTopicBatch::new(
                pin.repo_id.clone(),
                pin.revision_id.clone(),
                pin.manifest_generation,
            )
            .entry(a.clone(), "security")
            .entry(a.clone(), "platform")
            .entry(b.clone(), "ml"),
        )?,
        pin.manifest_generation,
        3,
    )?;
    check_aux(
        "RepoCommitRecency",
        history.publish_repo_commit_recency(
            &RepoCommitRecencyBatch::new(
                pin.repo_id.clone(),
                pin.revision_id.clone(),
                pin.manifest_generation,
            )
            .entry(a.clone(), now_ms.saturating_sub(6 * 60 * 60 * 1000))
            .entry(b.clone(), 1_700_000_000_000),
        )?,
        pin.manifest_generation,
        2,
    )?;
    check_aux(
        "RepoDescription",
        history.publish_repo_description(
            &RepoDescriptionBatch::new(
                pin.repo_id.clone(),
                pin.revision_id.clone(),
                pin.manifest_generation,
            )
            .entry(a.clone(), "Apache distributed systems platform")
            .entry(b.clone(), "Machine learning training pipelines"),
        )?,
        pin.manifest_generation,
        2,
    )?;
    check_aux(
        "FileOwnership",
        history.publish_file_ownership(
            &FileOwnershipBatch::new(
                pin.repo_id.clone(),
                pin.revision_id.clone(),
                pin.manifest_generation,
            )
            .entry(
                a.clone(),
                RepoRelativePath::new("src/corp-a.rs"),
                vec!["@alice".into(), "@acme/platform".into()],
            )
            .entry(
                a.clone(),
                RepoRelativePath::new("src/gate-a.rs"),
                vec!["@alice".into()],
            )
            .entry(
                b.clone(),
                RepoRelativePath::new("src/corp-b.rs"),
                vec!["@bob".into()],
            )
            .entry(
                b.clone(),
                RepoRelativePath::new("src/gate-b.py"),
                Vec::new(),
            ),
        )?,
        pin.manifest_generation,
        4,
    )?;
    let alice = || FileContributorIdentityEntry {
        canonical: "alice".into(),
        name: Some("Alice Example".into()),
        email: Some("alice@example.com".into()),
    };
    check_aux(
        "FileContributor",
        history.publish_file_contributor(
            &FileContributorBatch::new(
                pin.repo_id.clone(),
                pin.revision_id.clone(),
                pin.manifest_generation,
            )
            .entry_identities(
                a.clone(),
                RepoRelativePath::new("src/corp-a.rs"),
                vec![
                    alice(),
                    FileContributorIdentityEntry {
                        canonical: "carol".into(),
                        name: Some("Carol Example".into()),
                        email: Some("carol@example.com".into()),
                    },
                ],
            )
            .entry_identities(a, RepoRelativePath::new("src/gate-a.rs"), vec![alice()])
            .entry_identities(
                b.clone(),
                RepoRelativePath::new("src/corp-b.rs"),
                vec![FileContributorIdentityEntry {
                    canonical: "bob".into(),
                    name: Some("Bob Builder".into()),
                    email: Some("bob@example.com".into()),
                }],
            )
            .entry_identities(b, RepoRelativePath::new("src/gate-b.py"), Vec::new()),
        )?,
        pin.manifest_generation,
        4,
    )?;
    let (receipt, activation) = client.search_corpus().publish_and_activate(&source, None)?;
    ensure!(
        receipt.sealed
            && receipt.applied
            && receipt.durable_sequence > 0
            && receipt.generation == pin.manifest_generation
            && receipt.manifest_digest.as_deref() == Some(source.manifest_digest())
            && receipt.accepted_replace_scopes == 11
            && receipt.accepted_semantic_replace_scopes == 11
            && activation.active.generation.lexical.manifest_generation == pin.manifest_generation
            && client
                .generations()
                .active_head(pin.repo_id.clone(), pin.revision_id.clone())?
                == Some(activation.active),
        "Source seal and activation did not match reserved generation",
    );
    Ok(pin)
}

fn now_epoch_ms() -> AnyResult<u64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("system time before unix epoch: {err}"))?
        .as_millis();
    u64::try_from(millis).map_err(|err| anyhow::anyhow!("epoch millis overflow u64: {err}"))
}

fn verify_repo_meta_description_and_repo_file(route: &mut SdkRoute) -> AnyResult<()> {
    let regex_pair = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/license/:/apache.*/) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&regex_pair) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex key/value repo:has.meta must gate corp-a, got {:?}",
        sorted_candidate_paths(&regex_pair),
    );

    let exact_key_regex_value = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license:/gpl-.*/) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&exact_key_regex_value)
            == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "exact-key regex-value repo:has.meta must gate corp-b, got {:?}",
        sorted_candidate_paths(&exact_key_regex_value),
    );

    let regex_key_only = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/tier/) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&regex_key_only) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex key-only repo:has.meta must gate corp-a, got {:?}",
        sorted_candidate_paths(&regex_key_only),
    );

    let key_only = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&key_only)
            == [
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py",
            ],
        "repo:has.meta(key) must gate every repo with the key present, got {:?}",
        sorted_candidate_paths(&key_only),
    );

    let tag_existence = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(tier:) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&tag_existence) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.meta(tag:) must gate only repos with the key present, got {:?}",
        sorted_candidate_paths(&tag_existence),
    );

    let invalid_meta_regex = route.query(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/license(/:/apache.*/) shared_oracle_needle",
        10,
    )?;
    let Some(meta_error) = invalid_meta_regex.typed_error else {
        anyhow::bail!("invalid repo:has.meta regex must typed-fail");
    };
    ensure!(
        meta_error.code.as_str() == "LEX_REGEX_PARSE_FAIL",
        "invalid repo:has.meta regex must fail with LEX_REGEX_PARSE_FAIL, got {}",
        meta_error.code,
    );

    let description = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"systems|pipelines\") shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&description)
            == [
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py",
            ],
        "repo:has.description alternation must gate both repos, got {:?}",
        sorted_candidate_paths(&description),
    );

    let invalid_description = route.query(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"a[b\") shared_oracle_needle",
        10,
    )?;
    let Some(description_error) = invalid_description.typed_error else {
        anyhow::bail!("invalid repo:has.description regex must typed-fail");
    };
    ensure!(
        description_error.code.as_str() == "LEX_REGEX_PARSE_FAIL",
        "invalid repo:has.description regex must fail with LEX_REGEX_PARSE_FAIL, got {}",
        description_error.code,
    );

    let correlated = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs, content:123) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&correlated) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.file(path+content) must gate corp-a on same-doc correlation, got {:?}",
        sorted_candidate_paths(&correlated),
    );

    let anti_overmatch = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/corp-a.rs, content:123) shared_oracle_needle",
        10,
    )?;
    ensure!(
        anti_overmatch.candidate_ids.is_empty(),
        "repo:has.file(path+content) must not overmatch across docs, got {:?}",
        sorted_candidate_paths(&anti_overmatch),
    );

    let empty_content = route.query(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs, content:) shared_oracle_needle",
        10,
    )?;
    let Some(repo_file_error) = empty_content.typed_error else {
        anyhow::bail!("empty repo:has.file content matcher must typed-fail");
    };
    ensure!(
        repo_file_error.code.as_str() == "LEX_PREDICATE_UNIMPLEMENTED",
        "empty repo:has.file content matcher must fail closed, got {}",
        repo_file_error.code,
    );
    Ok(())
}

fn verify_repo_content_allow_list_and_projection(route: &mut SdkRoute) -> AnyResult<()> {
    let corp_a_paths = ["src/corp-a.rs", "src/gate-a.rs"];
    let corp_b_paths = ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"];

    for query in [
        "repo:has.content(\"gate-a only\") shared_oracle_needle",
        "repo:has.content('gate-a only') shared_oracle_needle",
        "repo:contains.content(\"gate-a only\") shared_oracle_needle",
        "repo:has.content(123) shared_oracle_needle",
        "shared_oracle_needle AND (repo:has.content(\"gate-a only\") OR missing_hellgate_token)",
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        ensure!(
            sorted_candidate_paths(&result) == corp_a_paths,
            "repo content gate {query:?} must select corp-a's two needle rows, got {:?}",
            sorted_candidate_paths(&result),
        );
    }
    let complement = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "shared_oracle_needle NOT repo:has.content(\"gate-a only\")",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&complement) == corp_b_paths,
        "repo content complement must select corp-b's three needle rows, got {:?}",
        sorted_candidate_paths(&complement),
    );

    for (query, expected) in [
        ("file:contains(123)", &["src/gate-a.rs"][..]),
        ("file:has.content(123)", &["src/gate-a.rs"][..]),
        ("file:has.content(404)", &[][..]),
        ("repo:has.content(404) shared_oracle_needle", &[][..]),
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        ensure!(
            sorted_candidate_paths(&result) == expected,
            "numeric content gate {query:?} returned {:?}",
            sorted_candidate_paths(&result),
        );
    }

    for (query, expected) in [
        ("repo:corp-a shared_oracle_needle", corp_a_paths.as_slice()),
        ("repo:corp-b shared_oracle_needle", corp_b_paths.as_slice()),
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        ensure!(
            sorted_candidate_paths(&result) == expected,
            "repository filter {query:?} escaped its source repository, got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    // Indexed old-Q matches the complete stored path term and rejects anchors.
    for (query, expected) in [
        (
            "repo:corp-a file:src/gate-a[.]rs lang:rust shared_oracle_needle",
            &["e2e-3-src/gate-a.rs"][..],
        ),
        (
            "repo:corp-b file:src/gate-a[.]rs lang:rust shared_oracle_needle",
            &[][..],
        ),
        (
            "repo:corp-a file:src/gate-a[.]rs lang:python shared_oracle_needle",
            &[][..],
        ),
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 1)?;
        ensure!(
            result.candidate_ids == expected,
            "{query:?} returned {:?}",
            result.candidate_ids
        );
    }

    for query in [
        "select:repo shared_oracle_needle",
        "type:repo shared_oracle_needle",
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        let mut repositories = result
            .candidates
            .iter()
            .map(|row| row.source_repo_id.as_str())
            .collect::<Vec<_>>();
        repositories.sort_unstable();
        ensure!(
            repositories == ["corp-a", "corp-b"],
            "repository projection {query:?} must return one representative per source repository, got {repositories:?} from {:?}",
            sorted_candidate_paths(&result),
        );
    }
    Ok(())
}

fn verify_repo_topic_and_commit_recency(route: &mut SdkRoute) -> AnyResult<()> {
    let topic = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.topic(security) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&topic) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.topic(security) must gate corp-a, got {:?}",
        sorted_candidate_paths(&topic),
    );

    let topic_miss = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.topic(nope) shared_oracle_needle",
        10,
    )?;
    ensure!(
        topic_miss.candidate_ids.is_empty(),
        "repo:has.topic(nope) must miss, got {:?}",
        sorted_candidate_paths(&topic_miss),
    );

    let commit_after = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(1 year ago) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&commit_after) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.commit.after(1 year ago) must gate recent corp-a only, got {:?}",
        sorted_candidate_paths(&commit_after),
    );

    let contains_commit_after = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.commit.after(1 year ago) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&contains_commit_after) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:contains.commit.after alias must match repo:has.commit.after, got {:?}",
        sorted_candidate_paths(&contains_commit_after),
    );

    let commit_after_miss = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(2100-01-01) shared_oracle_needle",
        10,
    )?;
    ensure!(
        commit_after_miss.candidate_ids.is_empty(),
        "repo:has.commit.after in the far future must miss, got {:?}",
        sorted_candidate_paths(&commit_after_miss),
    );

    let invalid_timeref = route.query(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(definitely-not-a-timeref) shared_oracle_needle",
        10,
    )?;
    let Some(error) = invalid_timeref.typed_error else {
        anyhow::bail!("invalid repo:has.commit.after timeref must typed-fail");
    };
    ensure!(
        error.code.as_str() == "HISTORY_INVALID_TIMEREF" && error.message.contains("timeref"),
        "invalid repo:has.commit.after must fail with HISTORY_INVALID_TIMEREF + timeref message, got {} {:?}",
        error.code,
        error.message,
    );
    Ok(())
}

fn verify_scoped_file_content_name_and_boolean(route: &mut SdkRoute) -> AnyResult<()> {
    for query in [
        "file:contains(name:colors.md, \"lemon yellow banana\")",
        "file:has.content(name:colors.md, \"lemon yellow banana\")",
        "file:contains(path:docs/colors.md, \"lemon yellow banana\") OR missing_hellgate_token",
        "file:has.content(path:docs/colors.md, \"lemon yellow banana\") OR missing_hellgate_token",
        r"file:contains(path:docs/colors.md, /yellow\s+banana/)",
        r"file:has.content(name:colors.md, /yellow\s+banana/)",
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        ensure!(result.typed_error.is_none(), "{query} must execute");
        ensure!(
            sorted_candidate_paths(&result) == ["docs/colors.md"],
            "{query} must isolate docs/colors.md, got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    for (query, expected) in [
        (
            r#"file:contains.content("lemon yellow banana")"#,
            &["docs/colors.md"][..],
        ),
        (
            "file:contains(path:config/path_only_needle.toml, 1)",
            &["config/path_only_needle.toml"][..],
        ),
        (
            "file:has.content(path:config/path_only_needle.toml, 1)",
            &["config/path_only_needle.toml"][..],
        ),
        (
            r#"file:has.content(lang:markdown, "lemon yellow banana")"#,
            &["docs/colors.md"][..],
        ),
        (
            r#"file:has.content(lang:rust, "lemon yellow banana")"#,
            &[][..],
        ),
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        let actual = sorted_candidate_paths(&result);
        ensure!(actual == expected, "{query:?} returned {actual:?}");
    }
    for query in [
        r#"file:contains("lemon", "banana")"#,
        "repo:has.content(path:src, 7) shared_oracle_needle",
    ] {
        let result = route.query(TextQuerySyntax::Sourcegraph, query, 10)?;
        ensure!(
            result.typed_error.as_ref().map(|error| error.code.as_str())
                == Some("LEX_PREDICATE_UNIMPLEMENTED"),
            "{query:?} must reject its predicate shape, got {:?}",
            result.typed_error
        );
    }

    for query in [
        "ripens NOT file:contains(path:docs/colors.md, \"lemon yellow banana\")",
        "ripens NOT file:has.content(path:docs/colors.md, \"lemon yellow banana\")",
        r"file:contains(path:src/corp-a.rs, /yellow\s+banana/)",
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, query, 10)?;
        ensure!(result.typed_error.is_none(), "{query} must execute");
        ensure!(
            result.candidate_ids.is_empty(),
            "{query} must exclude the scoped file and return empty, got {:?}",
            sorted_candidate_paths(&result),
        );
    }
    Ok(())
}

fn verify_file_owner_contributor_and_projection(route: &mut SdkRoute) -> AnyResult<()> {
    let owner = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@alice) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&owner) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.owner(@alice) must gate corp-a paths, got {:?}",
        sorted_candidate_paths(&owner),
    );

    let any_owner = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner() shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&any_owner) == ["src/corp-a.rs", "src/corp-b.rs", "src/gate-a.rs"],
        "file:has.owner() must gate files with owners, got {:?}",
        sorted_candidate_paths(&any_owner),
    );

    let owner_miss = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@mallory) shared_oracle_needle",
        10,
    )?;
    ensure!(
        owner_miss.candidate_ids.is_empty(),
        "file:has.owner(@mallory) must miss, got {:?}",
        sorted_candidate_paths(&owner_miss),
    );

    let contributor = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(alice) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&contributor) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor exact must gate alice paths, got {:?}",
        sorted_candidate_paths(&contributor),
    );

    let contributor_miss = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(mallory) shared_oracle_needle",
        10,
    )?;
    ensure!(
        contributor_miss.candidate_ids.is_empty(),
        "file:has.contributor(mallory) must miss, got {:?}",
        sorted_candidate_paths(&contributor_miss),
    );

    let name_regex = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(/alice examp.*/) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&name_regex) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor name regex must gate alice paths, got {:?}",
        sorted_candidate_paths(&name_regex),
    );

    let email_regex = route.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/alice@example\.com/) shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&email_regex) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor email regex must gate alice paths, got {:?}",
        sorted_candidate_paths(&email_regex),
    );

    let no_canonical_fallback = route.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/^alice$/) shared_oracle_needle",
        10,
    )?;
    ensure!(
        no_canonical_fallback.candidate_ids.is_empty(),
        "file:has.contributor regex must not fall back to canonical-only identities, got {:?}",
        sorted_candidate_paths(&no_canonical_fallback),
    );

    let invalid_contributor_regex = route.query(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/alice(/) shared_oracle_needle",
        10,
    )?;
    let Some(contributor_error) = invalid_contributor_regex.typed_error else {
        anyhow::bail!("invalid contributor regex must typed-fail");
    };
    ensure!(
        contributor_error.code.as_str() == "LEX_REGEX_PARSE_FAIL",
        "invalid contributor regex must fail with LEX_REGEX_PARSE_FAIL, got {}",
        contributor_error.code,
    );

    let owners = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "select:file.owners shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&owners)
            == [
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py",
            ],
        "select:file.owners must preserve lexical candidate set, got {:?}, typed_error={:?}",
        sorted_candidate_paths(&owners),
        owners.typed_error,
    );
    ensure!(
        sorted_file_owner_projection(&owners)
            == [
                ("lib/gate-a.rs".to_string(), Vec::new()),
                (
                    "src/corp-a.rs".to_string(),
                    vec!["@acme/platform".to_string(), "@alice".to_string()],
                ),
                ("src/corp-b.rs".to_string(), vec!["@bob".to_string()]),
                ("src/gate-a.rs".to_string(), vec!["@alice".to_string()]),
                ("src/gate-b.py".to_string(), Vec::new()),
            ],
        "select:file.owners must project deterministic owner rows, got {:?}",
        sorted_file_owner_projection(&owners),
    );
    Ok(())
}

fn verify_manual_scoped_content_and_language(route: &mut SdkRoute) -> AnyResult<()> {
    let scoped = "file:contains(name:colors.md, lemon)";
    for (query, expected) in [
        (format!("index:no {scoped}"), &["docs/colors.md"][..]),
        (
            format!("index:no {scoped} AND ripens"),
            &["docs/colors.md"][..],
        ),
        (
            format!("index:no ({scoped} OR missing_hellgate_token)"),
            &["docs/colors.md"][..],
        ),
        (format!("index:no ripens AND NOT {scoped}"), &[][..]),
        (
            "index:no file:contains(name:corp-a.rs, lemon)".to_owned(),
            &[][..],
        ),
    ] {
        let result = route.query_text(TextQuerySyntax::Sourcegraph, &query, 10)?;
        let actual = sorted_candidate_paths(&result);
        ensure!(actual == expected, "{query:?} returned {actual:?}");
    }

    let ungrouped = format!("index:no {scoped} OR missing_hellgate_token");
    let result = route.query(TextQuerySyntax::Sourcegraph, &ungrouped, 10)?;
    let Some(error) = result.typed_error else {
        anyhow::bail!("{ungrouped:?} must refuse an index directive scoped to one OR branch");
    };
    ensure!(
        error.code == "BRIDGE_TRANSLATE_FAIL",
        "{ungrouped:?} returned {error:?}"
    );

    let indexed = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "lang:python shared_oracle_needle",
        10,
    )?;
    ensure!(
        sorted_candidate_paths(&indexed) == ["src/gate-b.py"],
        "indexed language control returned {:?}",
        sorted_candidate_paths(&indexed)
    );
    for query in [
        "index:no lang:python shared_oracle_needle",
        "index:no file:contains(lang:python, shared_oracle_needle)",
    ] {
        let result = route.query(TextQuerySyntax::Sourcegraph, query, 10)?;
        let Some(error) = result.typed_error else {
            anyhow::bail!("{query:?} must refuse unavailable language authority");
        };
        ensure!(
            error.code == "NOT_IMPLEMENTED" && error.message.contains("indexed execution"),
            "{query:?} returned {error:?}"
        );
    }
    Ok(())
}

fn verify_ranked_projection_contracts(route: &mut SdkRoute) -> AnyResult<()> {
    let tied = route.query_text(TextQuerySyntax::Sourcegraph, "repo_tie_needle", 10)?;
    ensure!(
        sorted_candidate_ids(&tied) == ["alpha", "zeta"],
        "tie fixture returned {:?}",
        tied.candidate_ids
    );
    let [first, second] = tied.candidates.as_slice() else {
        anyhow::bail!("select:repo control must expose both source documents");
    };
    ensure!(
        first.score.to_bits() == second.score.to_bits(),
        "select:repo control scores differ: {} vs {}",
        first.score,
        second.score
    );
    let projected = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "select:repo repo_tie_needle",
        10,
    )?;
    ensure!(
        projected.candidate_ids == ["alpha"],
        "select:repo must select the canonical representative, got {:?}",
        projected.candidate_ids
    );

    let mixed = route.query_text(TextQuerySyntax::Sourcegraph, "count:2 path_only_needle", 10)?;
    ensure!(
        mixed.candidate_ids == ["beta", "eta"],
        "mixed path/body top-two order drifted: {:?}",
        mixed.candidate_ids
    );
    Ok(())
}

fn verify_legacy_index_and_boost(route: &mut SdkRoute) -> AnyResult<()> {
    let baseline = route.query_text(TextQuerySyntax::Sourcegraph, "shared_oracle_needle", 10)?;
    ensure!(
        baseline.typed_error.is_none(),
        "baseline lexical query must succeed",
    );
    let mut source_rows = baseline
        .candidates
        .iter()
        .map(|row| (row.repo_relative_path.as_str(), row.source_repo_id.as_str()))
        .collect::<Vec<_>>();
    source_rows.sort_unstable();
    ensure!(
        source_rows
            == [
                ("lib/gate-a.rs", "corp-b"),
                ("src/corp-a.rs", "corp-a"),
                ("src/corp-b.rs", "corp-b"),
                ("src/gate-a.rs", "corp-a"),
                ("src/gate-b.py", "corp-b"),
            ],
        "baseline lexical rows lost their authenticated source repository: {source_rows:?}",
    );

    let index_no = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "index:no shared_oracle_needle",
        10,
    )?;
    ensure!(index_no.typed_error.is_none(), "index:no must execute");
    ensure!(
        sorted_candidate_ids(&index_no) == sorted_candidate_ids(&baseline),
        "index:no must preserve candidate universe, got {:?} vs {:?}",
        sorted_candidate_ids(&index_no),
        sorted_candidate_ids(&baseline),
    );

    let boosted = route.query_text(
        TextQuerySyntax::Sourcegraph,
        "boost:5 shared_oracle_needle",
        10,
    )?;
    ensure!(boosted.typed_error.is_none(), "boost: must execute");
    ensure!(
        boosted.candidate_ids == baseline.candidate_ids,
        "boost: must preserve ranked candidate order, got {:?} vs {:?}",
        boosted.candidate_ids,
        baseline.candidate_ids,
    );
    let baseline_score = baseline
        .candidates
        .first()
        .ok_or_else(|| anyhow::anyhow!("baseline query returned no candidates"))?
        .score;
    let boosted_score = boosted
        .candidates
        .first()
        .ok_or_else(|| anyhow::anyhow!("boost query returned no candidates"))?
        .score;
    ensure!(
        boosted_score > baseline_score,
        "boost: must increase lexical score magnitude, got baseline={baseline_score} boosted={boosted_score}",
    );
    Ok(())
}

fn publish_bundle_metadata_fixture(state_root: &std::path::Path) -> AnyResult<GenerationPin> {
    use ciborium::value::Value;

    let pin = GenerationPin::new(
        RepoId::new("repo-e2e-metadata")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    );
    let mut fixture = SourceCorpusFixture::new(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        "e2e-typed-bundle-metadata".to_owned(),
    );
    let body = "metadata_gate_needle\n";
    let language = LanguageCode::new("rust").map_err(anyhow::Error::msg)?;
    let end_byte = u32::try_from(body.len())?;
    fixture.replace_chunks(
        [("meta-a", "src/meta-a.rs"), ("meta-b", "src/meta-b.rs")]
            .into_iter()
            .map(|(id, path)| ChunkRecord {
                chunk_id: ChunkId::new(id),
                repo_relative_path: RepoRelativePath::new(path),
                language: language.clone(),
                start_byte: 0,
                end_byte,
                start_line: 1,
                end_line: 2,
                text: body.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            })
            .collect(),
    )?;
    let mut payload = Vec::new();
    let metadata = Value::Map(vec![
        (Value::Text("fork".into()), Value::Bool(true)),
        (Value::Text("archived".into()), Value::Bool(false)),
        (
            Value::Text("visibility".into()),
            Value::serialized(&LqVisibility::Private)?,
        ),
        (
            Value::Text("contexts".into()),
            Value::serialized(&["team-search", "staging"])?,
        ),
    ]);
    ciborium::into_writer(&metadata, &mut payload)?;
    fixture.set_metadata(payload)?;
    let [_, _, ingest_socket] = daemon_socket_paths(state_root);
    fixture.publish(&ingest_socket, 1)?;
    Ok(pin)
}

fn verify_bundle_metadata_filters(route: &mut SdkRoute) -> AnyResult<()> {
    for (filters, expected) in [
        (
            "fork:only archived:no visibility:private context:team-search",
            &["meta-a", "meta-b"][..],
        ),
        (
            "fork:no archived:no visibility:private context:team-search",
            &[][..],
        ),
        (
            "fork:only archived:only visibility:private context:team-search",
            &[][..],
        ),
        (
            "fork:only archived:no visibility:public context:team-search",
            &[][..],
        ),
        (
            "fork:only archived:no visibility:private context:prod",
            &[][..],
        ),
    ] {
        let query = format!("{filters} metadata_gate_needle");
        let result = route.query_text(TextQuerySyntax::Sourcegraph, &query, 10)?;
        let actual = sorted_candidate_ids(&result);
        ensure!(actual == expected, "{query:?} returned {actual:?}");
    }
    Ok(())
}

#[test]
fn sourcegraph_text_route_authorities_share_one_indexed_fixture() -> AnyResult<()> {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("old-Q Text route binary: {error}"))?;
    let process = binary
        .start(&state_root, 8)
        .map_err(|error| anyhow::anyhow!("old-Q Text route process start: {error}"))?;
    let outcome = (|| -> AnyResult<GenerationPin> {
        let client = process
            .connect()
            .map_err(|error| anyhow::anyhow!("old-Q Text route SDK connect: {error}"))?;
        let pin = boot_with_text_route_authorities(&client)?;
        let mut route = SdkRoute { client, pin };
        let verify_repo_meta_description_and_repo_file_fn: fn(&mut SdkRoute) -> AnyResult<()> =
            verify_repo_meta_description_and_repo_file;
        for (name, verify) in [
            (
                "repo_meta_description_and_repo_file",
                verify_repo_meta_description_and_repo_file_fn,
            ),
            (
                "repo_content_allow_list_and_projection",
                verify_repo_content_allow_list_and_projection,
            ),
            (
                "repo_topic_and_commit_recency",
                verify_repo_topic_and_commit_recency,
            ),
            (
                "scoped_file_content_name_and_boolean",
                verify_scoped_file_content_name_and_boolean,
            ),
            (
                "manual_scoped_content_and_language",
                verify_manual_scoped_content_and_language,
            ),
            (
                "ranked_projection_contracts",
                verify_ranked_projection_contracts,
            ),
            (
                "file_owner_contributor_and_projection",
                verify_file_owner_contributor_and_projection,
            ),
            ("legacy_index_and_boost", verify_legacy_index_and_boost),
        ] {
            verify(&mut route).map_err(|error| anyhow::anyhow!("{name}: {error:#}"))?;
        }
        route.pin = publish_bundle_metadata_fixture(&state_root)?;
        verify_bundle_metadata_filters(&mut route)?;
        Ok(route.pin)
    })();
    let stopped = process.stop();
    let metadata_pin = outcome?;
    stopped.map_err(|error| anyhow::anyhow!("old-Q Text route process stop: {error}"))?;

    let reopened = binary
        .start(&state_root, 8)
        .map_err(|error| anyhow::anyhow!("old-Q Text route reopen: {error}"))?;
    let outcome = (|| -> AnyResult<()> {
        let client = reopened
            .connect()
            .map_err(|error| anyhow::anyhow!("old-Q Text route SDK reconnect: {error}"))?;
        verify_bundle_metadata_filters(&mut SdkRoute {
            client,
            pin: metadata_pin,
        })
    })();
    let stopped = reopened.stop();
    outcome?;
    stopped.map_err(|error| anyhow::anyhow!("old-Q Text route reopened process stop: {error}"))?;
    binary
        .verify()
        .map_err(|error| anyhow::anyhow!("old-Q Text route binary changed: {error}"))
}
