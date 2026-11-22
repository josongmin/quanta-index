//! QI-BB-011 — one text normalizer for the whole DSL pipeline, through the
//! pinned Legacy daemon.
//!
//! The lexical adapter's golden table (`unicode_normalization_goldens`)
//! builds `LqQuery` values directly and so never sees the query DSL. This
//! rail sends the *text* of each query through the daemon — Native and
//! Sourcegraph syntax, with and without `case:no`, with and without
//! `index:no` — and pins the exact document set for the golden corpus
//! classes: Greek final sigma, composed / decomposed / uppercase Latin,
//! CJK, emoji boundaries, the Kelvin-sign NFC singleton, punctuation,
//! `snake_case` and `camelCase`.
//!
//! The Greek rows are the regression: the DSL normalizer used to fold a
//! `case:no` query (or any query beside a `(?i)` regex) with
//! `str::to_lowercase`, whose final-sigma rule turned `ΟΔΟΣ` into `οδος`
//! while the index held the per-character fold `οδοσ`, so the same word
//! matched or not depending on whether `case:no` was spelled out. Now the
//! DSL records the option and the text normalizer folds exactly once.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::{error::Error, time::Duration};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, publish_legacy_source},
    private_tempdir,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

const TOP_K: u32 = 64;

/// Corpus: `(label, path, text)`. One chunk per path.
const CORPUS: &[(&str, &str, &str)] = &[
    ("greek_upper", "docs/greek_upper.txt", "ΟΔΟΣ"),
    ("greek_lower_final", "docs/greek_lower.txt", "οδος"),
    ("latin_lower", "docs/latin_lower.txt", "café au lait"),
    ("latin_upper", "docs/latin_upper.txt", "CAFÉ AU LAIT"),
    ("latin_nfd", "docs/latin_nfd.txt", "cafe\u{301} au lait"),
    ("cjk_spaced", "docs/cjk_spaced.txt", "検索 エンジン"),
    ("cjk_joined", "docs/cjk_joined.txt", "全文検索エンジン"),
    ("emoji_glue", "docs/emoji_glue.txt", "ok👍done"),
    ("kelvin", "docs/kelvin.txt", "\u{212A}elvin scale"),
    ("dot", "docs/dot.txt", "foo.bar"),
    ("space", "docs/space.txt", "foo bar"),
    ("snake", "docs/snake.txt", "foo_bar"),
    ("camel", "docs/camel.txt", "fooBar"),
];

/// One golden row: the query text as a user would type it, the exact
/// labels it matches, and why.
struct Golden {
    query: &'static str,
    /// Whether the row may also be run with ` case:no` appended (rows
    /// that already spell a `case:` option cannot).
    case_option_free: bool,
    /// The syntaxes the query text is valid in: every row is Native; a row
    /// whose text has no Sourcegraph reading (the `'...'` raw-string leaf)
    /// is Native only.
    syntaxes: &'static [TextQuerySyntax],
    expected: &'static [&'static str],
    label: &'static str,
}

const BOTH_SYNTAXES: &[TextQuerySyntax] = &[TextQuerySyntax::Native, TextQuerySyntax::Sourcegraph];
const NATIVE_ONLY: &[TextQuerySyntax] = &[TextQuerySyntax::Native];

const fn row(
    query: &'static str,
    expected: &'static [&'static str],
    label: &'static str,
) -> Golden {
    Golden {
        query,
        case_option_free: true,
        syntaxes: BOTH_SYNTAXES,
        expected,
        label,
    }
}

const fn cased_row(
    query: &'static str,
    expected: &'static [&'static str],
    label: &'static str,
) -> Golden {
    Golden {
        query,
        case_option_free: false,
        syntaxes: BOTH_SYNTAXES,
        expected,
        label,
    }
}

const fn native_row(
    query: &'static str,
    expected: &'static [&'static str],
    label: &'static str,
) -> Golden {
    Golden {
        query,
        case_option_free: true,
        syntaxes: NATIVE_ONLY,
        expected,
        label,
    }
}

const ALL_CAFE: &[&str] = &["latin_lower", "latin_nfd", "latin_upper"];

