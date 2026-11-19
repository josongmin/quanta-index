//! Exact on-disk corpus through the product Native UDS SDK.
//! This is an independent diagnostic rail; synthetic scale tier artifacts retain their contract.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use nix::fcntl::OFlag;
use quanta_index_contract::index::{
    CatalogView, CoverageStatus, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile,
    InputDomain, InstanceSpec, ObjectRef, OperationStatus, OwnerMutation, PublicationGroupId,
    SchemaId, SourceEvent, SourceInput, SourceTextAdmission, SourceTransition, StorageFormatId,
    SubmitOutcome, UpdateKind, UpdateRequest, ViewRef, ViewSelection,
};
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_SOURCE_OWNER_LOOKUP, NativeFilePathEntry, NativeIndexRequest, NativeIndexResponse,
};
use quanta_index_contract::{
    QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_embed::{LOCAL_CODE_DIMENSION, LOCAL_CODE_MODEL_ID, LOCAL_CODE_MODEL_REVISION};
use quanta_index_sdk::native_uds::{
    BorrowedSourceInput, MAX_NATIVE_STAGE_SOURCES_INPUTS, MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES,
    NativeUdsClient, NativeUdsError,
};
use quanta_index_searchd::DiagnosticCaptureLimitsV1;
use quanta_index_semantic::native::{
    EmbeddingInputPolicy, EmbeddingRecipe, Utf8ChunkPolicy, VectorNormalization,
    validate_semantic_recipe_instance,
};
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

/// Explicit benchmark binding for a configured Native local-code owner.
/// The daemon owns embedding execution and validates this spec at Reconfigure.
pub struct NativeSemanticCorpusInput<'a> {
    pub config_path: &'a Path,
    pub config_sha256: &'a str,
    pub socket_path: &'a Path,
    pub instance: [u8; 16],
    pub kind: [u8; 16],
    pub group: [u8; 16],
    pub schema: [u8; 16],
    pub format: [u8; 16],
    pub max_source_bytes: u64,
    pub source_universe_sha256: &'a str,
    pub source_binding_sha256: &'a str,
    pub wait_seconds: u64,
}

struct AdmittedSemanticCorpus {
    config: PathBuf,
    socket: PathBuf,
    state_root: PathBuf,
    config_sha256: String,
    instance: InstanceSpec,
    source_universe_sha256: String,
    source_binding_sha256: String,
    wait_seconds: u64,
    max_source_bytes: u64,
    input_kind: &'static str,
    max_utf8_bytes: Option<u32>,
    max_vectors: u32,
}

impl AdmittedSemanticCorpus {
    fn verify_config(&self) -> Result<()> {
        anyhow::ensure!(
            digest(&fs::read(&self.config)?) == self.config_sha256,
            "Native Semantic config changed during benchmark execution"
        );
        Ok(())
    }
}

fn lower_sha256(raw: &str, name: &str) -> Result<()> {
    anyhow::ensure!(
        raw.len() == 64
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{name} must be 64 lowercase hex digits"
    );
    Ok(())
}

fn source_bytes_digest(files: &[SourceInput]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"qidx.native.corpus.source.bytes.v1\n");
    for file in files {
        hash.update((file.path.len() as u64).to_be_bytes());
        hash.update(file.path.as_bytes());
        hash.update((file.bytes.len() as u64).to_be_bytes());
        hash.update(&file.bytes);
    }
    format!("{:x}", hash.finalize())
}

/// Mirror only the public recipe composition at the Native product boundary.
/// The daemon still parses and validates its own complete Semantic config.
fn semantic_recipe_from_config(
    input: Option<&Value>,
    max_source_bytes: u64,
) -> Result<(EmbeddingRecipe, &'static str)> {
    let (input_kind, chunks, max_vectors) = match input {
        None => ("source_utf8", None, 1),
        Some(value) => {
            let fields = value
                .as_object()
                .context("Native Semantic input must be an object")?;
            match fields.get("kind").and_then(Value::as_str) {
                Some("source_utf8") if fields.len() == 1 => ("source_utf8", None, 1),
                Some("file_local_chunks") if fields.len() == 3 => {
                    let max_utf8_bytes = fields
                        .get("max_utf8_bytes")
                        .and_then(Value::as_u64)
                        .context("file_local_chunks requires max_utf8_bytes as u32")?;
                    let max_vectors = fields
                        .get("max_vectors")
                        .and_then(Value::as_u64)
                        .context("file_local_chunks requires max_vectors as u32")?;
                    let policy = Utf8ChunkPolicy {
                        max_utf8_bytes: u32::try_from(max_utf8_bytes)?,
                    };
                    policy.validate()?;
                    (
                        "file_local_chunks",
                        Some(policy),
                        u32::try_from(max_vectors)?,
                    )
                }
                _ => anyhow::bail!(
                    "Native Semantic corpus input must be source_utf8 or exact file_local_chunks"
                ),
            }
        }
    };
    let recipe = EmbeddingRecipe::from_text_model(
        LOCAL_CODE_MODEL_ID,
        LOCAL_CODE_MODEL_REVISION,
        u32::try_from(LOCAL_CODE_DIMENSION)?,
        VectorNormalization::L2Unit,
        EmbeddingInputPolicy::SourceUtf8,
        chunks,
        max_vectors,
        max_source_bytes,
    )?;
    Ok((recipe, input_kind))
}

