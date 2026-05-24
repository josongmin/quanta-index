//! Production query engine wiring frozen contract requests to the lexical /
//! semantic adapters and the generation-pin port.
//!
//! Replaces the [`super::stub_engine::StubQueryEngine`] in the composition
//! root. The stub is retained behind `#[cfg(test)]` for orchestration tests
//! that don't need actual search execution.

use quanta_index_contract::{
    LexicalCandidate, LqExpr, LqFilter, LqFilterSet, LqQuery, ManifestGeneration,
    PublishedGenerationSet, RepoId, RepoRelativePath, RevisionId, SearchExplanation,
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse, SearchPlaneLexicalQueryRequest,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};
use quanta_index_control::ControlPlane;
use quanta_index_core::{
    CoreError, GenerationPinPort, QueryPolicy, SearchPlaneExplainQueryPort,
    SearchPlaneHybridQueryPort, SearchPlaneLexicalQueryPort, SearchPlaneSemanticQueryPort,
};
use quanta_index_lexical::TantivyLexicalAdapter;
use quanta_index_semantic::LanceSemanticAdapter;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, OwnedValue, Schema};
use tantivy::{Index, TantivyDocument, Term};

/// `(occur, query)` tuple used to assemble Tantivy `BooleanQuery` clauses.
type QueryClause = (Occur, Box<dyn Query>);

/// Embedding provider for semantic / hybrid queries.
///
/// Turns `query_text` into a vector. Phase 1 deployments without a configured
/// provider keep this at `None`; semantic queries return
/// `CoreError::NotImplemented` until a real provider is injected.
pub trait QueryEmbedder: Send + Sync {
    fn embed_query(&self, query_text: &str) -> Result<Vec<f32>, CoreError>;
}

/// Domain query engine. Holds borrowed references to the adapters and control
/// plane so callers can re-use the same engine across many requests.
pub struct DomainQueryEngine<'a> {
    pub lexical: &'a TantivyLexicalAdapter,
    pub semantic: &'a LanceSemanticAdapter,
    pub control: &'a ControlPlane,
    pub embedder: Option<&'a dyn QueryEmbedder>,
    pub default_top_k: usize,
}

impl DomainQueryEngine<'_> {
    /// Resolve the generation to pin against.
    ///
    /// Explicit request override wins; otherwise the active generation is
    /// read from the control plane using `repo` / `rev` filters lifted off
    /// the request.
    fn resolve_generation(
        &self,
        explicit: Option<&PublishedGenerationSet>,
        filters: &LqFilterSet,
    ) -> Result<PublishedGenerationSet, CoreError> {
        explicit.map_or_else(
            || {
                let (repo, rev) = filters_repo_rev(filters)?;
                let pin = self.control.pin_generation(&repo, &rev)?;
                pin.pinned_generation()?.map_or_else(
                    || {
                        Err(CoreError::NotReady(format!(
                            "no active generation for repo={} rev={}",
                            repo.as_str(),
                            rev.as_str(),
                        )))
                    },
                    Ok,
                )
            },
            |generation| Ok(generation.clone()),
        )
    }

    /// Execute the lexical path: validate, pin, parse + run a Tantivy query.
    fn execute_lexical(
        &self,
        query: &LqQuery,
        generation: &PublishedGenerationSet,
        top_k: usize,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        let index = self.lexical.open_index_for_query(generation)?;
        let reader = index
            .reader()
            .map_err(|error| CoreError::Storage(format!("tantivy reader: {error}")))?;
        let searcher = reader.searcher();
        let schema = index.schema();
        let fields = LexicalFields::from_schema(&schema)?;
        let tantivy_query =
            build_tantivy_query(&fields, &index, &query.expr, &query.filters, generation)?;
        let top_docs = searcher
            .search(tantivy_query.as_ref(), &TopDocs::with_limit(top_k))
            .map_err(|error| CoreError::Storage(format!("tantivy search: {error}")))?;
        let mut out = Vec::with_capacity(top_docs.len());
        for (score, address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| CoreError::Storage(format!("tantivy doc fetch: {error}")))?;
            out.push(doc_to_candidate(&fields, &doc, score)?);
        }
        Ok(out)
    }
}

