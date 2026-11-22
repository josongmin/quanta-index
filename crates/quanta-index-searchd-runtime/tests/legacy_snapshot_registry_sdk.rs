//! QI-BB-001 / QI-BB-017: registry behavior through a pinned old-Q process.
//! The held-reader reclaim oracle remains in `e2e_snapshot_registry` because
//! its live reader pin is deliberately inaccessible through the public SDK.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchPlaneErrorCodeV2,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_sdk::{QuantaIndex, RepoMetaBatch, SdkError};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, publish_legacy_source, request_when_ready,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const QUERY: &str = "needle";
const TOP_K: u32 = 10;

struct Fixture {
    process: Option<SearchdBinaryProcess>,
    binary: LegacySearchdBinary,
    root: tempfile::TempDir,
    repo: RepoId,
    revision: RevisionId,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let root = private_tempdir()?;
        let binary = LegacySearchdBinary::from_environment()?;
        let mut fixture = Self {
            process: None,
            binary,
            root,
            repo: RepoId::new("repo-e2e")?,
            revision: RevisionId::new("rev-e2e")?,
        };
        fixture.start()?;
        Ok(fixture)
    }

    fn start(&mut self) -> TestResult {
        if self.process.is_some() {
            return Err("old-Q registry fixture already started".into());
        }
        let command = self
            .binary
            .benchmark_command(self.root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
        self.process = Some(SearchdBinaryProcess::start_with_pinned_command(
            self.root.path(),
            command,
        )?);
        Ok(())
    }

    fn stop(&mut self) -> TestResult {
        self.process
            .take()
            .ok_or("old-Q registry fixture is not started")?
            .stop()
    }

    fn client(&self) -> TestResult<QuantaIndex> {
        self.process
            .as_ref()
            .ok_or_else(|| -> Box<dyn Error> { "old-Q registry fixture is not started".into() })?
            .connect()
    }

    fn pin(&self) -> GenerationPin {
        GenerationPin::new(
            self.repo.clone(),
            self.revision.clone(),
            ManifestGeneration::new(1),
        )
    }

    fn publish(&self) -> TestResult {
        let client = self.client()?;
        let (_head, _receipt) = publish_legacy_source(
            &client,
            &self.repo,
            &self.revision,
            1,
            None,
            &[
                ("src/needle.rs", "fn needle() { let needle = 1; }", 1),
                ("src/other.rs", "fn other() { let needle = 2; }", 2),
            ],
            None,
        )?;
        Ok(())
    }

    fn generation_dir(&self, track: &str) -> TestResult<PathBuf> {
        let canonical = std::fs::canonicalize(self.root.path())?;
        Ok(
            GenerationStorageKeyV1::for_repo_revision(&self.repo, &self.revision).generation_dir(
                &canonical.join(format!("indexes/{track}")),
                ManifestGeneration::new(1),
            ),
        )
    }
}

fn with_fixture(test: impl FnOnce(&mut Fixture) -> TestResult) -> TestResult {
    let mut fixture = Fixture::new()?;
    let outcome = test(&mut fixture);
    let stopped = if fixture.process.is_some() {
        fixture.stop()
    } else {
        Ok(())
    };
    outcome.and(stopped)?;
    fixture.binary.verify()
}

fn lexical_ids(client: &QuantaIndex, pin: &GenerationPin) -> TestResult<Vec<String>> {
    let response = request_when_ready(|| {
        client
            .lexical()
            .query()
            .native(QUERY)
            .pinned(pin.clone())
            .top_k(TOP_K)
            .execute()
    })?;
    if response.generation != *pin {
        return Err("old-Q lexical query answered the wrong generation".into());
    }
    Ok(response
        .results
        .into_iter()
        .map(|row| row.candidate_id)
        .collect())
}

fn semantic_ids(client: &QuantaIndex, pin: &GenerationPin) -> TestResult<Vec<String>> {
    let response = request_when_ready(|| {
        client
            .semantic()
            .query()
            .text(QUERY)
            .pinned(pin.clone())
            .top_k(TOP_K)
            .execute()
    })?;
    if response.generation != *pin {
        return Err("old-Q semantic query answered the wrong generation".into());
    }
    Ok(response
        .results
        .into_iter()
        .map(|row| row.candidate_id)
        .collect())
}

