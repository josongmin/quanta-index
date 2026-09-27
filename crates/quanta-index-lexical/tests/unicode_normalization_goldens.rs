//! QI-BB-011 — one text normalizer for the Tantivy index and every sidecar.
//!
//! Before this, the keyword path tokenized through Tantivy's analyzer
//! (Unicode lowercase, `_` a boundary), the phrase sidecar split on
//! whitespace and ASCII-lowercased, and the raw/regex sidecars folded with
//! ASCII lowercase, so the same DSL query answered differently depending on
//! the leaf kind. Now `quanta_index_lq_text_normalizer` is the single
//! authority: NFC before indexing and before query lowering, per-char
//! Unicode lowercase folding, and tokens as maximal runs of
//! `char::is_alphanumeric() || '_'` (combining marks never break a token).
//!
//! This table builds `LqQuery` values directly and so exercises the adapter
//! alone; the same corpus classes go through the daemon's DSL pipeline in
//! `quanta-index-searchd-runtime/tests/e2e_unicode_text_semantics.rs`.
//!
//! The oracle is an explicit golden table over a corpus that exercises
//! punctuation adjacency, `snake_case` vs `camelCase`, accented Latin,
//! composed vs decomposed spellings, CJK, emoji boundaries, mixed scripts
//! and NFC singletons. Every row is executed through the indexed route and again
//! through the `index:no` manual scan, and both must return exactly the
//! pinned candidate set — no snapshot files, no "whatever it returns".
//!
//! Rows that pin a documented limit (no diacritic stripping, no width
//! folding, no CJK segmentation, no `ß`→`ss`, no Turkic dotted-I folding,
//! per-char final-sigma folding) say so in their label so a future normalizer
//! change updates the golden deliberately.

#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use std::collections::BTreeSet;

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqCase, LqExpr, LqLeaf, LqOptions,
    LqQuery, LqSpan, LqYesNoOnly, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
};
use quanta_index_core::{
    CoreError, LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher, RequestBudgetV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

const TOP_K: u32 = 64;

/// Decomposed `é`: `e` followed by U+0301 COMBINING ACUTE ACCENT.
const E_ACUTE_NFD: &str = "e\u{301}";
/// U+212A KELVIN SIGN, an NFC singleton that normalizes to ASCII `K`.
const KELVIN_SIGN: &str = "\u{212A}";

/// Corpus: `(candidate id, chunk text)`.
const CORPUS: &[(&str, &str)] = &[
    ("dot", "foo.bar"),
    ("dash", "foo-bar"),
    ("snake", "foo_bar"),
    ("space", "foo bar"),
    ("camel", "fooBar"),
    ("snake_long", "alpha_needle_omega"),
    ("latin_lower", "café au lait"),
    ("latin_upper", "CAFÉ AU LAIT"),
    ("latin_nfd", "cafe\u{301} au lait"),
    ("cjk_spaced", "検索 エンジン"),
    ("cjk_joined", "全文検索エンジン"),
    ("emoji_glue", "ok👍done"),
    ("emoji_only", "👍👍"),
    ("mixed", "Straße_Данные_测试 mix"),
    ("kelvin", "\u{212A}elvin scale"),
    ("fullwidth", "ｆｏｏ wide"),
    ("turkish", "İstanbul"),
    ("devanagari", "नमस्ते दुनिया"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leaf {
    Keyword,
    Phrase,
    Raw,
    Regex,
}

impl Leaf {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Phrase => "phrase",
            Self::Raw => "raw",
            Self::Regex => "regex",
        }
    }

    fn expr(self, text: &str) -> LqExpr {
        let text = text.to_string();
        LqExpr::Leaf(match self {
            Self::Keyword => LqLeaf::Keyword(text),
            Self::Phrase => LqLeaf::Phrase(text),
            Self::Raw => LqLeaf::RawString(text),
            Self::Regex => LqLeaf::Regex(text),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    /// `case:no`, the DSL default: Unicode case-folded matching.
    Insensitive,
    /// `case:yes`: byte-exact case.
    Sensitive,
}

impl Case {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Insensitive => "case:no",
            Self::Sensitive => "case:yes",
        }
    }

    const fn option(self) -> Option<LqCase> {
        match self {
            Self::Insensitive => None,
            Self::Sensitive => Some(LqCase::Sensitive),
        }
    }
}

struct Golden {
    leaf: Leaf,
    case: Case,
    query: &'static str,
    expected: &'static [&'static str],
    /// Why this row is what it is; names the documented limit when it pins one.
    label: &'static str,
}

const fn row(
    leaf: Leaf,
    case: Case,
    query: &'static str,
    expected: &'static [&'static str],
    label: &'static str,
) -> Golden {
    Golden {
        leaf,
        case,
        query,
        expected,
        label,
    }
}

