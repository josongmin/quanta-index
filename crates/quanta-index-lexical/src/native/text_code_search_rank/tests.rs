//! Legacy `CodeSearch` ranking goldens.
//!
//! The scorer goldens are the Legacy unit tests of `searcher/code_search.rs`
//! and the fixed file scores of `tests/l3_exact_source.rs` at d8a490565e6f,
//! restated on the same bytes. The representative-match expectations are
//! byte offsets and lines read off the fixture bytes by hand.

use std::time::{Duration, Instant};

use quanta_index_core::index::{IndexError, WorkBudget, WorkLimits};

use quanta_index_lq_regex::RegexExecutor;

use super::{
    CodeSearchMatch, CodeSearchRank, CodeSearchScore, HitSurface, OverlappingMatches, RankMatcher,
    RankTerm, TermMatch, TermScope, best_in, boundary_score, min_cover_gap,
};
use crate::native::text::CaseMode;

macro_rules! check {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(IndexError::BackendViolation(format!(
                "test assertion failed at {}:{}: {}",
                file!(),
                line!(),
                format!($($message)+)
            )));
        }
    };
}

macro_rules! check_eq {
    ($left:expr, $right:expr) => {{
        let (left, right) = (&$left, &$right);
        check!(left == right, "{left:?} != {right:?}");
    }};
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1024 * 1024 * 1024,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn unscoped<'t>(terms: &[&'t str], case: CaseMode) -> CodeSearchRank<'t> {
    CodeSearchRank::new(
        terms
            .iter()
            .map(|term| (RankMatcher::Literal(term), TermScope::Both)),
        case,
    )
}

fn total(
    rank: &CodeSearchRank<'_>,
    path: &str,
    body: Option<&str>,
) -> Result<Option<u32>, IndexError> {
    Ok(rank
        .score(body, path, &mut budget()?)?
        .map(|ranked| ranked.score.total()))
}

fn content_term(text: &str) -> RankTerm<'static> {
    RankTerm {
        matcher: TermMatch::Literal {
            text: text.into(),
            needle: text.into(),
        },
        scope: TermScope::Content,
    }
}

#[test]
fn exact_identifier_beats_prefix_and_infix() -> Result<(), IndexError> {
    check!(
        boundary_score("foo", 0..3, false) > boundary_score("foobar", 0..3, false),
        "exact must beat prefix"
    );
    check!(
        boundary_score("foobar", 0..3, false) > boundary_score("afoobar", 1..4, false),
        "prefix must beat infix"
    );
    Ok(())
}

#[test]
fn combining_mark_is_not_an_identifier_boundary() -> Result<(), IndexError> {
    let exact = boundary_score("म", 0.."म".len(), false);
    let prefix = boundary_score("म्", 0.."म".len(), false);
    check_eq!(exact, 100);
    check_eq!(prefix, 75);
    Ok(())
}

