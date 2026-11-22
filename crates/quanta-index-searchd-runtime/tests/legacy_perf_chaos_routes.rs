//! Bounded query and typed refusal regressions over a pinned Legacy daemon.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;

use quanta_index_contract::{
    BatchIngestMode, ChunkId, EarlyStopReason, EngineTouched, GenerationPin, HistoryOrderV1,
    HybridQueryRequest, HybridQueryResponse, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneStructuralQueryResponse, SearchScopeKey, SearchScopeSurface, SourceFileKey,
    StructuralIngestBatch, StructuralQueryRequest, StructuralReplaceScope, StructuralTreeRecord,
    TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_core::IntegrityScrubPolicyV1;
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_sdk::{CommitRecord, CommitSha, HistoryBatch, QuantaIndex, SdkError};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths, legacy_source_batch,
        publish_legacy_source, request_when_ready,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
    structural_function_tree_record,
};

#[path = "e2e_perf_chaos/structural_trait_fixture.rs"]
mod structural_trait_fixture;
use structural_trait_fixture::structural_trait_record;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn with_daemon(run: impl FnOnce(&Path, &QuantaIndex, &GenerationPin) -> TestResult) -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.benchmark_command(root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let _configured = command
        .env("QUANTA_INDEX_MAINTENANCE_TICK_MS", "50")
        .env("QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS", "86400000")
        .env(
            "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP",
            IntegrityScrubPolicyV1::DEFAULT
                .max_bytes_per_step
                .to_string(),
        );
    let daemon = SearchdBinaryProcess::start_with_pinned_command(root.path(), command)?;
    let pin = GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    );
    let outcome = daemon
        .connect()
        .and_then(|client| run(root.path(), &client, &pin));
    let stopped = daemon.stop();
    outcome.and(stopped)?;
    binary.verify()
}

fn seed_source(
    client: &QuantaIndex,
    pin: &GenerationPin,
    files: &[(&str, &str, u64)],
) -> TestResult {
    let (_head, receipt) = publish_legacy_source(
        client,
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        None,
        files,
        None,
    )?;
    if !receipt.sealed || usize::try_from(receipt.accepted_replace_scopes)? != files.len() {
        return Err(format!("Source fixture did not seal all paths: {receipt:?}").into());
    }
    Ok(())
}

fn lexical(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
) -> Result<TextQueryResponse, SdkError> {
    request_when_ready(|| {
        client
            .lexical()
            .query()
            .native(query)
            .pinned(pin.clone())
            .top_k(10)
            .execute()
    })
}

fn require_exact_regex_hit(answer: TextQueryResponse, pin: &GenerationPin) -> TestResult {
    let [row] = answer.results.as_slice() else {
        return Err(format!("expected only the exact regex fixture row: {answer:?}").into());
    };
    if answer.generation != *pin || row.candidate_id.as_str() != "e2e-1-src/exact.txt" {
        return Err(format!("the exact lexical row or pin changed: {answer:?}").into());
    }
    Ok(())
}

fn require_refusal<T: std::fmt::Debug>(
    observed: Result<T, SdkError>,
    code: &str,
    detail: &str,
) -> TestResult {
    match observed {
        Err(SdkError::Remote {
            code: actual,
            message,
            ..
        }) if actual.as_wire_str() == code && message.contains(detail) => Ok(()),
        other => Err(format!("expected remote {code} with {detail:?}, got {other:?}").into()),
    }
}

fn with_regex_fixture(run: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult) -> TestResult {
    with_daemon(|_root, client, pin| {
        seed_source(
            client,
            pin,
            &[
                ("src/exact.txt", "needle_x exact", 1),
                ("src/bait.txt", "needle_xx bait", 2),
            ],
        )?;
        run(client, pin)
    })
}

fn lexical_refusal_and_follow_up(query: &str, code: &str, detail: &str) -> TestResult {
    with_regex_fixture(|client, pin| {
        require_refusal(lexical(client, pin, query), code, detail)?;
        require_exact_regex_hit(lexical(client, pin, "needle_x")?, pin)
    })
}

