//! Paired SDK quality replay of a blind pack on an already sealed generation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ExactRepoRelativePathV1, ExecutionOutcomeV2, GenerationPin, GoTestTraceFileLaneWitnessV1,
    GoTestTraceFrameClassV1, GoTestTraceModuleCacheFrameV1, GoTestTraceModuleContextV1,
    GoTestTraceSourceFramesV1, LexicalCandidate, ManifestGeneration, QueryResultWindowV2, RepoId,
    RevisionId, TextRankUnit,
};
use quanta_index_retrieval_bench::query_plan::{
    NlPlanConfig, QueryInputPolicy, QueryPlan, execution_profile_id,
};
use quanta_index_retrieval_bench::record::{
    GENERIC_SEMANTIC_REPRESENTATION_POLICY, GO_TRACE_SEMANTIC_PROFILE, GoTraceSemanticInput,
    QueryPack, load_query_pack_with_file_digest,
};
use quanta_index_retrieval_bench::symbols::extract_symbols;
use quanta_index_retrieval_bench::{BenchError, BenchResult, sha256_hex};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use serde_json::{Value, json};

use super::{Args, optional_u64, reject_unknown, required, write_json_bound};

fn bound_window(
    route: &str,
    expected: &GenerationPin,
    actual: &GenerationPin,
    window: &QueryResultWindowV2,
) -> BenchResult<()> {
    if actual != expected {
        return Err(BenchError::Protocol(format!("{route}: generation drift")));
    }
    if matches!(
        window.outcome(),
        ExecutionOutcomeV2::InterruptedPartial { .. } | ExecutionOutcomeV2::CappedUnknown { .. }
    ) {
        return Err(BenchError::Protocol(format!(
            "{route}: execution did not produce a complete bounded window"
        )));
    }
    Ok(())
}

fn row(candidate: &LexicalCandidate) -> Value {
    json!({
        "candidate_id": candidate.candidate_id,
        "source_repo_id": candidate.source_repo_id,
        "path": candidate.repo_relative_path,
        "score": candidate.score,
    })
}

fn witness(witness: &GoTestTraceFileLaneWitnessV1) -> Value {
    json!({
        "candidate_id": witness.candidate.candidate_id,
        "source_repo_id": witness.candidate.source_repo_id,
        "path": witness.candidate.repo_relative_path,
        "chunk_rank": witness.chunk_rank,
        "file_rank": witness.file_rank,
        "raw_score": witness.candidate.score,
    })
}

/// Optional reviewed failure-source evidence for a frozen pilot. A generation
/// pin alone never supplies failure-source authority.
struct SourceOwnerBinding {
    by_task: BTreeMap<String, TaskSourceContext>,
    collection_repo_id: RepoId,
    pack_file_sha256: String,
    plan_sha256: String,
    input_sha256: String,
    manifest_sha256: String,
}

struct TaskSourceContext {
    owner: Option<RepoId>,
    module: Option<BoundModuleContext>,
}

struct BoundModuleContext {
    context: GoTestTraceModuleContextV1,
    frame: Option<Value>,
}

