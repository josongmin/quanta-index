//! Execution-coverage tests for Sourcegraph filters that previously parsed and
//! translated but had no test asserting their *runtime effect*.
//!
//! Each filter is exercised through the real daemon (harness boot -> ingest ->
//! seal -> activate -> query) with a positive case (filter admits the row) and
//! a negative case (filter excludes it). This is the execution evidence the
//! `tools/benchmark/sourcegraph_parity.py` matrix scans for, lifting these
//! filters out of the "accepted but execution-untested" waiver.
//!
//! Covered here: history filters (`rev:` / `author:` / `committer:` /
//! `message:`, commit-route only), the `content:` pattern filter, the
//! `timeout:` option (typed-refused off the regex surface), and the repo
//! metadata filters (`archived:` / `context:`).

#![forbid(unsafe_code)]

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, FileContributorEntry, FileContributorIdentityEntry,
    FileContributorIngestBatch, FileOwnershipEntry, FileOwnershipIngestBatch, GenerationPin,
    HistoryIngestBatch, HistoryRefMutation, HistoryRefUpsert, LqVisibility, ManifestGeneration,
    RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoDescriptionEntry,
    RepoDescriptionIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicEntry, RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface, TextQuerySyntax, lex::CommitSha,
    lex::LanguageCode,
};
use std::time::{SystemTime, UNIX_EPOCH};

use quanta_index_searchd_harness as e2e_harness;

use crate::e2e_harness::{E2eHistoryFixtureSpec, E2eRuntime, E2eTextChunkSpec};

const REPO: &str = "repo-filter-exec";

/// Boot a runtime with one deterministic commit + a text chunk, sealed + active.
///
/// The commit carries distinct `author` / `committer` / `message` / `ref_name`
/// so each history filter can be pinned independently.
fn boot_with_history() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/history.rs";
    rt.ingest_text(REPO, path, "history proof needle_token\n")?;
    rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
        commit_sha: "0123456789abcdef0123456789abcdef01234567",
        file_path: path,
        author: "alice",
        committer: "bob",
        message: "fix: needle_token in history",
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 12,
        ref_name: "refs/heads/main",
        tag_name: "v1.0.0",
        added_text: "added needle_token line",
        removed_text: "removed line",
        touched_text: "touched line",
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn history_commit_count(rt: &mut E2eRuntime, query: &str) -> AnyResult<usize> {
    let result = rt.query_history(TextQuerySyntax::Sourcegraph, query, 10);
    if let Some(error) = result.typed_error {
        anyhow::bail!(
            "history query `{query}` returned typed error {}",
            error.code
        );
    }
    Ok(result.commit_ids.len())
}

#[test]
fn rev_filter_admits_matching_ref_and_excludes_others() -> AnyResult<()> {
    let mut rt = boot_with_history()?;
    let admitted = history_commit_count(&mut rt, "type:commit rev:refs/heads/main needle_token")?;
    ensure!(
        admitted == 1,
        "rev: on the seeded ref must admit the commit, got {admitted}"
    );
    let excluded = history_commit_count(&mut rt, "type:commit rev:refs/heads/absent needle_token")?;
    ensure!(
        excluded == 0,
        "rev: on an unknown ref must exclude the commit, got {excluded}"
    );
    Ok(())
}

#[test]
fn author_filter_admits_matching_author_and_excludes_others() -> AnyResult<()> {
    let mut rt = boot_with_history()?;
    let admitted = history_commit_count(&mut rt, "type:commit author:alice needle_token")?;
    ensure!(
        admitted == 1,
        "author: matching the commit author must admit it, got {admitted}"
    );
    let excluded = history_commit_count(&mut rt, "type:commit author:nobodyxyz needle_token")?;
    ensure!(
        excluded == 0,
        "author: not matching must exclude the commit, got {excluded}"
    );
    Ok(())
}

#[test]
fn committer_filter_admits_matching_committer_and_excludes_others() -> AnyResult<()> {
    let mut rt = boot_with_history()?;
    let admitted = history_commit_count(&mut rt, "type:commit committer:bob needle_token")?;
    ensure!(
        admitted == 1,
        "committer: matching must admit the commit, got {admitted}"
    );
    let excluded = history_commit_count(&mut rt, "type:commit committer:nobodyxyz needle_token")?;
    ensure!(
        excluded == 0,
        "committer: not matching must exclude the commit, got {excluded}"
    );
    Ok(())
}

#[test]
fn message_filter_admits_matching_message_and_excludes_others() -> AnyResult<()> {
    let mut rt = boot_with_history()?;
    let admitted = history_commit_count(&mut rt, "type:commit message:needle_token needle_token")?;
    ensure!(
        admitted == 1,
        "message: matching the commit message must admit it, got {admitted}"
    );
    let excluded = history_commit_count(&mut rt, "type:commit message:absentword needle_token")?;
    ensure!(
        excluded == 0,
        "message: not matching must exclude the commit, got {excluded}"
    );
    Ok(())
}

/// Boot a runtime with a small lexical corpus, sealed + active.
fn boot_with_lexical() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text(REPO, "src/lib.rs", "fn parity_needle_alpha() {}\n")?;
    rt.ingest_text(REPO, "src/other.rs", "let unrelated = quartz;\n")?;
    rt.ingest_text(REPO, "config/path_only_needle.toml", "value = 1\n")?;
    rt.ingest_text(REPO, "docs/colors.md", "the lemon yellow banana ripens\n")?;
    rt.ingest_text(
        REPO,
        "src/version.rs",
        "const VERSION: &str = \"v1.2.3-rc.4\";\n",
    )?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn candidate_paths(result: &e2e_harness::E2eQueryResult) -> Vec<String> {
    result
        .candidates
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect()
}

fn sorted_candidate_paths(result: &e2e_harness::E2eQueryResult) -> Vec<String> {
    let mut out = candidate_paths(result);
    out.sort();
    out
}

fn sorted_candidate_ids(result: &e2e_harness::E2eQueryResult) -> Vec<String> {
    let mut out = result.candidate_ids.clone();
    out.sort();
    out
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

fn boot_with_multi_repo() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
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
    Ok(())
}

fn now_epoch_ms() -> AnyResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("system time before unix epoch: {err}"))?
        .as_millis();
    u64::try_from(millis).map_err(|err| anyhow::anyhow!("epoch millis overflow u64: {err}"))
}

