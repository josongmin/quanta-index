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
use quanta_index_contract::{LqVisibility, TextQuerySyntax};

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
    rt.ingest_text(REPO, "docs/colors.md", "the lemon yellow banana ripens\n")?;
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

fn boot_with_multi_repo() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
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
            content: "shared_oracle_needle gate-a only\n",
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("corp-a"),
        }],
    )?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
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
        candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.content(corp-a) must gate to corp-a paths, got {:?}",
        candidate_paths(&admitted),
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
        candidate_paths(&admitted) == ["src/corp-a.rs", "src/gate-a.rs"],
        "repo:has.path(src/gate-a.rs) must gate to corp-a paths, got {:?}",
        candidate_paths(&admitted),
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
        candidate_paths(&admitted) == ["docs/colors.md"],
        "file:contains.content phrase must match the phrase doc, got {:?}",
        candidate_paths(&admitted),
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