/// Parse the first matching module function/source pair in the first goroutine
/// after the last panic marker. Earlier runtime and other-module pairs are
/// skipped, but a malformed matching pair refuses the whole frame. A bare
/// `*.go:<line>` mention cannot pass this parser. Returned coordinates remain
/// untrusted until bound to frozen source.
fn workspace_panic_frame(
    excerpt: &str,
    module_path: &str,
    checkout_prefix: &str,
) -> Option<GoTestTraceModuleCacheFrameV1> {
    if !checkout_prefix.starts_with('/')
        || checkout_prefix.ends_with('/')
        || checkout_prefix
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return None;
    }
    let lines = excerpt.lines().collect::<Vec<_>>();
    let (panic_index, log_prefix) = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.starts_with("panic:") {
                return Some((index, ""));
            }
            let (prefix, _) = line.split_once("panic:")?;
            prefix.ends_with(": ").then_some((index, prefix))
        })
        .last()?;
    let mut tail = lines[panic_index + 1..].iter();
    let header = loop {
        let line = tail.next()?.strip_prefix(log_prefix)?.trim();
        if line.is_empty() || line.starts_with("[signal ") {
            continue;
        }
        break line;
    };
    let (id, state) = header.strip_prefix("goroutine ")?.split_once(" [")?;
    if id.parse::<std::num::NonZeroU64>().is_err() || state != "running]:" {
        return None;
    }
    let (symbol_line, coordinate) = loop {
        let symbol_line = tail.next()?.strip_prefix(log_prefix)?.trim();
        if symbol_line.is_empty()
            || symbol_line.starts_with("goroutine ")
            || symbol_line.starts_with("created by ")
            || symbol_line.starts_with('/')
        {
            return None;
        }
        let coordinate_line = tail.next()?.strip_prefix(log_prefix)?;
        if !coordinate_line.starts_with(' ') && !coordinate_line.starts_with('\t') {
            return None;
        }
        let coordinate = coordinate_line.trim();
        if coordinate.is_empty()
            || coordinate.starts_with("goroutine ")
            || coordinate.starts_with("created by ")
        {
            return None;
        }
        let module_suffix = symbol_line.strip_prefix(module_path);
        if !module_suffix.is_some_and(|suffix| suffix.starts_with('/') || suffix.starts_with('.')) {
            continue;
        }
        break (symbol_line, coordinate);
    };
    let (stem, line_tail) = coordinate.split_once(".go:")?;
    let mut coordinate_parts = line_tail.split_ascii_whitespace();
    let number = coordinate_parts.next()?;
    if !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if let Some(offset) = coordinate_parts.next() {
        if !offset.starts_with("+0x")
            || offset.len() == 3
            || !offset[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
            || coordinate_parts.next().is_some()
        {
            return None;
        }
    }
    let line = number.parse::<std::num::NonZeroU32>().ok()?.get();
    let relative = if stem.starts_with('/') {
        stem.strip_prefix(checkout_prefix)?.strip_prefix('/')?
    } else {
        stem.strip_prefix("./").unwrap_or(stem)
    };
    let path = ExactRepoRelativePathV1::new(format!("{relative}.go")).ok()?;
    if path.as_str().ends_with("_test.go") {
        return None;
    }
    let package_suffix = match path.as_str().rsplit_once('/') {
        Some((parent, _)) => symbol_line
            .strip_prefix(module_path)?
            .strip_prefix('/')?
            .strip_prefix(parent)?,
        None => symbol_line.strip_prefix(module_path)?,
    };
    let function = package_suffix.strip_prefix('.')?;
    let (raw_symbol, arguments) = function.rsplit_once('(')?;
    if !arguments.ends_with(')') {
        return None;
    }
    let identifier = |value: &str| {
        let mut bytes = value.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    let symbol = if let Some(receiver_and_name) = raw_symbol.strip_prefix('(') {
        let (receiver, name) = receiver_and_name.split_once(").")?;
        let receiver = receiver.strip_prefix('*').unwrap_or(receiver);
        if !identifier(receiver) || !identifier(name) {
            return None;
        }
        format!("{receiver}.{name}")
    } else {
        if !identifier(raw_symbol) {
            return None;
        }
        raw_symbol.to_owned()
    };
    Some(GoTestTraceModuleCacheFrameV1 {
        path,
        line,
        symbol: Some(symbol),
    })
}

fn verified_module_frame(
    directory: &Path,
    frame: &quanta_index_contract::GoTestTraceModuleCacheFrameV1,
    expected_digest: &str,
) -> BenchResult<Value> {
    let path = frame.path.as_str();
    let symbol = frame.symbol.as_deref().ok_or_else(|| {
        BenchError::Config("reviewed module frame has no adjacent Go declaration".into())
    })?;
    let corpus = std::fs::canonicalize(directory.join("corpus"))
        .map_err(|error| BenchError::Config(format!("reviewed module corpus: {error}")))?;
    let source_path = corpus.join(path);
    let canonical = std::fs::canonicalize(&source_path)
        .map_err(|error| BenchError::Config(format!("reviewed module frame path: {error}")))?;
    if canonical != source_path {
        return Err(BenchError::Config(
            "reviewed module frame source is aliased".into(),
        ));
    }
    let bytes = std::fs::read(&source_path)
        .map_err(|error| BenchError::Config(format!("reviewed module frame source: {error}")))?;
    if sha256_hex(&bytes) != expected_digest {
        return Err(BenchError::Config(
            "reviewed module frame source digest differs".into(),
        ));
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|error| BenchError::Config(format!("reviewed module frame source: {error}")))?;
    let declarations = extract_symbols(path, source)
        .map_err(|error| BenchError::Config(format!("reviewed module frame symbols: {error}")))?;
    if declarations
        .iter()
        .filter(|item| {
            matches!(item.symbol_kind.as_str(), "function" | "method")
                && item.qualified_name.as_ref() == symbol
                && (item.definition_span.line_start..=item.definition_span.line_end)
                    .contains(&frame.line)
        })
        .count()
        != 1
    {
        return Err(BenchError::Config(
            "reviewed module frame has no unique source declaration".into(),
        ));
    }
    Ok(json!({"path": path, "line": frame.line, "symbol": symbol, "file_sha256": expected_digest}))
}

fn read_json(path: &Path) -> BenchResult<(Value, String)> {
    let bytes = std::fs::read(path)
        .map_err(|error| BenchError::Config(format!("{}: {error}", path.display())))?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|error| BenchError::Config(format!("{}: {error}", path.display())))?;
    Ok((value, sha256_hex(&bytes)))
}