fn boot_with_multi_repo_and_commit_recency() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    let now_ms = now_epoch_ms()?;
    rt.publish_repo_commit_recency_batch(RepoCommitRecencyIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-repo-commit-recency".to_string(),
        entries: vec![
            RepoCommitRecencyEntry {
                source_repo_id: RepoId::new("corp-a"),
                latest_committer_time_ms: now_ms.saturating_sub(6 * 60 * 60 * 1000),
            },
            RepoCommitRecencyEntry {
                source_repo_id: RepoId::new("corp-b"),
                latest_committer_time_ms: 1_700_000_000_000,
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn boot_with_multi_repo_and_repo_meta() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    rt.publish_repo_meta_batch(RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-repo-meta".to_string(),
        entries: vec![
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-a"),
                key: "license".to_string(),
                value: "apache-2.0".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-b"),
                key: "license".to_string(),
                value: "gpl-3.0".to_string(),
            },
            RepoMetaEntry {
                source_repo_id: RepoId::new("corp-a"),
                key: "tier".to_string(),
                value: "prod".to_string(),
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn boot_with_multi_repo_and_repo_topic() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    rt.publish_repo_topic_batch(RepoTopicIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-repo-topic".to_string(),
        entries: vec![
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-a"),
                topic: "security".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-a"),
                topic: "platform".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new("corp-b"),
                topic: "ml".to_string(),
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn boot_with_multi_repo_and_repo_description() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    rt.publish_repo_description_batch(RepoDescriptionIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-repo-description".to_string(),
        entries: vec![
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-a"),
                description: "Apache distributed systems platform".to_string(),
            },
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-b"),
                description: "Machine learning training pipelines".to_string(),
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn boot_with_multi_repo_and_file_ownership() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    rt.publish_file_ownership_batch(FileOwnershipIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-file-ownership".to_string(),
        entries: vec![
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-a"),
                repo_relative_path: RepoRelativePath::new("src/corp-a.rs"),
                owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-a"),
                repo_relative_path: RepoRelativePath::new("src/gate-a.rs"),
                owners: vec!["@alice".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-b"),
                repo_relative_path: RepoRelativePath::new("src/corp-b.rs"),
                owners: vec!["@bob".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new("corp-b"),
                repo_relative_path: RepoRelativePath::new("src/gate-b.py"),
                owners: Vec::new(),
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn boot_with_multi_repo_and_file_contributor() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    rt.publish_file_contributor_batch(FileContributorIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-file-contributor".to_string(),
        entries: vec![
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-a"),
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
                source_repo_id: RepoId::new("corp-a"),
                repo_relative_path: RepoRelativePath::new("src/gate-a.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "alice".to_string(),
                    name: Some("Alice Example".to_string()),
                    email: Some("alice@example.com".to_string()),
                }],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-b"),
                repo_relative_path: RepoRelativePath::new("src/corp-b.rs"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "bob".to_string(),
                    name: Some("Bob Builder".to_string()),
                    email: Some("bob@example.com".to_string()),
                }],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new("corp-b"),
                repo_relative_path: RepoRelativePath::new("src/gate-b.py"),
                contributors: Vec::new(),
            },
        ],
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn rev_at_time_ancestor_revision() -> RevisionId {
    RevisionId::new("1111111111111111111111111111111111111111")
}

fn rev_at_time_head_revision() -> RevisionId {
    RevisionId::new("2222222222222222222222222222222222222222")
}

fn revision_scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn publish_revision_text_generation(
    rt: &mut E2eRuntime,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    path: &str,
    candidate_id: &str,
    content: &str,
) -> AnyResult<GenerationPin> {
    let manifest_digest = format!("lex:{path}:{}", generation.get());
    rt.publish_search_corpus_batch(SearchCorpusIngestBatch {
        repo_id: rt.repo(),
        revision_id: revision_id.clone(),
        generation,
        base_generation: None,
        manifest_digest,
        batch_digest: format!("lex-batch:{path}:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            scope: revision_scope_key(path),
            scope_digest: format!("scope:{path}:1-chunk"),
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
        "rev-at-time-ancestor",
        "needle_token legacy_choice\n",
    )?;
    let head_pin = publish_revision_text_generation(
        &mut rt,
        rev_at_time_head_revision(),
        ManifestGeneration::new(9),
        "src/head.rs",
        "rev-at-time-head",
        "needle_token head_choice\n",
    )?;
    rt.publish_history_batch(HistoryIngestBatch {
        repo_id: rt.repo(),
        revision_id: rev_at_time_head_revision(),
        generation: ManifestGeneration::new(9),
        manifest_digest: Some("rev-at-time-history".to_string()),
        batch_digest: "rev-at-time-history-batch".to_string(),
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

#[test]
fn content_filter_executes_as_a_text_pattern() -> AnyResult<()> {
    let mut rt = boot_with_lexical()?;
    let via_content = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "content:parity_needle_alpha",
        10,
    );
    ensure!(via_content.typed_error.is_none(), "content: must not error");
    ensure!(
        via_content.candidate_ids.len() == 1,
        "content:parity_needle_alpha must match the one doc that contains it, got {}",
        via_content.candidate_ids.len(),
    );
    let via_keyword = rt.query_text(TextQuerySyntax::Sourcegraph, "parity_needle_alpha", 10);
    ensure!(
        via_content.candidate_ids.len() == via_keyword.candidate_ids.len(),
        "content: pattern must match the same docs as the bare keyword ({} vs {})",
        via_content.candidate_ids.len(),
        via_keyword.candidate_ids.len(),
    );
    Ok(())
}

#[test]
fn timeout_option_is_typed_refused_off_the_regex_surface() -> AnyResult<()> {
    let mut rt = boot_with_lexical()?;
    // `timeout:` is executable only for regex-backed lexical queries; on a plain
    // keyword it must fail typed (fail-closed), not silently ignore the option.
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "timeout:5s parity_needle_alpha",
        10,
    );
    ensure!(
        result.typed_error.is_some(),
        "timeout: on a non-regex query must return a typed error, got {:?}",
        result.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_content_predicate_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.content(corp-a) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.content positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.content(corp-a) must gate to corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );
    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.content(missing-corpus-token) shared_oracle_needle",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:has.content miss must not error"
    );
    ensure!(
        excluded.candidate_ids.is_empty(),
        "repo:has.content miss must return no docs, got {:?}",
        excluded.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_commit_after_predicate_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_commit_recency()?;
    for query in [
        r#"repo:has.commit.after("2025-01-01T00:00:00Z") shared_oracle_needle"#,
        r#"repo:has.commit.after("2025-01-01") shared_oracle_needle"#,
        "repo:has.commit.after(400d) shared_oracle_needle",
    ] {
        let admitted = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            admitted.typed_error.is_none(),
            "{query} must not error: {:?}",
            admitted.typed_error,
        );
        ensure!(
            sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
            "{query} must gate to corp-a paths, got {:?}",
            sorted_candidate_paths(&admitted),
        );
    }

    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r#"repo:has.commit.after("2030-01-01T00:00:00Z") shared_oracle_needle"#,
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "future repo:has.commit.after miss must not error"
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "future repo:has.commit.after miss must return no docs, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_contains_commit_after_alias_executes_with_human_timeref_and_boolean_scope() -> AnyResult<()>
{
    let mut rt = boot_with_multi_repo_and_commit_recency()?;
    for query in [
        r#"repo:contains.commit.after("June 25 2025") shared_oracle_needle"#,
        "repo:contains.commit.after(yesterday) shared_oracle_needle",
        r#"repo:contains.commit.after("1 year ago") shared_oracle_needle"#,
    ] {
        let admitted = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            admitted.typed_error.is_none(),
            "{query} must not error: {:?}",
            admitted.typed_error,
        );
        ensure!(
            sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
            "{query} must gate to corp-a paths, got {:?}",
            sorted_candidate_paths(&admitted),
        );
    }

    let or_alias = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.commit.after(2025-01-01) OR missing_corpus_token",
        10,
    );
    ensure!(
        or_alias.typed_error.is_none(),
        "repo:contains.commit.after OR must not error"
    );
    ensure!(
        sorted_candidate_paths(&or_alias) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:contains.commit.after OR must stay on corp-a paths, got {:?}",
        sorted_candidate_paths(&or_alias),
    );

    let not_alias = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "shared_oracle_needle NOT repo:contains.commit.after(2025-01-01)",
        10,
    );
    ensure!(
        not_alias.typed_error.is_none(),
        "repo:contains.commit.after NOT must not error"
    );
    ensure!(
        sorted_candidate_paths(&not_alias) == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "repo:contains.commit.after NOT must leave only corp-b, got {:?}",
        sorted_candidate_paths(&not_alias),
    );
    Ok(())
}

#[test]
fn repo_has_commit_after_rejects_invalid_timeref_typed() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_commit_recency()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(definitely-not-a-timeref) shared_oracle_needle",
        10,
    );
    let Some(error) = result.typed_error else {
        anyhow::bail!(
            "invalid repo:has.commit.after timeref must typed-fail, got {:?}",
            result.candidate_ids
        );
    };
    ensure!(
        error.code == "HISTORY_INVALID_TIMEREF",
        "invalid repo:has.commit.after timeref must fail with HISTORY_INVALID_TIMEREF, got {}",
        error.code
    );
    Ok(())
}

#[test]
fn repo_has_meta_predicate_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_repo_meta()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license:apache-2.0) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.meta positive must not error: {:?}",
        admitted.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.meta(license:apache-2.0) must gate to corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );

    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license:mit) shared_oracle_needle",
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "repo:has.meta miss must not error: {:?}",
        miss.typed_error,
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "repo:has.meta miss must return no docs, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_meta_key_only_existence_executes_on_sourcegraph_surface() -> AnyResult<()> {
    // SGX-03: `repo:has.meta(key)` gates repos that have the key PRESENT with any
    // value — genuine existence (contains_key), not a wildcard or empty-string
    // match. Corpus: corp-a={license, tier}, corp-b={license}.
    let mut rt = boot_with_multi_repo_and_repo_meta()?;

    // `tier` exists only on corp-a → gates to corp-a. (An inverted "key absent"
    // gate would wrongly select corp-b, so this distinguishes the two.)
    let tier = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(tier) shared_oracle_needle",
        10,
    );
    ensure!(
        tier.typed_error.is_none(),
        "key existence must not error: {:?}",
        tier.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&tier) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.meta(tier) must gate to corp-a (only repo with key `tier`), got {:?}",
        sorted_candidate_paths(&tier),
    );

    // `license` exists on both repos → gates to both.
    let license = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&license)
            == [
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py"
            ],
        "repo:has.meta(license) must gate to both repos, got {:?}",
        sorted_candidate_paths(&license),
    );

    // A key present on no repo → empty.
    let missing = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(absent_key_zzz) shared_oracle_needle",
        10,
    );
    ensure!(
        missing.typed_error.is_none() && missing.candidate_ids.is_empty(),
        "absent key must gate to empty, got {:?}",
        sorted_candidate_paths(&missing),
    );

    // Exact `key:value` must NOT regress: corp-a has license=apache-2.0.
    let kv = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license:apache-2.0) shared_oracle_needle",
        10,
    );
    ensure!(
        sorted_candidate_paths(&kv) == ["src/corp-a.rs", "src/gate-a.rs"],
        "exact key:value must still gate to corp-a only, got {:?}",
        sorted_candidate_paths(&kv),
    );
    Ok(())
}

