//! E2E-07 — boundedness / fail-closed / observability owner rail.
//!
//! This is not a stopwatch benchmark. The rail exercises the hard runtime
//! shapes that must stay bounded and typed:
//! - regex false positives must be rejected by exact verify,
//! - unsupported regex syntax must fail typed and not poison the next query,
//! - hybrid over-fetch / fuse paths must surface a truthful early-stop reason.

#![forbid(unsafe_code)]

use quanta_index_searchd_harness as e2e_harness;

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    EarlyStopReason, EngineTouched, HistoryIngestBatch, HistoryRefMutation, SearchPlaneTrackKind,
    TextQuerySyntax,
};

use crate::e2e_harness::{
    E2eHistoryFixtureSpec, E2eRuntime, E2eRuntimeCatalogSpec, E2eRuntimeChangedSpec,
    E2eRuntimeEdgeSpec, E2eRuntimeFacetSpec, E2eRuntimeSnapshotSpec, E2eTypedError,
};

fn require_no_typed_error(error: Option<E2eTypedError>, context: &str) -> AnyResult<()> {
    if let Some(error) = error {
        return Err(anyhow::anyhow!(
            "{context}: unexpected typed error code={} message={}",
            error.code,
            error.message
        ));
    }
    Ok(())
}

fn assert_closed_metric_suffix(
    rt: &E2eRuntime,
    expected_suffix: &[&str],
    allowed: &[&str],
    leaked_terms: &[&str],
) -> AnyResult<()> {
    let errors = rt.query_metric_errors()?;
    if !errors.is_empty() {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric errors: {errors:?}"
        ));
    }
    let samples = rt.query_metrics_snapshot()?;
    let names = samples
        .iter()
        .map(|sample| sample.name.as_ref().to_string())
        .collect::<Vec<_>>();
    if names.iter().any(|name| !allowed.contains(&name.as_str())) {
        return Err(anyhow::anyhow!(
            "runtime metric names escaped closed set: {names:?}"
        ));
    }
    let suffix = names
        .get(names.len().saturating_sub(expected_suffix.len())..)
        .unwrap_or_default()
        .to_vec();
    let expected = expected_suffix
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if suffix != expected {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric suffix: names={names:?} expected_suffix={expected:?}"
        ));
    }
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-e2e"
            || sample.dimensions.generation_id != 1
        {
            return Err(anyhow::anyhow!(
                "unexpected runtime metric dimensions: {:?}",
                sample.dimensions
            ));
        }
        for leaked in leaked_terms {
            if sample.name.contains(leaked) {
                return Err(anyhow::anyhow!(
                    "runtime metric leaked query content `{leaked}` in name={}",
                    sample.name
                ));
            }
        }
    }
    Ok(())
}

fn seed_regex_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/exact.txt", "needle_x exact")?;
    rt.ingest_text("repo-e2e", "src/bait.txt", "needle_xx bait")?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_hybrid_count_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/alpha.rs", "scope alpha keep")?;
    rt.ingest_text("repo-e2e", "src/beta.rs", "scope beta keep")?;
    rt.ingest_text("repo-e2e", "src/gamma.rs", "scope gamma keep")?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(())
}

fn seed_hybrid_tie_fixture(rt: &mut E2eRuntime, count: usize) -> AnyResult<()> {
    for idx in 0..count {
        let path = format!("src/tie-{idx:02}.rs");
        rt.ingest_text("repo-e2e", &path, "scope tie keep")?;
    }
    _ = rt.seal()?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(())
}

fn ingest_structural_trait_tree(rt: &mut E2eRuntime, path: &str, content: &str) -> AnyResult<()> {
    use quanta_index_contract::lex::compute_parse_tree_source_hash;
    use quanta_index_contract::lex::{LanguageCode, ParseNode, ParseTreeRecord};

    let identifier = "chaos_structural_trait";
    let identifier_start = content.find(identifier).ok_or_else(|| {
        anyhow::anyhow!("e2e chaos trait identifier `{identifier}` missing from `{content}`")
    })?;
    let identifier_end = identifier_start.saturating_add(identifier.len());
    let byte_end = u32::try_from(content.len())
        .map_err(|err| anyhow::anyhow!("structural trait content overflow: {err}"))?;
    let identifier_start = u32::try_from(identifier_start)
        .map_err(|err| anyhow::anyhow!("structural trait identifier start overflow: {err}"))?;
    let identifier_end = u32::try_from(identifier_end)
        .map_err(|err| anyhow::anyhow!("structural trait identifier end overflow: {err}"))?;
    let block_start = byte_end.saturating_sub(2);
    let tree = ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust")
            .map_err(|err| anyhow::anyhow!("invalid rust lang code: {err}"))?,
        root: ParseNode {
            kind: "trait_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end,
            children: vec![
                ParseNode {
                    kind: "identifier".to_string().into_boxed_str(),
                    byte_start: identifier_start,
                    byte_end: identifier_end,
                    children: Vec::new(),
                },
                ParseNode {
                    kind: "block".to_string().into_boxed_str(),
                    byte_start: block_start,
                    byte_end,
                    children: Vec::new(),
                },
            ],
        },
        source_hash: compute_parse_tree_source_hash(content),
        role_tag_schema_version: 1,
        role_tags: Vec::new(),
    };
    rt.ingest_structural_tree(path, tree)
}

fn seed_structural_pure_negative_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/structural.rs";
    let content = "fn chaos_structural_alpha() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "chaos_structural_alpha")?;
    _ = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    Ok(())
}

fn seed_structural_boolean_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let fn_path = "src/structural.rs";
    let fn_content = "fn chaos_structural_alpha() {}";
    rt.ingest_text("repo-e2e", fn_path, fn_content)?;
    rt.ingest_structural_function_tree(fn_path, fn_content, "chaos_structural_alpha")?;

    let trait_path = "src/structural_trait.rs";
    let trait_content = "trait chaos_structural_trait {}";
    rt.ingest_text("repo-e2e", trait_path, trait_content)?;
    ingest_structural_trait_tree(rt, trait_path, trait_content)?;

    _ = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    Ok(())
}

