//! Declared scale-tier source fixtures across real daemon OS-process restart.
//!
//! This is a functional process/custody regression, not a scale timing or
//! Linux release/physical-I/O qualification. The two daemons publish/open the
//! same production state root through SDK and disk adapters.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::time::Duration;

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, LexicalCandidate, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2, SourceFileKey, SourcePublicationEvent,
    TextQueryResponse, lex::LanguageCode,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, private_tempdir,
    scale::{
        SCALE_HISTORY_MAX_BYTES, SCALE_HISTORY_MAX_TOTAL_BYTES, ScaleTier, ScopedFile,
        ScopedOracle, generate_scoped_corpus, params_for, repo_query_token,
    },
};
use sha2::{Digest, Sha256};

use crate::fail_closed_wait::{RealTicker, wait_for};
use crate::searchd_binary_process::SearchdBinaryProcess;

type TestResult = Result<(), Box<dyn Error>>;

const SEED: u64 = 0x5161_5343_414c_4531;
const REPO: &str = "repo-scale-process-restart";
const REVISION: &str = "revision-scale-process-restart";
const EVENT_1: &str = "fixture:scale-process-restart:g1";
const EVENT_2: &str = "fixture:scale-process-restart:g2";
const RETAINED_TOKEN: &str = "scalefileneedle001file00000";
const DELETED_TOKEN: &str = "scalefileneedle000file00000";
const SUPPORTED_PROFILE_TIMEOUT: Duration = Duration::from_secs(600);

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new(REPO)?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new(REVISION)?)
}

fn event(id: &str, prior: Option<&str>) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: "fixture:scale-process-restart".to_string(),
        event_id: id.to_string(),
        expected_base_event_id: prior.map(str::to_string),
        payload_sha256: [0; 32],
    }
}

fn initial_batch(
    files: Vec<ScopedFile>,
    tier: ScaleTier,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let params = params_for(tier);
    let fixed_count = match tier {
        ScaleTier::Medium => 256,
        ScaleTier::Large => 4_096,
        ScaleTier::Xlarge => 32_768,
        ScaleTier::Small => {
            return Err("process restart fixture requires medium, large or xlarge".into());
        }
    };
    if files.len() != usize::try_from(params.total_files())? || files.len() != fixed_count {
        return Err(format!(
            "source-derived {tier:?} tier must contain exactly {fixed_count} files"
        )
        .into());
    }
    let owner = repo()?;
    let revision = revision()?;
    let mut batch = SearchCorpusBatch::replace_generation(
        owner,
        revision.clone(),
        ManifestGeneration::new(1),
        "manifest:scale-process-restart:g1",
    )
    .source_event(event(EVENT_1, None));
    for (index, file) in files.into_iter().enumerate() {
        let source_repo = RepoId::new(file.source_repo_id.as_str())?;
        let path = RepoRelativePath::new(file.repo_relative_path.as_str());
        let end_byte = u32::try_from(file.content.len())?;
        let end_line = u32::try_from(file.content.lines().count())?;
        let record = ChunkRecord {
            chunk_id: ChunkId::new(format!("scale-process-{index}")),
            repo_relative_path: path.clone(),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line,
            text: file.content.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(source_repo.clone()),
        };
        let scope = fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id: source_repo,
                repo_relative_path: path,
            },
            revision.clone(),
            vec![record],
            Vec::new(),
        )?;
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

fn tombstone_batch() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    Ok(SearchCorpusBatch::delta(
        repo()?,
        revision()?,
        ManifestGeneration::new(2),
        ManifestGeneration::new(1),
        "manifest:scale-process-restart:g2",
    )
    .source_event(event(EVENT_2, Some(EVENT_1)))
    .tombstone_scope(SourceFileKey {
        source_repo_id: RepoId::new("repo0")?,
        repo_relative_path: RepoRelativePath::new("src/file_0.rs"),
    }))
}

type SourceKey = (String, String);

struct SourceTruth {
    // fixture_source_scope_v1 appends one newline after each chunk's bytes.
    source: BTreeMap<SourceKey, ([u8; 32], u32)>,
    repo2_paths: BTreeSet<SourceKey>,
}