#[test]
fn repo_has_meta_tag_existence_executes_on_sourcegraph_surface() -> AnyResult<()> {
    // SGX-03: `repo:has.meta(tag:)` (empty value) is the key-existence shape —
    // `repo:has.meta(tier:)` gates repos where key `tier` is present (corp-a),
    // NOT repos with an empty-string `tier` value.
    let mut rt = boot_with_multi_repo_and_repo_meta()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(tier:) shared_oracle_needle",
        10,
    );
    ensure!(
        result.typed_error.is_none(),
        "tag existence must not error: {:?}",
        result.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&result) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.meta(tier:) must gate to corp-a (key present), got {:?}",
        sorted_candidate_paths(&result),
    );
    Ok(())
}

#[test]
fn repo_has_meta_regex_family_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_repo_meta()?;

    let regex_pair = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/license/:/apache.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        regex_pair.typed_error.is_none(),
        "regex key/value must not error: {:?}",
        regex_pair.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&regex_pair) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex key/value must gate to corp-a only, got {:?}",
        sorted_candidate_paths(&regex_pair),
    );

    let regex_key_only = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/tier/) shared_oracle_needle",
        10,
    );
    ensure!(
        regex_key_only.typed_error.is_none(),
        "regex key-only must not error: {:?}",
        regex_key_only.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&regex_key_only) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex key-only must gate to corp-a only, got {:?}",
        sorted_candidate_paths(&regex_key_only),
    );

    let exact_key_regex_value = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(license:/gpl-.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        exact_key_regex_value.typed_error.is_none(),
        "exact-key regex-value must not error: {:?}",
        exact_key_regex_value.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&exact_key_regex_value)
            == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "exact-key regex-value must gate to corp-b only, got {:?}",
        sorted_candidate_paths(&exact_key_regex_value),
    );

    let regex_key_exact_value = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/licens./:apache-2.0) shared_oracle_needle",
        10,
    );
    ensure!(
        regex_key_exact_value.typed_error.is_none(),
        "regex-key exact-value must not error: {:?}",
        regex_key_exact_value.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&regex_key_exact_value) == ["src/corp-a.rs", "src/gate-a.rs"],
        "regex-key exact-value must gate to corp-a only, got {:?}",
        sorted_candidate_paths(&regex_key_exact_value),
    );

    let regex_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/nope/:/apache.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        regex_miss.typed_error.is_none(),
        "regex miss must not error: {:?}",
        regex_miss.typed_error,
    );
    ensure!(
        regex_miss.candidate_ids.is_empty(),
        "regex miss must return no docs, got {:?}",
        sorted_candidate_paths(&regex_miss),
    );
    Ok(())
}

#[test]
fn repo_has_meta_invalid_regex_typed_fails() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_repo_meta()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.meta(/license(/:/apache.*/) shared_oracle_needle",
        10,
    );
    let Some(error) = result.typed_error else {
        anyhow::bail!(
            "invalid regex repo:has.meta must typed-fail, got {:?}",
            result.candidate_ids
        );
    };
    ensure!(
        error.code.starts_with("LEX_REGEX_"),
        "invalid regex repo:has.meta must fail with lexical regex code, got {}",
        error.code
    );
    ensure!(
        error.message.contains("failed to compile"),
        "invalid regex repo:has.meta diagnostic must name compile failure, got {:?}",
        error.message
    );
    Ok(())
}

