//! Fixed snippet corpus, queries, and pure intent metadata shared by both rails.

/// Repo id under which the snippet fixture is ingested.
pub const SNIPPET_REPO: &str = "repo-snippet";

/// Result cap requested for every snippet query.
pub const TOP_K: u32 = 10;

/// Declared maximum snippet length (bytes) a quality snippet may emit.
///
/// A snippet longer than this is, by definition, not a bounded hit-centered
/// window: the long-line intent must be truncated below this ceiling. Sized so
/// a normal multi-line code chunk passes but an unbounded long line trips.
pub const MAX_SNIPPET_LEN: usize = 240;

/// Minimum leading context (bytes) a *long-line* hit must carry.
///
/// If a long-line snippet places the hit within this many bytes of byte 0, it is
/// "pinned to the leading edge" and fails the centering gate (no leading context
/// was retained). Short snippets (whole source shorter than [`MAX_SNIPPET_LEN`])
/// are exempt: there is nothing to center because no truncation was required.
pub const MIN_LEADING_CONTEXT: usize = 16;

/// The snippet intent a judged query exercises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnippetIntent {
    /// A multi-token phrase that must appear verbatim and centered.
    Phrase,
    /// A token reachable via regex syntax; needle is the literal it resolves to.
    Regex,
    /// A token that occurs multiple times in the chunk (multi-hit count > 1).
    MultiHit,
    /// A single very long source line that MUST be truncated + centered.
    LongLine,
}

impl SnippetIntent {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Phrase => "phrase",
            Self::Regex => "regex",
            Self::MultiHit => "multi_hit",
            Self::LongLine => "long_line",
        }
    }

    /// Whether this intent's source is long enough that a bounded, centered
    /// window is *required* (vs. a short chunk that is legitimately emitted
    /// whole).
    #[must_use]
    pub fn requires_truncation(self) -> bool {
        matches!(self, Self::LongLine)
    }
}

/// One seeded snippet fixture document.
#[derive(Clone, Copy, Debug)]
pub struct SnippetFixture {
    pub path: &'static str,
    pub content: &'static str,
}

/// One judged snippet query: query text, the needle the snippet must center on,
/// and the declared intent that selects the gate.
#[derive(Clone, Copy, Debug)]
pub struct SnippetQuery {
    /// Stable query id (artifact key + regression anchor).
    pub id: &'static str,
    /// Query text issued to the engine.
    pub query: &'static str,
    /// The literal substring the snippet must contain and center on.
    pub needle: &'static str,
    /// Intent — selects the blocking gate.
    pub intent: SnippetIntent,
    /// Repo-relative path of the fixture doc the query is expected to hit.
    pub expect_path: &'static str,
}

/// A long source line whose needle is buried near the middle.
///
/// A single physical line far exceeding [`MAX_SNIPPET_LEN`], so a correct
/// snippet MUST truncate and center, and the live (full-text) snippet will
/// exceed the bound.
const LONG_LINE: &str = "let prefix_padding_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa = compute_target_marker_zzz(0) + suffix_padding_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb;\n";

/// `(path, content)` snippet fixtures, including phrase and regex controls.
pub const SNIPPET_CORPUS: &[SnippetFixture] = &[
    SnippetFixture {
        path: "src/snippet/phrase.rs",
        content: "fn header() {}\n\
                  // the quick brown fox marker sits here for the phrase intent\n\
                  fn footer() {}\n",
    },
    SnippetFixture {
        path: "src/snippet/phrase_decoy.rs",
        content: "// The marker follows a fox through brown leaves.\n",
    },
    SnippetFixture {
        path: "src/snippet/regex.rs",
        content: "fn alpha() {}\n\
                  pub fn regex_target_symbol(input: &str) -> usize { input.len() }\n\
                  fn omega() {}\n",
    },
    // Shares the keyword posting anchor, but not the signature required by
    // the regex. Dropping regex truth must leave a visible extra candidate.
    SnippetFixture {
        path: "src/snippet/regex_decoy.rs",
        content: "pub fn regex_target_symbol(input: usize) -> usize { input }\n",
    },
    SnippetFixture {
        path: "src/snippet/multi.rs",
        content: "fn repeated_marker_a() { repeated_marker_a(); repeated_marker_a(); }\n",
    },
    SnippetFixture {
        path: "src/snippet/longline.rs",
        content: LONG_LINE,
    },
];

/// The judged snippet query set (the snippet rail SSOT).
pub const SNIPPET_QUERIES: &[SnippetQuery] = &[
    SnippetQuery {
        id: "snip.phrase.brown_fox",
        query: "\"brown fox marker\"",
        needle: "brown fox marker",
        intent: SnippetIntent::Phrase,
        expect_path: "src/snippet/phrase.rs",
    },
    SnippetQuery {
        id: "snip.regex.target_symbol",
        query: r"regex_target_symbol AND /regex_target_symbol\(input: &str\)/",
        needle: "regex_target_symbol",
        intent: SnippetIntent::Regex,
        expect_path: "src/snippet/regex.rs",
    },
    SnippetQuery {
        id: "snip.multi.repeated_marker",
        query: "repeated_marker_a",
        needle: "repeated_marker_a",
        intent: SnippetIntent::MultiHit,
        expect_path: "src/snippet/multi.rs",
    },
    SnippetQuery {
        id: "snip.longline.compute_target",
        query: "compute_target_marker_zzz",
        needle: "compute_target_marker_zzz",
        intent: SnippetIntent::LongLine,
        expect_path: "src/snippet/longline.rs",
    },
];
