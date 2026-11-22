//! Measure the retained Structural and RuntimeMetadata DSL scenarios through the Native SDK.

use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, time::Instant};

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::index::{
    Digest, OperationKey, SourceInput, SourceTextAdmission, ViewRef, ViewSelection,
};
use quanta_index_contract::lex::{DirtyRecord, LanguageCode};
use quanta_index_contract::native_protocol::{
    NATIVE_DSL_PRODUCER_V1, NativeDslCorpusV1, NativeDslFileV1, NativeDslKindV1, NativeDslPageV1,
    NativeDslQueryV1, NativeDslRowsV1, NativeDslRuntimeCatalogV1, NativeDslScopeV1,
    NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse, NativeReadSessionRequest,
    NativeReadSessionState, NativeReadSessionStatus,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, QueryResultWindowV1, RepoId,
    RepoRelativePath, RevisionId, RuntimeCatalogIngestBatch, RuntimeChangedRecord,
    StructuralTreeRecord,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::{
    NativeDslChangesV1, NativeDslFileChangeV1, NativeDslPublicationV1, NativeUdsClient,
    NativeUdsError,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use serde_json::{Value, json};

use super::retained_read::{QUERY_DEADLINE, text_query};
use super::{
    NativeDaemon, NativeDslTiming, NativeExecutable, PublicationAudit, PublicationLabel,
    advance_staged, budget, cold_source_mutations, configure_text_index, create_workspace,
    hex_sha256, native_dsl_history_config, operation, owner, publication,
};
use crate::artifact::LatencySummary;
use crate::scenarios::{DslBenchScenario, FixtureKind, LEXICAL_CORPUS, TOP_K, scenario_by_id};
use crate::{
    RUNTIME_CATALOG_FILES, RUNTIME_DIRTY_EPOCH_MS, STRUCTURAL_TREE_CONTENT,
    STRUCTURAL_TREE_IDENTIFIER, STRUCTURAL_TREE_PATH, runtime_catalog_batch,
    runtime_catalog_fixture_spec, structural_function_tree_record,
};

const TREE_PREFIX: &str = "// fixture\n";
const FACET_GOLD: &[&str] = &[
    "e2e-5-src/owner.rs",
    "e2e-6-src/service.rs",
    "e2e-7-src/layer.rs",
    "e2e-8-src/surface.rs",
];

pub(super) fn candidate_gold(id: &str) -> Result<&'static [&'static str]> {
    Ok(match id {
        "runtime.dirty_no.sourcegraph" => &["e2e-1-src/clean.rs"],
        "runtime.changed.sourcegraph" => &["e2e-3-src/changed.rs", "e2e-4-src/stale.rs"],
        "runtime.stale.sourcegraph" => &[
            "e2e-1-src/clean.rs",
            "e2e-2-src/dirty.rs",
            "e2e-3-src/changed.rs",
            "e2e-4-src/stale.rs",
            "e2e-5-src/owner.rs",
            "e2e-6-src/service.rs",
            "e2e-7-src/layer.rs",
            "e2e-8-src/surface.rs",
            "e2e-9-src/snap.rs",
        ],
        "runtime.snapshot.sourcegraph" => &["e2e-3-src/changed.rs", "e2e-9-src/snap.rs"],
        "runtime.meta_owner.sourcegraph"
        | "runtime.meta_service.sourcegraph"
        | "runtime.meta_layer.sourcegraph"
        | "runtime.meta_surface.sourcegraph" => FACET_GOLD,
        "runtime.affected.sourcegraph" | "runtime.invalidated_by.sourcegraph" => {
            &["e2e-3-src/changed.rs"]
        }
        "structural.tree_match.native" | "structural.tree_match.sourcegraph" => {
            &["e2e-5-src/tree.rs"]
        }
        _ => anyhow::bail!("missing retained DSL oracle for {id}"),
    })
}

pub(super) fn runtime_metadata_source(candidate_id: &str) -> Option<(&'static str, &'static str)> {
    let (_, candidate_path) = candidate_id.strip_prefix("e2e-")?.split_once('-')?;
    RUNTIME_CATALOG_FILES
        .iter()
        .copied()
        .find(|(path, _)| *path == candidate_path)
}

fn fixture_pin() -> Result<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn fixture_kind(fixture: FixtureKind) -> Result<NativeDslKindV1> {
    match fixture {
        FixtureKind::RuntimeCatalog => Ok(NativeDslKindV1::RuntimeMetadata),
        FixtureKind::StructuralTree => Ok(NativeDslKindV1::Structural),
        _ => anyhow::bail!("{} is not a retained DSL fixture", fixture.as_str()),
    }
}

pub(super) fn retained_dsl_query(scenario: &DslBenchScenario) -> Result<NativeDslQueryV1> {
    let mut text = text_query(&scenario.query.resolve(), TOP_K);
    text.syntax = scenario.syntax.into();
    Ok(NativeDslQueryV1 {
        scope: NativeDslScopeV1::for_pin(&fixture_pin()?),
        kind: fixture_kind(scenario.fixture)?,
        text,
        after: None,
    })
}

pub(super) fn verify_retained_page(
    scenario: &DslBenchScenario,
    view: ViewRef,
    page: &NativeDslPageV1,
) -> Result<()> {
    let expected = candidate_gold(scenario.id)?;
    ensure!(
        page.view == view
            && page.rows.kind() == fixture_kind(scenario.fixture)?
            && page.window == QueryResultWindowV1::exact(u32::try_from(expected.len())?)
            && page.after.is_none(),
        "retained DSL view, row kind or result window differs for {}",
        scenario.id,
    );
    ensure!(
        page.rows.candidate_ids().eq(expected.iter().copied()),
        "retained DSL candidate oracle differs for {}",
        scenario.id,
    );
    let repository = fixture_pin()?.repo_id;
    if let NativeDslRowsV1::Structural(rows) = &page.rows {
        let [row] = rows.as_slice() else {
            anyhow::bail!("expected one Structural row")
        };
        let range = row.match_range;
        ensure!(
            row.source_repo_id == repository
                && row.path.as_str() == STRUCTURAL_TREE_PATH
                && (
                    range.byte_start,
                    range.byte_end,
                    range.line_start,
                    range.line_end
                ) == (11, 37, 2, 2),
            "retained DSL Structural source or match range differs for {}",
            scenario.id,
        );
        let [binding] = row.candidate.bindings.as_slice() else {
            anyhow::bail!("expected one Structural capture")
        };
        ensure!(
            binding.metavariable.trim_start_matches(':') == "name"
                && (
                    binding.start_byte,
                    binding.end_byte,
                    binding.start_line,
                    binding.end_line
                ) == (14, 33, 2, 2),
            "retained DSL Structural capture differs for {}",
            scenario.id,
        );
    }
    if let NativeDslRowsV1::RuntimeMetadata(rows) = &page.rows {
        for row in rows {
            let (path, snippet) = runtime_metadata_source(&row.candidate_id)
                .context("retained DSL RuntimeMetadata candidate has no fixture source")?;
            ensure!(
                row.path.as_str() == path
                    && row.source_repo_id == repository
                    && (row.start_line, row.end_line) == (1, 1)
                    && row.snippet == snippet,
                "retained DSL RuntimeMetadata payload differs for {} candidate {}",
                scenario.id,
                row.candidate_id,
            );
        }
    }
    Ok(())
}

struct RetainedFixture {
    pin: GenerationPin,
    sources: Vec<SourceInput>,
    header: NativeDslCorpusV1,
    changes: Vec<NativeDslFileChangeV1>,
    runtime: Option<(RuntimeCatalogIngestBatch, DirtyRecord)>,
}

impl RetainedFixture {
    fn build(fixture: FixtureKind) -> Result<Self> {
        Self::build_with_trees(fixture, &[STRUCTURAL_TREE_PATH])
    }

    fn build_with_trees(fixture: FixtureKind, tree_paths: &[&str]) -> Result<Self> {
        let kind = fixture_kind(fixture)?;
        let pin = fixture_pin()?;
        let tree = format!("{TREE_PREFIX}{STRUCTURAL_TREE_CONTENT}");
        let files = if kind == NativeDslKindV1::Structural {
            LEXICAL_CORPUS
                .iter()
                .copied()
                .chain(tree_paths.iter().map(|path| (*path, tree.as_str())))
                .collect::<Vec<_>>()
        } else {
            RUNTIME_CATALOG_FILES.to_vec()
        };
        let mut sources = Vec::with_capacity(files.len());
        let mut changes = Vec::with_capacity(files.len());
        let mut chunk_ids = std::collections::BTreeMap::new();
        for (index, (path, source)) in files.into_iter().enumerate() {
            let chunk_id = ChunkId::new(format!("e2e-{}-{path}", index + 1));
            let (chunk, start_byte, start_line, trees) = if tree_paths.contains(&path) {
                (
                    STRUCTURAL_TREE_CONTENT,
                    u32::try_from(TREE_PREFIX.len())?,
                    2,
                    vec![StructuralTreeRecord {
                        chunk_id: chunk_id.clone(),
                        record: structural_function_tree_record(
                            path,
                            STRUCTURAL_TREE_CONTENT,
                            STRUCTURAL_TREE_IDENTIFIER,
                        )?,
                    }],
                )
            } else {
                (source, 0, 1, Vec::new())
            };
            let _previous = chunk_ids.insert(path, chunk_id.clone());
            let language = LanguageCode::new(if path.ends_with(".rs") {
                "rust"
            } else {
                "markdown"
            })
            .map_err(anyhow::Error::msg)?;
            sources.push(SourceInput {
                path: path.into(),
                bytes: source.as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: Some(language.clone()),
            });
            changes.push(NativeDslFileChangeV1 {
                source_owner: owner(index)?,
                next: Some(NativeDslFileV1 {
                    chunks: vec![ChunkRecord {
                        chunk_id,
                        repo_relative_path: RepoRelativePath::new(path),
                        language,
                        start_byte,
                        end_byte: u32::try_from(source.len())?,
                        start_line,
                        end_line: start_line
                            + u32::try_from(chunk.bytes().filter(|&byte| byte == b'\n').count())?,
                        text: chunk.into(),
                        structural: None,
                        parent_chunk_id: None,
                        source_repo_id: None,
                    }],
                    trees,
                }),
            });
        }
        let runtime = if kind == NativeDslKindV1::RuntimeMetadata {
            let catalog = runtime_catalog_batch(&runtime_catalog_fixture_spec(), &pin, |path| {
                chunk_ids.get(path).cloned()
            })?;
            let dirty = DirtyRecord {
                wire_version: 1,
                doc_id: chunk_ids
                    .get("src/dirty.rs")
                    .context("missing dirty fixture")?
                    .clone(),
                applied_at_ms: RUNTIME_DIRTY_EPOCH_MS,
                payload_hash: [0x5a; 32],
            };
            Some((catalog, dirty))
        } else {
            None
        };
        let header = NativeDslCorpusV1 {
            pin: pin.clone(),
            source_producer: operation(1).producer,
            structural_ready: kind == NativeDslKindV1::Structural,
        };
        Ok(Self {
            pin,
            sources,
            header,
            changes,
            runtime,
        })
    }

    fn digest(&self) -> Result<String> {
        let bytes =
            serde_json::to_vec(&(&self.sources, &self.header, &self.changes, &self.runtime))?;
        Ok(format!("sha256:{}", hex_sha256(&bytes)))
    }

    fn publish(self, client: &NativeUdsClient) -> Result<ViewRef> {
        let mut audit = PublicationAudit::default();
        let genesis = create_workspace(client)?;
        let configured = configure_text_index(client, genesis, &mut audit)?;
        let ingress = client.begin_input(operation(2), &budget())?;
        let source_count = self.sources.len();
        let NativeIndexResponse::StagedSources(refs) = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository: self.pin.repo_id.clone(),
                inputs: self.sources,
            },
            &budget(),
        )?
        else {
            anyhow::bail!("unexpected retained DSL Source staging reply")
        };
        ensure!(
            refs.len() == source_count,
            "incomplete retained DSL Source staging"
        );
        let source = advance_staged(
            client,
            ingress,
            configured,
            cold_source_mutations(&refs)?,
            1,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut audit,
        )?;
        let scope = NativeDslScopeV1::for_pin(&self.pin);
        let corpus = publish_projection(
            client,
            &scope,
            NativeDslKindV1::Structural,
            source,
            0,
            NativeDslChangesV1::Corpus {
                header: self.header,
                files: self.changes,
            },
            &mut audit,
        )?;
        let Some((catalog, dirty)) = self.runtime else {
            return Ok(corpus);
        };
        let corpus_certificate = client
            .read_projection_binding(
                ViewSelection::Exact(corpus),
                scope.projection(NativeDslKindV1::Structural),
                &budget(),
            )?
            .binding
            .context("retained DSL corpus was not published")?
            .certificate;
        publish_projection(
            client,
            &scope,
            NativeDslKindV1::RuntimeMetadata,
            corpus,
            0,
            NativeDslChangesV1::RuntimeMetadata(NativeDslRuntimeCatalogV1 {
                corpus_certificate,
                dirty: vec![dirty],
                catalog,
            }),
            &mut audit,
        )
    }
}