const GOLDENS: &[Golden] = &[
    // --- Greek final sigma: the pipeline-level regression -----------------
    row(
        "ΟΔΟΣ",
        &["greek_upper"],
        "per-character fold on both sides: `ΟΔΟΣ` is `οδοσ` in the index and in the query",
    ),
    row(
        "οδοσ",
        &["greek_upper"],
        "the folded spelling finds the uppercase document",
    ),
    row(
        "οδος",
        &["greek_lower_final"],
        "documented limit: no final-sigma rule, `οδος` and `οδοσ` are different tokens",
    ),
    row(
        "\"ΟΔΟΣ\"",
        &["greek_upper"],
        "a phrase folds like a keyword",
    ),
    native_row(
        "ΟΔΟΣ /(?i)δ/",
        &["greek_upper"],
        "a regex's `(?i)` records `case:no` without touching the keyword beside it (the Sourcegraph route hands its regex source to the engine verbatim and refuses the flag typed)",
    ),
    cased_row(
        "ΟΔΟΣ case:yes",
        &["greek_upper"],
        "case:yes keeps the uppercase document only",
    ),
    cased_row(
        "οδοσ case:yes",
        &[],
        "case:yes finds no lowercase document spelled with a non-final sigma",
    ),
    // --- accented Latin, composed and decomposed ----------------------------
    row(
        "café",
        ALL_CAFE,
        "NFC + Unicode fold unify `café`, `CAFÉ` and decomposed `cafe\\u{301}`",
    ),
    row(
        "CAFÉ",
        ALL_CAFE,
        "an uppercase accented query folds with Unicode lowercase, not ASCII",
    ),
    row(
        "cafe\u{301}",
        ALL_CAFE,
        "a decomposed query spelling is NFC-normalized before lowering",
    ),
    row("cafe", &[], "documented limit: no diacritic stripping"),
    row("\"café au\"", ALL_CAFE, "a phrase over accented tokens"),
    cased_row(
        "CAFÉ case:yes",
        &["latin_upper"],
        "case:yes accented uppercase",
    ),
    cased_row(
        "café case:yes",
        &["latin_lower", "latin_nfd"],
        "case:yes still NFC-normalizes: the decomposed document equals the composed query",
    ),
    // --- CJK --------------------------------------------------------------
    row(
        "検索",
        &["cjk_spaced"],
        "a CJK run delimited by whitespace is one token",
    ),
    row(
        "全文検索エンジン",
        &["cjk_joined"],
        "documented limit: no CJK segmentation, a contiguous CJK run is one token",
    ),
    row(
        "\"検索 エンジン\"",
        &["cjk_spaced"],
        "a phrase over CJK tokens",
    ),
    // --- emoji boundaries ---------------------------------------------------
    row("ok", &["emoji_glue"], "an emoji is a token boundary"),
    row(
        "\"ok done\"",
        &["emoji_glue"],
        "tokens around an emoji are consecutive",
    ),
    // --- NFC singleton ------------------------------------------------------
    row(
        "kelvin",
        &["kelvin"],
        "U+212A KELVIN SIGN normalizes to ASCII `K` at index time",
    ),
    row(
        "\u{212A}elvin",
        &["kelvin"],
        "the singleton in the query normalizes identically",
    ),
    // --- punctuation, snake_case, camelCase: text vs byte semantics ---------
    row(
        "foo.bar",
        &["dot", "space"],
        "a keyword with an inner boundary is the token sequence [foo, bar]",
    ),
    row(
        "foo",
        &["dot", "space"],
        "punctuation and whitespace are boundaries; `_` and camelCase are not",
    ),
    row(
        "foo_bar",
        &["snake"],
        "a snake_case identifier is one token",
    ),
    row(
        "foobar",
        &["camel"],
        "camelCase folds to one lowercase token",
    ),
    row(
        "FOOBAR",
        &["camel"],
        "query folding is the same Unicode lowercase as index folding",
    ),
    cased_row("fooBar case:yes", &["camel"], "case:yes camelCase token"),
    cased_row("foobar case:yes", &[], "case:yes does not fold camelCase"),
    native_row(
        "'foo.bar'",
        &["dot"],
        "a raw string is a substring of the NFC text: punctuation is literal (Sourcegraph syntax has no raw-string leaf)",
    ),
];

