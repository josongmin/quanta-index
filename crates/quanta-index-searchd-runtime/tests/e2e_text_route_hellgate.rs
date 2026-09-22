#![forbid(unsafe_code)]

use crate::e2e_harness;
use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, FileContributorEntry, FileContributorIdentityEntry,
    FileContributorIngestBatch, FileOwnershipEntry, FileOwnershipIngestBatch, GenerationPin,
    HistoryIngestBatch, HistoryRefMutation, HistoryRefUpsert, ManifestGeneration,
    RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoDescriptionEntry,
    RepoDescriptionIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicEntry, RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface, TextQuerySyntax,
    lex::{CommitSha, LanguageCode},
};

use crate::e2e_harness::{E2eRuntime, E2eTextChunkSpec};

const REPO: &str = "repo-text-hellgate";

fn sorted_candidate_paths(result: &e2e_harness::E2eQueryResult) -> Vec<String> {
    let mut paths = result
        .candidates
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn sorted_candidate_ids(result: &e2e_harness::E2eQueryResult) -> Vec<String> {
    let mut ids = result.candidate_ids.clone();
    ids.sort();
    ids
}

fn sorted_file_owner_projection(
    result: &e2e_harness::E2eQueryResult,
) -> Vec<(String, Vec<String>)> {
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

fn seed_multi_repo_chunks(rt: &mut E2eRuntime) -> AnyResult<()> {
    let _corp_a_ids = rt.ingest_text_chunks(
        REPO,
        "src/corp-a.rs",
        &[E2eTextChunkSpec {
            content: "shared_oracle_needle corp-a branch\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-a"),
        }],
    )?;
    let _corp_b_ids = rt.ingest_text_chunks(
        REPO,
        "src/corp-b.rs",
        &[E2eTextChunkSpec {
            content: "shared_oracle_needle corp-b branch\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-b"),
        }],
    )?;
    let _gate_a_ids = rt.ingest_text_chunks(
        REPO,
        "src/gate-a.rs",
        &[E2eTextChunkSpec {
            content: "shared_oracle_needle gate-a only 123\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-a"),
        }],
    )?;
    let _corp_b_name_ids = rt.ingest_text_chunks(
        REPO,
        "lib/gate-a.rs",
        &[E2eTextChunkSpec {
            content: "shared_oracle_needle corp-b gate-a branch\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-b"),
        }],
    )?;
    let _corp_b_path_lang_ids = rt.ingest_text_chunks(
        REPO,
        "src/gate-b.py",
        &[E2eTextChunkSpec {
            content: "shared_oracle_needle corp-b python branch\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-b"),
        }],
    )?;
    let _scoped_content_ids = rt.ingest_text_chunks(
        REPO,
        "docs/colors.md",
        &[E2eTextChunkSpec {
            content: "the lemon yellow banana ripens\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-a"),
        }],
    )?;
    Ok(())
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn boot_with_text_route_authorities() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    let now_ms = now_epoch_ms()?;
    rt.publish_repo_meta_batch(RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "text-hellgate-repo-meta".to_string(),
        entries: vec![
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "license".to_string(),
                value: "apache-2.0".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "license".to_string(),
                value: "gpl-3.0".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-a")
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
        batch_digest: "text-hellgate-repo-topic".to_string(),
        entries: vec![
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "security".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "platform".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                topic: "ml".to_string(),
            },
        ],
    })?;
    rt.publish_repo_commit_recency_batch(RepoCommitRecencyIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "text-hellgate-repo-commit-recency".to_string(),
        entries: vec![
            RepoCommitRecencyEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                latest_committer_time_ms: now_ms.saturating_sub(6 * 60 * 60 * 1000),
            },
            RepoCommitRecencyEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                latest_committer_time_ms: 1_700_000_000_000,
            },
        ],
    })?;
    rt.publish_repo_description_batch(RepoDescriptionIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "text-hellgate-repo-description".to_string(),
        entries: vec![
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "Apache distributed systems platform".to_string(),
            },
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                description: "Machine learning training pipelines".to_string(),
            },
        ],
    })?;
    rt.publish_file_ownership_batch(FileOwnershipIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "text-hellgate-file-owners".to_string(),
        entries: vec![
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/corp-a.rs"),
                owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/gate-a.rs"),
                owners: vec!["@alice".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/corp-b.rs"),
                owners: vec!["@bob".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/gate-b.py"),
                owners: Vec::new(),
            },
        ],
    })?;
    rt.publish_file_contributor_batch(FileContributorIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "text-hellgate-file-contributors".to_string(),
        entries: vec![
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/corp-a.rs"),
                contributors: vec![
                    FileContributorIdentityEntry {
                        canonical: "alice".to_string(),
                        name: Some("Alice Example".to_string()),
                        email: Some("alice@example.com".to_string()),
                    },
                    FileContributorIdentityEntry {
                        canonical: "carol".to_string(),
                        name: Some("Carol Example".to_string()),
                        email: Some("carol@example.com".to_string()),
                    },
                ],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-a")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/gate-a.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "alice".to_string(),
                    name: Some("Alice Example".to_string()),
                    email: Some("alice@example.com".to_string()),
                }],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/corp-b.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "bob".to_string(),
                    name: Some("Bob Builder".to_string()),
                    email: Some("bob@example.com".to_string()),
                }],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-b")
                    .expect("static fixture ID satisfies canonical policy"),
                repo_relative_path: RepoRelativePath::new("src/gate-b.py"),
                contributors: Vec::new(),
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn boot_with_runtime_dirty_fixture() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text(REPO, "src/dirty.rs", "todo dirty scope")?;
    rt.ingest_text(REPO, "src/clean.rs", "todo clean scope")?;
    rt.ingest_dirty_for_path("src/dirty.rs", 100)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn rev_at_time_ancestor_revision() -> RevisionId {
    RevisionId::new("1111111111111111111111111111111111111111")
        .expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn rev_at_time_head_revision() -> RevisionId {
    RevisionId::new("2222222222222222222222222222222222222222")
        .expect("static fixture ID satisfies canonical policy")
}

fn revision_scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn now_epoch_ms() -> AnyResult<u64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("system time before unix epoch: {err}"))?
        .as_millis();
    u64::try_from(millis).map_err(|err| anyhow::anyhow!("epoch millis overflow u64: {err}"))
}