#[test]
fn regex_false_positive_candidate_is_rejected_by_exact_verify() -> TestResult {
    with_regex_fixture(|client, pin| {
        require_exact_regex_hit(lexical(client, pin, r"/needle_x\b/")?, pin)
    })
}

#[test]
fn regex_typed_rejection_does_not_poison_next_query() -> TestResult {
    lexical_refusal_and_follow_up("/(?<=needle_)x/", "PARSE_FAIL", "regex")
}

#[test]
fn regex_timeout_is_typed_and_does_not_poison_next_query() -> TestResult {
    lexical_refusal_and_follow_up(r"timeout:0ms /needle_x\b/", "QUERY_TIMEOUT", "timed out")
}

#[test]
fn oversized_raw_substring_query_fails_parse_and_does_not_poison_next_query() -> TestResult {
    let query = format!("'{}'", "abcdefghijklmnopq".repeat(1024));
    lexical_refusal_and_follow_up(&query, "PARSE_FAIL", "16 KiB cap")
}

fn trigram_limit_query() -> String {
    let alphabet = *b"abcdefghijklmnopq";
    let mut out = String::from("'");
    for a in alphabet {
        for b in alphabet {
            for c in alphabet {
                out.push(char::from(a));
                out.push(char::from(b));
                out.push(char::from(c));
            }
        }
    }
    out.push('\'');
    out
}

#[test]
fn raw_substring_trigram_plan_limit_is_typed_and_does_not_poison_next_query() -> TestResult {
    lexical_refusal_and_follow_up(
        &trigram_limit_query(),
        "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED",
        "distinct trigrams",
    )
}

#[test]
fn trigram_plan_limit_query_fails_typed_and_does_not_poison_next_query() -> TestResult {
    lexical_refusal_and_follow_up(
        &trigram_limit_query(),
        "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED",
        "dimension=trigram-set",
    )
}

fn hybrid(root: &Path, pin: &GenerationPin, top_k: u32) -> TestResult<HybridQueryResponse> {
    let [query, _, _] = daemon_socket_paths(root);
    let envelope = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 1,
        diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "scope".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            semantic_query_text: "scope".into(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k,
        }),
    };
    let answer = request_when_ready(|| {
        let reply: SearchPlaneQueryIpcResponseEnvelope =
            send_request(&query, &envelope, ClientIoPolicy::default())
                .map_err(SdkError::Transport)?;
        match reply.payload {
            SearchPlaneQueryIpcResponse::Hybrid(answer)
                if reply.request_id == envelope.request_id =>
            {
                Ok(answer)
            }
            SearchPlaneQueryIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            other => Err(SdkError::Protocol(format!(
                "expected the corresponding Hybrid reply, got {other:?}"
            ))),
        }
    })?;
    if answer.generation != *pin
        || answer.explanation.early_stop_reason != Some(EarlyStopReason::CountReached)
    {
        return Err(format!(
            "Hybrid response did not reach its count on the fixture pin: {answer:?}"
        )
        .into());
    }
    Ok(answer)
}

#[test]
fn hybrid_count_cap_surfaces_truthful_early_stop_reason() -> TestResult {
    with_daemon(|root, client, pin| {
        seed_source(
            client,
            pin,
            &[
                ("src/alpha.rs", "scope alpha keep", 1),
                ("src/beta.rs", "scope beta keep", 2),
                ("src/gamma.rs", "scope gamma keep", 3),
            ],
        )?;
        let answer = hybrid(root, pin, 2)?;
        if answer.results.len() != 2
            || answer.explanation.engines_touched
                != [EngineTouched::Lexical, EngineTouched::Semantic]
            || answer.explanation.summary
                != "hybrid fused 3 lexical and 3 semantic candidates into 2 results"
        {
            return Err(
                format!("Hybrid cap lost its exact rows or fused counts: {answer:?}").into(),
            );
        }
        Ok(())
    })
}

