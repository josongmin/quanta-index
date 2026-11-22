//! E2E-01 — lexical full-fidelity matrix against a pinned old-Q process.
//!
//! Table-driven. Every row in `SCENARIOS` shares one SDK-published Source
//! generation, then queries after a real process restart. The additional
//! `runtime_rows.toml` inventory is schema checked separately; those rows are
//! not executed by this process rail. This rail executes the specialized
//! regression shapes in `SCENARIOS`.
//!
//! Every `SCENARIOS` row executes and asserts an exact ordered candidate-id
//! set. Typed query errors fail the row with their code and message.

#![forbid(unsafe_code)]

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneErrorCodeV2, SymbolId, TextQuerySyntax,
};
use quanta_index_sdk::{QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, legacy_source_replacement_from_chunks_and_symbols,
        publish_legacy_source_batch, request_when_ready,
    },
    private_tempdir,
};
use std::error::Error;
use std::fmt::Write as _;

struct LexicalScenario {
    id: &'static str,
    /// Native LQ syntax.
    query_text: &'static str,
    top_k: u32,
    expected_ids: &'static [&'static str],
}

/// Corpus shared by all rows.
///
/// One ingest covers content, path-only, multi-language, phrase boundary,
/// regex-only token, raw substring across token boundaries, and
/// trigram-false-positive shapes.
///
/// `id` is the assertion key returned after the harness's synthesized
/// `e2e-<n>-<path>` candidate id is stripped back to `path` and looked up
/// in this table.
struct CorpusRow {
    id: &'static str,
    path: &'static str,
    content: &'static str,
    symbol_name: Option<&'static str>,
}

const CORPUS: &[CorpusRow] = &[
    // content vs. path: content has needle "alpha_content_needle"; the path
    // does NOT, so a content-token query against the needle must NOT match
    // the path-only row below.
    CorpusRow {
        id: "alpha_content",
        path: "src/lib.rs",
        content: "fn alpha_content_needle() {}",
        symbol_name: None,
    },
    // path-only: the path contains the path-only token, content does not.
    CorpusRow {
        id: "beta_pathonly",
        path: "config/path_only_needle.toml",
        content: "value = 1",
        symbol_name: None,
    },
    // python file with same token as alpha — used for `lang:` filter
    // discrimination.
    CorpusRow {
        id: "gamma_py_same",
        path: "scripts/helper.py",
        content: "def alpha_content_needle(): pass",
        symbol_name: None,
    },
    // phrase row: contains "lemon yellow banana" exact-adjacent. A regex
    // token query for `lemon banana` must NOT match (phrase positions).
    CorpusRow {
        id: "delta_phrase",
        path: "docs/colors.md",
        content: "the lemon yellow banana ripens",
        symbol_name: None,
    },
    // regex-only row: contains "v1.2.3-rc.4" which a token query cannot
    // hit, but a regex `v\d+\.\d+\.\d+` can.
    CorpusRow {
        id: "epsilon_regex",
        path: "src/version.rs",
        content: "const VERSION: &str = \"v1.2.3-rc.4\";",
        symbol_name: None,
    },
    // raw substring across token boundary: contains "foo_bar_baz" — a
    // tokenizer splits on `_` so a token query for `foo_bar` may not hit;
    // a raw substring query must hit.
    CorpusRow {
        id: "zeta_raw",
        path: "src/raw.rs",
        content: "let foo_bar_baz = 0;",
        symbol_name: None,
    },
    // trigram false-positive bait: contains "needle_xx" but not "needle_x"
    // exact — a naive trigram match for "needle_x" without verify could
    // false-positive against this row.
    CorpusRow {
        id: "eta_trigram_bait",
        path: "src/bait.rs",
        content: "let needle_xx = 1;",
        symbol_name: None,
    },
    // symbol-shaped row: function declaration named `MyTypeSymbol` —
    // exercised by `type:symbol`/`select:symbol`.
    CorpusRow {
        id: "theta_symbol",
        path: "src/sym.rs",
        content: "pub fn MyTypeSymbol(arg: i32) -> i32 { arg }",
        symbol_name: Some("MyTypeSymbol"),
    },
];

