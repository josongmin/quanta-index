//! Lowering a relevance text expression onto one kind's index.
//!
//! Only what has a BM25 score is admitted: keyword and phrase leaves,
//! combined with `All` (every clause must match; scores add) and `Any`
//! (at least one clause must match; the matching clauses' scores add),
//! and `Not` only as a clause of an `All` that has a positive sibling
//! (the negated documents are excluded; nothing is scored by absence).
//! A raw string, a regex, a predicate, a structural block, a bare or
//! `Any`-side negation, an `All`/`Any` with no positive clause, or an
//! empty expression is refused [`HISTORY_TEXT_QUERY_UNSCORABLE_CODE`]:
//! the recency order runs those as filters, relevance never invents a
//! score for them.
//!
//! A keyword with several tokens is a phrase, as on the lexical route
//! (`standard` mode: adjacency is strict). Case follows the query's
//! `case:` option; absent means folded, the DSL default.

use quanta_index_contract::{LqCase, LqExpr, LqLeaf, LqOptions, LqPatternType};
use quanta_index_core::{CoreError, HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextQueryV1};
use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};

use crate::history_text_index::schema::KindSchema;
use crate::normalize::{self, CaseMode, TextQueryError, Token};

/// Typed refusal code for a literal without a token.
///
/// The same code the corpus route answers with for the same literal
/// (`LEX_TEXT_QUERY_NO_TOKENS` in the adapter root), since both lower
/// through one tokenizer.
const LEX_TEXT_QUERY_NO_TOKENS: &str = "LEX_TEXT_QUERY_NO_TOKENS";
/// Typed refusal code for a literal with a run past the term cap; the
/// corpus route's `LEX_TEXT_QUERY_TOKEN_TOO_LONG`.
const LEX_TEXT_QUERY_TOKEN_TOO_LONG: &str = "LEX_TEXT_QUERY_TOKEN_TOO_LONG";

fn unscorable(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: HISTORY_TEXT_QUERY_UNSCORABLE_CODE.to_string(),
        message: format!("history relevance: {}", message.into()),
    }
}

fn map_text_query_error(err: &TextQueryError) -> CoreError {
    let code = match err {
        TextQueryError::NoTokens => LEX_TEXT_QUERY_NO_TOKENS,
        TextQueryError::TokenTooLong { .. } => LEX_TEXT_QUERY_TOKEN_TOO_LONG,
    };
    CoreError::Typed {
        code: code.to_string(),
        message: format!("history relevance: {err}"),
    }
}

/// The case mode a query's options select.
#[must_use]
pub(super) fn case_mode(options: &LqOptions) -> CaseMode {
    CaseMode::from_case_sensitive(matches!(options.case, Some(LqCase::Sensitive)))
}

/// One clause of a boolean query: how it occurs and what it is.
struct Clause {
    occur: Occur,
    query: Box<dyn Query>,
}

/// Compile `query` against `schema`.
pub(super) fn compile(
    schema: &KindSchema,
    query: &HistoryTextQueryV1,
) -> Result<Box<dyn Query>, CoreError> {
    match query.options.pattern_type {
        LqPatternType::Standard | LqPatternType::Keyword => {}
        LqPatternType::Literal | LqPatternType::Regexp | LqPatternType::Structural => {
            return Err(unscorable(format!(
                "pattern type `{}` has no BM25 score; use `order: recency` for it",
                query.options.pattern_type.as_str()
            )));
        }
    }
    let field = schema.text_field(case_mode(&query.options));
    let case = case_mode(&query.options);
    compile_positive(&query.expr, field, case)
}

/// Compile an expression that must contribute a positive (scored) match.
fn compile_positive(
    expr: &LqExpr,
    field: Field,
    case: CaseMode,
) -> Result<Box<dyn Query>, CoreError> {
    match expr {
        LqExpr::Empty => Err(unscorable(
            "relevance order requires a text expression to score; the query has none",
        )),
        LqExpr::Leaf(leaf) => compile_leaf(leaf, field, case),
        LqExpr::Not(_) => Err(unscorable(
            "a negation can only be scored beside a positive clause it excludes from (`a AND NOT b`)",
        )),
        LqExpr::All(children) => {
            let mut clauses = Vec::with_capacity(children.len());
            for child in children {
                clauses.push(match child {
                    LqExpr::Not(inner) => Clause {
                        occur: Occur::MustNot,
                        query: compile_positive(inner, field, case)?,
                    },
                    LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::All(_) | LqExpr::Any(_) => Clause {
                        occur: Occur::Must,
                        query: compile_positive(child, field, case)?,
                    },
                });
            }
            if !clauses.iter().any(|clause| clause.occur == Occur::Must) {
                return Err(unscorable(
                    "a conjunction needs at least one positive clause to score",
                ));
            }
            Ok(boolean(clauses))
        }
        LqExpr::Any(children) => {
            if children.is_empty() {
                return Err(unscorable("an empty disjunction has nothing to score"));
            }
            let mut clauses = Vec::with_capacity(children.len());
            for child in children {
                clauses.push(Clause {
                    occur: Occur::Should,
                    query: compile_positive(child, field, case)?,
                });
            }
            Ok(boolean(clauses))
        }
    }
}

fn boolean(clauses: Vec<Clause>) -> Box<dyn Query> {
    Box::new(BooleanQuery::new(
        clauses
            .into_iter()
            .map(|clause| (clause.occur, clause.query))
            .collect(),
    ))
}

fn compile_leaf(leaf: &LqLeaf, field: Field, case: CaseMode) -> Result<Box<dyn Query>, CoreError> {
    match leaf {
        LqLeaf::Keyword(text) | LqLeaf::Phrase(text) => {
            let tokens =
                normalize::query_tokens(text, case).map_err(|err| map_text_query_error(&err))?;
            token_sequence_query(field, &tokens)
        }
        LqLeaf::RawString(_) => Err(unscorable(
            "a raw string has substring semantics and no BM25 score; use a keyword or `order: recency`",
        )),
        LqLeaf::Regex(_) => Err(unscorable(
            "a regex has no BM25 score; use a keyword or `order: recency`",
        )),
        LqLeaf::StructuralBlock(_) => Err(unscorable(
            "a structural block is not executable on the history route",
        )),
        LqLeaf::Predicate { name, .. } => Err(unscorable(format!(
            "predicate `{name}` is not executable on the history route"
        ))),
    }
}

/// A term query for one token, a phrase query for a sequence.
///
/// `tokens` is a [`normalize::query_tokens`] result and so never empty;
/// an empty sequence is still answered typed rather than handed to the
/// engine, which would panic on it.
fn token_sequence_query(field: Field, tokens: &[Token]) -> Result<Box<dyn Query>, CoreError> {
    let terms: Vec<Term> = tokens
        .iter()
        .map(|token| Term::from_field_text(field, &token.text))
        .collect();
    match terms.as_slice() {
        [] => Err(map_text_query_error(&TextQueryError::NoTokens)),
        [single] => Ok(Box::new(TermQuery::new(
            single.clone(),
            IndexRecordOption::WithFreqs,
        ))),
        _ => Ok(Box::new(PhraseQuery::new(terms))),
    }
}