fn registry_counters(client: &QuantaIndex) -> TestResult<BTreeMap<String, u64>> {
    Ok(client
        .observability()
        .metrics_snapshot()?
        .counters
        .into_iter()
        .filter(|counter| counter.name.starts_with("snapshot_registry_"))
        .map(|counter| (counter.name, counter.value))
        .collect())
}

fn counter(counters: &BTreeMap<String, u64>, name: &str) -> TestResult<u64> {
    counters
        .get(name)
        .copied()
        .ok_or_else(|| format!("counter `{name}` is in the scrape: {counters:?}").into())
}

fn assert_registry_hit(
    before: &BTreeMap<String, u64>,
    after: &BTreeMap<String, u64>,
    track: &str,
) -> TestResult {
    let hits = format!("snapshot_registry_{track}_hits_total");
    let misses = format!("snapshot_registry_{track}_misses_total");
    if counter(after, &hits)? != counter(before, &hits)?.saturating_add(1)
        || counter(after, &misses)? != counter(before, &misses)?
    {
        return Err(format!(
            "{track}: expected one resident hit without a cold open: {before:?} -> {after:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn second_lexical_query_reuses_resident_handle() -> TestResult {
    with_fixture(|fixture| {
        fixture.publish()?;
        let client = fixture.client()?;
        let pin = fixture.pin();
        let first = lexical_ids(&client, &pin)?;
        if first.len() != 2 {
            return Err(format!("fixture served {} rows, expected 2", first.len()).into());
        }
        let before = registry_counters(&client)?;
        let second = lexical_ids(&client, &pin)?;
        if second != first {
            return Err(format!(
                "resident lexical handle answered differently: first={first:?} second={second:?}"
            )
            .into());
        }
        assert_registry_hit(&before, &registry_counters(&client)?, "lexical")
    })
}

#[test]
fn second_semantic_query_reuses_resident_handle() -> TestResult {
    with_fixture(|fixture| {
        fixture.publish()?;
        let client = fixture.client()?;
        let pin = fixture.pin();
        let first = semantic_ids(&client, &pin)?;
        if first.is_empty() {
            return Err("fixture served no semantic rows".into());
        }
        let before = registry_counters(&client)?;
        let second = semantic_ids(&client, &pin)?;
        if second != first {
            return Err(format!(
                "resident semantic handle answered differently: first={first:?} second={second:?}"
            )
            .into());
        }
        assert_registry_hit(&before, &registry_counters(&client)?, "semantic")
    })
}

#[test]
fn an_auxiliary_publish_into_a_sealed_generation_is_refused_and_keeps_residency() -> TestResult {
    with_fixture(|fixture| {
        fixture.publish()?;
        let client = fixture.client()?;
        let pin = fixture.pin();
        let first = lexical_ids(&client, &pin)?;
        if first.len() != 2 {
            return Err(format!("fixture served {} rows, expected 2", first.len()).into());
        }
        let first_semantic = semantic_ids(&client, &pin)?;
        if first_semantic.is_empty() {
            return Err("fixture served no semantic rows".into());
        }
        let attempt = client.history().publish_repo_meta(
            &RepoMetaBatch::new(
                fixture.repo.clone(),
                fixture.revision.clone(),
                pin.manifest_generation,
            )
            .entry(RepoId::new("repo")?, "license", "apache-2.0"),
        );
        match attempt {
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::GenerationImmutable,
                ..
            }) => {}
            other => return Err(format!("expected GENERATION_IMMUTABLE: {other:?}").into()),
        }

        let dir = fixture.generation_dir("lexical")?;
        for entry in std::fs::read_dir(&dir)? {
            if entry?.file_name().to_string_lossy().contains("repo-meta") {
                return Err(
                    format!("a refused publish wrote an overlay into {}", dir.display()).into(),
                );
            }
        }
        let before = registry_counters(&client)?;
        let after = lexical_ids(&client, &pin)?;
        if after != first {
            return Err(format!("the resident handle served {after:?}, expected {first:?}").into());
        }
        let after_semantic = semantic_ids(&client, &pin)?;
        if after_semantic != first_semantic {
            return Err(format!(
                "the semantic handle was dropped by a refused publish: first={first_semantic:?} after={after_semantic:?}"
            ).into());
        }
        let after_counters = registry_counters(&client)?;
        for track in ["lexical", "semantic"] {
            assert_registry_hit(&before, &after_counters, track)?;
        }
        Ok(())
    })
}

fn remove_semantic_open_proofs(dir: &Path) -> TestResult {
    for name in ["MARKER_SEALED", "semantic-manifest.cbor"] {
        let path = dir.join(name);
        if !path.is_file() {
            return Err(format!("expected semantic open proof at {}", path.display()).into());
        }
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn assert_remote_query_refusal<T>(result: Result<T, SdkError>, track: &str) -> TestResult {
    match result {
        Err(SdkError::Remote { .. }) => Ok(()),
        Err(error) => Err(format!("{track}: expected typed remote refusal, got {error}").into()),
        Ok(_) => Err(format!("{track}: physically missing generation was served").into()),
    }
}

#[test]
fn missing_physical_seals_evict_resident_handles_before_the_next_query() -> TestResult {
    with_fixture(|fixture| {
        fixture.publish()?;
        let client = fixture.client()?;
        let pin = fixture.pin();
        let _lexical = lexical_ids(&client, &pin)?;
        let _semantic = semantic_ids(&client, &pin)?;
        let before = registry_counters(&client)?;
        std::fs::remove_dir_all(fixture.generation_dir("lexical")?)?;
        remove_semantic_open_proofs(&fixture.generation_dir("semantic")?)?;

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let now = registry_counters(&client)?;
            let invalidated = ["lexical", "semantic"].into_iter().all(|track| {
                let name = format!("snapshot_registry_{track}_inventory_invalidations_total");
                matches!((counter(&now, &name), counter(&before, &name)), (Ok(current), Ok(prior)) if current > prior)
            });
            if invalidated {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "physical loss did not invalidate both residents: {before:?} -> {now:?}"
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_remote_query_refusal(
            client
                .lexical()
                .query()
                .native(QUERY)
                .pinned(pin.clone())
                .top_k(TOP_K)
                .execute(),
            "lexical",
        )?;
        assert_remote_query_refusal(
            client
                .semantic()
                .query()
                .text(QUERY)
                .pinned(pin)
                .top_k(TOP_K)
                .execute(),
            "semantic",
        )
    })
}

fn assert_first_queries_are_promoted_hits(fixture: &Fixture) -> TestResult {
    let client = fixture.client()?;
    let pin = fixture.pin();
    let before = registry_counters(&client)?;
    let text = lexical_ids(&client, &pin)?;
    if text.len() != 2 {
        return Err(format!(
            "the promoted lexical handle served {} rows, expected 2",
            text.len()
        )
        .into());
    }
    let semantic = semantic_ids(&client, &pin)?;
    if semantic.is_empty() {
        return Err("the promoted semantic handle served no rows".into());
    }
    let after = registry_counters(&client)?;
    for track in ["lexical", "semantic"] {
        let misses = format!("snapshot_registry_{track}_misses_total");
        if counter(&after, &misses)? != counter(&before, &misses)? {
            return Err(format!(
                "{track}: the first query ran a cold open: {before:?} -> {after:?}"
            )
            .into());
        }
        if counter(
            &after,
            &format!("snapshot_registry_{track}_promotions_total"),
        )? != 1
        {
            return Err(format!("{track}: exactly one promotion is expected: {after:?}").into());
        }
        let hits = format!("snapshot_registry_{track}_hits_total");
        if counter(&after, &hits)? != counter(&before, &hits)?.saturating_add(1) {
            return Err(
                format!("{track}: the first query is one hit: {before:?} -> {after:?}").into(),
            );
        }
    }
    Ok(())
}

#[test]
fn the_first_query_after_activation_is_served_from_the_promoted_handle() -> TestResult {
    with_fixture(|fixture| {
        fixture.publish()?;
        assert_first_queries_are_promoted_hits(fixture)
    })
}

#[test]
fn the_first_query_after_restart_is_served_from_the_promoted_handle() -> TestResult {
    with_fixture(|fixture| {
        fixture.publish()?;
        fixture.stop()?;
        fixture.start()?;
        assert_first_queries_are_promoted_hits(fixture)
    })
}