fn publish_revision_text_generation(
    rt: &mut E2eRuntime,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    path: &str,
    candidate_id: &str,
    content: &str,
) -> AnyResult<GenerationPin> {
    let manifest_digest = format!("hellgate-lex:{path}:{}", generation.get());
    rt.publish_search_corpus_batch(SearchCorpusIngestBatch {
        repo_id: rt.repo(),
        revision_id: revision_id.clone(),
        generation,
        base_generation: None,
        manifest_digest,
        batch_digest: format!("hellgate-lex-batch:{path}:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            scope: revision_scope_key(path),
            scope_digest: format!("hellgate-scope:{path}:1-chunk"),
            chunks: vec![ChunkRecord {
                chunk_id: ChunkId::new(candidate_id),
                repo_relative_path: RepoRelativePath::new(path),
                language: LanguageCode::new("rust")
                    .map_err(|err| anyhow::anyhow!("invalid rev_at_time language code: {err}"))?,
                start_byte: 0,
                end_byte: u32::try_from(content.len())
                    .map_err(|err| anyhow::anyhow!("rev_at_time content overflow: {err}"))?,
                start_line: 1,
                end_line: 2,
                text: content.to_string().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
            symbols: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })?;
    let pin = GenerationPin::new(rt.repo(), revision_id, generation);
    rt.activate_last_sealed_generation()?;
    Ok(pin)
}

fn boot_with_rev_at_time_generations() -> AnyResult<(E2eRuntime, GenerationPin)> {
    let mut rt = E2eRuntime::boot()?;
    let now_ms = now_epoch_ms()?;
    let _ancestor_pin = publish_revision_text_generation(
        &mut rt,
        rev_at_time_ancestor_revision(),
        ManifestGeneration::new(7),
        "src/legacy.rs",
        "hellgate-rev-at-time-ancestor",
        "needle_token legacy_choice\n",
    )?;
    let head_pin = publish_revision_text_generation(
        &mut rt,
        rev_at_time_head_revision(),
        ManifestGeneration::new(9),
        "src/head.rs",
        "hellgate-rev-at-time-head",
        "needle_token head_choice\n",
    )?;
    rt.publish_history_batch(HistoryIngestBatch {
        repo_id: rt.repo(),
        revision_id: rev_at_time_head_revision(),
        generation: ManifestGeneration::new(9),
        manifest_digest: Some("hellgate-rev-at-time-history".to_string()),
        batch_digest: "hellgate-rev-at-time-history-batch".to_string(),
        commits: vec![
            quanta_index_contract::lex::CommitRecord {
                wire_version: 1,
                sha: CommitSha::from_hex("1111111111111111111111111111111111111111")
                    .map_err(|err| anyhow::anyhow!("bad ancestor sha: {err}"))?,
                parents: Vec::new(),
                author_time_ms: now_ms.saturating_sub(63_072_000_000),
                committer_time_ms: now_ms.saturating_sub(63_072_000_000),
                applied_at_ms: now_ms.saturating_sub(63_072_000_000),
                author: "alice".to_string().into_boxed_str(),
                author_name: None,
                author_email: None,
                committer: "alice".to_string().into_boxed_str(),
                committer_name: None,
                committer_email: None,
                message: "legacy commit".to_string().into_boxed_str(),
                is_merge: false,
                tags: Vec::new(),
            },
            quanta_index_contract::lex::CommitRecord {
                wire_version: 1,
                sha: CommitSha::from_hex("2222222222222222222222222222222222222222")
                    .map_err(|err| anyhow::anyhow!("bad head sha: {err}"))?,
                parents: vec![
                    CommitSha::from_hex("1111111111111111111111111111111111111111")
                        .map_err(|err| anyhow::anyhow!("bad ancestor parent sha: {err}"))?,
                ],
                author_time_ms: now_ms.saturating_sub(12 * 60 * 60 * 1000),
                committer_time_ms: now_ms.saturating_sub(12 * 60 * 60 * 1000),
                applied_at_ms: now_ms.saturating_sub(12 * 60 * 60 * 1000),
                author: "alice".to_string().into_boxed_str(),
                author_name: None,
                author_email: None,
                committer: "alice".to_string().into_boxed_str(),
                committer_name: None,
                committer_email: None,
                message: "head commit".to_string().into_boxed_str(),
                is_merge: false,
                tags: Vec::new(),
            },
        ],
        refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: "HEAD".to_string().into_boxed_str(),
            sha: CommitSha::from_hex("2222222222222222222222222222222222222222")
                .map_err(|err| anyhow::anyhow!("bad head ref sha: {err}"))?,
        })],
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    })?;
    Ok((rt.reopen(), head_pin))
}