fn source_owner_binding(
    args: &Args,
    pack: &QueryPack,
    pack_file_sha256: &str,
    requested_repo_id: &RepoId,
) -> BenchResult<Option<SourceOwnerBinding>> {
    let plan_path = args.flags.get("source-owner-plan");
    let input_path = args.flags.get("source-owner-inputs");
    let (Some(plan_path), Some(input_path)) = (plan_path, input_path) else {
        if plan_path.is_some() || input_path.is_some() {
            return Err(BenchError::Config(
                "source-owner-plan and source-owner-inputs must be supplied together".into(),
            ));
        }
        return Ok(None);
    };
    let (plan, plan_sha256) = read_json(Path::new(plan_path))?;
    let (inputs, input_sha256) = read_json(Path::new(input_path))?;
    if plan["input_sha256"].as_str() != Some(input_sha256.as_str()) {
        return Err(BenchError::Config(
            "source-owner inputs do not match the frozen plan".into(),
        ));
    }
    let matches = plan["plans"]
        .as_array()
        .ok_or_else(|| BenchError::Config("source-owner plan lacks cases".into()))?
        .iter()
        .filter(|case| case["pack_sha256"].as_str() == Some(pack_file_sha256))
        .collect::<Vec<_>>();
    let [case] = matches.as_slice() else {
        return Err(BenchError::Config(
            "source-owner plan must have exactly one matching pack case".into(),
        ));
    };
    if case["repo_id"].as_str() != Some(requested_repo_id.as_str())
        || case["commit"].as_str() != Some(pack.repository_commit.as_str())
    {
        return Err(BenchError::Config(
            "source-owner case differs from the selected repository or commit".into(),
        ));
    }
    let task_ids = case["task_ids"]
        .as_array()
        .ok_or_else(|| BenchError::Config("source-owner case lacks task IDs".into()))?;
    if task_ids.len() != pack.tasks.len()
        || pack
            .tasks
            .iter()
            .zip(task_ids)
            .any(|(task, id)| id.as_str() != Some(task.task_id.as_str()))
    {
        return Err(BenchError::Config(
            "source-owner case task IDs differ from the query pack".into(),
        ));
    }
    let directory = case["directory"]
        .as_str()
        .ok_or_else(|| BenchError::Config("source-owner case lacks directory".into()))?;
    let (manifest, manifest_sha256) = read_json(&Path::new(directory).join("manifest.json"))?;
    if case["manifest_sha256"].as_str() != Some(manifest_sha256.as_str())
        || manifest["repository_commit"].as_str() != Some(pack.repository_commit.as_str())
    {
        return Err(BenchError::Config(
            "source-owner manifest differs from the frozen case".into(),
        ));
    }
    let manifest_files = manifest["files"]
        .as_array()
        .ok_or_else(|| BenchError::Config("source-owner manifest lacks files".into()))?
        .iter()
        .map(|file| {
            Ok((
                file["path"]
                    .as_str()
                    .ok_or_else(|| BenchError::Config("manifest file lacks path".into()))?
                    .to_owned(),
                file["file_sha256"]
                    .as_str()
                    .ok_or_else(|| BenchError::Config("manifest file lacks digest".into()))?
                    .to_owned(),
            ))
        })
        .collect::<BenchResult<Vec<_>>>()?;
    if manifest_files != pack.file_universe {
        return Err(BenchError::Config(
            "source-owner manifest files differ from the query pack".into(),
        ));
    }
    let inputs = inputs
        .as_array()
        .ok_or_else(|| BenchError::Config("source-owner inputs must be a list".into()))?;
    let mut by_task = BTreeMap::new();
    for task in &pack.tasks {
        let Some(semantic_input) = &task.semantic_input else {
            return Err(BenchError::Config(
                "source-owner context requires a Go failure excerpt".into(),
            ));
        };
        let matches = inputs
            .iter()
            .filter(|input| input["task_id"].as_str() == Some(task.task_id.as_str()))
            .collect::<Vec<_>>();
        let [input] = matches.as_slice() else {
            return Err(BenchError::Config(format!(
                "source-owner inputs must uniquely identify task {}",
                task.task_id
            )));
        };
        let input_owner = input["repo_id"]
            .as_str()
            .ok_or_else(|| BenchError::Config("source-owner input lacks repository ID".into()))
            .and_then(|id| {
                RepoId::new(id).map_err(|error| BenchError::Config(error.to_string()))
            })?;
        let excerpt = semantic_input.failure_excerpt();
        if &input_owner != requested_repo_id
            || input["commit"].as_str() != Some(pack.repository_commit.as_str())
            || input["failure_excerpt"].as_str() != Some(excerpt)
            || input["provenance"]["failure_excerpt_sha256"].as_str()
                != Some(sha256_hex(excerpt.as_bytes()).as_str())
            || input["provenance"]["kind"].as_str()
                != Some("original_upstream_issue_failure_transcript")
        {
            return Err(BenchError::Config(format!(
                "source-owner input provenance differs from task {}",
                task.task_id
            )));
        }
        // External reporter frames and tests retain an unowned request. An
        // in-repository frame needs both panic syntax and frozen source proof.
        let context = match input["provenance"]["frame_stratum"].as_str() {
            Some("in_repo_source_stack_frame") => {
                if plan["status"].as_str() != Some("FROZEN_GO_TRACE_HOLDOUT_REVIEWED_V1") {
                    return Err(BenchError::Config(
                        "in-repository frame requires a reviewed frozen plan".into(),
                    ));
                }
                let module_path = input["provenance"]["module_path"].as_str().ok_or_else(|| {
                    BenchError::Config("in-repository frame lacks module path".into())
                })?;
                let checkout_prefix =
                    input["provenance"]["checkout_prefix"]
                        .as_str()
                        .ok_or_else(|| {
                            BenchError::Config("in-repository frame lacks checkout prefix".into())
                        })?;
                if case["module_path"].as_str() != Some(module_path)
                    || case["checkout_prefix"].as_str() != Some(checkout_prefix)
                {
                    return Err(BenchError::Config(
                        "in-repository frame context differs from frozen plan".into(),
                    ));
                }
                let frame = workspace_panic_frame(excerpt, module_path, checkout_prefix)
                    .ok_or_else(|| {
                        BenchError::Config(format!(
                            "no adjacent in-repository panic frame for task {}",
                            task.task_id
                        ))
                    })?;
                let (_, digest) = manifest_files
                    .iter()
                    .find(|(path, _)| path == frame.path.as_str())
                    .ok_or_else(|| {
                        BenchError::Config(format!(
                            "in-repository frame is absent from frozen source for task {}",
                            task.task_id
                        ))
                    })?;
                let verified = verified_module_frame(Path::new(directory), &frame, digest)?;
                if case["workspace_frame"] != verified {
                    return Err(BenchError::Config(
                        "reviewed workspace frame differs from frozen plan".into(),
                    ));
                }
                let hints = GoTestTraceSourceFramesV1::from_failure_excerpt(excerpt);
                if hints.frame_class() == GoTestTraceFrameClassV1::Exact
                    && !hints.contains_path(frame.path.as_str())
                {
                    return Err(BenchError::Config(
                        "SDK frame hint differs from verified workspace frame".into(),
                    ));
                }
                TaskSourceContext {
                    owner: Some(input_owner),
                    module: None,
                }
            }
            Some("in_repo_module_cache_panic_frame") => {
                let module = GoTestTraceModuleContextV1 {
                    module_path: input["provenance"]["module_path"]
                        .as_str()
                        .ok_or_else(|| BenchError::Config("module path is absent".into()))?
                        .to_owned(),
                    module_version: input["provenance"]["module_version"]
                        .as_str()
                        .ok_or_else(|| BenchError::Config("module version is absent".into()))?
                        .to_owned(),
                };
                module.validate_v1().map_err(BenchError::Config)?;
                if input["provenance"]["module_tag_commit"].as_str()
                    != Some(pack.repository_commit.as_str())
                {
                    return Err(BenchError::Config(format!(
                        "module tag differs from frozen source for task {}",
                        task.task_id
                    )));
                }
                let frame = GoTestTraceSourceFramesV1::module_cache_panic_frame(excerpt, &module)
                    .ok_or_else(|| {
                    BenchError::Config(format!(
                        "module panic frame is not uniquely bound for task {}",
                        task.task_id
                    ))
                })?;
                let (_, digest) = manifest_files
                    .iter()
                    .find(|(path, _)| path == frame.path.as_str())
                    .ok_or_else(|| {
                        BenchError::Config(format!(
                            "module panic frame is absent from frozen source for task {}",
                            task.task_id
                        ))
                    })?;
                let frame = if plan["status"].as_str()
                    == Some("FROZEN_GO_TRACE_HOLDOUT_REVIEWED_V1")
                {
                    let verified = verified_module_frame(Path::new(directory), &frame, digest)?;
                    if case["module_frame"] != verified
                        || case["module_path"].as_str() != Some(module.module_path.as_str())
                        || case["module_version"].as_str() != Some(module.module_version.as_str())
                    {
                        return Err(BenchError::Config(
                            "reviewed module frame differs from frozen plan".into(),
                        ));
                    }
                    Some(verified)
                } else {
                    None
                };
                TaskSourceContext {
                    owner: Some(input_owner),
                    module: Some(BoundModuleContext {
                        context: module,
                        frame,
                    }),
                }
            }
            Some("external_reporter_test_frame") => TaskSourceContext {
                owner: None,
                module: None,
            },
            _ => {
                return Err(BenchError::Config(format!(
                    "source-owner frame stratum is unrecognized for task {}",
                    task.task_id
                )));
            }
        };
        let _previous = by_task.insert(task.task_id.clone(), context);
    }
    Ok(Some(SourceOwnerBinding {
        by_task,
        collection_repo_id: requested_repo_id.clone(),
        pack_file_sha256: pack_file_sha256.to_owned(),
        plan_sha256,
        input_sha256,
        manifest_sha256,
    }))
}

