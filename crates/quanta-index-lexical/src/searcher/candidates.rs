//! Turning stored documents into result candidates.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::PreparedPredicatePlan;
use crate::TantivySearcher;
use crate::documents::{stored_text, stored_u32};
use crate::searcher::snippets::{
    SelectedSnippetSource, SnippetContext, SnippetLimits, integrity, render_selected,
};
use quanta_index_contract::lex::{SymbolKindCode, SymbolKindFamily};
use quanta_index_contract::{LexicalCandidate, RepoRelativePath, SymbolCandidate};
use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqPatternType, LqQuery, PreviewKind, PreviewMetadata,
    PreviewUnavailableReason, RepoId, RevisionId, SourceFileKey, SourceFileRevision,
};
use quanta_index_core::CoreError;
use quanta_index_core::{LexicalCollectionBudget, LexicalMemoryReservation, RequestBudgetV1};
use quanta_index_lq_regex::RegexExecutor;
use std::collections::BTreeMap;
use tantivy::schema::{Field, OwnedValue, TantivyDocument, Value};

/// One selected page's executor pool sharing the request's preview account.
/// Retained output leases remain here until the caller transfers the response.
pub(crate) struct SelectedPreviewContext<'a> {
    query: &'a LqQuery,
    prepared: &'a PreparedPredicatePlan,
    request: &'a RequestBudgetV1,
    ledger: LexicalCollectionBudget,
    executors: BTreeMap<String, RegexExecutor>,
    executor_reservations: Vec<LexicalMemoryReservation>,
    output_reservations: Vec<LexicalMemoryReservation>,
    output_slot: Option<usize>,
    prepared_for_render: bool,
    unavailable: Option<PreviewUnavailableReason>,
}

fn unique_value(doc: &TantivyDocument, field: Field) -> Result<Option<&OwnedValue>, CoreError> {
    let mut values = doc.get_all(field);
    let value = values.next();
    if values.next().is_some() {
        return Err(integrity("duplicate stored source field"));
    }
    Ok(value)
}

fn source_text(doc: &TantivyDocument, field: Field) -> Result<Option<&str>, CoreError> {
    unique_value(doc, field)?
        .map(|value| Value::as_str(&value).ok_or_else(|| integrity("malformed stored source text")))
        .transpose()
}

fn source_digest(doc: &TantivyDocument, field: Field) -> Result<Option<[u8; 32]>, CoreError> {
    unique_value(doc, field)?
        .map(|value| {
            Value::as_bytes(&value)
                .ok_or_else(|| integrity("malformed stored source digest"))?
                .try_into()
                .map_err(|_length| integrity("stored source digest is not 32 bytes"))
        })
        .transpose()
}

fn source_offset(doc: &TantivyDocument, field: Field) -> Result<Option<u64>, CoreError> {
    unique_value(doc, field)?
        .map(|value| Value::as_u64(&value).ok_or_else(|| integrity("malformed source offset")))
        .transpose()
}