#[test]
fn repo_has_description_predicate_executes_on_sourcegraph_surface() -> AnyResult<()> {
    // SGX-02: Sourcegraph's `repo:has.description(<regex>)` filters repos by their
    // producer-published description text, matched as a regex. The description is
    // a distinct source-repo keyed authority (RepoDescriptionIngestBatch ->
    // repo-description.cbor shard), NOT folded into repo:has.meta or repo topics,
    // and the search-plane never fabricates source bytes — the producer publishes
    // the description and the gate matches it.
    let mut rt = boot_with_multi_repo_and_repo_description()?;

    // Literal substring pattern gates to the owning repo's docs.
    let corp_a = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(distributed) shared_oracle_needle",
        10,
    );
    ensure!(
        corp_a.typed_error.is_none(),
        "repo:has.description positive must not error: {:?}",
        corp_a.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&corp_a) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.description(distributed) must gate to corp-a description-owned paths, got {:?}",
        sorted_candidate_paths(&corp_a),
    );

    // A different pattern selects the other repo — proving per-repo correlation,
    // not a single shared match set.
    let corp_b = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(learning) shared_oracle_needle",
        10,
    );
    ensure!(
        corp_b.typed_error.is_none(),
        "repo:has.description(learning) must not error: {:?}",
        corp_b.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&corp_b) == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "repo:has.description(learning) must gate to corp-b description-owned paths, got {:?}",
        sorted_candidate_paths(&corp_b),
    );

    // Regex semantics, not literal substring: `distribut.d` matches
    // "distributed" via the `.` wildcard (a literal-substring match would miss).
    let regex_hit = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"distribut.d\") shared_oracle_needle",
        10,
    );
    ensure!(
        regex_hit.typed_error.is_none(),
        "repo:has.description regex must not error: {:?}",
        regex_hit.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&regex_hit) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.description regex `distribut.d` must match corp-a via wildcard, got {:?}",
        sorted_candidate_paths(&regex_hit),
    );

    // Anchored regex is honored, not stripped: `^Apache` matches corp-a (its
    // description starts with "Apache"); `^distributed` misses because
    // "distributed" is not at offset 0 of "Apache distributed systems platform".
    let anchor_hit = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"^Apache\") shared_oracle_needle",
        10,
    );
    ensure!(
        anchor_hit.typed_error.is_none()
            && sorted_candidate_paths(&anchor_hit) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.description(^Apache) must anchor-match corp-a, got err={:?} paths={:?}",
        anchor_hit.typed_error,
        sorted_candidate_paths(&anchor_hit),
    );
    let anchor_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"^distributed\") shared_oracle_needle",
        10,
    );
    ensure!(
        anchor_miss.typed_error.is_none() && anchor_miss.candidate_ids.is_empty(),
        "repo:has.description(^distributed) must anchor-miss (not at start), got err={:?} ids={:?}",
        anchor_miss.typed_error,
        anchor_miss.candidate_ids,
    );

    // A single pattern matching BOTH repos returns the full corpus — proving the
    // collector yields a multi-repo id set, not single-repo selection.
    let both = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"systems|pipelines\") shared_oracle_needle",
        10,
    );
    ensure!(
        both.typed_error.is_none()
            && sorted_candidate_paths(&both)
                == [
                    "lib/gate-a.rs",
                    "src/corp-a.rs",
                    "src/corp-b.rs",
                    "src/gate-a.rs",
                    "src/gate-b.py",
                ],
        "repo:has.description(systems|pipelines) must gate to both repos, got err={:?} paths={:?}",
        both.typed_error,
        sorted_candidate_paths(&both),
    );

    // Miss returns no docs, fails open to neither all nor a fabricated set.
    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(nonexistentxyz) shared_oracle_needle",
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "repo:has.description miss must not error: {:?}",
        miss.typed_error,
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "repo:has.description miss must return no docs, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_description_invalid_regex_fails_closed() -> AnyResult<()> {
    // A malformed regex pattern must surface a typed error, never a silently
    // empty candidate set.
    let mut rt = boot_with_multi_repo_and_repo_description()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(\"a[b\") shared_oracle_needle",
        10,
    );
    let Some(error) = result.typed_error else {
        anyhow::bail!(
            "malformed repo:has.description regex must typed-fail, got {:?}",
            result.candidate_ids
        );
    };
    ensure!(
        error.code.starts_with("LEX_REGEX_"),
        "malformed description regex must fail with a LEX_REGEX_* code, got {}",
        error.code
    );
    Ok(())
}

#[test]
fn repo_has_description_without_authority_fails_closed() -> AnyResult<()> {
    // When no producer has published a description authority for the generation,
    // the gate must fail closed with a typed unavailable error — never silently
    // match nothing as if every repo lacked a description.
    let mut rt = boot_with_multi_repo_and_repo_meta()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(enginecorp) shared_oracle_needle",
        10,
    );
    let Some(error) = result.typed_error else {
        anyhow::bail!(
            "repo:has.description without authority must typed-fail, got {:?}",
            result.candidate_ids
        );
    };
    ensure!(
        error.code == "REPO_DESCRIPTION_UNAVAILABLE",
        "missing description authority must fail with REPO_DESCRIPTION_UNAVAILABLE, got {}",
        error.code
    );
    Ok(())
}

#[test]
fn repo_has_description_conflicting_batch_fails_closed() -> AnyResult<()> {
    // A batch carrying two different descriptions for the same source repo is a
    // conflicting authority input. Publish must fail closed, never silently
    // last-wins one of the two.
    let mut rt = E2eRuntime::boot()?;
    seed_multi_repo_chunks(&mut rt)?;
    let publish = rt.publish_repo_description_batch(RepoDescriptionIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "e2e-repo-description-conflict".to_string(),
        entries: vec![
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-a"),
                description: "First description".to_string(),
            },
            RepoDescriptionEntry {
                source_repo_id: RepoId::new("corp-a"),
                description: "Conflicting second description".to_string(),
            },
        ],
    });
    ensure!(
        publish.is_err(),
        "conflicting repo description batch must fail closed at publish, but it succeeded",
    );
    Ok(())
}

#[test]
fn repo_has_description_non_textual_arg_is_typed_unsupported() -> AnyResult<()> {
    // A numeric argument is not an admissible description pattern; it must
    // typed-fail through the unimplemented-predicate path, never coerce to a
    // string pattern or silently match nothing.
    let mut rt = boot_with_multi_repo_and_repo_description()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.description(123) shared_oracle_needle",
        10,
    );
    let Some(error) = result.typed_error else {
        anyhow::bail!(
            "numeric repo:has.description arg must typed-fail, got {:?}",
            result.candidate_ids
        );
    };
    ensure!(
        error.code == "LEX_PREDICATE_UNIMPLEMENTED",
        "numeric description arg must fail with LEX_PREDICATE_UNIMPLEMENTED, got {}",
        error.code
    );
    Ok(())
}

#[test]
fn repo_has_topic_predicate_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_repo_topic()?;

    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.topic(security) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.topic positive must not error: {:?}",
        admitted.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.topic positive must gate to corp-a topic-owned paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );

    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.topic(compliance) shared_oracle_needle",
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "repo:has.topic miss must not error: {:?}",
        miss.typed_error,
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "repo:has.topic miss must return no docs, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn select_file_owners_projects_owner_rows_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_file_ownership()?;
    let result = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "select:file.owners shared_oracle_needle",
        10,
    );
    ensure!(
        result.typed_error.is_none(),
        "select:file.owners must not typed-fail: {:?}",
        result.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&result)
            == [
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py",
            ],
        "select:file.owners must preserve the lexical candidate set, got {:?}",
        sorted_candidate_paths(&result),
    );
    ensure!(
        sorted_file_owner_projection(&result)
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
        sorted_file_owner_projection(&result),
    );
    Ok(())
}