fn trace_files(
    client: &QuantaIndex,
    pin: &GenerationPin,
    plan: &QueryPlan,
    lane_top_k: u32,
    semantic_input: &GoTraceSemanticInput,
    source_owner: Option<&RepoId>,
    source_module: Option<&BoundModuleContext>,
    source_binding: Option<&SourceOwnerBinding>,
) -> BenchResult<Value> {
    let start = Instant::now();
    let query = client
        .search()
        .hybrid()
        .native(&plan.lexical_request)
        .semantic_text(semantic_input.text())
        .pinned(pin.clone())
        .top_k(lane_top_k);
    let response = match (source_owner, source_module) {
        (Some(owner), Some(module)) => query.go_test_trace_file_with_module_context(
            owner.clone(),
            module.context.clone(),
            semantic_input.failure_excerpt(),
            20,
        ),
        (Some(owner), None) => {
            query.go_test_trace_file_with_source_context(
                owner.clone(),
                semantic_input.failure_excerpt(),
                20,
            )
        }
        (None, None) => query.go_test_trace_file(semantic_input.failure_excerpt(), 20),
        (None, Some(_)) => return Err(BenchError::Config("module lacks source owner".into())),
    }
    .map_err(|error| BenchError::Sdk(error.to_string()))?;
    let query_micros = start.elapsed().as_micros();
    bound_window(
        "trace lexical",
        pin,
        &response.generation,
        &response.lexical_window,
    )?;
    bound_window(
        "trace dense",
        pin,
        &response.generation,
        &response.dense_window,
    )?;
    let candidates = response
        .results
        .iter()
        .map(|hit| {
            let candidate = hit.representative().ok_or_else(|| {
                BenchError::Protocol("trace file response lacks a lane witness".to_string())
            })?;
            Ok(json!({
                "file": hit.file,
                "source_repo_id": hit.file.source_repo_id,
                "path": hit.file.repo_relative_path,
                "candidate_id": candidate.candidate_id,
                "file_rrf_score": hit.file_rrf_score,
                "production_source": hit.production_source,
                "source_frame": hit.source_frame,
                "lexical": hit.lexical.as_ref().map(witness),
                "dense": hit.dense.as_ref().map(witness),
            }))
        })
        .collect::<BenchResult<Vec<_>>>()?;
    Ok(json!({
        "profile": "go_trace_file_rrf_v1",
        "sdk_query_micros": query_micros,
        "frame_class": response.frame_class.as_str(),
        "failure_excerpt_sha256": sha256_hex(semantic_input.failure_excerpt().as_bytes()),
        "semantic_representation_policy": GO_TRACE_SEMANTIC_PROFILE,
        "semantic_text_sha256": semantic_input.text_sha256(),
        "source_context": {
            "mode": if source_module.is_some() { "frozen_module_panic_v1" } else if source_owner.is_some() { "explicit_pilot_owner_v1" } else { "unowned" },
            "failure_source_repo_id": source_owner,
            "module_path": source_module.map(|module| module.context.module_path.as_str()),
            "module_version": source_module.map(|module| module.context.module_version.as_str()),
            "module_frame": source_module.and_then(|module| module.frame.as_ref()),
            "pilot_plan_sha256": source_binding.map(|binding| binding.plan_sha256.as_str()),
            "pilot_inputs_sha256": source_binding.map(|binding| binding.input_sha256.as_str()),
            "manifest_sha256": source_binding.map(|binding| binding.manifest_sha256.as_str()),
            "pack_file_sha256": source_binding.map(|binding| binding.pack_file_sha256.as_str()),
        },
        "lexical_window": response.lexical_window,
        "dense_window": response.dense_window,
        "candidates": candidates,
    }))
}

