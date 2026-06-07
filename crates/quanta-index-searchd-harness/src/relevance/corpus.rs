//! Construct-by-seed judged relevance corpus (the rail's single source of truth).
//!
//! "What is relevant" lives here as a checked-in manifest, not as a per-run
//! human judgment. Each [`JudgedQuery`] declares its route family, intent label,
//! exact query text, a graded judgment table, and the ordering invariants a
//! correct ranker must satisfy. The fixture documents are seeded so the intended
//! ordering is unambiguous *by construction* (definition site + high term
//! frequency outranks an incidental mention, which outranks a token-overlap hard
//! negative) — the rail then checks the live ranker against that intent rather
//! than fitting gold to whatever the ranker currently emits.
//!
//! Every doc id is a stable, human-auditable key (a repo-relative path), never
//! an opaque engine-generated id, so a reviewer can read the manifest against the
//! fixture and confirm the grades by eye.

use crate::artifact::BenchSyntax;

/// The retrieval route family a judged query exercises.
///
/// Kept separate from the bench `RouteFamily` because the relevance rail reports
/// per family and the set of *rankable* families is a deliberate subset: each
/// variant here must map to a harness query method that returns an ordered,
/// path- or id-keyed result list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelevanceRoute {
    /// `query_text` over the lexical corpus; doc id = repo-relative path.
    Lexical,
}

impl RelevanceRoute {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
        }
    }
}

/// One graded judgment row: `(repo-relative path, ordinal grade)`.
///
/// grade `0` = hard negative / distractor, `1` = incidental, `2` = supporting,
/// `3` = primary (definition site / best answer).
pub type JudgmentRow = (&'static str, u8);

/// Ordering invariants a correct ranking must satisfy for one query.
///
/// These are the *blocking* shape checks, complementary to the aggregate metric
/// thresholds: they pin specific top-of-ranking facts that an averaged metric
/// could otherwise hide (e.g. a demoted top-1 masked by good tail recall).
#[derive(Clone, Copy, Debug)]
pub struct OrderingInvariants {
    /// The doc that MUST occupy rank 1 (the unambiguous best answer).
    pub top1: Option<&'static str>,
    /// Docs that MUST appear within the produced top-k.
    pub top_k_contains: &'static [&'static str],
    /// `(doc, max_rank_exclusive)` — the doc MUST NOT appear at a rank strictly
    /// below `max_rank_exclusive` (1-based). Used to keep token-overlap hard
    /// negatives out of the head of the ranking.
    pub forbidden_within: &'static [(&'static str, usize)],
}

/// A single judged query: the atomic unit of the relevance corpus.
#[derive(Clone, Copy, Debug)]
pub struct JudgedQuery {
    /// Stable query id (used as the artifact key and regression anchor).
    pub id: &'static str,
    /// Route family — selects the harness query method and the report bucket.
    pub route: RelevanceRoute,
    /// Human intent label (why these docs are relevant).
    pub intent: &'static str,
    /// Exact query text issued to the engine.
    pub query: &'static str,
    /// Query syntax dialect.
    pub syntax: BenchSyntax,
    /// Graded judgments; docs absent here are grade `0` by omission.
    pub judgments: &'static [JudgmentRow],
    /// Blocking ordering invariants.
    pub ordering: OrderingInvariants,
}

// ---------------------------------------------------------------------------
// Seeded fixture documents.
// ---------------------------------------------------------------------------

/// Repo id under which the relevance fixture is ingested.
pub const RELEVANCE_REPO: &str = "repo-relevance";

/// Result cap requested for every relevance query.
///
/// Set to 20 so `Recall@20` is observable; `MRR@10` / `NDCG@10` truncate the
/// same produced ranking at 10 inside the metric layer.
pub const TOP_K: u32 = 20;

