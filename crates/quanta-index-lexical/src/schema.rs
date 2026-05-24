//! Tantivy schema for the Phase 1 lexical index.
//!
//! The schema is intentionally minimal:
//! - `candidate_id`, `repo_id`, `revision_id`, `repo_relative_path`: `STRING`
//!   (indexed, stored, untokenized).
//! - `manifest_generation`: `U64` (indexed, stored).
//! - `start_line`, `end_line`: `U64` (stored only).
//! - `text`: `TEXT` with the `en_stem` tokenizer (indexed and stored).
//!
//! The `LexicalFields` handle bundles field references so callers can index a
//! document without re-resolving fields from the schema on every call.

use tantivy::schema::{Field, INDEXED, STORED, STRING, Schema, TextFieldIndexing, TextOptions};

/// Field handles for the Phase 1 lexical schema.
pub(crate) struct LexicalFields {
    pub(crate) candidate_id: Field,
    pub(crate) repo_id: Field,
    pub(crate) revision_id: Field,
    pub(crate) manifest_generation: Field,
    pub(crate) repo_relative_path: Field,
    pub(crate) start_line: Field,
    pub(crate) end_line: Field,
    pub(crate) text: Field,
}

/// Build the Phase 1 lexical schema and resolve every field handle.
pub(crate) fn build_schema() -> (Schema, LexicalFields) {
    let mut builder = Schema::builder();
    let candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
    let repo_id = builder.add_text_field("repo_id", STRING | STORED);
    let revision_id = builder.add_text_field("revision_id", STRING | STORED);
    let manifest_generation = builder.add_u64_field("manifest_generation", INDEXED | STORED);
    let repo_relative_path = builder.add_text_field("repo_relative_path", STRING | STORED);
    let start_line = builder.add_u64_field("start_line", STORED);
    let end_line = builder.add_u64_field("end_line", STORED);

    // `en_stem` is the bundled English tokenizer: SimpleTokenizer + lowercase +
    // Stop + Stemmer. Tantivy registers it under that name by default.
    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer("en_stem")
        .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions);
    let text_options = TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored();
    let text = builder.add_text_field("text", text_options);

    let schema = builder.build();
    let fields = LexicalFields {
        candidate_id,
        repo_id,
        revision_id,
        manifest_generation,
        repo_relative_path,
        start_line,
        end_line,
        text,
    };
    (schema, fields)
}
