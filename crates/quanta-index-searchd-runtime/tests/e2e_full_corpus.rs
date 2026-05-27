//! E2E-06 — machine-readable real-engine corpus rail.
//!
//! Runtime rows execute against the live daemon harness. Parser-only and
//! external-producer rows stay in the same corpus file but are counted
//! separately so parser conformance never masquerades as runtime coverage.

#![forbid(unsafe_code)]

#[path = "common/e2e_harness.rs"]
mod e2e_harness;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;
use quanta_index_corpus_smoke::{
    CorpusRow, ExpectedShape, Gate, RowClassification, RuntimeSyntax, load_corpus,
};
use toml::Value;

use crate::e2e_harness::{E2eQueryResult, E2eRuntime};

struct FixtureDoc {
    id: String,
    path: String,
    content: String,
    symbol_name: Option<String>,
}

struct LoadedFixture {
    docs: Vec<FixtureDoc>,
    path_to_id: BTreeMap<String, String>,
}

struct RunSummary {
    runtime_passed: usize,
    typed_unavailable_passed: usize,
    parser_only_rows: usize,
    deferred_rows: usize,
}

struct RowReport {
    id: String,
    failure: Option<String>,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lexical_corpus")
}

fn runtime_rows_path() -> PathBuf {
    fixtures_root().join("runtime_rows.toml")
}

fn load_fixture(name: &str) -> AnyResult<LoadedFixture> {
    let path = fixtures_root().join(name);
    let raw = std::fs::read_to_string(&path)?;
    let root: Value = raw.parse::<Value>()?;
    let table = root
        .as_table()
        .ok_or_else(|| anyhow::anyhow!("fixture {} root must be a table", path.display()))?;
    let docs = table
        .get("doc")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("fixture {} missing [[doc]] array", path.display()))?;

    let mut out = Vec::with_capacity(docs.len());
    let mut path_to_id = BTreeMap::new();
    for doc in docs {
        let table = doc
            .as_table()
            .ok_or_else(|| anyhow::anyhow!("fixture {} doc row must be a table", path.display()))?;
        let id = require_string(table, "id", &path)?;
        let doc_path = require_string(table, "path", &path)?;
        let content = require_string(table, "content", &path)?;
        let symbol_name = optional_string(table, "symbol_name", &path)?;
        if let Some(previous_id) = path_to_id.insert(doc_path.clone(), id.clone()) {
            return Err(anyhow::anyhow!(
                "fixture {} reuses doc path {} for ids {} and {}",
                path.display(),
                doc_path,
                previous_id,
                id
            ));
        }
        out.push(FixtureDoc {
            id,
            path: doc_path,
            content,
            symbol_name,
        });
    }

    Ok(LoadedFixture {
        docs: out,
        path_to_id,
    })
}

fn require_string(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<String> {
    match table.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be string, got {other:?}",
            path.display()
        )),
        None => Err(anyhow::anyhow!(
            "fixture {} missing `{field}`",
            path.display()
        )),
    }
}

fn optional_string(
    table: &toml::map::Map<String, Value>,
    field: &str,
    path: &Path,
) -> AnyResult<Option<String>> {
    match table.get(field) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(anyhow::anyhow!(
            "fixture {} field `{field}` must be string, got {other:?}",
            path.display()
        )),
        None => Ok(None),
    }
}

fn ingest_fixture(rt: &mut E2eRuntime, fixture: &LoadedFixture) -> AnyResult<()> {
    for doc in &fixture.docs {
        rt.ingest_text("repo-e2e", &doc.path, &doc.content)?;
        if let Some(symbol_name) = doc.symbol_name.as_deref() {
            rt.ingest_symbol("repo-e2e", &doc.path, &doc.id, symbol_name)?;
        }
    }
    Ok(())
}