fn publish_projection(
    client: &NativeUdsClient,
    scope: &NativeDslScopeV1,
    kind: NativeDslKindV1,
    expected_view: ViewRef,
    event_predecessor: u64,
    changes: NativeDslChangesV1,
    audit: &mut PublicationAudit,
) -> Result<ViewRef> {
    let request = client.prepare_dsl_publication_v1(
        &NativeDslPublicationV1 {
            operation: OperationKey {
                producer: NATIVE_DSL_PRODUCER_V1,
                stream: scope.stream(kind),
                ..operation(event_predecessor + 1)
            },
            expected_view,
            event_predecessor,
            authority: Digest([22; 32]),
            changes,
        },
        &budget(),
    )?;
    publication::publish(
        client,
        request,
        PublicationLabel {
            phase: "dsl_projection_ack",
            cycle: None,
        },
        audit,
    )
}

/// Walk exact SDK projection pages while the daemon publishes a successor.
/// The caller controls the executable and provides a fresh workspace endpoint.
pub fn verify_native_dsl_retained_keyset(
    client: &NativeUdsClient,
    kind: NativeDslKindV1,
) -> Result<()> {
    const STRUCTURAL_PATHS: [&str; 3] = ["src/keyset-a.rs", "src/keyset-b.rs", "src/keyset-c.rs"];
    let (fixture, scenario, structural, old_ids, new_ids) = match kind {
        NativeDslKindV1::Structural => {
            let mut fixture =
                RetainedFixture::build_with_trees(FixtureKind::StructuralTree, &STRUCTURAL_PATHS)?;
            let late = fixture
                .changes
                .get_mut(LEXICAL_CORPUS.len() + 1)
                .context("missing Structural continuation fixture")?;
            let update = late.clone();
            late.next
                .as_mut()
                .context("Structural fixture lacks Source")?
                .trees
                .clear();
            let changes = NativeDslChangesV1::Corpus {
                header: fixture.header.clone(),
                files: vec![update],
            };
            (
                fixture,
                "structural.tree_match.native",
                Some(changes),
                &["e2e-5-src/keyset-a.rs", "e2e-7-src/keyset-c.rs"][..],
                &[
                    "e2e-5-src/keyset-a.rs",
                    "e2e-6-src/keyset-b.rs",
                    "e2e-7-src/keyset-c.rs",
                ][..],
            )
        }
        NativeDslKindV1::RuntimeMetadata => (
            RetainedFixture::build(FixtureKind::RuntimeCatalog)?,
            "runtime.changed.sourcegraph",
            None,
            &["e2e-3-src/changed.rs", "e2e-4-src/stale.rs"][..],
            &[
                "e2e-3-src/changed.rs",
                "e2e-4-src/stale.rs",
                "e2e-5-src/owner.rs",
            ][..],
        ),
    };
    let mut query =
        retained_dsl_query(scenario_by_id(scenario).context("missing keyset scenario")?)?;
    query.text.top_k = 1;
    let runtime = fixture.runtime.clone();
    let old_view = fixture.publish(client)?;
    let retained = client.open_read_session(ViewSelection::Exact(old_view), [83; 32], &budget())?;
    let first = client.search_dsl_in_read_session_v1(&retained, query.clone(), &budget())?;
    let cursor = first
        .after
        .clone()
        .context("keyset is missing the retained continuation")?;
    let changes = if let Some(structural) = structural {
        structural
    } else {
        let (mut catalog, dirty) =
            runtime.context("missing RuntimeMetadata continuation fixture")?;
        catalog.overlay_epoch_ms += 1;
        catalog.changed_entries.push(RuntimeChangedRecord {
            doc_id: ChunkId::new("e2e-5-src/owner.rs"),
            applied_at_ms: 30,
            payload_hash: [0xaa; 32],
        });
        let corpus_certificate = client
            .read_projection_binding(
                ViewSelection::Exact(old_view),
                query.scope.projection(NativeDslKindV1::Structural),
                &budget(),
            )?
            .binding
            .context("missing retained RuntimeMetadata corpus")?
            .certificate;
        NativeDslChangesV1::RuntimeMetadata(NativeDslRuntimeCatalogV1 {
            corpus_certificate,
            dirty: vec![dirty],
            catalog,
        })
    };
    let new_view = publish_projection(
        client,
        &query.scope,
        kind,
        old_view,
        1,
        changes,
        &mut PublicationAudit::default(),
    )?;
    ensure!(
        new_view != old_view,
        "projection update did not advance the view"
    );
    let fresh = client.open_read_session(ViewSelection::Exact(new_view), [84; 32], &budget())?;
    let mut foreign = query.clone();
    foreign.after = Some(cursor);
    ensure!(
        matches!(
            client.request(NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchDslV1 {
                session: fresh.session,
                query: foreign,
            }), &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
        ),
        "successor accepted the retained projection cursor",
    );
    walk_projection_pages(client, &retained, &query, old_view, first, old_ids)?;
    let new_first = client.search_dsl_in_read_session_v1(&fresh, query.clone(), &budget())?;
    walk_projection_pages(client, &fresh, &query, new_view, new_first, new_ids)?;
    for session in [retained, fresh] {
        let (released, _) = client.release_read_session(&session, &budget())?;
        ensure!(
            released.state == NativeReadSessionState::Released,
            "keyset session was not released"
        );
    }
    Ok(())
}