fn paired_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    plan: &QueryPlan,
    top_k: u32,
    semantic_input: Option<&GoTraceSemanticInput>,
    source_owner: Option<&RepoId>,
    source_module: Option<&BoundModuleContext>,
    source_binding: Option<&SourceOwnerBinding>,
) -> BenchResult<Value> {
    let sdk_error = |error: quanta_index_sdk::SdkError| BenchError::Sdk(error.to_string());
    let start = Instant::now();
    let lexical = client
        .lexical()
        .query()
        .native(&plan.lexical_request)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()
        .map_err(sdk_error)?;
    let lexical_micros = start.elapsed().as_micros();
    bound_window("lexical", pin, &lexical.generation, &lexical.window)?;
    if lexical.rank_unit != TextRankUnit::Chunk {
        return Err(BenchError::Protocol(
            "trace-profile expects chunk-ranked lexical candidates".to_string(),
        ));
    }
    let start = Instant::now();
    let semantic = client
        .semantic()
        .query()
        .text(&plan.semantic_text)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()
        .map_err(sdk_error)?;
    let semantic_micros = start.elapsed().as_micros();
    bound_window("semantic", pin, &semantic.generation, &semantic.window)?;
    let start = Instant::now();
    let hybrid = client
        .search()
        .hybrid()
        .native(&plan.lexical_request)
        .semantic_text(&plan.semantic_text)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()
        .map_err(sdk_error)?;
    let hybrid_micros = start.elapsed().as_micros();
    bound_window("hybrid", pin, &hybrid.generation, &hybrid.window)?;
    if let Some(binding) = source_binding {
        let owner = &binding.collection_repo_id;
        if lexical
            .results
            .iter()
            .any(|row| &row.source_repo_id != owner)
            || semantic
                .results
                .iter()
                .any(|row| &row.source_repo_id != owner)
            || hybrid
                .results
                .iter()
                .any(|hit| &hit.candidate.source_repo_id != owner)
        {
            return Err(BenchError::Protocol(
                "source-owner pilot returned a candidate from another repository".into(),
            ));
        }
    }
    let trace_files = semantic_input
        .map(|input| {
            trace_files(
                client,
                pin,
                plan,
                top_k,
                input,
                source_owner,
                source_module,
                source_binding,
            )
        })
        .transpose()?;
    Ok(json!({
        "query_identity": {
            "original_query_sha256": plan.original_query_sha256,
            "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
            "semantic_text_sha256": plan.semantic_text_sha256,
            "generic_semantic_representation_policy": GENERIC_SEMANTIC_REPRESENTATION_POLICY,
            "trace_semantic_representation_policy": semantic_input.map(|_| GO_TRACE_SEMANTIC_PROFILE),
            "trace_semantic_text_sha256": semantic_input.map(GoTraceSemanticInput::text_sha256),
        },
        "lexical": {
            "query_micros": lexical_micros,
            "window": lexical.window,
            "candidates": lexical.results.iter().map(row).collect::<Vec<_>>(),
        },
        "semantic": {
            "query_micros": semantic_micros,
            "window": semantic.window,
            "candidates": semantic.results.iter().map(row).collect::<Vec<_>>(),
        },
        "hybrid": {
            "query_micros": hybrid_micros,
            "window": hybrid.window,
            "candidates": hybrid.results.iter().map(|hit| json!({
                "candidate_id": hit.candidate.candidate_id,
                "source_repo_id": hit.candidate.source_repo_id,
                "path": hit.candidate.repo_relative_path,
                "score": hit.fused_score,
                "contributions": hit.contributions,
            })).collect::<Vec<_>>(),
        },
        "trace_files": trace_files,
    }))
}