#[test]
fn file_has_contributor_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_file_contributor()?;

    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(alice) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "file:has.contributor positive must not error: {:?}",
        admitted.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.contributor(alice) must gate to alice-contributed corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );

    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(mallory) shared_oracle_needle",
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "file:has.contributor miss must not error: {:?}",
        miss.typed_error,
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "file:has.contributor miss must return no docs, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn file_has_contributor_supports_name_and_email_regex_without_canonical_fallback() -> AnyResult<()>
{
    let mut rt = boot_with_multi_repo_and_file_contributor()?;

    let prefix = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(alic) shared_oracle_needle",
        10,
    );
    ensure!(
        prefix.typed_error.is_none(),
        "file:has.contributor(alic) prefix must not error: {:?}",
        prefix.typed_error,
    );
    ensure!(
        prefix.candidate_ids.is_empty(),
        "file:has.contributor(alic) prefix must not substring-match alice, got {:?}",
        sorted_candidate_paths(&prefix),
    );

    let name_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.contributor(/alice examp.*/) shared_oracle_needle",
        10,
    );
    ensure!(
        name_regex.typed_error.is_none(),
        "name-regex file:has.contributor must not error: {:?}",
        name_regex.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&name_regex) == ["src/corp-a.rs", "src/gate-a.rs"],
        "name-regex file:has.contributor must gate to alice-contributed corp-a paths, got {:?}",
        sorted_candidate_paths(&name_regex),
    );

    let email_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/alice@example\.com/) shared_oracle_needle",
        10,
    );
    ensure!(
        email_regex.typed_error.is_none(),
        "email-regex file:has.contributor must not error: {:?}",
        email_regex.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&email_regex) == ["src/corp-a.rs", "src/gate-a.rs"],
        "email-regex file:has.contributor must gate to alice email-matched corp-a paths, got {:?}",
        sorted_candidate_paths(&email_regex),
    );

    let no_canonical_fallback = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/^alice$/) shared_oracle_needle",
        10,
    );
    ensure!(
        no_canonical_fallback.typed_error.is_none(),
        "canonical-only regex file:has.contributor must not error: {:?}",
        no_canonical_fallback.typed_error,
    );
    ensure!(
        no_canonical_fallback.candidate_ids.is_empty(),
        "regex file:has.contributor must not fall back to canonical-only identities, got {:?}",
        sorted_candidate_paths(&no_canonical_fallback),
    );

    let invalid_regex = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r"file:has.contributor(/alice(/) shared_oracle_needle",
        10,
    );
    let Some(error) = invalid_regex.typed_error else {
        anyhow::bail!(
            "invalid regex file:has.contributor must typed-fail, got {:?}",
            invalid_regex.candidate_ids
        );
    };
    ensure!(
        error.code.starts_with("LEX_REGEX_"),
        "invalid regex file:has.contributor must fail with LEX_REGEX_*, got {}",
        error.code
    );
    Ok(())
}

#[test]
fn file_has_owner_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_file_ownership()?;

    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@alice) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "file:has.owner positive must not error: {:?}",
        admitted.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.owner(@alice) must gate to @alice-owned corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );

    let any_owner = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner() shared_oracle_needle",
        10,
    );
    ensure!(
        any_owner.typed_error.is_none(),
        "file:has.owner() must not error: {:?}",
        any_owner.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&any_owner) == ["src/corp-a.rs", "src/corp-b.rs", "src/gate-a.rs"],
        "file:has.owner() must gate to files with any owner, got {:?}",
        sorted_candidate_paths(&any_owner),
    );

    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@carol) shared_oracle_needle",
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "file:has.owner miss must not error: {:?}",
        miss.typed_error,
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "file:has.owner miss must return no docs, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn file_has_owner_executes_inside_boolean_scope() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo_and_file_ownership()?;

    let or_query = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.owner(@alice) OR missing_corpus_token",
        10,
    );
    ensure!(
        or_query.typed_error.is_none(),
        "file:has.owner OR must not error: {:?}",
        or_query.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&or_query) == ["src/corp-a.rs", "src/gate-a.rs"],
        "file:has.owner OR must stay on @alice-owned paths, got {:?}",
        sorted_candidate_paths(&or_query),
    );

    let not_query = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "shared_oracle_needle NOT file:has.owner()",
        10,
    );
    ensure!(
        not_query.typed_error.is_none(),
        "file:has.owner NOT must not error: {:?}",
        not_query.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&not_query) == ["lib/gate-a.rs", "src/gate-b.py"],
        "shared_oracle_needle NOT file:has.owner() must leave ownerless files only, got {:?}",
        sorted_candidate_paths(&not_query),
    );
    Ok(())
}

#[test]
fn rev_at_time_text_route_rebinds_to_selected_revision_generation() -> AnyResult<()> {
    let (mut rt, head_pin) = boot_with_rev_at_time_generations()?;

    let head = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(2100-01-01T00:00:00Z) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        head.typed_error.is_none(),
        "future rev:at.time boundary must not error: {:?}",
        head.typed_error
    );
    ensure!(
        sorted_candidate_paths(&head) == ["src/head.rs"],
        "future rev:at.time boundary must stay on head revision, got {:?}",
        sorted_candidate_paths(&head)
    );

    let human_relative = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(1 year ago) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        human_relative.typed_error.is_none(),
        "human relative rev:at.time must not error: {:?}",
        human_relative.typed_error
    );
    ensure!(
        sorted_candidate_paths(&human_relative) == ["src/legacy.rs"],
        "1 year ago must rebind to the reachable ancestor, got {:?}",
        sorted_candidate_paths(&human_relative)
    );

    let human_calendar = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(june 25 2017) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        human_calendar.typed_error.is_none(),
        "calendar rev:at.time must not error: {:?}",
        human_calendar.typed_error
    );
    ensure!(
        sorted_candidate_paths(&human_calendar).is_empty(),
        "calendar rev:at.time before all reachable commits must return nothing, got {:?}",
        sorted_candidate_paths(&human_calendar)
    );

    let named_relative = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(yesterday) needle_token",
        10,
        Some(head_pin.clone()),
    );
    ensure!(
        named_relative.typed_error.is_none(),
        "named relative rev:at.time must not error: {:?}",
        named_relative.typed_error
    );
    ensure!(
        sorted_candidate_paths(&named_relative) == ["src/legacy.rs"],
        "yesterday must rebind to the reachable ancestor, got {:?}",
        sorted_candidate_paths(&named_relative)
    );

    let invalid = rt.query_text_with_pin(
        TextQuerySyntax::Sourcegraph,
        "rev:at.time(definitely-not-a-timeref) needle_token",
        10,
        Some(head_pin),
    );
    let Some(error) = invalid.typed_error else {
        anyhow::bail!(
            "invalid rev:at.time timeref must typed-fail, got {:?}",
            invalid.candidate_ids
        );
    };
    ensure!(
        error.code == "HISTORY_INVALID_TIMEREF",
        "invalid rev:at.time timeref must fail with HISTORY_INVALID_TIMEREF, got {}",
        error.code
    );
    ensure!(
        error.message.contains("timeref"),
        "invalid rev:at.time timeref must mention the timeref parse failure, got {:?}",
        error.message
    );
    Ok(())
}