fn seed_history_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/history.rs";
    rt.ingest_text("repo-e2e", path, "history lexical proof")?;
    rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
        commit_sha: "0123456789abcdef0123456789abcdef01234567",
        file_path: path,
        author: "alice",
        committer: "alice",
        message: "fix: sample history alpha_content_needle",
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        ref_name: "refs/heads/main",
        tag_name: "v1.0.0",
        added_text: "history added line",
        removed_text: "history removed line",
        touched_text: "history touched line",
    })?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn history_partial_shard_batch(
    rt: &E2eRuntime,
    file_path: &str,
    include_ref: bool,
    include_tag: bool,
) -> HistoryIngestBatch {
    use quanta_index_contract::lex::{CommitRecord, CommitSha};

    let commit_sha = CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ]);
    HistoryIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        manifest_digest: Some(format!(
            "history-partial:{}:{}",
            file_path,
            rt.current_generation().get()
        )),
        batch_digest: format!(
            "history-partial-batch:{file_path}:{}",
            rt.current_generation().get()
        ),
        commits: vec![CommitRecord {
            wire_version: 1,
            sha: commit_sha,
            parents: Vec::new(),
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            author: "alice".to_string().into_boxed_str(),
            committer: "alice".to_string().into_boxed_str(),
            message: "fix: sample history".to_string().into_boxed_str(),
            is_merge: false,
            tags: vec!["v1.0.0".to_string().into_boxed_str()],
        }],
        refs: if include_ref {
            vec![HistoryRefMutation::Upsert(
                quanta_index_contract::HistoryRefUpsert {
                    name: "refs/heads/main".to_string().into_boxed_str(),
                    sha: commit_sha,
                },
            )]
        } else {
            Vec::new()
        },
        tags: if include_tag {
            vec![HistoryRefMutation::Upsert(
                quanta_index_contract::HistoryRefUpsert {
                    name: "v1.0.0".to_string().into_boxed_str(),
                    sha: commit_sha,
                },
            )]
        } else {
            Vec::new()
        },
        diff_hunks: Vec::new(),
    }
}

fn seed_history_partial_shard_fixture(
    rt: &mut E2eRuntime,
    include_ref: bool,
    include_tag: bool,
) -> AnyResult<()> {
    let path = "src/history.rs";
    rt.ingest_text("repo-e2e", path, "history lexical proof")?;
    rt.publish_history_batch(history_partial_shard_batch(
        rt,
        path,
        include_ref,
        include_tag,
    ))?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_runtime_dirty_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let dirty_path = "src/dirty.rs";
    rt.ingest_text("repo-e2e", dirty_path, "todo dirty scope")?;
    rt.ingest_text("repo-e2e", "src/clean.rs", "todo clean scope")?;
    rt.ingest_dirty_for_path(dirty_path, 100)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_runtime_catalog_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    for (path, content) in [
        ("src/changed.rs", "fn catalog_changed_needle() {}"),
        ("src/changed-other.rs", "fn catalog_changed_needle() {}"),
        ("src/unchanged.rs", "fn catalog_unchanged_needle() {}"),
        ("src/owner.rs", "fn catalog_owner_needle() {}"),
        ("src/owner-other.rs", "fn catalog_owner_needle() {}"),
        ("src/service.rs", "fn catalog_service_needle() {}"),
        ("src/service-other.rs", "fn catalog_service_needle() {}"),
        ("src/layer.rs", "fn catalog_layer_needle() {}"),
        ("src/layer-other.rs", "fn catalog_layer_needle() {}"),
        ("src/surface.rs", "fn catalog_surface_needle() {}"),
        ("src/surface-other.rs", "fn catalog_surface_needle() {}"),
        ("src/snap.rs", "fn catalog_snapshot_needle() {}"),
        ("src/snap-other.rs", "fn catalog_snapshot_needle() {}"),
        ("src/stale.rs", "fn catalog_stale_needle() {}"),
    ] {
        rt.ingest_text("repo-e2e", path, content)?;
    }
    rt.ingest_runtime_catalog(&E2eRuntimeCatalogSpec {
        producer_head_applied_at_ms: 100,
        generation_materialized_at_ms: 20,
        changed: vec![E2eRuntimeChangedSpec {
            path: "src/changed.rs".to_string(),
            applied_at_ms: 25,
        }],
        facets: vec![
            E2eRuntimeFacetSpec {
                path: "src/owner.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/owner-other.rs".to_string(),
                owner: Some("team-b".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/service.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/service-other.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("build".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/layer.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/layer-other.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("query".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/surface.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/surface-other.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("semantic".to_string()),
            },
        ],
        snapshots: vec![E2eRuntimeSnapshotSpec {
            name: "active".to_string(),
            paths: vec!["src/changed.rs".to_string(), "src/snap.rs".to_string()],
        }],
        affected: vec![E2eRuntimeEdgeSpec {
            key: "rebuild=lexical".to_string(),
            paths: vec!["src/changed.rs".to_string()],
        }],
        invalidated_by: vec![E2eRuntimeEdgeSpec {
            key: "rebuild=lexical".to_string(),
            paths: vec!["src/changed.rs".to_string()],
        }],
    })?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_predicate_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text(
        "repo-e2e",
        "src/file_contains.rs",
        "foo oo_ba filecontainsmarker",
    )?;
    rt.ingest_text("repo-e2e", "src/lib.rs", "needle alpha")?;
    rt.ingest_text("repo-e2e", "src/main.rs", "needle beta")?;
    rt.ingest_text("repo-e2e", "docs/readme.md", "other text")?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn assert_success_runtime_metadata_metrics(
    rt: &E2eRuntime,
    leaked_terms: &[&str],
) -> AnyResult<()> {
    assert_closed_metric_suffix(
        rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_typed_error_unavailable_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        leaked_terms,
    )
}

fn oversized_raw_substring_query() -> String {
    "abcdefghijklmnopq".repeat(1024)
}

fn trigram_limit_raw_substring_query() -> String {
    trigram_plan_limit_raw_substring_query()
}

fn trigram_plan_limit_raw_substring_query() -> String {
    let alphabet = *b"abcdefghijklmnopq";
    let mut out = String::new();
    for a in alphabet {
        for b in alphabet {
            for c in alphabet {
                out.push(char::from(a));
                out.push(char::from(b));
                out.push(char::from(c));
            }
        }
    }
    out
}

#[test]
fn regex_false_positive_candidate_is_rejected_by_exact_verify() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let bait_id = rt.candidate_id_for_path("src/bait.txt")?;

    let result = rt.query_text(TextQuerySyntax::Native, "/needle_x\\b/", 10);
    require_no_typed_error(result.typed_error, "regex exact-verify query")?;
    if result.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "expected only exact regex hit, got {:?}",
            result.candidate_ids
        ));
    }
    if result.candidate_ids.contains(&bait_id) {
        return Err(anyhow::anyhow!(
            "regex verify leaked trigram false positive {bait_id}"
        ));
    }
    Ok(())
}

