//! Exact on-disk corpus through the product Native UDS SDK.
//! This is an independent diagnostic rail; synthetic scale tier artifacts retain their contract.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use quanta_index_contract::index::{
    Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile, InputDomain, InstanceSpec,
    OperationStatus, OwnerMutation, PublicationGroupId, SchemaId, SourceEvent, SourceInput,
    SourceTextAdmission, SourceTransition, StorageFormatId, SubmitOutcome, UpdateKind,
    UpdateRequest, ViewRef, ViewSelection,
};
use quanta_index_contract::native_protocol::{NativeIndexRequest, NativeIndexResponse};
use quanta_index_contract::{QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::{
    BorrowedSourceInput, MAX_NATIVE_STAGE_SOURCES_INPUTS, MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES,
    NativeUdsClient, NativeUdsError,
};
use quanta_index_searchd::DiagnosticCaptureLimitsV1;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::workload::{FIXED_FOREGROUND_WORK_LIMITS, NativeForegroundWorkload, NativeWorkloadMode};
use super::{
    NativeDaemon, NativeExecutable, native_config, operation, owner, text_spec, workspace,
};

const SDK_TIMEOUT: Duration = Duration::from_secs(60);
const STAGE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
pub struct NativeCorpusInput<'a> {
    pub source_root: &'a Path,
    pub manifest: &'a Path,
    pub manifest_sha256: &'a str,
    pub repository_commit: &'a str,
    pub query_path: &'a str,
    pub query_token: &'a str,
    pub daemon_bin: &'a Path,
    pub output_root: &'a Path,
    /// None keeps the original Off rail; Some enables bounded daemon Record capture.
    pub trace_max_output_bytes: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    repository_commit: String,
    files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    file_sha256: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn safe_path(path: &str) -> Result<()> {
    anyhow::ensure!(!path.is_empty(), "empty corpus path");
    anyhow::ensure!(
        Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "unsafe corpus path: {path}"
    );
    Ok(())
}

fn actual_paths(root: &Path) -> Result<BTreeSet<String>> {
    fn visit(root: &Path, dir: &Path, found: &mut BTreeSet<String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            anyhow::ensure!(
                !kind.is_symlink(),
                "symlink in exact corpus: {}",
                path.display()
            );
            if kind.is_dir() {
                visit(root, &path, found)?;
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("non UTF-8 corpus path")?;
                let _inserted = found.insert(relative.to_owned());
            } else {
                anyhow::bail!("non-file corpus entry: {}", path.display());
            }
        }
        Ok(())
    }
    let mut found = BTreeSet::new();
    visit(root, root, &mut found)?;
    Ok(found)
}

fn bound_corpus(input: &NativeCorpusInput<'_>) -> Result<(Vec<SourceInput>, u64, String)> {
    anyhow::ensure!(
        input.source_root.is_absolute() && input.manifest.is_absolute(),
        "corpus inputs must be absolute"
    );
    anyhow::ensure!(
        input.manifest_sha256.len() == 64
            && input.manifest_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "manifest SHA-256 must be 64 hex digits"
    );
    let raw = fs::read(input.manifest)?;
    anyhow::ensure!(
        digest(&raw) == input.manifest_sha256.to_ascii_lowercase(),
        "manifest SHA-256 mismatch"
    );
    let manifest: Manifest = serde_json::from_slice(&raw)?;
    anyhow::ensure!(
        manifest.repository_commit == input.repository_commit,
        "repository commit mismatch"
    );
    anyhow::ensure!(!manifest.files.is_empty(), "empty manifest");
    let mut seen = BTreeSet::new();
    let mut files = Vec::with_capacity(manifest.files.len());
    let mut bytes = 0_u64;
    let mut query_occurrences = 0_usize;
    let mut query_bound = false;
    safe_path(input.query_path)?;
    anyhow::ensure!(
        !input.query_token.is_empty()
            && input
                .query_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "query token must be nonempty ASCII word"
    );
    for file in manifest.files {
        safe_path(&file.path)?;
        anyhow::ensure!(
            seen.insert(file.path.clone()),
            "duplicate manifest path: {}",
            file.path
        );
        let content = fs::read(input.source_root.join(&file.path))
            .with_context(|| format!("read pinned source {}", file.path))?;
        anyhow::ensure!(
            digest(&content) == file.file_sha256.to_ascii_lowercase(),
            "source SHA-256 mismatch: {}",
            file.path
        );
        bytes = bytes
            .checked_add(u64::try_from(content.len())?)
            .context("corpus bytes overflow")?;
        if content
            .windows(input.query_token.len())
            .any(|window| window == input.query_token.as_bytes())
        {
            query_occurrences += 1;
            query_bound = file.path == input.query_path;
        }
        files.push(SourceInput {
            path: file.path,
            bytes: content,
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        });
    }
    anyhow::ensure!(
        actual_paths(input.source_root)? == seen,
        "source tree differs from manifest file set"
    );
    anyhow::ensure!(
        query_occurrences == 1 && query_bound,
        "query token must occur in exactly the declared source file"
    );
    Ok((files, bytes, digest(&raw)))
}

