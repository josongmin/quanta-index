//! Runner record v3 emission (RB-02 single owner, RB-05 read-only).
//!
//! Maps SDK query outcomes to the exact `runner.schema.json` v3 wire form:
//! ordered results per `(task, route)`, recomputed byte spans, block hashes
//! and `qi-regex-v1` token counts from pinned source bytes, finite
//! timings, typed non-success statuses, the echoed comparison contract,
//! and per-capture provenance (chunk strategy/config, runner + searchd
//! binary identity, generation, receipt/activation digests, model).
//! RB-05 consumes these records without editing this module.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::canonical::canonical_json;
use crate::chunking::count_tokens;
use crate::corpus::SourceFile;
use crate::published_units::{PublishedUnitKind, PublishedUnitRegistry};
use crate::query_plan::{
    NlPlanConfig, QueryInputPolicy, QueryPlan, execution_profile_sha256, execution_profile_value,
    ordering_contract, plan_query,
};
use crate::sdk::{QueryOutcome, RankedHit};
use crate::{BenchError, BenchResult, sha256_hex};

pub const RUNNER_SCHEMA_VERSION: u64 = 5;
pub const TOKENIZER: &str = "qi-regex-v1";
pub const TOKENIZER_BUDGET_VERSION: &str = "qb-v1";
pub const OUTPUT_UNIT_POLICY: &str = "rank_prefix";
pub const SPAN_UNIT: &str = "byte_span_with_line_projection_v1";
pub const CAPTURE_SYSTEM_QUANTA: &str = "quanta";

/// Parse the raw pack without discarding repeated JSON keys.
///
/// A repeated `tasks` key could otherwise hide an earlier gold-bearing value from the
/// post-parse blindness check while leaving those bytes visible to the runner.
struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without repeated object keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Bool(value)))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        let number = serde_json::Number::from_f64(value)
            .ok_or_else(|| E::custom("non-finite JSON number"))?;
        Ok(UniqueJson(Value::Number(number)))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value.to_string())))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value)))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(UniqueJson(item)) = sequence.next_element()? {
            items.push(item);
        }
        Ok(UniqueJson(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Self::Value, A::Error> {
        let mut object = Map::new();
        while let Some((key, UniqueJson(value))) = entries.next_entry::<String, UniqueJson>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate JSON key: {key}")));
            }
            let _previous = object.insert(key, value);
        }
        Ok(UniqueJson(Value::Object(object)))
    }
}

/// One blind query-pack task.
#[derive(Debug, Clone)]
pub struct PackTask {
    pub task_id: String,
    pub query: String,
    pub query_sha256: String,
}

/// Validated blind query pack plus its canonical digest.
#[derive(Debug, Clone)]
pub struct QueryPack {
    pub suite_id: String,
    pub suite_commitment_sha256: String,
    pub repository_commit: String,
    pub tokenizer: String,
    pub tokenizer_budget_version: Option<String>,
    pub routes: Vec<String>,
    pub file_universe: Vec<(String, String)>,
    pub file_universe_digest: String,
    pub tasks: Vec<PackTask>,
    pub pack_sha256: String,
    /// The validated comparison contract, echoed verbatim into records.
    pub comparison_contract: Value,
    /// The contract's `top_k`: the CLI-declared cap must equal it exactly.
    pub contract_top_k: u32,
}

