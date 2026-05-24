//! TOML corpus loader.
//!
//! TOML chosen over YAML for: built-in Cargo ecosystem support
//! (no extra parser surface to audit), strict typing through the
//! `toml::Value` tree, deterministic on-disk layout, and direct
//! line/column reporting for parse errors. Per `usecase.md` § 6 the
//! corpus format is TOML.
//!
//! Parsing is fail-closed: unknown row-level fields are rejected
//! as [`CorpusLoadError::UnknownField`], missing required fields as
//! [`CorpusLoadError::MissingField`], and any gate/`gating_ticket`
//! mismatch as [`CorpusLoadError::MissingGatingTicket`] /
//! [`CorpusLoadError::UnexpectedGatingTicket`]. We never silently
//! drop or default a malformed row.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::corpus::{Corpus, CorpusRow, ExpectedShape, Gate};
use crate::errors::{ConformanceError, CorpusLoadError};

/// Allow-listed row-level keys. Anything else triggers
/// [`CorpusLoadError::UnknownField`].
const ALLOWED_ROW_KEYS: &[&str] = &[
    "id",
    "query",
    "gate",
    "gating_ticket",
    "persona",
    "engines",
    "filters",
    "expected",
];

/// Allow-listed keys under `[expected]`.
const ALLOWED_EXPECTED_KEYS: &[&str] = &["kind", "min", "max", "page_size", "code"];

/// Allow-listed values for `gate`.
const ALLOWED_GATES: &[&str] = &["active", "pending", "blocked"];

/// Allow-listed values for `expected.kind`.
const ALLOWED_EXPECTED_KINDS: &[&str] =
    &["empty", "single", "multi", "paginated", "error"];

/// Load a TOML corpus file from disk.
///
/// Top-level layout:
///
/// ```toml
/// [[row]]
/// id = "SYN-01"
/// query = "fooBar"
/// gate = "active"
/// [row.expected]
/// kind = "multi"
/// min = 1
/// ```
///
/// Errors:
/// - IO failure → [`CorpusLoadError::Io`]
/// - TOML parse error → [`CorpusLoadError::TomlParse`]
/// - Schema violation → typed [`CorpusLoadError`] variant
pub fn load_corpus(path: &Path) -> Result<Corpus, CorpusLoadError> {
    let path_str = path.display().to_string();
    let raw = fs::read_to_string(path).map_err(|e| CorpusLoadError::Io {
        path: path_str,
        message: e.to_string(),
    })?;
    parse_corpus(&raw, path)
}

/// Parse a corpus string. Split out from [`load_corpus`] so tests can
/// drive it without touching the filesystem.
pub fn parse_corpus(raw: &str, path: &Path) -> Result<Corpus, CorpusLoadError> {
    let path_str = path.display().to_string();
    let root: Value = raw.parse::<Value>().map_err(|e| {
        let (line, column) = e
            .span()
            .map_or((0u32, 0u32), |span| offset_to_line_col(raw, span.start));
        CorpusLoadError::TomlParse {
            path: path_str.clone(),
            line,
            column,
            message: e.message().to_owned(),
        }
    })?;

    let Value::Table(table) = root else {
        return Err(CorpusLoadError::TypeMismatch {
            path: path_str,
            field: "<root>",
            expected: "table",
            observed: type_name(&root),
        });
    };

    let rows_value = table
        .get("row")
        .ok_or_else(|| CorpusLoadError::MissingField {
            path: path_str.clone(),
            field: "row",
        })?;

    let Value::Array(row_array) = rows_value else {
        return Err(CorpusLoadError::TypeMismatch {
            path: path_str,
            field: "row",
            expected: "array of tables",
            observed: type_name(rows_value),
        });
    };

    let mut rows: Vec<CorpusRow> = Vec::with_capacity(row_array.len());
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for row_val in row_array {
        let row = parse_row(row_val, &path_str)?;
        if !seen_ids.insert(row.id.clone()) {
            return Err(CorpusLoadError::DuplicateRowId { id: row.id });
        }
        rows.push(row);
    }

    Ok(Corpus {
        rows,
        source_path: PathBuf::from(path),
    })
}