fn walk_projection_pages(
    client: &NativeUdsClient,
    session: &NativeReadSessionStatus,
    query: &NativeDslQueryV1,
    view: ViewRef,
    mut page: NativeDslPageV1,
    expected: &[&str],
) -> Result<()> {
    let mut query = query.clone();
    let projection = page.projection;
    let query_digest = page.query;
    for (index, key) in expected.iter().enumerate() {
        ensure!(
            page.view == view
                && page.projection == projection
                && page.query == query_digest
                && page.rows.candidate_ids().eq([*key]),
            "keyset page {} changed view, order, or multiplicity: {:?}",
            index + 1,
            page.rows,
        );
        query.after = page.after.take();
        let continues = index + 1 < expected.len();
        ensure!(
            query.after.is_some() == continues && page.window.has_more() == continues,
            "keyset continuation does not match the expected remaining rows"
        );
        if continues {
            page = client.search_dsl_in_read_session_v1(session, query.clone(), &budget())?;
        }
    }
    Ok(())
}

pub(super) fn retained_fixture_digest(fixture: FixtureKind) -> Result<String> {
    RetainedFixture::build(fixture)?.digest()
}

pub(super) fn retained_config_digest() -> Result<String> {
    let template =
        native_dsl_history_config(Path::new("/fixture-state"), Path::new("/fixture.sock"))?;
    Ok(hex_sha256(&serde_json::to_vec(&template)?))
}