#[test]
fn repo_has_path_alias_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.path(src/gate-a.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.path positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.path(src/gate-a.rs) must gate to corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );
    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.path(src/missing.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:has.path miss must not error"
    );
    ensure!(
        excluded.candidate_ids.is_empty(),
        "repo:has.path miss must return no docs, got {:?}",
        excluded.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_file_scalar_path_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(src/gate-a.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.file(<scalar-path>) positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.file(src/gate-a.rs) must gate to corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );
    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(src/missing.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:has.file(<scalar-path>) miss must not error"
    );
    ensure!(
        excluded.candidate_ids.is_empty(),
        "repo:has.file(<scalar-path>) miss must return no docs, got {:?}",
        excluded.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_file_matcher_combinations_execute_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    for (query, expected) in [
        (
            "repo:has.file(path:src/gate-a.rs, name:gate-a.rs) shared_oracle_needle",
            vec!["src/corp-a.rs", "src/gate-a.rs"],
        ),
        (
            "repo:has.file(path:src/gate-a.rs, lang:rust) shared_oracle_needle",
            vec!["src/corp-a.rs", "src/gate-a.rs"],
        ),
        (
            "repo:has.file(name:gate-a.rs, lang:rust) shared_oracle_needle",
            vec![
                "lib/gate-a.rs",
                "src/corp-a.rs",
                "src/corp-b.rs",
                "src/gate-a.rs",
                "src/gate-b.py",
            ],
        ),
        (
            "repo:has.file(path:src/gate-a.rs, name:gate-a.rs, lang:rust) shared_oracle_needle",
            vec!["src/corp-a.rs", "src/gate-a.rs"],
        ),
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "{query} must not error: {:?}",
            result.typed_error,
        );
        let expected = expected.into_iter().map(str::to_string).collect::<Vec<_>>();
        ensure!(
            sorted_candidate_paths(&result) == expected,
            "{query} must gate exact repo set, got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    for query in [
        "repo:has.file(path:src/gate-a.rs, name:missing.rs) shared_oracle_needle",
        "repo:has.file(path:src/gate-a.rs, lang:python) shared_oracle_needle",
        "repo:has.file(name:gate-a.rs, lang:python) shared_oracle_needle",
        "repo:has.file(path:src/gate-a.rs, name:gate-a.rs, lang:python) shared_oracle_needle",
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "{query} must not error: {:?}",
            result.typed_error,
        );
        ensure!(
            sorted_candidate_paths(&result).is_empty(),
            "{query} must miss, got {:?}",
            sorted_candidate_paths(&result),
        );
    }
    Ok(())
}

#[test]
fn repo_contains_path_alias_executes_on_sourcegraph_surface() -> AnyResult<()> {
    // SGT-01: `repo:contains.path(...)` is Sourcegraph's alias of
    // `repo:has.path(...)`; it canonicalizes onto the repo-file gate exactly
    // like the native `repo.has.path` alias.
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.path(src/gate-a.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:contains.path positive must not error: {:?}",
        admitted.typed_error,
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:contains.path(src/gate-a.rs) must gate to corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );
    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.path(src/missing.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:contains.path miss must not error"
    );
    ensure!(
        excluded.candidate_ids.is_empty(),
        "repo:contains.path miss must return no docs, got {:?}",
        excluded.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_contains_file_alias_executes_on_sourcegraph_surface() -> AnyResult<()> {
    // SGT-01: `repo:contains.file(...)` is Sourcegraph's alias of
    // `repo:has.file(...)`; unlike `contains.path` it forwards the full matcher
    // surface (scalar shorthand AND path:/name:/lang: filters) unchanged.
    let mut rt = boot_with_multi_repo()?;
    for (query, expected) in [
        (
            "repo:contains.file(src/gate-a.rs) shared_oracle_needle",
            vec!["src/corp-a.rs", "src/gate-a.rs"],
        ),
        (
            "repo:contains.file(path:src/gate-a.rs, lang:rust) shared_oracle_needle",
            vec!["src/corp-a.rs", "src/gate-a.rs"],
        ),
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "{query} must not error: {:?}",
            result.typed_error,
        );
        let expected = expected.into_iter().map(str::to_string).collect::<Vec<_>>();
        ensure!(
            sorted_candidate_paths(&result) == expected,
            "{query} must gate exact repo set, got {:?}",
            sorted_candidate_paths(&result),
        );
    }
    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.file(src/missing.rs) shared_oracle_needle",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:contains.file miss must not error"
    );
    ensure!(
        excluded.candidate_ids.is_empty(),
        "repo:contains.file miss must return no docs, got {:?}",
        excluded.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_has_file_path_content_correlates_per_document_on_sourcegraph_surface() -> AnyResult<()> {
    // SGX-01: `repo:has.file(path:... content:...)` gates a repo only when ONE
    // file satisfies BOTH path AND content — true per-document correlation, not a
    // repo-level cross-product. Corpus: corp-a has src/gate-a.rs (contains the
    // unique token "123") and src/corp-a.rs (does NOT contain "123").
    let mut rt = boot_with_multi_repo()?;

    // Positive: the file at src/gate-a.rs DOES contain "123" → gates corp-a.
    for query in [
        "repo:has.file(path:src/gate-a.rs, content:123) shared_oracle_needle",
        "repo:contains.file(path:src/gate-a.rs, content:123) shared_oracle_needle",
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "{query} must not error: {:?}",
            result.typed_error,
        );
        ensure!(
            sorted_candidate_paths(&result) == ["src/corp-a.rs", "src/gate-a.rs"],
            "{query} must gate to corp-a (file at path contains content), got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    // Anti-overmatch: src/corp-a.rs does NOT contain "123" (only src/gate-a.rs,
    // a DIFFERENT file in the same repo, does). A repo-level conjunction would
    // wrongly match corp-a; true per-doc correlation must return EMPTY.
    let overmatch = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/corp-a.rs, content:123) shared_oracle_needle",
        10,
    );
    ensure!(
        overmatch.typed_error.is_none(),
        "anti-overmatch query must not error: {:?}",
        overmatch.typed_error,
    );
    ensure!(
        overmatch.candidate_ids.is_empty(),
        "src/corp-a.rs has no `123`; path+content must NOT match via a different file, got {:?}",
        sorted_candidate_paths(&overmatch),
    );

    // Content-miss: a content token present in no file at the path → empty.
    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs, content:absent_zzz_token) shared_oracle_needle",
        10,
    );
    ensure!(
        miss.typed_error.is_none(),
        "content-miss must not error: {:?}",
        miss.typed_error,
    );
    ensure!(
        miss.candidate_ids.is_empty(),
        "content-miss must return no docs, got {:?}",
        sorted_candidate_paths(&miss),
    );

    // Empty content value still fails closed (typed).
    let empty = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs, content:) shared_oracle_needle",
        10,
    );
    let Some(error) = empty.typed_error else {
        anyhow::bail!(
            "empty content value must typed-fail, got {:?}",
            empty.candidate_ids
        );
    };
    ensure!(
        error.code == "LEX_PREDICATE_UNIMPLEMENTED" && error.message.contains("content:"),
        "empty content must fail with the content: empty reason, got {} {:?}",
        error.code,
        error.message,
    );
    Ok(())
}

#[test]
fn file_contains_content_alias_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_lexical()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:contains.content(\"lemon yellow banana\")",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "file:contains.content positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["docs/colors.md"],
        "file:contains.content phrase must match the phrase doc, got {:?}",
        sorted_candidate_paths(&admitted),
    );
    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:contains.content(\"absent_zzz_token\")",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "file:contains.content miss must not error"
    );
    ensure!(
        excluded.candidate_ids.is_empty(),
        "file:contains.content miss must return nothing, got {:?}",
        excluded.candidate_ids,
    );
    Ok(())
}