#[test]
fn hybrid_large_tied_result_set_keeps_order_stable() -> TestResult {
    let paths: Vec<_> = (0..24)
        .map(|index| format!("src/tie-{index:02}.rs"))
        .collect();
    let files: Vec<_> = paths
        .iter()
        .zip(1_u64..)
        .map(|(path, id)| (path.as_str(), "scope tie keep", id))
        .collect();
    with_daemon(|root, client, pin| {
        seed_source(client, pin, &files)?;
        let mut baseline = None;
        for run in 0..5 {
            let ids: Vec<_> = hybrid(root, pin, 10)?
                .results
                .into_iter()
                .map(|row| row.candidate.candidate_id)
                .collect();
            if ids.len() != 10 || baseline.as_ref().is_some_and(|previous| previous != &ids) {
                return Err(format!("Hybrid tie order or size changed on run {run}: baseline={baseline:?}, current={ids:?}").into());
            }
            let _baseline = baseline.get_or_insert(ids);
        }
        Ok(())
    })
}

const FUNCTION_PATH: &str = "src/structural.rs";
const FUNCTION_TEXT: &str = "fn chaos_structural_alpha() {}";
const FUNCTION_ID: &str = "e2e-1-src/structural.rs";
const TRAIT_PATH: &str = "src/structural_trait.rs";
const TRAIT_TEXT: &str = "trait chaos_structural_trait {}";
const TRAIT_ID: &str = "e2e-2-src/structural_trait.rs";
const TYPED_HOLE_QUERY: &str = "match { function_item { { :[name.expr] } } }";

fn with_structural_fixture(
    run: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult,
) -> TestResult {
    with_daemon(|_root, client, pin| {
        let batch = legacy_source_batch(
            &pin.repo_id,
            &pin.revision_id,
            pin.manifest_generation.get(),
            None,
            &[
                (FUNCTION_PATH, FUNCTION_TEXT, 1),
                (TRAIT_PATH, TRAIT_TEXT, 2),
            ],
        )?;
        let source = client.search_corpus().publish_outcome(&batch)?;
        if !source.receipt.applied || !source.receipt.sealed || source.receipt.durable_sequence == 0
        {
            return Err(format!(
                "Structural fixture has no durable Source seal: {:?}",
                source.receipt
            )
            .into());
        }
        for (path, id, record) in [
            (
                FUNCTION_PATH,
                FUNCTION_ID,
                structural_function_tree_record(
                    FUNCTION_PATH,
                    FUNCTION_TEXT,
                    "chaos_structural_alpha",
                )?,
            ),
            (TRAIT_PATH, TRAIT_ID, structural_trait_record(TRAIT_TEXT)?),
        ] {
            let receipt = client
                .structural()
                .publish_wire_batch(StructuralIngestBatch {
                    repo_id: pin.repo_id.clone(),
                    revision_id: pin.revision_id.clone(),
                    generation: pin.manifest_generation,
                    base_generation: None,
                    manifest_digest: format!("struct:{path}:{}", pin.manifest_generation.get()),
                    batch_digest: String::new(),
                    mode: BatchIngestMode::Delta,
                    replace_scopes: vec![StructuralReplaceScope {
                        scope: SearchScopeKey {
                            doc_surface: SearchScopeSurface::Chunk,
                            source_file: SourceFileKey {
                                source_repo_id: pin.repo_id.clone(),
                                repo_relative_path: RepoRelativePath::new(path),
                            },
                        },
                        scope_digest: format!("struct-scope:{path}"),
                        trees: vec![StructuralTreeRecord {
                            chunk_id: ChunkId::new(id),
                            record,
                        }],
                    }],
                    tombstone_scopes: Vec::new(),
                    seal: false,
                })?;
            if !receipt.applied
                || receipt.sealed
                || receipt.durable_sequence == 0
                || receipt.accepted_replace_scopes != 1
            {
                return Err(format!("Structural delta did not commit: {receipt:?}").into());
            }
        }
        // These queries use their retained Source pin before activation.
        run(client, pin)
    })
}

