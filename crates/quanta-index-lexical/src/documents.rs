//! Building the documents the index stores: fields, snippets, languages.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::analyzer::tokenizer_name;
use crate::metadata_normalize::normalize_language;
use crate::normalize::CaseMode;
use crate::{SchemaFields, normalize};
use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{ChunkRecord, LqExpr, SourceFileRevision};
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};
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
    let local = normalize::nfc(symbol.local_name.as_ref());
    doc.add_text(
        fields.symbol_local_name_original,
        symbol.local_name.as_ref(),
    );
    doc.add_text(
        fields.symbol_qualified_name_original,
        symbol.qualified_name.as_ref(),
    );
    if let Some(signature) = &symbol.signature {
        doc.add_text(fields.symbol_signature, signature.as_ref());
    }
    doc.add_u64(
        fields.symbol_definition_start_byte,
        u64::from(symbol.definition_span.byte_start),
    );
    doc.add_u64(
        fields.symbol_definition_end_byte,
        u64::from(symbol.definition_span.byte_end),
    );
    let qualified = normalize::nfc(symbol.qualified_name.as_ref());
    doc.add_text(fields.symbol_local_name, local.as_ref());
    doc.add_text(
        fields.symbol_local_name_folded,
        normalize::fold(local.as_ref()),
    );
    doc.add_text(fields.symbol_qualified_name, qualified.as_ref());
    doc.add_text(
        fields.symbol_qualified_name_folded,
        normalize::fold(qualified.as_ref()),
    );
    doc.add_text(fields.symbol_kind, symbol.symbol_kind.as_str());
    if let Some(symbol_kind_family) = symbol.symbol_kind_family {
        doc.add_text(fields.symbol_kind_family, symbol_kind_family.as_code_str());
    }
}

/// Provenance is supplied by the canonical file replacement, never inferred
/// from the containing index generation or from returned search hits.
pub(crate) fn add_source_fields(
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    source: &SourceFileRevision,
) {
    doc.add_text(fields.source_revision_id, source.revision_id.as_str());
    doc.add_bytes(fields.source_sha256, source.source_sha256);
}

/// Commit the actual raw chunk bytes independently of the normalized postings.
pub(crate) fn add_chunk_provenance_fields(
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    chunk: &ChunkRecord,
) {
    doc.add_u64(fields.chunk_start_byte, u64::from(chunk.start_byte));
    doc.add_u64(fields.chunk_end_byte, u64::from(chunk.end_byte));
    let digest: [u8; 32] = Sha256::digest(chunk.text.as_bytes()).into();
    doc.add_bytes(fields.chunk_raw_sha256, digest);
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