#[test]
fn repo_contains_content_alias_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.content(\"gate-a only\") shared_oracle_needle",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:contains.content positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:contains.content(\"gate-a only\") must gate to corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );
    let or_alias = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:contains.content(\"gate-a only\") OR missing_corpus_token",
        10,
    );
    ensure!(
        or_alias.typed_error.is_none(),
        "repo:contains.content alias OR must not error"
    );
    ensure!(
        sorted_candidate_paths(&or_alias) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:contains.content alias OR must return corp-a repo only, got {:?}",
        sorted_candidate_paths(&or_alias),
    );
    let not_alias = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "shared_oracle_needle NOT repo:contains.content(\"gate-a only\")",
        10,
    );
    ensure!(
        not_alias.typed_error.is_none(),
        "repo:contains.content alias NOT must not error"
    );
    ensure!(
        sorted_candidate_paths(&not_alias) == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "shared_oracle_needle NOT repo:contains.content(...) must return corp-b repo only, got {:?}",
        sorted_candidate_paths(&not_alias),
    );
    Ok(())
}

#[test]
fn numeric_content_predicates_execute_on_sourcegraph_surface() -> AnyResult<()> {
    let mut lexical = boot_with_lexical()?;
    let file_contains = lexical.query_text(TextQuerySyntax::Sourcegraph, "file:contains(1)", 10);
    ensure!(
        file_contains.typed_error.is_none(),
        "file:contains(1) must not error"
    );
    ensure!(
        sorted_candidate_paths(&file_contains) == ["config/path_only_needle.toml"],
        "file:contains(1) must hit the numeric file, got {:?}",
        sorted_candidate_paths(&file_contains),
    );
    let file_has_content =
        lexical.query_text(TextQuerySyntax::Sourcegraph, "file:has.content(1)", 10);
    ensure!(
        file_has_content.typed_error.is_none(),
        "file:has.content(1) must not error"
    );
    ensure!(
        sorted_candidate_paths(&file_has_content) == ["config/path_only_needle.toml"],
        "file:has.content(1) must hit the numeric file, got {:?}",
        sorted_candidate_paths(&file_has_content),
    );
    let file_has_content_miss =
        lexical.query_text(TextQuerySyntax::Sourcegraph, "file:has.content(404)", 10);
    ensure!(
        file_has_content_miss.typed_error.is_none(),
        "file:has.content(404) must not error"
    );
    ensure!(
        sorted_candidate_paths(&file_has_content_miss).is_empty(),
        "file:has.content(404) must miss, got {:?}",
        sorted_candidate_paths(&file_has_content_miss),
    );

    let mut multi_repo = boot_with_multi_repo()?;
    let repo_content = multi_repo.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.content(123) shared_oracle_needle",
        10,
    );
    ensure!(
        repo_content.typed_error.is_none(),
        "repo:has.content(123) must not error"
    );
    ensure!(
        sorted_candidate_paths(&repo_content) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.content(123) must gate corp-a open, got {:?}",
        sorted_candidate_paths(&repo_content),
    );
    Ok(())
}

#[test]
fn scoped_file_content_predicates_execute_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_lexical()?;
    let path_hit = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.content(path:docs/colors.md, \"lemon yellow banana\")",
        10,
    );
    ensure!(
        path_hit.typed_error.is_none(),
        "scoped file:has.content(path:..., phrase) must not error"
    );
    ensure!(
        sorted_candidate_paths(&path_hit) == ["docs/colors.md"],
        "scoped file:has.content(path:...) must isolate docs/colors.md, got {:?}",
        sorted_candidate_paths(&path_hit),
    );

    let file_hit = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.content(file:colors.md, \"lemon yellow banana\")",
        10,
    );
    ensure!(
        file_hit.typed_error.is_none(),
        "scoped file:has.content(file:..., phrase) must not error"
    );
    ensure!(
        sorted_candidate_paths(&file_hit) == ["docs/colors.md"],
        "scoped file:has.content(file:...) must isolate docs/colors.md, got {:?}",
        sorted_candidate_paths(&file_hit),
    );
    let path_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.content(path:src/lib.rs, \"lemon yellow banana\")",
        10,
    );
    ensure!(
        path_miss.typed_error.is_none(),
        "scoped file:has.content(path:src/lib.rs, phrase) must not error"
    );
    ensure!(
        sorted_candidate_paths(&path_miss).is_empty(),
        "scoped file:has.content(path:src/lib.rs, ...) must miss, got {:?}",
        sorted_candidate_paths(&path_miss),
    );
    let file_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.content(file:missing.md, \"lemon yellow banana\")",
        10,
    );
    ensure!(
        file_miss.typed_error.is_none(),
        "scoped file:has.content(file:missing.md, phrase) must not error"
    );
    ensure!(
        sorted_candidate_paths(&file_miss).is_empty(),
        "scoped file:has.content(file:missing.md, ...) must miss, got {:?}",
        sorted_candidate_paths(&file_miss),
    );

    let lang_hit = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.content(lang:rust, /v\\d+\\.\\d+\\.\\d+/)",
        10,
    );
    ensure!(
        lang_hit.typed_error.is_none(),
        "scoped file:has.content(lang:rust, regex) must not error"
    );
    ensure!(
        sorted_candidate_paths(&lang_hit) == ["src/version.rs"],
        "scoped file:has.content(lang:rust, regex) must isolate src/version.rs, got {:?}",
        sorted_candidate_paths(&lang_hit),
    );
    let lang_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:has.content(lang:markdown, /v\\d+\\.\\d+\\.\\d+/)",
        10,
    );
    ensure!(
        lang_miss.typed_error.is_none(),
        "scoped file:has.content(lang:markdown, regex) must not error"
    );
    ensure!(
        sorted_candidate_paths(&lang_miss).is_empty(),
        "scoped file:has.content(lang:markdown, regex) must miss, got {:?}",
        sorted_candidate_paths(&lang_miss),
    );

    let scoped_and_miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "file:contains(path:src/lib.rs, \"lemon yellow banana\") AND ripens",
        10,
    );
    ensure!(
        scoped_and_miss.typed_error.is_none(),
        "scoped file:contains(path:src/lib.rs, ...) AND ripens must not error"
    );
    ensure!(
        sorted_candidate_paths(&scoped_and_miss).is_empty(),
        "scoped file:contains(path:src/lib.rs, ...) AND ripens must miss, got {:?}",
        sorted_candidate_paths(&scoped_and_miss),
    );
    Ok(())
}