fn verify_repo_meta_description_and_repo_file(rt: &mut E2eRuntime) -> AnyResult<()> {
    let regex_pair = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/license/:/apache.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&regex_pair) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex key/value repo:has.meta must gate corp-a, got {:?}",
        sorted_candidate_paths(&regex_pair),
    );

    let exact_key_regex_value = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license:/gpl-.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&exact_key_regex_value)
            == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "exact-key regex-value repo:has.meta must gate corp-b, got {:?}",
        sorted_candidate_paths(&exact_key_regex_value),
    );

    let regex_key_only = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/tier/) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&regex_key_only) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex key-only repo:has.meta must gate corp-a, got {:?}",
        sorted_candidate_paths(&regex_key_only),
    );

    let key_only = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license) shared_oracle_needle",
        10,
    );
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

    let tag_existence = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(tier:) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&tag_existence) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.meta(tag:) must gate only repos with the key present, got {:?}",
        sorted_candidate_paths(&tag_existence),
    );

    let invalid_meta_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/license(/:/apache.*/) shared_oracle_needle",
        10,
    );
    let Some(meta_error) = invalid_meta_regex.typed_error else {
        anyhow::bail!("invalid repo:has.meta regex must typed-fail");
    };
    ensure!(
        meta_error.code.as_str() == "LEX_REGEX_PARSE_FAIL",
        "invalid repo:has.meta regex must fail with LEX_REGEX_PARSE_FAIL, got {}",
        meta_error.code,
    );

    let description = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"systems|pipelines\") shared_oracle_needle",
        10,
    );
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

    let invalid_description = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"a[b\") shared_oracle_needle",
        10,
    );
    let Some(description_error) = invalid_description.typed_error else {
        anyhow::bail!("invalid repo:has.description regex must typed-fail");
    };
    ensure!(
        description_error.code.as_str() == "LEX_REGEX_PARSE_FAIL",
        "invalid repo:has.description regex must fail with LEX_REGEX_PARSE_FAIL, got {}",
        description_error.code,
    );

    let correlated = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs, content:123) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&correlated) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.file(path+content) must gate corp-a on same-doc correlation, got {:?}",
        sorted_candidate_paths(&correlated),
    );

    let anti_overmatch = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/corp-a.rs, content:123) shared_oracle_needle",
        10,
    );
    ensure!(
        anti_overmatch.candidate_ids.is_empty(),
        "repo:has.file(path+content) must not overmatch across docs, got {:?}",
        sorted_candidate_paths(&anti_overmatch),
    );

    let empty_content = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs, content:) shared_oracle_needle",
        10,
    );
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