fn parse_row(value: &Value, path: &str) -> Result<CorpusRow, CorpusLoadError> {
    let Value::Table(table) = value else {
        return Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field: "row",
            expected: "table",
            observed: type_name(value),
        });
    };

    for key in table.keys() {
        if !ALLOWED_ROW_KEYS.contains(&key.as_str()) {
            return Err(CorpusLoadError::UnknownField {
                path: path.to_owned(),
                field: key.clone(),
            });
        }
    }

    let id = require_string(table, "id", path)?;
    let query = require_string(table, "query", path)?;
    let gate_str = require_string(table, "gate", path)?;
    let gating_ticket_opt = optional_string(table, "gating_ticket", path)?;

    let gate = parse_gate(&gate_str, gating_ticket_opt.as_deref(), &id, path)?;

    let persona = optional_string(table, "persona", path)?;
    let engines = parse_string_array(table, "engines", path)?;
    let filters = parse_string_array(table, "filters", path)?;

    let expected_val = table
        .get("expected")
        .ok_or_else(|| CorpusLoadError::MissingField {
            path: path.to_owned(),
            field: "expected",
        })?;
    let expected = parse_expected(expected_val, path)?;

    Ok(CorpusRow {
        id,
        query,
        gate,
        expected,
        persona,
        engines,
        filters,
    })
}

fn parse_gate(
    gate_str: &str,
    gating_ticket: Option<&str>,
    row_id: &str,
    path: &str,
) -> Result<Gate, CorpusLoadError> {
    match gate_str {
        "active" => {
            if gating_ticket.is_some() {
                return Err(CorpusLoadError::UnexpectedGatingTicket {
                    row_id: row_id.to_owned(),
                });
            }
            Ok(Gate::Active)
        }
        "pending" => {
            let ticket = gating_ticket.ok_or_else(|| CorpusLoadError::MissingGatingTicket {
                row_id: row_id.to_owned(),
            })?;
            Ok(Gate::Pending {
                gating_ticket: ticket.to_owned(),
            })
        }
        "blocked" => {
            let ticket = gating_ticket.ok_or_else(|| CorpusLoadError::MissingGatingTicket {
                row_id: row_id.to_owned(),
            })?;
            Ok(Gate::Blocked {
                gating_ticket: ticket.to_owned(),
            })
        }
        other => Err(CorpusLoadError::UnknownEnumValue {
            path: path.to_owned(),
            field: "gate",
            value: other.to_owned(),
            allowed: ALLOWED_GATES,
        }),
    }
}

fn parse_expected(value: &Value, path: &str) -> Result<ExpectedShape, CorpusLoadError> {
    let Value::Table(table) = value else {
        return Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field: "expected",
            expected: "table",
            observed: type_name(value),
        });
    };

    for key in table.keys() {
        if !ALLOWED_EXPECTED_KEYS.contains(&key.as_str()) {
            return Err(CorpusLoadError::UnknownField {
                path: path.to_owned(),
                field: format!("expected.{key}"),
            });
        }
    }

    let kind = require_string(table, "kind", path)?;
    match kind.as_str() {
        "empty" => Ok(ExpectedShape::Empty),
        "single" => Ok(ExpectedShape::Single),
        "multi" => {
            let min = require_u32(table, "min", path)?;
            let max = optional_u32(table, "max", path)?;
            Ok(ExpectedShape::Multi { min, max })
        }
        "paginated" => {
            let page_size = require_u32(table, "page_size", path)?;
            Ok(ExpectedShape::Paginated { page_size })
        }
        "error" => {
            let code_str = require_string(table, "code", path)?;
            let code = ConformanceError::from_code_str(&code_str).ok_or_else(|| {
                CorpusLoadError::UnknownErrorCode {
                    path: path.to_owned(),
                    value: code_str,
                }
            })?;
            Ok(ExpectedShape::Error { code })
        }
        other => Err(CorpusLoadError::UnknownEnumValue {
            path: path.to_owned(),
            field: "expected.kind",
            value: other.to_owned(),
            allowed: ALLOWED_EXPECTED_KINDS,
        }),
    }
}