/// Measure the first or repeated queries on an exact published DSL view in a fresh daemon.
pub fn measure_native_dsl_retained(
    daemon_bin: &Path,
    id: &str,
    timing: NativeDslTiming,
) -> Result<Value> {
    ensure!(
        timing.passes > 0 && timing.samples > 0,
        "no retained DSL samples requested"
    );
    let scenario = scenario_by_id(id).context("unregistered DSL scenario")?;
    let query = retained_dsl_query(scenario)?;
    let executable = NativeExecutable::capture(daemon_bin)?;
    let mut samples_ms = Vec::new();
    let mut published = Vec::new();
    let mut daemon_peak_rss_bytes = None;
    for _ in 0..timing.passes {
        let (samples, witness, peak_rss_bytes) =
            measure_pass(&executable, scenario, &query, timing)
                .with_context(|| format!("Native retained DSL scenario {id}"))?;
        samples_ms.extend(samples);
        published.push(witness);
        daemon_peak_rss_bytes = daemon_peak_rss_bytes.max(peak_rss_bytes);
    }
    let latency =
        LatencySummary::from_samples_ms(&samples_ms).context("empty retained DSL sample set")?;
    Ok(json!({
        "schema": "native-dsl-retained-samples-v1", "scenario_id": id, "query": query,
        "daemon_sha256": executable.sha256(), "fixture_sha256": retained_fixture_digest(scenario.fixture)?,
        "daemon_peak_rss_bytes": daemon_peak_rss_bytes,
        "config_template_sha256": retained_config_digest()?,
        "passes": timing.passes, "samples_per_pass": timing.samples, "prime_queries": timing.prime_queries,
        "cooldown_ms": timing.cooldown.as_secs_f64() * 1000.0,
        "pass_settle_ms": timing.pass_settle.as_secs_f64() * 1000.0,
        "samples_ms": samples_ms, "latency": latency, "published": published,
    }))
}

