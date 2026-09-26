//! The index schema: the fields every document carries and the document kinds.

use crate::documents::tokenized_text_options;
use crate::normalize::CaseMode;
use crate::{QueryDocKind, SYMBOL_DOC_KIND, SchemaFields, TEXT_DOC_KIND};
use tantivy::schema::{FAST, INDEXED, STORED, STRING, Schema};

impl QueryDocKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => TEXT_DOC_KIND,
            Self::Symbol => SYMBOL_DOC_KIND,
        }
    }
}

impl SchemaFields {
    pub(crate) fn build() -> Self {
        let mut builder = Schema::builder();
        // The ranked page order's columns are fast columns, so pages are
        // ranked, cut and grouped without reading stored documents
        // (QI-BB-005).
        let candidate_id = builder.add_text_field("candidate_id", STRING | STORED | FAST);
        let repo_id = builder.add_text_field("repo_id", STRING | STORED | FAST);
        let revision_id = builder.add_text_field("revision_id", STRING | STORED);
        let source_revision_id = builder.add_text_field("source_revision_id", STRING | STORED);
        let source_sha256 = builder.add_bytes_field("source_sha256", STORED);
        let chunk_start_byte = builder.add_u64_field("chunk_start_byte", STORED);
        let chunk_end_byte = builder.add_u64_field("chunk_end_byte", STORED);
        let chunk_raw_sha256 = builder.add_bytes_field("chunk_raw_sha256", STORED);
        let doc_kind = builder.add_text_field("doc_kind", STRING | STORED);
        let repo_relative_path =
            builder.add_text_field("repo_relative_path", STRING | STORED | FAST);
        let repo_relative_path_query = builder.add_text_field(
            "repo_relative_path_query",
            tokenized_text_options(CaseMode::Folded),
        );
        let repo_relative_path_case = builder.add_text_field(
            "repo_relative_path_case",
            tokenized_text_options(CaseMode::Sensitive),
        );
        let file_name = builder.add_text_field("file_name", STRING);
        let language = builder.add_text_field("language", STRING);
        let start_line = builder.add_u64_field("start_line", STORED | FAST);
        let end_line = builder.add_u64_field("end_line", STORED | FAST);
        let snippet = builder.add_text_field("snippet", STORED);
        let chunk_text = builder.add_text_field(
            "chunk_text",
            tokenized_text_options(CaseMode::Folded) | STORED,
        );
        let chunk_text_case = builder.add_text_field(
            "chunk_text_case",
            tokenized_text_options(CaseMode::Sensitive),
        );
        let symbol_kind = builder.add_text_field("symbol_kind", STRING | STORED);
        let symbol_kind_family = builder.add_text_field("symbol_kind_family", STRING | STORED);
        let symbol_local_name = builder.add_text_field("symbol_local_name", STRING | STORED);
        let symbol_local_name_folded = builder.add_text_field("symbol_local_name_folded", STRING);
        let symbol_qualified_name =
            builder.add_text_field("symbol_qualified_name", STRING | STORED);
        let symbol_qualified_name_folded =
            builder.add_text_field("symbol_qualified_name_folded", STRING);
        let symbol_local_name_original =
            builder.add_text_field("symbol_local_name_original", STORED);
        let symbol_qualified_name_original =
            builder.add_text_field("symbol_qualified_name_original", STORED);
        let symbol_signature = builder.add_text_field("symbol_signature", STORED);
        let symbol_definition_start_byte =
            builder.add_u64_field("symbol_definition_start_byte", STORED);
        let symbol_definition_end_byte =
            builder.add_u64_field("symbol_definition_end_byte", STORED);
        let text_authority_doc_id =
            builder.add_u64_field("text_authority_doc_id", STORED | INDEXED | FAST);
        let schema = builder.build();
        Self {
            schema,
            candidate_id,
            repo_id,
            revision_id,
            source_revision_id,
            source_sha256,
            chunk_start_byte,
            chunk_end_byte,
            chunk_raw_sha256,
            doc_kind,
            repo_relative_path,
            repo_relative_path_query,
            repo_relative_path_case,
            file_name,
            language,
            start_line,
            end_line,
            snippet,
            chunk_text,
            chunk_text_case,
            symbol_kind,
            symbol_kind_family,
            symbol_local_name,
            symbol_local_name_folded,
            symbol_qualified_name,
            symbol_qualified_name_folded,
            symbol_local_name_original,
            symbol_qualified_name_original,
            symbol_signature,
            symbol_definition_start_byte,
            symbol_definition_end_byte,
            text_authority_doc_id,
        }
    }
}