const FOO_THEN_BAR: &[&str] = &["dash", "dot", "space"];
const ALL_CAFE: &[&str] = &["latin_lower", "latin_nfd", "latin_upper"];
const LOWER_CAFE: &[&str] = &["latin_lower", "latin_nfd"];

/// The golden table: `(leaf, case mode, query text) -> exact candidate ids`.
const GOLDENS: &[Golden] = &[
    // --- keyword, case:no -------------------------------------------------
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "foo",
        FOO_THEN_BAR,
        "punctuation and whitespace are token boundaries; `_` and camelCase are not",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "foo_bar",
        &["snake"],
        "snake_case identifier is one token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "foo.bar",
        FOO_THEN_BAR,
        "a keyword with an inner boundary lowers to the token sequence [foo, bar]",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "foo-bar",
        FOO_THEN_BAR,
        "`-` is a boundary exactly like `.`",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "foobar",
        &["camel"],
        "camelCase folds to one lowercase token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "FOOBAR",
        &["camel"],
        "query folding is the same Unicode lowercase as index folding",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "needle",
        &[],
        "a keyword matches whole tokens only; `needle` is inside `alpha_needle_omega`",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "alpha_needle_omega",
        &["snake_long"],
        "whole snake_case token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "café",
        ALL_CAFE,
        "accented Latin: NFC + Unicode fold unify `café`, `CAFÉ`, and decomposed `cafe\\u{301}`",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "CAFÉ",
        ALL_CAFE,
        "uppercase accented query folds with Unicode lowercase, not ASCII",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "cafe\u{301}",
        ALL_CAFE,
        "decomposed query spelling is NFC-normalized before lowering",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "cafe",
        &[],
        "documented limit: no diacritic stripping (`cafe` != `café`)",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "検索",
        &["cjk_spaced"],
        "CJK run delimited by whitespace is one token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "全文検索エンジン",
        &["cjk_joined"],
        "documented limit: no CJK segmentation, a contiguous CJK run is one token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "ok",
        &["emoji_glue"],
        "an emoji is a token boundary",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "done",
        &["emoji_glue"],
        "an emoji is a token boundary (trailing side)",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "straße_данные_测试",
        &["mixed"],
        "mixed-script identifier is one token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "STRAßE_ДАННЫЕ_测试",
        &["mixed"],
        "Cyrillic folds per Unicode; `ß` is already lowercase",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "strasse_данные_测试",
        &[],
        "documented limit: `char::to_lowercase` never expands `ß` to `ss`",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "kelvin",
        &["kelvin"],
        "NFC singleton: U+212A KELVIN SIGN normalizes to ASCII `K` at index time",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "\u{212A}elvin",
        &["kelvin"],
        "NFC singleton in the query normalizes identically",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "ｆｏｏ",
        &["fullwidth"],
        "full-width letters are alphanumeric and form a token",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "İstanbul",
        &["turkish"],
        "U+0130 folds to `i` + U+0307 on both sides",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "istanbul",
        &[],
        "documented limit: no Turkic dotted-I folding (`İ` does not fold to plain `i`)",
    ),
    row(
        Leaf::Keyword,
        Case::Insensitive,
        "नमस्ते",
        &["devanagari"],
        "combining marks (virama) never break a token",
    ),
    // --- keyword, case:yes ------------------------------------------------
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "foo",
        FOO_THEN_BAR,
        "case:yes keeps the same boundaries",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "Foo",
        &[],
        "case:yes is byte-exact on case",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "fooBar",
        &["camel"],
        "case:yes camelCase token",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "foobar",
        &[],
        "case:yes does not fold camelCase",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "CAFÉ",
        &["latin_upper"],
        "case:yes accented uppercase",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "café",
        LOWER_CAFE,
        "case:yes still NFC-normalizes: decomposed doc equals composed query",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "cafe\u{301}",
        LOWER_CAFE,
        "case:yes decomposed query equals composed doc",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "Kelvin",
        &["kelvin"],
        "case:yes after NFC singleton mapping",
    ),
    row(
        Leaf::Keyword,
        Case::Sensitive,
        "İstanbul",
        &["turkish"],
        "case:yes exact dotted capital I",
    ),
    // --- phrase, case:no --------------------------------------------------
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "foo bar",
        FOO_THEN_BAR,
        "phrase over the same token stream as the keyword path: `foo.bar` == \"foo bar\"",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "foo.bar",
        FOO_THEN_BAR,
        "phrase text is tokenized, punctuation is a boundary",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "foo_bar",
        &["snake"],
        "phrase with a snake_case token",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "FOO BAR",
        FOO_THEN_BAR,
        "phrase folds with Unicode lowercase",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "bar foo",
        &[],
        "phrase order matters",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "café au",
        ALL_CAFE,
        "phrase: NFC + Unicode fold unify composed, uppercase and decomposed docs",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "CAFÉ AU",
        ALL_CAFE,
        "phrase: uppercase accented query",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "cafe\u{301} au",
        ALL_CAFE,
        "phrase: decomposed query is NFC-normalized",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "ok done",
        &["emoji_glue"],
        "phrase: emoji is a boundary, adjacent tokens are consecutive",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "検索 エンジン",
        &["cjk_spaced"],
        "phrase over CJK tokens",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "नमस्ते दुनिया",
        &["devanagari"],
        "phrase over tokens carrying combining marks",
    ),
    row(
        Leaf::Phrase,
        Case::Insensitive,
        "kelvin scale",
        &["kelvin"],
        "phrase after NFC singleton mapping",
    ),
    // --- phrase, case:yes -------------------------------------------------
    row(
        Leaf::Phrase,
        Case::Sensitive,
        "CAFÉ AU",
        &["latin_upper"],
        "case:yes phrase",
    ),
    row(
        Leaf::Phrase,
        Case::Sensitive,
        "café au",
        LOWER_CAFE,
        "case:yes phrase, decomposed doc equals composed query",
    ),
    row(
        Leaf::Phrase,
        Case::Sensitive,
        "Foo bar",
        &[],
        "case:yes phrase is byte-exact on case",
    ),
    // --- raw string, case:no ---------------------------------------------
    row(
        Leaf::Raw,
        Case::Insensitive,
        "foo.bar",
        &["dot"],
        "raw string is a substring of the NFC text: punctuation is literal",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "foo_bar",
        &["snake"],
        "raw string: `_` is literal",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "foo",
        &["camel", "dash", "dot", "snake", "space"],
        "raw string ignores token boundaries",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "FOO",
        &["camel", "dash", "dot", "snake", "space"],
        "raw string case:no folds with the tokenizer's Unicode lowercase",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "café",
        ALL_CAFE,
        "raw string case:no: Unicode fold, not ASCII",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "CAFÉ",
        ALL_CAFE,
        "raw string case:no uppercase accented",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "cafe\u{301}",
        ALL_CAFE,
        "raw string query is NFC-normalized",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "検索",
        &["cjk_joined", "cjk_spaced"],
        "raw string finds CJK inside a longer run",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "ok👍done",
        &["emoji_glue"],
        "raw string with an emoji",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "👍",
        &["emoji_glue", "emoji_only"],
        "raw string finds a token-less document",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "straße",
        &["mixed"],
        "raw string mixed script",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "STRASSE",
        &[],
        "documented limit: raw fold never expands `ß`",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "Kelvin",
        &["kelvin"],
        "raw string over NFC text: U+212A was mapped to `K` at index time",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "\u{212A}elvin",
        &["kelvin"],
        "raw query NFC singleton",
    ),
    row(
        Leaf::Raw,
        Case::Insensitive,
        "नमस्ते",
        &["devanagari"],
        "raw string with combining marks",
    ),
    // --- raw string, case:yes --------------------------------------------
    row(
        Leaf::Raw,
        Case::Sensitive,
        "CAFÉ",
        &["latin_upper"],
        "case:yes raw string",
    ),
    row(
        Leaf::Raw,
        Case::Sensitive,
        "café",
        LOWER_CAFE,
        "case:yes raw string, decomposed doc equals composed query",
    ),
    row(
        Leaf::Raw,
        Case::Sensitive,
        "Foo",
        &[],
        "case:yes raw string is byte-exact on case",
    ),
    row(
        Leaf::Raw,
        Case::Sensitive,
        "fooBar",
        &["camel"],
        "case:yes raw string camelCase",
    ),
    // --- regex, case:no ---------------------------------------------------
    row(
        Leaf::Regex,
        Case::Insensitive,
        "foo[._-]bar",
        &["dash", "dot", "snake"],
        "regex runs over the NFC text bytes; punctuation is literal",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "foo bar",
        &["space"],
        "regex whitespace is literal",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "caf.",
        ALL_CAFE,
        "regex `.` sees one NFC char for `é`, including in the decomposed doc",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "cafe\\x{301}",
        &[],
        "documented contract: the indexed text is NFC, so a decomposed sequence never matches",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "CAFÉ",
        ALL_CAFE,
        "regex case:no is `(?i)` over the NFC text",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "検索",
        &["cjk_joined", "cjk_spaced"],
        "regex CJK",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "ok.done",
        &["emoji_glue"],
        "regex `.` sees one char for an emoji",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "👍👍",
        &["emoji_only"],
        "regex over a token-less document",
    ),
    row(
        Leaf::Regex,
        Case::Insensitive,
        "kelvin",
        &["kelvin"],
        "regex over NFC text sees the mapped `K`",
    ),
    // --- regex, case:yes --------------------------------------------------
    row(
        Leaf::Regex,
        Case::Sensitive,
        "CAFÉ",
        &["latin_upper"],
        "case:yes regex",
    ),
    row(
        Leaf::Regex,
        Case::Sensitive,
        "café",
        LOWER_CAFE,
        "case:yes regex, decomposed doc equals composed pattern",
    ),
    row(
        Leaf::Regex,
        Case::Sensitive,
        "Foo",
        &[],
        "case:yes regex is byte-exact on case",
    ),
];

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("unicode-golden-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("unicode-golden-rev").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn scope(
    index: usize,
    candidate_id: &str,
    body: &str,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let path = format!("src/doc_{index:02}.txt");
    let language = LanguageCode::new("text")
        .map_err(|err| -> Box<dyn Error> { format!("language code: {err}").into() })?;
    Ok(source_fixture::complete_file(
        source_fixture::file_key(&repo(), &path),
        &revision(),
        language.clone(),
        body.as_bytes(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(candidate_id),
            repo_relative_path: RepoRelativePath::new(&path),
            language,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    )?)
}

fn sealed_batch(corpus: &[(&str, &str)]) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = source_fixture::sealed_batch(
        &repo(),
        &revision(),
        generation(),
        corpus
            .iter()
            .enumerate()
            .map(|(index, (candidate_id, body))| scope(index, candidate_id, body))
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    batch.manifest_digest = "unicode-golden-manifest:1".into();
    Ok(batch)
}

/// One sealed generation over a corpus, held open for the test's lifetime.
struct Fixture {
    _dir: tempfile::TempDir,
    _adapter: LexicalAdapter,
    searcher: Box<dyn LexicalSearcher>,
}

fn open_corpus(corpus: &[(&str, &str)]) -> Result<Fixture, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    adapter.build_batch(&sealed_batch(corpus)?)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    Ok(Fixture {
        _dir: dir,
        _adapter: adapter,
        searcher,
    })
}