fn measure_pass(
    executable: &NativeExecutable,
    scenario: &DslBenchScenario,
    query: &NativeDslQueryV1,
    timing: NativeDslTiming,
) -> Result<(Vec<f64>, Value, Option<u64>)> {
    let temp = crate::private_tempdir()?;
    let socket = temp.path().join("native.sock");
    let config = temp.path().join("native.json");
    let config_bytes = serde_json::to_vec(&native_dsl_history_config(
        &temp.path().join("state"),
        &socket,
    )?)?;
    fs::write(&config, config_bytes)?;
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
    let daemon = NativeDaemon::start(
        executable,
        &config,
        &socket,
        None,
        0,
        &temp.path().join("daemon.stderr"),
    )?;
    let measured = (|| -> Result<_> {
        let client = NativeUdsClient::new(socket.clone());
        let view = RetainedFixture::build(scenario.fixture)?.publish(&client)?;
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(view),
        ))?;
        let deadline = || RequestBudgetV1::for_duration(QUERY_DEADLINE);
        let mut session = sdk.reader().native()?.open([11; 32], &deadline())?;
        let mut observed = None;
        let samples = timing.sample_pass(|| {
            let query = query.clone();
            let request_budget = deadline();
            let started = Instant::now();
            let page = session.dsl_v1(query, &request_budget)?;
            let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
            verify_retained_page(scenario, view, &page)?;
            observed = Some(page);
            Ok(latency_ms)
        });
        let release = session.release(&deadline());
        let samples = samples?;
        let _remaining_pins = release?;
        Ok((
            samples,
            json!({"view": view, "page": observed.context("missing retained DSL observation")?}),
            daemon.linux_peak_rss_bytes()?,
        ))
    })();
    match (measured, daemon.stop(&mut None)) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(cause), Ok(())) | (Ok(_), Err(cause)) => Err(cause),
        (Err(cause), Err(drain)) => {
            Err(cause.context(format!("retained DSL daemon drain failed: {drain:#}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::SCENARIOS;

    #[test]
    fn every_retained_scenario_has_an_exact_fixture_and_oracle() -> Result<()> {
        let scenarios = SCENARIOS
            .iter()
            .filter(|scenario| fixture_kind(scenario.fixture).is_ok());
        assert_eq!(scenarios.clone().count(), 12);
        for scenario in scenarios {
            let query = retained_dsl_query(scenario)?;
            assert_eq!(query.text.query_text, scenario.query.resolve());
            assert_eq!(
                Some(u64::try_from(candidate_gold(scenario.id)?.len())?),
                scenario.expected_count
            );
        }
        assert_ne!(
            retained_fixture_digest(FixtureKind::RuntimeCatalog)?,
            retained_fixture_digest(FixtureKind::StructuralTree)?
        );
        Ok(())
    }
}