fn verify_repo_topic_and_commit_recency(rt: &mut E2eRuntime) -> AnyResult<()> {
    let topic = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.topic(security) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&topic) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.topic(security) must gate corp-a, got {:?}",
        sorted_candidate_paths(&topic),
    );

    let topic_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.topic(nope) shared_oracle_needle",
        10,
    );
    ensure!(
        topic_miss.candidate_ids.is_empty(),
        "repo:has.topic(nope) must miss, got {:?}",
        sorted_candidate_paths(&topic_miss),
    );

    let commit_after = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(1 year ago) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&commit_after) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.commit.after(1 year ago) must gate recent corp-a only, got {:?}",
        sorted_candidate_paths(&commit_after),
    );

    let contains_commit_after = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.commit.after(1 year ago) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&contains_commit_after) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:contains.commit.after alias must match repo:has.commit.after, got {:?}",
        sorted_candidate_paths(&contains_commit_after),
    );

    let commit_after_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(2100-01-01) shared_oracle_needle",
        10,
    );
    ensure!(
        commit_after_miss.candidate_ids.is_empty(),
        "repo:has.commit.after in the far future must miss, got {:?}",
        sorted_candidate_paths(&commit_after_miss),
    );

    let invalid_timeref = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(definitely-not-a-timeref) shared_oracle_needle",
        10,
    );
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

fn verify_scoped_file_content_name_and_boolean(rt: &mut E2eRuntime) -> AnyResult<()> {
    for query in [
        "file:contains(name:colors.md, \"lemon yellow banana\")",
        "file:has.content(name:colors.md, \"lemon yellow banana\")",
        "file:contains(path:docs/colors.md, \"lemon yellow banana\") OR missing_hellgate_token",
        "file:has.content(path:docs/colors.md, \"lemon yellow banana\") OR missing_hellgate_token",
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(result.typed_error.is_none(), "{query} must execute");
        ensure!(
            sorted_candidate_paths(&result) == ["docs/colors.md"],
            "{query} must isolate docs/colors.md, got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    for query in [
        "ripens NOT file:contains(path:docs/colors.md, \"lemon yellow banana\")",
        "ripens NOT file:has.content(path:docs/colors.md, \"lemon yellow banana\")",
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(result.typed_error.is_none(), "{query} must execute");
        ensure!(
            result.candidate_ids.is_empty(),
            "{query} must exclude the scoped file and return empty, got {:?}",
            sorted_candidate_paths(&result),
        );
    }
    Ok(())
}

fn verify_file_owner_contributor_and_projection(rt: &mut E2eRuntime) -> AnyResult<()> {
    let owner = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@alice) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&owner) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.owner(@alice) must gate corp-a paths, got {:?}",
        sorted_candidate_paths(&owner),
    );

    let any_owner = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner() shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&any_owner) == ["src/corp-a.rs", "src/corp-b.rs", "src/gate-a.rs"],
        "file:has.owner() must gate files with owners, got {:?}",
        sorted_candidate_paths(&any_owner),
    );

    let owner_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@mallory) shared_oracle_needle",
        10,
    );
    ensure!(
        owner_miss.candidate_ids.is_empty(),
        "file:has.owner(@mallory) must miss, got {:?}",
        sorted_candidate_paths(&owner_miss),
    );

    let contributor = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(alice) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&contributor) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor exact must gate alice paths, got {:?}",
        sorted_candidate_paths(&contributor),
    );

    let contributor_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(mallory) shared_oracle_needle",
        10,
    );
    ensure!(
        contributor_miss.candidate_ids.is_empty(),
        "file:has.contributor(mallory) must miss, got {:?}",
        sorted_candidate_paths(&contributor_miss),
    );

    let name_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(/alice examp.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&name_regex) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor name regex must gate alice paths, got {:?}",
        sorted_candidate_paths(&name_regex),
    );

    let email_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/alice@example\.com/) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&email_regex) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor email regex must gate alice paths, got {:?}",
        sorted_candidate_paths(&email_regex),
    );

    let no_canonical_fallback = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/^alice$/) shared_oracle_needle",
        10,
    );
    ensure!(
        no_canonical_fallback.candidate_ids.is_empty(),
        "file:has.contributor regex must not fall back to canonical-only identities, got {:?}",
        sorted_candidate_paths(&no_canonical_fallback),
    );

    let invalid_contributor_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/alice(/) shared_oracle_needle",
        10,
    );
    let Some(contributor_error) = invalid_contributor_regex.typed_error else {
        anyhow::bail!("invalid contributor regex must typed-fail");
    };
    ensure!(
        contributor_error.code.as_str() == "LEX_REGEX_PARSE_FAIL",
        "invalid contributor regex must fail with LEX_REGEX_PARSE_FAIL, got {}",
        contributor_error.code,
    );

    let owners = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "select:file.owners shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&owners)
            == [
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py",
            ],
        "select:file.owners must preserve lexical candidate set, got {:?}",
        sorted_candidate_paths(&owners),
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

