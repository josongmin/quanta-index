use crate::{LqCase, LqExpr, LqFilter, LqLeaf, LqPatternType, LqPredicateArg, LqSelect};

use super::{
    CodeSearchNativeLeaf, CodeSearchQueryError, MAX_NATIVE_CONTENT_REGEX_LEAVES,
    MAX_NATIVE_FILE_CONTAINS_LEAVES, MAX_NATIVE_PATH_CONTAINS_LEAVES,
    code_search_file_candidate_id, code_search_query_backing_bytes, lower_code_search_native_query,
    parse_code_search_query,
};
use crate::lex::LexicalErrorCode;
use crate::{
    MAX_NATIVE_PATH_REGEX_LEAVES, parse_native_content_query, tokenize_native_content_query,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn contains(needle: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Predicate {
        name: "file.contains".into(),
        args: vec![LqPredicateArg::RawString(needle.into())],
    })
}

fn path_contains(needle: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Predicate {
        name: "path.contains".into(),
        args: vec![LqPredicateArg::RawString(needle.into())],
    })
}

/// A bare term matches the Source body or the Source path.
fn body_or_path(needle: &str) -> LqExpr {
    LqExpr::Any(vec![contains(needle), path_contains(needle)])
}

fn regex(source: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Regex(source.into()))
}

fn path_regex(source: &str) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Predicate {
        name: "path.regex".into(),
        args: vec![LqPredicateArg::RawString(source.into())],
    })
}

/// Whether `expr` is the disjunction of exactly `children`, in any order.
fn is_any_of(expr: &LqExpr, children: &[LqExpr]) -> bool {
    matches!(expr, LqExpr::Any(actual)
        if actual.len() == children.len() && children.iter().all(|child| actual.contains(child)))
}

/// Whether `expr` is the conjunction of exactly `children`, in any order.
fn is_all_of(expr: &LqExpr, children: &[LqExpr]) -> bool {
    matches!(expr, LqExpr::All(actual)
        if actual.len() == children.len() && children.iter().all(|child| actual.contains(child)))
}

/// A bare regex matches the Source body or the Source path, with one
/// pattern, byte for byte, in both leaves.
fn is_regex_or_path(expr: &LqExpr, source: &str) -> bool {
    is_any_of(expr, &[regex(source), path_regex(source)])
}