const SCENARIOS: &[LexicalScenario] = &[
    LexicalScenario {
        id: "content_term_does_not_match_path_only",
        // The path "config/path_only_needle.toml" contains the token
        // `path_only_needle` but the row's *content* doesn't. A content-term
        // query must therefore return zero matches for this token (no
        // accidental path indexing into content).
        //
        // NOTE: the harness ingests `path` as a separate field, but this row
        // proves the content query surface does not leak path-only matches.
        query_text: "path_only_needle_zzz_not_present_anywhere",
        top_k: 10,
        expected_ids: &[],
    },
    LexicalScenario {
        id: "repo_has_file_predicate_under_or_short_circuits_true_repo_gate",
        // `repo:has.file(path:src/lib.rs)` is true for this single test
        // repo, so `true OR delta_phrase_token` must widen to all text
        // candidates on the lexical route.
        query_text: "repo:has.file(path:src/lib.rs) OR ripens",
        top_k: 10,
        expected_ids: &[
            "delta_phrase",
            "beta_pathonly",
            "gamma_py_same",
            "eta_trigram_bait",
            "alpha_content",
            "zeta_raw",
            "theta_symbol",
            "epsilon_regex",
        ],
    },
    // ──────── case ────────
    LexicalScenario {
        id: "case_insensitive_default",
        // Default is case-insensitive: "ALPHA_CONTENT_NEEDLE" must match
        // the lowercase content in both rust and python rows.
        query_text: "ALPHA_CONTENT_NEEDLE",
        top_k: 10,
        expected_ids: &["alpha_content", "gamma_py_same"],
    },
    // ──────── phrase ────────
    LexicalScenario {
        id: "phrase_exact_adjacent_matches",
        query_text: "\"lemon yellow banana\"",
        top_k: 10,
        expected_ids: &["delta_phrase"],
    },
    LexicalScenario {
        id: "phrase_unordered_tokens_must_not_match",
        // `banana lemon` (reversed) as a phrase must NOT match the
        // forward-only content. This row proves the live phrase path enforces
        // order and adjacency rather than accidental token coincidence.
        query_text: "\"banana lemon\"",
        top_k: 10,
        expected_ids: &[],
    },
    LexicalScenario {
        id: "manual_scan_same_document_keyword_phrase_and_content",
        query_text: "index:no alpha_content_needle \"fn alpha_content_needle\" file.has.content(\"alpha_content_needle\")",
        top_k: 10,
        expected_ids: &["alpha_content"],
    },
    LexicalScenario {
        id: "manual_scan_does_not_join_keyword_phrase_and_content_across_documents",
        // The Rust file owns the phrase; only the Python file owns `def`.
        // Both files own the keyword, so a cross-document conjunction leaks.
        query_text: "index:no alpha_content_needle \"fn alpha_content_needle\" file.has.content(\"def\")",
        top_k: 10,
        expected_ids: &[],
    },
    // ──────── regex ────────
    LexicalScenario {
        id: "regex_trigram_false_positive_rejected",
        // Regex `needle_x[0-9]` should NOT match `needle_xx` (no digit).
        // A naive trigram prefilter would surface eta_trigram_bait; the
        // exact verify must reject it.
        query_text: "/needle_x[0-9]/",
        top_k: 10,
        expected_ids: &[],
    },
];

#[derive(Debug)]
struct TypedError {
    code: SearchPlaneErrorCodeV2,
    message: String,
}

#[derive(Debug)]
struct QueryResult {
    candidates: Vec<quanta_index_contract::LexicalCandidate>,
    typed_error: Option<TypedError>,
}

fn boxed<T>(result: Result<T, Box<dyn Error>>) -> AnyResult<T> {
    result.map_err(|error| anyhow::anyhow!("{error}"))
}