impl SearchPlaneLexicalQueryPort for DomainQueryEngine<'_> {
    fn lexical_query(
        &self,
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        QueryPolicy::validate_query(&request.query)?;
        let generation =
            self.resolve_generation(request.generation.as_ref(), &request.query.filters)?;
        let top_k = request
            .query
            .options
            .limit
            .map_or(self.default_top_k, |raw| match usize::try_from(raw) {
                Ok(value) if value > 0 => value,
                _ => self.default_top_k,
            });
        let results = self.execute_lexical(&request.query, &generation, top_k)?;
        Ok(SearchPlaneLexicalQueryResponse {
            generation,
            results,
        })
    }
}

impl SearchPlaneSemanticQueryPort for DomainQueryEngine<'_> {
    fn semantic_query(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
        // Phase 1: semantic search requires a `QueryEmbedder` to turn the
        // textual query into a vector. Without one configured we fail-closed
        // rather than guess (no heuristic fallback to lexical-only).
        let Some(_embedder) = self.embedder else {
            return Err(CoreError::NotImplemented(
                "semantic_query: no QueryEmbedder configured for this deployment".into(),
            ));
        };
        // Even with an embedder, the production semantic path (ANN over Lance)
        // requires Lance ANN integration that lives outside this Phase 1 cut.
        // Keep the error reason precise so operators don't misread it.
        Err(CoreError::NotImplemented(format!(
            "semantic_query: ANN over Lance dataset for query_text={:?} not yet wired",
            request.query_text
        )))
    }
}

impl SearchPlaneHybridQueryPort for DomainQueryEngine<'_> {
    fn hybrid_query(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
        // Hybrid requires semantic. Until the embedding/ANN path is wired,
        // hybrid is fail-closed too — we deliberately do NOT fall back to
        // lexical-only because the contract response asserts "hybrid" which
        // implies a multi-signal score.
        if self.embedder.is_none() {
            return Err(CoreError::NotImplemented(
                "hybrid_query: requires a configured QueryEmbedder + Lance ANN, neither wired in Phase 1".into(),
            ));
        }
        Err(CoreError::NotImplemented(format!(
            "hybrid_query: ANN+lexical fusion not yet wired (query_text={:?})",
            request.semantic_query_text
        )))
    }
}

impl SearchPlaneExplainQueryPort for DomainQueryEngine<'_> {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        let generation = request.generation.clone();
        let summary = format!(
            "candidate={} score={:.4} repo={} rev={} gen={} path={} lines={}..{}",
            request.candidate.candidate_id,
            request.candidate.score,
            request.candidate.repo_id.as_str(),
            request.candidate.revision_id.as_str(),
            request.candidate.manifest_generation.get(),
            request.candidate.repo_relative_path.as_str(),
            request.candidate.start_line,
            request.candidate.end_line,
        );
        Ok(SearchPlaneExplainQueryResponse {
            generation,
            explanation: SearchExplanation { summary },
        })
    }
}

/// Field handles cached against a particular schema instance.
struct LexicalFields {
    candidate_id: Field,
    repo_id: Field,
    revision_id: Field,
    manifest_generation: Field,
    repo_relative_path: Field,
    start_line: Field,
    end_line: Field,
    text: Field,
}

impl LexicalFields {
    fn from_schema(schema: &Schema) -> Result<Self, CoreError> {
        let lookup = |name: &str| {
            schema
                .get_field(name)
                .map_err(|error| CoreError::Storage(format!("tantivy field {name}: {error}")))
        };
        Ok(Self {
            candidate_id: lookup("candidate_id")?,
            repo_id: lookup("repo_id")?,
            revision_id: lookup("revision_id")?,
            manifest_generation: lookup("manifest_generation")?,
            repo_relative_path: lookup("repo_relative_path")?,
            start_line: lookup("start_line")?,
            end_line: lookup("end_line")?,
            text: lookup("text")?,
        })
    }
}

