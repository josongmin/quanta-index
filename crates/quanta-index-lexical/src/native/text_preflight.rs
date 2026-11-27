//! Allocation-free capability and shape gate for native Content/BM25 planning.
//!
//! The general lexical planner also supports regex, substring and other
//! engines. This gate runs before it on the native text route so unsupported
//! leaves cannot allocate their plans before the route refuses them.

use crate::plan::file_contains_raw_literal_supported;
use quanta_index_contract::{
    LqCountBound, LqExpr, LqFilter, LqLeaf, LqPatternType, LqPredicateArg, LqQuery, LqSelect,
};
use quanta_index_core::index::IndexError;
use quanta_index_lq_text_normalizer::is_token_char;
use quanta_index_lq_trigram::NecessaryGramExpr;

const MAX_QUERY_NODES: usize = 256;
const MAX_QUERY_DEPTH: usize = 32;
const MAX_CONTENT_TERMS: usize = 64;
const MAX_LITERAL_BYTES: usize = 4_096;
const MAX_TOTAL_LITERAL_BYTES: usize = 16 * 1_024;
const MAX_REGEX_LEAVES: usize = quanta_index_contract::MAX_NATIVE_CONTENT_REGEX_LEAVES;
// The contract owns these fan-out limits so clients refuse before any I/O.
pub(super) use quanta_index_contract::{
    MAX_NATIVE_FILE_CONTAINS_LEAVES as MAX_FILE_CONTAINS_LEAVES,
    MAX_NATIVE_PATH_CONTAINS_LEAVES as MAX_PATH_CONTAINS_LEAVES,
    MAX_NATIVE_PATH_FILTERS as MAX_PATH_FILTERS,
    MAX_NATIVE_PATH_REGEX_LEAVES as MAX_PATH_REGEX_LEAVES,
};
const MAX_REPO_FILTERS: usize = super::text_regex::MAX_NATIVE_REPO_FILTERS;

/// The plan-backing grants the native text route acquires around planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeTextPlanBacking {
    /// Held while the plan lives: the plan trees, child vectors, strings and
    /// phrase tokens, and each regex plan's necessary-gram condition, which
    /// the planner caps at `MAX_NECESSARY_GRAM_NODES` nodes.
    pub retained: u64,
    /// Held only while the planner runs: the regex HIR and the short-lived
    /// engine sets it builds for each regex, path, source and repository
    /// predicate, and drops before the plan is returned.
    pub planning: u64,
}