#[test]
fn regex_typed_rejection_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let invalid = rt.query_text(TextQuerySyntax::Native, "/(?<=needle_)x/", 10);
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed regex rejection"))?;
    if error.code != "PARSE_FAIL" {
        return Err(anyhow::anyhow!("expected PARSE_FAIL, got {}", error.code));
    }
    if !error.message.contains("regex") {
        return Err(anyhow::anyhow!(
            "regex rejection lost fail-closed regex detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after regex reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after regex reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn regex_timeout_is_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let timed_out = rt.query_text(TextQuerySyntax::Native, "timeout:0ms /needle_x\\b/", 10);
    let error = timed_out
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed regex timeout"))?;
    if error.code != "QUERY_TIMEOUT" {
        return Err(anyhow::anyhow!(
            "expected QUERY_TIMEOUT, got {}",
            error.code
        ));
    }
    if !error.message.contains("timed out") {
        return Err(anyhow::anyhow!(
            "regex timeout lost timeout detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after regex timeout",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after regex timeout: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn oversized_raw_substring_query_fails_parse_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", oversized_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected oversized raw-substring parse rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "PARSE_FAIL" {
        return Err(anyhow::anyhow!("expected PARSE_FAIL, got {}", error.code));
    }
    if !error.message.contains("16 KiB cap") {
        return Err(anyhow::anyhow!(
            "oversized raw-substring rejection lost parser byte-cap detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after oversized raw-substring reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after oversized raw-substring reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn raw_substring_trigram_plan_limit_is_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", trigram_plan_limit_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected trigram plan-limit rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED" {
        return Err(anyhow::anyhow!(
            "expected LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED, got {}",
            error.code
        ));
    }
    if !error.message.contains("distinct trigrams") {
        return Err(anyhow::anyhow!(
            "trigram plan-limit rejection lost distinct-trigram detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after trigram plan-limit reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after trigram plan-limit reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn trigram_plan_limit_query_fails_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", trigram_limit_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected trigram plan-limit rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED" {
        return Err(anyhow::anyhow!(
            "expected LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED, got {}",
            error.code
        ));
    }
    if !error.message.contains("dimension=trigram-set") {
        return Err(anyhow::anyhow!(
            "trigram plan-limit rejection lost dimension detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after trigram plan-limit reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after trigram plan-limit reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn hybrid_count_cap_surfaces_truthful_early_stop_reason() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_count_fixture(&mut rt)?;

    let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 2);
    require_no_typed_error(result.typed_error, "hybrid count-cap query")?;
    if result.candidate_ids.len() != 2 {
        return Err(anyhow::anyhow!(
            "expected exactly 2 hybrid results after top_k cap, got {:?}",
            result.candidate_ids
        ));
    }
    let explanation = result
        .explanation
        .ok_or_else(|| anyhow::anyhow!("hybrid count-cap query returned no explanation"))?;
    if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err(anyhow::anyhow!(
            "expected CountReached, got {:?}",
            explanation.early_stop_reason
        ));
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(anyhow::anyhow!(
            "unexpected hybrid count-cap engines_touched: {:?}",
            explanation.engines_touched
        ));
    }
    if explanation.summary != "hybrid fused 3 lexical and 3 semantic candidates into 2 results" {
        return Err(anyhow::anyhow!(
            "hybrid count-cap summary missing fused-count detail: {}",
            explanation.summary
        ));
    }
    Ok(())
}

#[test]
fn hybrid_runtime_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_count_fixture(&mut rt)?;

    let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 2);
    require_no_typed_error(result.typed_error, "hybrid runtime metrics query")?;

    let errors = rt.query_metric_errors()?;
    if !errors.is_empty() {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric errors: {errors:?}"
        ));
    }
    let samples = rt.query_metrics_snapshot()?;
    let names = samples
        .iter()
        .map(|sample| sample.name.as_ref().to_string())
        .collect::<Vec<_>>();
    let expected_success_suffix = vec![
        "lq_query_intake_total".to_string(),
        "lq_planner_total".to_string(),
        "lq_engine_fanout_count".to_string(),
        "lq_merge_result_count".to_string(),
        "lq_early_stop_total".to_string(),
    ];
    let allowed = [
        "lq_query_intake_total",
        "lq_typed_error_not_ready_total",
        "lq_planner_total",
        "lq_engine_fanout_count",
        "lq_merge_result_count",
        "lq_early_stop_total",
    ];
    if names.iter().any(|name| !allowed.contains(&name.as_str())) {
        return Err(anyhow::anyhow!(
            "runtime metric names escaped closed set: {names:?}"
        ));
    }
    let success_suffix = names
        .get(names.len().saturating_sub(expected_success_suffix.len())..)
        .unwrap_or_default()
        .to_vec();
    if success_suffix != expected_success_suffix {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric names: {names:?}"
        ));
    }
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-e2e"
            || sample.dimensions.generation_id != 1
        {
            return Err(anyhow::anyhow!(
                "unexpected runtime metric dimensions: {:?}",
                sample.dimensions
            ));
        }
        if sample.name.contains("scope") || sample.name.contains("1.0 0.0") {
            return Err(anyhow::anyhow!(
                "runtime metric leaked query content in name={}",
                sample.name
            ));
        }
    }
    Ok(())
}

#[test]
fn history_runtime_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let result = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
    require_no_typed_error(result.typed_error, "history metrics query")?;
    if result.commit_ids.len() != 1 || !result.diff_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "unexpected history query result commit_ids={:?} diff_paths={:?}",
            result.commit_ids,
            result.diff_paths
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_other_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["fix", "history", "alice"],
    )
}