/// Build a composite Tantivy query: text expression × structural filters ×
/// the active generation pin.
fn build_tantivy_query(
    fields: &LexicalFields,
    index: &Index,
    expr: &LqExpr,
    filters: &LqFilterSet,
    generation: &PublishedGenerationSet,
) -> Result<Box<dyn Query>, CoreError> {
    let mut clauses: Vec<QueryClause> = Vec::new();
    let text_query = build_text_query(fields, index, expr)?;
    clauses.push((Occur::Must, text_query));
    // Pin to (repo, rev, manifest_generation)
    clauses.push((
        Occur::Must,
        Box::new(TermQuery::new(
            Term::from_field_text(fields.repo_id, generation.repo_id.as_str()),
            IndexRecordOption::Basic,
        )),
    ));
    clauses.push((
        Occur::Must,
        Box::new(TermQuery::new(
            Term::from_field_text(fields.revision_id, generation.revision_id.as_str()),
            IndexRecordOption::Basic,
        )),
    ));
    clauses.push((
        Occur::Must,
        Box::new(TermQuery::new(
            Term::from_field_u64(
                fields.manifest_generation,
                generation.manifest_generation.get(),
            ),
            IndexRecordOption::Basic,
        )),
    ));
    // Translate any additional path/file filters into structural Tantivy terms.
    for filter in &filters.filters {
        if let Some(clause) = filter_to_clause(fields, filter)? {
            clauses.push(clause);
        }
    }
    Ok(Box::new(BooleanQuery::new(clauses)))
}

fn build_text_query(
    fields: &LexicalFields,
    index: &Index,
    expr: &LqExpr,
) -> Result<Box<dyn Query>, CoreError> {
    match expr {
        LqExpr::MatchAll => Err(CoreError::InvalidContract(
            "MatchAll not allowed on lexical path".into(),
        )),
        LqExpr::Raw(text) => {
            if text.trim().is_empty() {
                // Empty raw text → match every doc in the pinned generation.
                return Ok(Box::new(AllQuery));
            }
            let parser = QueryParser::for_index(index, vec![fields.text]);
            parser
                .parse_query(text)
                .map_err(|error| CoreError::InvalidContract(format!("query parse: {error}")))
        }
        LqExpr::All(inner) => combine(fields, index, inner, Occur::Must),
        LqExpr::Any(inner) => combine(fields, index, inner, Occur::Should),
        LqExpr::Not(inner) => {
            let positive = build_text_query(fields, index, inner)?;
            let negation: Vec<QueryClause> = vec![
                (Occur::Must, Box::new(AllQuery)),
                (Occur::MustNot, positive),
            ];
            Ok(Box::new(BooleanQuery::new(negation)))
        }
    }
}

fn combine(
    fields: &LexicalFields,
    index: &Index,
    inner: &[LqExpr],
    occur: Occur,
) -> Result<Box<dyn Query>, CoreError> {
    if inner.is_empty() {
        return Err(CoreError::InvalidContract(
            "All/Any LqExpr requires at least one child".into(),
        ));
    }
    let mut clauses: Vec<QueryClause> = Vec::with_capacity(inner.len());
    for child in inner {
        clauses.push((occur, build_text_query(fields, index, child)?));
    }
    Ok(Box::new(BooleanQuery::new(clauses)))
}