fn runtime_syntax(row: &CorpusRow) -> Result<TextQuerySyntax, String> {
    match row.syntax {
        Some(RuntimeSyntax::Native) => Ok(TextQuerySyntax::Native),
        Some(RuntimeSyntax::Sourcegraph) => Ok(TextQuerySyntax::Sourcegraph),
        None => Err(format!("row {} missing runtime syntax", row.id)),
    }
}

fn ensure_runtime_row_config(row: &CorpusRow) -> Result<(), String> {
    match row.classification {
        Some(RowClassification::Runtime) => {
            if !matches!(&row.gate, Gate::Active) {
                return Err(format!(
                    "row {} runtime classification must use gate=active",
                    row.id
                ));
            }
            if row.fixture.is_none() || row.top_k.is_none() || row.syntax.is_none() {
                return Err(format!(
                    "row {} runtime classification requires fixture, top_k, and syntax",
                    row.id
                ));
            }
            if row.runtime_error_code.is_some() {
                return Err(format!(
                    "row {} runtime classification must not carry runtime_error_code",
                    row.id
                ));
            }
            Ok(())
        }
        Some(RowClassification::TypedUnavailable) => {
            if !matches!(&row.gate, Gate::Active) {
                return Err(format!(
                    "row {} typed_unavailable classification must use gate=active",
                    row.id
                ));
            }
            if row.runtime_error_code.is_none()
                || row.fixture.is_none()
                || row.top_k.is_none()
                || row.syntax.is_none()
            {
                return Err(format!(
                    "row {} typed_unavailable classification requires runtime_error_code, fixture, top_k, and syntax",
                    row.id
                ));
            }
            Ok(())
        }
        Some(RowClassification::ParserOnly) => {
            if !matches!(&row.gate, Gate::Pending { .. }) {
                return Err(format!(
                    "row {} parser_only classification must use gate=pending",
                    row.id
                ));
            }
            Ok(())
        }
        Some(RowClassification::DeferredExternalProducer) => {
            if !matches!(&row.gate, Gate::Blocked { .. }) {
                return Err(format!(
                    "row {} deferred_external_producer classification must use gate=blocked",
                    row.id
                ));
            }
            Ok(())
        }
        None => Err(format!("row {} missing classification", row.id)),
    }
}

fn observed_fixture_ids(
    result: &E2eQueryResult,
    path_to_id: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    result
        .candidates
        .iter()
        .map(|candidate| {
            path_to_id
                .get(candidate.repo_relative_path.as_str())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "unmapped candidate path `{}` in fixture",
                        candidate.repo_relative_path.as_str()
                    )
                })
        })
        .collect()
}

fn expected_shape_matches_ids(row: &CorpusRow, observed: &[String]) -> Result<(), String> {
    match &row.expected {
        ExpectedShape::Empty => {
            if observed.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "row {} expected empty result set, got ids={observed:?}",
                    row.id
                ))
            }
        }
        ExpectedShape::Single => {
            if observed.len() == 1 {
                Ok(())
            } else {
                Err(format!(
                    "row {} expected single result, got ids={observed:?}",
                    row.id
                ))
            }
        }
        ExpectedShape::Multi { min, max } => {
            let len = u32::try_from(observed.len())
                .map_err(|err| format!("row {} observed ids length overflow: {err}", row.id))?;
            let upper_ok = max.is_none_or(|upper| len <= upper);
            if len >= *min && upper_ok {
                Ok(())
            } else {
                Err(format!(
                    "row {} expected multi bounds min={} max={:?}, got ids={observed:?}",
                    row.id, min, max
                ))
            }
        }
        ExpectedShape::Paginated { page_size } => Err(format!(
            "row {} uses paginated expected shape unsupported by runtime rail (page_size={page_size})",
            row.id
        )),
        ExpectedShape::Error { code } => Err(format!(
            "row {} uses parser/conformance error {:?}; runtime rail expects runtime_error_code instead",
            row.id, code
        )),
    }
}

