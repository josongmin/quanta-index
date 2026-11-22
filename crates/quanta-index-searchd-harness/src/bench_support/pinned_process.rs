//! Pinned Legacy process and SDK fixtures for DSL correctness and timing.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::lex::{DirtyRecord, LanguageCode};
use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, ChunkId, ChunkRecord, ExecutionOutcomeV2, GenerationPin,
    HistoryOrderV1, ManifestGeneration, QueryResultWindowV2, RepoId, RepoRelativePath, RevisionId,
    SearchScopeKey, SearchScopeSurface, SourceFileKey, StructuralIngestBatch,
    StructuralReplaceScope, StructuralTreeRecord,
};
use quanta_index_sdk::{DirtyBatch, QuantaIndex, SdkError, SearchCorpusBatch};

use crate::bench_fixture::{
    RUNTIME_CATALOG_FILES, RUNTIME_DIRTY_EPOCH_MS, STRUCTURAL_TREE_CONTENT,
    STRUCTURAL_TREE_IDENTIFIER, STRUCTURAL_TREE_PATH, language_from_path, runtime_catalog_batch,
    runtime_catalog_fixture_spec, structural_function_tree_record,
};
use crate::bench_support::{
    QueryOutcome, candidate_outcome, engine_labels, history_outcome, typed_error_outcome,
};
use crate::history_fixture::{HISTORY_LEDGER, HISTORY_LEDGER_TEXT};
use crate::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, legacy_source_replacement_from_chunks, source_event,
};
use crate::scenarios::{DslBenchScenario, FixtureKind, LEXICAL_CORPUS, TOP_K};
use crate::{E2eErrorCode, private_tempdir};

const REPO: &str = "repo-e2e";
const REVISION: &str = "rev-e2e";
const GENERATION: ManifestGeneration = ManifestGeneration::new(1);

fn accepted(what: &str, receipt: &BatchPublishReceipt) -> Result<()> {
    ensure!(
        receipt.generation == GENERATION && receipt.applied && receipt.durable_sequence != 0,
        "DSL fixture {what} did not durably publish G1: {receipt:?}"
    );
    Ok(())
}

fn source_record(path: &str, content: &str, id: u64) -> Result<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-{id}-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new(language_from_path(path))
            .map_err(|error| anyhow::anyhow!("DSL fixture source language for {path}: {error}"))?,
        start_byte: 0,
        end_byte: u32::try_from(content.len())?,
        start_line: 1,
        end_line: 2,
        text: content.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn source_fixture(
    kind: Option<FixtureKind>,
) -> Result<(Vec<ChunkRecord>, BTreeMap<String, ChunkId>)> {
    let mut chunks = Vec::new();
    let mut by_path = BTreeMap::new();
    let mut next_id = 1_u64;
    fn push(
        chunks: &mut Vec<ChunkRecord>,
        by_path: &mut BTreeMap<String, ChunkId>,
        next_id: &mut u64,
        path: &str,
        content: &str,
    ) -> Result<()> {
        let record = source_record(path, content, *next_id)?;
        *next_id = next_id
            .checked_add(1)
            .context("DSL fixture source ID overflow")?;
        ensure!(
            by_path
                .insert(path.to_string(), record.chunk_id.clone())
                .is_none(),
            "DSL fixture repeats source path {path}"
        );
        chunks.push(record);
        Ok(())
    }
    if kind.is_none_or(|kind| {
        matches!(
            kind,
            FixtureKind::LexicalCorpus | FixtureKind::StructuralTree
        )
    }) {
        for (path, content) in LEXICAL_CORPUS {
            push(&mut chunks, &mut by_path, &mut next_id, path, content)?;
        }
    }
    if kind.is_none_or(|kind| kind == FixtureKind::StructuralTree) {
        push(
            &mut chunks,
            &mut by_path,
            &mut next_id,
            STRUCTURAL_TREE_PATH,
            STRUCTURAL_TREE_CONTENT,
        )?;
    }
    if kind.is_none_or(|kind| kind == FixtureKind::HistoryLedger) {
        push(
            &mut chunks,
            &mut by_path,
            &mut next_id,
            HISTORY_LEDGER.file_path,
            HISTORY_LEDGER_TEXT,
        )?;
        // The original harness immediately publishes History. Its request ID
        // occupies slot 7 when the warm fixture is used.
        next_id = next_id
            .checked_add(1)
            .context("DSL fixture History ID overflow")?;
    }
    if kind.is_none_or(|kind| kind == FixtureKind::RuntimeCatalog) {
        for (path, content) in RUNTIME_CATALOG_FILES {
            push(&mut chunks, &mut by_path, &mut next_id, path, content)?;
        }
    }
    // The Dirty IPC consumes request ID 17 after all source paths are staged.
    ensure!(
        kind.is_some() || next_id == 17,
        "warm DSL fixture source ID sequence changed"
    );
    Ok((chunks, by_path))
}