#[test]
fn overlapping_literal_witnesses_preserve_minimum_file_proximity() -> Result<(), IndexError> {
    check_eq!(
        OverlappingMatches::new("aaaaab", "aaa").collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    check_eq!(
        OverlappingMatches::new("가가가", "가가").collect::<Vec<_>>(),
        vec![0, 3]
    );
    let terms = [content_term("aaa"), content_term("b")];
    // The final aaa occupies bytes 2..5 and b occupies 5..6.
    check_eq!(
        min_cover_gap("aaaaab", &terms, HitSurface::Content, &mut budget()?)?,
        Some(0)
    );
    let witness = best_in(
        "aaaaab",
        "aaa",
        HitSurface::Content,
        CaseMode::Sensitive,
        &mut budget()?,
    )?
    .ok_or_else(|| IndexError::BackendViolation("literal witness".into()))?;
    check_eq!(witness.occurrences, 3);
    Ok(())
}

#[test]
fn ranking_considers_exact_boundary_after_thirty_two_prefix_occurrences() -> Result<(), IndexError>
{
    let text = format!("{}foo", "foobar ".repeat(33));
    let witness = best_in(
        &text,
        "foo",
        HitSurface::Content,
        CaseMode::Sensitive,
        &mut budget()?,
    )?
    .ok_or_else(|| IndexError::BackendViolation("witness".into()))?;
    check_eq!(witness.normalized.start, "foobar ".len() * 33);
    check_eq!(witness.score, boundary_score("foo", 0..3, false));
    Ok(())
}

#[test]
fn code_search_fixed_scores() -> Result<(), IndexError> {
    let alpha = unscoped(&["alpha"], CaseMode::Sensitive);
    check_eq!(total(&alpha, "single.rs", Some("alpha"))?, Some(100));
    // Five occurrences, three extra count.
    check_eq!(
        total(&alpha, "many.rs", Some("alpha alpha alpha alpha alpha"))?,
        Some(106)
    );
    let two = unscoped(&["alpha", "beta"], CaseMode::Sensitive);
    // 100 + 100 + (32 - 1 byte gap).
    check_eq!(total(&two, "adjacent.rs", Some("alpha beta"))?, Some(231));
    // Gap beyond 32 bytes.
    let distant = format!("alpha {} beta", "x".repeat(40));
    check_eq!(total(&two, "distant.rs", Some(&distant))?, Some(200));
    // Two alpha occurrences add 2.
    check_eq!(
        total(&two, "plain.rs", Some("alpha beta alpha"))?,
        Some(233)
    );
    let folded = unscoped(&["Alpha"], CaseMode::Folded);
    check_eq!(total(&folded, "case_upper.rs", Some("Alpha"))?, Some(105));
    check_eq!(total(&folded, "case_lower.rs", Some("alpha"))?, Some(100));
    let path = CodeSearchRank::new(
        [(RankMatcher::Literal("alpha"), TermScope::Path)],
        CaseMode::Sensitive,
    );
    check_eq!(total(&path, "alpha.rs", None)?, Some(135));
    Ok(())
}

#[test]
fn folded_exact_case_is_a_separate_component() -> Result<(), IndexError> {
    let rank = unscoped(&["needle"], CaseMode::Folded);
    let ranked = rank
        .score(Some("needle needle needle"), "first.rs", &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("match".into()))?;
    check_eq!(ranked.score.boundary_and_path, 100);
    check_eq!(ranked.score.exact_case, 5);
    Ok(())
}

#[test]
fn large_unicode_sources_keep_fixed_scores() -> Result<(), IndexError> {
    // U+0130 expands under folding before the witness.
    let body = format!("İ{}fooBar", " ".repeat(120_000));
    for (needle, case, expected) in [
        ("bar", CaseMode::Folded, 40),
        ("Bar", CaseMode::Folded, 45),
        ("Bar", CaseMode::Sensitive, 40),
    ] {
        check_eq!(
            total(&unscoped(&[needle], case), "large.rs", Some(&body))?,
            Some(expected)
        );
    }
    let nfd = format!("cafe\u{301}{}fooBar", " ".repeat(120_000));
    check_eq!(
        total(
            &unscoped(&["bar"], CaseMode::Folded),
            "nfd-large.rs",
            Some(&nfd)
        )?,
        Some(40)
    );
    Ok(())
}

#[test]
fn a_term_without_a_witness_ranks_no_file() -> Result<(), IndexError> {
    let rank = unscoped(&["alpha", "gamma"], CaseMode::Sensitive);
    check_eq!(total(&rank, "src/x.rs", Some("alpha beta"))?, None);
    Ok(())
}

fn matched(
    rank: &CodeSearchRank<'_>,
    path: &str,
    body: Option<&str>,
) -> Result<CodeSearchMatch, IndexError> {
    Ok(rank
        .score(body, path, &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("expected a ranked file".into()))?
        .matched)
}

#[test]
fn representative_match_is_reported_on_verified_original_bytes() -> Result<(), IndexError> {
    let folded = unscoped(&["bar"], CaseMode::Folded);
    // ASCII: bytes 20..23 ("Bar") on line 2.
    check_eq!(
        matched(&folded, "src/a.rs", Some("fn main() {}\nlet fooBar = 1;\n"))?,
        CodeSearchMatch::Content {
            start: 20,
            end: 23,
            start_line: 2,
            end_line: 2,
        }
    );
    // NFC with a folding expansion: "İ" is two source bytes and three folded
    // bytes, so the original offset stays at the raw "Bar".
    check_eq!(
        matched(&folded, "src/a.rs", Some("İ\nlet fooBar\n"))?,
        CodeSearchMatch::Content {
            start: 10,
            end: 13,
            start_line: 2,
            end_line: 2,
        }
    );
    // Non-NFC source goes through the provenance map.
    check_eq!(
        matched(&folded, "src/a.rs", Some("cafe\u{301}\n\nfooBar"))?,
        CodeSearchMatch::Content {
            start: 11,
            end: 14,
            start_line: 3,
            end_line: 3,
        }
    );
    // A witness spanning a line break reports both lines.
    check_eq!(
        matched(
            &unscoped(&["a\nb"], CaseMode::Sensitive),
            "src/a.rs",
            Some("x a\nb y")
        )?,
        CodeSearchMatch::Content {
            start: 2,
            end: 5,
            start_line: 1,
            end_line: 2,
        }
    );
    Ok(())
}

#[test]
fn representative_match_prefers_the_strictly_better_surface_and_term() -> Result<(), IndexError> {
    let alpha = unscoped(&["alpha"], CaseMode::Sensitive);
    // Basename path 135 beats an exact body identifier 100.
    check_eq!(
        matched(&alpha, "src/alpha.rs", Some("alpha"))?,
        CodeSearchMatch::Path { start: 4, end: 9 }
    );
    // A directory path infix (40 + 15) loses to the exact body identifier.
    check_eq!(
        matched(&alpha, "xalphay/z.rs", Some("let alpha;"))?,
        CodeSearchMatch::Content {
            start: 4,
            end: 9,
            start_line: 1,
            end_line: 1,
        }
    );
    // Across terms the higher boundary score wins: "pre" is a prefix (75),
    // "word" is exact (100).
    check_eq!(
        matched(
            &unscoped(&["pre", "word"], CaseMode::Sensitive),
            "src/z.rs",
            Some("prefix\nword\n")
        )?,
        CodeSearchMatch::Content {
            start: 7,
            end: 11,
            start_line: 2,
            end_line: 2,
        }
    );
    // On a tie the earlier term wins.
    check_eq!(
        matched(
            &unscoped(&["one", "two"], CaseMode::Sensitive),
            "src/z.rs",
            Some("two one")
        )?,
        CodeSearchMatch::Content {
            start: 4,
            end: 7,
            start_line: 1,
            end_line: 1,
        }
    );
    Ok(())
}

fn regex(pattern: &str) -> Result<RegexExecutor, IndexError> {
    RegexExecutor::compile(pattern).map_err(|error| IndexError::InvalidInput(error.to_string()))
}

/// Legacy `cached_literal_score_preserves_earlier_regex_witness_and_rejection`.
#[test]
fn regex_and_literal_witnesses_keep_the_legacy_primary_and_rejection() -> Result<(), IndexError> {
    let abc = regex("abc")?;
    let rank = CodeSearchRank::new(
        [
            (
                RankMatcher::Regex {
                    content: Some(&abc),
                    path: None,
                },
                TermScope::Content,
            ),
            (RankMatcher::Literal("abc"), TermScope::Content),
        ],
        CaseMode::Sensitive,
    );
    let ranked = rank
        .score(Some("abc abc"), "file.rs", &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("complete match".into()))?;
    // Each term: an exact identifier (100) with one extra occurrence (2). A
    // regex term never earns exact case or proximity.
    check_eq!(
        ranked.score,
        CodeSearchScore {
            boundary_and_path: 200,
            occurrence: 4,
            exact_case: 0,
            proximity: 0,
        }
    );
    // The equal-score tie keeps the earlier term: the regex witness.
    check_eq!(
        ranked.matched,
        CodeSearchMatch::Content {
            start: 0,
            end: 3,
            start_line: 1,
            end_line: 1,
        }
    );
    let missing = regex("missing")?;
    let rejected = CodeSearchRank::new(
        [
            (
                RankMatcher::Regex {
                    content: Some(&missing),
                    path: None,
                },
                TermScope::Content,
            ),
            (RankMatcher::Literal("abc"), TermScope::Content),
        ],
        CaseMode::Sensitive,
    );
    check_eq!(
        rejected.score(Some("abc abc"), "file.rs", &mut budget()?)?,
        None
    );
    Ok(())
}

#[test]
fn regex_terms_score_each_legacy_scope() -> Result<(), IndexError> {
    // `path:/beta[.]rs/`: a basename path witness, 100 + 35.
    let beta = regex("(?i)beta[.]rs")?;
    let path = CodeSearchRank::new(
        [(
            RankMatcher::Regex {
                content: None,
                path: Some(&beta),
            },
            TermScope::Path,
        )],
        CaseMode::Folded,
    );
    let ranked = path
        .score(None, "src/beta.rs", &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("path match".into()))?;
    check_eq!(ranked.score.total(), 135);
    check_eq!(ranked.matched, CodeSearchMatch::Path { start: 4, end: 11 });
    // An unscoped regex searches the body and the path.
    let sphinx = regex("(?i)sphinx.*quartz")?;
    let both = CodeSearchRank::new(
        [(
            RankMatcher::Regex {
                content: Some(&sphinx),
                path: Some(&sphinx),
            },
            TermScope::Both,
        )],
        CaseMode::Folded,
    );
    let ranked = both
        .score(Some("sphinx of quartz\n"), "src/alpha.rs", &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("body match".into()))?;
    check_eq!(ranked.score.total(), 100);
    check_eq!(
        ranked.matched,
        CodeSearchMatch::Content {
            start: 0,
            end: 16,
            start_line: 1,
            end_line: 1,
        }
    );
    let quartz = regex("(?i)quartz[.]md")?;
    let named = CodeSearchRank::new(
        [(
            RankMatcher::Regex {
                content: Some(&quartz),
                path: Some(&quartz),
            },
            TermScope::Both,
        )],
        CaseMode::Folded,
    );
    let ranked = named
        .score(Some("no match here"), "docs/quartz.md", &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("path match".into()))?;
    check_eq!(ranked.score.total(), 135);
    check_eq!(ranked.matched, CodeSearchMatch::Path { start: 5, end: 14 });
    Ok(())
}

#[test]
fn regex_terms_skip_proximity_and_scan_the_unfolded_nfc_text() -> Result<(), IndexError> {
    // Legacy proximity needs every term to be a literal.
    let beta = regex("beta")?;
    let mixed = CodeSearchRank::new(
        [
            (RankMatcher::Literal("alpha"), TermScope::Both),
            (
                RankMatcher::Regex {
                    content: Some(&beta),
                    path: Some(&beta),
                },
                TermScope::Both,
            ),
        ],
        CaseMode::Sensitive,
    );
    check_eq!(total(&mixed, "adjacent.rs", Some("alpha beta"))?, Some(200));
    // A folded regex runs on the NFC text with an inline case flag, so its
    // span maps back through NFC only: "Bar" after "İ\nfoo" is bytes 6..9.
    let bar = regex("(?i)bar")?;
    let folded = |body: &str| -> Result<CodeSearchMatch, IndexError> {
        Ok(CodeSearchRank::new(
            [(
                RankMatcher::Regex {
                    content: Some(&bar),
                    path: Some(&bar),
                },
                TermScope::Both,
            )],
            CaseMode::Folded,
        )
        .score(Some(body), "src/a.rs", &mut budget()?)?
        .ok_or_else(|| IndexError::BackendViolation("regex match".into()))?
        .matched)
    };
    check_eq!(
        folded("İ\nfooBar")?,
        CodeSearchMatch::Content {
            start: 6,
            end: 9,
            start_line: 2,
            end_line: 2,
        }
    );
    // NFD source: the NFC span 6..9 is raw bytes 7..10.
    check_eq!(
        folded("cafe\u{301}\nBar")?,
        CodeSearchMatch::Content {
            start: 7,
            end: 10,
            start_line: 2,
            end_line: 2,
        }
    );
    Ok(())
}

#[test]
fn regex_witness_refuses_over_cap_and_zero_width_matches() -> Result<(), IndexError> {
    let one = regex("a")?;
    let rank = CodeSearchRank::new(
        [(
            RankMatcher::Regex {
                content: Some(&one),
                path: None,
            },
            TermScope::Content,
        )],
        CaseMode::Sensitive,
    );
    // Legacy refuses more than 4,096 matches on one surface.
    let crowded = "a ".repeat(4_097);
    check!(
        matches!(
            rank.score(Some(&crowded), "src/a.rs", &mut budget()?),
            Err(IndexError::ResourceLimited)
        ),
        "over-cap regex must refuse"
    );
    let capped = "a ".repeat(4_096);
    check_eq!(total(&rank, "src/a.rs", Some(&capped))?, Some(106));
    let empty = regex("x*")?;
    let zero = CodeSearchRank::new(
        [(
            RankMatcher::Regex {
                content: Some(&empty),
                path: None,
            },
            TermScope::Content,
        )],
        CaseMode::Sensitive,
    );
    check!(
        matches!(
            zero.score(Some("yyy"), "src/a.rs", &mut budget()?),
            Err(IndexError::InvalidInput(_))
        ),
        "zero-width regex witness must refuse"
    );
    Ok(())
}

/// A fixed-seed linear congruential generator; tests need no entropy.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bound = u64::try_from(bound.max(1)).map_or(1, |bound| bound);
        let value = self
            .0
            .checked_shr(33)
            .and_then(|high| high.checked_rem(bound))
            .map_or(0, |value| value);
        usize::try_from(value).map_or(0, |value| value)
    }
}

