//! Turning stored documents into result candidates.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::PreparedPredicatePlan;
use crate::TantivySearcher;
use crate::documents::stored_u32;
use crate::searcher::snippets::{
    SelectedSnippetSource, SnippetContext, SnippetLimits, integrity, render_selected,
    verify_raw_digest,
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

const PREVIEW_REGEX_BASE_BYTES: u64 = 16 * 1024 * 1024;
// A conservative *policy charge* for complex plans, not an allocator-enforced
// heap bound. The regex engine does not expose aggregate temporary allocation.
const PREVIEW_REGEX_BYTES_PER_ESTIMATED_STATE: u64 = 256;

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

/// A selected source chunk must retain its integrity requirement even when
/// optional preview preparation refuses to render it.
fn unavailable_selected_preview(
    kind: PreviewKind,
    reason: PreviewUnavailableReason,
    raw: &str,
    digest: Option<[u8; 32]>,
    request: &RequestBudgetV1,
) -> Result<PreviewMetadata, CoreError> {
    match (kind, digest) {
        (PreviewKind::SourceChunk, Some(expected)) => {
            verify_raw_digest(raw, expected, request)?;
        }
        (PreviewKind::SourceChunk, None) => {
            return Err(integrity("immutable raw digest missing"));
        }
        _ => {}
    }
    request.checkpoint("lexical:preview-unavailable")?;
    Ok(PreviewMetadata::unavailable(kind, reason, None))
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
        // Reserve before AST/HIR planning. The additional state-proportional
        // charge below precedes the engine's much larger automata allocation.
        // Neither charge claims to be an allocator-enforced heap ceiling.
        let reservation = match self.ledger.reserve_bytes(PREVIEW_REGEX_BASE_BYTES) {
            Ok(reservation) => reservation,
            Err(error) => {
                let _admitted = self.admit(Err(error))?;
                return Ok(());
            }
        };
        if self.executor_reservations.try_reserve_exact(2).is_err() {
            self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
            return Ok(());
        }
        let pattern = TantivySearcher::regex_source_for_options(text, &self.query.options);
        let plan = RegexExecutor::prepare(&pattern)
            .map_err(|error| integrity(&format!("prepared regex failed to plan: {error}")))?;
        self.request.checkpoint("lexical:preview-regex-planned")?;
        let extra_bytes = plan
            .estimated_states()
            .checked_mul(PREVIEW_REGEX_BYTES_PER_ESTIMATED_STATE)
            .ok_or_else(|| integrity("preview regex state charge overflow"))?;
        let extra_reservation = match self.ledger.reserve_bytes(extra_bytes) {
            Ok(reservation) => reservation,
            Err(error) => {
                let _admitted = self.admit(Err(error))?;
                return Ok(());
            }
        };
        let executor = match RegexExecutor::compile_prepared(plan) {
            Ok(executor) => executor,
            Err(error)
                if error.code == quanta_index_lq_regex::RegexErrorCode::PlanLimitExceeded =>
            {
                self.unavailable = Some(PreviewUnavailableReason::WorkBudget);
                return Ok(());
            }
            Err(error) => {
                return Err(integrity(&format!(
                    "prepared regex failed to compile: {error}"
                )));
            }
        };
        self.request.checkpoint("lexical:preview-regex-compiled")?;
        drop(self.executors.insert(text.clone(), executor));
        self.executor_reservations.push(reservation);
        self.executor_reservations.push(extra_reservation);
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
            candidate.preview = Some(unavailable_selected_preview(
                kind,
                PreviewUnavailableReason::WorkBudget,
                raw,
                digest,
                context.request,
            )?);
            return Ok(candidate);
        }
        context.prepare_for_render()?;
        if let Some(reason) = context.unavailable {
            candidate.preview = Some(unavailable_selected_preview(
                kind,
                reason,
                raw,
                digest,
                context.request,
            )?);
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
        let symbol_kind = source_text(doc, self.fields.symbol_kind)?
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
        let symbol_kind_family = match source_text(doc, self.fields.symbol_kind_family)? {
            Some(raw) => Some(SymbolKindFamily::from_code_str(raw).ok_or_else(|| {
                CoreError::Storage(format!(
                    "lexical: invalid stored symbol_kind_family `{raw}`"
                ))
            })?),
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

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "stored-field regressions assert corruption refusals"
)]
mod l4_source_decode_regressions {
    use super::*;
    use crate::analyzer::register_analyzers;
    use crate::documents::{add_content_fields, add_metadata_fields, add_snippet_field};
    use crate::ranked_keys::{self, RankedKeyTables, SegmentKeys};
    use crate::regex::RegexPolicy;
    use crate::regex_match_cache::RegexMatchCache;
    use crate::text_authority::{self, AddedTextDoc, ShardedTextAuthority};
    use crate::{SchemaFields, TantivySearcher};
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan, LqYesNoOnly,
        ManifestGeneration, PreviewUnavailableReason, RepoId, RevisionId, SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::{
        LexicalArtifactIdentityV1, LexicalExecutionBudgetV1, LexicalSearcher,
        RegexMatchCachePolicy, RepoMetadataAuthoritiesV1, TextNormalizerVersionV1,
    };
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    use tantivy::schema::{STORED, STRING, Schema};
    use tantivy::{Index, ReloadPolicy};

    #[test]
    fn indexed_and_manual_search_refuse_corrupt_selected_row_under_optional_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let fields = SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        register_analyzers(&index);
        let raw = "needle";
        let mut doc = TantivyDocument::new();
        doc.add_text(fields.candidate_id, "chunk-needle.rs");
        doc.add_text(fields.repo_id, "source-repo");
        doc.add_text(fields.revision_id, "index-r1");
        doc.add_text(fields.source_revision_id, "source-r1");
        let source_digest: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
        doc.add_bytes(fields.source_sha256, source_digest);
        doc.add_text(fields.doc_kind, crate::TEXT_DOC_KIND);
        add_metadata_fields(&fields, &mut doc, "needle.rs", Some("rust"));
        doc.add_u64(fields.start_line, 1);
        doc.add_u64(fields.end_line, 1);
        add_snippet_field(&fields, &mut doc, raw);
        add_content_fields(&fields, &mut doc, raw);
        doc.add_u64(fields.chunk_start_byte, 0);
        doc.add_u64(fields.chunk_end_byte, u64::try_from(raw.len())?);
        doc.add_bytes(fields.chunk_raw_sha256, [0; 32]);
        doc.add_u64(fields.text_authority_doc_id, 1);
        let mut writer = index.writer(50_000_000)?;
        let _opstamp = writer.add_document(doc)?;
        let _commit = writer.commit()?;
        drop(writer);

        let reader: tantivy::IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        reader.reload()?;
        let segments = reader.searcher();
        let segment_readers = segments.segment_readers();
        let tables = segment_readers
            .iter()
            .map(|segment| {
                let bytes = ranked_keys::encode(segment)?;
                Ok(Arc::new(SegmentKeys::decode(bytes, segment)?))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let ranked_keys = Arc::new(RankedKeyTables::bind(tables, segment_readers)?);

        let generation = ManifestGeneration::new(1);
        let _receipt = text_authority::rebuild(
            state.path(),
            generation,
            vec![AddedTextDoc {
                doc_id: 1,
                candidate_id: "chunk-needle.rs".into(),
                text: raw.into(),
            }],
            None,
            1,
        )?;
        let manifest = text_authority::read_manifest(state.path())?
            .ok_or("text authority manifest missing")?;
        let shards = manifest
            .shards
            .iter()
            .map(|entry| {
                Ok((
                    entry.index,
                    text_authority::load_shard(state.path(), entry)?,
                ))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let text_authority = ShardedTextAuthority::from_proved_shards(shards)?;

        let searcher = TantivySearcher {
            source_coverage: None,
            source_publication_event: None,
            repo_id: RepoId::new("source-repo")?,
            revision_id: RevisionId::new("index-r1")?,
            generation,
            fields,
            reader,
            ranked_keys,
            repo_metadata: None,
            regex_match_cache: Arc::new(Mutex::new(RegexMatchCache::new(
                RegexMatchCachePolicy::DEFAULT,
            ))),
            regex_policy: RegexPolicy::defaults(),
            execution_budget: LexicalExecutionBudgetV1::DEFAULT,
            text_authority: Some(text_authority),
            repo_commit_recency: None,
            repo_meta: None,
            repo_topic: None,
            repo_description: None,
            file_ownership: None,
            file_contributor: None,
            resident_bytes_estimate: 0,
            artifact_identity: LexicalArtifactIdentityV1 {
                manifest_digest: "test-generation".into(),
                normalizer: TextNormalizerVersionV1 {
                    major: crate::normalize::TEXT_NORMALIZER_VERSION.major,
                    minor: crate::normalize::TEXT_NORMALIZER_VERSION.minor,
                },
                repo_metadata: RepoMetadataAuthoritiesV1::NONE,
            },
        };
        for index_mode in [None, Some(LqYesNoOnly::No)] {
            let query = LqQuery {
                lq_version: LQ_VERSION_TAG,
                expr: LqExpr::Leaf(LqLeaf::Keyword(raw.into())),
                filters: Vec::new(),
                options: LqOptions {
                    index_mode,
                    ..LqOptions::defaults()
                },
                directives: Vec::new(),
                source_span: LqSpan::eof(0),
            };
            let request = RequestBudgetV1::unbounded();
            let preview_budget = request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
            preview_budget.charge_work(10_000_000)?;
            assert!(matches!(
                searcher.search(&query, 1, &request),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
                    ..
                })
            ));
        }
        Ok(())
    }

    #[test]
    fn optional_refusal_cannot_hide_a_selected_source_chunk_digest_mismatch()
    -> Result<(), CoreError> {
        let request = RequestBudgetV1::unbounded();
        let raw = "needle";
        let expected: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
        for reason in [
            PreviewUnavailableReason::WorkBudget,
            PreviewUnavailableReason::UnsupportedRange,
        ] {
            assert!(matches!(
                unavailable_selected_preview(
                    PreviewKind::SourceChunk,
                    reason,
                    raw,
                    Some([0; 32]),
                    &request,
                ),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
                    ..
                })
            ));
            assert!(matches!(
                unavailable_selected_preview(PreviewKind::SourceChunk, reason, raw, None, &request),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
                    ..
                })
            ));
            let preview = unavailable_selected_preview(
                PreviewKind::SourceChunk,
                reason,
                raw,
                Some(expected),
                &request,
            )?;
            assert_eq!(preview.unavailable_reason, Some(reason));
        }
        Ok(())
    }

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

    #[test]
    fn ranked_line_fields_refuse_ambiguous_or_malformed_storage() -> Result<(), CoreError> {
        let mut builder = Schema::builder();
        let line = builder.add_u64_field("start_line", STORED);
        let _schema = builder.build();
        assert_eq!(stored_u32(&TantivyDocument::new(), line)?, None);

        let mut malformed = TantivyDocument::new();
        malformed.add_text(line, "7");
        assert!(stored_u32(&malformed, line).is_err());

        let mut duplicate = TantivyDocument::new();
        duplicate.add_u64(line, 7);
        duplicate.add_u64(line, 8);
        assert!(stored_u32(&duplicate, line).is_err());

        let mut overflow = TantivyDocument::new();
        overflow.add_u64(line, u64::from(u32::MAX) + 1);
        assert!(stored_u32(&overflow, line).is_err());

        let mut valid = TantivyDocument::new();
        valid.add_u64(line, 7);
        assert_eq!(stored_u32(&valid, line)?, Some(7));
        Ok(())
    }
}