#[test]
fn runtime_metadata_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_dirty_fixture(&mut rt)?;

    let result = rt.query_runtime_metadata(TextQuerySyntax::Native, "dirty:yes todo", 10);
    require_no_typed_error(result.typed_error, "runtime metadata metrics query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "unexpected runtime metadata candidate ids: {:?}",
            result.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["dirty", "todo", "src/dirty.rs"],
    )
}

#[test]
fn structural_runtime_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let result = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(result.typed_error, "structural metrics query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "unexpected structural candidate ids: {:?}",
            result.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["function_item", "name.expr", "chaos_structural_alpha"],
    )
}

#[test]
fn lexical_timeout_runtime_metrics_use_plan_limit_bucket_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let result = rt.query_text(TextQuerySyntax::Native, "timeout:0ms /needle_x\\b/", 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected lexical timeout typed error"))?;
    if error.code != "QUERY_TIMEOUT" {
        return Err(anyhow::anyhow!(
            "expected QUERY_TIMEOUT, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_plan_limit_total"],
        &["lq_query_intake_total", "lq_typed_error_plan_limit_total"],
        &["needle_x", "timeout:0ms", "regex"],
    )
}

#[test]
fn history_missing_ref_shard_uses_closed_unavailable_metric() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_partial_shard_fixture(&mut rt, false, false)?;

    let result = rt.query_history(
        TextQuerySyntax::Sourcegraph,
        "type:commit rev:refs/heads/main fix",
        10,
    );
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected history ref-shard typed error"))?;
    if error.code != "HISTORY_SHARD_UNAVAILABLE" {
        return Err(anyhow::anyhow!(
            "expected HISTORY_SHARD_UNAVAILABLE, got {}",
            error.code
        ));
    }
    if !error.message.contains("ref shard is unavailable") {
        return Err(anyhow::anyhow!(
            "unexpected history ref-shard message: {}",
            error.message
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["refs/heads/main", "fix", "history"],
    )
}

#[test]
fn history_missing_tag_shard_uses_closed_unavailable_metric() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_partial_shard_fixture(&mut rt, true, false)?;

    let result = rt.query_history(
        TextQuerySyntax::Sourcegraph,
        "type:commit rev:v1.0.0 fix",
        10,
    );
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected history tag-shard typed error"))?;
    if error.code != "HISTORY_SHARD_UNAVAILABLE" {
        return Err(anyhow::anyhow!(
            "expected HISTORY_SHARD_UNAVAILABLE, got {}",
            error.code
        ));
    }
    if !error.message.contains("tag shard is unavailable") {
        return Err(anyhow::anyhow!(
            "unexpected history tag-shard message: {}",
            error.message
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["v1.0.0", "fix", "history"],
    )
}

#[test]
fn hybrid_large_tied_result_set_keeps_order_stable() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_tie_fixture(&mut rt, 24)?;
    let mut baseline: Option<Vec<String>> = None;

    for run in 0..5 {
        let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 10);
        require_no_typed_error(result.typed_error, "hybrid tied-order query")?;
        if result.candidate_ids.len() != 10 {
            return Err(anyhow::anyhow!(
                "hybrid tied-order query returned {} results on run {run}, expected 10",
                result.candidate_ids.len()
            ));
        }
        let explanation = result
            .explanation
            .ok_or_else(|| anyhow::anyhow!("hybrid tied-order query returned no explanation"))?;
        if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
            return Err(anyhow::anyhow!(
                "expected CountReached on hybrid tied-order query, got {:?}",
                explanation.early_stop_reason
            ));
        }
        match baseline.as_ref() {
            Some(previous) if previous != &result.candidate_ids => {
                return Err(anyhow::anyhow!(
                    "hybrid tied-order query drifted across runs: baseline={previous:?} current={:?}",
                    result.candidate_ids
                ));
            }
            None => baseline = Some(result.candidate_ids),
            Some(_) => {}
        }
    }
    Ok(())
}

#[test]
fn structural_missing_parse_tree_fails_typed_generation_not_ready() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-e2e", "src/tree.rs", "fn orphaned() {}")?;
    _ = rt.seal_lexical_generation_for_tracks(&[SearchPlaneTrackKind::Lexical])?;
    rt.activate_last_sealed_generation_with_tracks(&[SearchPlaneTrackKind::Lexical])?;

    let result = rt.query_structural(TextQuerySyntax::Native, "match { :[x] }", 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural readiness error"))?;
    if error.code != "STR_GENERATION_NOT_READY" {
        return Err(anyhow::anyhow!(
            "expected STR_GENERATION_NOT_READY, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_not_ready_total"],
        &["lq_query_intake_total", "lq_typed_error_not_ready_total"],
        &["match", "tree.rs"],
    )
}

#[test]
fn structural_orphan_chunk_authority_fails_typed_shard_unavailable() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/tree.rs";
    let content = "fn orphaned() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "orphaned")?;
    rt.delete_chunk_for_path(path)?;
    _ = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;

    let result = rt.query_structural(TextQuerySyntax::Native, "match { :[x] }", 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural shard-unavailable error"))?;
    if error.code != "STR_SHARD_UNAVAILABLE" {
        return Err(anyhow::anyhow!(
            "expected STR_SHARD_UNAVAILABLE, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["match", "tree.rs"],
    )
}

#[test]
fn structural_mixed_lexical_boolean_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let mixed = rt.query_structural(
        TextQuerySyntax::Native,
        "chaos_structural_alpha AND match { function_item }",
        10,
    );
    require_no_typed_error(
        mixed.typed_error,
        "mixed lexical/structural boolean execution",
    )?;
    if mixed.candidate_ids != vec![expected_id.clone()] {
        return Err(anyhow::anyhow!(
            "mixed lexical/structural boolean diverged: {:?}",
            mixed.candidate_ids
        ));
    }

    let follow_up = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after mixed lexical/structural execution",
    )?;
    if follow_up.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after mixed lexical/structural execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "chaos_structural_alpha",
            "function_item",
            "name.expr",
            "trait_item",
        ],
    )
}