fn literal_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn file_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([17; 16]),
        kind: IndexKind([18; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([21; 32]),
        format: StorageFormatId([19; 16]),
    }
}

fn publish(client: &NativeUdsClient, request: UpdateRequest) -> Result<ViewRef> {
    let key = request.operation;
    let budget = RequestBudgetV1::for_duration(SDK_TIMEOUT);
    let outcome = client.submit(request, &budget);
    match outcome {
        Ok(SubmitOutcome::Terminal(OperationStatus::Published(view))) => return Ok(view),
        Ok(SubmitOutcome::Terminal(OperationStatus::Refused(reason))) => {
            anyhow::bail!("Native publication refused: {reason:?}")
        }
        Ok(SubmitOutcome::Accepted(attempt)) if attempt.operation == key => {}
        Ok(SubmitOutcome::Unknown(observed)) | Err(NativeUdsError::Unknown(observed))
            if observed == key => {}
        Err(error) => return Err(error.into()),
        other => anyhow::bail!("Native submit returned invalid outcome: {other:?}"),
    }
    loop {
        budget.checkpoint("native_corpus_publication_status")?;
        match client.status(key, &budget)? {
            OperationStatus::Published(view) => return Ok(view),
            OperationStatus::Refused(reason) => {
                anyhow::bail!("Native publication refused: {reason:?}")
            }
            OperationStatus::Pending(attempt) if attempt.operation == key => {
                thread::sleep(Duration::from_millis(50))
            }
            other => anyhow::bail!("Native publication status invalid: {other:?}"),
        }
    }
}

fn query(client: &NativeUdsClient, view: ViewRef, token: &str, path: &str) -> Result<Value> {
    let budget = RequestBudgetV1::for_duration(SDK_TIMEOUT);
    let started = Instant::now();
    let paths = client.query_paths(
        ViewSelection::Exact(view),
        file_spec().instance,
        path,
        None,
        2,
        &budget,
    )?;
    let file_map_ms = started.elapsed().as_secs_f64() * 1000.0;
    anyhow::ensure!(
        paths.view == view && paths.entries.iter().any(|entry| entry.path == path),
        "FileMap query did not return exact source"
    );
    let started = Instant::now();
    let literal = client.search_literal(
        ViewSelection::Exact(view),
        literal_spec().instance,
        token.as_bytes(),
        None,
        2,
        &budget,
    )?;
    let literal_ms = started.elapsed().as_secs_f64() * 1000.0;
    anyhow::ensure!(
        literal.view == view
            && !literal.has_more
            && literal.hits.len() == 1
            && literal.hits[0].record.path == path,
        "literal query did not return exact source"
    );
    let started = Instant::now();
    let text = client.search_text(
        ViewSelection::Exact(view),
        text_spec().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: token.to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 8,
            cursor: None,
        },
        &budget,
    )?;
    let text_ms = started.elapsed().as_secs_f64() * 1000.0;
    anyhow::ensure!(
        text.view == view && text.exhaustive && text.hits.iter().any(|hit| hit.path == path),
        "text query did not return exact source"
    );
    Ok(
        json!({"file_map_ms":file_map_ms,"literal_ms":literal_ms,"text_ms":text_ms,"literal_hits":literal.hits.len(),"text_matched":text.matched}),
    )
}