fn require_string(
    table: &toml::map::Map<String, Value>,
    field: &'static str,
    path: &str,
) -> Result<String, CorpusLoadError> {
    match table.get(field) {
        None => Err(CorpusLoadError::MissingField {
            path: path.to_owned(),
            field,
        }),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field,
            expected: "string",
            observed: type_name(other),
        }),
    }
}

fn optional_string(
    table: &toml::map::Map<String, Value>,
    field: &'static str,
    path: &str,
) -> Result<Option<String>, CorpusLoadError> {
    match table.get(field) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field,
            expected: "string",
            observed: type_name(other),
        }),
    }
}

fn require_u32(
    table: &toml::map::Map<String, Value>,
    field: &'static str,
    path: &str,
) -> Result<u32, CorpusLoadError> {
    match table.get(field) {
        None => Err(CorpusLoadError::MissingField {
            path: path.to_owned(),
            field,
        }),
        Some(Value::Integer(i)) => to_u32(*i, field, path),
        Some(other) => Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field,
            expected: "integer",
            observed: type_name(other),
        }),
    }
}

fn optional_u32(
    table: &toml::map::Map<String, Value>,
    field: &'static str,
    path: &str,
) -> Result<Option<u32>, CorpusLoadError> {
    match table.get(field) {
        None => Ok(None),
        Some(Value::Integer(i)) => to_u32(*i, field, path).map(Some),
        Some(other) => Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field,
            expected: "integer",
            observed: type_name(other),
        }),
    }
}

fn to_u32(value: i64, field: &'static str, path: &str) -> Result<u32, CorpusLoadError> {
    u32::try_from(value).map_err(|_err| CorpusLoadError::TypeMismatch {
        path: path.to_owned(),
        field,
        expected: "u32",
        observed: "out-of-range integer",
    })
}

fn parse_string_array(
    table: &toml::map::Map<String, Value>,
    field: &'static str,
    path: &str,
) -> Result<Vec<String>, CorpusLoadError> {
    match table.get(field) {
        None => Ok(Vec::new()),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for entry in arr {
                match entry {
                    Value::String(s) => out.push(s.clone()),
                    Value::Integer(_)
                    | Value::Float(_)
                    | Value::Boolean(_)
                    | Value::Datetime(_)
                    | Value::Array(_)
                    | Value::Table(_) => {
                        return Err(CorpusLoadError::TypeMismatch {
                            path: path.to_owned(),
                            field,
                            expected: "array of strings",
                            observed: type_name(entry),
                        });
                    }
                }
            }
            Ok(out)
        }
        Some(other) => Err(CorpusLoadError::TypeMismatch {
            path: path.to_owned(),
            field,
            expected: "array of strings",
            observed: type_name(other),
        }),
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::Boolean(_) => "boolean",
        Value::Datetime(_) => "datetime",
        Value::Array(_) => "array",
        Value::Table(_) => "table",
    }
}