#[test]
fn structural_mixed_lexical_or_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let fn_id = rt.candidate_id_for_path("src/structural.rs")?;
    let trait_id = rt.candidate_id_for_path("src/structural_trait.rs")?;
    let mixed_or = rt.query_structural(
        TextQuerySyntax::Native,
        "chaos_structural_alpha OR match { trait_item }",
        10,
    );
    require_no_typed_error(
        mixed_or.typed_error,
        "mixed lexical/structural OR execution",
    )?;
    let mut observed = mixed_or.candidate_ids;
    observed.sort();
    let mut expected = vec![fn_id.clone(), trait_id];
    expected.sort();
    if observed != expected {
        return Err(anyhow::anyhow!(
            "mixed lexical/structural OR diverged: observed={observed:?} expected={expected:?}"
        ));
    }

    let follow_up = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after mixed OR execution",
    )?;
    if follow_up.candidate_ids != vec![fn_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after mixed OR execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn structural_mixed_lexical_and_not_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let fn_id = rt.candidate_id_for_path("src/structural.rs")?;
    let mixed_and_not = rt.query_structural(
        TextQuerySyntax::Native,
        "chaos_structural_alpha AND NOT match { trait_item }",
        10,
    );
    require_no_typed_error(
        mixed_and_not.typed_error,
        "mixed lexical/structural AND NOT execution",
    )?;
    if mixed_and_not.candidate_ids != vec![fn_id.clone()] {
        return Err(anyhow::anyhow!(
            "mixed lexical/structural AND NOT diverged: {:?}",
            mixed_and_not.candidate_ids
        ));
    }

    let follow_up = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after mixed AND NOT execution",
    )?;
    if follow_up.candidate_ids != vec![fn_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after mixed AND NOT execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn structural_pure_negative_boolean_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_pure_negative_fixture(&mut rt)?;

    let pure_negative =
        rt.query_structural(TextQuerySyntax::Native, "NOT match { function_item }", 10);
    require_no_typed_error(
        pure_negative.typed_error,
        "pure-negative structural root execution",
    )?;
    if !pure_negative.candidate_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "single-function structural fixture should yield empty pure-negative survivors, got {:?}",
            pure_negative.candidate_ids
        ));
    }

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let follow_up = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after pure-negative execution",
    )?;
    if follow_up.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after pure-negative execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["function_item", "name.expr", "NOT match"],
    )
}

#[test]
fn structural_typed_hole_kind_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let invalid = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.lambda] } } }",
        10,
    );
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural typed-hole rejection"))?;
    if error.code != "STR_HOLE_KIND_UNSUPPORTED" {
        return Err(anyhow::anyhow!(
            "expected STR_HOLE_KIND_UNSUPPORTED, got {}",
            error.code
        ));
    }
    if !error.message.contains("typed hole kind `lambda`") {
        return Err(anyhow::anyhow!(
            "typed-hole rejection lost exact unsupported-kind detail: {}",
            error.message
        ));
    }

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let follow_up = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { :[name.expr] } }""#,
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after typed-hole reject",
    )?;
    if follow_up.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after typed-hole reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn runtime_catalog_changed_executes_and_metrics_are_bounded() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let result = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        10,
    );
    require_no_typed_error(result.typed_error, "runtime catalog changed query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog changed query diverged: {:?}",
            result.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(
        &rt,
        &["catalog_changed_needle", "1970-01-01", "src/changed.rs"],
    )
}

#[test]
fn runtime_catalog_stale_executes_and_metrics_are_bounded() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let result = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs catalog_stale_needle",
        10,
    );
    require_no_typed_error(result.typed_error, "runtime catalog stale query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog stale query diverged: {:?}",
            result.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(
        &rt,
        &["catalog_stale_needle", "1970-01-01", "src/stale.rs"],
    )
}

#[test]
fn runtime_catalog_snapshot_executes_and_metrics_are_bounded() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let result = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "snapshot:active file:src/snap.rs catalog_snapshot_needle",
        10,
    );
    require_no_typed_error(result.typed_error, "runtime catalog snapshot query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog snapshot query diverged: {:?}",
            result.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(
        &rt,
        &["catalog_snapshot_needle", "snapshot:active", "src/snap.rs"],
    )
}

#[test]
fn runtime_catalog_without_authority_fails_typed_and_metrics_are_bounded() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/changed.rs";
    rt.ingest_text("repo-e2e", path, "fn catalog_changed_needle() {}")?;
    rt.ingest_dirty_for_path(path, 100)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let result = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        10,
    );
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected RUNTIME_CATALOG_NOT_READY typed error"))?;
    if error.code != "RUNTIME_CATALOG_NOT_READY" {
        return Err(anyhow::anyhow!(
            "expected RUNTIME_CATALOG_NOT_READY, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_not_ready_total"],
        &["lq_query_intake_total", "lq_typed_error_not_ready_total"],
        &["catalog_changed_needle", "1970-01-01", "src/changed.rs"],
    )
}

