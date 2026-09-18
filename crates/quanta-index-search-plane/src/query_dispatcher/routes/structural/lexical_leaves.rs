//! Lexical sub-expression evaluation inside mixed structural boolean trees.

use quanta_index_contract::{
    ChunkRecord, LQ_VERSION_TAG, LexicalCandidate, LqExpr, LqLeaf, LqPatternType, LqQuery,
    SymbolCandidate,
};
use quanta_index_core::{
    CoreError, LexicalPolicy, LexicalSearcher, RequestBudgetV1, StructuralMatchCandidate,
};

use crate::query_dispatcher::routes::structural::buckets::{
    StructuralCandidateBuckets, normalize_structural_match_bucket,
};
use crate::query_dispatcher::routes::structural::read::StructuralRead;
use crate::readiness::StructuralAuthorityState;

/// Evaluates the lexical leaves of a mixed structural tree on the lexical
/// handle the query's read view pinned.
///
/// Symbol projections read the same structural snapshot as the rest of
/// the query (QI-BB-020 W2).
pub(super) struct LexicalSubexprEvaluator<'a> {
    pub(super) searcher: &'a dyn LexicalSearcher,
    pub(super) read: StructuralRead<'a>,
    pub(super) query: &'a LqQuery,
    pub(super) budget: &'a RequestBudgetV1,
}

impl LexicalSubexprEvaluator<'_> {
    pub(super) fn evaluate(&self, expr: &LqExpr) -> Result<StructuralCandidateBuckets, CoreError> {
        let mut options = self.query.options.clone();
        if options.pattern_type == LqPatternType::Structural {
            options.pattern_type = LqPatternType::Standard;
        }
        let subquery = LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: expr.clone(),
            filters: self.query.filters.clone(),
            directives: self.query.directives.clone(),
            options,
            source_span: self.query.source_span,
        };
        LexicalPolicy::validate_query(&subquery)?;
        // Every lexical leaf of a structural expression is its own native
        // search; a boolean tree can hold many, so each one is a checkpoint.
        self.budget.checkpoint("structural:lexical-leaf")?;
        if symbol_name_predicate_leaf(expr) {
            let results = self.searcher.search_symbols_all(&subquery, self.budget)?;
            return Ok(symbol_hits_to_structural_buckets(results, self.read.state));
        }
        let results = self.searcher.search_all(&subquery, self.budget)?;
        Ok(lexical_hits_to_structural_buckets(results))
    }
}

fn symbol_name_predicate_leaf(expr: &LqExpr) -> bool {
    matches!(
        expr,
        LqExpr::Leaf(LqLeaf::Predicate { name, .. }) if name == "symbol.has.name"
    )
}

fn lexical_hits_to_structural_buckets(
    results: Vec<LexicalCandidate>,
) -> StructuralCandidateBuckets {
    let mut buckets = StructuralCandidateBuckets::new();
    for hit in results {
        buckets
            .entry(hit.candidate_id.clone())
            .or_default()
            .push(StructuralMatchCandidate {
                candidate_id: hit.candidate_id,
                pattern_start_byte: 0,
                pattern_end_byte: 0,
                bindings: Vec::new(),
            });
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    buckets
}

pub(crate) fn symbol_hits_to_structural_buckets(
    results: Vec<SymbolCandidate>,
    structural_state: &StructuralAuthorityState,
) -> StructuralCandidateBuckets {
    let mut buckets = StructuralCandidateBuckets::new();
    for hit in results {
        for (chunk_id, chunk) in structural_state.chunks() {
            if chunk.repo_relative_path != hit.repo_relative_path {
                continue;
            }
            if !symbol_span_overlaps_chunk_lines(&hit, chunk) {
                continue;
            }
            let candidate_id = chunk_id.as_str().to_string();
            buckets
                .entry(candidate_id.clone())
                .or_default()
                .push(StructuralMatchCandidate {
                    candidate_id,
                    pattern_start_byte: chunk.start_byte,
                    pattern_end_byte: chunk.end_byte,
                    bindings: Vec::new(),
                });
        }
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    buckets
}

fn symbol_span_overlaps_chunk_lines(hit: &SymbolCandidate, chunk: &ChunkRecord) -> bool {
    let hit_start = hit.start_line.max(1);
    let hit_end = hit.end_line.max(hit_start);
    let chunk_start = chunk.start_line.max(1);
    let chunk_end = chunk.end_line.max(chunk_start);
    hit_start <= chunk_end && hit_end >= chunk_start
}
