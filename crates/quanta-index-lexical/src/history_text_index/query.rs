//! Lowering a relevance text expression onto one kind's index.
//!
//! The index's job under `relevance` is to *enumerate and score*; the
//! search plane's row predicate (which re-evaluates the whole expression
//! with the same normalizer) decides membership, so the two orders answer
//! the same row set. The compiled query is therefore a sound
//! over-approximation of the expression — every row the expression
//! matches is a hit of the compiled query — scored by the keyword and
//! phrase leaves it contains:
//!
//! - a keyword or phrase leaf is a term or phrase query (exact);
//! - a raw string has substring semantics and no BM25 score: inside a
//!   conjunction it narrows the row set through the predicate and is
//!   dropped from the compiled query, and under a negation it is dropped
//!   the same way (`NOT 'x'` cannot exclude anything soundly, so the
//!   predicate excludes);
//! - `All` is a conjunction of its scorable children (scores add), with a
//!   negated child a `MustNot` of that child's *under*-approximation
//!   (excluding a superset would drop matches);
//! - `Any` is a disjunction of its children (the matching clauses' scores
//!   add), and is unconstrained as soon as one child is — a row could then
//!   match through a clause that has no score.
//!
//! An expression whose over-approximation is unconstrained (empty, a raw
//! string alone or as an alternative, a bare or `Any`-side negation, an
//! `All` with no positive scorable clause) has rows no scorable leaf
//! reaches, and relevance never invents a score for them: it is refused
//! [`quanta_index_core::HISTORY_TEXT_QUERY_UNSCORABLE_CODE`]. Regex, predicate and structural
//! leaves are refused the same way (the route refuses them earlier for
//! both orders). The recency order runs the same expression as a filter
//! and needs no score, so it serves what relevance refuses.
//!
//! A keyword with several tokens is a phrase, as on the lexical route
//! (`standard` mode: adjacency is strict). Case follows the query's
//! `case:` option through the DSL's one default
//! (`LqOptions::case_mode`).

use quanta_index_contract::{LqExpr, LqLeaf, LqPatternType};
use quanta_index_core::{CoreError, HistoryTextQueryV1};
use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};

use crate::history_text_index::schema::KindSchema;
use crate::normalize::{self, CaseMode, TextQueryError, Token};

fn unscorable(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextQueryUnscorable,
        message: format!("history relevance: {}", message.into()),
    }
}

fn map_text_query_error(err: &TextQueryError) -> CoreError {
    CoreError::Typed {
        code: match err {
            TextQueryError::NoTokens => {
                quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryNoTokens
            }
            TextQueryError::TokenTooLong { .. } => {
                quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryTokenTooLong
            }
        },
        message: format!("history relevance: {err}"),
    }
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
    let case = query.options.case_mode();
    let lowering = Lowering {
        field: schema.text_field(case),
        case,
    };
    match lowering.approximate(&query.expr, Side::Over)? {
        Approximation::Query(query) => Ok(query),
        Approximation::Everything => Err(unscorable(
            "relevance order scores keyword and phrase leaves, and every row on the page must be reached by one; this expression has rows no scorable leaf reaches (an empty expression, a raw string alone or as an alternative, or a negation with no positive clause beside it) — use `order: recency`, or put the raw string beside a keyword (`needle AND 'x'`)",
        )),
        Approximation::Nothing => Err(unscorable(
            "the expression can match no row (it contradicts itself); nothing to score",
        )),
        Approximation::Excluding(_) => Err(unscorable(
            "a negation can only be scored beside a positive clause it excludes from (`a AND NOT b`)",
        )),
    }
}

/// Which bound of the expression's row set is being computed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Side {
    /// A superset of the rows the expression matches.
    Over,
    /// A subset of the rows the expression matches.
    Under,
}

impl Side {
    const fn flipped(self) -> Self {
        match self {
            Self::Over => Self::Under,
            Self::Under => Self::Over,
        }
    }

    /// What an unscorable leaf contributes on this side.
    fn unscored(self) -> Approximation {
        match self {
            Self::Over => Approximation::Everything,
            Self::Under => Approximation::Nothing,
        }
    }
}

/// One bound of a sub-expression's row set.
enum Approximation {
    /// The rows this query hits.
    Query(Box<dyn Query>),
    /// Every row; carries no score.
    Everything,
    /// No row.
    Nothing,
    /// Every row the inner query does not hit; only expressible as a
    /// `MustNot` clause of a conjunction.
    Excluding(Box<dyn Query>),
}