/// Pieces covering ASCII identifiers and every fold class the scorer maps:
/// NFC and NFD accents, the dotted capital I, sharp s and the Kelvin sign.
const PIECES: [&str; 14] = [
    "foo", "Bar", "_", "a", "b", " ", "\n", "é", "e\u{301}", "İ", "ß", "K", "\u{212A}", "/",
];

#[expect(
    clippy::indexing_slicing,
    reason = "the generator indexes its own fixed piece table"
)]
fn text(random: &mut Lcg, pieces: usize) -> String {
    (0..pieces)
        .map(|_| PIECES[random.below(PIECES.len())])
        .collect()
}

/// Literal scores and witnesses follow oracles written here.
///
/// Random literal terms over random bodies and paths: whether each term
/// occurs (NFC, then the case mode, on its surfaces), that the total does
/// not depend on term order, and that the representative match is a real
/// span of the verified bytes whose normalized form contains a needle, with
/// lines counted on those bytes.
#[test]
#[expect(
    clippy::indexing_slicing,
    clippy::naive_bytecount,
    clippy::disallowed_methods,
    reason = "the oracle indexes its own generated terms and counts newlines directly"
)]
fn literal_scores_and_witnesses_follow_independent_oracles() -> Result<(), IndexError> {
    let mut random = Lcg(0x5eed_0009_0003);
    let mut ranked_files = 0usize;
    for _case in 0..600 {
        let case = if random.below(2) == 0 {
            CaseMode::Folded
        } else {
            CaseMode::Sensitive
        };
        let body = text(&mut random, 12);
        let path = format!("src/{}.rs", text(&mut random, 3).replace(['\n', ' '], "x"));
        let count = 1 + random.below(3);
        let terms: Vec<String> = (0..count)
            .map(|_| {
                // Mostly a substring of the body or path, sometimes absent.
                let source = match random.below(3) {
                    0 => path.clone(),
                    1 => body.clone(),
                    _ => text(&mut random, 2),
                };
                let chars: Vec<char> = source.chars().collect();
                let start = random.below(chars.len());
                let len = 1 + random.below(3);
                chars.iter().skip(start).take(len).collect::<String>()
            })
            .filter(|term| !term.is_empty())
            .collect();
        if terms.is_empty() {
            continue;
        }
        let scopes: Vec<TermScope> = terms
            .iter()
            .map(|_| match random.below(3) {
                0 => TermScope::Content,
                1 => TermScope::Path,
                _ => TermScope::Both,
            })
            .collect();
        let rank = |order: &[usize]| {
            CodeSearchRank::new(
                order
                    .iter()
                    .map(|index| (RankMatcher::Literal(&terms[*index]), scopes[*index])),
                case,
            )
        };
        let forward: Vec<usize> = (0..terms.len()).collect();
        let reverse: Vec<usize> = forward.iter().rev().copied().collect();
        let scored = rank(&forward).score(Some(&body), &path, &mut budget()?)?;
        let reversed = rank(&reverse).score(Some(&body), &path, &mut budget()?)?;
        // Oracle: every term occurs on one of its surfaces.
        let surface = |value: &str| {
            crate::normalize::apply_case(&crate::normalize::nfc(value), case).into_owned()
        };
        let (body_form, path_form) = (surface(&body), surface(&path));
        let occurs = terms.iter().zip(&scopes).all(|(term, scope)| {
            let needle = surface(term);
            (!matches!(scope, TermScope::Path) && body_form.contains(&needle))
                || (!matches!(scope, TermScope::Content) && path_form.contains(&needle))
        });
        check_eq!(scored.is_some(), occurs);
        check_eq!(
            scored.map(|ranked| ranked.score.total()),
            reversed.map(|ranked| ranked.score.total())
        );
        let Some(ranked) = scored else { continue };
        ranked_files += 1;
        let needles: Vec<String> = terms.iter().map(|term| surface(term)).collect();
        let (raw, start, end) = match ranked.matched {
            CodeSearchMatch::Content {
                start,
                end,
                start_line,
                end_line,
            } => {
                let (start, end) = (
                    usize::try_from(start).map_err(|_e| IndexError::ResourceLimited)?,
                    usize::try_from(end).map_err(|_e| IndexError::ResourceLimited)?,
                );
                let lines_before = |at: usize| {
                    u32::try_from(
                        body.as_bytes()[..at]
                            .iter()
                            .filter(|byte| **byte == b'\n')
                            .count(),
                    )
                    .unwrap_or(u32::MAX)
                        + 1
                };
                check!(start < end, "empty witness");
                check_eq!(start_line, lines_before(start));
                check_eq!(end_line, lines_before(end - 1));
                (body.as_str(), start, end)
            }
            CodeSearchMatch::Path { start, end } => (
                path.as_str(),
                usize::try_from(start).map_err(|_e| IndexError::ResourceLimited)?,
                usize::try_from(end).map_err(|_e| IndexError::ResourceLimited)?,
            ),
        };
        let span = raw.get(start..end).ok_or_else(|| {
            IndexError::BackendViolation(format!("{start}..{end} is not a span of {raw:?}"))
        })?;
        let span_form = surface(span);
        check!(
            needles
                .iter()
                .any(|needle| span_form.contains(needle.as_str())),
            "witness {span:?} holds no needle of {terms:?} in {raw:?}"
        );
    }
    check!(ranked_files > 100, "only {ranked_files} files ranked");
    Ok(())
}