fn structural(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query: &str,
) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
    request_when_ready(|| {
        client.structural().query_request(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 10,
                cursor: None,
            },
            cursor: None,
        })
    })
}

fn require_structural_ids(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query: &str,
    expected: &[&str],
) -> TestResult {
    let answer = structural(client, pin, syntax, query)?;
    let mut ids: Vec<_> = answer
        .results
        .iter()
        .map(|row| row.candidate_id.as_str())
        .collect();
    ids.sort_unstable();
    if answer.generation != *pin || ids.as_slice() != expected {
        return Err(format!("{query}: expected {expected:?} at {pin:?}, got {answer:?}").into());
    }
    Ok(())
}

#[test]
fn structural_mixed_lexical_or_executes_and_does_not_poison_next_query() -> TestResult {
    with_structural_fixture(|client, pin| {
        require_structural_ids(
            client,
            pin,
            TextQuerySyntax::Native,
            "chaos_structural_alpha OR match { trait_item }",
            &[FUNCTION_ID, TRAIT_ID],
        )?;
        require_structural_ids(
            client,
            pin,
            TextQuerySyntax::Native,
            TYPED_HOLE_QUERY,
            &[FUNCTION_ID],
        )
    })
}

#[test]
fn structural_mixed_lexical_and_not_executes_and_does_not_poison_next_query() -> TestResult {
    with_structural_fixture(|client, pin| {
        require_structural_ids(
            client,
            pin,
            TextQuerySyntax::Native,
            "chaos_structural_alpha AND NOT match { trait_item }",
            &[FUNCTION_ID],
        )?;
        require_structural_ids(
            client,
            pin,
            TextQuerySyntax::Native,
            TYPED_HOLE_QUERY,
            &[FUNCTION_ID],
        )
    })
}

#[test]
fn structural_typed_hole_kind_rejects_typed_and_does_not_poison_next_query() -> TestResult {
    with_structural_fixture(|client, pin| {
        let invalid = structural(
            client,
            pin,
            TextQuerySyntax::Native,
            "match { function_item { { :[name.lambda] } } }",
        );
        require_refusal(
            invalid,
            "STR_HOLE_KIND_UNSUPPORTED",
            "typed hole kind `lambda`",
        )?;
        require_structural_ids(
            client,
            pin,
            TextQuerySyntax::Sourcegraph,
            r#"patterntype:structural "function_item { { :[name.expr] } }""#,
            &[FUNCTION_ID],
        )
    })
}

fn history_commit() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: CommitSha::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
        ]),
        parents: Vec::new(),
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        author: "alice".to_string().into_boxed_str(),
        author_name: None,
        author_email: None,
        committer: "alice".to_string().into_boxed_str(),
        committer_name: None,
        committer_email: None,
        message: "fix: sample history alpha_content_needle"
            .to_string()
            .into_boxed_str(),
        is_merge: false,
        tags: vec!["v1.0.0".to_string().into_boxed_str()],
    }
}

fn with_history_fixture(
    include_ref: bool,
    include_tag: bool,
    run: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult,
) -> TestResult {
    with_daemon(|_root, client, pin| {
        seed_source(
            client,
            pin,
            &[("src/history.rs", "history lexical proof", 1)],
        )?;
        let commit = history_commit();
        let mut batch = HistoryBatch::new(
            pin.repo_id.clone(),
            pin.revision_id.clone(),
            pin.manifest_generation,
        )
        .manifest_digest(format!(
            "legacy-perf-history:{}:{}:{}",
            pin.manifest_generation.get(),
            include_ref,
            include_tag
        ))
        .commit(commit.clone());
        if include_ref {
            batch = batch.ref_upsert("refs/heads/main", commit.sha);
        }
        if include_tag {
            batch = batch.tag_upsert("v1.0.0", commit.sha);
        }
        let receipt = client.history().publish(&batch)?;
        if !receipt.applied
            || receipt.generation != pin.manifest_generation
            || receipt.durable_sequence == 0
            || receipt.accepted_replace_scopes
                != 1 + u32::from(include_ref) + u32::from(include_tag)
        {
            return Err(
                format!("History fixture did not publish exact shards: {receipt:?}").into(),
            );
        }
        run(client, pin)
    })
}