/// `(path, content)` fixture seeded into one sealed generation.
///
/// Covers two lexical intents. Intent A — "find where `parse_config` is defined
/// and used" — is separable by construction (definition TF-dense > caller >
/// mention > token-overlap negative). Intent B — the `connect` call/def sites vs
/// a stemming distractor — is seeded at the bottom of this list. Relevance for
/// intent A is separable by construction:
///
/// - `src/config/parser.rs` — the definition site, `parse_config` appears in the
///   signature and is referenced repeatedly (primary, grade 3);
/// - `src/config/loader.rs` — a genuine call site (supporting, grade 2);
/// - `src/config/mod.rs` — a doc-comment mention only (incidental, grade 1);
/// - `src/net/client.rs` — a token-overlap HARD NEGATIVE: it contains the tokens
///   `parse` and `config` in unrelated network code but never `parse_config`
///   (grade 0, must stay out of the head);
/// - `src/config/legacy_parser.rs` — a NEAR-DUPLICATE distractor of the
///   definition site for a *different* symbol (`parse_legacy_config`), sharing
///   most tokens but not the queried symbol (grade 0).
pub const LEXICAL_RELEVANCE_CORPUS: &[(&str, &str)] = &[
    (
        "src/config/parser.rs",
        "/// Parse the on-disk config into a typed `Config`.\n\
         pub fn parse_config(raw: &str) -> Config {\n    \
             let parsed = parse_config_inner(raw);\n    \
             parse_config_validate(&parsed);\n    \
             parsed\n}\n\n\
         fn parse_config_inner(raw: &str) -> Config { Config::from_raw(raw) }\n\
         fn parse_config_validate(cfg: &Config) {}\n",
    ),
    (
        "src/config/loader.rs",
        "use crate::config::parser::parse_config;\n\n\
         pub fn load(path: &str) -> Config {\n    \
             let raw = read_to_string(path);\n    \
             parse_config(&raw)\n}\n",
    ),
    (
        "src/config/mod.rs",
        "pub mod loader;\npub mod parser;\n\
         //! Config module. See `parse_config` for the entry point.\n",
    ),
    (
        "src/net/client.rs",
        "pub fn connect(addr: &str) -> Socket {\n    \
             let parse = Url::parse(addr);\n    \
             let config = TcpConfig::default();\n    \
             Socket::open(parse, config)\n}\n",
    ),
    (
        "src/config/legacy_parser.rs",
        "/// Parse the legacy config format.\n\
         pub fn parse_legacy_config(raw: &str) -> Config {\n    \
             let parsed = parse_legacy_config_inner(raw);\n    \
             parsed\n}\n\n\
         fn parse_legacy_config_inner(raw: &str) -> Config { Config::from_raw(raw) }\n",
    ),
    // Second intent — "find the network `connect` entry point". `client.rs`
    // already defines `connect`; `pool.rs` calls it; `connection.rs` is a
    // stemming HARD NEGATIVE (`connection`/`Connected`, never `connect` as a
    // call/def of the queried symbol).
    (
        "src/net/pool.rs",
        "use crate::net::client::connect;\n\n\
         pub fn acquire(addr: &str) -> Socket {\n    \
             connect(addr)\n}\n",
    ),
    (
        "src/db/connection.rs",
        "pub struct DatabaseConnection { connected: bool }\n\
         impl DatabaseConnection {\n    \
             pub fn is_connected(&self) -> bool { self.connected }\n}\n",
    ),
];

/// The judged query set (the relevance SSOT).
///
/// Two lexical intents: a graded symbol-usage ranking (`parse_config`) and a
/// term-density retrieval with a stemming hard negative (`connect`). The report
/// is route-generic, so adding a non-lexical family (history / structural) is a
/// matter of extending this slice plus its fixture, not reworking the engine —
/// note those routes are match/filter-shaped and want their own (non-NDCG) gate
/// design rather than graded relevance.
pub const JUDGED_QUERIES: &[JudgedQuery] = &[
    JudgedQuery {
        id: "lex.parse_config.symbol_usage",
        route: RelevanceRoute::Lexical,
        intent: "locate the definition and call sites of the `parse_config` symbol",
        query: "parse_config",
        syntax: BenchSyntax::Native,
        judgments: &[
            ("src/config/parser.rs", 3),
            ("src/config/loader.rs", 2),
            ("src/config/mod.rs", 1),
            ("src/net/client.rs", 0),
            ("src/config/legacy_parser.rs", 0),
        ],
        ordering: OrderingInvariants {
            top1: Some("src/config/parser.rs"),
            top_k_contains: &[
                "src/config/parser.rs",
                "src/config/loader.rs",
                "src/config/mod.rs",
            ],
            // The token-overlap network file must never reach the top 3.
            forbidden_within: &[("src/net/client.rs", 3)],
        },
    },
    JudgedQuery {
        // Calibration lesson (kept deliberately): a PURE LEXICAL route ranks by term
        // statistics (BM25 TF / length), not by code semantics. "Definition outranks
        // caller" is a SYMBOL-route property the lexical route neither has nor owes —
        // here the caller `pool.rs` carries `connect` twice in a short body and so
        // legitimately outscores the definition `client.rs`. Grading both call/def
        // sites as primary (3) keeps the judgment honest for this route; the
        // intra-relevant order between them is intentionally NOT constrained. The
        // load-bearing lexical assertions are: both on-topic files are retrieved and
        // the `connection`/`connected` stemming distractor is excluded from the head.
        id: "lex.connect.network_entrypoint",
        route: RelevanceRoute::Lexical,
        intent: "retrieve the network `connect` call/def sites, excluding stemming distractors",
        query: "connect",
        syntax: BenchSyntax::Native,
        judgments: &[
            ("src/net/client.rs", 3),
            ("src/net/pool.rs", 3),
            ("src/db/connection.rs", 0),
        ],
        ordering: OrderingInvariants {
            top1: None,
            top_k_contains: &["src/net/client.rs", "src/net/pool.rs"],
            // The `connection`/`connected` stemming distractor must stay out of top 2.
            forbidden_within: &[("src/db/connection.rs", 2)],
        },
    },
];