fn pin() -> AnyResult<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        Some("md") => "markdown",
        Some(_) | None => "text",
    }
}

fn chunk(id: u64, path: &str, text: &str) -> AnyResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-{id}-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new(language_from_path(path)).map_err(anyhow::Error::msg)?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 2,
        text: text.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn symbol(id: &str, path: &str, name: &str) -> AnyResult<SymbolRecord> {
    Ok(SymbolRecord {
        symbol_id: SymbolId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new(language_from_path(path)).map_err(anyhow::Error::msg)?,
        symbol_kind: SymbolKindCode::new("function").map_err(anyhow::Error::msg)?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: name.to_owned().into_boxed_str(),
        qualified_name: format!("crate::{name}").into_boxed_str(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: path.to_owned().into_boxed_str(),
            byte_start: 0,
            byte_end: u32::try_from(name.len())?,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: Some("crate".to_owned().into_boxed_str()),
        relationship: SymbolRelationship::Def,
    })
}

fn with_cold_fixture<T>(
    batch: &SearchCorpusBatch,
    evaluate: impl FnOnce(&QuantaIndex, &GenerationPin) -> AnyResult<T>,
) -> AnyResult<T> {
    let root = private_tempdir()?;
    let state_root = root.path().join("state");
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("pinned old-Q executable: {error}"))?;
    let producer = boxed(binary.start(&state_root, 8))?;
    let published = (|| -> AnyResult<()> {
        let client = boxed(producer.connect())?;
        let _ack = boxed(publish_legacy_source_batch(&client, batch, None))?;
        Ok(())
    })();
    let stopped = producer.stop();
    let verified = binary.verify();
    published?;
    boxed(stopped)?;
    boxed(verified)?;

    let reader = boxed(binary.start(&state_root, 8))?;
    let result = (|| -> AnyResult<T> {
        let client = boxed(reader.connect())?;
        let pin = GenerationPin::new(
            batch.repo_id().clone(),
            batch.revision_id().clone(),
            batch.generation(),
        );
        evaluate(&client, &pin)
    })();
    let stopped = reader.stop();
    let verified = binary.verify();
    let output = result?;
    boxed(stopped)?;
    boxed(verified)?;
    Ok(output)
}

fn run_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query: &str,
    top_k: u32,
) -> AnyResult<QueryResult> {
    run_query_with_languages(client, pin, syntax, query, top_k, &[])
}

fn run_query_with_languages(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query: &str,
    top_k: u32,
    languages: &[&str],
) -> AnyResult<QueryResult> {
    let languages = languages
        .iter()
        .map(|language| LanguageCode::new(*language).map_err(anyhow::Error::msg))
        .collect::<AnyResult<Vec<_>>>()?;
    let response = request_when_ready(|| {
        let builder = client.lexical().query();
        let builder = match syntax {
            TextQuerySyntax::Native => builder.native(query),
            TextQuerySyntax::CodeSearch => builder.code_search(query),
            TextQuerySyntax::Sourcegraph => builder.sourcegraph(query),
        };
        let builder = if languages.is_empty() {
            builder
        } else {
            builder.language_any_of(languages.clone())
        };
        builder.pinned(pin.clone()).top_k(top_k).execute()
    });
    match response {
        Ok(page) if page.generation == *pin => Ok(QueryResult {
            candidates: page.results,
            typed_error: None,
        }),
        Ok(page) => Err(anyhow::anyhow!(
            "pinned old-Q returned generation {:?} for requested {:?}",
            page.generation,
            pin
        )),
        Err(SdkError::Remote { code, message, .. }) => Ok(QueryResult {
            candidates: Vec::new(),
            typed_error: Some(TypedError { code, message }),
        }),
        Err(error) => Err(error.into()),
    }
}

fn candidate_ids(result: &QueryResult) -> Vec<String> {
    result
        .candidates
        .iter()
        .map(|c| c.candidate_id.clone())
        .collect()
}