fn query(leaf: Leaf, case: Case, text: &str, index_mode: Option<LqYesNoOnly>) -> LqQuery {
    let mut options = LqOptions::defaults();
    options.case = case.option();
    options.index_mode = index_mode;
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: leaf.expr(text),
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn candidate_ids(
    searcher: &dyn LexicalSearcher,
    query: &LqQuery,
) -> Result<BTreeSet<String>, CoreError> {
    let page = searcher.search_constrained(
        query,
        &QueryConstraintSetV1::unconstrained(),
        &LexicalPageSpec::first(TOP_K),
        &RequestBudgetV1::unbounded(),
    )?;
    Ok(page
        .candidates
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect())
}

fn expected_set(expected: &[&str]) -> BTreeSet<String> {
    expected.iter().map(|id| (*id).to_string()).collect()
}

/// Every golden row, through the indexed route and the `index:no` scan.
#[test]
fn golden_table_holds_on_both_routes() -> TestResult {
    let fixture = open_corpus(CORPUS)?;
    let searcher: &dyn LexicalSearcher = fixture.searcher.as_ref();
    let mut failures: Vec<String> = Vec::new();
    for golden in GOLDENS {
        let expected = expected_set(golden.expected);
        for (route, index_mode) in [("indexed", None), ("index:no", Some(LqYesNoOnly::No))] {
            let lowered = query(golden.leaf, golden.case, golden.query, index_mode);
            match candidate_ids(searcher, &lowered) {
                Ok(actual) if actual == expected => {}
                Ok(actual) => failures.push(format!(
                    "{route} {} {} {:?}: expected {:?}, got {:?} ({})",
                    golden.leaf.as_str(),
                    golden.case.as_str(),
                    golden.query,
                    expected,
                    actual,
                    golden.label
                )),
                Err(err) => failures.push(format!(
                    "{route} {} {} {:?}: expected {:?}, got error {err} ({})",
                    golden.leaf.as_str(),
                    golden.case.as_str(),
                    golden.query,
                    expected,
                    golden.label
                )),
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} golden rows failed:\n{}",
            failures.len(),
            failures.join("\n")
        )
        .into())
    }
}