/// Strict corpus admission is outside the SDK clock. Submit-to-Published is capped at 60 seconds.
pub fn measure_native_corpus(input: &NativeCorpusInput<'_>) -> Result<Value> {
    if let Some(bytes) = input.trace_max_output_bytes {
        let _limits = DiagnosticCaptureLimitsV1::default()
            .with_max_output_bytes(bytes)
            .context("Native corpus Record capture limit outside supported range")?;
    }
    anyhow::ensure!(
        input.output_root.is_absolute() && !input.output_root.exists(),
        "output root must be a new absolute path"
    );
    let checkout = fs::canonicalize(".")?;
    let output_parent = fs::canonicalize(
        input
            .output_root
            .parent()
            .context("output root has no parent")?,
    )?;
    anyhow::ensure!(
        !output_parent.starts_with(&checkout),
        "output root must be outside checkout"
    );
    fs::create_dir(input.output_root)?;
    let setup_start = Instant::now();
    let (files, source_bytes, manifest_digest) = bound_corpus(input)?;
    let input_binding_ms = setup_start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "native_corpus input_bound files={} bytes={} elapsed_ms={input_binding_ms:.1}",
        files.len(),
        source_bytes
    );
    let executable = NativeExecutable::capture(input.daemon_bin)?;
    let temp = TempDir::new()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config = temp.path().join("native.json");
    let workload = NativeForegroundWorkload {
        limits: FIXED_FOREGROUND_WORK_LIMITS,
        mode: NativeWorkloadMode::Fixed,
    };
    let mut configured = native_config(&root, &socket, workload);
    configured["resources"]["physical"]["inventory"]["max_files"] = json!(30_000);
    fs::write(&config, serde_json::to_vec(&configured)?)?;
    let first_trace = input
        .trace_max_output_bytes
        .map(|_| input.output_root.join("first.trace.jsonl"));
    let reopen_trace = input
        .trace_max_output_bytes
        .map(|_| input.output_root.join("reopen.trace.jsonl"));
    let daemon_start = Instant::now();
    let first = NativeDaemon::start(
        &executable,
        &config,
        &socket,
        first_trace.as_deref(),
        input.trace_max_output_bytes.unwrap_or(0),
        &input.output_root.join("first.stderr"),
    )?;
    if let Some(path) = &first_trace {
        eprintln!("native_corpus first Record trace={}", path.display());
    }
    let daemon_boot_ms = daemon_start.elapsed().as_secs_f64() * 1000.0;
    let client = NativeUdsClient::new(socket.clone());
    let setup = Instant::now();
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &RequestBudgetV1::for_duration(SDK_TIMEOUT),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        anyhow::bail!("Native workspace create returned {created:?}")
    };
    let ingress = client.begin_input(operation(1), &RequestBudgetV1::for_duration(SDK_TIMEOUT))?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec(), text_spec()],
        },
        &RequestBudgetV1::for_duration(SDK_TIMEOUT),
    )?;
    let configured_view = publish(
        &client,
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
    )?;
    let workspace_setup_ms = setup.elapsed().as_secs_f64() * 1000.0;
    eprintln!("native_corpus profile_published elapsed_ms={workspace_setup_ms:.1}");
    let ingest_start = Instant::now();
    let ingress = client.begin_input(operation(2), &RequestBudgetV1::for_duration(SDK_TIMEOUT))?;
    let staged_at = Instant::now();
    let mut refs = Vec::with_capacity(files.len());
    let mut offset = 0;
    while offset < files.len() {
        let mut weight = 0_usize;
        let mut end = offset;
        while end < files.len() && end - offset < MAX_NATIVE_STAGE_SOURCES_INPUTS {
            let next = files[end]
                .path
                .len()
                .checked_add(files[end].bytes.len())
                .and_then(|size| size.checked_add(256))
                .context("Native source wire weight overflow")?;
            anyhow::ensure!(
                next <= MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES,
                "source exceeds Native StageSources frame: {}",
                files[end].path
            );
            if weight
                .checked_add(next)
                .context("Native source batch weight overflow")?
                > MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES
            {
                break;
            }
            weight += next;
            end += 1;
        }
        let page = files[offset..end]
            .iter()
            .map(|file| BorrowedSourceInput {
                path: &file.path,
                bytes: &file.bytes,
                text: file.text,
            })
            .collect::<Vec<_>>();
        let staged = client.stage_sources_borrowed(
            ingress,
            &page,
            &RequestBudgetV1::for_duration(STAGE_TIMEOUT),
        )?;
        anyhow::ensure!(
            staged.len() == page.len(),
            "Native stage_sources omitted refs"
        );
        refs.extend(staged);
        offset = end;
    }
    let stage_ms = staged_at.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "native_corpus sources_staged count={} elapsed_ms={stage_ms:.1}",
        refs.len()
    );
    let prior = client.read_view(
        ViewSelection::Exact(configured_view),
        &RequestBudgetV1::for_duration(SDK_TIMEOUT),
    )?;
    let mutations = refs
        .iter()
        .enumerate()
        .map(|(index, reference)| {
            Ok(OwnerMutation {
                owner: owner(index)?,
                expected: None,
                next: Some(*reference),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mutation_start = Instant::now();
    let owners = client.stage_mutations(
        ingress,
        mutations,
        &RequestBudgetV1::for_duration(SDK_TIMEOUT),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::with_capacity(1),
    };
    transition.events.push(SourceEvent {
        producer: ingress.operation.producer,
        stream: ingress.operation.stream,
        stream_epoch: ingress.operation.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let mutation_stage_ms = mutation_start.elapsed().as_secs_f64() * 1000.0;
    let submit_at = Instant::now();
    eprintln!("native_corpus full_submit_started");
    let full_view = match publish(
        &client,
        UpdateRequest {
            operation: operation(2),
            expected_view: configured_view,
            kind: UpdateKind::SourceAdvance(transition),
        },
    ) {
        Ok(view) => view,
        Err(error) => {
            // A refused Submit is still a completed SDK outcome. Drain the daemon so
            // its Record footer is written before retaining the original error.
            let mut no_sampler = None;
            return Err(match first.stop(&mut no_sampler) {
                Ok(()) => error,
                Err(drain_error) => error.context(format!(
                    "Native trace drain failed after publication refusal: {drain_error:#}"
                )),
            });
        }
    };
    let submit_to_published_ms = submit_at.elapsed().as_secs_f64() * 1000.0;
    eprintln!("native_corpus full_published elapsed_ms={submit_to_published_ms:.1}");
    let search = query(&client, full_view, input.query_token, input.query_path)?;
    let stage_to_searchable_ms = ingest_start.elapsed().as_secs_f64() * 1000.0;
    let mut no_sampler = None;
    first.stop(&mut no_sampler)?;
    let reopen_at = Instant::now();
    let reopened = NativeDaemon::start(
        &executable,
        &config,
        &socket,
        reopen_trace.as_deref(),
        input.trace_max_output_bytes.unwrap_or(0),
        &input.output_root.join("reopen.stderr"),
    )?;
    if let Some(path) = &reopen_trace {
        eprintln!("native_corpus reopen Record trace={}", path.display());
    }
    let reopen_ms = reopen_at.elapsed().as_secs_f64() * 1000.0;
    let client = NativeUdsClient::new(socket);
    let head = client
        .read_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &RequestBudgetV1::for_duration(SDK_TIMEOUT),
        )?
        .identity;
    anyhow::ensure!(
        head == full_view,
        "reopened Native head differs from full ACK"
    );
    let reopen_search = query(&client, head, input.query_token, input.query_path)?;
    reopened.stop(&mut no_sampler)?;
    let result = json!({
        "status":"VERIFIED", "scope":"Native SDK FileMap, literal and text Core index; no Semantic or Hybrid claim",
        "repository_commit":input.repository_commit,"manifest_sha256":manifest_digest,
        "source_root":input.source_root,"file_count":files.len(),"source_bytes":source_bytes,
        "daemon_sha256":executable.sha256(),"inventory_max_files":30_000,
        "input_binding_ms":input_binding_ms,"daemon_boot_ms":daemon_boot_ms,"workspace_setup_ms":workspace_setup_ms,
        "stage_sources_ms":stage_ms,"mutation_stage_ms":mutation_stage_ms,"submit_to_published_ms":submit_to_published_ms,
        "stage_to_searchable_ms":stage_to_searchable_ms,"search":search,
        "reopen_ms":reopen_ms,"reopen_search":reopen_search,
        "semantic_hybrid_status":"NOT_RUN",
        "record_trace":input.trace_max_output_bytes.map(|max_output_bytes| json!({
            "max_output_bytes":max_output_bytes,
            "first":first_trace,
            "reopen":reopen_trace,
            "verification":"NOT_RUN"
        })),
    });
    fs::write(
        input.output_root.join("result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_manifest_rejects_wrong_digest_and_unlisted_file() -> Result<()> {
        let temp = TempDir::new()?;
        let source = temp.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("a.go"), b"uniqueCorpusNeedle")?;
        let manifest = temp.path().join("manifest.json");
        let raw = serde_json::to_vec(&json!({
            "repository_commit":"pinned-commit",
            "files":[{"path":"a.go","file_sha256":digest(b"uniqueCorpusNeedle")}],
        }))?;
        fs::write(&manifest, &raw)?;
        let actual_digest = digest(&raw);
        let input = NativeCorpusInput {
            source_root: &source,
            manifest: &manifest,
            manifest_sha256: &actual_digest,
            repository_commit: "pinned-commit",
            query_path: "a.go",
            query_token: "uniqueCorpusNeedle",
            daemon_bin: Path::new("/not-used"),
            output_root: Path::new("/not-used"),
            trace_max_output_bytes: None,
        };
        anyhow::ensure!(
            bound_corpus(&input)?.0.len() == 1,
            "valid manifest did not bind"
        );
        let wrong_digest = "0".repeat(64);
        let wrong = NativeCorpusInput {
            manifest_sha256: &wrong_digest,
            ..input
        };
        anyhow::ensure!(
            bound_corpus(&wrong).is_err(),
            "wrong manifest digest accepted"
        );
        fs::write(source.join("unlisted.go"), b"extra")?;
        anyhow::ensure!(
            bound_corpus(&input).is_err(),
            "unlisted source file accepted"
        );
        Ok(())
    }
}