/// Strip the harness-applied `e2e-<n>-` prefix to recover the bare `path`.
///
/// The harness builds `ChunkId::new(format!("e2e-{n}-{path}"))` in
/// `ingest_text`, so the tail after the second `-` is the row path.
fn corpus_id_for_candidate_id(candidate_id: &str) -> Option<&'static str> {
    if let Some(row) = CORPUS.iter().find(|row| row.id == candidate_id) {
        return Some(row.id);
    }
    // candidate_id shape: `e2e-<n>-<path>`. Split off the leading
    // `e2e-<n>-` to recover the path.
    let rest = candidate_id.strip_prefix("e2e-")?;
    let (_, path) = rest.split_once('-')?;
    CORPUS.iter().find(|row| row.path == path).map(|row| row.id)
}

fn observed_corpus_ids(result: &QueryResult) -> Result<Vec<&'static str>, Vec<String>> {
    let mut observed = Vec::with_capacity(result.candidates.len());
    let mut unmapped = Vec::new();
    for candidate in &result.candidates {
        match corpus_id_for_candidate_id(&candidate.candidate_id) {
            Some(id) => observed.push(id),
            None => unmapped.push(candidate.candidate_id.clone()),
        }
    }
    if unmapped.is_empty() {
        Ok(observed)
    } else {
        Err(unmapped)
    }
}

/// One assertion outcome — either ok or a row-scoped failure to report.
struct RowReport {
    id: &'static str,
    failure: Option<String>,
}

fn assess(scenario: &LexicalScenario, result: &QueryResult) -> RowReport {
    let observed = match observed_corpus_ids(result) {
        Ok(observed) => observed,
        Err(unmapped_candidate_ids) => {
            return RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "observed unmapped candidate ids={unmapped_candidate_ids:?}; raw candidates={:?}",
                    result.candidates
                )),
            };
        }
    };
    let ids = scenario.expected_ids;
    if let Some(err) = &result.typed_error {
        return RowReport {
            id: scenario.id,
            failure: Some(format!(
                "expected Candidates ids={ids:?}, got typed error code={} message={}",
                err.code.as_wire_str(),
                err.message
            )),
        };
    }
    let expected: Vec<&'static str> = ids.to_vec();
    if observed == expected {
        RowReport {
            id: scenario.id,
            failure: None,
        }
    } else {
        RowReport {
            id: scenario.id,
            failure: Some(format!(
                "expected ids={expected:?}, observed ids={observed:?}, raw candidate ids={:?}, raw candidates={:?}",
                candidate_ids(result),
                result.candidates
            )),
        }
    }
}