/// NFC and NFD spellings of one word are the same query, on the keyword path
/// (Tantivy index) and on the phrase path (positions sidecar), and both paths
/// agree with each other on the same word.
#[test]
fn composed_and_decomposed_spellings_agree_across_keyword_and_phrase() -> TestResult {
    let fixture = open_corpus(CORPUS)?;
    let searcher: &dyn LexicalSearcher = fixture.searcher.as_ref();
    let composed = "café";
    let decomposed = format!("caf{E_ACUTE_NFD}");
    let expected = expected_set(ALL_CAFE);
    for case in [Case::Insensitive] {
        for leaf in [Leaf::Keyword, Leaf::Phrase] {
            let nfc = candidate_ids(searcher, &query(leaf, case, composed, None))?;
            let nfd = candidate_ids(searcher, &query(leaf, case, &decomposed, None))?;
            if nfc != expected || nfd != expected {
                return Err(format!(
                    "{} {}: NFC {nfc:?} / NFD {nfd:?}, expected {expected:?}",
                    leaf.as_str(),
                    case.as_str()
                )
                .into());
            }
        }
    }
    // The Kelvin singleton: keyword (index) and phrase (sidecar) see the same
    // NFC text.
    for spelling in ["kelvin", &format!("{KELVIN_SIGN}elvin")] {
        let keyword = candidate_ids(
            searcher,
            &query(Leaf::Keyword, Case::Insensitive, spelling, None),
        )?;
        let phrase = candidate_ids(
            searcher,
            &query(Leaf::Phrase, Case::Insensitive, spelling, None),
        )?;
        let expected = expected_set(&["kelvin"]);
        if keyword != expected || phrase != expected {
            return Err(format!(
                "{spelling:?}: keyword {keyword:?} / phrase {phrase:?}, expected {expected:?}"
            )
            .into());
        }
    }
    Ok(())
}