#[derive(Default)]
struct Shape {
    nodes: usize,
    terms: usize,
    literal_bytes: usize,
    regexes: usize,
    file_contains: usize,
    path_contains: usize,
    repo_has_file: usize,
    path_regexes: usize,
    path_regex_leaves: usize,
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
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) if name == "file.contains" => {
                let [LqPredicateArg::RawString(needle)] = args.as_slice() else {
                    return Err(IndexError::InvalidInput(
                        "native file.contains requires one raw string".into(),
                    ));
                };
                if !file_contains_raw_literal_supported(needle) {
                    return Err(IndexError::InvalidInput(
                        "native file.contains requires a bounded literal raw substring".into(),
                    ));
                }
                self.literal(needle)?;
                self.file_contains = self
                    .file_contains
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.file_contains > MAX_FILE_CONTAINS_LEAVES {
                    return Err(IndexError::ResourceLimited);
                }
                Ok(false)
            }
            // Decided from Source path metadata; its exact matching-path
            // census bounds the candidates, so it anchors like a term.
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) if name == "path.contains" => {
                let [LqPredicateArg::RawString(needle)] = args.as_slice() else {
                    return Err(IndexError::InvalidInput(
                        "native path.contains requires one raw string".into(),
                    ));
                };
                if !file_contains_raw_literal_supported(needle) {
                    return Err(IndexError::InvalidInput(
                        "native path.contains requires a bounded literal raw substring".into(),
                    ));
                }
                self.literal(needle)?;
                self.path_contains = self
                    .path_contains
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.path_contains > MAX_PATH_CONTAINS_LEAVES {
                    return Err(IndexError::ResourceLimited);
                }
                Ok(true)
            }
            // A regex over Source path metadata: its exact census bounds the
            // candidates like `path.contains`.
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) if name == "path.regex" => {
                let [LqPredicateArg::RawString(source)] = args.as_slice() else {
                    return Err(IndexError::InvalidInput(
                        "native path.regex requires one raw pattern".into(),
                    ));
                };
                if source.is_empty() {
                    return Err(IndexError::InvalidInput(
                        "native path.regex requires a pattern".into(),
                    ));
                }
                self.literal(source)?;
                self.path_regex_leaves = self
                    .path_regex_leaves
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.path_regex_leaves > MAX_PATH_REGEX_LEAVES {
                    return Err(IndexError::ResourceLimited);
                }
                Ok(true)
            }
            LqExpr::Leaf(LqLeaf::Predicate { name, args })
                if matches!(
                    name.as_str(),
                    "repo.has.file" | "repo.has.path" | "repo.contains.file" | "repo.contains.path"
                ) =>
            {
                if args.is_empty() || args.len() > 4 {
                    return Err(IndexError::InvalidInput(
                        "native repo.has.file requires bounded file matchers".into(),
                    ));
                }
                for arg in args {
                    match arg {
                        LqPredicateArg::Filter { name, value }
                            if matches!(name.as_str(), "path" | "name" | "content" | "lang")
                                && !value.trim().is_empty() =>
                        {
                            self.literal(value)?;
                        }
                        LqPredicateArg::Keyword(value)
                        | LqPredicateArg::Phrase(value)
                        | LqPredicateArg::RawString(value)
                            if args.len() == 1 && !value.trim().is_empty() =>
                        {
                            self.literal(value)?;
                        }
                        LqPredicateArg::Keyword(_)
                        | LqPredicateArg::Phrase(_)
                        | LqPredicateArg::RawString(_)
                        | LqPredicateArg::Number(_)
                        | LqPredicateArg::Filter { .. } => {
                            return Err(IndexError::InvalidInput(
                                "native repo.has.file has unsupported file matcher".into(),
                            ));
                        }
                    }
                }
                self.repo_has_file = self
                    .repo_has_file
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if self.repo_has_file > 4 {
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

    fn plan_backing(&self) -> Result<NativeTextPlanBacking, IndexError> {
        // Logical reservation for the two plan trees, child vectors, trace
        // strings, phrase tokens and temporary normalization. This is a route
        // admission policy, not a physical RSS/allocator ceiling.
        let structural = self
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
                    .checked_mul(quanta_index_lq_regex::MAX_NECESSARY_GRAM_NODES)?
                    .checked_mul(std::mem::size_of::<NecessaryGramExpr>())
                    .and_then(|grams| total.checked_add(grams))
            })
            .ok_or(IndexError::ResourceLimited)?;
        let engines = self
            .regexes
            .checked_add(self.path_regexes)
            .and_then(|engines| engines.checked_add(self.path_regex_leaves))
            .and_then(|engines| engines.checked_add(self.file_contains))
            .and_then(|engines| engines.checked_add(self.path_contains))
            .and_then(|engines| {
                self.repo_has_file
                    .checked_mul(4)
                    .and_then(|predicates| engines.checked_add(predicates))
            })
            .ok_or(IndexError::ResourceLimited)?;
        let planning = u64::try_from(engines)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(quanta_index_lq_regex::MAX_NFA_STATES)
            .and_then(|bytes| bytes.checked_mul(32))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(NativeTextPlanBacking {
            retained: u64::try_from(structural).map_err(|_error| IndexError::ResourceLimited)?,
            planning,
        })
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
/// Returns the plan-backing grants to acquire before planning.
pub fn validate_native_text_input(query: &LqQuery) -> Result<NativeTextPlanBacking, IndexError> {
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
    let mut repo_filters = 0usize;
    let mut path_selection = false;
    for filter in &query.filters {
        match filter {
            // `repo:` and `lang:` are verified against authenticated Source
            // record metadata; see `text_regex::RecordMetadataFilters`.
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    return Err(IndexError::InvalidInput(
                        "native repo filter revisions require a history producer".into(),
                    ));
                }
                if pattern.is_empty() {
                    return Err(IndexError::InvalidInput(
                        "native repo filter requires a pattern".into(),
                    ));
                }
                repo_filters = repo_filters
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if repo_filters > MAX_REPO_FILTERS {
                    return Err(IndexError::ResourceLimited);
                }
                shape.literal(pattern)?;
            }
            // Language ids carry no engine; the literal byte budget bounds
            // their number. A blank id would be dropped by the planner, so it
            // is refused here rather than silently widening the filter.
            LqFilter::Lang { id } => {
                if id.trim().is_empty() {
                    return Err(IndexError::InvalidInput(
                        "native lang filter requires a language id".into(),
                    ));
                }
                shape.literal(id)?;
            }
            LqFilter::File { pattern, .. } => {
                path_filters = path_filters
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if path_filters > MAX_PATH_FILTERS {
                    return Err(IndexError::ResourceLimited);
                }
                shape.literal(pattern)?;
            }
            LqFilter::Select {
                dim: LqSelect::Path,
            } if !path_selection => {
                path_selection = true;
            }
            _ => {
                return Err(IndexError::InvalidInput(
                    "native BM25 filter requires authoritative file path".into(),
                ));
            }
        }
    }
    shape.path_regexes = path_filters
        .checked_add(repo_filters)
        .ok_or(IndexError::ResourceLimited)?;
    // The selected member's admitted document count is available only after
    // planning and pinning. Keep the shape cap here; search_content refuses an
    // unanchored query above its finite-universe bound before scanning pages.
    // An empty expression is a path-only query: its path filters supply
    // and bound the candidates from Source path metadata.
    if matches!(query.expr, LqExpr::Empty) {
        if path_filters == 0 {
            return Err(IndexError::InvalidInput(
                "native path-only query requires a path filter".into(),
            ));
        }
    } else {
        let _anchored = shape.visit(&query.expr, 0)?;
    }
    shape.plan_backing()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{LqFileScope, LqFilter, LqPredicateArg, LqSpan, LqType};

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
            matches!((one_bytes, two_bytes), (Ok(one), Ok(two)) if one.retained.checked_add(256) == Some(two.retained))
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
    fn explicit_path_selection_preserves_file_filter_and_refuses_other_surfaces() {
        let mut selected = query(LqExpr::Leaf(LqLeaf::Keyword("needle".into())));
        selected.filters.push(LqFilter::Select {
            dim: LqSelect::Path,
        });
        selected.filters.push(LqFilter::File {
            pattern: "src/*".into(),
            scope: LqFileScope::PathOnly,
        });
        assert!(validate_native_text_input(&selected).is_ok());

        selected.filters.push(LqFilter::Select {
            dim: LqSelect::Path,
        });
        assert!(matches!(
            validate_native_text_input(&selected),
            Err(IndexError::InvalidInput(_))
        ));
        assert!(matches!(
            selected.filters.pop(),
            Some(LqFilter::Select {
                dim: LqSelect::Path
            })
        ));

        for unsupported in [
            LqFilter::Select {
                dim: LqSelect::Repo,
            },
            LqFilter::Type { kind: LqType::Path },
        ] {
            selected.filters.push(unsupported);
            assert!(matches!(
                validate_native_text_input(&selected),
                Err(IndexError::InvalidInput(_))
            ));
            assert!(selected.filters.pop().is_some());
        }
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
    fn file_contains_raw_substring_has_bounded_shape_without_admitting_other_predicates() {
        let file_contains = query(LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains".into(),
            args: vec![LqPredicateArg::RawString("oo_ba".into())],
        }));
        assert!(validate_native_text_input(&file_contains).is_ok());
        for name in ["file.unknown", "symbol.has.name"] {
            let other = query(LqExpr::Leaf(LqLeaf::Predicate {
                name: name.into(),
                args: vec![LqPredicateArg::RawString("oo_ba".into())],
            }));
            assert!(matches!(
                validate_native_text_input(&other),
                Err(IndexError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn repo_file_gate_admits_correlated_source_and_language_matchers() {
        let admitted = query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".into(),
                args: vec![
                    LqPredicateArg::Filter {
                        name: "path".into(),
                        value: "src/lib.rs".into(),
                    },
                    LqPredicateArg::Filter {
                        name: "name".into(),
                        value: "lib.rs".into(),
                    },
                    LqPredicateArg::Filter {
                        name: "content".into(),
                        value: "needle".into(),
                    },
                ],
            }),
        ]));
        assert!(validate_native_text_input(&admitted).is_ok());

        let language = query(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".into(),
            args: vec![LqPredicateArg::Filter {
                name: "lang".into(),
                value: "rust".into(),
            }],
        }));
        assert!(validate_native_text_input(&language).is_ok());
        let empty = query(LqExpr::Leaf(LqLeaf::Predicate {
            name: "repo.has.file".into(),
            args: vec![LqPredicateArg::Filter {
                name: "lang".into(),
                value: "  ".into(),
            }],
        }));
        assert!(matches!(
            validate_native_text_input(&empty),
            Err(IndexError::InvalidInput(_))
        ));
    }

    #[test]
    fn file_contains_refuses_regex_spelling_and_unavailable_scopes() {
        for args in [
            vec![LqPredicateArg::RawString("/foo.*/".into())],
            vec![LqPredicateArg::RawString(String::new())],
            vec![LqPredicateArg::Keyword("foo".into())],
            vec![
                LqPredicateArg::RawString("foo".into()),
                LqPredicateArg::Filter {
                    name: "path".into(),
                    value: "src/lib.rs".into(),
                },
            ],
        ] {
            let unsupported = query(LqExpr::Leaf(LqLeaf::Predicate {
                name: "file.contains".into(),
                args,
            }));
            assert!(matches!(
                validate_native_text_input(&unsupported),
                Err(IndexError::InvalidInput(_))
            ));
        }
        let alias = query(LqExpr::Leaf(LqLeaf::Predicate {
            name: "file.contains.content".into(),
            args: vec![LqPredicateArg::RawString("foo".into())],
        }));
        assert!(matches!(
            validate_native_text_input(&alias),
            Err(IndexError::InvalidInput(_))
        ));
        for literal in ["oo", "/"] {
            let short_literal = query(LqExpr::Leaf(LqLeaf::Predicate {
                name: "file.contains".into(),
                args: vec![LqPredicateArg::RawString(literal.into())],
            }));
            assert!(validate_native_text_input(&short_literal).is_ok());
        }
    }

    #[test]
    fn rejects_unsupported_filters_and_options_but_admits_unanchored_shape() {
        let mut filtered = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
        filtered.filters.push(LqFilter::Rev {
            spec: "main".into(),
        });
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
    fn admits_path_contains_and_path_only_shapes() {
        let path_leaf = |needle: &str| {
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "path.contains".into(),
                args: vec![LqPredicateArg::RawString(needle.into())],
            })
        };
        assert!(validate_native_text_input(&query(path_leaf("src/"))).is_ok());
        assert!(matches!(
            validate_native_text_input(&query(LqExpr::Leaf(LqLeaf::Predicate {
                name: "path.contains".into(),
                args: vec![LqPredicateArg::Keyword("src".into())],
            }))),
            Err(IndexError::InvalidInput(_))
        ));
        let many = query(LqExpr::Any(
            (0..=MAX_PATH_CONTAINS_LEAVES)
                .map(|_| path_leaf("src"))
                .collect(),
        ));
        assert!(matches!(
            validate_native_text_input(&many),
            Err(IndexError::ResourceLimited)
        ));
        let mut path_only = query(LqExpr::Empty);
        assert!(matches!(
            validate_native_text_input(&path_only),
            Err(IndexError::InvalidInput(_))
        ));
        path_only.filters.push(LqFilter::File {
            pattern: "^src/".into(),
            scope: LqFileScope::PathOnly,
        });
        assert!(validate_native_text_input(&path_only).is_ok());
    }

    #[test]
    fn admits_metadata_filters_and_refuses_unexecutable_shapes() {
        let mut filtered = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
        filtered.filters.push(LqFilter::Lang { id: "rust".into() });
        filtered.filters.push(LqFilter::Repo {
            pattern: "^org/alpha$".into(),
            revs: Vec::new(),
        });
        assert!(validate_native_text_input(&filtered).is_ok());
        for refused in [
            LqFilter::Lang { id: " ".into() },
            LqFilter::Repo {
                pattern: "alpha".into(),
                revs: vec!["main".into()],
            },
            LqFilter::Repo {
                pattern: String::new(),
                revs: Vec::new(),
            },
        ] {
            let mut shaped = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
            shaped.filters.push(refused);
            assert!(matches!(
                validate_native_text_input(&shaped),
                Err(IndexError::InvalidInput(_))
            ));
        }
        let mut many = query(LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));
        for _ in 0..=MAX_REPO_FILTERS {
            many.filters.push(LqFilter::Repo {
                pattern: "alpha".into(),
                revs: Vec::new(),
            });
        }
        assert!(matches!(
            validate_native_text_input(&many),
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