const SELECT_PATH: LqFilter = LqFilter::Select {
    dim: LqSelect::Path,
};

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn canonical_grammar_keeps_the_product_file_shape() -> TestResult {
    let query = parse_code_search_query("writeContent Type")?;
    assert_eq!(query.options.pattern_type, LqPatternType::CodeSearch);
    assert_eq!(query.options.case, None);
    assert_eq!(
        query.filters,
        [LqFilter::Select {
            dim: LqSelect::File
        }]
    );
    assert_eq!(
        query.expr,
        LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::RawString("Type".into())),
            LqExpr::Leaf(LqLeaf::RawString("writeContent".into())),
        ])
    );
    let refusal = parse_code_search_query("lang:rust needle");
    assert!(
        matches!(&refusal, Err(CodeSearchQueryError::Grammar(message)) if message.contains("native syntax")),
        "{refusal:?}"
    );
    assert!(
        refusal
            .err()
            .is_some_and(|error| error.to_string().starts_with("code_search: "))
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn native_lowering_maps_bare_terms_to_body_or_path_and_scopes_to_their_leaves() -> TestResult {
    let query = lower_code_search_native_query(
        r#"Context foo::bar content:"hello brave world" path:Src/Lib.rs"#,
    )?;
    assert_eq!(query.options.pattern_type, LqPatternType::Standard);
    assert_eq!(query.options.case, None);
    assert!(query.directives.is_empty());
    let LqExpr::All(children) = &query.expr else {
        return Err(format!("several terms must stay conjunctive: {:?}", query.expr).into());
    };
    assert_eq!(children.len(), 4);
    for bare in ["Context", "foo::bar"] {
        assert!(children.contains(&body_or_path(bare)), "{bare}");
    }
    assert!(children.contains(&contains("hello brave world")));
    // A `path:` literal is a path substring leaf, not a regex filter.
    assert!(children.contains(&path_contains("Src/Lib.rs")));
    assert_eq!(query.filters, [SELECT_PATH]);

    let single = lower_code_search_native_query("WithContext")?;
    assert_eq!(single.expr, body_or_path("WithContext"));
    assert_eq!(single.filters, [SELECT_PATH]);
    let scoped = lower_code_search_native_query("content:WithContext")?;
    assert_eq!(scoped.expr, contains("WithContext"));
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn native_lowering_keeps_case_policy_and_regex_scopes() -> TestResult {
    // The query case policy governs every leaf, path substrings included.
    let sensitive = lower_code_search_native_query("case:yes Context path:src/Lib.rs")?;
    assert_eq!(sensitive.options.case, Some(LqCase::Sensitive));
    let LqExpr::All(children) = &sensitive.expr else {
        return Err(format!("two terms must stay conjunctive: {:?}", sensitive.expr).into());
    };
    assert_eq!(children.len(), 2);
    assert!(children.contains(&body_or_path("Context")));
    assert!(children.contains(&path_contains("src/Lib.rs")));
    assert_eq!(sensitive.filters, [SELECT_PATH]);

    let folded = lower_code_search_native_query("case:no Context")?;
    assert_eq!(folded.options.case, Some(LqCase::Insensitive));

    // A bare regex is body-or-path; `content:` keeps the body, `path:` the
    // path. No regex becomes a `file:` filter, and none gains a `(?i)`: the
    // query case policy governs every regex leaf.
    let regexes = lower_code_search_native_query(
        r"regex:/sphinx.*quartz/ content:regex:/fn\s+main/ path:regex:/beta[.]rs/",
    )?;
    let LqExpr::All(children) = &regexes.expr else {
        return Err(format!("regex terms must stay conjunctive: {:?}", regexes.expr).into());
    };
    assert_eq!(children.len(), 3, "{children:?}");
    assert!(
        children
            .iter()
            .any(|child| is_regex_or_path(child, "sphinx.*quartz")),
        "{children:?}"
    );
    assert!(children.contains(&regex(r"fn\s+main")));
    assert!(children.contains(&path_regex("beta[.]rs")));
    assert_eq!(regexes.filters, [SELECT_PATH]);
    assert_eq!(regexes.options.case, None);

    let exact = lower_code_search_native_query("case:yes sphinx path:regex:/Beta/")?;
    assert!(
        is_all_of(&exact.expr, &[body_or_path("sphinx"), path_regex("Beta")]),
        "{:?}",
        exact.expr
    );
    assert_eq!(exact.filters, [SELECT_PATH]);
    assert_eq!(exact.options.case, Some(LqCase::Sensitive));

    // A leading `(?i)` is the query's case policy, so both leaves of a bare
    // regex carry the pattern without it.
    let inline = lower_code_search_native_query("regex:/(?i)Beta/")?;
    assert!(is_regex_or_path(&inline.expr, "Beta"), "{:?}", inline.expr);
    assert_eq!(inline.options.case, Some(LqCase::Insensitive));
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn path_only_queries_lower_to_path_leaves() -> TestResult {
    // A path regex alone is one `path.regex` leaf.
    let only_regex = lower_code_search_native_query("path:regex:/beta[.]rs/")?;
    assert_eq!(only_regex.expr, path_regex("beta[.]rs"));
    assert_eq!(only_regex.filters, [SELECT_PATH]);
    // A path literal stays a path substring leaf.
    let literal = lower_code_search_native_query("path:alpha")?;
    assert_eq!(literal.expr, path_contains("alpha"));
    assert_eq!(literal.filters, [SELECT_PATH]);
    let both = lower_code_search_native_query("path:src path:regex:/beta/")?;
    assert!(
        is_all_of(&both.expr, &[path_contains("src"), path_regex("beta")]),
        "{:?}",
        both.expr
    );
    assert_eq!(both.filters, [SELECT_PATH]);
    // A bare regex alone may match a path only.
    let bare = lower_code_search_native_query("regex:/beta[.]rs/")?;
    assert!(is_regex_or_path(&bare.expr, "beta[.]rs"), "{:?}", bare.expr);
    assert_eq!(bare.filters, [SELECT_PATH]);
    Ok(())
}

#[test]
fn native_leaf_fanout_above_the_native_limits_is_a_typed_refusal() {
    assert_eq!(
        (
            MAX_NATIVE_FILE_CONTAINS_LEAVES,
            MAX_NATIVE_PATH_CONTAINS_LEAVES,
            MAX_NATIVE_CONTENT_REGEX_LEAVES,
            MAX_NATIVE_PATH_REGEX_LEAVES
        ),
        (4, 4, 4, 4)
    );
    // Four of each kind is the largest accepted shape.
    for accepted in [
        "a b c d path:regex:/e/ path:regex:/f/ path:regex:/g/ path:regex:/h/",
        "a b c d content:regex:/e/ content:regex:/f/ content:regex:/g/ content:regex:/h/",
        // Each bare regex spends one content and one path regex.
        "regex:/a/ regex:/b/ regex:/c/ regex:/d/",
        "regex:/a/ regex:/b/ content:regex:/c/ path:regex:/d/ content:regex:/e/ path:regex:/f/",
    ] {
        let lowered = lower_code_search_native_query(accepted);
        assert!(lowered.is_ok(), "{accepted}: {lowered:?}");
    }
    for (query, leaf) in [
        // Each bare term spends one body and one path substring.
        ("a b c d e", CodeSearchNativeLeaf::FileContains),
        (
            "content:a content:b content:c content:d content:e",
            CodeSearchNativeLeaf::FileContains,
        ),
        (
            "content:a content:b content:c content:d e",
            CodeSearchNativeLeaf::FileContains,
        ),
        (
            "path:a path:b path:c path:d path:e",
            CodeSearchNativeLeaf::PathContains,
        ),
        ("a b c path:d path:e", CodeSearchNativeLeaf::PathContains),
        (
            "x path:regex:/a/ path:regex:/b/ path:regex:/c/ path:regex:/d/ path:regex:/e/",
            CodeSearchNativeLeaf::PathRegex,
        ),
        (
            "content:regex:/a/ content:regex:/b/ content:regex:/c/ content:regex:/d/ content:regex:/e/",
            CodeSearchNativeLeaf::ContentRegex,
        ),
        (
            "regex:/a/ regex:/b/ regex:/c/ regex:/d/ content:regex:/e/",
            CodeSearchNativeLeaf::ContentRegex,
        ),
        (
            "regex:/a/ regex:/b/ regex:/c/ path:regex:/d/ path:regex:/e/",
            CodeSearchNativeLeaf::PathRegex,
        ),
        // Each bare regex spends its content regex first.
        (
            "regex:/a/ regex:/b/ regex:/c/ regex:/d/ regex:/e/",
            CodeSearchNativeLeaf::ContentRegex,
        ),
    ] {
        let refusal = lower_code_search_native_query(query);
        assert!(
            matches!(&refusal, Err(CodeSearchQueryError::NativeFanout { leaf: refused, limit: 4 }) if *refused == leaf),
            "{query}: {refusal:?}"
        );
        assert_eq!(
            refusal.err().map(|error| error.lexical_code()),
            Some(LexicalErrorCode::LimitExceededFanout),
            "{query}"
        );
    }
}

#[test]
fn native_lowering_refuses_modes_without_a_native_plan() {
    for (query, reason) in [
        ("typo:load_jsom", "typo:"),
        (r#"components:"update available""#, "components:"),
        ("symbol:Type", "symbol route"),
        ("needle case:maybe", "yes or no"),
    ] {
        let refusal = lower_code_search_native_query(query);
        assert!(
            matches!(&refusal, Err(CodeSearchQueryError::Grammar(message)) if message.contains(reason)),
            "{query}: {refusal:?}"
        );
    }
    let contradictory = lower_code_search_native_query("case:yes regex:/(?i)needle/");
    assert!(
        matches!(contradictory, Err(CodeSearchQueryError::Lq(_))),
        "a leading (?i) under case:yes stays a canonical LQ refusal"
    );
}

#[test]
fn backing_is_bounded_by_the_canonical_input_limit() {
    assert!(code_search_query_backing_bytes(16 * 1024).is_some());
    assert!(code_search_query_backing_bytes(16 * 1024 + 1).is_none());
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn file_candidate_identity_matches_the_published_legacy_goldens() -> TestResult {
    // Fixed ids published by the Legacy CodeSearch executor for these files.
    assert_eq!(
        code_search_file_candidate_id("repo-e2e", "src/migration.rs")?,
        "file:3c95b3152f276384c83c74ed7f113e9eb0caaa7f944d588e26d6360fb6c7e66a"
    );
    assert_eq!(
        code_search_file_candidate_id("repo-e2e", "src/post_restore.rs")?,
        "file:ce5f197dbf2c55b121f3c6ef865ba26dd0fa5df2e91a35b55c14db6266005204"
    );
    Ok(())
}

#[test]
fn refusal_codes_separate_unplannable_shapes_from_parse_failures() {
    for (query, expected) in [
        (
            "symbol.has.name(needle)",
            LexicalErrorCode::UnsupportedCombo,
        ),
        ("\"unterminated", LexicalErrorCode::ParseFail),
        ("needle AND", LexicalErrorCode::ParseFail),
        ("/[/", LexicalErrorCode::ParseFail),
    ] {
        let refusal = tokenize_native_content_query(query)
            .and_then(super::super::NativeContentQueryTokens::parse)
            .err();
        assert_eq!(
            refusal.as_ref().map(super::native_query_refusal_code),
            Some(expected),
            "{query}"
        );
    }
    assert!(parse_native_content_query("needle").is_ok());
    for query in ["typo:load_jsom", "lang:rust needle"] {
        assert_eq!(
            lower_code_search_native_query(query)
                .err()
                .map(|error| error.lexical_code()),
            Some(LexicalErrorCode::UnsupportedCombo),
            "{query}"
        );
    }
}

/// A path regex follows the canonical regex rules of a Content regex.
///
/// A leading `(?i)` is the query's `case:no`, it contradicts `case:yes`, and
/// a mid-pattern inline flag is forbidden. A path regex never carries a flag
/// past the lowering.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn path_regexes_follow_the_canonical_regex_flag_rules() -> TestResult {
    let folded = lower_code_search_native_query("path:regex:/(?i)Beta/")?;
    assert_eq!(folded.expr, path_regex("Beta"));
    assert_eq!(folded.options.case, Some(LqCase::Insensitive));
    let content = lower_code_search_native_query("content:regex:/(?i)Beta/")?;
    assert_eq!(content.expr, regex("Beta"));
    assert_eq!(content.options.case, Some(LqCase::Insensitive));
    for query in [
        "case:yes path:regex:/(?i)beta/",
        "case:yes content:regex:/(?i)beta/",
        "path:regex:/a(?i)b/",
        "content:regex:/a(?i)b/",
        "regex:/a(?i)b/",
    ] {
        let refusal = lower_code_search_native_query(query);
        assert!(
            matches!(&refusal, Err(CodeSearchQueryError::Lq(_))),
            "{query}: {refusal:?}"
        );
    }
    Ok(())
}

/// A fixed-seed xorshift64* stream, so every generated table is reproducible.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> Result<T, String> {
        let draw = self.next();
        u64::try_from(items.len())
            .map_err(|error| error.to_string())
            .and_then(|len| {
                let index = draw.checked_rem(len).ok_or("empty pick table")?;
                usize::try_from(index).map_err(|error| error.to_string())
            })
            .and_then(|index| {
                items
                    .get(index)
                    .copied()
                    .ok_or_else(|| "pick out of range".into())
            })
    }
}

/// Files of the lowering property: paths and bodies over `a`, `b`, `A`, `B`.
const PROPERTY_CORPUS: [(&str, &str); 7] = [
    ("src/ab.rs", "Ba ba"),
    ("lib/BA.rs", "aab"),
    ("a/b.rs", "bb"),
    ("x.rs", "AB ab"),
    ("ab/ab.rs", ""),
    ("b.rs", "a"),
    ("Bb/x.rs", "BAB"),
];

#[derive(Clone, Copy, Debug)]
enum TermScope {
    Both,
    Content,
    Path,
}

/// The product meaning of a conjunction of terms, independent of lowering.
///
/// A term matches a file when its needle occurs in a surface its scope
/// names, under the query's case policy. The generated regexes are plain
/// letters, so a regex term means the same as its literal.
fn reference_matches(terms: &[(TermScope, &str)], sensitive: bool, path: &str, body: &str) -> bool {
    let fold = |text: &str| {
        if sensitive {
            text.to_owned()
        } else {
            text.to_lowercase()
        }
    };
    terms.iter().all(|(scope, needle)| {
        let needle = fold(needle);
        let in_body = fold(body).contains(&needle);
        let in_path = fold(path).contains(&needle);
        match scope {
            TermScope::Both => in_body || in_path,
            TermScope::Content => in_body,
            TermScope::Path => in_path,
        }
    })
}

/// Evaluate a lowered expression over one file with the Native leaf meaning.
fn lowered_matches(expr: &LqExpr, sensitive: bool, path: &str, body: &str) -> Result<bool, String> {
    let contains = |haystack: &str, needle: &str| {
        if sensitive {
            haystack.contains(needle)
        } else {
            haystack.to_lowercase().contains(&needle.to_lowercase())
        }
    };
    match expr {
        LqExpr::All(children) => children.iter().try_fold(true, |all, child| {
            Ok(all && lowered_matches(child, sensitive, path, body)?)
        }),
        LqExpr::Any(children) => children.iter().try_fold(false, |any, child| {
            Ok(any || lowered_matches(child, sensitive, path, body)?)
        }),
        LqExpr::Leaf(LqLeaf::Regex(pattern)) => Ok(contains(body, pattern)),
        LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
            let [LqPredicateArg::RawString(needle)] = args.as_slice() else {
                return Err(format!("{name} lost its single raw argument"));
            };
            match name.as_str() {
                "file.contains" => Ok(contains(body, needle)),
                "path.contains" | "path.regex" => Ok(contains(path, needle)),
                other => Err(format!("unexpected Native leaf {other}")),
            }
        }
        other @ (LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_)) => {
            Err(format!("unexpected lowered shape {other:?}"))
        }
    }
}