#[test]
fn runtime_catalog_dirty_no_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_dirty_fixture(&mut rt)?;

    let clean = rt.query_runtime_metadata(TextQuerySyntax::Sourcegraph, "dirty:no todo", 10);
    require_no_typed_error(clean.typed_error, "dirty:no query")?;
    if clean.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "dirty:no query expected exactly one clean candidate, got {:?}",
            clean.candidate_ids
        ));
    }

    let follow_up = rt.query_runtime_metadata(TextQuerySyntax::Native, "dirty:yes todo", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime metadata query after dirty:no execution",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime metadata query diverged after dirty:no execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(&rt, &["dirty", "todo", "src/dirty.rs"])
}

#[test]
fn runtime_catalog_dirty_only_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_dirty_fixture(&mut rt)?;

    let rejected = rt.query_runtime_metadata(TextQuerySyntax::Sourcegraph, "dirty:only todo", 10);
    let error = rejected.typed_error.ok_or_else(|| {
        anyhow::anyhow!("expected RUNTIME_DIRTY_ONLY_UNSUPPORTED typed rejection")
    })?;
    if error.code != "RUNTIME_DIRTY_ONLY_UNSUPPORTED" {
        return Err(anyhow::anyhow!(
            "expected RUNTIME_DIRTY_ONLY_UNSUPPORTED, got {}",
            error.code
        ));
    }

    let follow_up = rt.query_runtime_metadata(TextQuerySyntax::Native, "dirty:yes todo", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime metadata query after dirty:only reject",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime metadata query diverged after dirty:only reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_invalid_request_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["dirty:only", "todo"],
    )
}

#[test]
fn runtime_catalog_affected_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let affected = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "affected:rebuild=lexical catalog_changed_needle",
        10,
    );
    require_no_typed_error(affected.typed_error, "affected query")?;
    if affected.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "affected query expected exactly one candidate, got {:?}",
            affected.candidate_ids
        ));
    }

    let follow_up = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime catalog query after affected: execution",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime catalog query diverged after affected: execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(
        &rt,
        &[
            "affected:rebuild=lexical",
            "catalog_changed_needle",
            "src/changed.rs",
        ],
    )
}

#[test]
fn runtime_catalog_invalidated_by_executes_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let invalidated = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "invalidated_by:rebuild=lexical catalog_changed_needle",
        10,
    );
    require_no_typed_error(invalidated.typed_error, "invalidated_by query")?;
    if invalidated.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "invalidated_by query expected exactly one candidate, got {:?}",
            invalidated.candidate_ids
        ));
    }

    let follow_up = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime catalog query after invalidated_by: execution",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime catalog query diverged after invalidated_by: execution: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(
        &rt,
        &[
            "invalidated_by:rebuild=lexical",
            "catalog_changed_needle",
            "src/changed.rs",
        ],
    )
}

#[test]
fn runtime_catalog_snapshot_unknown_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()>
{
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let rejected = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "snapshot:missing file:src/snap.rs catalog_snapshot_needle",
        10,
    );
    let error = rejected
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected SNAPSHOT_UNKNOWN typed rejection"))?;
    if error.code != "SNAPSHOT_UNKNOWN" {
        return Err(anyhow::anyhow!(
            "expected SNAPSHOT_UNKNOWN, got {}",
            error.code
        ));
    }

    let follow_up = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "snapshot:active file:src/snap.rs catalog_snapshot_needle",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime catalog query after snapshot:missing reject",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime catalog query diverged after snapshot:missing reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_typed_error_other_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["snapshot:missing", "catalog_snapshot_needle", "src/snap.rs"],
    )
}

#[test]
fn predicate_file_contains_executes_and_miss_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_predicate_fixture(&mut rt)?;

    let expected_id = rt.candidate_id_for_path("src/file_contains.rs")?;
    let hit = rt.query_text(TextQuerySyntax::Native, "file.contains('oo_ba')", 10);
    require_no_typed_error(hit.typed_error, "file.contains hit query")?;
    if hit.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "file.contains hit query diverged: {:?}",
            hit.candidate_ids
        ));
    }

    let miss = rt.query_text(
        TextQuerySyntax::Native,
        "file.contains(\"banana lemon\")",
        10,
    );
    require_no_typed_error(miss.typed_error, "file.contains miss query")?;
    if !miss.candidate_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "file.contains miss query expected zero candidates, got {:?}",
            miss.candidate_ids
        ));
    }

    let alpha_id = rt.candidate_id_for_path("src/lib.rs")?;
    let beta_id = rt.candidate_id_for_path("src/main.rs")?;
    let follow_up = rt.query_text(TextQuerySyntax::Sourcegraph, "needle", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after file.contains miss",
    )?;
    if follow_up.candidate_ids != vec![alpha_id, beta_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after file.contains miss: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
        ],
        &["oo_ba", "banana lemon", "needle"],
    )
}

#[test]
fn predicate_repo_has_file_executes_and_miss_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_predicate_fixture(&mut rt)?;

    let alpha_id = rt.candidate_id_for_path("src/lib.rs")?;
    let beta_id = rt.candidate_id_for_path("src/main.rs")?;

    let hit = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/lib.rs) needle",
        10,
    );
    require_no_typed_error(hit.typed_error, "repo.has.file hit query")?;
    if hit.candidate_ids != vec![alpha_id.clone(), beta_id.clone()] {
        return Err(anyhow::anyhow!(
            "repo.has.file hit query diverged: {:?}",
            hit.candidate_ids
        ));
    }

    let miss = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:missing.rs) needle",
        10,
    );
    require_no_typed_error(miss.typed_error, "repo.has.file miss query")?;
    if !miss.candidate_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "repo.has.file miss query expected zero candidates, got {:?}",
            miss.candidate_ids
        ));
    }

    let follow_up = rt.query_text(TextQuerySyntax::Sourcegraph, "needle", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after repo.has.file miss",
    )?;
    if follow_up.candidate_ids != vec![alpha_id, beta_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after repo.has.file miss: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
        ],
        &["repo:has.file", "missing.rs", "needle"],
    )
}