#[test]
fn scoped_file_content_predicates_execute_under_or_not_and_name_scope() -> AnyResult<()> {
    let mut rt = boot_with_lexical()?;
    for (query, expected) in [
        (
            "file:contains(path:docs/colors.md, \"lemon yellow banana\") OR missing_corpus_token",
            vec!["docs/colors.md"],
        ),
        (
            "file:has.content(path:docs/colors.md, \"lemon yellow banana\") OR missing_corpus_token",
            vec!["docs/colors.md"],
        ),
        (
            "file:contains(name:colors.md, \"lemon yellow banana\")",
            vec!["docs/colors.md"],
        ),
        (
            "file:has.content(name:colors.md, \"lemon yellow banana\")",
            vec!["docs/colors.md"],
        ),
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "query `{query}` must execute, got {:?}",
            result.typed_error
        );
        ensure!(
            sorted_candidate_paths(&result) == expected,
            "query `{query}` must yield {expected:?}, got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    for query in [
        "ripens NOT file:contains(path:docs/colors.md, \"lemon yellow banana\")",
        "ripens NOT file:has.content(path:docs/colors.md, \"lemon yellow banana\")",
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "query `{query}` must execute, got {:?}",
            result.typed_error
        );
        ensure!(
            sorted_candidate_paths(&result).is_empty(),
            "query `{query}` must exclude the scoped file and return empty, got {:?}",
            sorted_candidate_paths(&result),
        );
    }

    for query in [
        "file:contains(\"lemon\", \"banana\")",
        "repo:has.content(path:src, 7)",
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        let Some(error) = result.typed_error else {
            anyhow::bail!(
                "query `{query}` must typed-fail, got {:?}",
                result.candidate_ids
            );
        };
        ensure!(
            error.code == "LEX_PREDICATE_UNIMPLEMENTED",
            "query `{query}` must fail closed on unsupported scoped content shape, got {}",
            error.code
        );
    }
    Ok(())
}

#[test]
fn repo_has_file_predicate_under_or_and_not_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/gate-a.rs) OR missing_corpus_token",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.file OR positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.file(path:src/gate-a.rs) OR missing token must stay on corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );

    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "shared_oracle_needle NOT repo:has.file(path:src/gate-a.rs)",
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:has.file NOT positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&excluded) == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "shared_oracle_needle NOT repo:has.file(path:src/gate-a.rs) must leave only corp-b, got {:?}",
        sorted_candidate_paths(&excluded),
    );
    Ok(())
}

#[test]
fn repo_has_content_phrase_and_raw_string_execute_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    for query in [
        r#"repo:has.content("gate-a only") shared_oracle_needle"#,
        "repo:has.content('gate-a only') shared_oracle_needle",
    ] {
        let admitted = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            admitted.typed_error.is_none(),
            "repo:has.content textual scalar must not error for `{query}`"
        );
        ensure!(
            sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
            "repo:has.content textual scalar must gate to corp-a paths for `{query}`, got {:?}",
            sorted_candidate_paths(&admitted),
        );
    }
    Ok(())
}

#[test]
fn repo_has_content_predicate_under_or_and_not_executes_on_sourcegraph_surface() -> AnyResult<()> {
    let mut rt = boot_with_multi_repo()?;
    let admitted = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r#"repo:has.content("gate-a only") OR missing_corpus_token"#,
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "repo:has.content OR positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.content(\"gate-a only\") OR missing token must stay on corp-a paths, got {:?}",
        sorted_candidate_paths(&admitted),
    );

    let excluded = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        r#"shared_oracle_needle NOT repo:has.content("gate-a only")"#,
        10,
    );
    ensure!(
        excluded.typed_error.is_none(),
        "repo:has.content NOT positive must not error"
    );
    ensure!(
        sorted_candidate_paths(&excluded) == ["lib/gate-a.rs", "src/corp-b.rs", "src/gate-b.py"],
        "shared_oracle_needle NOT repo:has.content(\"gate-a only\") must leave only corp-b, got {:?}",
        sorted_candidate_paths(&excluded),
    );
    Ok(())
}

/// CBOR repo-metadata bundle payload, matching the lexical adapter's expected
/// `{fork, archived, visibility, contexts}` wire map.
fn encode_repo_metadata(fork: bool, archived: bool, contexts: &[&str]) -> AnyResult<Vec<u8>> {
    let mut visibility_payload = Vec::new();
    ciborium::into_writer(&LqVisibility::Public, &mut visibility_payload)
        .map_err(|err| anyhow::anyhow!("encode visibility: {err}"))?;
    let visibility_wire: ciborium::Value = ciborium::from_reader(visibility_payload.as_slice())
        .map_err(|err| anyhow::anyhow!("decode visibility wire: {err}"))?;
    let wire = ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("fork".to_string()),
            ciborium::Value::Bool(fork),
        ),
        (
            ciborium::Value::Text("archived".to_string()),
            ciborium::Value::Bool(archived),
        ),
        (
            ciborium::Value::Text("visibility".to_string()),
            visibility_wire,
        ),
        (
            ciborium::Value::Text("contexts".to_string()),
            ciborium::Value::Array(
                contexts
                    .iter()
                    .map(|ctx| ciborium::Value::Text((*ctx).to_string()))
                    .collect(),
            ),
        ),
    ]);
    let mut payload = Vec::new();
    ciborium::into_writer(&wire, &mut payload)
        .map_err(|err| anyhow::anyhow!("encode repo metadata: {err}"))?;
    Ok(payload)
}

/// Boot a runtime whose single repo is published as not-archived, public, and a
/// member of `bench-ctx`, sealed + active.
fn boot_with_metadata() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text(REPO, "src/lib.rs", "fn parity_needle_alpha() {}\n")?;
    rt.publish_repo_metadata_bundle(encode_repo_metadata(false, false, &["bench-ctx"])?)?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

#[test]
fn archived_filter_admits_non_archived_and_excludes_only() -> AnyResult<()> {
    let mut rt = boot_with_metadata()?;
    let no = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "archived:no parity_needle_alpha",
        10,
    );
    ensure!(no.typed_error.is_none(), "archived:no must not error");
    ensure!(
        no.candidate_ids.len() == 1,
        "archived:no must admit the non-archived repo's doc, got {}",
        no.candidate_ids.len(),
    );
    let only = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "archived:only parity_needle_alpha",
        10,
    );
    ensure!(only.typed_error.is_none(), "archived:only must not error");
    ensure!(
        only.candidate_ids.is_empty(),
        "archived:only must exclude a non-archived repo, got {}",
        only.candidate_ids.len(),
    );
    Ok(())
}

#[test]
fn context_filter_admits_member_context_and_excludes_others() -> AnyResult<()> {
    let mut rt = boot_with_metadata()?;
    let member = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "context:bench-ctx parity_needle_alpha",
        10,
    );
    ensure!(
        member.typed_error.is_none(),
        "context: member must not error"
    );
    ensure!(
        member.candidate_ids.len() == 1,
        "context: matching a member context must admit the doc, got {}",
        member.candidate_ids.len(),
    );
    let other = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "context:absent-ctx parity_needle_alpha",
        10,
    );
    ensure!(
        other.typed_error.is_none(),
        "context: non-member must not error"
    );
    ensure!(
        other.candidate_ids.is_empty(),
        "context: for a non-member context must exclude the doc, got {}",
        other.candidate_ids.len(),
    );
    Ok(())
}

#[test]
fn sourcegraph_legacy_index_and_boost_execute_on_active_stack() -> AnyResult<()> {
    let mut rt = boot_with_lexical()?;
    let baseline = rt.query_text(TextQuerySyntax::Sourcegraph, "parity_needle_alpha", 10);
    ensure!(
        baseline.typed_error.is_none(),
        "baseline lexical query must succeed"
    );

    let index_no = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "index:no parity_needle_alpha",
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
        "boost:5 parity_needle_alpha",
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