/// The folded sidecar copy and the folded index field agree on every word.
///
/// For every word in the corpus, a single-token keyword (inverted index) and
/// a single-token phrase (position sidecar) return the same set, both
/// case-insensitively and case-sensitively.
#[test]
fn keyword_index_and_phrase_sidecar_agree_on_every_corpus_token() -> TestResult {
    let fixture = open_corpus(CORPUS)?;
    let searcher: &dyn LexicalSearcher = fixture.searcher.as_ref();
    let words = [
        "foo",
        "bar",
        "foo_bar",
        "fooBar",
        "FOOBAR",
        "alpha_needle_omega",
        "café",
        "CAFÉ",
        "Café",
        "au",
        "LAIT",
        "検索",
        "エンジン",
        "全文検索エンジン",
        "ok",
        "done",
        "Straße_Данные_测试",
        "straße_данные_测试",
        "mix",
        "Kelvin",
        "kelvin",
        "scale",
        "ｆｏｏ",
        "wide",
        "İstanbul",
        "नमस्ते",
        "दुनिया",
    ];
    let mut disagreements: Vec<String> = Vec::new();
    for word in words {
        for case in [Case::Insensitive, Case::Sensitive] {
            let keyword = candidate_ids(searcher, &query(Leaf::Keyword, case, word, None))?;
            let phrase = candidate_ids(searcher, &query(Leaf::Phrase, case, word, None))?;
            if keyword != phrase {
                disagreements.push(format!(
                    "{word:?} {}: keyword {keyword:?} != phrase {phrase:?}",
                    case.as_str()
                ));
            }
            if keyword.is_empty() && case == Case::Insensitive {
                disagreements.push(format!(
                    "{word:?} {}: a corpus token must be found by its own spelling",
                    case.as_str()
                ));
            }
        }
    }
    if disagreements.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} keyword/phrase disagreements:\n{}",
            disagreements.len(),
            disagreements.join("\n")
        )
        .into())
    }
}