fn forbidden_pack_key(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if matches!(
                    key.as_str(),
                    "gold" | "grade" | "answerable" | "gold_access"
                ) {
                    return Some(key.clone());
                }
                if let Some(nested) = forbidden_pack_key(child) {
                    return Some(nested);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(forbidden_pack_key),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

fn exact_keys(object: &Map<String, Value>, expected: &[&str], context: &str) -> BenchResult<()> {
    let actual: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = expected.iter().copied().collect();
    if actual != expected {
        return Err(BenchError::Protocol(format!(
            "{context} fields differ from frozen query-pack contract: missing={:?}, unexpected={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        )));
    }
    Ok(())
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// File-universe digest, byte-identical to the evaluator's formula:
/// canonical JSON of `[{path, file_sha256}]` sorted by path, SHA-256.
pub fn pack_universe_digest(universe: &[(String, String)]) -> BenchResult<String> {
    let mut rows: Vec<(&str, &str)> = universe
        .iter()
        .map(|(path, digest)| (path.as_str(), digest.as_str()))
        .collect();
    rows.sort_unstable();
    let entries: Vec<Value> = rows
        .iter()
        .map(|(path, digest)| serde_json::json!({"path": path, "file_sha256": digest}))
        .collect();
    let rendered = canonical_json(&Value::Array(entries))?;
    Ok(sha256_hex(rendered.as_bytes()))
}

/// Validate the v3 comparison contract, returning its `top_k`.
fn validate_contract(contract: &Value) -> BenchResult<u32> {
    let object = contract
        .as_object()
        .ok_or_else(|| BenchError::Protocol("comparison contract must be an object".to_string()))?;
    exact_keys(
        object,
        &[
            "top_k",
            "tokenizer",
            "tokenizer_budget_version",
            "output_unit_policy",
            "span_unit",
        ],
        "comparison contract",
    )?;
    let top_k = object
        .get("top_k")
        .and_then(Value::as_u64)
        .filter(|value| *value >= 1)
        .ok_or_else(|| {
            BenchError::Protocol("comparison contract top_k must be a positive integer".to_string())
        })?;
    let top_k = u32::try_from(top_k).map_err(|err| {
        BenchError::Protocol(format!(
            "comparison contract top_k exceeds u32 range: {err}"
        ))
    })?;
    for (key, expected) in [
        ("tokenizer", TOKENIZER),
        ("tokenizer_budget_version", TOKENIZER_BUDGET_VERSION),
        ("output_unit_policy", OUTPUT_UNIT_POLICY),
        ("span_unit", SPAN_UNIT),
    ] {
        if object.get(key).and_then(Value::as_str) != Some(expected) {
            return Err(BenchError::Protocol(format!(
                "comparison contract {key} must be {expected}"
            )));
        }
    }
    Ok(top_k)
}

/// Load a `freeze`-produced query pack, verifying blindness (no gold-bearing
/// keys anywhere), query hashes and tokenizer binding.
pub fn load_query_pack(path: &Path) -> BenchResult<QueryPack> {
    let raw = std::fs::read_to_string(path).map_err(|err| BenchError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    let UniqueJson(value) = serde_json::from_str(&raw).map_err(|err| BenchError::Json {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    if let Some(key) = forbidden_pack_key(&value) {
        return Err(BenchError::Protocol(format!(
            "query pack leaks gold-bearing key: {key}"
        )));
    }
    let object = value.as_object().ok_or_else(|| {
        BenchError::Protocol(format!("query pack must be an object: {}", path.display()))
    })?;
    exact_keys(
        object,
        &[
            "schema_version",
            "suite_id",
            "suite_commitment_sha256",
            "repository_commit",
            "tokenizer",
            "tokenizer_budget_version",
            "routes",
            "file_universe",
            "file_universe_digest",
            "comparison_contract",
            "tasks",
        ],
        "query pack",
    )?;
    let get_str = |key: &str| -> BenchResult<String> {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| BenchError::Protocol(format!("query pack lacks nonempty string: {key}")))
    };
    let version = object.get("schema_version").and_then(Value::as_u64);
    if version != Some(3) {
        return Err(BenchError::Protocol(
            "query pack schema_version must be 3".to_string(),
        ));
    }
    let contract = object
        .get("comparison_contract")
        .ok_or_else(|| BenchError::Protocol("query pack lacks comparison_contract".to_string()))?;
    let contract_top_k = validate_contract(contract)?;
    let tokenizer = get_str("tokenizer")?;
    if tokenizer != TOKENIZER {
        return Err(BenchError::Protocol(format!(
            "query pack tokenizer mismatch: {tokenizer}"
        )));
    }
    let budget_version = get_str("tokenizer_budget_version")?;
    if budget_version != TOKENIZER_BUDGET_VERSION {
        return Err(BenchError::Protocol(format!(
            "query pack tokenizer/budget version mismatch: {budget_version}"
        )));
    }
    let routes = object
        .get("routes")
        .and_then(Value::as_array)
        .ok_or_else(|| BenchError::Protocol("query pack lacks routes".to_string()))?;
    let mut route_names = Vec::new();
    for route in routes {
        let name = route
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| BenchError::Protocol("query pack has an empty route".to_string()))?;
        route_names.push(name.to_string());
    }
    if route_names.is_empty()
        || route_names.iter().collect::<BTreeSet<_>>().len() != route_names.len()
    {
        return Err(BenchError::Protocol(
            "query pack routes must be nonempty and unique".to_string(),
        ));
    }
    let entries = object
        .get("file_universe")
        .ok_or_else(|| BenchError::Protocol("query pack lacks file_universe".to_string()))?;
    let list = entries
        .as_array()
        .ok_or_else(|| BenchError::Protocol("file_universe must be a list".to_string()))?;
    let mut universe = Vec::with_capacity(list.len());
    for entry in list {
        let item = entry.as_object().ok_or_else(|| {
            BenchError::Protocol("file_universe entry must be an object".to_string())
        })?;
        exact_keys(item, &["path", "file_sha256"], "file_universe entry")?;
        let path = item
            .get("path")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| BenchError::Protocol("file_universe entry lacks path".to_string()))?;
        let digest = item
            .get("file_sha256")
            .and_then(Value::as_str)
            .filter(|text| is_hex64(text))
            .ok_or_else(|| {
                BenchError::Protocol("file_universe entry lacks a file_sha256 digest".to_string())
            })?;
        universe.push((path.to_string(), digest.to_string()));
    }
    if universe
        .iter()
        .map(|(path, _)| path)
        .collect::<BTreeSet<_>>()
        .len()
        != universe.len()
    {
        return Err(BenchError::Protocol(
            "duplicate file_universe path".to_string(),
        ));
    }
    let file_universe_digest = get_str("file_universe_digest")?;
    if !is_hex64(&file_universe_digest) {
        return Err(BenchError::Protocol(
            "query pack file_universe_digest must be a lowercase sha256".to_string(),
        ));
    }
    if pack_universe_digest(&universe)? != file_universe_digest {
        return Err(BenchError::Protocol(
            "query pack file universe digest mismatch".to_string(),
        ));
    }
    let tasks = object
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| BenchError::Protocol("query pack lacks tasks".to_string()))?;
    if tasks.is_empty() {
        return Err(BenchError::Protocol(
            "query pack holds no tasks".to_string(),
        ));
    }
    let mut seen_ids = BTreeSet::new();
    let mut seen_queries = BTreeSet::new();
    let mut parsed = Vec::with_capacity(tasks.len());
    for task in tasks {
        let item = task
            .as_object()
            .ok_or_else(|| BenchError::Protocol("query pack task must be an object".to_string()))?;
        exact_keys(
            item,
            &["task_id", "query", "query_sha256"],
            "query pack task",
        )?;
        let task_id = item
            .get("task_id")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| BenchError::Protocol("task lacks task_id".to_string()))?;
        let query = item
            .get("query")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| BenchError::Protocol(format!("task lacks query: {task_id}")))?;
        let query_sha = item
            .get("query_sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| BenchError::Protocol(format!("task lacks query_sha256: {task_id}")))?;
        if sha256_hex(query.as_bytes()) != query_sha {
            return Err(BenchError::Protocol(format!(
                "query hash mismatch: {task_id}"
            )));
        }
        if !seen_ids.insert(task_id.to_string()) {
            return Err(BenchError::Protocol(format!(
                "duplicate task_id: {task_id}"
            )));
        }
        if !seen_queries.insert(query_sha.to_string()) {
            return Err(BenchError::Protocol(format!(
                "duplicate query text: {task_id}"
            )));
        }
        parsed.push(PackTask {
            task_id: task_id.to_string(),
            query: query.to_string(),
            query_sha256: query_sha.to_string(),
        });
    }
    let pack_sha256 = sha256_hex(canonical_json(&value)?.as_bytes());
    Ok(QueryPack {
        suite_id: get_str("suite_id")?,
        suite_commitment_sha256: get_str("suite_commitment_sha256")?,
        repository_commit: get_str("repository_commit")?,
        tokenizer,
        tokenizer_budget_version: Some(budget_version),
        routes: route_names,
        file_universe: universe,
        file_universe_digest,
        tasks: parsed,
        pack_sha256,
        comparison_contract: contract.clone(),
        contract_top_k,
    })
}

/// Runner identity block for the record.
#[derive(Debug, Clone)]
pub struct RunnerIdentity {
    pub name: String,
    pub revision: String,
    pub run_id: String,
    pub blinding: String,
    pub isolation_method: String,
    pub access_block_log: String,
}

impl RunnerIdentity {
    pub fn new(
        name: String,
        revision: String,
        run_id: String,
        blinding: String,
        isolation_method: String,
        access_block_log: String,
    ) -> BenchResult<Self> {
        if blinding != "isolated" && blinding != "attested" {
            return Err(BenchError::Config(
                "blinding must be isolated or attested".to_string(),
            ));
        }
        for (label, value) in [
            ("name", &name),
            ("revision", &revision),
            ("run_id", &run_id),
            ("isolation_method", &isolation_method),
            ("access_block_log", &access_block_log),
        ] {
            if value.trim().is_empty() {
                return Err(BenchError::Config(format!(
                    "runner {label} must not be empty"
                )));
            }
        }
        Ok(Self {
            name,
            revision,
            run_id,
            blinding,
            isolation_method,
            access_block_log,
        })
    }
}

/// Per-route provenance: a reference to the capture that served it.
/// System/model facts live in the capture, not here.
#[derive(Debug, Clone)]
pub struct RouteProvenance {
    pub capture_id: String,
}

/// One v3 capture: the complete provenance of a routed observation.
#[derive(Debug, Clone)]
pub struct CaptureProvenance {
    pub chunk_strategy: String,
    pub chunk_config: Value,
    pub runner_binary_name: String,
    pub runner_binary_digest: String,
    pub searchd_binary_digest: String,
    pub generation: u64,
    pub source_repo_id: String,
    pub source_revision_id: String,
    pub receipt_digest: String,
    pub activation_digest: String,
    pub model: String,
    pub model_revision: String,
    pub execution_profile: Value,
    pub execution_profile_sha256: String,
}

fn validate_chunk_config_value(config: &Value) -> BenchResult<()> {
    let object = config.as_object().ok_or_else(|| {
        BenchError::Protocol("capture chunk_config must be an object".to_string())
    })?;
    for (key, value) in object {
        match key.as_str() {
            "window_bytes" | "max_item_bytes" => {
                if value.as_u64().filter(|n| *n >= 1).is_none() {
                    return Err(BenchError::Protocol(format!(
                        "capture chunk_config.{key} must be a positive integer"
                    )));
                }
            }
            "overlap_bytes" => {
                if value.as_u64().is_none() {
                    return Err(BenchError::Protocol(
                        "capture chunk_config.overlap_bytes must be a non-negative integer"
                            .to_string(),
                    ));
                }
            }
            "alignment" => {
                if value.as_str() != Some("byte") && value.as_str() != Some("line") {
                    return Err(BenchError::Protocol(
                        "capture chunk_config.alignment must be byte or line".to_string(),
                    ));
                }
            }
            "byte_cap_strict" => {
                if value.as_bool().is_none() {
                    return Err(BenchError::Protocol(
                        "capture chunk_config.byte_cap_strict must be a boolean".to_string(),
                    ));
                }
            }
            other => {
                return Err(BenchError::Protocol(format!(
                    "capture chunk_config holds unknown key: {other}"
                )));
            }
        }
    }
    Ok(())
}

fn capture_value(capture_id: &str, capture: &CaptureProvenance) -> BenchResult<Value> {
    if ![
        "whole_file",
        "fixed_window_strict",
        "fixed_window_line_aligned",
        "brace_heuristic",
    ]
    .contains(&capture.chunk_strategy.as_str())
    {
        return Err(BenchError::Protocol(format!(
            "capture {capture_id} chunk_strategy is not a frozen quanta strategy: {}",
            capture.chunk_strategy
        )));
    }
    validate_chunk_config_value(&capture.chunk_config)?;
    if capture.runner_binary_name.trim().is_empty() {
        return Err(BenchError::Protocol(format!(
            "capture {capture_id} runner_binary.name must not be empty"
        )));
    }
    for (label, digest) in [
        ("runner_binary.digest", &capture.runner_binary_digest),
        (
            "searchd_binary.binary_digest",
            &capture.searchd_binary_digest,
        ),
        ("receipt_digest", &capture.receipt_digest),
        ("activation_digest", &capture.activation_digest),
        (
            "execution_profile_sha256",
            &capture.execution_profile_sha256,
        ),
    ] {
        if !is_hex64(digest) {
            return Err(BenchError::Protocol(format!(
                "capture {capture_id} {label} must be a lowercase sha256"
            )));
        }
    }
    let canonical_profile = canonical_json(&capture.execution_profile)?;
    if sha256_hex(canonical_profile.as_bytes()) != capture.execution_profile_sha256 {
        return Err(BenchError::Protocol(format!(
            "capture {capture_id} execution profile digest mismatch"
        )));
    }
    for (label, text) in [
        ("model", &capture.model),
        ("model_revision", &capture.model_revision),
        ("source_repo_id", &capture.source_repo_id),
        ("source_revision_id", &capture.source_revision_id),
    ] {
        if text.trim().is_empty() {
            return Err(BenchError::Protocol(format!(
                "capture {capture_id} {label} must not be empty"
            )));
        }
    }
    let mut value = serde_json::json!({
        "system": CAPTURE_SYSTEM_QUANTA,
        "chunk_strategy": capture.chunk_strategy,
        "chunk_config": capture.chunk_config,
        "runner_binary": {
            "name": capture.runner_binary_name,
            "digest": capture.runner_binary_digest,
        },
        "searchd_binary": {"binary_digest": capture.searchd_binary_digest},
        "generation": capture.generation,
        "receipt_digest": capture.receipt_digest,
        "activation_digest": capture.activation_digest,
        "model": capture.model,
        "model_revision": capture.model_revision,
        "execution_profile": capture.execution_profile,
        "execution_profile_sha256": capture.execution_profile_sha256,
    });
    if matches!(
        capture
            .execution_profile
            .get("policy")
            .and_then(Value::as_str),
        Some("code_search_file" | "code_search_exact_content_file")
    ) {
        let object = value.as_object_mut().ok_or_else(|| {
            BenchError::Protocol(format!("capture {capture_id} JSON is not an object"))
        })?;
        drop(object.insert(
            "source_repo_id".to_string(),
            serde_json::json!(capture.source_repo_id),
        ));
        drop(object.insert(
            "source_revision_id".to_string(),
            serde_json::json!(capture.source_revision_id),
        ));
    }
    Ok(value)
}

fn duration_ms(latency: Duration) -> BenchResult<f64> {
    let ms = latency.as_secs_f64() * 1000.0;
    if !ms.is_finite() || ms < 0.0 {
        return Err(BenchError::Protocol(
            "non-finite query latency cannot be recorded".to_string(),
        ));
    }
    Ok(ms)
}

struct ProvenHit {
    candidate: Value,
    scored_span: (String, u64, u64),
    indexed_span: (u32, u32),
    unit_kind: PublishedUnitKind,
}

/// Source bytes proved for every native hit, before scored-span collapse.
pub struct NativeSpanProof {
    /// The published unit which produced this hit.
    pub unit_id: String,
    /// Canonical source file and half-open byte range used for scoring.
    pub span: (String, u64, u64),
    /// Typed published identity and definition bytes, independent of context.
    pub unit_kind: PublishedUnitKind,
    pub indexed_span: (u32, u32),
}

/// Reuse the recorder's source proof for diagnostic projection witnesses.
pub fn native_span_proofs(
    outcomes: &BTreeMap<(String, String), QueryOutcome>,
    files: &BTreeMap<String, SourceFile>,
    units: &PublishedUnitRegistry,
) -> BenchResult<BTreeMap<(String, String), Vec<NativeSpanProof>>> {
    let mut proofs = BTreeMap::new();
    for (key, outcome) in outcomes {
        if let QueryOutcome::ReturnedWindow { hits, .. } = outcome {
            let mut spans = Vec::with_capacity(hits.len());
            for (position, hit) in hits.iter().enumerate() {
                let rank = position.checked_add(1).ok_or_else(|| {
                    BenchError::Protocol("native projection rank overflow".to_string())
                })?;
                let proven = prove_hit(hit, rank, files, units)?;
                spans.push(NativeSpanProof {
                    unit_id: hit.candidate_id.clone(),
                    span: proven.scored_span,
                    unit_kind: proven.unit_kind,
                    indexed_span: proven.indexed_span,
                });
            }
            let _previous = proofs.insert(key.clone(), spans);
        }
    }
    Ok(proofs)
}

/// Prove one SDK hit against pinned source bytes and emit the evaluator's
/// candidate object. Returned with its 1-based rank.
///
/// Authority is the typed published-unit registry (RBR-05): a chunk hit
/// must match its published chunk span (unanchored hits fall back to the
/// published chunk bytes), and a symbol hit must match its published
/// definition span — the engine's snippet is a reference name, never
/// source-byte evidence.
fn prove_hit(
    hit: &RankedHit,
    rank: usize,
    files: &BTreeMap<String, SourceFile>,
    units: &PublishedUnitRegistry,
) -> BenchResult<ProvenHit> {
    if hit.file_authority.is_some() {
        return prove_file_hit(hit, rank, files);
    }
    let file = files.get(&hit.path).ok_or_else(|| {
        BenchError::Protocol(format!("SDK hit outside admitted universe: {}", hit.path))
    })?;
    let unit = units.get(&hit.candidate_id).ok_or_else(|| {
        BenchError::Protocol(format!(
            "SDK hit has no published unit ID: {}",
            hit.candidate_id
        ))
    })?;
    if unit.path != hit.path {
        return Err(BenchError::Protocol(format!(
            "SDK hit path differs from published unit: {}",
            hit.candidate_id
        )));
    }
    let (start_line, end_line) = if hit.start_line == 0 && hit.end_line == 0 {
        match unit.kind {
            PublishedUnitKind::Chunk => {
                // Only the chunk authority can prove an unanchored hit, and
                // only when the returned snippet is exactly the published
                // chunk text.
                let chunk = units.chunk_text(&hit.candidate_id).ok_or_else(|| {
                    BenchError::Protocol(format!(
                        "published chunk disappeared: {}",
                        hit.candidate_id
                    ))
                })?;
                if chunk != hit.snippet.as_str() {
                    return Err(BenchError::Protocol(format!(
                        "SDK unanchored hit differs from published chunk: {}",
                        hit.candidate_id
                    )));
                }
                (unit.start_line, unit.end_line)
            }
            PublishedUnitKind::Symbol => {
                return Err(BenchError::Protocol(format!(
                    "SDK unanchored symbol hit has no proving authority: {}",
                    hit.candidate_id
                )));
            }
            PublishedUnitKind::File => {
                return Err(BenchError::Protocol(
                    "file ID appeared in chunk/symbol registry".to_string(),
                ));
            }
        }
    } else {
        if (hit.start_line, hit.end_line) != (unit.start_line, unit.end_line) {
            return Err(BenchError::Protocol(format!(
                "SDK hit span differs from published unit: {}:{}-{} (published {}-{})",
                hit.path, hit.start_line, hit.end_line, unit.start_line, unit.end_line
            )));
        }
        (hit.start_line, hit.end_line)
    };
    if start_line == 0 || end_line == 0 || start_line > end_line {
        return Err(BenchError::Protocol(format!(
            "SDK hit has an inverted line span: {}:{}-{}",
            hit.path, start_line, end_line
        )));
    }
    let start_line_index = usize::try_from(start_line).map_err(|err| {
        BenchError::Protocol(format!("SDK hit start line cannot fit usize: {err}"))
    })?;
    let end_line_index = usize::try_from(end_line)
        .map_err(|err| BenchError::Protocol(format!("SDK hit end line cannot fit usize: {err}")))?;
    let (start, end) = file.line_span_bytes(start_line_index, end_line_index)?;
    let block = file.bytes.get(start..end).ok_or_else(|| {
        BenchError::Protocol(format!(
            "SDK hit block is outside source bytes: {}",
            hit.path
        ))
    })?;
    let text = std::str::from_utf8(block).map_err(|err| {
        BenchError::Protocol(format!("SDK hit block is not UTF-8: {}: {err}", hit.path))
    })?;
    let tokens = count_tokens(text);
    if tokens == 0 {
        return Err(BenchError::Protocol(format!(
            "SDK hit block holds no retrievable tokens: {}",
            hit.path
        )));
    }
    let tokens = u64::try_from(tokens).map_err(|err| {
        BenchError::Protocol(format!("SDK hit token count cannot fit u64: {err}"))
    })?;
    let indexed_start = usize::try_from(unit.byte_start).map_err(|err| {
        BenchError::Protocol(format!("published unit start cannot fit usize: {err}"))
    })?;
    let indexed_end = usize::try_from(unit.byte_end).map_err(|err| {
        BenchError::Protocol(format!("published unit end cannot fit usize: {err}"))
    })?;
    if indexed_start < start || indexed_end > end || indexed_start >= indexed_end {
        return Err(BenchError::Protocol(format!(
            "published unit span is outside scored line projection: {}",
            hit.candidate_id
        )));
    }
    let scored_bytes = end.checked_sub(start).ok_or_else(|| {
        BenchError::Protocol("scored projection has inverted byte span".to_string())
    })?;
    let indexed_bytes = indexed_end
        .checked_sub(indexed_start)
        .ok_or_else(|| BenchError::Protocol("published unit has inverted byte span".to_string()))?;
    let extra_context_bytes = scored_bytes.checked_sub(indexed_bytes).ok_or_else(|| {
        BenchError::Protocol("published unit exceeds scored projection".to_string())
    })?;
    let extra_context_bytes = u64::try_from(extra_context_bytes)
        .map_err(|err| BenchError::Protocol(format!("context expansion cannot fit u64: {err}")))?;
    let rank = u64::try_from(rank)
        .map_err(|err| BenchError::Protocol(format!("SDK hit rank cannot fit u64: {err}")))?;
    let start_byte = u64::try_from(start)
        .map_err(|err| BenchError::Protocol(format!("SDK hit start byte cannot fit u64: {err}")))?;
    let end_byte = u64::try_from(end)
        .map_err(|err| BenchError::Protocol(format!("SDK hit end byte cannot fit u64: {err}")))?;
    let mut candidate = serde_json::json!({
        "path": hit.path,
        "start_byte": start_byte,
        "end_byte": end_byte,
        "start_line": start_line,
        "end_line": end_line,
        "file_sha256": file.sha256,
        "block_sha256": sha256_hex(block),
        "tokens": tokens,
        "rank": rank,
        "span_accounting": {
            "unit_kind": unit.kind.as_str(),
            "unit_id": unit.id,
            "producer_identity": unit.producer_identity,
            "indexed_start_byte": unit.byte_start,
            "indexed_end_byte": unit.byte_end,
            "sdk_start_line": hit.start_line,
            "sdk_end_line": hit.end_line,
            "extra_context_bytes": extra_context_bytes,
        },
    });
    if let Some(name_span) = &unit.name_span {
        candidate["span_accounting"]["name_span"] = serde_json::to_value(name_span)
            .map_err(|err| BenchError::Protocol(format!("cannot encode symbol name span: {err}")))?;
    }
    Ok(ProvenHit {
        candidate,
        scored_span: (hit.path.clone(), start_byte, end_byte),
        indexed_span: (unit.byte_start, unit.byte_end),
        unit_kind: unit.kind,
    })
}

fn expected_file_candidate_id(source_repo_id: &str, path: &str) -> BenchResult<String> {
    let repo = source_repo_id.as_bytes();
    let path = path.as_bytes();
    let repo_len = u64::try_from(repo.len())
        .map_err(|err| BenchError::Protocol(format!("source repo ID length overflow: {err}")))?;
    let path_len = u64::try_from(path.len())
        .map_err(|err| BenchError::Protocol(format!("source path length overflow: {err}")))?;
    let mut bytes = b"quanta-index:code-search-file:v1\0".to_vec();
    bytes.extend_from_slice(&repo_len.to_le_bytes());
    bytes.extend_from_slice(repo);
    bytes.extend_from_slice(&path_len.to_le_bytes());
    bytes.extend_from_slice(path);
    Ok(format!("file:{}", sha256_hex(&bytes)))
}

/// Independently prove a product file candidate against the frozen source
/// universe. This path never consults the chunk/symbol unit registry.
fn prove_file_hit(
    hit: &RankedHit,
    rank: usize,
    files: &BTreeMap<String, SourceFile>,
) -> BenchResult<ProvenHit> {
    use quanta_index_contract::PreviewKind;

    let authority = hit.file_authority.as_ref().ok_or_else(|| {
        BenchError::Protocol("code search file hit lacks source authority".to_string())
    })?;
    let file = files.get(&hit.path).ok_or_else(|| {
        BenchError::Protocol(format!("file hit outside admitted universe: {}", hit.path))
    })?;
    let source = &authority.source;
    if source.file.source_repo_id != authority.source_repo_id
        || source.file.repo_relative_path.as_str() != hit.path
        || authority.repo_id != authority.source_repo_id
        || authority.revision_id != source.revision_id
        || authority.preview.source.as_ref() != Some(source)
    {
        return Err(BenchError::Protocol(
            "file candidate source identity differs from SDK row".to_string(),
        ));
    }
    let mut source_digest = String::with_capacity(64);
    for byte in source.source_sha256 {
        use std::fmt::Write;
        write!(source_digest, "{byte:02x}").map_err(|err| {
            BenchError::Protocol(format!("file source digest formatting failed: {err}"))
        })?;
    }
    if source_digest != file.sha256 || sha256_hex(&file.bytes) != file.sha256 {
        return Err(BenchError::Protocol(format!(
            "file candidate source digest differs from frozen bytes: {}",
            hit.path
        )));
    }
    let expected_id = expected_file_candidate_id(authority.source_repo_id.as_str(), &hit.path)?;
    if hit.candidate_id != expected_id {
        return Err(BenchError::Protocol(format!(
            "file candidate ID differs from source repo/path identity: {}",
            hit.path
        )));
    }
    authority
        .preview
        .validate_emission(&hit.snippet)
        .map_err(|reason| BenchError::Protocol(format!("file preview is invalid: {reason}")))?;
    let (preview_kind, preview_start_byte, preview_end_byte) = match authority.preview.kind {
        PreviewKind::SourceFile => {
            if let Some(context) = authority.preview.original_context {
                let start = usize::try_from(context.start).map_err(|err| {
                    BenchError::Protocol(format!("file preview start overflow: {err}"))
                })?;
                let end = usize::try_from(context.end).map_err(|err| {
                    BenchError::Protocol(format!("file preview end overflow: {err}"))
                })?;
                if file.bytes.get(start..end) != Some(hit.snippet.as_bytes()) {
                    return Err(BenchError::Protocol(format!(
                        "file preview differs from frozen source bytes: {}",
                        hit.path
                    )));
                }
                ("source_file", Some(context.start), Some(context.end))
            } else if authority.preview.unavailable_reason.is_none() {
                return Err(BenchError::Protocol(
                    "available file preview has no source context".to_string(),
                ));
            } else {
                ("source_file_unavailable", None, None)
            }
        }
        PreviewKind::Path => {
            if (hit.start_line, hit.end_line) != (0, 0) {
                return Err(BenchError::Protocol(
                    "path-only file hit claims source line evidence".to_string(),
                ));
            }
            if hit.snippet != hit.path {
                return Err(BenchError::Protocol(
                    "path-only file preview differs from the candidate path".to_string(),
                ));
            }
            ("path", None, None)
        }
        PreviewKind::SourceChunk | PreviewKind::SyntheticSymbolLabel => {
            return Err(BenchError::Protocol(
                "file candidate has non-file preview kind".to_string(),
            ));
        }
    };
    let end = u64::try_from(file.bytes.len())
        .map_err(|err| BenchError::Protocol(format!("file size overflow: {err}")))?;
    let end_u32 = u32::try_from(file.bytes.len())
        .map_err(|err| BenchError::Protocol(format!("file indexed span overflow: {err}")))?;
    let rank = u64::try_from(rank)
        .map_err(|err| BenchError::Protocol(format!("file rank overflow: {err}")))?;
    let tokens = u64::try_from(count_tokens(&hit.snippet))
        .map_err(|err| BenchError::Protocol(format!("file preview token count overflow: {err}")))?;
    let end_line = u32::try_from(file.line_count())
        .map_err(|err| BenchError::Protocol(format!("file line count overflow: {err}")))?;
    let start_line = u32::from(end_line != 0);
    let candidate = serde_json::json!({
        "path": hit.path,
        "start_byte": 0,
        "end_byte": end,
        "start_line": start_line,
        "end_line": end_line,
        "file_sha256": file.sha256,
        "block_sha256": file.sha256,
        "tokens": tokens,
        "rank": rank,
        "span_accounting": {
            "unit_kind": "file",
            "unit_id": hit.candidate_id,
            "producer_identity": "code-search-file-v1",
            "indexed_start_byte": 0,
            "indexed_end_byte": end,
            "sdk_start_line": hit.start_line,
            "sdk_end_line": hit.end_line,
            "extra_context_bytes": 0,
            "source_repo_id": authority.source_repo_id.as_str(),
            "source_revision_id": source.revision_id.as_str(),
            "preview_kind": preview_kind,
            "preview_start_byte": preview_start_byte,
            "preview_end_byte": preview_end_byte,
            "snippet_sha256": sha256_hex(hit.snippet.as_bytes()),
        },
    });
    Ok(ProvenHit {
        candidate,
        scored_span: (hit.path.clone(), 0, end),
        indexed_span: (0, end_u32),
        unit_kind: PublishedUnitKind::File,
    })
}

fn error_value(code: &str, message: &str) -> Value {
    serde_json::json!({"code": code, "message": message})
}

fn timings_value(latency: Duration) -> BenchResult<Value> {
    let ms = duration_ms(latency)?;
    let number = serde_json::Number::from_f64(ms).ok_or_else(|| {
        BenchError::Protocol("non-finite query latency cannot be recorded".to_string())
    })?;
    Ok(serde_json::json!({"query_latency_ms": number}))
}

/// Bind the result unit and, for file projections, how the files are ordered.
///
/// `rank_unit` says only what one ranked item is. `ordering` says whether the
/// order is a scored ranking or the path order of a match-only restriction.
fn bind_rank_unit(mut result: Value, policy: QueryInputPolicy) -> BenchResult<Value> {
    let rank_unit = match policy {
        QueryInputPolicy::LiteralFile
        | QueryInputPolicy::KeywordFile
        | QueryInputPolicy::SubstringFile
        | QueryInputPolicy::CodeSearchFile
        | QueryInputPolicy::CodeSearchExactContentFile
        | QueryInputPolicy::CodeSearchTypoFile
        | QueryInputPolicy::CodeSearchComponentsFile
        | QueryInputPolicy::NaturalLanguageFile => Some("distinct_file"),
        QueryInputPolicy::ExactSymbolName => Some("symbol"),
        QueryInputPolicy::Native
        | QueryInputPolicy::Literal
        | QueryInputPolicy::NaturalLanguage => None,
    };
    let object = result
        .as_object_mut()
        .ok_or_else(|| BenchError::Protocol("result object is malformed".to_string()))?;
    if let Some(rank_unit) = rank_unit {
        let _previous = object.insert("rank_unit".to_string(), Value::from(rank_unit));
    }
    if let Some(ordering) = ordering_contract(policy) {
        let _previous = object.insert("ordering".to_string(), Value::from(ordering));
    }
    if matches!(
        policy,
        QueryInputPolicy::KeywordFile
            | QueryInputPolicy::CodeSearchFile
            | QueryInputPolicy::CodeSearchExactContentFile
            | QueryInputPolicy::CodeSearchTypoFile
            | QueryInputPolicy::CodeSearchComponentsFile
            | QueryInputPolicy::NaturalLanguageFile
    ) {
        let _previous = object.insert(
            "score_evidence".to_string(),
            Value::from("native_sdk_score_v1"),
        );
    }
    Ok(result)
}

/// Map one query outcome to a v3 result object. `top_k` is the declared cap;
/// more hits than the cap is a protocol violation, never a truncation.
pub fn result_value(
    task_id: &str,
    route: &str,
    outcome: &QueryOutcome,
    plan: &QueryPlan,
    top_k: u32,
    files: &BTreeMap<String, SourceFile>,
    units: &PublishedUnitRegistry,
) -> BenchResult<Value> {
    if ordering_contract(plan.policy).is_some() && route != "lexical" {
        return Err(BenchError::Protocol(format!(
            "{} result requires lexical route, got {route}",
            plan.policy.as_str()
        )));
    }
    if plan.policy == QueryInputPolicy::ExactSymbolName && route != "symbol" {
        return Err(BenchError::Protocol(format!(
            "exact_symbol_name result requires symbol route, got {route}"
        )));
    }
    let query_identity = serde_json::json!({
        "original_query_sha256": plan.original_query_sha256,
        "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
        "semantic_text_sha256": plan.semantic_text_sha256,
    });
    let classification = outcome.classification().map_err(|message| {
        BenchError::Protocol(format!(
            "invalid typed query outcome for {task_id}/{route}: {message}"
        ))
    })?;
    match outcome {
        QueryOutcome::ReturnedWindow { hits, latency, .. } => {
            let hit_count = u64::try_from(hits.len()).map_err(|err| {
                BenchError::Protocol(format!("SDK hit count cannot fit u64: {err}"))
            })?;
            if hit_count > u64::from(top_k) {
                return Err(BenchError::Protocol(format!(
                    "{route} returned {} hits above top_k={top_k} for {task_id}",
                    hits.len()
                )));
            }
            if hits.is_empty() {
                let error = classification
                    .error_code
                    .as_ref()
                    .map_or(Value::Null, |code| {
                        error_value(
                            code,
                            classification
                                .error_message
                                .as_deref()
                                .unwrap_or("missing error message"),
                        )
                    });
                return bind_rank_unit(
                    serde_json::json!({
                        "task_id": task_id,
                        "route": route,
                        "status": classification.status,
                        "candidates": [],
                        "query_identity": query_identity,
                        "timings": timings_value(*latency)?,
                        "error": error,
                    }),
                    plan.policy,
                );
            }
            let mut candidates = Vec::with_capacity(hits.len());
            let mut seen_files = BTreeSet::new();
            let mut seen_unit_ids = BTreeSet::new();
            let mut seen_spans = BTreeSet::new();
            let mut seen_symbol_spans = BTreeSet::new();
            let mut previous_scored_file: Option<(f64, String)> = None;
            for hit in hits {
                // Prove every raw hit before the legacy source-span collapse.
                // Only the bound exact-symbol policy preserves independent
                // symbol ranks when two definitions share a context line.
                let mut proven = prove_hit(hit, candidates.len().saturating_add(1), files, units)?;
                if matches!(
                    plan.policy,
                    QueryInputPolicy::CodeSearchFile
                        | QueryInputPolicy::CodeSearchExactContentFile
                        | QueryInputPolicy::CodeSearchTypoFile
                        | QueryInputPolicy::CodeSearchComponentsFile
                ) != (proven.unit_kind == PublishedUnitKind::File)
                {
                    return Err(BenchError::Protocol(format!(
                        "{} result has incompatible file identity: {}",
                        plan.policy.as_str(),
                        hit.candidate_id
                    )));
                }
                if matches!(
                    plan.policy,
                    QueryInputPolicy::KeywordFile
                        | QueryInputPolicy::CodeSearchFile
                        | QueryInputPolicy::CodeSearchExactContentFile
                        | QueryInputPolicy::CodeSearchTypoFile
                        | QueryInputPolicy::CodeSearchComponentsFile
                        | QueryInputPolicy::NaturalLanguageFile
                ) {
                    let score = serde_json::Number::from_f64(hit.score).ok_or_else(|| {
                        BenchError::Protocol(format!(
                            "{} SDK score is not finite",
                            plan.policy.as_str()
                        ))
                    })?;
                    if previous_scored_file.as_ref().is_some_and(
                        |(previous_score, previous_path)| match hit.score.total_cmp(previous_score)
                        {
                            std::cmp::Ordering::Greater => true,
                            std::cmp::Ordering::Equal => {
                                hit.path.as_bytes() < previous_path.as_bytes()
                            }
                            std::cmp::Ordering::Less => false,
                        },
                    ) {
                        return Err(BenchError::Protocol(format!(
                            "{} SDK score/path order is invalid",
                            plan.policy.as_str()
                        )));
                    }
                    previous_scored_file = Some((hit.score, hit.path.clone()));
                    let object = proven.candidate.as_object_mut().ok_or_else(|| {
                        BenchError::Protocol("proved candidate is not an object".to_string())
                    })?;
                    let _previous = object.insert("score".to_string(), Value::Number(score));
                }
                if plan.policy == QueryInputPolicy::ExactSymbolName
                    && proven.unit_kind != PublishedUnitKind::Symbol
                {
                    return Err(BenchError::Protocol(
                        "symbol rank requires published symbol units".to_string(),
                    ));
                }
                if ordering_contract(plan.policy).is_some() && !seen_files.insert(hit.path.as_str())
                {
                    return Err(BenchError::Protocol(format!(
                        "{} returned duplicate file path: {}",
                        plan.policy.as_str(),
                        hit.path
                    )));
                }
                if !seen_unit_ids.insert(&hit.candidate_id) {
                    return Err(BenchError::Protocol(format!(
                        "SDK returned a duplicate published unit ID: {}",
                        hit.candidate_id
                    )));
                }
                if plan.policy != QueryInputPolicy::ExactSymbolName {
                    if seen_spans.insert(proven.scored_span) {
                        candidates.push(proven.candidate);
                    }
                    continue;
                }
                if !seen_symbol_spans.insert((proven.scored_span, proven.indexed_span)) {
                    return Err(BenchError::Protocol(format!(
                        "SDK returned a duplicate published symbol indexed span: {}",
                        hit.path
                    )));
                }
                candidates.push(proven.candidate);
            }
            bind_rank_unit(
                serde_json::json!({
                    "task_id": task_id,
                    "route": route,
                    "status": classification.status,
                    "candidates": candidates,
                    "query_identity": query_identity,
                    "timings": timings_value(*latency)?,
                    "error": null,
                }),
                plan.policy,
            )
        }
        QueryOutcome::RejectedResponse { latency, .. }
        | QueryOutcome::SdkFailure { latency, .. } => bind_rank_unit(
            serde_json::json!({
                "task_id": task_id,
                "route": route,
                "status": classification.status,
                "candidates": [],
                "query_identity": query_identity,
                "timings": timings_value(*latency)?,
                "error": error_value(
                    classification.error_code.as_deref().unwrap_or("missing_error_code"),
                    classification.error_message.as_deref().unwrap_or("missing error message"),
                ),
            }),
            plan.policy,
        ),
    }
}

/// Assemble the complete v5 runner record.
///
/// Results emit in deterministic `(task_id, route)` order from the
/// pack's task order and sorted routes. Every route resolves to a
/// validated capture; unreferenced captures refuse (a capture with no
/// route is meaningless provenance). Every result carries the per-task
/// query identity of the single shared plan (RBR-02), and the runner
/// block binds the policy/config identity all plans agreed on.
#[derive(Clone, Copy)]
pub struct RunnerRecordInput<'a> {
    pub pack: &'a QueryPack,
    pub identity: &'a RunnerIdentity,
    pub provenance: &'a BTreeMap<String, RouteProvenance>,
    pub captures: &'a BTreeMap<String, CaptureProvenance>,
    pub outcomes: &'a BTreeMap<(String, String), QueryOutcome>,
    pub completed_results: &'a BTreeMap<(String, String), Value>,
    pub plans: &'a BTreeMap<String, QueryPlan>,
    pub nl_config: &'a NlPlanConfig,
    pub top_k: u32,
}

pub fn runner_record(input: &RunnerRecordInput<'_>) -> BenchResult<Value> {
    let RunnerRecordInput {
        pack,
        identity,
        provenance,
        captures,
        outcomes,
        completed_results,
        plans,
        nl_config,
        top_k,
    } = *input;
    if top_k != pack.contract_top_k {
        return Err(BenchError::Protocol(format!(
            "runner top_k={top_k} differs from the comparison contract top_k={}",
            pack.contract_top_k
        )));
    }
    // Every task must have exactly one plan, and all plans must agree on
    // the policy/config identity: a record cannot mix policies.
    let mut agreed_policy: Option<(QueryInputPolicy, String, bool)> = None;
    for task in &pack.tasks {
        let plan = plans.get(&task.task_id).ok_or_else(|| {
            BenchError::Protocol(format!("missing query plan for task {}", task.task_id))
        })?;
        if sha256_hex(plan.original.as_bytes()) != task.query_sha256 {
            return Err(BenchError::Protocol(format!(
                "query plan for task {} does not bind the pack query digest",
                task.task_id
            )));
        }
        let derived_plan = plan_query(plan.policy, &task.query, nl_config).map_err(|error| {
            BenchError::Protocol(format!(
                "query plan for task {} cannot be re-derived: {error}",
                task.task_id
            ))
        })?;
        if *plan != derived_plan {
            return Err(BenchError::Protocol(format!(
                "query plan for task {} differs from canonical request identity",
                task.task_id
            )));
        }
        let signature = (
            plan.policy,
            plan.policy_config_sha256.clone(),
            plan.planning_cost_in_latency,
        );
        match &agreed_policy {
            None => agreed_policy = Some(signature),
            Some(expected) => {
                if &signature != expected {
                    return Err(BenchError::Protocol(format!(
                        "query plans for task {} disagree on policy/config identity",
                        task.task_id
                    )));
                }
            }
        }
    }
    let (policy, _policy_config_sha256, _planning_cost_in_latency) =
        agreed_policy.ok_or_else(|| BenchError::Protocol("query pack has no tasks".to_string()))?;
    let expected_profile = execution_profile_value(policy, nl_config);
    let expected_profile_sha256 = execution_profile_sha256(policy, nl_config);
    let mut routes: Vec<&String> = provenance.keys().collect();
    routes.sort();
    if routes.is_empty() {
        return Err(BenchError::Protocol(
            "runner record needs at least one route".to_string(),
        ));
    }
    for route in &routes {
        if !pack.routes.iter().any(|name| name == *route) {
            return Err(BenchError::Protocol(format!(
                "route {route} is not registered in the query pack"
            )));
        }
    }
    let mut provenance_value = Map::new();
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for route in &routes {
        let entry = provenance
            .get(*route)
            .ok_or_else(|| BenchError::Protocol(format!("missing provenance for route {route}")))?;
        if entry.capture_id.trim().is_empty() {
            return Err(BenchError::Protocol(format!(
                "route {route} has an empty capture_id"
            )));
        }
        if !captures.contains_key(&entry.capture_id) {
            return Err(BenchError::Protocol(format!(
                "route {route} references unknown capture_id {}",
                entry.capture_id
            )));
        }
        let _used = referenced.insert(entry.capture_id.as_str());
        let _previous = provenance_value.insert(
            (*route).clone(),
            serde_json::json!({"capture_id": entry.capture_id}),
        );
    }
    let mut captures_value = Map::new();
    for (capture_id, capture) in captures {
        if !referenced.contains(capture_id.as_str()) {
            return Err(BenchError::Protocol(format!(
                "capture_id {capture_id} is not referenced by any route"
            )));
        }
        if capture.execution_profile != expected_profile
            || capture.execution_profile_sha256 != expected_profile_sha256
        {
            return Err(BenchError::Protocol(format!(
                "capture_id {capture_id} execution profile differs from the task plans"
            )));
        }
        let _previous =
            captures_value.insert(capture_id.clone(), capture_value(capture_id, capture)?);
    }
    let mut results = Vec::new();
    for task in &pack.tasks {
        let plan = plans
            .get(&task.task_id)
            .ok_or_else(|| BenchError::Protocol(format!("missing plan for {}", task.task_id)))?;
        for route in &routes {
            let outcome = outcomes
                .get(&(task.task_id.clone(), (*route).clone()))
                .ok_or_else(|| {
                    BenchError::Protocol(format!("missing outcome for ({}, {route})", task.task_id))
                })?;
            let row = completed_results
                .get(&(task.task_id.clone(), (*route).clone()))
                .ok_or_else(|| {
                    BenchError::Protocol(format!(
                        "missing completed response for ({}, {route})",
                        task.task_id
                    ))
                })?;
            let classification = outcome
                .classification()
                .map_err(|message| BenchError::Protocol(message.to_string()))?;
            if row["task_id"] != task.task_id
                || row["route"] != **route
                || row["status"] != classification.status
                || row["timings"] != timings_value(outcome.latency())?
                || row["query_identity"]
                    != serde_json::json!({
                        "original_query_sha256": plan.original_query_sha256,
                        "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
                        "semantic_text_sha256": plan.semantic_text_sha256,
                    })
            {
                return Err(BenchError::Protocol(
                    "completed response binding differs".to_string(),
                ));
            }
            results.push(row.clone());
        }
    }
    if completed_results.len() != results.len() {
        return Err(BenchError::Protocol(
            "completed response inventory differs".to_string(),
        ));
    }
    Ok(serde_json::json!({
        "schema_version": RUNNER_SCHEMA_VERSION,
        "span_accounting_version": 1,
        "query_pack_sha256": pack.pack_sha256,
        "comparison_contract": pack.comparison_contract,
        "runner": {
            "name": identity.name,
            "revision": identity.revision,
            "run_id": identity.run_id,
            "tokenizer": TOKENIZER,
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            "gold_access": false,
            "blinding": identity.blinding,
            "isolation_method": identity.isolation_method,
            "access_block_log": identity.access_block_log,
        },
        "captures": captures_value,
        "route_provenance": provenance_value,
        "results": results,
    }))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixture JSON assertions intentionally index known keys"
)]
mod tests {
    use super::*;
    use crate::chunking::Chunk;
    use crate::sdk::{FileHitAuthority, RouteExplanation};
    use quanta_index_contract::{
        ManifestGeneration, PreviewByteRange, PreviewKind, PreviewMetadata, QueryResultWindowV2,
        RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision,
    };
    use sha2::Digest as _;

    fn file_hit_fixture(
        path: &str,
        text: &str,
        path_only: bool,
    ) -> (RankedHit, BTreeMap<String, SourceFile>) {
        let repo = RepoId::new("bench-repo").expect("repo ID");
        let revision = RevisionId::new("bench-revision").expect("revision ID");
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: repo.clone(),
                repo_relative_path: RepoRelativePath::new(path.to_string()),
            },
            revision_id: revision.clone(),
            source_sha256: sha2::Sha256::digest(text.as_bytes()).into(),
        };
        let preview = if path_only {
            PreviewMetadata {
                kind: PreviewKind::Path,
                source: Some(source.clone()),
                chunk_start_byte: None,
                original_focus: None,
                original_context: None,
                normalized_focus: None,
                normalization_equivalent: false,
                unavailable_reason: None,
            }
        } else {
            let end = u64::try_from(text.len()).expect("fixture length");
            PreviewMetadata {
                kind: PreviewKind::SourceFile,
                source: Some(source.clone()),
                chunk_start_byte: None,
                original_focus: Some(PreviewByteRange { start: 0, end }),
                original_context: Some(PreviewByteRange { start: 0, end }),
                normalized_focus: Some(PreviewByteRange { start: 0, end }),
                normalization_equivalent: true,
                unavailable_reason: None,
            }
        };
        let (line_starts, exotic) = crate::corpus::split_line_starts(text);
        assert!(!exotic);
        let hit = RankedHit {
            candidate_id: expected_file_candidate_id(repo.as_str(), path).expect("file ID"),
            path: path.to_string(),
            start_line: u32::from(!path_only),
            end_line: if path_only {
                0
            } else {
                u32::try_from(line_starts.len()).expect("lines")
            },
            snippet: if path_only {
                path.to_string()
            } else {
                text.to_string()
            },
            score: 2.0,
            file_authority: Some(FileHitAuthority {
                source_repo_id: repo.clone(),
                source,
                preview,
                repo_id: repo,
                revision_id: revision,
                generation: ManifestGeneration::new(1),
            }),
            contributions: Vec::new(),
        };
        let file = SourceFile {
            path: path.to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts,
            sha256: sha256_hex(text.as_bytes()),
        };
        (hit, BTreeMap::from([(path.to_string(), file)]))
    }

    #[test]
    fn file_hit_proof_accepts_content_and_empty_path_file() {
        let (hit, files) = file_hit_fixture("src/main.go", "package main\n", false);
        let proven = prove_file_hit(&hit, 1, &files).expect("source-bound file hit");
        assert_eq!(proven.unit_kind, PublishedUnitKind::File);
        assert_eq!(proven.candidate["start_byte"], 0);
        assert_eq!(proven.candidate["end_byte"], 13);
        assert_eq!(proven.candidate["span_accounting"]["unit_kind"], "file");

        let (empty, files) = file_hit_fixture("empty.go", "", true);
        let proven = prove_file_hit(&empty, 1, &files).expect("empty path-only file hit");
        assert_eq!(proven.candidate["start_line"], 0);
        assert_eq!(proven.candidate["end_byte"], 0);
        assert_eq!(proven.candidate["tokens"], 3);
    }

    #[test]
    fn scored_file_profiles_preserve_file_identity_and_native_score() {
        let (hit, files) = file_hit_fixture("src/main.go", "func load_json() {}\n", false);
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &files,
        )
        .expect("registry");
        for (policy, raw) in [
            (QueryInputPolicy::CodeSearchTypoFile, "load_jsom"),
            (QueryInputPolicy::CodeSearchExactContentFile, "load_json()"),
        ] {
            let plan = plan_query(policy, raw, &NlPlanConfig::default()).expect("file plan");
            let row = result_value(
                "T1",
                "lexical",
                &QueryOutcome::ReturnedWindow {
                    hits: vec![hit.clone()],
                    window: QueryResultWindowV2::exact_probe(1),
                    explanation: Some(RouteExplanation::default()),
                    latency: Duration::from_millis(1),
                },
                &plan,
                10,
                &files,
                &units,
            )
            .expect("source-bound file result");
            assert_eq!(row["rank_unit"], "distinct_file");
            assert_eq!(row["score_evidence"], "native_sdk_score_v1");
            assert_eq!(row["candidates"][0]["score"].as_f64(), Some(2.0));
            assert_eq!(row["candidates"][0]["span_accounting"]["unit_kind"], "file");
            assert_eq!(
                row["query_identity"]["effective_lexical_request_sha256"],
                plan.effective_lexical_request_sha256
            );
        }
    }

    #[test]
    fn natural_language_file_preserves_native_representative_chunk_authority() {
        // Native select:file groups before top-k, but retains the winning
        // chunk identity. Rank unit and published unit kind are independent.
        let (files, units, hit) = status_fixture();
        let plan = plan_query(
            QueryInputPolicy::NaturalLanguageFile,
            "find main",
            &NlPlanConfig::default(),
        )
        .expect("native file plan");
        let outcome = QueryOutcome::ReturnedWindow {
            hits: vec![hit.clone()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        let row = result_value("T1", "lexical", &outcome, &plan, 10, &files, &units)
            .expect("source-bound native representative");
        assert_eq!(row["rank_unit"], "distinct_file");
        assert_eq!(row["score_evidence"], "native_sdk_score_v1");
        assert_eq!(row["candidates"][0]["score"].as_f64(), Some(1.0));
        assert_eq!(
            row["candidates"][0]["span_accounting"]["unit_kind"],
            "chunk"
        );
        assert_eq!(
            row["candidates"][0]["span_accounting"]["unit_id"],
            "chunk-id"
        );
        let duplicate = QueryOutcome::ReturnedWindow {
            hits: vec![hit.clone(), hit],
            window: QueryResultWindowV2::exact_probe(2),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        assert!(
            result_value("T1", "lexical", &duplicate, &plan, 10, &files, &units)
                .expect_err("duplicate files must fail")
                .to_string()
                .contains("duplicate file path")
        );
        let (file_hit, files) = file_hit_fixture("a.txt", "fn main() {}\n", false);
        let wrong_identity = QueryOutcome::ReturnedWindow {
            hits: vec![file_hit],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        assert!(
            result_value("T1", "lexical", &wrong_identity, &plan, 10, &files, &units)
                .expect_err("CodeSearch identity is not a Native representative")
                .to_string()
                .contains("incompatible file identity")
        );
    }

    #[test]
    fn file_hit_proof_rejects_identity_preview_and_source_drift() {
        let (hit, files) = file_hit_fixture("src/main.go", "package main\n", false);
        let mut forged = hit.clone();
        forged.candidate_id = "file:invalid".to_string();
        assert!(prove_file_hit(&forged, 1, &files).is_err());
        let (mut empty, empty_files) = file_hit_fixture("empty.go", "", true);
        empty.snippet = "forged.go".to_string();
        assert!(prove_file_hit(&empty, 1, &empty_files).is_err());
        let mut forged = hit.clone();
        forged.snippet = "package fake\n".to_string();
        assert!(prove_file_hit(&forged, 1, &files).is_err());
        let mut forged = hit;
        forged
            .file_authority
            .as_mut()
            .expect("authority")
            .source
            .source_sha256 = [0; 32];
        assert!(prove_file_hit(&forged, 1, &files).is_err());
    }

    #[test]
    fn gold_bearing_pack_keys_are_rejected() {
        for raw in [
            r#"{"tasks":[{"task_id":"t","query":"q","query_sha256":"s","gold":[]}]}"#,
            r#"{"tasks":[],"grade":3}"#,
            r#"{"tasks":[],"answerable":true}"#,
        ] {
            let value: Value = serde_json::from_str(raw).expect("fixture parses");
            assert!(forbidden_pack_key(&value).is_some(), "must flag {raw}");
        }
        let clean: Value = serde_json::from_str(r#"{"tasks":[{"task_id":"t","query":"q"}]}"#)
            .expect("fixture parses");
        assert!(forbidden_pack_key(&clean).is_none());
    }

    #[test]
    fn repeated_pack_keys_are_rejected_before_gold_can_be_hidden() {
        let raw = r#"{"tasks":[{"gold":[{"path":"secret"}]}],"tasks":[],"routes":[]}"#;
        let error = serde_json::from_str::<UniqueJson>(raw)
            .err()
            .expect("duplicate key must fail");
        assert!(error.to_string().contains("duplicate JSON key: tasks"));
        assert!(serde_json::from_str::<UniqueJson>(r#"{"x":{"a":1,"a":2}}"#).is_err());
    }

    #[test]
    fn pack_contract_rejects_unknown_fields_that_could_leak_labels() {
        let mut object = Map::new();
        assert!(
            object
                .insert("task_id".to_string(), Value::String("q1".to_string()))
                .is_none()
        );
        assert!(
            object
                .insert("query".to_string(), Value::String("needle".to_string()))
                .is_none()
        );
        assert!(
            object
                .insert(
                    "query_sha256".to_string(),
                    Value::String("digest".to_string())
                )
                .is_none()
        );
        assert!(exact_keys(&object, &["task_id", "query", "query_sha256"], "task").is_ok());
        assert!(
            object
                .insert("relevant_spans".to_string(), Value::Array(Vec::new()))
                .is_none()
        );
        assert!(exact_keys(&object, &["task_id", "query", "query_sha256"], "task").is_err());
    }

    #[test]
    fn unanchored_semantic_hit_uses_only_the_exact_published_chunk() {
        let chunk_text = "fn main() {}\n";
        let text = "fn main() {}\nfn second() {}\n";
        let path = "src/lib.rs";
        let file = SourceFile {
            path: path.to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts: vec![0, chunk_text.len()],
            sha256: sha256_hex(text.as_bytes()),
        };
        let mut other = file.clone();
        other.path = "other.rs".to_string();
        let files = BTreeMap::from([(path.to_string(), file), (other.path.clone(), other)]);
        let chunk = Chunk {
            path: path.to_string(),
            start_byte: 0,
            end_byte: u32::try_from(chunk_text.len()).expect("short fixture"),
            start_line: 1,
            end_line: 1,
            text: chunk_text.to_string(),
            strategy: "whole_file".to_string(),
            version: "test".to_string(),
            config: "test".to_string(),
            chunk_id: "chunk-id".to_string(),
            fallback: false,
        };
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::from([(path.to_string(), vec![chunk])]),
            &BTreeMap::new(),
            &files,
        )
        .expect("registry");
        let hit = RankedHit {
            candidate_id: "chunk-id".to_string(),
            path: path.to_string(),
            start_line: 0,
            end_line: 0,
            snippet: chunk_text.to_string(),
            score: 1.0,
            file_authority: None,
            contributions: Vec::new(),
        };
        let candidate = prove_hit(&hit, 1, &files, &units)
            .expect("anchored by published ID")
            .candidate;
        assert_eq!(
            candidate.get("start_line"),
            Some(&Value::Number(1_u64.into()))
        );
        assert_eq!(
            candidate.get("end_line"),
            Some(&Value::Number(1_u64.into()))
        );
        assert_eq!(
            candidate.get("block_sha256").and_then(Value::as_str),
            Some(sha256_hex(chunk_text.as_bytes()).as_str())
        );

        let mut changed = hit.clone();
        changed.snippet = "not the published chunk".to_string();
        assert!(prove_hit(&changed, 1, &files, &units).is_err());
        changed = hit.clone();
        changed.candidate_id = "unknown".to_string();
        assert!(prove_hit(&changed, 1, &files, &units).is_err());
        changed = hit;
        changed.end_line = 1;
        assert!(prove_hit(&changed, 1, &files, &units).is_err());

        let mut anchored = changed;
        anchored.start_line = 1;
        assert!(prove_hit(&anchored, 1, &files, &units).is_ok());
        anchored.candidate_id = "unknown".to_string();
        assert!(prove_hit(&anchored, 1, &files, &units).is_err());
        anchored.candidate_id = "chunk-id".to_string();
        anchored.path = "other.rs".to_string();
        assert!(prove_hit(&anchored, 1, &files, &units).is_err());
        anchored.path = path.to_string();
        anchored.end_line = 2;
        assert!(prove_hit(&anchored, 1, &files, &units).is_err());
    }

    #[test]
    fn same_line_symbols_keep_distinct_published_units() {
        use quanta_index_contract::QueryResultWindowV2;

        let path = "src/symbols.go";
        let text = "package fixture; type A struct{}; type B struct{}; func (A) writeContentType() {}; func (B) writeContentType() {}\n";
        let file = SourceFile {
            path: path.to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts: vec![0],
            sha256: sha256_hex(text.as_bytes()),
        };
        let files = BTreeMap::from([(path.to_string(), file)]);
        let extraction = crate::symbols::extract_corpus_symbols(&files).expect("symbol extraction");
        let symbols = extraction.symbols.get(path).expect("file symbols");
        let symbols: Vec<_> = symbols
            .iter()
            .filter(|symbol| symbol.local_name.as_ref() == "writeContentType")
            .collect();
        assert_eq!(symbols.len(), 2, "two independent receiver declarations");
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::new(),
            &extraction.symbols,
            &files,
        )
        .expect("published symbols");
        let hits = symbols
            .iter()
            .map(|symbol| RankedHit {
                candidate_id: symbol.symbol_id.as_str().to_string(),
                path: path.to_string(),
                start_line: symbol.definition_span.line_start,
                end_line: symbol.definition_span.line_end,
                snippet: symbol.local_name.to_string(),
                score: 1.0,
                file_authority: None,
                contributions: Vec::new(),
            })
            .collect();
        let outcome = QueryOutcome::ReturnedWindow {
            hits,
            window: QueryResultWindowV2::exact_probe(2),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        let plan = plan_query(
            QueryInputPolicy::ExactSymbolName,
            "writeContentType",
            &NlPlanConfig::default(),
        )
        .expect("exact symbol plan");
        let result = result_value("T1", "symbol", &outcome, &plan, 10, &files, &units)
            .expect("same-line symbol result");
        let candidates = result["candidates"].as_array().expect("candidates");
        assert_eq!(result["rank_unit"], "symbol");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0]["start_byte"], candidates[1]["start_byte"]);
        assert_eq!(candidates[0]["end_byte"], candidates[1]["end_byte"]);
        assert_ne!(
            candidates[0]["span_accounting"]["unit_id"],
            candidates[1]["span_accounting"]["unit_id"]
        );
        assert_ne!(
            candidates[0]["span_accounting"]["indexed_start_byte"],
            candidates[1]["span_accounting"]["indexed_start_byte"]
        );
        assert_eq!(candidates[0]["rank"], 1);
        assert_eq!(candidates[1]["rank"], 2);
        let legacy_plan = plan_query(
            QueryInputPolicy::Native,
            "writeContentType",
            &NlPlanConfig::default(),
        )
        .expect("native symbol plan");
        let legacy = result_value("T1", "symbol", &outcome, &legacy_plan, 10, &files, &units)
            .expect("legacy context scoring remains valid");
        let legacy_candidates = legacy["candidates"].as_array().expect("legacy candidates");
        assert_eq!(legacy_candidates.len(), 1);
        assert!(legacy.get("rank_unit").is_none());
        assert_eq!(
            legacy_candidates[0]["span_accounting"]["unit_id"],
            candidates[0]["span_accounting"]["unit_id"]
        );
    }

    #[test]
    fn pack_universe_digest_matches_evaluator_oracle() {
        // Independent oracle: evaluator.universe_digest over the same rows.
        let one = vec![("a.txt".to_string(), "a".repeat(64))];
        assert_eq!(
            pack_universe_digest(&one).expect("digest"),
            "8451038bb67eef77ce2bc5966579609ed4555d18bfdddad0569e1566709694c3"
        );
        let two = vec![
            ("b.txt".to_string(), "b".repeat(64)),
            ("a.txt".to_string(), "a".repeat(64)),
        ];
        assert_eq!(
            pack_universe_digest(&two).expect("digest"),
            "4b65c29126852e1fdcac332733a3ea4ede6bf60bc4f2714e3ddf7880ba70e717"
        );
    }

    fn v3_pack_fixture() -> Value {
        let universe = vec![("a.txt".to_string(), "a".repeat(64))];
        let digest = pack_universe_digest(&universe).expect("universe digest");
        let query = "needle";
        serde_json::json!({
            "schema_version": 3,
            "suite_id": "s",
            "suite_commitment_sha256": "c".repeat(64),
            "repository_commit": "d".repeat(40),
            "tokenizer": TOKENIZER,
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            "routes": ["lexical"],
            "file_universe": [{"path": "a.txt", "file_sha256": "a".repeat(64)}],
            "file_universe_digest": digest,
            "comparison_contract": {
                "top_k": 10,
                "tokenizer": TOKENIZER,
                "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
                "output_unit_policy": OUTPUT_UNIT_POLICY,
                "span_unit": SPAN_UNIT,
            },
            "tasks": [{
                "task_id": "T1",
                "query": query,
                "query_sha256": sha256_hex(query.as_bytes()),
            }],
        })
    }

    fn load_fixture(pack: &Value) -> BenchResult<QueryPack> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pack.json");
        std::fs::write(&path, serde_json::to_string(pack).expect("pack renders"))
            .expect("pack writes");
        load_query_pack(&path)
    }

    fn completed_results_fixture(
        outcomes: &BTreeMap<(String, String), QueryOutcome>,
        plans: &BTreeMap<String, QueryPlan>,
        files: &BTreeMap<String, SourceFile>,
        units: &PublishedUnitRegistry,
    ) -> BTreeMap<(String, String), Value> {
        outcomes
            .iter()
            .map(|((task, route), outcome)| {
                let value = result_value(task, route, outcome, &plans[task], 10, files, units)
                    .expect("fixture normalized response");
                ((task.clone(), route.clone()), value)
            })
            .collect()
    }

    fn native_plans_fixture(pack: &QueryPack) -> BTreeMap<String, QueryPlan> {
        pack.tasks
            .iter()
            .map(|task| {
                (
                    task.task_id.clone(),
                    plan_query(
                        QueryInputPolicy::Native,
                        task.query.as_str(),
                        &NlPlanConfig::default(),
                    )
                    .expect("native plan"),
                )
            })
            .collect()
    }

    #[test]
    fn v3_pack_loads_and_echoes_contract() {
        let pack = load_fixture(&v3_pack_fixture()).expect("v3 pack loads");
        assert_eq!(pack.contract_top_k, 10);
        assert_eq!(
            pack.comparison_contract["span_unit"].as_str(),
            Some(SPAN_UNIT)
        );
        assert_eq!(pack.pack_sha256.len(), 64);
        assert_eq!(
            pack.file_universe,
            vec![("a.txt".to_string(), "a".repeat(64))]
        );
    }

    #[test]
    fn v3_pack_rejects_v2_and_contract_drift() {
        let mut legacy = v3_pack_fixture();
        legacy["schema_version"] = Value::from(2);
        assert!(load_fixture(&legacy).is_err());
        for mutate in [
            |pack: &mut Value| pack["comparison_contract"]["top_k"] = Value::from(0),
            |pack: &mut Value| pack["comparison_contract"]["span_unit"] = Value::from("lines"),
            |pack: &mut Value| {
                pack["comparison_contract"]["output_unit_policy"] = Value::from("other");
            },
            |pack: &mut Value| pack["file_universe_digest"] = Value::from("0".repeat(64)),
        ] {
            let mut pack = v3_pack_fixture();
            mutate(&mut pack);
            assert!(load_fixture(&pack).is_err());
        }
    }

    fn status_fixture() -> (
        BTreeMap<String, SourceFile>,
        PublishedUnitRegistry,
        RankedHit,
    ) {
        let text = "fn main() {}\n";
        let file = SourceFile {
            path: "a.txt".to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts: vec![0],
            sha256: sha256_hex(text.as_bytes()),
        };
        let chunk = Chunk {
            path: "a.txt".to_string(),
            start_byte: 0,
            end_byte: u32::try_from(text.len()).expect("short"),
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
            strategy: "whole_file".to_string(),
            version: "test".to_string(),
            config: "test".to_string(),
            chunk_id: "chunk-id".to_string(),
            fallback: false,
        };
        let hit = RankedHit {
            candidate_id: "chunk-id".to_string(),
            path: "a.txt".to_string(),
            start_line: 1,
            end_line: 1,
            snippet: text.to_string(),
            score: 1.0,
            file_authority: None,
            contributions: Vec::new(),
        };
        let files = BTreeMap::from([("a.txt".to_string(), file)]);
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::from([("a.txt".to_string(), vec![chunk])]),
            &BTreeMap::new(),
            &files,
        )
        .expect("registry");
        (files, units, hit)
    }

    #[test]
    fn outcome_statuses_never_silently_downgrade() {
        use quanta_index_contract::{
            CandidateCountV1, CoverageV1, EmptyProvenanceV2, ExaminedUniverseV1,
            ExecutionOutcomeV2, QueryResultWindowV2,
        };
        let (files, chunks, hit) = status_fixture();
        let plan = plan_query(QueryInputPolicy::Native, "needle", &NlPlanConfig::default())
            .expect("native plan");
        let run = |outcome: QueryOutcome| {
            result_value("T1", "lexical", &outcome, &plan, 10, &files, &chunks).expect("maps")
        };
        let exhausted = run(QueryOutcome::ReturnedWindow {
            hits: vec![hit.clone()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        });
        assert_eq!(exhausted["status"].as_str(), Some("success"));
        let capped = run(QueryOutcome::ReturnedWindow {
            hits: vec![hit],
            window: QueryResultWindowV2::new(
                1,
                CandidateCountV1::AtLeast(1),
                ExecutionOutcomeV2::LowerBound {
                    continuation: false,
                },
                CoverageV1::new(ExaminedUniverseV1::AtLeast(1), None, Vec::new()),
                None,
            )
            .expect("capped window"),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        });
        assert_eq!(capped["status"].as_str(), Some("capped"));
        let empty_capped = run(QueryOutcome::ReturnedWindow {
            hits: Vec::new(),
            window: QueryResultWindowV2::new(
                0,
                CandidateCountV1::AtLeast(1),
                ExecutionOutcomeV2::LowerBound { continuation: true },
                CoverageV1::new(ExaminedUniverseV1::AtLeast(1), None, Vec::new()),
                Some(EmptyProvenanceV2::ZeroHitExecuted),
            )
            .expect("empty capped window"),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        });
        assert_eq!(empty_capped["status"].as_str(), Some("error"));
        assert_eq!(
            empty_capped["error"]["code"].as_str(),
            Some("empty_non_exhausted_window")
        );
        let abstained = run(QueryOutcome::ReturnedWindow {
            hits: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        });
        assert_eq!(abstained["status"].as_str(), Some("abstained"));
        // A capped rank is never treated as exhaustive: it keeps its
        // status even when the merge would otherwise score it.
        assert_eq!(capped["candidates"].as_array().expect("hits").len(), 1);
    }

    #[test]
    fn literal_file_result_records_rank_unit_and_refuses_duplicate_paths() {
        use quanta_index_contract::QueryResultWindowV2;

        let (files, units, hit) = status_fixture();
        let plan = plan_query(
            QueryInputPolicy::LiteralFile,
            "main",
            &NlPlanConfig::default(),
        )
        .expect("literal file plan");
        let single = result_value(
            "T1",
            "lexical",
            &QueryOutcome::ReturnedWindow {
                hits: vec![hit.clone()],
                window: QueryResultWindowV2::exact_probe(1),
                explanation: Some(RouteExplanation::default()),
                latency: Duration::from_millis(1),
            },
            &plan,
            10,
            &files,
            &units,
        )
        .expect("one file result");
        assert_eq!(single["rank_unit"], "distinct_file");
        assert_eq!(
            single["candidates"][0]["span_accounting"]["unit_kind"],
            "chunk"
        );

        let empty = result_value(
            "T1",
            "lexical",
            &QueryOutcome::ReturnedWindow {
                hits: Vec::new(),
                window: QueryResultWindowV2::exact_probe(0),
                explanation: Some(RouteExplanation::default()),
                latency: Duration::from_millis(1),
            },
            &plan,
            10,
            &files,
            &units,
        )
        .expect("file abstention");
        assert_eq!(empty["rank_unit"], "distinct_file");
        assert_eq!(empty["status"], "abstained");

        let duplicate = result_value(
            "T1",
            "lexical",
            &QueryOutcome::ReturnedWindow {
                hits: vec![hit.clone(), hit],
                window: QueryResultWindowV2::exact_probe(2),
                explanation: Some(RouteExplanation::default()),
                latency: Duration::from_millis(1),
            },
            &plan,
            10,
            &files,
            &units,
        );
        assert!(duplicate.is_err_and(|error| error.to_string().contains("duplicate file path")));
    }

    #[test]
    fn keyword_file_result_preserves_finite_native_score() {
        use quanta_index_contract::QueryResultWindowV2;

        let (files, units, hit) = status_fixture();
        let plan = plan_query(
            QueryInputPolicy::KeywordFile,
            "main",
            &NlPlanConfig::default(),
        )
        .expect("keyword file plan");
        let outcome = |score| QueryOutcome::ReturnedWindow {
            hits: vec![RankedHit {
                score,
                ..hit.clone()
            }],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        let row = result_value("T1", "lexical", &outcome(2.5), &plan, 10, &files, &units)
            .expect("finite score is recorded");
        assert_eq!(row["score_evidence"], "native_sdk_score_v1");
        assert_eq!(row["candidates"][0]["score"].as_f64(), Some(2.5));
        assert!(
            result_value(
                "T1",
                "lexical",
                &outcome(f64::NAN),
                &plan,
                10,
                &files,
                &units,
            )
            .is_err()
        );

        let mut second = files["a.txt"].clone();
        second.path = "b.txt".to_string();
        let files = BTreeMap::from([
            ("a.txt".to_string(), files["a.txt"].clone()),
            ("b.txt".to_string(), second),
        ]);
        let make_chunk = |path: &str, id: &str| Chunk {
            path: path.to_string(),
            start_byte: 0,
            end_byte: u32::try_from(files[path].bytes.len()).expect("short fixture"),
            start_line: 1,
            end_line: 1,
            text: files[path].text.clone(),
            strategy: "whole_file".to_string(),
            version: "test".to_string(),
            config: "test".to_string(),
            chunk_id: id.to_string(),
            fallback: false,
        };
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::from([
                ("a.txt".to_string(), vec![make_chunk("a.txt", "chunk-id")]),
                ("b.txt".to_string(), vec![make_chunk("b.txt", "chunk-b")]),
            ]),
            &BTreeMap::new(),
            &files,
        )
        .expect("two-file registry");
        let second_hit = RankedHit {
            path: "b.txt".to_string(),
            candidate_id: "chunk-b".to_string(),
            ..hit.clone()
        };
        let two = |first: RankedHit, second: RankedHit| QueryOutcome::ReturnedWindow {
            hits: vec![first, second],
            window: QueryResultWindowV2::exact_probe(2),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        assert!(
            result_value(
                "T1",
                "lexical",
                &two(
                    hit.clone(),
                    RankedHit {
                        score: 2.0,
                        ..second_hit.clone()
                    }
                ),
                &plan,
                10,
                &files,
                &units,
            )
            .is_err()
        );
        assert!(
            result_value(
                "T1",
                "lexical",
                &two(second_hit, hit),
                &plan,
                10,
                &files,
                &units,
            )
            .is_err()
        );
    }

    fn v3_capture_fixture() -> CaptureProvenance {
        CaptureProvenance {
            chunk_strategy: "whole_file".to_string(),
            chunk_config: serde_json::json!({}),
            runner_binary_name: "quanta-sdk-runner".to_string(),
            runner_binary_digest: "e".repeat(64),
            searchd_binary_digest: "f".repeat(64),
            generation: 7,
            source_repo_id: "bench-repo".to_string(),
            source_revision_id: "bench-revision".to_string(),
            receipt_digest: "a".repeat(64),
            activation_digest: "b".repeat(64),
            model: "none:lexical".to_string(),
            model_revision: "not-applicable".to_string(),
            execution_profile: execution_profile_value(
                QueryInputPolicy::Native,
                &NlPlanConfig::default(),
            ),
            execution_profile_sha256: execution_profile_sha256(
                QueryInputPolicy::Native,
                &NlPlanConfig::default(),
            ),
        }
    }

    #[test]
    fn quanta_capture_binds_source_pin() {
        let mut capture = v3_capture_fixture();
        let historical = capture_value("cap-1", &capture).expect("historical capture");
        assert!(historical.get("source_repo_id").is_none());
        assert!(historical.get("source_revision_id").is_none());
        for policy in [
            QueryInputPolicy::CodeSearchFile,
            QueryInputPolicy::CodeSearchExactContentFile,
        ] {
            capture.execution_profile = execution_profile_value(policy, &NlPlanConfig::default());
            capture.execution_profile_sha256 =
                execution_profile_sha256(policy, &NlPlanConfig::default());
            let value = capture_value("cap-1", &capture)
                .expect("source-bound capture without candidate rows");
            assert_eq!(value["source_repo_id"], "bench-repo");
            assert_eq!(value["source_revision_id"], "bench-revision");
        }
        let mut missing = capture;
        missing.source_revision_id.clear();
        assert!(capture_value("cap-1", &missing).is_err());
    }

    #[test]
    fn current_record_assembles_captures_and_byte_spans() {
        let pack = load_fixture(&v3_pack_fixture()).expect("v3 pack loads");
        let identity = RunnerIdentity::new(
            "quanta-sdk-runner".to_string(),
            "sha256:".to_string() + &"e".repeat(64),
            "run-1".to_string(),
            "attested".to_string(),
            "m".to_string(),
            "l".to_string(),
        )
        .expect("identity");
        let text = "fn main() {}\n";
        let file = SourceFile {
            path: "a.txt".to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts: vec![0],
            sha256: sha256_hex(text.as_bytes()),
        };
        let files = BTreeMap::from([("a.txt".to_string(), file)]);
        let chunk = Chunk {
            path: "a.txt".to_string(),
            start_byte: 0,
            end_byte: u32::try_from(text.len()).expect("short"),
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
            strategy: "whole_file".to_string(),
            version: "test".to_string(),
            config: "test".to_string(),
            chunk_id: "chunk-id".to_string(),
            fallback: false,
        };
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::from([("a.txt".to_string(), vec![chunk])]),
            &BTreeMap::new(),
            &files,
        )
        .expect("registry");
        let hit = RankedHit {
            candidate_id: "chunk-id".to_string(),
            path: "a.txt".to_string(),
            start_line: 1,
            end_line: 1,
            snippet: text.to_string(),
            score: 1.0,
            file_authority: None,
            contributions: Vec::new(),
        };
        let outcome = QueryOutcome::ReturnedWindow {
            hits: vec![hit],
            window: quanta_index_contract::QueryResultWindowV2::exact_probe(1),
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(3),
        };
        let outcomes = BTreeMap::from([(("T1".to_string(), "lexical".to_string()), outcome)]);
        let provenance = BTreeMap::from([(
            "lexical".to_string(),
            RouteProvenance {
                capture_id: "cap-1".to_string(),
            },
        )]);
        let captures = BTreeMap::from([("cap-1".to_string(), v3_capture_fixture())]);
        let plans = native_plans_fixture(&pack);
        let nl_config = NlPlanConfig::default();
        let completed_results = completed_results_fixture(&outcomes, &plans, &files, &units);
        let record = runner_record(&RunnerRecordInput {
            pack: &pack,
            identity: &identity,
            provenance: &provenance,
            captures: &captures,
            outcomes: &outcomes,
            completed_results: &completed_results,
            plans: &plans,
            nl_config: &nl_config,
            top_k: 10,
        })
        .expect("v3 record assembles");
        assert_eq!(
            record["schema_version"],
            serde_json::json!(RUNNER_SCHEMA_VERSION)
        );
        assert_eq!(record["span_accounting_version"], 1);
        assert!(record["runner"].get("query_input_policy").is_none());
        assert_eq!(
            record["captures"]["cap-1"]["execution_profile"]["policy"],
            serde_json::json!("native")
        );
        assert_eq!(
            record["captures"]["cap-1"]["execution_profile_sha256"],
            serde_json::json!(execution_profile_sha256(
                QueryInputPolicy::Native,
                &NlPlanConfig::default()
            ))
        );
        assert_eq!(
            record["results"][0]["query_identity"]["original_query_sha256"],
            serde_json::json!(sha256_hex(b"needle"))
        );
        assert_eq!(record["comparison_contract"], pack.comparison_contract);
        assert_eq!(
            record["captures"]["cap-1"]["system"],
            serde_json::json!("quanta")
        );
        assert_eq!(
            record["captures"]["cap-1"]["generation"],
            serde_json::json!(7)
        );
        assert_eq!(
            record["route_provenance"]["lexical"]["capture_id"],
            serde_json::json!("cap-1")
        );
        let candidate = &record["results"][0]["candidates"][0];
        assert_eq!(candidate["start_byte"], serde_json::json!(0));
        assert_eq!(candidate["end_byte"], serde_json::json!(text.len()));
        assert_eq!(candidate["span_accounting"]["unit_kind"], "chunk");
        assert_eq!(candidate["span_accounting"]["indexed_start_byte"], 0);
        assert_eq!(candidate["span_accounting"]["indexed_end_byte"], text.len());
        assert_eq!(candidate["span_accounting"]["extra_context_bytes"], 0);
        assert_eq!(record["results"][0]["status"], serde_json::json!("success"));
        assert!(record["results"][0].get("rank_unit").is_none());
        let mut tampered_plans = plans;
        tampered_plans
            .get_mut("T1")
            .expect("plan")
            .effective_lexical_request_sha256 = "0".repeat(64);
        let tampered = runner_record(&RunnerRecordInput {
            pack: &pack,
            identity: &identity,
            provenance: &provenance,
            captures: &captures,
            outcomes: &outcomes,
            completed_results: &completed_results,
            plans: &tampered_plans,
            nl_config: &nl_config,
            top_k: 10,
        });
        assert!(
            tampered.is_err_and(|error| error.to_string().contains("canonical request identity"))
        );
    }

    #[test]
    fn indexed_span_remains_distinct_from_scored_line_projection() {
        let text = "aa fn ok() {}\n";
        let file = SourceFile {
            path: "a.rs".to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts: vec![0],
            sha256: sha256_hex(text.as_bytes()),
        };
        let chunk = Chunk {
            path: "a.rs".to_string(),
            start_byte: 3,
            end_byte: 13,
            start_line: 1,
            end_line: 1,
            text: "fn ok() {}".to_string(),
            strategy: "fixed_window_strict".to_string(),
            version: "test".to_string(),
            config: "test".to_string(),
            chunk_id: "mid-line".to_string(),
            fallback: false,
        };
        let files = BTreeMap::from([("a.rs".to_string(), file)]);
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::from([("a.rs".to_string(), vec![chunk])]),
            &BTreeMap::new(),
            &files,
        )
        .expect("published mid-line chunk");
        let hit = RankedHit {
            candidate_id: "mid-line".to_string(),
            path: "a.rs".to_string(),
            start_line: 1,
            end_line: 1,
            snippet: "fn ok() {}".to_string(),
            score: 1.0,
            file_authority: None,
            contributions: Vec::new(),
        };
        let candidate = prove_hit(&hit, 1, &files, &units)
            .expect("source-bound hit")
            .candidate;
        assert_eq!(candidate["start_byte"], 0);
        assert_eq!(candidate["end_byte"], 14);
        assert_eq!(candidate["span_accounting"]["indexed_start_byte"], 3);
        assert_eq!(candidate["span_accounting"]["indexed_end_byte"], 13);
        assert_eq!(candidate["span_accounting"]["sdk_start_line"], 1);
        assert_eq!(candidate["span_accounting"]["sdk_end_line"], 1);
        assert_eq!(candidate["span_accounting"]["extra_context_bytes"], 4);
    }

    #[test]
    fn scored_span_projection_keeps_first_rank_and_proves_duplicate_hits() {
        let text = "fn a() {} fn b() {}\nfn c() {}\n";
        let second_line = text.find("fn c").expect("second line");
        let file = SourceFile {
            path: "a.rs".to_string(),
            bytes: text.as_bytes().to_vec(),
            text: text.to_string(),
            line_starts: vec![0, second_line],
            sha256: sha256_hex(text.as_bytes()),
        };
        let files = BTreeMap::from([("a.rs".to_string(), file)]);
        let chunks: Vec<_> = [
            ("a", "fn a() {}", 1),
            ("b", "fn b() {}", 1),
            ("c", "fn c() {}", 2),
        ]
        .into_iter()
        .map(|(id, source, line)| {
            let start = text.find(source).expect("published source");
            Chunk {
                path: "a.rs".to_string(),
                start_byte: u32::try_from(start).expect("short source"),
                end_byte: u32::try_from(start + source.len()).expect("short source"),
                start_line: line,
                end_line: line,
                text: source.to_string(),
                strategy: "fixed_window_strict".to_string(),
                version: "test".to_string(),
                config: "test".to_string(),
                chunk_id: id.to_string(),
                fallback: false,
            }
        })
        .collect();
        let units = PublishedUnitRegistry::from_chunks_and_symbols(
            &BTreeMap::from([("a.rs".to_string(), chunks)]),
            &BTreeMap::new(),
            &files,
        )
        .expect("published units");
        let hits: Vec<_> = [("a", 1), ("b", 1), ("c", 2)]
            .into_iter()
            .map(|(id, line)| RankedHit {
                candidate_id: id.to_string(),
                path: "a.rs".to_string(),
                start_line: line,
                end_line: line,
                snippet: id.to_string(),
                score: 1.0,
                file_authority: None,
                contributions: Vec::new(),
            })
            .collect();
        let plan = plan_query(QueryInputPolicy::Native, "needle", &NlPlanConfig::default())
            .expect("native plan");
        let outcome = |hits: Vec<RankedHit>| QueryOutcome::ReturnedWindow {
            window: QueryResultWindowV2::exact_probe(
                u32::try_from(hits.len()).expect("short fixture"),
            ),
            hits,
            explanation: Some(RouteExplanation::default()),
            latency: Duration::from_millis(1),
        };
        let record = result_value(
            "T1",
            "semantic",
            &outcome(hits.clone()),
            &plan,
            10,
            &files,
            &units,
        )
        .expect("source-bound record");
        let candidates = record["candidates"].as_array().expect("candidates");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0]["span_accounting"]["unit_id"], "a");
        assert_eq!(candidates[0]["rank"], 1);
        assert_eq!(candidates[1]["span_accounting"]["unit_id"], "c");
        assert_eq!(candidates[1]["rank"], 2);

        let proofs = super::native_span_proofs(
            &BTreeMap::from([(
                ("T1".to_string(), "semantic".to_string()),
                outcome(hits.clone()),
            )]),
            &files,
            &units,
        )
        .expect("every native unit is proved against published source bytes");
        let raw = proofs
            .get(&("T1".to_string(), "semantic".to_string()))
            .expect("native proofs");
        assert_eq!(raw.len(), 3);
        assert_eq!(raw[0].unit_id, "a");
        assert_eq!(raw[1].unit_id, "b");
        assert_eq!(
            raw[0].span,
            (
                "a.rs".to_string(),
                0,
                u64::try_from(second_line).expect("small span")
            )
        );
        assert_eq!(raw[1].span, raw[0].span);
        assert_eq!(
            raw[2].span.1,
            u64::try_from(second_line).expect("small span")
        );

        let mut forged = hits.clone();
        forged.get_mut(1).expect("duplicate hit").candidate_id = "unknown".to_string();
        assert!(
            result_value(
                "T1",
                "semantic",
                &outcome(forged),
                &plan,
                10,
                &files,
                &units
            )
            .is_err()
        );

        let repeated_unit = vec![hits[0].clone(), hits[0].clone()];
        let duplicate = result_value(
            "T1",
            "semantic",
            &outcome(repeated_unit),
            &plan,
            10,
            &files,
            &units,
        )
        .expect_err("repeated published unit must not be hidden by span collapse");
        assert!(
            duplicate
                .to_string()
                .contains("duplicate published unit ID")
        );
    }

    #[test]
    fn current_record_refuses_broken_capture_binding() {
        let pack = load_fixture(&v3_pack_fixture()).expect("v3 pack loads");
        let identity = RunnerIdentity::new(
            "r".to_string(),
            "rev".to_string(),
            "run-1".to_string(),
            "attested".to_string(),
            "m".to_string(),
            "l".to_string(),
        )
        .expect("identity");
        let failed = QueryOutcome::SdkFailure {
            status: "error",
            code: "boom".to_string(),
            message: "typed".to_string(),
            latency: Duration::from_millis(1),
        };
        let outcomes = BTreeMap::from([(("T1".to_string(), "lexical".to_string()), failed)]);
        let files = BTreeMap::new();
        let units = PublishedUnitRegistry::default();
        let plans = native_plans_fixture(&pack);
        let nl_config = NlPlanConfig::default();
        let build = |provenance: &BTreeMap<String, RouteProvenance>,
                     captures: &BTreeMap<String, CaptureProvenance>,
                     top_k: u32|
         -> BenchResult<Value> {
            runner_record(&RunnerRecordInput {
                pack: &pack,
                identity: &identity,
                provenance,
                captures,
                outcomes: &outcomes,
                completed_results: &completed_results_fixture(&outcomes, &plans, &files, &units),
                plans: &plans,
                nl_config: &nl_config,
                top_k,
            })
        };
        let good_provenance = BTreeMap::from([(
            "lexical".to_string(),
            RouteProvenance {
                capture_id: "cap-1".to_string(),
            },
        )]);
        let good_captures = BTreeMap::from([("cap-1".to_string(), v3_capture_fixture())]);
        assert!(build(&good_provenance, &good_captures, 10).is_ok());
        // Dangling capture_id.
        let dangling = BTreeMap::from([(
            "lexical".to_string(),
            RouteProvenance {
                capture_id: "cap-9".to_string(),
            },
        )]);
        assert!(build(&dangling, &good_captures, 10).is_err());
        // Unreferenced capture.
        let mut extra = good_captures.clone();
        let _previous = extra.insert("cap-2".to_string(), v3_capture_fixture());
        assert!(build(&good_provenance, &extra, 10).is_err());
        // Bad digest.
        let mut bad = v3_capture_fixture();
        bad.receipt_digest = "zz".to_string();
        let bad_captures = BTreeMap::from([("cap-1".to_string(), bad)]);
        assert!(build(&good_provenance, &bad_captures, 10).is_err());
        // Non-frozen strategy.
        let mut bad = v3_capture_fixture();
        bad.chunk_strategy = "syntax".to_string();
        let bad_captures = BTreeMap::from([("cap-1".to_string(), bad)]);
        assert!(build(&good_provenance, &bad_captures, 10).is_err());
        // Unknown chunk_config key.
        let mut bad = v3_capture_fixture();
        bad.chunk_config = serde_json::json!({"nope": 1});
        let bad_captures = BTreeMap::from([("cap-1".to_string(), bad)]);
        assert!(build(&good_provenance, &bad_captures, 10).is_err());
        // top_k drift.
        assert!(build(&good_provenance, &good_captures, 9).is_err());
    }
}
