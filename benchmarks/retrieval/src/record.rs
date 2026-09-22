//! Runner record v2 emission (RB-02 single owner, RB-05 read-only).
//!
//! Maps SDK query outcomes to the exact `runner.schema.json` v2 wire form:
//! ordered results per `(task, route)`, recomputed block hashes and
//! `qi-regex-v1` token counts from pinned source bytes, finite timings,
//! and typed non-success statuses. RB-05 consumes these records without
//! editing this module.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::chunking::count_tokens;
use crate::corpus::SourceFile;
use crate::sdk::{QueryOutcome, RankedHit};
use crate::{BenchError, BenchResult, sha256_hex};

pub const RUNNER_SCHEMA_VERSION: u64 = 2;
pub const TOKENIZER: &str = "qi-regex-v1";
pub const TOKENIZER_BUDGET_VERSION: &str = "qb-v1";

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
    pub tasks: Vec<PackTask>,
    pub pack_sha256: String,
}

/// Canonical JSON: sorted keys, no whitespace, raw UTF-8, no floats.
/// Matches `evaluator.canonical` for the pack's string/int-only domain;
/// floats are refused rather than format-guessed.
pub fn canonical_json(value: &Value) -> BenchResult<String> {
    fn render(value: &Value, out: &mut String) -> BenchResult<()> {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(number) => {
                if number.is_f64() {
                    return Err(BenchError::Protocol(
                        "canonical JSON refuses floats (Python float formatting would diverge)"
                            .to_string(),
                    ));
                }
                out.push_str(&number.to_string());
            }
            Value::String(text) => {
                let rendered = serde_json::to_string(text).map_err(|err| BenchError::Json {
                    path: "<pack>".to_string(),
                    message: err.to_string(),
                })?;
                out.push_str(&rendered);
            }
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    render(item, out)?;
                }
                out.push(']');
            }
            Value::Object(object) => {
                out.push('{');
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort();
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let rendered_key =
                        serde_json::to_string(key).map_err(|err| BenchError::Json {
                            path: "<pack>".to_string(),
                            message: err.to_string(),
                        })?;
                    out.push_str(&rendered_key);
                    out.push(':');
                    if let Some(child) = object.get(key.as_str()) {
                        render(child, out)?;
                    }
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut out = String::new();
    render(value, &mut out)?;
    Ok(out)
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
        _ => None,
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

/// Load a `freeze`-produced query pack, verifying blindness (no gold-bearing
/// keys anywhere), query hashes and tokenizer binding.
pub fn load_query_pack(path: &Path) -> BenchResult<QueryPack> {
    let raw = std::fs::read_to_string(path).map_err(|err| BenchError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    let value: Value = serde_json::from_str(&raw).map_err(|err| BenchError::Json {
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
    if version != Some(2) {
        return Err(BenchError::Protocol(
            "query pack schema_version must be 2".to_string(),
        ));
    }
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
    let mut universe = Vec::new();
    if let Some(entries) = object.get("file_universe") {
        let list = entries
            .as_array()
            .ok_or_else(|| BenchError::Protocol("file_universe must be a list".to_string()))?;
        for entry in list {
            let item = entry.as_object().ok_or_else(|| {
                BenchError::Protocol("file_universe entry must be an object".to_string())
            })?;
            exact_keys(item, &["path", "file_sha256"], "file_universe entry")?;
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| {
                    BenchError::Protocol("file_universe entry lacks path".to_string())
                })?;
            let digest = item
                .get("file_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BenchError::Protocol("file_universe entry lacks file_sha256".to_string())
                })?;
            universe.push((path.to_string(), digest.to_string()));
        }
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
        tasks: parsed,
        pack_sha256,
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

/// Per-route provenance: what system/model actually served the route.
#[derive(Debug, Clone)]
pub struct RouteProvenance {
    pub system: String,
    pub model: String,
    pub model_revision: String,
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

/// Prove one SDK hit against pinned source bytes and emit the evaluator's
/// candidate object. Returned with its 1-based rank.
fn prove_hit(
    hit: &RankedHit,
    rank: usize,
    files: &BTreeMap<String, SourceFile>,
) -> BenchResult<Value> {
    let file = files.get(&hit.path).ok_or_else(|| {
        BenchError::Protocol(format!("SDK hit outside admitted universe: {}", hit.path))
    })?;
    if hit.start_line == 0 || hit.end_line == 0 || hit.start_line > hit.end_line {
        return Err(BenchError::Protocol(format!(
            "SDK hit has an inverted line span: {}:{}-{}",
            hit.path, hit.start_line, hit.end_line
        )));
    }
    let (start, end) = file.line_span_bytes(
        usize::try_from(hit.start_line).unwrap_or(usize::MAX),
        usize::try_from(hit.end_line).unwrap_or(0),
    )?;
    let block = &file.bytes[start..end];
    let text = std::str::from_utf8(block)
        .map_err(|_| BenchError::Protocol(format!("SDK hit block is not UTF-8: {}", hit.path)))?;
    let tokens = count_tokens(text);
    if tokens == 0 {
        return Err(BenchError::Protocol(format!(
            "SDK hit block holds no retrievable tokens: {}",
            hit.path
        )));
    }
    let mut candidate = Map::new();
    assert!(
        candidate
            .insert("path".to_string(), Value::String(hit.path.clone()))
            .is_none()
    );
    assert!(
        candidate
            .insert(
                "start_line".to_string(),
                Value::Number(hit.start_line.into()),
            )
            .is_none()
    );
    assert!(
        candidate
            .insert("end_line".to_string(), Value::Number(hit.end_line.into()))
            .is_none()
    );
    assert!(
        candidate
            .insert(
                "file_sha256".to_string(),
                Value::String(file.sha256.clone()),
            )
            .is_none()
    );
    assert!(
        candidate
            .insert("block_sha256".to_string(), Value::String(sha256_hex(block)),)
            .is_none()
    );
    assert!(
        candidate
            .insert("tokens".to_string(), Value::Number((tokens as u64).into()),)
            .is_none()
    );
    assert!(
        candidate
            .insert("rank".to_string(), Value::Number((rank as u64).into()),)
            .is_none()
    );
    Ok(Value::Object(candidate))
}

fn error_value(code: &str, message: &str) -> Value {
    let mut error = Map::new();
    assert!(
        error
            .insert("code".to_string(), Value::String(code.to_string()))
            .is_none()
    );
    assert!(
        error
            .insert("message".to_string(), Value::String(message.to_string()))
            .is_none()
    );
    Value::Object(error)
}

fn timings_value(latency: Duration) -> BenchResult<Value> {
    let mut timings = Map::new();
    let ms = duration_ms(latency)?;
    let number = serde_json::Number::from_f64(ms).ok_or_else(|| {
        BenchError::Protocol("non-finite query latency cannot be recorded".to_string())
    })?;
    assert!(
        timings
            .insert("query_latency_ms".to_string(), Value::Number(number))
            .is_none()
    );
    Ok(Value::Object(timings))
}

/// Map one query outcome to a v2 result object. `top_k` is the declared cap;
/// more hits than the cap is a protocol violation, never a truncation.
pub fn result_value(
    task_id: &str,
    route: &str,
    outcome: &QueryOutcome,
    top_k: u32,
    files: &BTreeMap<String, SourceFile>,
) -> BenchResult<Value> {
    let mut result = Map::new();
    assert!(
        result
            .insert("task_id".to_string(), Value::String(task_id.to_string()))
            .is_none()
    );
    assert!(
        result
            .insert("route".to_string(), Value::String(route.to_string()))
            .is_none()
    );
    match outcome {
        QueryOutcome::Hits {
            hits,
            outcome,
            latency,
        } => {
            if hits.len() as u64 > u64::from(top_k) {
                return Err(BenchError::Protocol(format!(
                    "{route} returned {} hits above top_k={top_k} for {task_id}",
                    hits.len()
                )));
            }
            if hits.is_empty() {
                if outcome.is_exhausted() {
                    assert!(
                        result
                            .insert("status".to_string(), Value::String("abstained".to_string()))
                            .is_none()
                    );
                    assert!(
                        result
                            .insert("candidates".to_string(), Value::Array(Vec::new()))
                            .is_none()
                    );
                    assert!(
                        result
                            .insert("timings".to_string(), timings_value(*latency)?)
                            .is_none()
                    );
                    assert!(result.insert("error".to_string(), Value::Null).is_none());
                } else {
                    assert!(
                        result
                            .insert("status".to_string(), Value::String("error".to_string()))
                            .is_none()
                    );
                    assert!(
                        result
                            .insert("candidates".to_string(), Value::Array(Vec::new()))
                            .is_none()
                    );
                    assert!(
                        result
                            .insert("timings".to_string(), timings_value(*latency)?)
                            .is_none()
                    );
                    assert!(
                        result
                            .insert(
                                "error".to_string(),
                                error_value(
                                    "empty_non_exhausted_window",
                                    "zero hits under a non-exhausted window cannot score",
                                ),
                            )
                            .is_none()
                    );
                }
                return Ok(Value::Object(result));
            }
            let status = if outcome.is_exhausted() {
                "success"
            } else {
                "capped"
            };
            let mut candidates = Vec::with_capacity(hits.len());
            for (index, hit) in hits.iter().enumerate() {
                candidates.push(prove_hit(hit, index + 1, files)?);
            }
            assert!(
                result
                    .insert("status".to_string(), Value::String(status.to_string()))
                    .is_none()
            );
            assert!(
                result
                    .insert("candidates".to_string(), Value::Array(candidates))
                    .is_none()
            );
            assert!(
                result
                    .insert("timings".to_string(), timings_value(*latency)?)
                    .is_none()
            );
            assert!(result.insert("error".to_string(), Value::Null).is_none());
        }
        QueryOutcome::Failed {
            status,
            code,
            message,
            latency,
        } => {
            assert!(
                result
                    .insert("status".to_string(), Value::String(status.to_string()),)
                    .is_none()
            );
            assert!(
                result
                    .insert("candidates".to_string(), Value::Array(Vec::new()))
                    .is_none()
            );
            assert!(
                result
                    .insert("timings".to_string(), timings_value(*latency)?)
                    .is_none()
            );
            assert!(
                result
                    .insert("error".to_string(), error_value(code, message))
                    .is_none()
            );
        }
    }
    Ok(Value::Object(result))
}

/// Assemble the complete v2 runner record. Results emit in deterministic
/// `(task_id, route)` order from the pack's task order and sorted routes.
#[allow(clippy::too_many_arguments)]
pub fn runner_record(
    pack: &QueryPack,
    identity: &RunnerIdentity,
    provenance: &BTreeMap<String, RouteProvenance>,
    outcomes: &BTreeMap<(String, String), QueryOutcome>,
    top_k: u32,
    files: &BTreeMap<String, SourceFile>,
) -> BenchResult<Value> {
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
    let mut record = Map::new();
    assert!(
        record
            .insert(
                "schema_version".to_string(),
                Value::Number(RUNNER_SCHEMA_VERSION.into()),
            )
            .is_none()
    );
    assert!(
        record
            .insert(
                "query_pack_sha256".to_string(),
                Value::String(pack.pack_sha256.clone()),
            )
            .is_none()
    );
    let mut runner = Map::new();
    assert!(
        runner
            .insert("name".to_string(), Value::String(identity.name.clone()))
            .is_none()
    );
    assert!(
        runner
            .insert(
                "revision".to_string(),
                Value::String(identity.revision.clone()),
            )
            .is_none()
    );
    assert!(
        runner
            .insert("run_id".to_string(), Value::String(identity.run_id.clone()),)
            .is_none()
    );
    assert!(
        runner
            .insert(
                "tokenizer".to_string(),
                Value::String(TOKENIZER.to_string()),
            )
            .is_none()
    );
    assert!(
        runner
            .insert(
                "tokenizer_budget_version".to_string(),
                Value::String(TOKENIZER_BUDGET_VERSION.to_string()),
            )
            .is_none()
    );
    assert!(
        runner
            .insert("gold_access".to_string(), Value::Bool(false))
            .is_none()
    );
    assert!(
        runner
            .insert(
                "blinding".to_string(),
                Value::String(identity.blinding.clone()),
            )
            .is_none()
    );
    assert!(
        runner
            .insert(
                "isolation_method".to_string(),
                Value::String(identity.isolation_method.clone()),
            )
            .is_none()
    );
    assert!(
        runner
            .insert(
                "access_block_log".to_string(),
                Value::String(identity.access_block_log.clone()),
            )
            .is_none()
    );
    assert!(
        record
            .insert("runner".to_string(), Value::Object(runner))
            .is_none()
    );
    let mut provenance_value = Map::new();
    for route in &routes {
        let entry = provenance
            .get(*route)
            .ok_or_else(|| BenchError::Protocol(format!("missing provenance for route {route}")))?;
        let mut item = Map::new();
        assert!(
            item.insert("system".to_string(), Value::String(entry.system.clone()))
                .is_none()
        );
        assert!(
            item.insert("model".to_string(), Value::String(entry.model.clone()))
                .is_none()
        );
        assert!(
            item.insert(
                "model_revision".to_string(),
                Value::String(entry.model_revision.clone()),
            )
            .is_none()
        );
        assert!(
            provenance_value
                .insert((*route).clone(), Value::Object(item))
                .is_none()
        );
    }
    assert!(
        record
            .insert(
                "route_provenance".to_string(),
                Value::Object(provenance_value),
            )
            .is_none()
    );
    let mut results = Vec::new();
    for task in &pack.tasks {
        for route in &routes {
            let outcome = outcomes
                .get(&(task.task_id.clone(), (*route).clone()))
                .ok_or_else(|| {
                    BenchError::Protocol(format!("missing outcome for ({}, {route})", task.task_id))
                })?;
            results.push(result_value(&task.task_id, route, outcome, top_k, files)?);
        }
    }
    assert!(
        record
            .insert("results".to_string(), Value::Array(results))
            .is_none()
    );
    Ok(Value::Object(record))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_sorts_keys_and_holds_utf8() {
        let value: Value = serde_json::from_str(r#"{"b":1,"a":[2,{"z":null,"m":"héllo"}]}"#)
            .expect("fixture parses");
        let rendered = canonical_json(&value).expect("canonical renders");
        assert_eq!(rendered, r#"{"a":[2,{"m":"héllo","z":null}],"b":1}"#);
    }

    #[test]
    fn canonical_form_refuses_floats() {
        let value: Value = serde_json::from_str(r#"{"a":1.5}"#).expect("fixture parses");
        assert!(canonical_json(&value).is_err());
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
}