/// Lowering preserves the product meaning over generated queries.
///
/// The lowered Native expression selects exactly the files the reference
/// selects, and a query is refused exactly when its distinct terms need more
/// leaves of one kind than the Native bound.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "generated equivalence assertions compare against an independent reference"
)]
fn lowering_matches_a_reference_evaluator_over_generated_queries() -> TestResult {
    const NEEDLES: [&str; 6] = ["a", "b", "ab", "Ba", "B", "bb"];
    let mut seeded = Seeded(0x51ce_b9b9_0c0d_e5ea);
    let mut refused = 0_u32;
    for _ in 0..600 {
        let sensitive = seeded.pick(&[true, false, false])?;
        let mut terms = Vec::new();
        let mut text = Vec::new();
        // [file.contains, path.contains, content regex, path.regex]
        let mut spent = [0_usize; 4];
        for _ in 0..seeded.pick(&[1, 2, 3, 4, 5, 6, 7])? {
            let needle = seeded.pick(&NEEDLES)?;
            let regex = seeded.pick(&[true, false])?;
            let scope = seeded.pick(&[TermScope::Both, TermScope::Content, TermScope::Path])?;
            let body = if regex {
                format!("regex:/{needle}/")
            } else {
                needle.to_owned()
            };
            let word = match scope {
                TermScope::Both => body,
                TermScope::Content => format!("content:{body}"),
                TermScope::Path => format!("path:{body}"),
            };
            // A repeated term is the same conjunct: it spends no new leaf.
            if text.contains(&word) {
                continue;
            }
            text.push(word);
            let (content_kind, path_kind) = if regex { (2, 3) } else { (0, 1) };
            for (spends, kind) in [
                (!matches!(scope, TermScope::Path), content_kind),
                (!matches!(scope, TermScope::Content), path_kind),
            ] {
                if let Some(count) = spent.get_mut(kind).filter(|_| spends) {
                    *count = count.saturating_add(1);
                }
            }
            terms.push((scope, needle));
        }
        if sensitive {
            text.push("case:yes".into());
        }
        let query = text.join(" ");
        let lowered = lower_code_search_native_query(&query);
        if spent.iter().any(|count| *count > 4) {
            refused = refused.saturating_add(1);
            assert!(
                matches!(
                    &lowered,
                    Err(CodeSearchQueryError::NativeFanout { limit: 4, .. })
                ),
                "{query}: {lowered:?}"
            );
            continue;
        }
        let lowered = lowered.map_err(|error| format!("{query}: {error}"))?;
        assert_eq!(lowered.filters, [SELECT_PATH], "{query}");
        assert_eq!(
            lowered.options.case == Some(LqCase::Sensitive),
            sensitive,
            "{query}"
        );
        for (path, body) in PROPERTY_CORPUS {
            assert_eq!(
                lowered_matches(&lowered.expr, sensitive, path, body)?,
                reference_matches(&terms, sensitive, path, body),
                "{query} over {path}"
            );
        }
    }
    // The table exercises both sides of the bound.
    assert!(refused > 0 && refused < 600, "{refused} refusals");
    Ok(())
}