fn explanation_artifact(rt: &mut E2eRuntime, result: &E2eQueryResult) -> String {
    let Some(candidate) = result.candidates.first().cloned() else {
        return "none".to_string();
    };
    let explain = rt.explain_candidate(candidate);
    if let Some(err) = explain.typed_error {
        format!("typed_error(code={}, message={})", err.code, err.message)
    } else if let Some(explanation) = explain.explanation {
        format!("{explanation:?}")
    } else {
        "none".to_string()
    }
}

fn assess_runtime_row(
    rt: &mut E2eRuntime,
    row: &CorpusRow,
    path_to_id: &BTreeMap<String, String>,
) -> RowReport {
    let syntax = match runtime_syntax(row) {
        Ok(syntax) => syntax,
        Err(err) => {
            return RowReport {
                id: row.id.clone(),
                failure: Some(err),
            };
        }
    };
    let Some(top_k) = row.top_k else {
        return RowReport {
            id: row.id.clone(),
            failure: Some(format!("row {} missing top_k", row.id)),
        };
    };
    let result = rt.query_text(syntax, &row.query, top_k);
    if let Some(err) = result.typed_error.as_ref() {
        return RowReport {
            id: row.id.clone(),
            failure: Some(format!(
                "row {} expected ids={:?} but got typed_error(code={}, message={}); query=`{}` syntax={:?} fixture={} explanation={}",
                row.id,
                row.expected_ids,
                err.code,
                err.message,
                row.query,
                row.syntax,
                row.fixture.as_deref().unwrap_or("<missing>"),
                explanation_artifact(rt, &result)
            )),
        };
    }
    let observed = match observed_fixture_ids(&result, path_to_id) {
        Ok(observed) => observed,
        Err(err) => {
            return RowReport {
                id: row.id.clone(),
                failure: Some(err),
            };
        }
    };
    if let Err(err) = expected_shape_matches_ids(row, &observed) {
        return RowReport {
            id: row.id.clone(),
            failure: Some(err),
        };
    }
    if observed != row.expected_ids {
        return RowReport {
            id: row.id.clone(),
            failure: Some(format!(
                "row {} expected ids={:?} observed ids={observed:?}; query=`{}` syntax={:?} fixture={} explanation={}",
                row.id,
                row.expected_ids,
                row.query,
                row.syntax,
                row.fixture.as_deref().unwrap_or("<missing>"),
                explanation_artifact(rt, &result)
            )),
        };
    }
    RowReport {
        id: row.id.clone(),
        failure: None,
    }
}

fn assess_typed_unavailable_row(rt: &mut E2eRuntime, row: &CorpusRow) -> RowReport {
    let syntax = match runtime_syntax(row) {
        Ok(syntax) => syntax,
        Err(err) => {
            return RowReport {
                id: row.id.clone(),
                failure: Some(err),
            };
        }
    };
    let Some(top_k) = row.top_k else {
        return RowReport {
            id: row.id.clone(),
            failure: Some(format!("row {} missing top_k", row.id)),
        };
    };
    let Some(expected_code) = row.runtime_error_code.as_deref() else {
        return RowReport {
            id: row.id.clone(),
            failure: Some(format!("row {} missing runtime_error_code", row.id)),
        };
    };
    let result = rt.query_text(syntax, &row.query, top_k);
    match result.typed_error {
        Some(err) if err.code == expected_code => RowReport {
            id: row.id.clone(),
            failure: None,
        },
        Some(err) => RowReport {
            id: row.id.clone(),
            failure: Some(format!(
                "row {} expected typed_error code={} but got code={} message={}; query=`{}` syntax={:?}",
                row.id, expected_code, err.code, err.message, row.query, row.syntax
            )),
        },
        None => RowReport {
            id: row.id.clone(),
            failure: Some(format!(
                "row {} expected typed_error code={} but got candidates={:?}; query=`{}` syntax={:?} explanation={}",
                row.id,
                expected_code,
                result
                    .candidates
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str())
                    .collect::<Vec<_>>(),
                row.query,
                row.syntax,
                explanation_artifact(rt, &result)
            )),
        },
    }
}