impl SelectedPreviewContext<'_> {
    /// Empty selections and unrenderable rows need no executor or output slot.
    fn prepare_for_render(&mut self) -> Result<(), CoreError> {
        self.request.checkpoint("lexical:preview-prepare")?;
        if self.prepared_for_render {
            return Ok(());
        }
        self.prepared_for_render = true;
        self.prepare_expr(&self.prepared.expr, 0)?;
        for filter in &self.query.filters {
            if let LqFilter::Content { leaf } = filter {
                self.prepare_leaf(leaf)?;
            }
        }
        Ok(())
    }

    /// Keep emitted allocations charged through the caller's request lifetime.
    pub(crate) fn retain_output_for_request(&mut self) -> Result<(), CoreError> {
        self.request.checkpoint("lexical:preview-output")?;
        // A finished context cannot emit additional unretained outputs.
        self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
        let Some(slot) = self.output_slot.take() else {
            if self.output_reservations.is_empty() {
                return Ok(());
            }
            return Err(integrity("preview output has no reserved request slot"));
        };
        self.request
            .retain_lexical_output(slot, core::mem::take(&mut self.output_reservations))
    }

    fn admit(&mut self, result: Result<(), CoreError>) -> Result<bool, CoreError> {
        self.request.checkpoint("lexical:preview-prepare")?;
        match result {
            Ok(()) => Ok(true),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
                ..
            }) => {
                self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn prepare_leaf(&mut self, leaf: &LqLeaf) -> Result<(), CoreError> {
        if self.unavailable.is_some() {
            return Ok(());
        }
        let text = match leaf {
            LqLeaf::Regex(text) => text,
            LqLeaf::Keyword(text) | LqLeaf::RawString(text)
                if self.query.options.pattern_type == LqPatternType::Regexp =>
            {
                text
            }
            LqLeaf::Predicate { .. } | LqLeaf::StructuralBlock(_) => {
                // A residual authority leaf has no bounded per-row truth API.
                // Keep the admitted hit and explicitly decline reconstruction.
                self.unavailable = Some(PreviewUnavailableReason::UnsupportedRange);
                return Ok(());
            }
            LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Phrase(_) => return Ok(()),
        };
        if self.executors.contains_key(text) || self.unavailable.is_some() {
            return Ok(());
        }
        if text.len() > SnippetLimits::default().source_bytes {
            self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
            return Ok(());
        }
        let work =
            u64::try_from(text.len()).map_err(|_overflow| integrity("pattern size overflow"))?;
        if !self.admit(self.ledger.charge_work(work.saturating_add(100_000)))? {
            return Ok(());
        }
        // This fixed logical charge bounds the number of retained executors.
        // The engine's 10 MiB limit applies per NFA, not to total compiler/HIR
        // allocations. Do not interpret this charge as a total heap ceiling.
        let reservation = match self.ledger.reserve_bytes(16 * 1024 * 1024) {
            Ok(reservation) => reservation,
            Err(error) => {
                let _admitted = self.admit(Err(error))?;
                return Ok(());
            }
        };
        if self.executor_reservations.try_reserve_exact(1).is_err() {
            self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
            return Ok(());
        }
        let pattern = TantivySearcher::regex_source_for_options(text, &self.query.options);
        let executor = RegexExecutor::compile(&pattern)
            .map_err(|error| integrity(&format!("prepared regex failed to compile: {error}")))?;
        self.request.checkpoint("lexical:preview-regex-compiled")?;
        drop(self.executors.insert(text.clone(), executor));
        self.executor_reservations.push(reservation);
        Ok(())
    }

    fn prepare_expr(&mut self, expr: &LqExpr, depth: usize) -> Result<(), CoreError> {
        if depth > 64 || self.unavailable.is_some() {
            if self.unavailable.is_none() {
                self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
            }
            return Ok(());
        }
        if !self.admit(self.ledger.charge_work(1))? {
            return Ok(());
        }
        match expr {
            LqExpr::Leaf(leaf) => self.prepare_leaf(leaf)?,
            LqExpr::All(children) | LqExpr::Any(children) => {
                for child in children {
                    self.prepare_expr(child, depth.saturating_add(1))?;
                }
            }
            LqExpr::Not(inner) => self.prepare_expr(inner, depth.saturating_add(1))?,
            LqExpr::Empty => {}
        }
        Ok(())
    }
}