impl NativeSemanticCorpusInput<'_> {
    fn preflight(&self) -> Result<AdmittedSemanticCorpus> {
        lower_sha256(self.config_sha256, "native config SHA-256")?;
        lower_sha256(self.source_universe_sha256, "source universe SHA-256")?;
        lower_sha256(self.source_binding_sha256, "source binding SHA-256")?;
        anyhow::ensure!(
            self.config_path.is_absolute() && self.socket_path.is_absolute(),
            "Native Semantic config and socket paths must be absolute"
        );
        anyhow::ensure!(
            (1..=3600).contains(&self.wait_seconds),
            "Semantic coverage wait must be 1..=3600 seconds"
        );
        let metadata = fs::symlink_metadata(self.config_path)?;
        anyhow::ensure!(
            metadata.file_type().is_file() && metadata.len() <= 64 * 1024,
            "Native Semantic config must be an absolute regular file <=64KiB"
        );
        let bytes = fs::read(self.config_path)?;
        anyhow::ensure!(
            digest(&bytes) == self.config_sha256,
            "Native Semantic config SHA-256 differs"
        );
        let config: Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            config
                .pointer("/semantic/provider/kind")
                .and_then(Value::as_str)
                == Some("local_code")
                && config.pointer("/owned_work").is_some_and(Value::is_object)
                && config
                    .pointer("/read_sessions")
                    .is_some_and(Value::is_object)
                && config
                    .pointer("/semantic/provider/egress/source_content_consent")
                    .and_then(Value::as_bool)
                    == Some(true),
            "Native Semantic requires a configured local-code provider and owned-work pump"
        );
        let selected_scope = json!({"workspace": workspace(), "epoch": 1});
        for path in ["/semantic/scopes", "/owned_work/scopes"] {
            anyhow::ensure!(
                config
                    .pointer(path)
                    .and_then(Value::as_array)
                    .is_some_and(|scopes| scopes.iter().any(|scope| scope == &selected_scope)),
                "Native Semantic provider/pump lacks the selected workspace scope"
            );
        }
        let root = config
            .pointer("/state_root")
            .and_then(Value::as_str)
            .context("Native Semantic config lacks state_root")?;
        let socket = config
            .pointer("/socket_path")
            .and_then(Value::as_str)
            .context("Native Semantic config lacks socket_path")?;
        let state_root = PathBuf::from(root);
        anyhow::ensure!(
            state_root.is_absolute() && socket == self.socket_path.to_string_lossy(),
            "Native Semantic config state root or socket differs from selected caller"
        );
        anyhow::ensure!(
            !state_root.exists() && !self.socket_path.exists(),
            "Native Semantic benchmark requires a fresh state root and socket"
        );
        let limits = config
            .pointer("/work_limits")
            .context("Native work limits absent")?;
        anyhow::ensure!(
            limits.get("reads").and_then(Value::as_u64) == Some(1_000_000)
                && limits.get("writes").and_then(Value::as_u64) == Some(1_000_000)
                && limits.get("bytes").and_then(Value::as_u64) == Some(536_870_912)
                && limits.get("mutations").and_then(Value::as_u64) == Some(1_000_000)
                && config
                    .pointer("/resources/physical/inventory/max_files")
                    .and_then(Value::as_u64)
                    == Some(30_000),
            "Native Semantic benchmark work/file caps differ from the declared SDK60 contract"
        );
        for class in ["capacity", "interactive", "enrichment"] {
            anyhow::ensure!(
                config
                    .pointer(&format!("/resources/execution/{class}/provider_units"))
                    .and_then(Value::as_u64)
                    .is_some_and(|units| units > 0),
                "Native Semantic {class} provider budget is absent"
            );
        }
        anyhow::ensure!(
            config
                .pointer("/semantic/max_source_bytes")
                .and_then(Value::as_u64)
                == Some(self.max_source_bytes),
            "Native Semantic max_source_bytes differs from selected recipe"
        );
        for (name, expected) in [
            ("kind", self.kind),
            ("schema", self.schema),
            ("format", self.format),
        ] {
            anyhow::ensure!(
                config.pointer(&format!("/semantic/descriptor/{name}")) == Some(&json!(expected)),
                "Native Semantic descriptor {name} differs from selected profile"
            );
        }
        let (recipe, input_kind) =
            semantic_recipe_from_config(config.pointer("/semantic/input"), self.max_source_bytes)?;
        let instance = InstanceSpec {
            instance: IndexInstanceId(self.instance),
            kind: IndexKind(self.kind),
            group: PublicationGroupId(self.group),
            activation: GroupActivation::AsyncCoverage,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId(self.schema),
            policy: recipe.fingerprint(),
            format: StorageFormatId(self.format),
        };
        validate_semantic_recipe_instance(&recipe, &instance)?;
        anyhow::ensure!(
            instance.instance != text_spec().instance
                && instance.instance != file_spec().instance
                && instance.instance != literal_spec().instance
                && instance.group != text_spec().group
                && instance.group != file_spec().group
                && instance.group != literal_spec().group,
            "Native Semantic instance/group collides with a Core instance"
        );
        Ok(AdmittedSemanticCorpus {
            config: self.config_path.to_path_buf(),
            socket: self.socket_path.to_path_buf(),
            state_root,
            config_sha256: self.config_sha256.to_owned(),
            instance,
            source_universe_sha256: self.source_universe_sha256.to_owned(),
            source_binding_sha256: self.source_binding_sha256.to_owned(),
            wait_seconds: self.wait_seconds,
            max_source_bytes: self.max_source_bytes,
            input_kind,
            max_utf8_bytes: recipe.chunk_policy.map(|policy| policy.max_utf8_bytes),
            max_vectors: recipe.max_vectors,
        })
    }
}

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