#[test]
fn runtime_catalog_meta_facets_execute_and_do_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    for (label, query_text, follow_up_query, leaked_terms) in [
        (
            "meta.owner",
            "meta.owner:team-a catalog_owner_needle",
            "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
            vec!["meta.owner:team-a", "catalog_owner_needle"],
        ),
        (
            "meta.service",
            "meta.service:search catalog_service_needle",
            "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
            vec!["meta.service:search", "catalog_service_needle"],
        ),
        (
            "meta.layer",
            "meta.layer:index catalog_layer_needle",
            "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
            vec!["meta.layer:index", "catalog_layer_needle"],
        ),
        (
            "meta.surface",
            "meta.surface:lexical catalog_surface_needle",
            "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
            vec!["meta.surface:lexical", "catalog_surface_needle"],
        ),
    ] {
        let result = rt.query_runtime_metadata(TextQuerySyntax::Sourcegraph, query_text, 10);
        require_no_typed_error(result.typed_error, label)?;
        if result.candidate_ids.len() != 1 {
            return Err(anyhow::anyhow!(
                "{label} query expected exactly one candidate, got {:?}",
                result.candidate_ids
            ));
        }

        let follow_up =
            rt.query_runtime_metadata(TextQuerySyntax::Sourcegraph, follow_up_query, 10);
        require_no_typed_error(
            follow_up.typed_error,
            &format!("follow-up runtime catalog query after {label} execution"),
        )?;
        if follow_up.candidate_ids.len() != 1 {
            return Err(anyhow::anyhow!(
                "follow-up runtime catalog query diverged after {label} execution: {:?}",
                follow_up.candidate_ids
            ));
        }
        assert_success_runtime_metadata_metrics(&rt, leaked_terms.as_slice())?;
    }
    Ok(())
}

#[test]
fn runtime_catalog_stale_miss_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let miss = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "stale:before=1970-01-01T00:00:00.010Z file:src/stale.rs catalog_stale_needle",
        10,
    );
    require_no_typed_error(miss.typed_error, "runtime catalog stale miss query")?;
    if !miss.candidate_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "runtime catalog stale miss query expected zero candidates, got {:?}",
            miss.candidate_ids
        ));
    }

    let follow_up = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs catalog_stale_needle",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime catalog query after stale miss",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime catalog query diverged after stale miss: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_success_runtime_metadata_metrics(
        &rt,
        &["catalog_stale_needle", "1970-01-01", "src/stale.rs"],
    )
}

#[test]
fn history_missing_type_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let rejected = rt.query_history(TextQuerySyntax::Native, "fix", 10);
    let error = rejected
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected INVALID_REQUEST for missing history type"))?;
    if error.code != "INVALID_REQUEST" {
        return Err(anyhow::anyhow!(
            "expected INVALID_REQUEST, got {}",
            error.code
        ));
    }
    if !error
        .message
        .contains("explicit `type:commit` or `type:diff` is required")
    {
        return Err(anyhow::anyhow!(
            "history missing-type rejection lost detail: {}",
            error.message
        ));
    }

    let follow_up = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up history query after missing type reject",
    )?;
    if follow_up.commit_ids.len() != 1 || !follow_up.diff_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "follow-up history query diverged after missing type reject: commit_ids={:?} diff_paths={:?}",
            follow_up.commit_ids,
            follow_up.diff_paths
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_invalid_request_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["fix", "type:commit"],
    )
}

#[test]
fn history_commit_file_filter_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let rejected = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit file:src/history.rs fix",
        10,
    );
    let error = rejected.typed_error.ok_or_else(|| {
        anyhow::anyhow!("expected INVALID_REQUEST for commit history file filter")
    })?;
    if error.code != "INVALID_REQUEST" {
        return Err(anyhow::anyhow!(
            "expected INVALID_REQUEST, got {}",
            error.code
        ));
    }
    if !error
        .message
        .contains("`file:` and `diff.*` filters require `type:diff`")
    {
        return Err(anyhow::anyhow!(
            "history commit-file rejection lost detail: {}",
            error.message
        ));
    }

    let follow_up = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up history query after commit file reject",
    )?;
    if follow_up.commit_ids.len() != 1 || !follow_up.diff_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "follow-up history query diverged after commit file reject: commit_ids={:?} diff_paths={:?}",
            follow_up.commit_ids,
            follow_up.diff_paths
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_invalid_request_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["type:commit", "file:src/history.rs", "fix"],
    )
}

#[test]
fn history_predicate_leaf_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let rejected = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit file.contains('fix')",
        10,
    );
    let error = rejected
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected NOT_IMPLEMENTED for history predicate leaf"))?;
    if error.code != "NOT_IMPLEMENTED" {
        return Err(anyhow::anyhow!(
            "expected NOT_IMPLEMENTED, got {}",
            error.code
        ));
    }
    if !error
        .message
        .contains("history: predicate leaves are not executable")
    {
        return Err(anyhow::anyhow!(
            "history predicate-leaf rejection lost detail: {}",
            error.message
        ));
    }

    let follow_up = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up history query after predicate-leaf reject",
    )?;
    if follow_up.commit_ids.len() != 1 || !follow_up.diff_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "follow-up history query diverged after predicate-leaf reject: commit_ids={:?} diff_paths={:?}",
            follow_up.commit_ids,
            follow_up.diff_paths
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_unavailable_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["file.contains", "type:commit", "fix"],
    )
}

#[test]
fn runtime_metadata_predicate_leaf_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_catalog_fixture(&mut rt)?;

    let rejected = rt.query_runtime_metadata(
        TextQuerySyntax::Native,
        "changed:since=1970-01-01T00:00:00.010Z file.contains('catalog_changed_needle')",
        10,
    );
    let error = rejected
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected NOT_IMPLEMENTED for runtime predicate leaf"))?;
    if error.code != "NOT_IMPLEMENTED" {
        return Err(anyhow::anyhow!(
            "expected NOT_IMPLEMENTED, got {}",
            error.code
        ));
    }
    if !error
        .message
        .contains("runtime metadata: predicate leaves are not executable")
    {
        return Err(anyhow::anyhow!(
            "runtime predicate-leaf rejection lost detail: {}",
            error.message
        ));
    }

    let follow_up = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up runtime metadata query after predicate-leaf reject",
    )?;
    if follow_up.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up runtime metadata query diverged after predicate-leaf reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_unavailable_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "file.contains",
            "catalog_changed_needle",
            "changed:since=",
            "src/changed.rs",
        ],
    )
}