fn source_truth(files: &[ScopedFile], tier: ScaleTier) -> Result<SourceTruth, Box<dyn Error>> {
    let _oracle = ScopedOracle::from_source(files, tier)?;
    let mut source = BTreeMap::new();
    let mut retained = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut repo2_paths = BTreeSet::new();
    let mut repo2_token_paths = BTreeSet::new();
    for file in files {
        let key = (file.source_repo_id.clone(), file.repo_relative_path.clone());
        let digest: [u8; 32] = Sha256::digest(format!("{}\n", file.content).as_bytes()).into();
        let lines = u32::try_from(file.content.lines().count())?;
        if source.insert(key.clone(), (digest, lines)).is_some() {
            return Err("fixture contains duplicate source identity".into());
        }
        if file.content.contains(RETAINED_TOKEN) {
            let _inserted = retained.insert(key.clone());
        }
        if file.content.contains(DELETED_TOKEN) {
            let _inserted = deleted.insert(key.clone());
        }
        if file.source_repo_id == "repo2" {
            let _inserted = repo2_paths.insert(key.clone());
        }
        if file.content.contains(&repo_query_token(2)) {
            let _inserted = repo2_token_paths.insert(key);
        }
    }
    if retained != BTreeSet::from([("repo1".into(), "src/file_0.rs".into())])
        || deleted != BTreeSet::from([("repo0".into(), "src/file_0.rs".into())])
        || repo2_paths.len() != usize::try_from(params_for(tier).files_per_repo)?
        || repo2_token_paths != repo2_paths
    {
        return Err("planted query tokens differ from independent fixture source".into());
    }
    Ok(SourceTruth {
        source,
        repo2_paths,
    })
}

fn wait_ready(client: &QuantaIndex, tier: ScaleTier) -> TestResult {
    let timeout = if tier == ScaleTier::Medium {
        Duration::from_secs(30)
    } else {
        SUPPORTED_PROFILE_TIMEOUT
    };
    let _ready = wait_for(
        &RealTicker::new(),
        timeout,
        Duration::from_millis(50),
        "real daemon ready with one active repository",
        || client.observability().process_readiness(),
        |report| report.ready && report.active_repositories == 1,
        |error| {
            matches!(
                error,
                SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::NotReady,
                    ..
                }
            )
        },
    )?;
    Ok(())
}

fn query(
    client: &QuantaIndex,
    token: &str,
    top_k: u32,
) -> Result<TextQueryResponse, Box<dyn Error>> {
    Ok(client
        .lexical()
        .query()
        .native(token)
        .active(repo()?, revision()?)
        .top_k(top_k)
        .execute()?)
}

fn check_row(row: &LexicalCandidate, truth: &SourceTruth, generation: u64) -> TestResult {
    let key = (
        row.source_repo_id.as_str().to_owned(),
        row.repo_relative_path.as_str().to_owned(),
    );
    let (expected_hash, expected_lines) = truth
        .source
        .get(&key)
        .ok_or_else(|| format!("foreign source result: {key:?}"))?;
    let source = row
        .source
        .as_ref()
        .ok_or("result has no source commitment")?;
    if source.file.source_repo_id != row.source_repo_id
        || source.file.repo_relative_path != row.repo_relative_path
        || source.revision_id != revision()?
        || source.source_sha256 != *expected_hash
        || row.repo_id != repo()?
        || row.revision_id != revision()?
        || row.manifest_generation != ManifestGeneration::new(generation)
        || row.start_line != 1
        || row.end_line != *expected_lines
    {
        return Err(format!("source identity or bytes changed for {key:?}").into());
    }
    Ok(())
}

fn unique_hit(
    client: &QuantaIndex,
    token: &str,
    generation: u64,
    expected: (&str, &str),
    truth: &SourceTruth,
) -> Result<LexicalCandidate, Box<dyn Error>> {
    let response = query(client, token, 10)?;
    if response.generation
        != GenerationPin::new(repo()?, revision()?, ManifestGeneration::new(generation))
        || response.results.len() != 1
        || response.next_cursor.is_some()
    {
        return Err(format!("unique source query returned wrong page: {token}").into());
    }
    let row = response
        .results
        .into_iter()
        .next()
        .ok_or("missing unique source row")?;
    check_row(&row, truth, generation)?;
    if row.source_repo_id.as_str() != expected.0 || row.repo_relative_path.as_str() != expected.1 {
        return Err(format!("wrong source for unique token {token}").into());
    }
    Ok(row)
}

#[derive(PartialEq)]
struct Observed {
    retained: LexicalCandidate,
    retained_score_bits: u32,
    scoped: Vec<LexicalCandidate>,
    scoped_score_bits: Vec<u32>,
}

fn observe_generation_two(
    client: &QuantaIndex,
    truth: &SourceTruth,
    tier: ScaleTier,
) -> Result<Observed, Box<dyn Error>> {
    wait_ready(client, tier)?;
    let retained = unique_hit(client, RETAINED_TOKEN, 2, ("repo1", "src/file_0.rs"), truth)?;
    let deleted = query(client, DELETED_TOKEN, 10)?;
    let expected_pin = GenerationPin::new(repo()?, revision()?, ManifestGeneration::new(2));
    if deleted.generation != expected_pin
        || !deleted.results.is_empty()
        || deleted.next_cursor.is_some()
    {
        return Err("tombstoned source reappeared".into());
    }
    let scoped = query(
        client,
        &repo_query_token(2),
        params_for(tier).files_per_repo,
    )?;
    let mut observed = BTreeSet::new();
    for row in &scoped.results {
        check_row(row, truth, 2)?;
        if !observed.insert((
            row.source_repo_id.as_str().to_owned(),
            row.repo_relative_path.as_str().to_owned(),
        )) {
            return Err("duplicate source identity in ranked page".into());
        }
    }
    if scoped.generation != expected_pin
        || scoped.results.len() != truth.repo2_paths.len()
        || observed != truth.repo2_paths
        || scoped.next_cursor.is_some()
    {
        return Err("repo2 ranked page differs from source-derived complete set".into());
    }
    Ok(Observed {
        retained_score_bits: retained.score.to_bits(),
        retained,
        scoped_score_bits: scoped
            .results
            .iter()
            .map(|row| row.score.to_bits())
            .collect(),
        scoped: scoped.results,
    })
}