impl TantivySearcher {
    pub(crate) fn selected_preview_context<'a>(
        &self,
        query: &'a LqQuery,
        prepared: &'a PreparedPredicatePlan,
        request: &'a RequestBudgetV1,
    ) -> Result<SelectedPreviewContext<'a>, CoreError> {
        let ledger = request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
        Ok(SelectedPreviewContext {
            query,
            prepared,
            request,
            ledger,
            executors: BTreeMap::new(),
            executor_reservations: Vec::new(),
            output_reservations: Vec::new(),
            output_slot: None,
            prepared_for_render: false,
            unavailable: None,
        })
    }

    fn selected_source_identity(
        &self,
        doc: &TantivyDocument,
        path: &str,
    ) -> Result<Option<SourceFileRevision>, CoreError> {
        let revision = source_text(doc, self.fields.source_revision_id)?;
        let digest = source_digest(doc, self.fields.source_sha256)?;
        let source = match (revision, digest) {
            (None, None) if self.source_coverage.is_none() => return Ok(None),
            (Some(revision), Some(digest)) => SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new(
                        source_text(doc, self.fields.repo_id)?
                            .ok_or_else(|| integrity("source owner missing"))?,
                    )
                    .map_err(|_invalid| integrity("invalid source owner"))?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new(revision)
                    .map_err(|_invalid| integrity("invalid source revision"))?,
                source_sha256: digest,
            },
            _ => return Err(integrity("missing or partial source identity")),
        };
        source.validate().map_err(integrity)?;
        if let Some(coverage) = &self.source_coverage {
            let admitted = coverage
                .get(&source.file)
                .ok_or_else(|| integrity("source absent from sealed coverage"))?;
            if admitted.source != source {
                return Err(integrity("source disagrees with sealed coverage"));
            }
        }
        Ok(Some(source))
    }

    pub(crate) fn document_to_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
        context: &mut SelectedPreviewContext<'_>,
    ) -> Result<LexicalCandidate, CoreError> {
        context.request.checkpoint("lexical:selected-preview")?;
        let mut candidate = self.document_to_candidate_identity(doc, score)?;
        let kind = match source_text(doc, self.fields.doc_kind)? {
            Some(crate::TEXT_DOC_KIND) => PreviewKind::SourceChunk,
            Some(crate::SYMBOL_DOC_KIND) => PreviewKind::SyntheticSymbolLabel,
            _ => return Err(integrity("selected document kind missing or invalid")),
        };
        if candidate.source.is_none() {
            candidate.preview = Some(PreviewMetadata::unavailable(
                kind,
                PreviewUnavailableReason::SourceNotProvided,
                None,
            ));
            return Ok(candidate);
        }
        let raw = source_text(doc, self.fields.snippet)?
            .ok_or_else(|| integrity("immutable raw snippet missing"))?;
        let indexed = source_text(doc, self.fields.chunk_text)?
            .ok_or_else(|| integrity("immutable indexed text missing"))?;
        let (start, digest) = if kind == PreviewKind::SourceChunk {
            let start = source_offset(doc, self.fields.chunk_start_byte)?
                .ok_or_else(|| integrity("chunk start offset missing"))?;
            let end = source_offset(doc, self.fields.chunk_end_byte)?
                .ok_or_else(|| integrity("chunk end offset missing"))?;
            let size =
                u64::try_from(raw.len()).map_err(|_overflow| integrity("chunk length overflow"))?;
            if end.checked_sub(start) != Some(size) {
                return Err(integrity("chunk extent disagrees with raw bytes"));
            }
            let digest = source_digest(doc, self.fields.chunk_raw_sha256)?
                .ok_or_else(|| integrity("immutable raw digest missing"))?;
            let authority = self
                .text_authority
                .as_ref()
                .ok_or_else(|| integrity("selected text authority missing"))?;
            let id = crate::text_docs::stored_text_authority_doc_id(
                doc,
                &self.fields,
                &candidate.candidate_id,
            )
            .map_err(|error| integrity(&format!("invalid selected text authority id: {error}")))?;
            let authoritative = authority
                .doc(id)
                .ok_or_else(|| integrity("selected text authority member missing"))?;
            if authoritative.candidate_id != candidate.candidate_id
                || authoritative.indexed_text != indexed
            {
                return Err(integrity("selected document disagrees with text authority"));
            }
            (Some(start), Some(digest))
        } else {
            (None, None)
        };
        let limits = SnippetLimits::default();
        if raw.len() > limits.source_bytes || indexed.len() > limits.transformed_bytes {
            context.request.checkpoint("lexical:preview-unavailable")?;
            candidate.preview = Some(PreviewMetadata::unavailable(
                kind,
                PreviewUnavailableReason::WorkBudget,
                None,
            ));
            return Ok(candidate);
        }
        context.prepare_for_render()?;
        if let Some(reason) = context.unavailable {
            candidate.preview = Some(PreviewMetadata::unavailable(kind, reason, None));
            return Ok(candidate);
        }
        let rendered = render_selected(
            &SnippetContext {
                expr: &context.prepared.expr,
                filters: &context.query.filters,
                options: &context.query.options,
                limits,
                ledger: &context.ledger,
                request: context.request,
            },
            &SelectedSnippetSource {
                raw: Some(raw),
                indexed_nfc: Some(indexed),
                path: candidate.repo_relative_path.as_str(),
                kind,
                source: candidate.source.as_ref(),
                chunk_start_byte: start,
                expected_raw_sha256: digest,
            },
            &|text| {
                context
                    .executors
                    .get(text)
                    .ok_or_else(|| integrity("prepared regex missing"))
            },
            &|_leaf| Err(integrity("residual authority leaf was not refused")),
        )?;
        // Only actual output needs a retention slot. A page consisting entirely
        // of unavailable previews must not exhaust the request's output carrier.
        // The renderer's lease charges the transient output through admission.
        if rendered.reservation.is_some() && context.output_slot.is_none() {
            context.output_slot = context.request.reserve_lexical_output_group()?;
        }
        // The retained lease includes this vector slot. Allocation/slot refusal
        // is still optional and must preserve the selected identity.
        if rendered.reservation.is_some()
            && (context.output_slot.is_none()
                || context.output_reservations.try_reserve_exact(1).is_err())
        {
            context
                .request
                .checkpoint("lexical:preview-output-allocation")?;
            context.unavailable = Some(PreviewUnavailableReason::WorkBudget);
            candidate.preview = Some(PreviewMetadata::unavailable(
                kind,
                PreviewUnavailableReason::WorkBudget,
                None,
            ));
            return Ok(candidate);
        }
        candidate.snippet = rendered.snippet;
        candidate.snippet_hit_offset = rendered.snippet_hit_offset;
        candidate.highlights = rendered.highlights;
        candidate.preview = Some(rendered.preview);
        if let Some(reservation) = rendered.reservation {
            context.output_reservations.push(reservation);
        }
        Ok(candidate)
    }

    pub(crate) fn document_to_symbol_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
        context: &mut SelectedPreviewContext<'_>,
    ) -> Result<SymbolCandidate, CoreError> {
        self.symbol_from_candidate(doc, self.document_to_candidate(doc, score, context)?)
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "stored-field regressions assert corruption refusals"
)]
mod l4_source_decode_regressions {
    use super::*;
    use tantivy::schema::{STORED, STRING, Schema};