fn trace_sha256(path: &Path, cap: u64) -> Result<String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits())
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "Native trace is not a regular file");
    let length = metadata.len();
    anyhow::ensure!(
        length > 0 && length <= cap,
        "Native trace length is out of range"
    );
    let mut hash = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    let mut remaining = length;
    while remaining > 0 {
        let read = file.read(&mut buffer)?;
        anyhow::ensure!(read > 0, "Native trace ended before its recorded length");
        remaining = remaining
            .checked_sub(u64::try_from(read)?)
            .context("Native trace grew during readback")?;
        hash.update(
            buffer
                .get(..read)
                .context("Native trace read exceeds buffer")?,
        );
    }
    anyhow::ensure!(
        file.read(&mut buffer)? == 0,
        "Native trace grew during readback"
    );
    Ok(format!("{:x}", hash.finalize()))
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
            query_occurrences = query_occurrences
                .checked_add(1)
                .context("corpus query occurrence count overflow")?;
            query_bound = file.path == input.query_path;
        }
        files.push(SourceInput {
            path: file.path,
            bytes: content,
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            language: None,
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
                thread::sleep(Duration::from_millis(50));
            }
            other @ (OperationStatus::Pending(_) | OperationStatus::Unknown) => {
                anyhow::bail!("Native publication status invalid: {other:?}");
            }
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
            && literal
                .hits
                .first()
                .is_some_and(|hit| hit.record.path == path),
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

fn verify_corpus_file(
    entry: &NativeFilePathEntry,
    input: &SourceInput,
    ordinal: usize,
    staged_ref: ObjectRef,
) -> Result<()> {
    // Compute the childless object reference from the independently pinned input bytes.
    let length = u32::try_from(input.bytes.len())?;
    let mut body = Sha256::new();
    body.update(b"quanta-index-immutable-object-v1\0QIOBJ001");
    body.update(length.to_be_bytes());
    body.update(0_u32.to_be_bytes());
    body.update(&input.bytes);
    anyhow::ensure!(
        entry.path == input.path
            && entry.owner == owner(ordinal)?
            && entry.source == staged_ref
            && entry.bytes == ObjectRef(Digest(body.finalize().into()))
            && entry.byte_len == u64::from(length),
        "Native FileMap witness differs from pinned source {}",
        input.path
    );
    Ok(())
}

fn verify_corpus_inventory(
    client: &NativeUdsClient,
    view: &CatalogView,
    files: &[SourceInput],
    staged_refs: &[ObjectRef],
) -> Result<()> {
    anyhow::ensure!(
        files.len() == staged_refs.len(),
        "Native staged source count differs"
    );
    let budget = RequestBudgetV1::for_duration(SDK_TIMEOUT);
    let selection = ViewSelection::Exact(view.identity);
    let mut expected: Vec<_> = files.iter().zip(staged_refs).enumerate().collect();
    expected.sort_unstable_by(|(_, (left, _)), (_, (right, _))| left.path.cmp(&right.path));
    let mut expected = expected.into_iter();
    let mut after = None;
    loop {
        let page = client.query_paths(
            selection,
            file_spec().instance,
            "",
            after.as_deref(),
            128,
            &budget,
        )?;
        anyhow::ensure!(
            page.view == view.identity && (!page.has_more || !page.entries.is_empty()),
            "Native corpus FileMap view or continuation differs"
        );
        for entry in page.entries {
            let (ordinal, (file, staged_ref)) = expected
                .next()
                .context("Native FileMap has an extra source")?;
            verify_corpus_file(&entry, file, ordinal, *staged_ref)?;
            after = Some(entry.path);
        }
        if !page.has_more {
            break;
        }
    }
    anyhow::ensure!(
        expected.next().is_none(),
        "Native FileMap is missing a pinned source"
    );

    // FileMap already proved the complete path set. Resolve its owners from
    // the same immutable SourceCut, including a retained coverage view.
    for (index, expected) in staged_refs
        .chunks(MAX_NATIVE_SOURCE_OWNER_LOOKUP)
        .enumerate()
    {
        let first = index * MAX_NATIVE_SOURCE_OWNER_LOOKUP;
        let owners = (first..first + expected.len())
            .map(owner)
            .collect::<Result<Vec<_>>>()?;
        let page = client.lookup_source_owners(selection, owners, &budget)?;
        anyhow::ensure!(
            page.view == view.identity
                && page.source == view.source
                && page.rows.as_slice().len() == expected.len(),
            "Native owner map differs from the published SourceCut"
        );
        for (ordinal, (row, staged_ref)) in page.rows.as_slice().iter().zip(expected).enumerate() {
            anyhow::ensure!(
                row.owner == owner(first + ordinal)? && row.source == Some(*staged_ref),
                "Native owner map differs from staged source reference"
            );
        }
    }
    Ok(())
}

/// Wait for the product-owned AsyncCoverage member, not merely the Source ACK.
/// Each SDK request is bounded by both the original SDK and coverage deadlines.
fn wait_semantic_covered_view(
    client: &NativeUdsClient,
    published: &CatalogView,
    semantic: &AdmittedSemanticCorpus,
    query_text: &str,
) -> Result<(ViewRef, Value)> {
    let source_view = published.identity;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(semantic.wait_seconds))
        .context("Native Semantic coverage deadline overflow")?;
    let mut inspected_view = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(
            !remaining.is_zero(),
            "Native Semantic coverage did not complete before the declared wait deadline; Source ACK={source_view:?}"
        );
        let budget = || RequestBudgetV1::for_duration(SDK_TIMEOUT).capped_at(deadline);
        let view = client.read_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        anyhow::ensure!(
            view.source == published.source && view.profile == published.profile,
            "Native Semantic coverage moved to a foreign SourceCut or profile"
        );
        if inspected_view == Some(view.identity) {
            thread::sleep(
                Duration::from_secs(1).min(deadline.saturating_duration_since(Instant::now())),
            );
            continue;
        }
        inspected_view = Some(view.identity);
        let result = client.search_semantic(
            ViewSelection::Exact(view.identity),
            semantic.instance.instance,
            SemanticQueryRequest {
                query_text: query_text.to_owned(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                lexical_scope: None,
                top_k: 1,
            },
            &budget(),
        )?;
        anyhow::ensure!(
            result.view == view.identity
                && result.instance == semantic.instance.instance
                && result.coverage.desired.root == published.source.owners
                && result.coverage.desired.policy == semantic.instance.policy,
            "Native Semantic response differs from the published SourceCut or recipe"
        );
        if matches!(result.coverage.status, CoverageStatus::Complete)
            && result.coverage.covered == Some(result.coverage.desired)
            && result.coverage.searchable.is_some()
            && !result.hits.is_empty()
        {
            return Ok((
                view.identity,
                json!({
                    "coverage": result.coverage,
                    "traversal": result.traversal,
                    "semantic_hits": result.hits.len(),
                    "source_view": source_view,
                    "covered_view": view.identity,
                }),
            ));
        }
        thread::sleep(
            Duration::from_secs(1).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

/// Strict corpus admission is outside the SDK clock. Submit-to-Published is capped at 60 seconds.
pub fn measure_native_corpus(input: &NativeCorpusInput<'_>) -> Result<Value> {
    measure_native_corpus_inner(input, None)
}

/// Benchmark-only opt-in. Reuses the exact source admission, SDK mutation and
/// publication body above. A real configured provider owns all vector work.
pub fn measure_native_corpus_with_semantic(
    input: &NativeCorpusInput<'_>,
    semantic: &NativeSemanticCorpusInput<'_>,
) -> Result<Value> {
    measure_native_corpus_inner(input, Some(semantic))
}

fn measure_native_corpus_inner(
    input: &NativeCorpusInput<'_>,
    semantic: Option<&NativeSemanticCorpusInput<'_>>,
) -> Result<Value> {
    // A missing or altered operator config refuses before output/state creation.
    let semantic = semantic
        .map(NativeSemanticCorpusInput::preflight)
        .transpose()?;
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
    if let Some(semantic) = &semantic {
        anyhow::ensure!(
            files.len() <= 30_000
                && files.iter().all(|file| u64::try_from(file.bytes.len())
                    .is_ok_and(|size| size <= semantic.max_source_bytes)),
            "Native Semantic source exceeds the 30k-file or configured per-owner input bound"
        );
    }
    let source_bytes_sha256 = source_bytes_digest(&files);
    let input_binding_ms = setup_start.elapsed().as_secs_f64() * 1000.0;
    writeln!(
        std::io::stderr().lock(),
        "native_corpus input_bound files={} bytes={} elapsed_ms={input_binding_ms:.1}",
        files.len(),
        source_bytes
    )?;
    let executable = NativeExecutable::capture(input.daemon_bin)?;
    let temp = TempDir::new()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let (root, socket, config) = if let Some(semantic) = &semantic {
        semantic.verify_config()?;
        (
            semantic.state_root.clone(),
            semantic.socket.clone(),
            semantic.config.clone(),
        )
    } else {
        let root = temp.path().join("state");
        let socket = temp.path().join("native.sock");
        let config = temp.path().join("native.json");
        let workload = NativeForegroundWorkload {
            limits: FIXED_FOREGROUND_WORK_LIMITS,
            mode: NativeWorkloadMode::Fixed,
        };
        let mut configured = native_config(&root, &socket, workload);
        *configured
            .pointer_mut("/resources/physical/inventory/max_files")
            .context("Native config inventory file limit absent")? = json!(30_000);
        fs::write(&config, serde_json::to_vec(&configured)?)?;
        (root, socket, config)
    };
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
        writeln!(
            std::io::stderr().lock(),
            "native_corpus first Record trace={}",
            path.display()
        )?;
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
    let mut instances = vec![literal_spec(), file_spec(), text_spec()];
    if let Some(semantic) = &semantic {
        instances.push(semantic.instance.clone());
    }
    let profile = client.stage_profile(
        ingress,
        IndexProfile { instances },
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
    writeln!(
        std::io::stderr().lock(),
        "native_corpus profile_published elapsed_ms={workspace_setup_ms:.1}"
    )?;
    let ingest_start = Instant::now();
    let ingress = client.begin_input(operation(2), &RequestBudgetV1::for_duration(SDK_TIMEOUT))?;
    let staged_at = Instant::now();
    let mut refs = Vec::with_capacity(files.len());
    let mut offset = 0;
    while offset < files.len() {
        let mut weight = 0_usize;
        let mut end = offset;
        while end < files.len()
            && end
                .checked_sub(offset)
                .context("Native source batch range regressed")?
                < MAX_NATIVE_STAGE_SOURCES_INPUTS
        {
            let source = files.get(end).context("Native source batch index absent")?;
            let next = source
                .path
                .len()
                .checked_add(source.bytes.len())
                .and_then(|size| size.checked_add(256))
                .context("Native source wire weight overflow")?;
            anyhow::ensure!(
                next <= MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES,
                "source exceeds Native StageSources frame: {}",
                source.path
            );
            let next_weight = weight
                .checked_add(next)
                .context("Native source batch weight overflow")?;
            if next_weight > MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES {
                break;
            }
            weight = next_weight;
            end = end
                .checked_add(1)
                .context("Native source batch index overflow")?;
        }
        let page = files
            .get(offset..end)
            .context("Native source batch range absent")?
            .iter()
            .map(|file| BorrowedSourceInput {
                path: &file.path,
                bytes: &file.bytes,
                text: file.text,
                language: None,
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
    writeln!(
        std::io::stderr().lock(),
        "native_corpus sources_staged count={} elapsed_ms={stage_ms:.1}",
        refs.len()
    )?;
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
    writeln!(
        std::io::stderr().lock(),
        "native_corpus full_submit_started"
    )?;
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
    writeln!(
        std::io::stderr().lock(),
        "native_corpus full_published elapsed_ms={submit_to_published_ms:.1}"
    )?;
    let published = match client.read_view(
        ViewSelection::Exact(full_view),
        &RequestBudgetV1::for_duration(SDK_TIMEOUT),
    ) {
        Ok(view) => view,
        Err(cause) => {
            let mut no_sampler = None;
            return Err(match first.stop(&mut no_sampler) {
                Ok(()) => cause.into(),
                Err(drain) => anyhow::Error::from(cause).context(format!(
                    "Native daemon drain after FULL readback refusal: {drain:#}"
                )),
            });
        }
    };
    if let Some(semantic) = &semantic {
        fs::write(
            input.output_root.join("source-ack.json"),
            serde_json::to_vec_pretty(&json!({
                "source_view": full_view,
                "source_cut": published.source,
                "profile_ref": published.profile,
                "manifest_sha256": &manifest_digest,
                "source_bytes_sha256": &source_bytes_sha256,
                "source_universe_sha256": &semantic.source_universe_sha256,
                "source_binding_sha256": &semantic.source_binding_sha256,
                "config_sha256": &semantic.config_sha256,
                "semantic_input_kind": semantic.input_kind,
                "semantic_max_utf8_bytes": semantic.max_utf8_bytes,
                "semantic_max_vectors": semantic.max_vectors,
                "semantic_policy": semantic.instance.policy,
                "status": "SOURCE_PUBLISHED_SEMANTIC_NOT_YET_VERIFIED",
            }))?,
        )?;
    }
    let search = match query(&client, full_view, input.query_token, input.query_path) {
        Ok(search) => search,
        Err(cause) if semantic.is_some() => {
            let mut no_sampler = None;
            return Err(match first.stop(&mut no_sampler) {
                Ok(()) => cause,
                Err(drain) => {
                    cause.context(format!("Native daemon drain after Text refusal: {drain:#}"))
                }
            });
        }
        Err(cause) => return Err(cause),
    };
    let stage_to_searchable_ms = ingest_start.elapsed().as_secs_f64() * 1000.0;
    let mut no_sampler = None;
    let verified = (|| -> Result<_> {
        let covered = semantic
            .as_ref()
            .map(|semantic| {
                wait_semantic_covered_view(&client, &published, semantic, input.query_token)
            })
            .transpose()?;
        let inventory_view = if let Some((view, _)) = &covered {
            let selected = client.read_view(
                ViewSelection::Exact(*view),
                &RequestBudgetV1::for_duration(SDK_TIMEOUT),
            )?;
            anyhow::ensure!(
                selected.source == published.source && selected.profile == published.profile,
                "Native inventory moved to a foreign SourceCut or profile"
            );
            selected
        } else {
            published.clone()
        };
        verify_corpus_inventory(&client, &inventory_view, &files, &refs)?;
        Ok((covered, inventory_view.identity))
    })();
    let (covered, inventory_view) = match verified {
        Ok(value) => value,
        Err(cause) => {
            return Err(match first.stop(&mut no_sampler) {
                Ok(()) => cause,
                Err(drain) => cause.context(format!(
                    "Native daemon drain after inventory or coverage refusal: {drain:#}"
                )),
            });
        }
    };
    first.stop(&mut no_sampler)?;
    if let Some(semantic) = &semantic {
        semantic.verify_config()?;
    }
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
        writeln!(
            std::io::stderr().lock(),
            "native_corpus reopen Record trace={}",
            path.display()
        )?;
    }
    let reopen_ms = reopen_at.elapsed().as_secs_f64() * 1000.0;
    let capture_view = covered.as_ref().map_or(full_view, |(view, _)| *view);
    let replay = (|| -> Result<_> {
        let client = NativeUdsClient::new(socket);
        let latest = client.read_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &RequestBudgetV1::for_duration(SDK_TIMEOUT),
        )?;
        let reopened = if semantic.is_none() {
            anyhow::ensure!(
                latest == published,
                "reopened Native head or SourceCut differs from full ACK"
            );
            latest
        } else {
            let selected = client.read_view(
                ViewSelection::Exact(capture_view),
                &RequestBudgetV1::for_duration(SDK_TIMEOUT),
            )?;
            anyhow::ensure!(
                selected.source == published.source
                    && selected.profile == published.profile
                    && latest.source == published.source,
                "reopened Native covered view lost the published SourceCut/profile"
            );
            selected
        };
        let reopen_search = query(&client, capture_view, input.query_token, input.query_path)?;
        verify_corpus_inventory(&client, &reopened, &files, &refs)?;
        let reopen_semantic = semantic
            .as_ref()
            .map(|semantic| {
                let result = client.search_semantic(
                    ViewSelection::Exact(capture_view),
                    semantic.instance.instance,
                    SemanticQueryRequest {
                        query_text: input.query_token.to_owned(),
                        constraints: QueryConstraintSetV1::unconstrained(),
                        generation: None,
                        generation_selector: None,
                        lexical_scope: None,
                        top_k: 1,
                    },
                    &RequestBudgetV1::for_duration(SDK_TIMEOUT),
                )?;
                anyhow::ensure!(
                    matches!(result.coverage.status, CoverageStatus::Complete)
                        && result.coverage.covered == Some(result.coverage.desired)
                        && result.view == capture_view
                        && !result.hits.is_empty(),
                    "reopened Native Semantic coverage is not complete"
                );
                Ok::<Value, anyhow::Error>(json!({
                    "coverage": result.coverage,
                    "semantic_hits": result.hits.len(),
                }))
            })
            .transpose()?;
        let capture_catalog = semantic
            .as_ref()
            .map(|_| {
                client
                    .read_view(
                        ViewSelection::Exact(capture_view),
                        &RequestBudgetV1::for_duration(SDK_TIMEOUT),
                    )
                    .map_err(anyhow::Error::from)
            })
            .transpose()?;
        Ok((reopen_search, reopen_semantic, capture_catalog))
    })();
    let stopped = reopened.stop(&mut no_sampler);
    let (reopen_search, reopen_semantic, capture_catalog) = match (replay, stopped) {
        (Ok(value), Ok(())) => value,
        (Err(cause), Ok(())) => return Err(cause),
        (Ok(_), Err(drain)) => return Err(drain),
        (Err(cause), Err(drain)) => {
            return Err(cause.context(format!(
                "Native daemon drain after reopen refusal: {drain:#}"
            )));
        }
    };
    if let Some(semantic) = &semantic {
        semantic.verify_config()?;
    }
    let record_trace = input
        .trace_max_output_bytes
        .map(|cap| -> Result<Value> {
            let first = first_trace
                .as_deref()
                .context("first Native Record trace is missing")?;
            let reopen = reopen_trace
                .as_deref()
                .context("reopened Native Record trace is missing")?;
            Ok(json!({
                "max_output_bytes": cap,
                "first": first,
                "first_sha256": trace_sha256(first, cap)?,
                "reopen": reopen,
                "reopen_sha256": trace_sha256(reopen, cap)?,
                "verification": "NOT_RUN",
            }))
        })
        .transpose()?;
    let mut result = json!({
        "status":"VERIFIED", "scope":"Native SDK FileMap, literal and text Core index; no Semantic or Hybrid claim",
        "repository_commit":input.repository_commit,"manifest_sha256":manifest_digest,
        "source_root":input.source_root,"file_count":files.len(),"source_bytes":source_bytes,
        "daemon_sha256":executable.sha256(),"inventory_max_files":30_000,
        "input_binding_ms":input_binding_ms,"daemon_boot_ms":daemon_boot_ms,"workspace_setup_ms":workspace_setup_ms,
        "stage_sources_ms":stage_ms,"mutation_stage_ms":mutation_stage_ms,"submit_to_published_ms":submit_to_published_ms,
        "stage_to_searchable_ms":stage_to_searchable_ms,"search":search,
        "reopen_ms":reopen_ms,"reopen_search":reopen_search,
        "published_inventory": {"file_count": files.len(), "view": inventory_view, "source_cut": published.source},
        "source_bytes_sha256": source_bytes_sha256,
        "semantic_hybrid_status":"NOT_RUN",
        "record_trace":record_trace,
    });
    if let Some(semantic) = &semantic {
        let (covered_view, first_semantic) =
            covered.context("Native Semantic coverage result absent")?;
        result["scope"] = json!(
            "Native SDK source/Text/File and actual configured Semantic coverage; Hybrid ranking remains a separate capture"
        );
        result["semantic_hybrid_status"] = json!("SEMANTIC_COVERED_HYBRID_NOT_RUN");
        result["source_full_view"] = json!(full_view);
        result["capture_view"] = json!(covered_view);
        result["semantic_instance"] = json!(semantic.instance.instance);
        result["text_instance"] = json!(text_spec().instance);
        result["file_instance"] = json!(file_spec().instance);
        result["semantic_policy"] = json!(semantic.instance.policy);
        result["semantic_input_kind"] = json!(semantic.input_kind);
        result["semantic_max_utf8_bytes"] = json!(semantic.max_utf8_bytes);
        result["semantic_max_vectors"] = json!(semantic.max_vectors);
        result["capture_catalog"] =
            json!(capture_catalog.context("Native capture CatalogView absent")?);
        result["semantic_coverage"] = first_semantic;
        result["reopen_semantic"] = reopen_semantic.context("reopened Semantic result absent")?;
        result["source_universe_sha256"] = json!(semantic.source_universe_sha256);
        result["source_binding_sha256"] = json!(semantic.source_binding_sha256);
        result["source_binding_status"] =
            json!("PROVIDED_DIGESTS_REQUIRE_INDEPENDENT_EVALUATOR_VALIDATION");
        result["native_config_sha256"] = json!(semantic.config_sha256);
        result["native_config_path"] = json!(semantic.config);
        result["native_socket"] = json!(semantic.socket);
        result["native_state_root"] = json!(root);
    }
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
    fn native_corpus_file_witness_rejects_wrong_owner_ref_or_body() -> Result<()> {
        let source = SourceInput {
            path: "a.go".into(),
            bytes: b"abc".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            language: None,
        };
        let reference = ObjectRef(Digest([11; 32]));
        let body = ObjectRef(Digest([
            0xe5, 0x32, 0xac, 0x9d, 0x7d, 0xb7, 0x75, 0x92, 0xe8, 0xa1, 0x9c, 0x2a, 0xa4, 0x18,
            0xf2, 0x79, 0x55, 0x65, 0xd5, 0x15, 0x99, 0x14, 0x45, 0xfb, 0xd5, 0x0f, 0x40, 0x1b,
            0x30, 0x0d, 0x2c, 0x24,
        ]));
        let entry = NativeFilePathEntry {
            owner: owner(0)?,
            path: source.path.clone(),
            source: reference,
            bytes: body,
            byte_len: 3,
        };
        verify_corpus_file(&entry, &source, 0, reference)?;
        for changed in [
            NativeFilePathEntry {
                owner: owner(1)?,
                ..entry.clone()
            },
            NativeFilePathEntry {
                source: ObjectRef(Digest([12; 32])),
                ..entry.clone()
            },
            NativeFilePathEntry {
                bytes: ObjectRef(Digest([13; 32])),
                ..entry.clone()
            },
            NativeFilePathEntry {
                byte_len: 4,
                ..entry.clone()
            },
            NativeFilePathEntry {
                path: "b.go".into(),
                ..entry.clone()
            },
        ] {
            anyhow::ensure!(verify_corpus_file(&changed, &source, 0, reference).is_err());
        }
        Ok(())
    }

    fn digest_hex(value: Digest) -> String {
        value.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn semantic_source_utf8_recipe_keeps_pinned_whole_file_identity() -> Result<()> {
        let (default_recipe, kind) = semantic_recipe_from_config(None, 1024)?;
        let (explicit_recipe, explicit_kind) =
            semantic_recipe_from_config(Some(&json!({"kind":"source_utf8"})), 1024)?;
        anyhow::ensure!(kind == "source_utf8" && explicit_kind == kind);
        anyhow::ensure!(default_recipe == explicit_recipe);
        anyhow::ensure!(default_recipe.chunk_policy.is_none() && default_recipe.max_vectors == 1);
        anyhow::ensure!(
            digest_hex(default_recipe.fingerprint())
                == "7d82b7075f51785fed6abfa1a7ffebdb69f1cb21bb02c021025c7329616001cf",
            "SourceUtf8 recipe identity drifted"
        );
        Ok(())
    }

    #[test]
    fn semantic_file_local_chunks_recipe_binds_policy_and_exact_capacity() -> Result<()> {
        let (recipe, kind) = semantic_recipe_from_config(
            Some(&json!({
                "kind":"file_local_chunks", "max_utf8_bytes":4096, "max_vectors":8,
            })),
            1024,
        )?;
        anyhow::ensure!(kind == "file_local_chunks");
        anyhow::ensure!(
            recipe.chunk_policy
                == Some(Utf8ChunkPolicy {
                    max_utf8_bytes: 4096
                })
        );
        anyhow::ensure!(recipe.max_vectors == 8 && recipe.max_source_bytes == 1024);
        anyhow::ensure!(
            digest_hex(recipe.output_policy)
                == "e13cd19b41a67899982906a865ea1ece2a19c9a7977f469a1a8560bafcedb62a",
            "FileLocalChunks output policy drifted"
        );
        anyhow::ensure!(
            digest_hex(recipe.fingerprint())
                == "7bdb1c69470a882b30ec6bbc81962c6b3f53ee025b76a98b9d72ecde968bfb3c",
            "FileLocalChunks recipe identity drifted"
        );
        Ok(())
    }

    #[test]
    fn semantic_recipe_refuses_wrong_kind_or_overflow_before_daemon() {
        for invalid in [
            json!({"kind":"certified_projection_chunks"}),
            json!({"kind":"source_utf8", "max_vectors":2}),
            json!({"kind":"file_local_chunks", "max_utf8_bytes":0, "max_vectors":8}),
            json!({"kind":"file_local_chunks", "max_utf8_bytes":u64::MAX, "max_vectors":8}),
            json!({"kind":"file_local_chunks", "max_utf8_bytes":4096, "max_vectors":261}),
            json!({"kind":"file_local_chunks", "max_utf8_bytes":4096, "max_vectors":u64::MAX}),
        ] {
            assert!(
                semantic_recipe_from_config(Some(&invalid), 1024).is_err(),
                "accepted {invalid}"
            );
        }
    }

    fn semantic_preflight_input<'a>(
        config: &'a Path,
        digest: &'a str,
        socket: &'a Path,
    ) -> NativeSemanticCorpusInput<'a> {
        NativeSemanticCorpusInput {
            config_path: config,
            config_sha256: digest,
            socket_path: socket,
            instance: [31; 16],
            kind: [32; 16],
            group: [33; 16],
            schema: [34; 16],
            format: [35; 16],
            max_source_bytes: 1024,
            source_universe_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            source_binding_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            wait_seconds: 60,
        }
    }

    #[test]
    fn semantic_preflight_refuses_text_only_config_before_daemon_or_state() -> Result<()> {
        let temp = TempDir::new()?;
        let config = temp.path().join("native.json");
        let socket = temp.path().join("native.sock");
        let bytes = serde_json::to_vec(&json!({
            "version": 1,
            "state_root": temp.path().join("state"),
            "socket_path": socket,
            "semantic": null,
            "owned_work": null,
        }))?;
        fs::write(&config, &bytes)?;
        let sha = digest(&bytes);
        let input = semantic_preflight_input(&config, &sha, &socket);
        anyhow::ensure!(
            input
                .preflight()
                .is_err_and(|error| error.to_string().contains("local-code provider")),
            "Text-only config entered Semantic corpus publication"
        );
        anyhow::ensure!(
            !temp.path().join("state").exists(),
            "preflight created Native state"
        );
        Ok(())
    }

    #[test]
    fn semantic_preflight_refuses_source_only_without_owned_work() -> Result<()> {
        let temp = TempDir::new()?;
        let config = temp.path().join("native.json");
        let socket = temp.path().join("native.sock");
        let bytes = serde_json::to_vec(&json!({
            "semantic": {
                "provider": {"kind":"local_code", "egress":{"source_content_consent":true}},
                "input":{"kind":"source_utf8"}
            },
            "read_sessions": {},
            "owned_work": null,
        }))?;
        fs::write(&config, &bytes)?;
        let sha = digest(&bytes);
        let input = semantic_preflight_input(&config, &sha, &socket);
        anyhow::ensure!(
            input
                .preflight()
                .is_err_and(|error| error.to_string().contains("owned-work pump")),
            "source-only Semantic config entered Native publication"
        );
        anyhow::ensure!(!socket.exists(), "preflight bound a Native socket");
        Ok(())
    }

    #[test]
    fn semantic_preflight_refuses_changed_operator_config_before_daemon() -> Result<()> {
        let temp = TempDir::new()?;
        let config = temp.path().join("native.json");
        let socket = temp.path().join("native.sock");
        fs::write(&config, b"{}")?;
        let original = digest(b"{}");
        fs::write(&config, b"{\"changed\":true}")?;
        let input = semantic_preflight_input(&config, &original, &socket);
        anyhow::ensure!(
            input
                .preflight()
                .is_err_and(|error| error.to_string().contains("SHA-256 differs")),
            "changed operator config entered Native publication"
        );
        Ok(())
    }

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