fn with_corpus(check: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult) -> TestResult {
    let parent = private_tempdir()?;
    let state = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state, 8)?;
    let result = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let files = CORPUS
            .iter()
            .zip(1_u64..)
            .map(|(&(_, path, text), owner)| (path, text, owner))
            .collect::<Vec<_>>();
        let (_head, _receipt) =
            publish_legacy_source(&client, &repo, &revision, 1, None, &files, None)?;
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q Unicode query backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        check(
            &client,
            &GenerationPin::new(repo, revision, ManifestGeneration::new(1)),
        )
    })();
    let stopped = process.stop();
    result.and(stopped)?;
    binary.verify()
}

fn query_text(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query: &str,
) -> Result<TextQueryResponse, SdkError> {
    match syntax {
        TextQuerySyntax::Native => client.lexical().query().native(query),
        TextQuerySyntax::Sourcegraph => client.lexical().query().sourcegraph(query),
        TextQuerySyntax::CodeSearch => client.lexical().query().code_search(query),
    }
    .pinned(pin.clone())
    .top_k(TOP_K)
    .execute()
}

fn observed_labels(
    result: TextQueryResponse,
    labels: &BTreeMap<&str, &'static str>,
) -> Result<BTreeSet<&'static str>, Box<dyn Error>> {
    result
        .results
        .iter()
        .map(|candidate| {
            labels
                .get(candidate.repo_relative_path.as_str())
                .copied()
                .ok_or_else(|| {
                    format!(
                        "candidate path {} is not in the corpus",
                        candidate.repo_relative_path.as_str()
                    )
                    .into()
                })
        })
        .collect()
}

/// The spellings one golden row is executed under.
fn variants(golden: &Golden) -> Vec<(TextQuerySyntax, String)> {
    let mut out = Vec::new();
    for syntax in golden.syntaxes.iter().copied() {
        out.push((syntax, golden.query.to_string()));
        out.push((syntax, format!("{} index:no", golden.query)));
        if golden.case_option_free {
            out.push((syntax, format!("{} case:no", golden.query)));
            out.push((syntax, format!("{} case:no index:no", golden.query)));
        }
    }
    out
}

#[test]
fn every_syntax_case_spelling_and_route_answers_the_golden_set() -> TestResult {
    with_corpus(|client, pin| {
        let labels = CORPUS
            .iter()
            .map(|&(label, path, _)| (path, label))
            .collect();
        let mut failures: Vec<String> = Vec::new();
        for golden in GOLDENS {
            let expected: BTreeSet<&'static str> = golden.expected.iter().copied().collect();
            for (syntax, query) in variants(golden) {
                let observed = query_text(client, pin, syntax, &query)
                    .map_err(Box::<dyn Error>::from)
                    .and_then(|result| observed_labels(result, &labels));
                match observed {
                    Ok(observed) if observed == expected => {}
                    Ok(observed) => failures.push(format!(
                        "{syntax:?} {query:?}: expected {expected:?}, got {observed:?} ({})",
                        golden.label
                    )),
                    Err(err) => failures.push(format!(
                        "{syntax:?} {query:?}: expected {expected:?}, got {err} ({})",
                        golden.label
                    )),
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{} golden spellings failed:\n{}",
                failures.len(),
                failures.join("\n")
            )
            .into())
        }
    })
}

/// `case:yes` and a regex's `(?i)` contradict each other: the Native DSL
/// refuses the query typed rather than letting one option silently win.
///
/// The Sourcegraph route does not run the DSL normalizer (the bridge
/// hands its regex source to the engine verbatim, which refuses the
/// inline flag typed), so it is not exercised here.
#[test]
fn a_regex_case_flag_beside_case_yes_is_refused_typed() -> TestResult {
    with_corpus(|client, pin| {
        match query_text(
            client,
            pin,
            TextQuerySyntax::Native,
            "ΟΔΟΣ /(?i)δ/ case:yes",
        ) {
            Err(SdkError::Remote { code, message, .. })
                if code.as_wire_str() == "PARSE_FAIL" && message.contains("contradicts") =>
            {
                Ok(())
            }
            other => Err(format!("expected the typed contradiction refusal, got {other:?}").into()),
        }
    })
}