fn filter_to_clause(
    fields: &LexicalFields,
    filter: &LqFilter,
) -> Result<Option<QueryClause>, CoreError> {
    let clause: QueryClause = match filter {
        LqFilter::Repo(_) | LqFilter::Rev(_) => {
            // Already pinned to (repo, rev) via the active generation clause.
            return Ok(None);
        }
        LqFilter::Path(value) | LqFilter::File(value) => {
            let q: Box<dyn Query> = Box::new(TermQuery::new(
                Term::from_field_text(fields.repo_relative_path, value),
                IndexRecordOption::Basic,
            ));
            (Occur::Must, q)
        }
        LqFilter::Lang(_) | LqFilter::Select(_) | LqFilter::Type(_) | LqFilter::Custom { .. } => {
            // Phase 1: indexed schema has no lang/select/type/custom fields.
            // Surface the limitation rather than silently dropping the filter.
            return Err(CoreError::NotImplemented(format!(
                "LqFilter {filter:?} not supported by Phase 1 lexical schema"
            )));
        }
    };
    Ok(Some(clause))
}

/// Pull `repo` and `rev` selectors out of a filter set. Required when the
/// request did not carry an explicit generation pin.
fn filters_repo_rev(filters: &LqFilterSet) -> Result<(RepoId, RevisionId), CoreError> {
    let mut repo: Option<&str> = None;
    let mut rev: Option<&str> = None;
    for filter in &filters.filters {
        match filter {
            LqFilter::Repo(value) => repo = Some(value.as_str()),
            LqFilter::Rev(value) => rev = Some(value.as_str()),
            LqFilter::File(_)
            | LqFilter::Path(_)
            | LqFilter::Lang(_)
            | LqFilter::Select(_)
            | LqFilter::Type(_)
            | LqFilter::Custom { .. } => {}
        }
    }
    match (repo, rev) {
        (Some(r), Some(v)) => Ok((RepoId::new(r), RevisionId::new(v))),
        _ => Err(CoreError::InvalidContract(
            "lexical_query: request must carry an explicit generation or both repo/rev filters"
                .into(),
        )),
    }
}

fn doc_to_candidate(
    fields: &LexicalFields,
    doc: &TantivyDocument,
    score: f32,
) -> Result<LexicalCandidate, CoreError> {
    let candidate_id = first_text(doc, fields.candidate_id, "candidate_id")?;
    let repo = first_text(doc, fields.repo_id, "repo_id")?;
    let revision = first_text(doc, fields.revision_id, "revision_id")?;
    let manifest_generation = first_u64(doc, fields.manifest_generation, "manifest_generation")?;
    let path = first_text(doc, fields.repo_relative_path, "repo_relative_path")?;
    let start_u64 = first_u64(doc, fields.start_line, "start_line")?;
    let end_u64 = first_u64(doc, fields.end_line, "end_line")?;
    let start_line = u32::try_from(start_u64).map_err(|error| {
        CoreError::Storage(format!("start_line {start_u64} exceeds u32: {error}"))
    })?;
    let end_line = u32::try_from(end_u64)
        .map_err(|error| CoreError::Storage(format!("end_line {end_u64} exceeds u32: {error}")))?;
    // Snippet text is best-effort: if the stored doc omits the text field
    // (shouldn't happen with our schema, but be defensive against future
    // schema migrations) we surface an empty snippet rather than failing the
    // whole search response. `Result::unwrap_or_default` is denied via
    // workspace `disallowed-methods`, so use `Result::map_or_else` which is
    // allowed and expresses the same intent.
    #[expect(
        clippy::manual_unwrap_or_default,
        reason = "Result::unwrap_or_default is banned by clippy.toml disallowed-methods"
    )]
    let snippet = match first_text(doc, fields.text, "text") {
        Ok(text) => text,
        Err(_missing) => String::new(),
    };
    Ok(LexicalCandidate {
        candidate_id,
        repo_id: RepoId::new(repo),
        revision_id: RevisionId::new(revision),
        manifest_generation: ManifestGeneration::new(manifest_generation),
        repo_relative_path: RepoRelativePath::new(path),
        start_line,
        end_line,
        score,
        snippet,
    })
}

fn first_text(doc: &TantivyDocument, field: Field, name: &str) -> Result<String, CoreError> {
    doc.get_first(field)
        .and_then(owned_value_as_str)
        .map_or_else(
            || {
                Err(CoreError::Storage(format!(
                    "tantivy doc missing string field {name}"
                )))
            },
            Ok,
        )
}

