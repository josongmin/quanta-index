//! The schema of one kind's index and how a document is written to it.
//!
//! The text field is analyzed by the shared text normalizer
//! ([`crate::analyzer::NormalizingTokenizer`]) in both case modes, exactly
//! as the corpus index is, so a keyword or phrase means the same tokens
//! here as on the lexical route. The row identity (sha, path) and the
//! committer time are fast columns: the collector reads them for every
//! scored document to place it under the relevance total order and to
//! hand it to the search plane's row predicate, without touching a
//! document store. Nothing is stored; the response rows come from the
//! epoch's snapshot, never from the index.

use quanta_index_core::{CoreError, HistoryTextDocKeyV1, HistoryTextDocV1, HistoryTextKindV1};
use tantivy::schema::{
    FAST, Field, IndexRecordOption, STRING, Schema, TantivyDocument, TextFieldIndexing, TextOptions,
};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{Index, Term};

use crate::analyzer::{NormalizingTokenizer, tokenizer_name};
use crate::normalize::{self, CaseMode};

const DOC_KEY_FIELD: &str = "doc_key";
const TEXT_FIELD: &str = "text";
const TEXT_CASE_FIELD: &str = "text_case";
const COMMITTER_TIME_FIELD: &str = "committer_time_ms";
const SHA_FIELD: &str = "sha";
const PATH_FIELD: &str = "path";
/// Separator between the sha and the path in a diff document's key.
const DOC_KEY_SEPARATOR: char = '\u{1f}';

/// The fields of one kind's index.
#[derive(Clone, Debug)]
pub(super) struct KindSchema {
    pub(super) kind: HistoryTextKindV1,
    pub(super) schema: Schema,
    doc_key: Field,
    text: Field,
    text_case: Field,
    committer_time_ms: Field,
    sha: Field,
    /// Present for the diff index only.
    path: Option<Field>,
}

impl KindSchema {
    #[must_use]
    pub(super) fn build(kind: HistoryTextKindV1) -> Self {
        let mut builder = Schema::builder();
        let doc_key = builder.add_text_field(DOC_KEY_FIELD, STRING);
        let text_options = |case: CaseMode| {
            TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(tokenizer_name(case))
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
        };
        let text = builder.add_text_field(TEXT_FIELD, text_options(CaseMode::Folded));
        let text_case = builder.add_text_field(TEXT_CASE_FIELD, text_options(CaseMode::Sensitive));
        let committer_time_ms = builder.add_u64_field(COMMITTER_TIME_FIELD, FAST);
        let sha = builder.add_bytes_field(SHA_FIELD, FAST);
        let path = match kind {
            HistoryTextKindV1::Commit => None,
            HistoryTextKindV1::Diff => Some(builder.add_text_field(PATH_FIELD, STRING | FAST)),
        };
        Self {
            kind,
            schema: builder.build(),
            doc_key,
            text,
            text_case,
            committer_time_ms,
            sha,
            path,
        }
    }

    /// The text field a query in `case` mode scores.
    #[must_use]
    pub(super) const fn text_field(&self, case: CaseMode) -> Field {
        match case {
            CaseMode::Folded => self.text,
            CaseMode::Sensitive => self.text_case,
        }
    }

    /// The names of the fast columns the collector reads.
    #[must_use]
    pub(super) const fn committer_time_column() -> &'static str {
        COMMITTER_TIME_FIELD
    }

    #[must_use]
    pub(super) const fn sha_column() -> &'static str {
        SHA_FIELD
    }

    #[must_use]
    pub(super) const fn path_column() -> &'static str {
        PATH_FIELD
    }

    /// The term that identifies exactly the document under `key`, for
    /// delete-before-upsert.
    pub(super) fn doc_key_term(&self, key: &HistoryTextDocKeyV1) -> Result<Term, CoreError> {
        self.require_kind(key.kind())?;
        Ok(Term::from_field_text(self.doc_key, &doc_key_text(key)))
    }

    /// The document `doc` becomes in this index.
    pub(super) fn document(&self, doc: &HistoryTextDocV1) -> Result<TantivyDocument, CoreError> {
        self.require_kind(doc.key.kind())?;
        let mut document = TantivyDocument::default();
        document.add_text(self.doc_key, doc_key_text(&doc.key));
        let text = normalize::nfc(&doc.text);
        document.add_text(self.text, text.as_ref());
        document.add_text(self.text_case, text.as_ref());
        document.add_u64(self.committer_time_ms, doc.committer_time_ms);
        document.add_bytes(self.sha, doc.key.sha().as_bytes().to_vec());
        match (self.path, doc.key.file_path()) {
            (Some(field), Some(path)) => document.add_text(field, path),
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(CoreError::Storage(format!(
                    "history text index: {} document key does not match the {} index",
                    doc.key.kind().as_str(),
                    self.kind.as_str()
                )));
            }
        }
        Ok(document)
    }

    fn require_kind(&self, kind: HistoryTextKindV1) -> Result<(), CoreError> {
        if kind == self.kind {
            return Ok(());
        }
        Err(CoreError::Storage(format!(
            "history text index: a {} document was routed to the {} index",
            kind.as_str(),
            self.kind.as_str()
        )))
    }
}

/// The key text of one document: the sha's hex, plus the path for diffs.
#[must_use]
pub(super) fn doc_key_text(key: &HistoryTextDocKeyV1) -> String {
    let mut text = key.sha().to_hex();
    if let Some(path) = key.file_path() {
        text.push(DOC_KEY_SEPARATOR);
        text.push_str(path);
    }
    text
}

/// Register the two analyzers the text fields name on an opened index.
///
/// Both are the shared normalizer; they differ only in case mode.
pub(super) fn register_tokenizers(index: &Index) {
    for case in [CaseMode::Folded, CaseMode::Sensitive] {
        index.tokenizers().register(
            tokenizer_name(case),
            TextAnalyzer::from(NormalizingTokenizer::new(case)),
        );
    }
}
