//! Allocation-free capability and shape gate for native Content/BM25 planning.
//!
//! The general lexical planner also supports regex, substring and other
//! engines. This gate runs before it on the native text route so unsupported
//! leaves cannot allocate their plans before the route refuses them.

use quanta_index_contract::{LqCountBound, LqExpr, LqFilter, LqLeaf, LqPatternType, LqQuery};
use quanta_index_core::index::IndexError;
use quanta_index_lq_text_normalizer::is_token_char;

const MAX_QUERY_NODES: usize = 256;
const MAX_QUERY_DEPTH: usize = 32;
const MAX_CONTENT_TERMS: usize = 64;
const MAX_LITERAL_BYTES: usize = 4_096;
const MAX_TOTAL_LITERAL_BYTES: usize = 16 * 1_024;
const MAX_REGEX_LEAVES: usize = super::text_regex::MAX_NATIVE_CONTENT_REGEX;
const MAX_PATH_FILTERS: usize = super::text_regex::MAX_NATIVE_PATH_FILTERS;

#[derive(Default)]
struct Shape {
    nodes: usize,
    terms: usize,
    literal_bytes: usize,
    regexes: usize,
    path_regexes: usize,
}

impl Shape {
    fn visit(&mut self, expr: &LqExpr, depth: usize) -> Result<bool, IndexError> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if depth > MAX_QUERY_DEPTH || self.nodes > MAX_QUERY_NODES {
            return Err(IndexError::InvalidInput(
                "native BM25 boolean complexity limit".into(),
            ));
        }
        match expr {
            LqExpr::Leaf(LqLeaf::Keyword(text)) => {
                self.literal(text)?;
                let runs = token_runs(text)?;
                if runs == 0 {
                    return Err(IndexError::InvalidInput(
                        quanta_index_lq_text_normalizer::TextQueryError::NoTokens.to_string(),
                    ));
                }
                self.add_terms(runs)?;
                Ok(true)
            }
            LqExpr::Leaf(LqLeaf::Phrase(text)) => {
                self.literal(text)?;
                let runs = token_runs(text)?;
                if runs == 0 {
                    return Err(IndexError::InvalidInput(
                        "native Content phrase has no terms".into(),
                    ));
                }
                self.add_terms(runs)?;
                Ok(true)
            }
            LqExpr::Leaf(LqLeaf::Regex(source)) => {
                self.literal(source)?;
                self.regexes = self
                    .regexes
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.regexes > MAX_REGEX_LEAVES {
                    return Err(IndexError::ResourceLimited);
                }
                Ok(false)
            }
            LqExpr::Leaf(_) => Err(IndexError::InvalidInput(
                "native BM25 requires Content keywords or phrases".into(),
            )),
            LqExpr::Not(child) => {
                let child_depth = depth.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let _child_anchor = self.visit(child, child_depth)?;
                Ok(false)
            }
            LqExpr::All(children) | LqExpr::Any(children) => {
                if children.is_empty()
                    || children.len() > MAX_QUERY_NODES.saturating_sub(self.nodes)
                {
                    return Err(IndexError::InvalidInput(
                        "native BM25 boolean complexity limit".into(),
                    ));
                }
                let all = matches!(expr, LqExpr::All(_));
                let mut anchored = !all;
                for child in children {
                    let child_depth = depth.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    let child_anchor = self.visit(child, child_depth)?;
                    anchored = if all {
                        anchored || child_anchor
                    } else {
                        anchored && child_anchor
                    };
                }
                Ok(anchored)
            }
            LqExpr::Empty => Err(IndexError::InvalidInput(
                "native BM25 requires a Content clause".into(),
            )),
        }
    }

    fn literal(&mut self, text: &str) -> Result<(), IndexError> {
        if text.len() > MAX_LITERAL_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        self.literal_bytes = self
            .literal_bytes
            .checked_add(text.len())
            .ok_or(IndexError::ResourceLimited)?;
        if self.literal_bytes > MAX_TOTAL_LITERAL_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        Ok(())
    }

    fn add_terms(&mut self, count: usize) -> Result<(), IndexError> {
        self.terms = self
            .terms
            .checked_add(count)
            .ok_or(IndexError::ResourceLimited)?;
        if self.terms > MAX_CONTENT_TERMS {
            return Err(IndexError::InvalidInput(
                "native query exceeds 64 term occurrences".into(),
            ));
        }
        Ok(())
    }

    fn plan_backing_bytes(&self) -> Result<u64, IndexError> {
        // Logical reservation for the two plan trees, child vectors, trace
        // strings, phrase tokens and temporary normalization. The fixed node
        // allowance also covers the planner's short-lived engine sets. This
        // is a route admission policy, not a physical RSS/allocator ceiling.
        let bytes = self
            .nodes
            .checked_mul(4_096)
            .and_then(|total| {
                self.literal_bytes
                    .checked_mul(32)
                    .and_then(|strings| total.checked_add(strings))
            })
            .and_then(|total| {
                self.terms
                    .checked_mul(256)
                    .and_then(|tokens| total.checked_add(tokens))
            })
            .and_then(|total| {
                self.regexes
                    .checked_add(self.path_regexes)?
                    .checked_mul(
                        match usize::try_from(quanta_index_lq_regex::MAX_NFA_STATES) {
                            Ok(value) => value,
                            Err(_overflow) => return None,
                        }
                        .checked_mul(32)?,
                    )
                    .and_then(|regex| total.checked_add(regex))
            })
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }
}