fn typed_code(result: &Result<BTreeSet<String>, CoreError>) -> Option<&str> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.as_wire_str()),
        Ok(_) | Err(_) => None,
    }
}

/// A keyword or phrase whose text carries no searchable token is refused
/// typed on both routes rather than answered with an empty page; the raw
/// string surface is the byte-semantics way to ask for it.
#[test]
fn token_less_text_query_is_refused_typed_on_both_routes() -> TestResult {
    let fixture = open_corpus(CORPUS)?;
    let searcher: &dyn LexicalSearcher = fixture.searcher.as_ref();
    for (leaf, text) in [
        (Leaf::Keyword, "👍"),
        (Leaf::Keyword, "..."),
        (Leaf::Phrase, "👍 👍"),
        (Leaf::Phrase, "  "),
    ] {
        for index_mode in [None, Some(LqYesNoOnly::No)] {
            let result = candidate_ids(searcher, &query(leaf, Case::Insensitive, text, index_mode));
            if typed_code(&result) != Some("LEX_TEXT_QUERY_NO_TOKENS") {
                return Err(format!(
                    "{} {text:?} index_mode={index_mode:?}: expected typed LEX_TEXT_QUERY_NO_TOKENS, got {result:?}",
                    leaf.as_str()
                )
                .into());
            }
        }
    }
    // The same bytes are a legitimate raw-string query.
    let raw = candidate_ids(searcher, &query(Leaf::Raw, Case::Insensitive, "👍", None))?;
    if raw != expected_set(&["emoji_glue", "emoji_only"]) {
        return Err(format!("raw 👍: got {raw:?}").into());
    }
    Ok(())
}