#[test]
fn full_corpus_runtime_fixture_executes_real_rows_only() -> AnyResult<()> {
    let corpus = load_corpus(&runtime_rows_path())?;
    let mut fixtures = BTreeMap::new();
    let mut current_fixture_name: Option<String> = None;
    let mut current_fixture_ids = BTreeMap::new();
    let mut runtime: Option<E2eRuntime> = None;

    let mut summary = RunSummary {
        runtime_passed: 0,
        typed_unavailable_passed: 0,
        parser_only_rows: 0,
        deferred_rows: 0,
    };
    let mut failures = Vec::new();

    for row in &corpus.rows {
        if let Err(err) = ensure_runtime_row_config(row) {
            failures.push(RowReport {
                id: row.id.clone(),
                failure: Some(err),
            });
            continue;
        }
        match row.classification {
            Some(RowClassification::Runtime | RowClassification::TypedUnavailable) => {
                let fixture_name = row.fixture.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("row {} missing fixture after validation", row.id)
                })?;
                if current_fixture_name.as_deref() != Some(fixture_name.as_str()) {
                    let fixture = if let Some(existing) = fixtures.get(fixture_name) {
                        existing
                    } else {
                        let loaded = load_fixture(fixture_name)?;
                        let _old = fixtures.insert(fixture_name.clone(), loaded);
                        fixtures
                            .get(fixture_name)
                            .ok_or_else(|| anyhow::anyhow!("fixture cache insert lost row"))?
                    };
                    let mut rt = E2eRuntime::boot()?;
                    ingest_fixture(&mut rt, fixture)?;
                    _ = rt.seal()?;
                    runtime = Some(rt.reopen());
                    current_fixture_name = Some(fixture_name.clone());
                    current_fixture_ids = fixture.path_to_id.clone();
                }
                let rt = runtime
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("runtime missing after fixture boot"))?;
                let report = match row.classification {
                    Some(RowClassification::Runtime) => {
                        assess_runtime_row(rt, row, &current_fixture_ids)
                    }
                    Some(RowClassification::TypedUnavailable) => {
                        assess_typed_unavailable_row(rt, row)
                    }
                    Some(other) => RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} reached runtime execution with non-runtime classification {:?}",
                            row.id, other
                        )),
                    },
                    None => RowReport {
                        id: row.id.clone(),
                        failure: Some(format!(
                            "row {} reached runtime execution without classification",
                            row.id
                        )),
                    },
                };
                if report.failure.is_some() {
                    failures.push(report);
                } else if matches!(row.classification, Some(RowClassification::Runtime)) {
                    summary.runtime_passed = summary.runtime_passed.saturating_add(1);
                } else {
                    summary.typed_unavailable_passed =
                        summary.typed_unavailable_passed.saturating_add(1);
                }
            }
            Some(RowClassification::ParserOnly) => {
                summary.parser_only_rows = summary.parser_only_rows.saturating_add(1);
            }
            Some(RowClassification::DeferredExternalProducer) => {
                summary.deferred_rows = summary.deferred_rows.saturating_add(1);
            }
            None => {
                failures.push(RowReport {
                    id: row.id.clone(),
                    failure: Some("row missing classification".to_string()),
                });
            }
        }
    }

    if failures.is_empty() {
        return Ok(());
    }

    let mut buf = String::new();
    writeln!(
        buf,
        "E2E-06 full corpus runtime rail: {} failures; runtime_passed={} typed_unavailable_passed={} parser_only_rows={} deferred_external_producer_rows={}",
        failures.len(),
        summary.runtime_passed,
        summary.typed_unavailable_passed,
        summary.parser_only_rows,
        summary.deferred_rows
    )?;
    for failure in &failures {
        if let Some(message) = &failure.failure {
            writeln!(buf, "  - [{}] {}", failure.id, message)?;
        }
    }
    Err(anyhow::anyhow!("{buf}"))
}
