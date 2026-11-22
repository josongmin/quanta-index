//! Pinned external Legacy reference: source/rank restart compatibility.
//! This is not the Native Large product-acceptance rail.

use std::{error::Error, fs, path::Path, time::Duration};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, ProcessRequestEventPlaneV1, ProcessRequestEventStageV1,
    ProcessRequestEventsRequestV1, ProcessRequestEventsV1, QueryConstraintSetV1, RepoId,
    RevisionId, SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1,
    legacy_reference::{
        LegacySearchdBinary, LegacySearchdCommand, SearchdBinaryProcess, daemon_socket_paths,
        publish_legacy_source,
    },
    semantic_source_scopes_for_chunk_records,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T, field: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{field}: expected {expected:?}, got {actual:?}").into())
    }
}

fn private_short_root() -> Result<tempfile::TempDir, Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::Builder::new().prefix("qlr-").tempdir_in("/tmp")?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

fn legacy_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    state_root: &Path,
    history_generations: usize,
    history_bytes: u64,
    trace: Option<&Path>,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    let mut command = binary.command()?;
    let _configured = command.arg("serve").arg("--state-root").arg(state_root);
    let _configured = command.apply_searchd_env(history_generations);
    let _configured = command.env(
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
        history_bytes.to_string(),
    );
    if let Some(trace) = trace {
        let _configured = command.arg("--diagnostic-trace").arg(trace);
    }
    Ok(command)
}

fn ingest_failure_event_tail_from_socket(
    control_socket: &std::path::Path,
) -> Result<ProcessRequestEventsV1, Box<dyn Error>> {
    let request = SearchPlaneControlIpcRequestEnvelope {
        request_id: u64::MAX,
        diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
        payload: SearchPlaneControlIpcRequest::ProcessRequestEventsV1(
            ProcessRequestEventsRequestV1 {
                plane: ProcessRequestEventPlaneV1::Ingest,
                limit: 64,
            },
        ),
    };
    let policy = quanta_index_ipc::ClientIoPolicy::try_new(Duration::from_secs(5))?;
    let response: SearchPlaneControlIpcResponseEnvelope =
        quanta_index_ipc::send_request(control_socket, &request, policy)?;
    require_eq(
        &response.request_id,
        &request.request_id,
        "failure event request ID",
    )?;
    if let SearchPlaneControlIpcResponse::ProcessRequestEventsV1(events) = response.payload {
        require_eq(
            &events.plane,
            &ProcessRequestEventPlaneV1::Ingest,
            "failure event plane",
        )?;
        Ok(events)
    } else {
        Err(format!("failure event read returned {:?}", response.payload).into())
    }
}

#[derive(Debug)]
struct LargeFixtureCloseoutError {
    primary: Box<dyn Error>,
    detail: String,
}

impl std::fmt::Display for LargeFixtureCloseoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}; {}", self.primary, self.detail)
    }
}

impl Error for LargeFixtureCloseoutError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

fn large_fixture_closeout_error(primary: Box<dyn Error>, detail: String) -> Box<dyn Error> {
    Box::new(LargeFixtureCloseoutError { primary, detail })
}

#[test]
fn large_fixture_closeout_preserves_primary_source_and_secondary_detail() -> TestResult {
    #[derive(Debug)]
    struct PrimarySentinel;
    impl std::fmt::Display for PrimarySentinel {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("primary sentinel")
        }
    }
    impl Error for PrimarySentinel {}

    let error = large_fixture_closeout_error(
        Box::new(PrimarySentinel),
        "daemon teardown also failed: secondary sentinel; diagnostic close: close sentinel".into(),
    );
    if error
        .source()
        .is_none_or(|source| source.downcast_ref::<PrimarySentinel>().is_none())
    {
        return Err("large fixture primary error source was lost".into());
    }
    let displayed = error.to_string();
    for marker in ["primary sentinel", "secondary sentinel", "close sentinel"] {
        if !displayed.contains(marker) {
            return Err(format!("large fixture closeout lost {marker}").into());
        }
    }
    Ok(())
}