fn structural_batch(
    repo: &RepoId,
    revision: &RevisionId,
    tree_id: ChunkId,
) -> Result<StructuralIngestBatch> {
    let path = STRUCTURAL_TREE_PATH;
    let tree =
        structural_function_tree_record(path, STRUCTURAL_TREE_CONTENT, STRUCTURAL_TREE_IDENTIFIER)?;
    Ok(StructuralIngestBatch {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: GENERATION,
        base_generation: None,
        manifest_digest: format!("struct:{path}:{}", GENERATION.get()),
        batch_digest: String::new(),
        mode: BatchIngestMode::Delta,
        replace_scopes: vec![StructuralReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                source_file: SourceFileKey {
                    source_repo_id: repo.clone(),
                    repo_relative_path: RepoRelativePath::new(path),
                },
            },
            scope_digest: format!("struct-scope:{path}:1"),
            trees: vec![StructuralTreeRecord {
                chunk_id: tree_id,
                record: tree,
            }],
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    })
}

fn source_batch(
    kind: Option<FixtureKind>,
    repo: &RepoId,
    revision: &RevisionId,
    chunks: Vec<ChunkRecord>,
) -> Result<SearchCorpusBatch> {
    // Staging the Structural scope consumes batch sequence 1 in the original
    // harness, before it assigns a sequence to the Source event.
    let sequence = if kind.is_none_or(|kind| kind == FixtureKind::StructuralTree) {
        2
    } else {
        1
    };
    Ok(
        legacy_source_replacement_from_chunks(repo, revision, GENERATION.get(), chunks)
            .map_err(|error| anyhow::anyhow!("DSL Source fixture: {error}"))?
            .source_event(source_event(sequence, None)),
    )
}

fn publish_fixture(client: &QuantaIndex, kind: Option<FixtureKind>) -> Result<GenerationPin> {
    let repo = RepoId::new(REPO)?;
    let revision = RevisionId::new(REVISION)?;
    let pin = GenerationPin::new(repo.clone(), revision.clone(), GENERATION);
    let (chunks, by_path) = source_fixture(kind)?;

    // Match E2eRuntime's immediate auxiliary publications, which precede
    // Source seal. Their request IDs account for the original chunk-ID gap.
    if kind.is_none_or(|kind| kind == FixtureKind::HistoryLedger) {
        let mut history = HISTORY_LEDGER.build_batch(repo.clone(), revision.clone(), GENERATION)?;
        quanta_index_sdk::stamp_batch_digest_v1(&mut history)?;
        let history_receipt = client.history().publish_history_borrowed_v1(&history)?;
        accepted("History", &history_receipt)?;
    }

    if kind.is_none_or(|kind| kind == FixtureKind::RuntimeCatalog) {
        let dirty_id = by_path
            .get("src/dirty.rs")
            .context("DSL fixture Dirty source file missing")?;
        let dirty = DirtyBatch::new(
            repo.clone(),
            revision.clone(),
            GENERATION,
            RUNTIME_DIRTY_EPOCH_MS,
        )
        .upsert(DirtyRecord {
            wire_version: 1,
            doc_id: dirty_id.clone(),
            applied_at_ms: RUNTIME_DIRTY_EPOCH_MS,
            payload_hash: [0x5a; 32],
        });
        let dirty_receipt = client.runtime().publish_dirty(&dirty)?;
        accepted("Dirty", &dirty_receipt)?;
    }

    let source = source_batch(kind, &repo, &revision, chunks)?;
    let published = client.search_corpus().publish_outcome(&source)?;
    accepted("Source", &published.receipt)?;
    ensure!(
        published.receipt.sealed,
        "DSL fixture Source G1 was not sealed"
    );

    if kind.is_none_or(|kind| kind == FixtureKind::StructuralTree) {
        let tree_id = by_path
            .get(STRUCTURAL_TREE_PATH)
            .cloned()
            .context("DSL fixture structural source file missing")?;
        let structural_receipt = client
            .structural()
            .publish_wire_batch(structural_batch(&repo, &revision, tree_id)?)?;
        accepted("Structural", &structural_receipt)?;
    }

    if kind.is_none_or(|kind| kind == FixtureKind::RuntimeCatalog) {
        let catalog = runtime_catalog_fixture_spec();
        let catalog_batch =
            runtime_catalog_batch(&catalog, &pin, |path| by_path.get(path).cloned())?;
        let catalog_receipt = client.runtime().publish_catalog(catalog_batch)?;
        accepted("Runtime Catalog", &catalog_receipt)?;
    }

    let evidence = quanta_index_sdk::PublishedBatchEvidence::from(&published);
    let activation = client.search_corpus().activate_published(&evidence, None)?;
    let observed = client
        .generations()
        .active_head(repo, revision)?
        .context("DSL fixture activated Source head missing")?;
    ensure!(
        observed == activation.active
            && observed.generation.lexical.manifest_generation == GENERATION,
        "DSL fixture activated Source head differs from G1 acknowledgement"
    );
    Ok(pin)
}