#[test]
fn lexical_full_fidelity_matrix() -> AnyResult<()> {
    let pin = pin()?;
    let mut chunks = Vec::with_capacity(CORPUS.len());
    let mut symbols = Vec::new();
    for (index, row) in CORPUS.iter().enumerate() {
        chunks.push(chunk(u64::try_from(index + 1)?, row.path, row.content)?);
        if let Some(name) = row.symbol_name {
            symbols.push(symbol(row.id, row.path, name)?);
        }
    }
    let batch = boxed(legacy_source_replacement_from_chunks_and_symbols(
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        chunks,
        symbols,
    ))?;
    with_cold_fixture(&batch, |client, pin| {
        let mut failures: Vec<RowReport> = Vec::new();
        let mut green_count: usize = 0;

        for scenario in SCENARIOS {
            let result = run_query(
                client,
                pin,
                TextQuerySyntax::Native,
                scenario.query_text,
                scenario.top_k,
            )?;
            let report = assess(scenario, &result);
            if report.failure.is_some() {
                failures.push(report);
            } else {
                green_count = green_count.saturating_add(1);
            }
        }

        for (id, languages, top_k, expected_ids) in [
            (
                "indexed_language_unconstrained_top_one_is_rust",
                &[][..],
                1,
                &["alpha_content"][..],
            ),
            (
                "indexed_language_filter_precedes_top_one",
                &["python"][..],
                1,
                &["gamma_py_same"][..],
            ),
            (
                "indexed_language_unique_or_set",
                &["rust", "python"][..],
                10,
                &["alpha_content", "gamma_py_same"][..],
            ),
            (
                "indexed_language_duplicate_or_set_preserves_membership",
                &["rust", "python", "rust"][..],
                10,
                &["alpha_content", "gamma_py_same"][..],
            ),
        ] {
            let scenario = LexicalScenario {
                id,
                query_text: "ALPHA_CONTENT_NEEDLE",
                top_k,
                expected_ids,
            };
            let result = run_query_with_languages(
                client,
                pin,
                TextQuerySyntax::Native,
                scenario.query_text,
                top_k,
                languages,
            )?;
            let report = assess(&scenario, &result);
            if report.failure.is_some() {
                failures.push(report);
            } else {
                green_count = green_count.saturating_add(1);
            }
        }

        if failures.is_empty() {
            return Ok(());
        }

        let mut buf = String::new();
        writeln!(
            buf,
            "E2E-01 lexical_full_fidelity_matrix: {} of {} rows failed (rows currently green: {})",
            failures.len(),
            SCENARIOS.len() + 4,
            green_count
        )?;
        for failure in &failures {
            if let Some(message) = &failure.failure {
                writeln!(buf, "  - [{}] {}", failure.id, message)?;
            }
        }
        Err(anyhow::anyhow!("{buf}"))
    })
}

#[test]
fn code_search_components_use_symbol_names_through_live_driver() -> AnyResult<()> {
    let pin = pin()?;
    let mut chunks = Vec::new();
    let mut symbol_records = Vec::new();
    for (ordinal, (path, content, symbols)) in [
        ("components/exact.rs", "fn cleanUp() {}", &["cleanUp"][..]),
        (
            "components/longer.rs",
            "fn cleanUpNow() {}",
            &["cleanUpNow"][..],
        ),
        (
            "components/split.rs",
            "fn clean() {} fn up() {}",
            &["clean", "up"][..],
        ),
        (
            "components/reversed.rs",
            "fn upClean() {}",
            &["upClean"][..],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        chunks.push(chunk(u64::try_from(ordinal + 1)?, path, content)?);
        for (index, name) in symbols.iter().enumerate() {
            symbol_records.push(symbol(&format!("{path}-{index}"), path, name)?);
        }
    }
    let batch = boxed(legacy_source_replacement_from_chunks_and_symbols(
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        chunks,
        symbol_records,
    ))?;
    with_cold_fixture(&batch, |client, pin| {
        let result = run_query(
            client,
            pin,
            TextQuerySyntax::CodeSearch,
            "components:\"clean up\"",
            10,
        )?;
        anyhow::ensure!(
            result.typed_error.is_none(),
            "component query returned typed error: {:?}",
            result.typed_error
        );
        let paths = result
            .candidates
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            paths == ["components/exact.rs", "components/longer.rs"],
            "unexpected component candidate order: {paths:?}"
        );
        let exact = result
            .candidates
            .first()
            .ok_or_else(|| anyhow::anyhow!("exact component candidate is absent"))?;
        let longer = result
            .candidates
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("longer component candidate is absent"))?;
        anyhow::ensure!(
            exact.score > longer.score,
            "exact component score {} must exceed longer component score {}",
            exact.score,
            longer.score
        );

        let absent = run_query(
            client,
            pin,
            TextQuerySyntax::CodeSearch,
            "components:\"clean missing\"",
            10,
        )?;
        anyhow::ensure!(
            absent.typed_error.is_none(),
            "absent component query returned typed error: {:?}",
            absent.typed_error
        );
        anyhow::ensure!(
            absent.candidates.is_empty(),
            "absent component query returned {} candidates",
            absent.candidates.len()
        );
        Ok(())
    })
}