    #[test]
    fn malformed_and_duplicate_authority_fields_are_not_defaulted() -> Result<(), CoreError> {
        let mut builder = Schema::builder();
        let revision = builder.add_text_field("revision", STRING | STORED);
        let digest = builder.add_bytes_field("digest", STORED);
        let offset = builder.add_u64_field("offset", STORED);
        let _schema = builder.build();
        let mut doc = TantivyDocument::new();
        assert_eq!(source_text(&doc, revision)?, None);
        doc.add_text(revision, "r1");
        doc.add_text(revision, "r2");
        assert!(source_text(&doc, revision).is_err());
        doc.add_bytes(digest, [7; 31]);
        assert!(source_digest(&doc, digest).is_err());
        doc.add_text(offset, "42");
        assert!(source_offset(&doc, offset).is_err());
        Ok(())
    }
}

impl TantivySearcher {
    pub(crate) fn document_to_candidate_identity(
        &self,
        doc: &TantivyDocument,
        score: f32,
    ) -> Result<LexicalCandidate, CoreError> {
        let candidate_id = source_text(doc, self.fields.candidate_id)?
            .map(str::to_owned)
            .ok_or_else(|| {
                CoreError::Storage("lexical: stored doc missing candidate_id field".to_string())
            })?;
        let repo_relative_path = source_text(doc, self.fields.repo_relative_path)?
            .map(str::to_owned)
            .ok_or_else(|| {
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
        let source = self.selected_source_identity(doc, &repo_relative_path)?;
        let source_repo_id = RepoId::new(
            source_text(doc, self.fields.repo_id)?
                .ok_or_else(|| integrity("stored source owner missing"))?,
        )
        .map_err(|_invalid| integrity("invalid stored source owner"))?;
        Ok(LexicalCandidate {
            source_repo_id,
            source,
            preview: None,
            candidate_id,
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            manifest_generation: self.generation,
            repo_relative_path: RepoRelativePath::new(repo_relative_path),
            start_line,
            end_line,
            score,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        })
    }

    pub(crate) fn document_to_symbol_candidate_identity(
        &self,
        doc: &TantivyDocument,
        score: f32,
    ) -> Result<SymbolCandidate, CoreError> {
        let candidate = self.document_to_candidate_identity(doc, score)?;
        self.symbol_from_candidate(doc, candidate)
    }

    fn symbol_from_candidate(
        &self,
        doc: &TantivyDocument,
        candidate: LexicalCandidate,
    ) -> Result<SymbolCandidate, CoreError> {
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
            Some(raw) => Some(
                SymbolKindFamily::from_code_str(raw.as_str()).ok_or_else(|| {
                    CoreError::Storage(format!(
                        "lexical: invalid stored symbol_kind_family `{raw}`"
                    ))
                })?,
            ),
            None => None,
        };
        Ok(SymbolCandidate {
            source_repo_id: candidate.source_repo_id,
            source: candidate.source,
            preview: candidate.preview,
            candidate_id: candidate.candidate_id,
            repo_id: candidate.repo_id,
            revision_id: candidate.revision_id,
            manifest_generation: candidate.manifest_generation,
            repo_relative_path: candidate.repo_relative_path,
            start_line: candidate.start_line,
            end_line: candidate.end_line,
            score: candidate.score,
            snippet: candidate.snippet,
            symbol_kind,
            symbol_kind_family,
        })
    }
}