/// Keep the process and its exact fixture alive for the caller's SDK work.
/// A cold fixture contains only the selected family.
pub fn with_fixture<T>(
    binary: &LegacySearchdBinary,
    kind: Option<FixtureKind>,
    evaluate: impl FnOnce(&QuantaIndex, &GenerationPin, &SearchdBinaryProcess) -> Result<T>,
) -> Result<T> {
    let state = private_tempdir()?;
    // The benchmark command clears inherited Quanta knobs, fixes hash-dev and
    // the search-owned 64-dimensional profile, and preserves executable custody.
    let command = binary
        .benchmark_command(state.path(), 8)
        .map_err(|error| anyhow::anyhow!("DSL fixture daemon command: {error}"))?;
    let process = SearchdBinaryProcess::start_with_pinned_command(state.path(), command)
        .map_err(|error| anyhow::anyhow!("DSL fixture start pinned daemon: {error}"))?;
    let result = (|| {
        let client = process
            .connect()
            .map_err(|error| anyhow::anyhow!("DSL fixture connect to pinned daemon: {error}"))?;
        let pin = publish_fixture(&client, kind)?;
        evaluate(&client, &pin, &process)
    })();
    let stopped = process
        .stop()
        .map_err(|error| anyhow::anyhow!("DSL fixture stop pinned daemon: {error}"));
    let verified = binary
        .verify()
        .map_err(|error| anyhow::anyhow!("DSL fixture verify pinned daemon: {error}"));
    let output = result?;
    stopped?;
    verified?;
    Ok(output)
}

fn sdk_outcome<T>(
    response: Result<T, SdkError>,
    served: impl FnOnce(T) -> QueryOutcome,
) -> Result<QueryOutcome> {
    match response {
        Ok(value) => Ok(served(value)),
        Err(SdkError::Remote { code, .. }) => {
            Ok(typed_error_outcome(E2eErrorCode::Remote(code), Vec::new()))
        }
        Err(error) => Err(anyhow::anyhow!("DSL fixture SDK query failed: {error}")),
    }
}

fn with_window(mut outcome: QueryOutcome, window: &QueryResultWindowV2) -> QueryOutcome {
    outcome.early_stop_reason = match window.outcome() {
        ExecutionOutcomeV2::CappedUnknown { cap } => Some(format!("scan cap {cap}")),
        ExecutionOutcomeV2::InterruptedPartial { reason } => Some(format!("{reason:?}")),
        ExecutionOutcomeV2::ExactExhausted => None,
        ExecutionOutcomeV2::LowerBound { continuation } => {
            Some(format!("lower bound (continuation={continuation})"))
        }
        ExecutionOutcomeV2::Approximate { method, .. } => {
            Some(format!("approximate result ({method:?})"))
        }
    };
    outcome
}

