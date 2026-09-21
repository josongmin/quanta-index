//! Turning stored documents into result candidates.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::TantivySearcher;
use crate::documents::{stored_text, stored_u32};
use crate::searcher::snippets::window_snippet;
use quanta_index_contract::lex::{SymbolKindCode, SymbolKindFamily};
use quanta_index_contract::{LexicalCandidate, RepoRelativePath, SymbolCandidate};
use quanta_index_core::CoreError;
use tantivy::schema::TantivyDocument;

impl TantivySearcher {
    pub(crate) fn document_to_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
        center_terms: &[String],
    ) -> Result<LexicalCandidate, CoreError> {
        let candidate_id = stored_text(doc, self.fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing candidate_id field".to_string())
        })?;
        let stored_snippet = stored_text(doc, self.fields.snippet)
            .or_else(|| stored_text(doc, self.fields.chunk_text))
            .ok_or_else(|| {
                CoreError::Storage(
                    "lexical: stored doc missing snippet/chunk_text field".to_string(),
                )
            })?;
        // J7Q-02: emit a hit-centered, deterministically-bounded window so a long
        // source line never streams an unbounded blob at the head of the result.
        // J7Q-07: the window also reports the primary hit's byte offset plus every
        // matched-hit span for UI highlighting, so a consumer never re-derives the
        // matches from raw text.
        let (snippet, snippet_hit_offset, highlights) =
            window_snippet(&stored_snippet, center_terms);
        let repo_relative_path =
            stored_text(doc, self.fields.repo_relative_path).ok_or_else(|| {
                CoreError::Storage(
                    "lexical: stored doc missing repo_relative_path field".to_string(),
                )
            })?;
        let start_line = stored_u32(doc, self.fields.start_line)?.ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing start_line field".to_string())
        })?;
        let end_line = stored_u32(doc, self.fields.end_line)?.ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing end_line field".to_string())
        })?;
        Ok(LexicalCandidate {
            candidate_id,
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            manifest_generation: self.generation,
            repo_relative_path: RepoRelativePath::new(repo_relative_path),
            start_line,
            end_line,
            score,
            snippet,
            snippet_hit_offset,
            highlights,
        })
    }

    pub(crate) fn document_to_symbol_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
        center_terms: &[String],
    ) -> Result<SymbolCandidate, CoreError> {
        let candidate = self.document_to_candidate(doc, score, center_terms)?;
        let symbol_kind = stored_text(doc, self.fields.symbol_kind)
            .ok_or_else(|| {
                CoreError::Storage(
                    "lexical: stored symbol doc missing symbol_kind field".to_string(),
                )
            })
            .and_then(|raw| {
                SymbolKindCode::new(raw).map_err(|err| {
                    CoreError::Storage(format!("lexical: invalid stored symbol_kind: {err}"))
                })
            })?;
        let symbol_kind_family = match stored_text(doc, self.fields.symbol_kind_family) {
            Some(raw) => Some(SymbolKindFamily::from_code_str(raw.as_str()).ok_or_else(|| {
                CoreError::Storage(format!("lexical: invalid stored symbol_kind_family `{raw}`"))
            })?),
            None => None,
        };
        Ok(SymbolCandidate {
            candidate_id: candidate.candidate_id,
            repo_id: candidate.repo_id,
            revision_id: candidate.revision_id,
            manifest_generation: candidate.manifest_generation,
            repo_relative_path: candidate.repo_relative_path,
            start_line: candidate.start_line,
            end_line: candidate.end_line,
            score,
            snippet: candidate.snippet,
            symbol_kind,
            symbol_kind_family,
        })
    }
}