/// An over-long token is refused typed on the token surfaces and found by raw.
///
/// A token longer than the normalizer's byte cap is never indexed as a
/// term, so the keyword and phrase surfaces refuse it instead of returning
/// an empty page, while the raw string surface still finds the bytes.
#[test]
fn over_long_token_is_refused_on_token_surfaces_and_found_by_raw() -> TestResult {
    let long = "a".repeat(300);
    let corpus: Vec<(&str, &str)> = vec![("long", long.as_str()), ("short", "aaa bbb")];
    let fixture = open_corpus(&corpus)?;
    let searcher: &dyn LexicalSearcher = fixture.searcher.as_ref();
    for (leaf, index_mode) in [
        (Leaf::Keyword, None),
        (Leaf::Keyword, Some(LqYesNoOnly::No)),
        (Leaf::Phrase, None),
        (Leaf::Phrase, Some(LqYesNoOnly::No)),
    ] {
        let result = candidate_ids(searcher, &query(leaf, Case::Insensitive, &long, index_mode));
        if typed_code(&result) != Some("LEX_TEXT_QUERY_TOKEN_TOO_LONG") {
            return Err(format!(
                "{} index_mode={index_mode:?}: expected typed LEX_TEXT_QUERY_TOKEN_TOO_LONG, got {result:?}",
                leaf.as_str()
            )
            .into());
        }
    }
    let raw = candidate_ids(searcher, &query(Leaf::Raw, Case::Insensitive, &long, None))?;
    if raw != expected_set(&["long"]) {
        return Err(format!("raw long token: got {raw:?}").into());
    }
    // The short document is unaffected.
    let short = candidate_ids(
        searcher,
        &query(Leaf::Keyword, Case::Insensitive, "bbb", None),
    )?;
    if short != expected_set(&["short"]) {
        return Err(format!("keyword bbb: got {short:?}").into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Storage compatibility: a generation built under the previous normalizer
// is refused typed, never served with the new query semantics.
// ---------------------------------------------------------------------------

const MANIFEST_FILE: &str = "search-corpus-generation-manifest.cbor";
const FORMAT_UNSUPPORTED: &str = "GENERATION_MANIFEST_FORMAT_UNSUPPORTED";
const NORMALIZER_UNSUPPORTED: &str = "GENERATION_NORMALIZER_UNSUPPORTED";

fn generation_dir(root: &std::path::Path, generation: ManifestGeneration) -> std::path::PathBuf {
    quanta_index_core::GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
        .generation_dir(root, generation)
}

fn identity(generation: ManifestGeneration) -> quanta_index_contract::GenerationSnapshot {
    quanta_index_contract::GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
        manifest_generation: generation,
        manifest_digest: "unicode-golden-manifest:1".to_string(),
    }
}

/// The manifest row as the sealed generation wrote it, decoded generically so
/// the test can rewrite it into an earlier shape without knowing the crate's
/// private types.
fn read_manifest_row(path: &std::path::Path) -> Result<Vec<ciborium::Value>, Box<dyn Error>> {
    let bytes = std::fs::read(path)?;
    match ciborium::from_reader::<ciborium::Value, _>(bytes.as_slice())? {
        ciborium::Value::Array(items) => Ok(items),
        // `ciborium::Value` is `#[non_exhaustive]`; the trailing wildcard is
        // the crate's required escape hatch, not a swallowed variant.
        other @ (ciborium::Value::Integer(_)
        | ciborium::Value::Bytes(_)
        | ciborium::Value::Float(_)
        | ciborium::Value::Text(_)
        | ciborium::Value::Bool(_)
        | ciborium::Value::Null
        | ciborium::Value::Tag(..)
        | ciborium::Value::Map(_)
        | _) => Err(format!("manifest is not a CBOR array: {other:?}").into()),
    }
}

fn write_manifest_row(path: &std::path::Path, row: Vec<ciborium::Value>) -> TestResult {
    let mut bytes = Vec::new();
    ciborium::into_writer(&ciborium::Value::Array(row), &mut bytes)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// What both doors answered: the activation validator and a query open.
fn knock(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
) -> (Option<String>, Option<String>) {
    use quanta_index_core::GenerationIdentityValidatePort as _;
    let validate = match adapter.validate_generation_identity(&identity(generation)) {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        Ok(()) | Err(_) => None,
    };
    let open = match adapter.open(&repo(), &revision(), generation) {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        Ok(_) | Err(_) => None,
    };
    (validate, open)
}

fn expect_both_doors(
    adapter: &LexicalAdapter,
    generation: ManifestGeneration,
    what: &str,
    code: &str,
) -> TestResult {
    let (validate, open) = knock(adapter, generation);
    if validate.as_deref() != Some(code) || open.as_deref() != Some(code) {
        return Err(format!(
            "{what}: expected typed {code} from both doors, validator answered {validate:?}, open answered {open:?}"
        )
        .into());
    }
    Ok(())
}

/// The manifest row rewritten as format 1: the same two leading elements,
/// format version 1, and none of the sections a later format added.
fn as_format_one(current: &[ciborium::Value]) -> Vec<ciborium::Value> {
    let mut legacy: Vec<ciborium::Value> = current.iter().take(2).cloned().collect();
    if let Some(version) = legacy.first_mut() {
        *version = ciborium::Value::from(1_u32);
    }
    legacy
}

/// The current manifest row's element count: format version, identity
/// digest, normalizer stamp, index meta, segment verification policy,
/// index segments, ranked keys, text authority, overlays, source-file coverage.
const MANIFEST_ROW_LEN: usize = 10;
/// Position of the normalizer stamp in the current manifest row.
const MANIFEST_NORMALIZER_INDEX: usize = 2;
/// The manifest format this build seals.
const CURRENT_FORMAT: u32 = 8;

/// Generations sealed under earlier manifest formats are refused typed by
/// both doors.
///
/// Format 1 is the pre-normalizer layout (no normalizer stamp); formats 4
/// and 5 are this row shape over indexes that lacked a fast column this
/// build ranks or restricts by (the text-authority doc id; the page order).
/// Format 6 predates the source-file coverage commitment; format 7 predates
/// the ranked-key commitment.
/// The validator and the query open both answer
/// `GENERATION_MANIFEST_FORMAT_UNSUPPORTED` for each, and the intact
/// current manifest is admitted again once restored.
#[test]
fn a_generation_sealed_under_the_previous_format_is_refused_typed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    adapter.build_batch(&sealed_batch(CORPUS)?)?;
    let manifest = generation_dir(dir.path(), generation()).join(MANIFEST_FILE);
    let original = std::fs::read(&manifest)?;
    let current = read_manifest_row(&manifest)?;
    if current.len() != MANIFEST_ROW_LEN
        || current.first() != Some(&ciborium::Value::from(CURRENT_FORMAT))
    {
        return Err(format!("unexpected current manifest row shape: {current:?}").into());
    }

    for earlier in [4_u32, 5, 6, 7] {
        let mut downgraded = current.clone();
        if let Some(version) = downgraded.first_mut() {
            *version = ciborium::Value::from(earlier);
        }
        write_manifest_row(&manifest, downgraded)?;
        expect_both_doors(
            &adapter,
            generation(),
            &format!("format {earlier} manifest"),
            FORMAT_UNSUPPORTED,
        )?;
    }

    write_manifest_row(&manifest, as_format_one(&current))?;
    expect_both_doors(
        &adapter,
        generation(),
        "format 1 manifest",
        FORMAT_UNSUPPORTED,
    )?;

    std::fs::write(&manifest, &original)?;
    let restored = candidate_ids(
        adapter.open(&repo(), &revision(), generation())?.as_ref(),
        &query(Leaf::Keyword, Case::Insensitive, "café", None),
    )?;
    if restored != expected_set(ALL_CAFE) {
        return Err(format!("restored manifest served {restored:?}").into());
    }
    Ok(())
}

/// A current manifest that names another normalizer version is refused
/// under `GENERATION_NORMALIZER_UNSUPPORTED` by both doors.
#[test]
fn a_generation_stamped_with_another_normalizer_is_refused_typed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    adapter.build_batch(&sealed_batch(CORPUS)?)?;
    let manifest = generation_dir(dir.path(), generation()).join(MANIFEST_FILE);
    let original = std::fs::read(&manifest)?;
    let mut stamped = read_manifest_row(&manifest)?;
    let stamp = stamped
        .get_mut(MANIFEST_NORMALIZER_INDEX)
        .ok_or("manifest row has no normalizer stamp")?;
    *stamp = ciborium::Value::Array(vec![
        ciborium::Value::from(1_u16),
        ciborium::Value::from(0_u16),
    ]);
    write_manifest_row(&manifest, stamped)?;
    expect_both_doors(
        &adapter,
        generation(),
        "normalizer 1.0 stamp",
        NORMALIZER_UNSUPPORTED,
    )?;
    std::fs::write(&manifest, &original)?;
    let (validate, open) = knock(&adapter, generation());
    if validate.is_some() || open.is_some() {
        return Err(format!("restored manifest still refused: {validate:?} / {open:?}").into());
    }
    Ok(())
}

/// A delta batch may not inherit a base sealed under the previous format:
/// the carry-forward would copy an index and sidecars built with other text
/// semantics under a fresh seal.
#[test]
fn a_delta_over_a_previous_format_base_is_refused_typed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    adapter.build_batch(&sealed_batch(CORPUS)?)?;
    let manifest = generation_dir(dir.path(), generation()).join(MANIFEST_FILE);
    let current = read_manifest_row(&manifest)?;
    write_manifest_row(&manifest, as_format_one(&current))?;

    let mut delta = sealed_batch(&[("fresh", "fresh text")])?;
    delta.generation = ManifestGeneration::new(2);
    delta.base_generation = Some(generation());
    delta.mode = BatchIngestMode::Delta;
    delta.manifest_digest = "unicode-golden-manifest:2".to_string();
    delta.source_event.event_id = "event-2".into();
    delta.source_event.expected_base_event_id = Some("event-1".into());
    delta.source_event.payload_sha256 = quanta_index_contract::source_event_payload_sha256(&delta)?;
    match adapter.build_batch(&delta) {
        Err(CoreError::Typed { code, .. }) if code.as_wire_str() == FORMAT_UNSUPPORTED => Ok(()),
        other => Err(format!("delta over a format-1 base answered {other:?}").into()),
    }
}