#[test]
fn medium_scale_source_survives_real_daemon_process_restart_and_delete() -> TestResult {
    run_scale_process_restart(ScaleTier::Medium)
}

#[test]
#[ignore = "manual costly Large4096 supported-profile real daemon process restart"]
fn large_scale_source_survives_real_daemon_process_restart_and_delete() -> TestResult {
    run_scale_process_restart(ScaleTier::Large)
}

#[test]
#[ignore = "manual costly XL32768 supported-profile real daemon process restart"]
fn xlarge_scale_source_survives_real_daemon_process_restart_and_delete() -> TestResult {
    run_scale_process_restart(ScaleTier::Xlarge)
}

fn start_process(
    state: &std::path::Path,
    tier: ScaleTier,
) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
    if tier == ScaleTier::Medium {
        return SearchdBinaryProcess::start_with_history_max_generations(state, 2);
    }
    let mut command = crate::searchd_binary_process::searchd_command(state, 2);
    let _configured = command
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            SCALE_HISTORY_MAX_BYTES.to_string(),
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            SCALE_HISTORY_MAX_TOTAL_BYTES.to_string(),
        )
        .env("QUANTA_INDEX_INGEST_MAX_RECORDS", "100000")
        .env(
            "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            (128_u64 * 1024 * 1024).to_string(),
        )
        .env(
            "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
            (256_u64 * 1024 * 1024).to_string(),
        )
        .env(
            "QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES",
            quanta_index_contract::SOURCE_PUBLICATION_UPLOAD_MAX_BYTES.to_string(),
        )
        .env(
            "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES",
            (4_u64 * 1024 * 1024 * 1024).to_string(),
        );
    SearchdBinaryProcess::start_with_command_and_timeout(state, command, SUPPORTED_PROFILE_TIMEOUT)
}

fn connect_process(
    process: &SearchdBinaryProcess,
    state: &std::path::Path,
    tier: ScaleTier,
) -> Result<QuantaIndex, Box<dyn Error>> {
    if tier == ScaleTier::Medium {
        return process.connect();
    }
    Ok(QuantaIndex::connect(
        ConnectOptions::from_state_root(state).with_request_io_timeout(SUPPORTED_PROFILE_TIMEOUT),
    )?)
}

fn run_scale_process_restart(tier: ScaleTier) -> TestResult {
    let files = generate_scoped_corpus(tier, SEED)?;
    let truth = source_truth(&files, tier)?;
    let state = private_tempdir()?;
    let first = start_process(state.path(), tier)?;
    let client = connect_process(&first, state.path(), tier)?;
    let (_full_receipt, first_head) = client
        .search_corpus()
        .publish_and_activate(&initial_batch(files, tier)?, None)?;
    if first_head.active.generation.lexical.manifest_generation != ManifestGeneration::new(1) {
        return Err("full source generation did not activate".into());
    }
    wait_ready(&client, tier)?;
    let _retained_before_delete = unique_hit(
        &client,
        RETAINED_TOKEN,
        1,
        ("repo1", "src/file_0.rs"),
        &truth,
    )?;
    let _deleted_before_delete = unique_hit(
        &client,
        DELETED_TOKEN,
        1,
        ("repo0", "src/file_0.rs"),
        &truth,
    )?;
    let (_delta_receipt, second_head) = client
        .search_corpus()
        .publish_and_activate(&tombstone_batch()?, Some(first_head.active))?;
    if second_head.active.generation.lexical.manifest_generation != ManifestGeneration::new(2) {
        return Err("deleted source generation did not activate".into());
    }
    let before_restart = observe_generation_two(&client, &truth, tier)?;
    drop(client);
    first.stop()?;
    let reopened = start_process(state.path(), tier)?;
    let reopened_client = connect_process(&reopened, state.path(), tier)?;
    let after_restart = observe_generation_two(&reopened_client, &truth, tier)?;
    if before_restart != after_restart {
        return Err(
            "ranked identities, source commitments, or scores changed across process restart"
                .into(),
        );
    }
    drop(reopened_client);
    reopened.stop()?;
    Ok(())
}