impl Approximation {
    /// The bound of `NOT expr` on the other side, given this bound of
    /// `expr`.
    fn complement(self) -> Self {
        match self {
            Self::Query(query) => Self::Excluding(query),
            Self::Excluding(query) => Self::Query(query),
            Self::Everything => Self::Nothing,
            Self::Nothing => Self::Everything,
        }
    }
}

/// The field and case mode one kind's index is queried under.
#[derive(Clone, Copy)]
struct Lowering {
    field: Field,
    case: CaseMode,
}

impl Lowering {
    /// The `side` bound of the rows `expr` matches.
    fn approximate(self, expr: &LqExpr, side: Side) -> Result<Approximation, CoreError> {
        match expr {
            LqExpr::Empty => Ok(Approximation::Everything),
            LqExpr::Leaf(leaf) => self.leaf(leaf, side),
            LqExpr::Not(inner) => Ok(self.approximate(inner, side.flipped())?.complement()),
            LqExpr::All(children) => {
                let parts = children
                    .iter()
                    .map(|child| self.approximate(child, side))
                    .collect::<Result<Vec<_>, CoreError>>()?;
                Ok(conjunction(parts, side))
            }
            LqExpr::Any(children) => {
                let parts = children
                    .iter()
                    .map(|child| self.approximate(child, side))
                    .collect::<Result<Vec<_>, CoreError>>()?;
                Ok(disjunction(parts, side))
            }
        }
    }

    /// A keyword or phrase leaf is exact on both sides; a raw string is
    /// unscored (everything as a superset, nothing as a subset); the other
    /// leaves are refused.
    fn leaf(self, leaf: &LqLeaf, side: Side) -> Result<Approximation, CoreError> {
        match leaf {
            LqLeaf::Keyword(text) | LqLeaf::Phrase(text) => {
                let tokens = normalize::query_tokens(text, self.case)
                    .map_err(|err| map_text_query_error(&err))?;
                token_sequence_query(self.field, &tokens).map(Approximation::Query)
            }
            LqLeaf::RawString(_) => Ok(side.unscored()),
            LqLeaf::Regex(_) => {
                Err(unscorable("a regex has no BM25 score; use a keyword or `order: recency`"))
            }
            LqLeaf::StructuralBlock(_) => {
                Err(unscorable("a structural block is not executable on the history route"))
            }
            LqLeaf::Predicate { name, .. } => Err(unscorable(format!(
                "predicate `{name}` is not executable on the history route"
            ))),
        }
    }
}

/// The intersection of `parts`.
///
/// `Must` for queries, `MustNot` for exclusions; unconstrained parts
/// vanish; one empty part empties the whole. A conjunction of exclusions
/// alone is not enumerable: as a superset it is everything, as a subset it
/// is nothing.
fn conjunction(parts: Vec<Approximation>, side: Side) -> Approximation {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            Approximation::Nothing => return Approximation::Nothing,
            Approximation::Everything => {}
            Approximation::Query(query) => clauses.push((Occur::Must, query)),
            Approximation::Excluding(query) => clauses.push((Occur::MustNot, query)),
        }
    }
    if clauses.is_empty() {
        return Approximation::Everything;
    }
    if !clauses.iter().any(|(occur, _)| *occur == Occur::Must) {
        return match side {
            Side::Over => Approximation::Everything,
            Side::Under => Approximation::Nothing,
        };
    }
    Approximation::Query(Box::new(BooleanQuery::new(clauses)))
}

/// The union of `parts`.
///
/// `Should` for queries; empty parts vanish. An unconstrained or excluding
/// part cannot be a `Should` clause: as a superset it makes the whole
/// unconstrained (a row could match through it without a score), as a
/// subset it is dropped.
fn disjunction(parts: Vec<Approximation>, side: Side) -> Approximation {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            Approximation::Everything | Approximation::Excluding(_) => match side {
                Side::Over => return Approximation::Everything,
                Side::Under => {}
            },
            Approximation::Nothing => {}
            Approximation::Query(query) => clauses.push((Occur::Should, query)),
        }
    }
    if clauses.is_empty() {
        return Approximation::Nothing;
    }
    Approximation::Query(Box::new(BooleanQuery::new(clauses)))
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
        [single] => Ok(Box::new(TermQuery::new(single.clone(), IndexRecordOption::WithFreqs))),
        _ => Ok(Box::new(PhraseQuery::new(terms))),
    }
}