fn validate_large_fixture_trace_path(output: &std::path::Path) -> TestResult {
    if !output.is_absolute() {
        return Err("Large fixture trace requires an absolute new output path".into());
    }
    match std::fs::symlink_metadata(output) {
        Ok(_) => return Err("Large fixture trace requires an absolute new output path".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = output.parent().ok_or("Large fixture trace has no parent")?;
    let parent = parent.canonicalize()?;
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    if parent.starts_with(checkout) {
        return Err("Large fixture trace must be outside the checkout".into());
    }
    Ok(())
}

fn large_fixture_restart_trace_paths(
    output: Option<&std::ffi::OsString>,
) -> Result<Option<[std::path::PathBuf; 2]>, Box<dyn Error>> {
    let Some(output) = output else {
        return Ok(None);
    };
    let paths = [".restart1", ".restart2"].map(|suffix| {
        let mut path = output.clone();
        path.push(suffix);
        std::path::PathBuf::from(path)
    });
    for path in &paths {
        validate_large_fixture_trace_path(path)?;
    }
    Ok(Some(paths))
}

fn large_fixture_sdk_batch(
    files: &[quanta_index_searchd_harness::source_fixture::ScopedFile],
) -> Result<quanta_index_sdk::SearchCorpusBatch, Box<dyn Error>> {
    use quanta_index_contract::{
        ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
        SourceFileKey, SourcePublicationEvent, lex::LanguageCode,
    };
    use sha2::Digest as _;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let mut batch = quanta_index_sdk::SearchCorpusBatch::replace_generation(
        repo.clone(),
        revision.clone(),
        ManifestGeneration::new(1),
        "lex-seal:1",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".to_owned(),
        event_id: "e2e-source-event-1".to_owned(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    let mut source_order = (0..files.len()).collect::<Vec<_>>();
    source_order.sort_by(|&left, &right| {
        files[left]
            .source_repo_id
            .cmp(&files[right].source_repo_id)
            .then(
                files[left]
                    .repo_relative_path
                    .cmp(&files[right].repo_relative_path),
            )
    });
    for index in source_order {
        let file = &files[index];
        let source_repo_id = RepoId::new(file.source_repo_id.as_str())?;
        let path = RepoRelativePath::new(file.repo_relative_path.as_str());
        let record = ChunkRecord {
            chunk_id: ChunkId::new(format!("e2e-{}-{}", index + 1, path.as_str())),
            repo_relative_path: path.clone(),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(file.content.len())?,
            start_line: 1,
            end_line: u32::try_from(file.content.lines().count())?,
            text: file.content.clone().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(source_repo_id.clone()),
        };
        let scope = fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id,
                repo_relative_path: path,
            },
            revision.clone(),
            vec![record],
            Vec::new(),
        )?;
        let source_sha256: [u8; 32] = sha2::Sha256::digest(&scope.source_bytes).into();
        let independent_sha256: [u8; 32] =
            sha2::Sha256::digest(format!("{}\n", file.content).as_bytes()).into();
        if source_sha256 != independent_sha256 {
            return Err("large SDK source bytes changed from the fixed fixture oracle".into());
        }
        for semantic in semantic_source_scopes_for_chunk_records(&repo, &scope.chunks) {
            batch = batch.replace_semantic_scope(
                semantic.scope,
                semantic.scope_digest,
                semantic.sources,
                semantic.cluster_memberships,
            );
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

#[test]
fn binary_daemon_restart_preserves_active_source_and_ranked_rows() -> TestResult {
    use sha2::Digest as _;

    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let source_path = "src/process_restart.rs";
    let source_text = "fn process_restart_needle() {}";
    let source_sha256: [u8; 32] =
        sha2::Sha256::digest(format!("{source_text}\n").as_bytes()).into();
    let candidate_id = format!("e2e-1-{source_path}");
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let legacy = LegacySearchdBinary::from_environment()?;
    let preparation = legacy.start(&state_root, 8)?;
    let published = (|| -> Result<_, Box<dyn Error>> {
        let client = preparation.connect()?;
        publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[(source_path, source_text, 1)],
            None,
        )
    })();
    let preparation_stopped = preparation.stop();
    let (head, _receipt) = published?;
    preparation_stopped?;
    let pin = GenerationPin::new(repo, revision, head.generation.lexical.manifest_generation);

    let observe =
        |process: &SearchdBinaryProcess,
         request_id: u64|
         -> Result<(String, Vec<quanta_index_contract::LexicalCandidate>), Box<dyn Error>> {
            let client = process.connect()?;
            let ready = wait_for(
                &RealTicker::new(),
                Duration::from_secs(20),
                Duration::from_millis(50),
                "binary daemon ready after process start",
                || client.observability().process_readiness(),
                |report| report.ready,
                |_| true,
            )?;
            require_eq(&ready.active_repositories, &1, "active repositories")?;
            let request = SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "process_restart_needle".to_owned(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 5,
                    cursor: None,
                }),
            };
            let sockets = daemon_socket_paths(&state_root);
            let response: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
                &sockets[0],
                &request,
                quanta_index_ipc::ClientIoPolicy::default(),
            )?;
            require_eq(
                &response.request_id,
                &request_id,
                "query response request ID",
            )?;
            let SearchPlaneQueryIpcResponse::Text(page) = response.payload else {
                return Err(format!("binary restart query returned {:?}", response.payload).into());
            };
            require_eq(&page.generation, &pin, "active query generation")?;
            if page.results.len() != 1 {
                return Err(format!("expected one source-backed row: {:?}", page.results).into());
            }
            let row = page.results.first().ok_or("missing source-backed row")?;
            require_eq(&row.candidate_id, &candidate_id, "source candidate ID")?;
            require_eq(&row.source_repo_id, &pin.repo_id, "source repository")?;
            require_eq(
                &row.repo_relative_path.as_str(),
                &source_path,
                "source relative path",
            )?;
            let source = row.source.as_ref().ok_or("missing source revision")?;
            require_eq(
                &source.source_sha256,
                &source_sha256,
                "source bytes SHA-256",
            )?;
            let events = client
                .observability()
                .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
            if !events.events.iter().any(|event| {
                event.request_id.get() == request_id
                    && event.stage == ProcessRequestEventStageV1::ResponseWritten
            }) {
                return Err("binary restart query has no terminal response event".into());
            }
            Ok((events.process_instance, page.results))
        };

    let first = legacy.start(&state_root, 8)?;
    let first_observed = observe(&first, 0x005e_ed01);
    let first_stopped = first.stop();
    let (first_instance, first_rows) = first_observed?;
    first_stopped?;

    let second = legacy.start(&state_root, 8)?;
    let second_observed = observe(&second, 0x005e_ed02);
    let second_stopped = second.stop();
    let (second_instance, second_rows) = second_observed?;
    second_stopped?;
    if first_instance == second_instance {
        return Err("binary process instance did not change after shutdown and restart".into());
    }
    require_eq(
        &second_rows,
        &first_rows,
        "ranked rows after binary restart",
    )?;
    legacy.verify()?;
    Ok(())
}

/// E4-05: observe every large-tier source after an OS process restart.
///
/// A pinned external Legacy preparation daemon publishes through the SDK, then
/// two fresh processes open the same sealed generation. The old in-process
/// parent-trace input is explicitly refused; SDK and restart traces remain.
/// The fixture supplies independent source bytes, identities and cardinality.
/// Ordered ranking equality across process instances is not a relevance golden.
#[test]
fn binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page() -> TestResult {
    use std::collections::{BTreeMap, BTreeSet};

    use quanta_index_searchd_harness::source_fixture::{
        ScaleTier, generate_scoped_corpus, params_for,
    };
    use sha2::Digest as _;

    type RankedSourceRow = (String, String, [u8; 32], String, u32);

    const SEED: u64 = 5_864_059_738_136_528_177;
    const HISTORY_MAX_BYTES: u64 = 268_435_456;
    const PAGE_SIZE: u32 = 256;
    const QUERY: &str = "scale_needle_token";

    if std::env::var_os("QUANTA_INDEX_LARGE_FIXTURE_TRACE_OUT").is_some() {
        return Err("in-process Large fixture trace is unavailable for the external Legacy reference; use QUANTA_INDEX_LARGE_FIXTURE_SDK_TRACE_OUT".into());
    }
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let params = params_for(ScaleTier::Large);
    let files = generate_scoped_corpus(ScaleTier::Large, SEED)?;
    require_eq(
        &files.len(),
        &usize::try_from(params.total_files())?,
        "large source count",
    )?;
    let mut expected = BTreeMap::new();
    let mut source_files = files.iter();
    for repo_index in 0..params.repo_count {
        for file_index in 0..params.files_per_repo {
            let file = source_files
                .next()
                .ok_or("missing deterministic source fixture")?;
            let source_repo_id = format!("repo{repo_index}");
            let repo_relative_path = format!("src/file_{file_index}.rs");
            require_eq(
                &file.source_repo_id,
                &source_repo_id,
                "fixture source owner",
            )?;
            require_eq(
                &file.repo_relative_path,
                &repo_relative_path,
                "fixture source path",
            )?;
            if !file.content.contains(QUERY) {
                return Err(format!(
                    "fixture source lacks query: {source_repo_id}/{repo_relative_path}"
                )
                .into());
            }
            let source_sha256: [u8; 32] =
                sha2::Sha256::digest(format!("{}\n", file.content).as_bytes()).into();
            if expected
                .insert((source_repo_id, repo_relative_path), source_sha256)
                .is_some()
            {
                return Err("duplicate fixture source identity".into());
            }
        }
    }
    if source_files.next().is_some() {
        return Err("extra deterministic source fixture".into());
    }
    require_eq(&expected.len(), &4096, "independent large identity count")?;

    let sdk_trace_output =
        std::env::var_os("QUANTA_INDEX_LARGE_FIXTURE_SDK_TRACE_OUT").map(std::path::PathBuf::from);
    let boot_trace_output = std::env::var_os("QUANTA_INDEX_LARGE_FIXTURE_BOOT_TRACE_OUT");
    let restart_traces = large_fixture_restart_trace_paths(boot_trace_output.as_ref())?;
    if let Some(path) = &sdk_trace_output {
        validate_large_fixture_trace_path(path)?;
    }
    if let (Some(parent), Some(children)) = (&sdk_trace_output, &restart_traces) {
        let parent = parent
            .parent()
            .ok_or("Large fixture trace has no parent")?
            .canonicalize()?
            .join(
                parent
                    .file_name()
                    .ok_or("Large fixture trace has no filename")?,
            );
        for child in children {
            let child = child
                .parent()
                .ok_or("Large boot trace has no parent")?
                .canonicalize()?
                .join(
                    child
                        .file_name()
                        .ok_or("Large boot trace has no filename")?,
                );
            if child == parent {
                return Err("Large fixture and boot traces must have distinct output paths".into());
            }
        }
    }
    require_eq(&files.len(), &expected.len(), "SDK staged source count")?;
    let batch = large_fixture_sdk_batch(&files)?;
    let legacy = LegacySearchdBinary::from_environment()?;
    let command = legacy_command(
        &legacy,
        &state_root,
        2,
        HISTORY_MAX_BYTES,
        sdk_trace_output.as_deref(),
    )?;
    let preparation_process =
        SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    legacy.verify()?;
    let preparation = (|| -> Result<GenerationPin, Box<dyn Error>> {
        let client = QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(&state_root)
                .with_request_io_timeout(Duration::from_secs(300)),
        )?;
        let (_receipt, activated) = client
            .search_corpus()
            .publish_and_activate(&batch, None)
            .map_err(|error| {
                let control = daemon_socket_paths(&state_root)[1].clone();
                let events = ingest_failure_event_tail_from_socket(&control);
                large_fixture_closeout_error(
                    Box::new(error),
                    format!("large fixture SDK publication failed; ingest event tail: {events:?}"),
                )
            })?;
        let lexical = &activated.active.generation.lexical;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        require_eq(&lexical.repo_id, &repo, "SDK active owner")?;
        require_eq(&lexical.revision_id, &revision, "SDK active revision")?;
        require_eq(
            &lexical.manifest_generation,
            &ManifestGeneration::new(1),
            "SDK active generation",
        )?;
        require_eq(
            &lexical.manifest_digest.as_str(),
            &"lex-seal:1",
            "SDK active manifest digest",
        )?;
        drop(client);
        Ok(GenerationPin::new(
            repo,
            revision,
            lexical.manifest_generation,
        ))
    })();
    let teardown = preparation_process.stop();
    let pin = match (preparation, teardown) {
        (Ok(pin), Ok(())) => pin,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(primary), Err(teardown_error)) => {
            return Err(large_fixture_closeout_error(
                primary,
                format!("preparation daemon teardown also failed: {teardown_error:#}"),
            ));
        }
    };

    let observe = |process: &SearchdBinaryProcess,
                   request_id_base: u64|
     -> Result<(String, Vec<RankedSourceRow>), Box<dyn Error>> {
        let client = process.connect()?;
        let ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(300),
            Duration::from_millis(50),
            "large binary daemon ready after process start",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        require_eq(&ready.active_repositories, &1, "active serving owners")?;
        let sockets = daemon_socket_paths(&state_root);
        let io = quanta_index_ipc::ClientIoPolicy::try_new(Duration::from_secs(300))?;
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        let mut ranked = Vec::with_capacity(expected.len());
        let mut previous: Option<quanta_index_contract::LexicalCandidate> = None;
        // Every admitted page is nonempty and has only new source identities.
        // A byte-cut page may be shorter than PAGE_SIZE, so only the source
        // cardinality bounds the number of pages independently of the engine.
        for page_index in 0..expected.len() {
            let request_id = request_id_base
                .checked_add(u64::try_from(page_index)?)
                .ok_or("large request ID overflow")?;
            let request = SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: QUERY.to_owned(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: PAGE_SIZE,
                    cursor,
                }),
            };
            let response: SearchPlaneQueryIpcResponseEnvelope =
                quanta_index_ipc::send_request(&sockets[0], &request, io)?;
            require_eq(&response.request_id, &request_id, "large page request ID")?;
            let SearchPlaneQueryIpcResponse::Text(page) = response.payload else {
                return Err(format!("large binary query returned {:?}", response.payload).into());
            };
            require_eq(&page.generation, &pin, "large page generation")?;
            if page.results.is_empty() || page.results.len() > usize::try_from(PAGE_SIZE)? {
                return Err(
                    format!("large page has invalid length: {}", page.results.len()).into(),
                );
            }
            require_eq(
                &usize::try_from(page.window.returned())?,
                &page.results.len(),
                "large page returned count",
            )?;
            for row in &page.results {
                let identity = (
                    row.source_repo_id.as_str().to_owned(),
                    row.repo_relative_path.as_str().to_owned(),
                );
                let expected_sha = expected.get(&identity).ok_or("foreign large source row")?;
                if !seen.insert(identity.clone()) {
                    return Err(format!("duplicate large source row: {identity:?}").into());
                }
                let source = row
                    .source
                    .as_ref()
                    .ok_or("large row has no source revision")?;
                require_eq(
                    &source.file.source_repo_id,
                    &row.source_repo_id,
                    "source owner",
                )?;
                require_eq(
                    &source.file.repo_relative_path,
                    &row.repo_relative_path,
                    "source path",
                )?;
                require_eq(&source.source_sha256, expected_sha, "source bytes SHA-256")?;
                require_eq(&row.repo_id, &pin.repo_id, "serving owner")?;
                require_eq(
                    &row.manifest_generation,
                    &pin.manifest_generation,
                    "row generation",
                )?;
                if let Some(prior) = &previous
                    && prior.order_key().order(&row.order_key()) != std::cmp::Ordering::Less
                {
                    return Err("large rows violate strict ranked page order".into());
                }
                previous = Some(row.clone());
                ranked.push((
                    identity.0,
                    identity.1,
                    source.source_sha256,
                    row.candidate_id.clone(),
                    row.score.to_bits(),
                ));
            }
            match (page.window.has_more(), page.next_cursor) {
                (Some(true), Some(next)) if seen.len() < expected.len() => cursor = Some(next),
                (Some(false), None) if seen.len() == expected.len() => {
                    let events = wait_for(
                        &RealTicker::new(),
                        Duration::from_secs(5),
                        Duration::from_millis(10),
                        "large binary query terminal response event",
                        || {
                            client
                                .observability()
                                .request_events(ProcessRequestEventPlaneV1::Query, 1024)
                        },
                        |events| {
                            events.events.iter().any(|event| {
                                event.request_id.get() == request_id
                                    && event.stage == ProcessRequestEventStageV1::ResponseWritten
                            })
                        },
                        |_| false,
                    )?;
                    require_eq(&seen.len(), &expected.len(), "observed large source count")?;
                    return Ok((events.process_instance, ranked));
                }
                other => return Err(format!("large page continuation mismatch: {other:?}").into()),
            }
        }
        Err("large page traversal exceeded the independent source count".into())
    };

    let command = legacy_command(
        &legacy,
        &state_root,
        2,
        HISTORY_MAX_BYTES,
        restart_traces.as_ref().map(|paths| paths[0].as_path()),
    )?;
    let first = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    legacy.verify()?;
    let first_observed = observe(&first, 0x5ca1_e000);
    let first_stopped = first.stop();
    let (first_instance, first_rows) = first_observed?;
    first_stopped?;

    let command = legacy_command(
        &legacy,
        &state_root,
        2,
        HISTORY_MAX_BYTES,
        restart_traces.as_ref().map(|paths| paths[1].as_path()),
    )?;
    let second = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    legacy.verify()?;
    let second_observed = observe(&second, 0x5ca2_e000);
    let second_stopped = second.stop();
    let (second_instance, second_rows) = second_observed?;
    second_stopped?;
    if first_instance == second_instance {
        return Err("large binary process instance did not change".into());
    }
    require_eq(
        &second_rows,
        &first_rows,
        "large ranked rows after binary restart",
    )?;
    legacy.verify()?;
    Ok(())
}