pub(super) fn run(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "query-pack",
            "state-root",
            "repo-id",
            "source-owner-plan",
            "source-owner-inputs",
            "generation",
            "nl-max-tokens",
            "io-timeout-secs",
            "out",
        ],
    )?;
    let (pack, pack_file_sha256) =
        load_query_pack_with_file_digest(&PathBuf::from(required(args, "query-pack")?))?;
    if pack.contract_top_k != 100 {
        return Err(BenchError::Config(
            "trace-profile requires a pinned 100-candidate lane budget".to_string(),
        ));
    }
    let out = PathBuf::from(required(args, "out")?);
    let state = PathBuf::from(required(args, "state-root")?);
    let repo_id = RepoId::new(required(args, "repo-id")?)
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let source_binding = source_owner_binding(args, &pack, &pack_file_sha256, &repo_id)?;
    let revision_id = RevisionId::new(pack.repository_commit.as_str())
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let generation = required(args, "generation")?
        .parse::<u64>()
        .ok()
        .filter(|generation| *generation > 0)
        .ok_or_else(|| BenchError::Config("expected a positive generation".to_string()))?;
    let pin = GenerationPin::new(repo_id, revision_id, ManifestGeneration::new(generation));
    let config = NlPlanConfig {
        max_tokens: super::optional_usize(
            args,
            "nl-max-tokens",
            NlPlanConfig::default().max_tokens,
        )?,
        ..NlPlanConfig::default()
    };
    let timeout = Duration::from_secs(optional_u64(args, "io-timeout-secs", 60)?);
    let client = QuantaIndex::connect(
        ConnectOptions::from_state_root(&state).with_request_io_timeout(timeout),
    )
    .map_err(|error| BenchError::Sdk(error.to_string()))?;
    let tasks = pack
        .tasks
        .iter()
        .map(|task| {
            let plan = task
                .plan(QueryInputPolicy::NaturalLanguage, &config)
                .map_err(|error| BenchError::Config(error.to_string()))?;
            let mut result = paired_query(
                &client,
                &pin,
                &plan,
                pack.contract_top_k,
                task.semantic_input.as_ref(),
                source_binding
                    .as_ref()
                    .and_then(|binding| binding.by_task.get(&task.task_id))
                    .and_then(|context| context.owner.as_ref()),
                source_binding
                    .as_ref()
                    .and_then(|binding| binding.by_task.get(&task.task_id))
                    .and_then(|context| context.module.as_ref()),
                source_binding.as_ref(),
            )?;
            result["task_id"] = json!(task.task_id);
            Ok(result)
        })
        .collect::<BenchResult<Vec<_>>>()?;
    let _digest = write_json_bound(
        &out,
        &json!({
            "schema_version": 2,
            "scope": "sealed-generation-sdk-quality-diagnostic",
            "generic_execution_profile_id": execution_profile_id(QueryInputPolicy::NaturalLanguage),
            "pack_sha256": pack.pack_sha256,
            "lane_top_k": pack.contract_top_k,
            "generation": pin,
            "tasks": tasks,
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_workspace_frame_requires_panic_pair_source_digest_and_declaration() {
        let directory = tempfile::tempdir().expect("reviewed corpus root");
        let package = directory.path().join("corpus/pkg");
        std::fs::create_dir_all(&package).expect("reviewed package");
        let source = b"package pkg\nfunc Work() {\n    panic(\"failure\")\n}\n";
        std::fs::write(package.join("worker.go"), source).expect("write source");
        let excerpt = concat!(
            "panic: failure\n",
            "goroutine 2 [running]:\n",
            "github.com/example/repo/pkg.Work(0x0)\n",
            "    ./pkg/worker.go:3 +0x1\n",
        );
        let frame = workspace_panic_frame(excerpt, "github.com/example/repo", "/build/repo")
            .expect("adjacent panic frame");
        let runtime_first = excerpt.replace(
            "github.com/example/repo/pkg.Work(0x0)",
            concat!(
                "runtime.gopanic(0x0)\n",
                "    /usr/local/go/src/runtime/panic.go:42 +0x2\n",
                "github.com/other/service/pkg.Other(0x0)\n",
                "    /go/pkg/mod/github.com/other/service@v1/pkg/other.go:8 +0x3\n",
                "github.com/example/repo/pkg.Work(0x0)",
            ),
        );
        assert_eq!(
            workspace_panic_frame(&runtime_first, "github.com/example/repo", "/build/repo"),
            Some(frame.clone()),
        );
        let digest = sha256_hex(source);
        assert_eq!(
            verified_module_frame(directory.path(), &frame, &digest).expect("source-bound frame"),
            json!({"path": "pkg/worker.go", "line": 3, "symbol": "Work", "file_sha256": digest})
        );
        assert!(
            GoTestTraceSourceFramesV1::from_failure_excerpt(excerpt).contains_path("pkg/worker.go")
        );
        assert!(
            workspace_panic_frame(
                "The issue mentions pkg/worker.go:3 but has no panic frame",
                "github.com/example/repo",
                "/build/repo",
            )
            .is_none()
        );
        for bad in [
            excerpt.replace("worker.go:3", "worker.go:9"),
            excerpt.replace("pkg.Work", "pkg.Other"),
        ] {
            let frame = workspace_panic_frame(&bad, "github.com/example/repo", "/build/repo")
                .expect("syntactic frame");
            assert!(verified_module_frame(directory.path(), &frame, &digest).is_err());
        }
        assert!(verified_module_frame(directory.path(), &frame, &"0".repeat(64)).is_err());
        for bad in [
            excerpt.replace("worker.go", "worker_test.go"),
            excerpt.replace("goroutine 2 [running]:", "goroutine 2 [waiting]:"),
            excerpt.replace("+0x1", "mentioned in prose"),
            runtime_first.replace(
                "github.com/example/repo/pkg.Work(0x0)",
                concat!(
                    "github.com/example/repo/pkg.Other(0x0)\n",
                    "    ./pkg/worker.go:word +0x1\n",
                    "github.com/example/repo/pkg.Work(0x0)",
                ),
            ),
            excerpt.replace(
                "pkg.Work(0x0)\n    ",
                "pkg.Work(0x0)\ncreated by runtime.goexit\n    ",
            ),
        ] {
            assert!(
                workspace_panic_frame(&bad, "github.com/example/repo", "/build/repo").is_none()
            );
        }
        let absolute = excerpt.replace("./pkg/worker.go", "/build/repo/pkg/worker.go");
        assert!(
            workspace_panic_frame(&absolute, "github.com/example/repo", "/build/repo").is_some()
        );
        assert!(
            workspace_panic_frame(&absolute, "github.com/example/repo", "/build/other").is_none()
        );
    }

    #[test]
    fn reviewed_module_frame_requires_pinned_digest_and_declaration_span() {
        let directory = tempfile::tempdir().expect("reviewed corpus root");
        let package = directory.path().join("corpus/internal");
        std::fs::create_dir_all(&package).expect("reviewed package");
        let source =
            b"package worker\ntype Worker struct{}\nfunc (w *Worker) Run() {\n    return\n}\n";
        std::fs::write(package.join("worker.go"), source).expect("write source");
        let module = GoTestTraceModuleContextV1 {
            module_path: "github.com/example/module".into(),
            module_version: "v1.2.3".into(),
        };
        let excerpt = concat!(
            "panic: failure\n",
            "github.com/example/module/internal.(*Worker).Run(0x0)\n",
            "/go/pkg/mod/github.com/example/module@v1.2.3/internal/worker.go:4 +0x11",
        );
        let frame = GoTestTraceSourceFramesV1::module_cache_panic_frame(excerpt, &module)
            .expect("exact transcript frame");
        let digest = sha256_hex(source);
        assert_eq!(
            verified_module_frame(directory.path(), &frame, &digest).expect("bound frame"),
            json!({"path": "internal/worker.go", "line": 4, "symbol": "Worker.Run", "file_sha256": digest})
        );
        let mut wrong = frame.clone();
        wrong.line = 2;
        assert!(verified_module_frame(directory.path(), &wrong, &digest).is_err());
        wrong.line = 4;
        wrong.symbol = Some("Worker.Other".into());
        assert!(verified_module_frame(directory.path(), &wrong, &digest).is_err());
        assert!(verified_module_frame(directory.path(), &frame, &"0".repeat(64)).is_err());
    }
}