fn history_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query_text: &str,
) -> Result<quanta_index_contract::SearchPlaneHistoryQueryResponse, SdkError> {
    request_when_ready(|| {
        let builder = client.history().query();
        let builder = match syntax {
            TextQuerySyntax::Native => builder.native(query_text),
            TextQuerySyntax::Sourcegraph => builder.sourcegraph(query_text),
            TextQuerySyntax::CodeSearch => {
                return Err(SdkError::Protocol(
                    "CodeSearch is outside this Legacy History fixture".into(),
                ));
            }
        };
        builder
            .pinned(pin.clone())
            .top_k(10)
            .order(HistoryOrderV1::Recency)
            .execute()
    })
}

#[test]
fn history_missing_ref_shard_refuses_typed_and_does_not_poison_next_query() -> TestResult {
    with_history_fixture(false, false, |client, pin| {
        require_refusal(
            history_query(
                client,
                pin,
                TextQuerySyntax::Sourcegraph,
                "type:commit rev:refs/heads/main fix",
            ),
            "HISTORY_SHARD_UNAVAILABLE",
            "ref shard is unavailable",
        )?;
        let follow_up =
            history_query(client, pin, TextQuerySyntax::Sourcegraph, "type:commit fix")?;
        if follow_up.generation != *pin || follow_up.commits.len() != 1 {
            return Err(
                format!("History query after missing ref shard diverged: {follow_up:?}").into(),
            );
        }
        Ok(())
    })
}

#[test]
fn history_missing_tag_shard_refuses_typed_and_does_not_poison_next_query() -> TestResult {
    with_history_fixture(true, false, |client, pin| {
        require_refusal(
            history_query(
                client,
                pin,
                TextQuerySyntax::Sourcegraph,
                "type:commit rev:v1.0.0 fix",
            ),
            "HISTORY_SHARD_UNAVAILABLE",
            "tag shard is unavailable",
        )?;
        let follow_up =
            history_query(client, pin, TextQuerySyntax::Sourcegraph, "type:commit fix")?;
        if follow_up.generation != *pin || follow_up.commits.len() != 1 {
            return Err(
                format!("History query after missing tag shard diverged: {follow_up:?}").into(),
            );
        }
        Ok(())
    })
}

#[test]
fn history_before_executes_on_the_pinned_generation() -> TestResult {
    with_history_fixture(true, true, |client, pin| {
        let response = history_query(
            client,
            pin,
            TextQuerySyntax::Native,
            "type:commit before:1970-01-01T00:00:00.020Z alpha_content_needle",
        )?;
        if response.generation != *pin || response.commits.len() != 1 || !response.diffs.is_empty()
        {
            return Err(format!("History before query diverged: {response:?}").into());
        }
        Ok(())
    })
}

#[test]
fn history_before_invalid_timeref_refuses_typed_and_does_not_poison_next_query() -> TestResult {
    with_history_fixture(true, true, |client, pin| {
        require_refusal(
            history_query(
                client,
                pin,
                TextQuerySyntax::Native,
                "type:commit before:not-a-date alpha_content_needle",
            ),
            "HISTORY_INVALID_TIMEREF",
            "not a valid RFC3339",
        )?;
        let follow_up =
            history_query(client, pin, TextQuerySyntax::Sourcegraph, "type:commit fix")?;
        if follow_up.generation != *pin || follow_up.commits.len() != 1 {
            return Err(format!(
                "History query after invalid before timeref diverged: {follow_up:?}"
            )
            .into());
        }
        Ok(())
    })
}