fn offset_to_line_col(raw: &str, byte_offset: usize) -> (u32, u32) {
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    for (i, ch) in raw.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line = line.saturating_add(1);
            col = 1;
        } else {
            col = col.saturating_add(1);
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> &'static Path {
        Path::new("test.toml")
    }

    /// Get the first row or fail the (`()`-returning) test loudly.
    /// The fallback branch never returns — `assert!(false, ..)`
    /// panics the test — but Rust requires a typed value.
    fn first_row(corpus: &Corpus) -> CorpusRow {
        corpus.rows.first().map_or_else(
            || {
                assert!(false, "expected at least one row");
                CorpusRow {
                    id: String::new(),
                    query: String::new(),
                    gate: Gate::Active,
                    expected: ExpectedShape::Empty,
                    persona: None,
                    engines: Vec::new(),
                    filters: Vec::new(),
                }
            },
            Clone::clone,
        )
    }

    fn expect_err(result: Result<Corpus, CorpusLoadError>) -> CorpusLoadError {
        result.map_or_else(
            |e| e,
            |_corpus| {
                assert!(false, "expected Err, got Ok");
                CorpusLoadError::Io {
                    path: String::new(),
                    message: String::new(),
                }
            },
        )
    }

    fn expect_ok(result: Result<Corpus, CorpusLoadError>) -> Corpus {
        result.unwrap_or_else(|e| {
            assert!(false, "expected Ok, got Err: {e}");
            Corpus {
                rows: Vec::new(),
                source_path: PathBuf::new(),
            }
        })
    }

    #[test]
    fn valid_corpus_parses() {
        let raw = r#"
            [[row]]
            id = "SYN-01"
            query = "fooBar"
            gate = "active"
            engines = ["lexical_content"]

            [row.expected]
            kind = "multi"
            min = 1
        "#;
        let corpus = expect_ok(parse_corpus(raw, p()));
        assert_eq!(corpus.rows.len(), 1);
        let row = first_row(&corpus);
        assert_eq!(row.id, "SYN-01");
        assert_eq!(row.gate, Gate::Active);
        assert_eq!(row.expected, ExpectedShape::Multi { min: 1, max: None });
    }

    #[test]
    fn duplicate_row_id_rejected() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "active"
            [row.expected]
            kind = "single"

            [[row]]
            id = "X"
            query = "b"
            gate = "active"
            [row.expected]
            kind = "single"
        "#;
        let err = expect_err(parse_corpus(raw, p()));
        assert!(matches!(err, CorpusLoadError::DuplicateRowId { id } if id == "X"));
    }

    #[test]
    fn unknown_field_rejected() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "active"
            mystery = "value"
            [row.expected]
            kind = "single"
        "#;
        let err = expect_err(parse_corpus(raw, p()));
        assert!(matches!(err, CorpusLoadError::UnknownField { field, .. } if field == "mystery"));
    }

    #[test]
    fn pending_without_gating_ticket_rejected() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "pending"
            [row.expected]
            kind = "single"
        "#;
        let err = expect_err(parse_corpus(raw, p()));
        assert!(matches!(err, CorpusLoadError::MissingGatingTicket { row_id } if row_id == "X"));
    }

    #[test]
    fn active_with_gating_ticket_rejected() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "active"
            gating_ticket = "T"
            [row.expected]
            kind = "single"
        "#;
        let err = expect_err(parse_corpus(raw, p()));
        assert!(matches!(err, CorpusLoadError::UnexpectedGatingTicket { row_id } if row_id == "X"));
    }

    #[test]
    fn unknown_error_code_rejected() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "active"
            [row.expected]
            kind = "error"
            code = "NOT_A_REAL_CODE"
        "#;
        let err = expect_err(parse_corpus(raw, p()));
        assert!(
            matches!(err, CorpusLoadError::UnknownErrorCode { value, .. } if value == "NOT_A_REAL_CODE")
        );
    }

    #[test]
    fn known_error_code_accepted() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "active"
            [row.expected]
            kind = "error"
            code = "OVERSIZED_REQUEST"
        "#;
        let corpus = expect_ok(parse_corpus(raw, p()));
        let row = first_row(&corpus);
        assert_eq!(
            row.expected,
            ExpectedShape::Error {
                code: ConformanceError::OversizedRequest,
            }
        );
    }

    #[test]
    fn malformed_toml_reports_line_col() {
        let raw = "this is = not = valid toml";
        let err = expect_err(parse_corpus(raw, p()));
        assert!(matches!(err, CorpusLoadError::TomlParse { .. }));
    }

    #[test]
    fn missing_required_field_rejected() {
        let raw = r#"
            [[row]]
            id = "X"
            gate = "active"
            [row.expected]
            kind = "single"
        "#;
        let err = expect_err(parse_corpus(raw, p()));
        assert!(matches!(
            err,
            CorpusLoadError::MissingField {
                field: "query",
                ..
            }
        ));
    }

    #[test]
    fn blocked_gate_with_ticket_accepted() {
        let raw = r#"
            [[row]]
            id = "X"
            query = "a"
            gate = "blocked"
            gating_ticket = "RT-01"
            [row.expected]
            kind = "single"
        "#;
        let corpus = expect_ok(parse_corpus(raw, p()));
        let row = first_row(&corpus);
        assert_eq!(
            row.gate,
            Gate::Blocked {
                gating_ticket: "RT-01".to_owned(),
            }
        );
    }
}