#[test]
fn history_before_executes_and_metrics_are_bounded() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let result = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit before:1970-01-01T00:00:00.020Z alpha_content_needle",
        10,
    );
    require_no_typed_error(result.typed_error, "history before query")?;
    if result.commit_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "history before query diverged: commit_ids={:?}",
            result.commit_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["alpha_content_needle", "1970-01-01", "alice"],
    )
}

#[test]
fn history_before_invalid_timeref_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let rejected = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit before:not-a-date alpha_content_needle",
        10,
    );
    let error = rejected
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected HISTORY_INVALID_TIMEREF typed error"))?;
    if error.code != "HISTORY_INVALID_TIMEREF" {
        return Err(anyhow::anyhow!(
            "expected HISTORY_INVALID_TIMEREF, got {}",
            error.code
        ));
    }
    if !error.message.contains("not a valid RFC3339") {
        return Err(anyhow::anyhow!(
            "history before invalid timeref lost detail: {}",
            error.message
        ));
    }

    let follow_up = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up history query after invalid before: timeref",
    )?;
    if follow_up.commit_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up history query diverged after invalid before: timeref: {:?}",
            follow_up.commit_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_typed_error_other_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["not-a-date", "alpha_content_needle", "fix"],
    )
}

#[test]
fn history_since_time_and_commit_execute_and_unknown_commit_fails_closed() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let since_time = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit since.time:1970-01-01T00:00:00.012Z alpha_content_needle",
        10,
    );
    require_no_typed_error(since_time.typed_error, "since.time query")?;
    if since_time.commit_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "since.time query expected one commit, got {:?}",
            since_time.commit_ids
        ));
    }

    let since_commit = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit since.commit:refs/heads/main alpha_content_needle",
        10,
    );
    require_no_typed_error(since_commit.typed_error, "since.commit query")?;
    if since_commit.commit_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "since.commit query expected one commit, got {:?}",
            since_commit.commit_ids
        ));
    }

    let rejected = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit since.commit:refs/heads/missing alpha_content_needle",
        10,
    );
    let error = rejected
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed error for since.commit unknown ref"))?;
    if error.code != "HISTORY_INVALID_TIMEREF" {
        return Err(anyhow::anyhow!(
            "expected HISTORY_INVALID_TIMEREF for since.commit unknown ref, got {}",
            error.code
        ));
    }
    if !error.message.contains("since.commit") {
        return Err(anyhow::anyhow!(
            "since.commit unknown ref lost detail: {}",
            error.message
        ));
    }

    let follow_up = rt.query_history(
        TextQuerySyntax::Native,
        "type:commit since:1970-01-01T00:00:00.010Z alpha_content_needle",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up history query after since.commit failure",
    )?;
    if follow_up.commit_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "follow-up history query diverged after since.commit failure: {:?}",
            follow_up.commit_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_other_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "since.commit:",
            "refs/heads/missing",
            "alpha_content_needle",
        ],
    )
}

#[test]
fn history_after_until_and_diff_filters_execute_and_do_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    for (label, query_text, expect_commit_count, expect_diff_count, leaked_terms) in [
        (
            "after",
            "type:commit after:1970-01-01T00:00:00.011Z alpha_content_needle",
            1usize,
            0usize,
            vec!["after:", "alpha_content_needle", "1970-01-01"],
        ),
        (
            "until",
            "type:commit until:1970-01-01T00:00:00.012Z alpha_content_needle",
            1usize,
            0usize,
            vec!["until:", "alpha_content_needle", "1970-01-01"],
        ),
        (
            "diff.added",
            "type:diff diff.added:history",
            0usize,
            1usize,
            vec!["diff.added:history", "src/history.rs"],
        ),
        (
            "diff.removed",
            "type:diff diff.removed:history",
            0usize,
            1usize,
            vec!["diff.removed:history", "src/history.rs"],
        ),
        (
            "diff.touched",
            "type:diff diff.touched:history",
            0usize,
            1usize,
            vec!["diff.touched:history", "src/history.rs"],
        ),
    ] {
        let result = rt.query_history(TextQuerySyntax::Sourcegraph, query_text, 10);
        require_no_typed_error(result.typed_error, label)?;
        if result.commit_ids.len() != expect_commit_count
            || result.diff_paths.len() != expect_diff_count
        {
            return Err(anyhow::anyhow!(
                "{label} query diverged: commit_ids={:?} diff_paths={:?}",
                result.commit_ids,
                result.diff_paths
            ));
        }

        let follow_up = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
        require_no_typed_error(
            follow_up.typed_error,
            &format!("follow-up history query after {label} execution"),
        )?;
        if follow_up.commit_ids.len() != 1 || !follow_up.diff_paths.is_empty() {
            return Err(anyhow::anyhow!(
                "follow-up history query diverged after {label} execution: commit_ids={:?} diff_paths={:?}",
                follow_up.commit_ids,
                follow_up.diff_paths
            ));
        }
        assert_closed_metric_suffix(
            &rt,
            &[
                "lq_query_intake_total",
                "lq_planner_total",
                "lq_engine_fanout_count",
                "lq_merge_result_count",
            ],
            &[
                "lq_query_intake_total",
                "lq_typed_error_not_ready_total",
                "lq_typed_error_other_total",
                "lq_planner_total",
                "lq_engine_fanout_count",
                "lq_merge_result_count",
            ],
            leaked_terms.as_slice(),
        )?;
    }
    Ok(())
}
