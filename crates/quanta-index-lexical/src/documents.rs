//! Building the documents the index stores: fields, snippets, languages.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::analyzer::tokenizer_name;
use crate::metadata_normalize::normalize_language;
use crate::normalize::CaseMode;
use crate::{SchemaFields, normalize};
use quanta_index_contract::LqExpr;
use quanta_index_contract::lex::SymbolRecord;
use quanta_index_core::CoreError;
use std::path::Path;
use tantivy::schema::{
    Field, IndexRecordOption, OwnedValue, TantivyDocument, TextFieldIndexing, TextOptions, Value,
};

pub(crate) fn file_name_for_path(path: &str) -> Option<&str> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

pub(crate) fn language_from_path_hint(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "md" => Some("markdown"),
        "java" => Some("java"),
        "js" => Some("javascript"),
        "ts" => Some("typescript"),
        "jsx" => Some("javascriptreact"),
        "tsx" => Some("typescriptreact"),
        "rb" => Some("ruby"),
        "go" => Some("go"),
        "c" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some("cpp"),
        "cs" => Some("csharp"),
        "kt" | "kts" => Some("kotlin"),
        "swift" => Some("swift"),
        "scala" => Some("scala"),
        "php" => Some("php"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "sh" | "bash" => Some("shell"),
        "txt" => Some("text"),
        _ => None,
    }
}

/// Indexing options for a text field analyzed by the shared normalizer
/// under `case`, with positions so keyword sequences can be phrase-matched.
pub(crate) fn tokenized_text_options(case: CaseMode) -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(tokenizer_name(case))
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    )
}

pub(crate) fn strip_regex_delimiters(text: &str) -> Option<&str> {
    text.strip_prefix('/')
        .and_then(|trimmed| trimmed.strip_suffix('/'))
}

pub(crate) fn collapse_exprs(items: Vec<LqExpr>, all: bool) -> LqExpr {
    match items.len() {
        0 => LqExpr::Empty,
        1 => items.into_iter().next().map_or(LqExpr::Empty, |item| item),
        _ if all => LqExpr::All(items),
        _ => LqExpr::Any(items),
    }
}

pub(crate) fn add_metadata_fields(
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    repo_relative_path: &str,
    language: Option<&str>,
) {
    doc.add_text(fields.repo_relative_path, repo_relative_path);
    doc.add_text(fields.repo_relative_path_query, repo_relative_path);
    doc.add_text(fields.repo_relative_path_case, repo_relative_path);
    if let Some(file_name) = file_name_for_path(repo_relative_path) {
        doc.add_text(fields.file_name, file_name);
    }
    if let Some(language) = language.and_then(normalize_language) {
        doc.add_text(fields.language, &language);
    }
}

pub(crate) fn add_snippet_field(fields: &SchemaFields, doc: &mut TantivyDocument, snippet: &str) {
    doc.add_text(fields.snippet, snippet);
}

/// Store and index a chunk's text in its NFC form.
///
/// The stored copy is what the text-authority sidecars and the `index:no`
/// scan read back, so normalizing here (and idempotently again inside the
/// analyzer) keeps every surface over the same bytes.
pub(crate) fn add_content_fields(
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    indexed_text: &str,
) {
    let indexed_text = normalize::nfc(indexed_text);
    doc.add_text(fields.chunk_text, indexed_text.as_ref());
    doc.add_text(fields.chunk_text_case, indexed_text.as_ref());
}

pub(crate) fn add_symbol_fields(
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    symbol: &SymbolRecord,
) {
    doc.add_text(fields.symbol_kind, symbol.symbol_kind.as_str());
    if let Some(symbol_kind_family) = symbol.symbol_kind_family {
        doc.add_text(fields.symbol_kind_family, symbol_kind_family.as_code_str());
    }
}

pub(crate) fn stored_text(doc: &TantivyDocument, field: Field) -> Option<String> {
    let value: &OwnedValue = doc.get_first(field)?;
    Value::as_str(&value).map(str::to_owned)
}

pub(crate) fn stored_u32(doc: &TantivyDocument, field: Field) -> Result<Option<u32>, CoreError> {
    let Some(value) = doc.get_first(field) else {
        return Ok(None);
    };
    let Some(raw) = Value::as_u64(&value) else {
        return Ok(None);
    };
    u32::try_from(raw)
        .map(Some)
        .map_err(|err| CoreError::Storage(format!("lexical: stored u32 exceeds range: {err}")))
}