fn token_runs(text: &str) -> Result<usize, IndexError> {
    // NFC and per-character case conversion preserve token-run boundaries.
    // Admission counts the shared tokenizer's runs before any plan allocation.
    let mut in_run = false;
    let mut runs = 0usize;
    for ch in text.chars() {
        let token = is_token_char(ch);
        if token && !in_run {
            runs = runs.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        in_run = token;
    }
    Ok(runs)
}

/// Refuse unsupported native capabilities before the general planner allocates.
/// Returns the logical plan-backing reservation to acquire before planning.
pub fn validate_native_text_input(query: &LqQuery) -> Result<u64, IndexError> {
    if !query.directives.is_empty()
        || !matches!(
            query.options.pattern_type,
            LqPatternType::Standard | LqPatternType::Keyword
        )
        || query.options.timeout_ms.is_some()
        || query.options.index_mode.is_some()
        || query.options.boost_millis.is_some()
        || matches!(
            query.options.count,
            Some(LqCountBound::All | LqCountBound::Bounded(0))
        )
    {
        return Err(IndexError::InvalidInput(
            "native BM25 query options or filters are unsupported".into(),
        ));
    }
    let mut shape = Shape::default();
    let mut path_filters = 0usize;
    for filter in &query.filters {
        let LqFilter::File { pattern, .. } = filter else {
            return Err(IndexError::InvalidInput(
                "native BM25 filter requires authoritative file path".into(),
            ));
        };
        path_filters = path_filters
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if path_filters > MAX_PATH_FILTERS {
            return Err(IndexError::ResourceLimited);
        }
        shape.literal(pattern)?;
    }
    shape.path_regexes = path_filters;
    // The selected member's admitted document count is available only after
    // planning and pinning. Keep the shape cap here; search_content refuses an
    // unanchored query above its finite-universe bound before scanning pages.
    let _anchored = shape.visit(&query.expr, 0)?;
    shape.plan_backing_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{LqFileScope, LqFilter, LqSpan};

    fn query(expr: LqExpr) -> LqQuery {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = expr;
        query
    }

    #[test]
    fn keyword_occurrences_share_phrase_admission_and_plan_backing() {
        for text in ["row-0", "one.one", "one_one", "CAFÉ-ΟΔΟΣ", "one-one-one"] {
            let keyword = query(LqExpr::Leaf(LqLeaf::Keyword(text.into())));
            let phrase = query(LqExpr::Leaf(LqLeaf::Phrase(text.into())));
            assert_eq!(
                validate_native_text_input(&keyword),
                validate_native_text_input(&phrase)
            );
        }
        let one = query(LqExpr::Leaf(LqLeaf::Keyword("one_one".into())));
        let two = query(LqExpr::Leaf(LqLeaf::Keyword("one-one".into())));
        let one_bytes = validate_native_text_input(&one);
        let two_bytes = validate_native_text_input(&two);
        assert!(
            matches!((one_bytes, two_bytes), (Ok(one), Ok(two)) if one.checked_add(256) == Some(two))
        );
    }

    #[test]
    fn keyword_term_occurrences_refuse_zero_and_over_sixty_four_before_planning() {
        let sixty_four = std::iter::repeat_n("one", 64).collect::<Vec<_>>().join("-");
        let sixty_five = std::iter::repeat_n("one", 65).collect::<Vec<_>>().join("-");
        assert!(
            validate_native_text_input(&query(LqExpr::Leaf(LqLeaf::Keyword(sixty_four)))).is_ok()
        );
        for text in ["---".to_owned(), sixty_five] {
            assert!(matches!(
                validate_native_text_input(&query(LqExpr::Leaf(LqLeaf::Keyword(text)))),
                Err(IndexError::InvalidInput(_))
            ));
        }
        let sixty_three = std::iter::repeat_n("one", 63).collect::<Vec<_>>().join("-");
        let mut mixed = query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Keyword(sixty_three)),
            LqExpr::Leaf(LqLeaf::Keyword("last".into())),
        ]));
        assert!(validate_native_text_input(&mixed).is_ok());
        if let LqExpr::All(children) = &mut mixed.expr {
            children.push(LqExpr::Leaf(LqLeaf::Keyword("extra".into())));
        }
        assert!(matches!(
            validate_native_text_input(&mixed),
            Err(IndexError::InvalidInput(_))
        ));
    }

    #[test]
    fn admits_anchored_content_and_bounded_count_without_allocation() {
        let mut query = query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Phrase("alpha beta".into())),
            LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("gamma".into())))),
        ]));
        query.options.count = Some(LqCountBound::Bounded(1));
        assert!(validate_native_text_input(&query).is_ok());
    }

    #[test]
    fn admits_bounded_regex_and_rejects_unsupported_engine_leaf() {
        let regex_query = query(LqExpr::Leaf(LqLeaf::Regex("(a|b)*".into())));
        assert!(validate_native_text_input(&regex_query).is_ok());
        let raw = query(LqExpr::Leaf(LqLeaf::RawString("a".into())));
        assert!(matches!(
            validate_native_text_input(&raw),
            Err(IndexError::InvalidInput(_))
        ));
    }

    #[test]
    fn rejects_unsupported_filters_and_options_but_admits_unanchored_shape() {
        let mut filtered = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
        filtered.filters.push(LqFilter::Lang { id: "rs".into() });
        assert!(validate_native_text_input(&filtered).is_err());

        let mut unbounded_count = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
        unbounded_count.options.count = Some(LqCountBound::All);
        assert!(validate_native_text_input(&unbounded_count).is_err());

        let unanchored = query(LqExpr::Any(vec![
            LqExpr::Leaf(LqLeaf::Keyword("alpha".into())),
            LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("beta".into())))),
        ]));
        assert!(validate_native_text_input(&unanchored).is_ok());
        assert!(
            validate_native_text_input(&query(LqExpr::Not(Box::new(LqExpr::Leaf(
                LqLeaf::Keyword("alpha".into())
            )))))
            .is_ok()
        );
    }

    #[test]
    fn admits_bounded_authoritative_path_filters() {
        let mut selected = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
        selected.filters.push(LqFilter::File {
            pattern: "^src/.*\\.rs$".into(),
            scope: LqFileScope::PathOnly,
        });
        assert!(validate_native_text_input(&selected).is_ok());
        for _ in 0..MAX_PATH_FILTERS {
            selected.filters.push(LqFilter::File {
                pattern: "^main\\.rs$".into(),
                scope: LqFileScope::NameAndPath,
            });
        }
        assert!(matches!(
            validate_native_text_input(&selected),
            Err(IndexError::ResourceLimited)
        ));
    }

    #[test]
    fn rejects_constructed_oversize_ast_before_planner_allocation() {
        let huge = query(LqExpr::All(
            (0..MAX_QUERY_NODES)
                .map(|_| LqExpr::Leaf(LqLeaf::Keyword("x".into())))
                .collect(),
        ));
        assert!(matches!(
            validate_native_text_input(&huge),
            Err(IndexError::InvalidInput(_))
        ));
    }
}