/// Run the scenario's original route through the public SDK. A transport,
/// binding or local usage failure aborts the rail; only remote typed errors
/// become query outcomes for the fixed adversarial golden.
pub fn run_scenario_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    scenario: &DslBenchScenario,
) -> Result<QueryOutcome> {
    let query = scenario.query.resolve();
    let query = query.as_ref();
    let syntax = scenario.syntax;
    match scenario.fixture {
        FixtureKind::LexicalCorpus => {
            let builder = client.lexical().query();
            let builder = match syntax {
                crate::artifact::BenchSyntax::Native => builder.native(query),
                crate::artifact::BenchSyntax::Sourcegraph => builder.sourcegraph(query),
            };
            sdk_outcome(builder.pinned(pin.clone()).top_k(TOP_K).execute(), |page| {
                with_window(
                    candidate_outcome(
                        page.results.len(),
                        engine_labels(&page.explanation.engines_touched),
                    ),
                    &page.window,
                )
            })
        }
        FixtureKind::HistoryLedger => {
            let builder = client.history().query();
            let builder = match syntax {
                crate::artifact::BenchSyntax::Native => builder.native(query),
                crate::artifact::BenchSyntax::Sourcegraph => builder.sourcegraph(query),
            };
            sdk_outcome(
                builder
                    .pinned(pin.clone())
                    .top_k(TOP_K)
                    .order(HistoryOrderV1::Recency)
                    .execute(),
                |page| {
                    with_window(
                        history_outcome(page.commits.len(), page.diffs.len()),
                        &page.window,
                    )
                },
            )
        }
        FixtureKind::RuntimeCatalog => {
            let builder = client.runtime().query();
            let builder = match syntax {
                crate::artifact::BenchSyntax::Native => builder.native(query),
                crate::artifact::BenchSyntax::Sourcegraph => builder.sourcegraph(query),
            };
            sdk_outcome(builder.pinned(pin.clone()).top_k(TOP_K).execute(), |page| {
                with_window(
                    candidate_outcome(page.results.len(), Vec::new()),
                    &page.window,
                )
            })
        }
        FixtureKind::StructuralTree => {
            let builder = client.structural().query();
            let builder = match syntax {
                crate::artifact::BenchSyntax::Native => builder.native(query),
                crate::artifact::BenchSyntax::Sourcegraph => builder.sourcegraph(query),
            };
            sdk_outcome(builder.pinned(pin.clone()).top_k(TOP_K).execute(), |page| {
                with_window(
                    candidate_outcome(page.results.len(), Vec::new()),
                    &page.window,
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        ApproximateMethodV2, ApproximateQualityContractV2, CandidateCountV1, CoverageV1,
        ExaminedUniverseV1,
    };

    use super::*;
    use crate::bench_support::{
        ScenarioTruthMode, fixture_corpus_files, validate_scenario_outcome,
        warm_fixture_corpus_files,
    };
    use crate::scenarios::SCENARIOS;

    #[test]
    fn warm_and_isolated_source_fixtures_match_their_declared_corpus() -> Result<()> {
        let repo = RepoId::new(REPO)?;
        let revision = RevisionId::new(REVISION)?;
        for (kind, event_id) in [
            (None, "e2e-source-event-2"),
            (Some(FixtureKind::LexicalCorpus), "e2e-source-event-1"),
            (Some(FixtureKind::StructuralTree), "e2e-source-event-2"),
            (Some(FixtureKind::HistoryLedger), "e2e-source-event-1"),
            (Some(FixtureKind::RuntimeCatalog), "e2e-source-event-1"),
        ] {
            let (records, by_path) = source_fixture(kind)?;
            let expected = kind.map_or_else(warm_fixture_corpus_files, fixture_corpus_files);
            let expected: Vec<_> = expected
                .into_iter()
                .filter(|(path, _)| !path.starts_with("fixture://"))
                .collect();
            let observed: Vec<_> = records
                .iter()
                .map(|record| {
                    (
                        record.repo_relative_path.as_str().to_owned(),
                        record.text.to_string(),
                    )
                })
                .collect();
            assert_eq!(observed, expected, "{kind:?}");
            let reservation =
                source_batch(kind, &repo, &revision, records)?.generation_reservation_request()?;
            assert_eq!(reservation.source_event.event_id, event_id, "{kind:?}");
            assert_eq!(reservation.source_event.expected_base_event_id, None);
            match kind {
                None => assert_eq!(
                    by_path.get("src/dirty.rs").map(|id| id.as_str()),
                    Some("e2e-9-src/dirty.rs")
                ),
                Some(FixtureKind::RuntimeCatalog) => assert_eq!(
                    by_path.get("src/dirty.rs").map(|id| id.as_str()),
                    Some("e2e-2-src/dirty.rs")
                ),
                Some(_) => {}
            }
        }
        Ok(())
    }

    #[test]
    fn matching_rows_require_exact_exhaustion() -> Result<()> {
        let scenario = SCENARIOS
            .iter()
            .find(|scenario| scenario.id == "lexical.keyword.native")
            .context("missing Tail lexical golden")?;
        let check = |window: &QueryResultWindowV2| {
            validate_scenario_outcome(
                scenario,
                ScenarioTruthMode::SharedWarmFixture,
                &with_window(candidate_outcome(4, Vec::new()), window),
            )
        };
        check(&QueryResultWindowV2::exact_probe(4))?;
        let lower_bound =
            QueryResultWindowV2::pageable(4, CandidateCountV1::AtLeast(4), false, Vec::new())
                .map_err(anyhow::Error::msg)?;
        let approximate = QueryResultWindowV2::new(
            4,
            CandidateCountV1::AtLeast(4),
            ExecutionOutcomeV2::Approximate {
                method: ApproximateMethodV2::FilteredRefill,
                quality_contract: ApproximateQualityContractV2::new(4),
            },
            CoverageV1::new(ExaminedUniverseV1::AtLeast(4), None, Vec::new()),
            None,
        )
        .map_err(anyhow::Error::msg)?;
        assert!(check(&lower_bound).is_err());
        assert!(check(&approximate).is_err());
        Ok(())
    }
}