fn first_u64(doc: &TantivyDocument, field: Field, name: &str) -> Result<u64, CoreError> {
    doc.get_first(field)
        .and_then(owned_value_as_u64)
        .map_or_else(
            || {
                Err(CoreError::Storage(format!(
                    "tantivy doc missing u64 field {name}"
                )))
            },
            Ok,
        )
}

/// Extract a `String` from a Tantivy stored value when the variant is `Str`.
///
/// Wildcard-match is explicit because `OwnedValue` has many non-text variants
/// (`U64`, `Date`, `Facet`, `Bytes`, …); enumerating them would not improve
/// behaviour — anything other than `Str` is "not a string".
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "intentional broad mismatch: anything non-Str is not a string"
)]
fn owned_value_as_str(value: &OwnedValue) -> Option<String> {
    match value {
        OwnedValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "intentional broad mismatch: anything non-U64 is not a u64"
)]
fn owned_value_as_u64(value: &OwnedValue) -> Option<u64> {
    match value {
        OwnedValue::U64(v) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::DomainQueryEngine;
    use quanta_index_contract::{
        BundleArtifactRef, BundleEncoding, GenerationId, LqDirectiveSet, LqExpr, LqFilter,
        LqFilterSet, LqOptionSet, LqQuery, ManifestDigest, ManifestGeneration,
        PublishedGenerationSet, PublishedSearchBundleManifest, RepoId, RevisionId,
        SearchExplanation, SearchPlaneExplainQueryRequest, SearchPlaneHybridQueryRequest,
        SearchPlaneLexicalQueryRequest, SearchPlaneSemanticQueryRequest,
    };
    use quanta_index_control::ControlPlane;
    use quanta_index_core::{
        CoreError, LexicalBuildInput, SearchPlaneExplainQueryPort, SearchPlaneHybridQueryPort,
        SearchPlaneLexicalIndexBuildPort, SearchPlaneLexicalQueryPort,
        SearchPlaneSemanticQueryPort,
    };
    use quanta_index_lexical::TantivyLexicalAdapter;
    use quanta_index_semantic::LanceSemanticAdapter;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    macro_rules! ok_or_fail {
        ($expr:expr, $msg:expr) => {
            match $expr {
                Ok(v) => v,
                Err(error) => {
                    assert!(false, "{}: {error}", $msg);
                    return;
                }
            }
        };
    }

    fn sample_generation() -> PublishedGenerationSet {
        PublishedGenerationSet {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            lexical_generation: GenerationId::new(10),
            symbol_generation: GenerationId::new(11),
            structural_generation: None,
            history_generation: None,
            semantic_generation: None,
            metadata_generation: None,
        }
    }

    fn hex_lower(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len().saturating_mul(2));
        for byte in bytes {
            let hi = usize::from(byte >> 4);
            let lo = usize::from(byte & 0x0f);
            out.push(char::from(HEX.get(hi).copied().unwrap_or(b'0')));
            out.push(char::from(HEX.get(lo).copied().unwrap_or(b'0')));
        }
        out
    }

    fn sample_manifest() -> PublishedSearchBundleManifest {
        // Fake artifact refs — query path doesn't read them, only the lexical
        // index needs to exist on disk for `open_index_for_query`.
        let mut hasher = Sha256::new();
        hasher.update(b"[]");
        let digest = hex_lower(&hasher.finalize());
        let chunk_ref = BundleArtifactRef {
            relative_path: "bundle/chunk.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 2,
            content_digest: ManifestDigest::new(digest.clone()),
        };
        let symbol_ref = BundleArtifactRef {
            relative_path: "bundle/symbol.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 2,
            content_digest: ManifestDigest::new(digest),
        };
        PublishedSearchBundleManifest {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(7),
            bundle_schema_version: 1,
            lexical_chunk_rows: chunk_ref,
            symbol_rows: symbol_ref,
            metadata_rows: None,
            graph_rows: None,
            embedding_input_views: None,
            embedding_records: None,
            mutation_delta: None,
        }
    }

    fn build_lexical_with_rows(
        state_root: &std::path::Path,
        chunk_rows_json: &[u8],
    ) -> (TantivyLexicalAdapter, PublishedSearchBundleManifest) {
        let mut adapter = TantivyLexicalAdapter::with_state_root(state_root.to_path_buf());
        let manifest = sample_manifest();
        let outcome = adapter.build_lexical_index(
            &manifest,
            LexicalBuildInput {
                chunk_rows: chunk_rows_json,
                symbol_rows: b"[]",
            },
        );
        assert!(outcome.is_ok(), "build_lexical: {outcome:?}");
        (adapter, manifest)
    }

    #[test]
    fn lexical_query_returns_not_ready_when_no_generation_in_request_or_pin() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let adapter = TantivyLexicalAdapter::with_state_root(dir.path().to_path_buf());
        let semantic = LanceSemanticAdapter::with_state_root(dir.path().to_path_buf());
        let control = ok_or_fail!(
            ControlPlane::open(&dir.path().join("c.sqlite3")),
            "open control"
        );

        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 5,
        };
        let request = SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw("hello".into()),
                filters: LqFilterSet {
                    filters: vec![LqFilter::Repo("repo".into()), LqFilter::Rev("rev".into())],
                },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet {
                    directives: Vec::new(),
                },
            },
            generation: None,
        };
        let result = engine.lexical_query(request);
        assert!(
            matches!(result, Err(CoreError::NotReady(_))),
            "expected NotReady (no active gen and no explicit), got {result:?}"
        );
    }

    #[test]
    fn lexical_query_rejects_match_all_expression() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let adapter = TantivyLexicalAdapter::with_state_root(dir.path().to_path_buf());
        let semantic = LanceSemanticAdapter::with_state_root(dir.path().to_path_buf());
        let control = ok_or_fail!(
            ControlPlane::open(&dir.path().join("c.sqlite3")),
            "open control"
        );

        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 5,
        };
        let request = SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::MatchAll,
                filters: LqFilterSet {
                    filters: Vec::new(),
                },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet {
                    directives: Vec::new(),
                },
            },
            generation: Some(sample_generation()),
        };
        let result = engine.lexical_query(request);
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "expected InvalidContract, got {result:?}"
        );
    }

    #[test]
    fn lexical_query_against_built_index_returns_expected_candidates() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let state_root = dir.path().to_path_buf();
        let chunk_rows = br#"[
            {"repo_relative_path":"src/lib.rs","start_line":1,"end_line":10,"text":"the brown fox jumps"},
            {"repo_relative_path":"src/main.rs","start_line":20,"end_line":30,"text":"hello world program"}
        ]"#;
        let (adapter, manifest) = build_lexical_with_rows(&state_root, chunk_rows);
        let semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
        let control = ok_or_fail!(
            ControlPlane::open(&state_root.join("c.sqlite3")),
            "open control"
        );
        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 5,
        };
        drop(manifest);
        let request = SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw("fox".into()),
                filters: LqFilterSet {
                    filters: Vec::new(),
                },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet {
                    directives: Vec::new(),
                },
            },
            generation: Some(sample_generation()),
        };
        let response = ok_or_fail!(engine.lexical_query(request), "lexical query");
        assert_eq!(response.generation, sample_generation());
        assert_eq!(
            response.results.len(),
            1,
            "expected one hit, got {:?}",
            response.results
        );
        let Some(hit) = response.results.first() else {
            assert!(false, "expected at least one hit");
            return;
        };
        assert_eq!(hit.repo_relative_path.as_str(), "src/lib.rs");
        assert!(hit.snippet.contains("fox"));
    }

    #[test]
    fn lexical_query_empty_raw_matches_all_pinned_docs() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let state_root = dir.path().to_path_buf();
        let chunk_rows = br#"[
            {"repo_relative_path":"a.rs","start_line":1,"end_line":2,"text":"alpha"},
            {"repo_relative_path":"b.rs","start_line":3,"end_line":4,"text":"beta"}
        ]"#;
        let (adapter, _) = build_lexical_with_rows(&state_root, chunk_rows);
        let semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
        let control = ok_or_fail!(
            ControlPlane::open(&state_root.join("c.sqlite3")),
            "open control"
        );
        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 10,
        };
        let request = SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw(String::new()),
                filters: LqFilterSet {
                    filters: Vec::new(),
                },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet {
                    directives: Vec::new(),
                },
            },
            generation: Some(sample_generation()),
        };
        let response = ok_or_fail!(engine.lexical_query(request), "lexical query");
        assert_eq!(
            response.results.len(),
            2,
            "expected both docs, got {:?}",
            response.results
        );
    }

    #[test]
    fn semantic_query_without_embedder_is_not_implemented() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let adapter = TantivyLexicalAdapter::with_state_root(dir.path().to_path_buf());
        let semantic = LanceSemanticAdapter::with_state_root(dir.path().to_path_buf());
        let control = ok_or_fail!(
            ControlPlane::open(&dir.path().join("c.sqlite3")),
            "open control"
        );
        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 5,
        };
        let request = SearchPlaneSemanticQueryRequest {
            query_text: "find me".into(),
            generation: Some(sample_generation()),
            lexical_filters: LqFilterSet {
                filters: Vec::new(),
            },
            top_k: 5,
        };
        let result = engine.semantic_query(request);
        assert!(
            matches!(result, Err(CoreError::NotImplemented(_))),
            "expected NotImplemented, got {result:?}"
        );
    }

    #[test]
    fn hybrid_query_without_embedder_is_not_implemented() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let adapter = TantivyLexicalAdapter::with_state_root(dir.path().to_path_buf());
        let semantic = LanceSemanticAdapter::with_state_root(dir.path().to_path_buf());
        let control = ok_or_fail!(
            ControlPlane::open(&dir.path().join("c.sqlite3")),
            "open control"
        );
        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 5,
        };
        let request = SearchPlaneHybridQueryRequest {
            lexical_query: LqQuery {
                expr: LqExpr::Raw("x".into()),
                filters: LqFilterSet {
                    filters: Vec::new(),
                },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet {
                    directives: Vec::new(),
                },
            },
            semantic_query_text: "y".into(),
            generation: Some(sample_generation()),
            top_k: 5,
        };
        let result = engine.hybrid_query(request);
        assert!(
            matches!(result, Err(CoreError::NotImplemented(_))),
            "expected NotImplemented, got {result:?}"
        );
    }

    #[test]
    fn explain_query_returns_non_empty_summary() {
        let dir = ok_or_fail!(tempdir(), "tempdir");
        let adapter = TantivyLexicalAdapter::with_state_root(dir.path().to_path_buf());
        let semantic = LanceSemanticAdapter::with_state_root(dir.path().to_path_buf());
        let control = ok_or_fail!(
            ControlPlane::open(&dir.path().join("c.sqlite3")),
            "open control"
        );
        let engine = DomainQueryEngine {
            lexical: &adapter,
            semantic: &semantic,
            control: &control,
            embedder: None,
            default_top_k: 5,
        };
        let request = SearchPlaneExplainQueryRequest {
            generation: sample_generation(),
            candidate: quanta_index_contract::LexicalCandidate {
                candidate_id: "cand-1".into(),
                repo_id: RepoId::new("repo"),
                revision_id: RevisionId::new("rev"),
                manifest_generation: ManifestGeneration::new(7),
                repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                start_line: 1,
                end_line: 10,
                score: 0.42,
                snippet: "fox jumps".into(),
            },
        };
        let response = ok_or_fail!(engine.explain_query(request), "explain");
        let SearchExplanation { summary } = response.explanation;
        assert!(summary.contains("cand-1"));
        assert!(summary.contains("0.4"));
        assert!(summary.contains("src/lib.rs"));
    }
}