#[test]
fn sourcegraph_rev_at_time_hellgate() -> AnyResult<()> {
    let (mut rt, head_pin) = boot_with_rev_at_time_generations()?;

    let head = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(2100-01-01T00:00:00Z) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        sorted_candidate_paths(&head) == ["src/head.rs"],
        "future rev:at.time must stay on head generation, got {:?}",
        sorted_candidate_paths(&head),
    );

    let relative = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(1 year ago) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        sorted_candidate_paths(&relative) == ["src/legacy.rs"],
        "relative rev:at.time must rebind to ancestor, got {:?}",
        sorted_candidate_paths(&relative),
    );

    let calendar = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(june 25 2017) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        calendar.candidate_ids.is_empty(),
        "calendar rev:at.time before reachable history must return empty, got {:?}",
        sorted_candidate_paths(&calendar),
    );

    let yesterday = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(yesterday) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        sorted_candidate_paths(&yesterday) == ["src/legacy.rs"],
        "named relative rev:at.time must rebind to ancestor, got {:?}",
        sorted_candidate_paths(&yesterday),
    );

    let invalid = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(definitely-not-a-timeref) needle_token",
        10,
        Some(head_pin),
    );
    let Some(error) = invalid.typed_error else {
        anyhow::bail!("invalid rev:at.time must typed-fail");
    };
    ensure!(
        error.code.as_str() == "HISTORY_INVALID_TIMEREF" && error.message.contains("timeref"),
        "invalid rev:at.time must fail with HISTORY_INVALID_TIMEREF + timeref message, got {} {:?}",
        error.code,
        error.message,
    );
    Ok(())
}

fn verify_legacy_index_and_boost(rt: &mut E2eRuntime) -> AnyResult<()> {
    let baseline = rt.query_text(TextQuerySyntax::Sourcegraph, "shared_oracle_needle", 10);
    ensure!(
        baseline.typed_error.is_none(),
        "baseline lexical query must succeed",
    );

    let index_no = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "index:no shared_oracle_needle",
        10,
    );
    ensure!(index_no.typed_error.is_none(), "index:no must execute");
    ensure!(
        sorted_candidate_ids(&index_no) == sorted_candidate_ids(&baseline),
        "index:no must preserve candidate universe, got {:?} vs {:?}",
        sorted_candidate_ids(&index_no),
        sorted_candidate_ids(&baseline),
    );

    let boosted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "boost:5 shared_oracle_needle",
        10,
    );
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

#[test]
fn sourcegraph_text_route_authorities_share_one_indexed_fixture() -> AnyResult<()> {
    let mut rt = boot_with_text_route_authorities()?;
    let verify_repo_meta_description_and_repo_file_fn: fn(&mut E2eRuntime) -> AnyResult<()> =
        verify_repo_meta_description_and_repo_file;
    for (name, verify) in [
        (
            "repo_meta_description_and_repo_file",
            verify_repo_meta_description_and_repo_file_fn,
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
            "file_owner_contributor_and_projection",
            verify_file_owner_contributor_and_projection,
        ),
        ("legacy_index_and_boost", verify_legacy_index_and_boost),
    ] {
        verify(&mut rt).map_err(|error| anyhow::anyhow!("{name}: {error:#}"))?;
    }
    Ok(())
}

#[test]
fn sourcegraph_dirty_only_runtime_metadata_hellgate() -> AnyResult<()> {
    let mut rt = boot_with_runtime_dirty_fixture()?;

    let dirty_only = rt.query_runtime_metadata(TextQuerySyntax::Sourcegraph, "dirty:only todo", 10);
    ensure!(
        dirty_only.typed_error.is_none(),
        "dirty:only runtime metadata query must execute: {:?}",
        dirty_only.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&dirty_only) == ["src/dirty.rs"],
        "dirty:only must isolate the dirty doc, got {:?}",
        sorted_candidate_paths(&dirty_only),
    );

    let follow_up = rt.query_runtime_metadata(TextQuerySyntax::Native, "dirty:yes todo", 10);
    ensure!(
        follow_up.typed_error.is_none(),
        "follow-up dirty:yes query must execute after dirty:only: {:?}",
        follow_up.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&follow_up) == ["src/dirty.rs"],
        "dirty:only must not poison the next runtime metadata query, got {:?}",
        sorted_candidate_paths(&follow_up),
    );
    Ok(())
}
