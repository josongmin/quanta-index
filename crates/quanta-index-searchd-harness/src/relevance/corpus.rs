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
    /// `query_semantic` (pure vector ranking, no lexical scope) over the
    /// semantic corpus; doc id = the candidate's repo-relative path AFTER
    /// same-path collapse to the earliest occurrence, so one file cannot hold
    /// multiple head ranks via repeated chunk hits.
    Semantic,
}

impl RelevanceRoute {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
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

// ---------------------------------------------------------------------------
// Semantic relevance fixture (RFC jun-23-embedding-pipeline-sota P1-3).
// ---------------------------------------------------------------------------

/// Repo id under which the semantic relevance fixture is ingested.
///
/// Separate from `RELEVANCE_REPO`: the semantic grades are NOT BM25-calibrated
/// and the lexical corpus MUST NOT be retrofitted (RFC §7).
pub const SEMANTIC_RELEVANCE_REPO: &str = "repo-relevance-semantic";

/// `(path, content)` semantic fixture, file-granular (one chunk per file is fine
/// — RFC §8 accepts file granularity for the first gate).
///
/// Two layers in one manifest (RFC §5 P1-3), distinguished by `intent_kind` on
/// the judged query, NOT by which corpus they live in:
///
/// - **CI mechanics layer** — `auth/token_refresh.rs` is the on-topic file for an
///   EXACT-TOKEN query ("refresh the auth token"); its tokens appear verbatim, so
///   the deterministic hash embedder genuinely retrieves it. `auth/login.rs` is a
///   sibling-symbol near-neighbour. `util/string_pad.rs` is an unambiguous
///   off-topic negative.
/// - **Local discriminative layer** — `cache/eviction.rs` is the on-topic file for
///   a PARAPHRASE query ("remove stale entries from the in-memory store") whose
///   wording shares little vocabulary with the file body (`evict`, `purge`,
///   `expire`); `db/migration.rs` is a lexical-trap negative that shares the
///   surface token `entries`/`store` but is unrelated. This layer is the one a
///   real neural embedder can separate; hash is expected to do poorly on it, so
///   the CI test asserts only determinism/mechanics over it, never nDCG.
///
/// `auth/token_refresh.rs` is intentionally ingested as MULTIPLE chunks by the
/// semantic test so the same-path-collapse rule has something to collapse.
pub const SEMANTIC_RELEVANCE_CORPUS: &[(&str, &str)] = &[
    (
        "auth/token_refresh.rs",
        "/// Refresh the auth token before it expires.\n\
         pub fn refresh_auth_token(session: &mut Session) -> AuthToken {\n    \
             let token = mint_auth_token(&session.refresh_token);\n    \
             session.auth_token = token.clone();\n    \
             token\n}\n",
    ),
    (
        "auth/login.rs",
        "/// Authenticate a user and open a session.\n\
         pub fn login(creds: &Credentials) -> Session {\n    \
             let session = Session::open(creds);\n    \
             session\n}\n",
    ),
    (
        "util/string_pad.rs",
        "/// Left-pad a string to a fixed width with spaces.\n\
         pub fn left_pad(input: &str, width: usize) -> String {\n    \
             format!(\"{input:>width$}\")\n}\n",
    ),
    (
        "cache/eviction.rs",
        "/// Evict and purge expired records held in the bounded LRU.\n\
         pub fn evict_expired(lru: &mut Lru) {\n    \
             lru.purge_expired();\n    \
             lru.compact();\n}\n",
    ),
    (
        "db/migration.rs",
        "/// Insert new entries into the persistent store during a migration.\n\
         pub fn migrate_entries(store: &mut Store, entries: Vec<Row>) {\n    \
             for row in entries { store.insert_row(row); }\n}\n",
    ),
];

/// How a judged semantic query is expected to behave on the deterministic hash
/// embedder, separating the two RFC layers as a code-level label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticIntentKind {
    /// Exact-token query: tokens appear verbatim in the on-topic file, so the
    /// hash embedder genuinely retrieves it. Recall is a fair CI assertion.
    ExactToken,
    /// Paraphrase/synonym query: low lexical overlap with the on-topic file. A
    /// neural embedder can separate this; hash cannot, so only determinism /
    /// mechanics are asserted in CI, never recall or nDCG.
    Paraphrase,
}

/// One judged semantic query (the semantic-route SSOT, kept apart from the
/// lexical `JUDGED_QUERIES` so the BM25-calibrated grades never bleed in).
#[derive(Clone, Copy, Debug)]
pub struct SemanticJudgedQuery {
    /// Stable query id (artifact key + regression anchor).
    pub id: &'static str,
    /// Which RFC layer / hash-behaviour class this query belongs to.
    pub intent_kind: SemanticIntentKind,
    /// Human intent label.
    pub intent: &'static str,
    /// Exact query text issued to the engine.
    pub query: &'static str,
    /// The single on-topic file the query targets (file-granular gold).
    pub on_topic_path: &'static str,
    /// A clear off-topic / lexical-trap negative that must not be the on-topic
    /// answer.
    pub hard_negative_path: &'static str,
}

/// The semantic queries that are GATED in the relevance rail's artifact
/// (`run_relevance_report`), as opposed to the determinism-only unit coverage.
///
/// Only the `ExactToken` layer is gated: its tokens appear verbatim in the
/// on-topic file, so the deterministic `Hash` embedder genuinely retrieves it and
/// `Recall@20 = 1.0` is a FAIR, achievable assertion (RFC §5 P1-3). The paraphrase
/// layer is intentionally absent here — hash cannot recall it, so gating it would
/// be either a false-green (floored recall) or a guaranteed red; it is covered as
/// a determinism-only unit test instead. MRR/NDCG stay floored at `0.0` in
/// `thresholds_for(Semantic)` because head-rank ordering is not a hash property;
/// neural-quality separation lives in the OpenAI-gated local A/B, not in CI.
///
/// Doc grades / negative path mirror `SEMANTIC_JUDGED_QUERIES`'s `ExactToken`
/// entry (`auth/token_refresh.rs` on-topic, `util/string_pad.rs` off-topic).
pub const SEMANTIC_GATED_QUERIES: &[JudgedQuery] = &[JudgedQuery {
    id: "sem.refresh_auth_token.exact_token_recall",
    route: RelevanceRoute::Semantic,
    intent: "semantic route retrieves the exact-token auth-token-refresh file (hash-fair recall gate)",
    query: "refresh auth token expires",
    syntax: BenchSyntax::Native,
    judgments: &[("auth/token_refresh.rs", 3), ("util/string_pad.rs", 0)],
    ordering: OrderingInvariants {
        // Non-vacuity (R-TEST-19/20): `Recall@20` alone is NOT discriminative on a
        // <20-doc fixture (every doc is always retrieved), so the load-bearing
        // gated fact is the TOP-1 rank. For a single-best-match EXACT-TOKEN query,
        // one file carries verbatim multi-token overlap (`refresh`/`auth`/`token`/
        // `expires`) while the rest share ~none, so the deterministic hash ranker
        // puts it at rank 1 — a fair hash property here (distinct from the
        // same-grade tie ordering the RFC §5 P1-3 caveat is about; MRR/NDCG stay
        // floored at 0.0 so no tie-order claim is made). The off-topic negative,
        // sharing zero query tokens, must stay out of rank 1.
        top1: Some("auth/token_refresh.rs"),
        top_k_contains: &["auth/token_refresh.rs"],
        forbidden_within: &[("util/string_pad.rs", 2)],
    },
}];

/// The judged semantic query set (determinism / mechanics unit coverage; the
/// `ExactToken` subset is additionally gated via [`SEMANTIC_GATED_QUERIES`]).
pub const SEMANTIC_JUDGED_QUERIES: &[SemanticJudgedQuery] = &[
    SemanticJudgedQuery {
        id: "sem.refresh_auth_token.exact",
        intent_kind: SemanticIntentKind::ExactToken,
        intent: "locate the file that refreshes the auth token (exact-token query)",
        query: "refresh auth token expires",
        on_topic_path: "auth/token_refresh.rs",
        hard_negative_path: "util/string_pad.rs",
    },
    SemanticJudgedQuery {
        id: "sem.evict_cache.paraphrase",
        intent_kind: SemanticIntentKind::Paraphrase,
        intent: "find where stale in-memory entries are removed (paraphrase / low overlap)",
        query: "remove stale entries from the in-memory store",
        on_topic_path: "cache/eviction.rs",
        hard_negative_path: "db/migration.rs",
    },
];

// ---------------------------------------------------------------------------
// Sourcegraph lexical overlap suite (J7Q-01B).
// ---------------------------------------------------------------------------

/// One Sourcegraph lexical overlap bucket.
///
/// The overlap suite compares quanta-index against Sourcegraph lexical ONLY on
/// shipped, non-semantic query families (the `COMMAND_AND_ARTIFACT_CONTRACT`
/// minimum-bucket set). Each bucket pins the exact query text, its syntax
/// dialect, and the Sourcegraph surface the comparison would use. The
/// quanta-index ordering is captured live against the seeded relevance corpus;
/// the Sourcegraph ordering stays `unprovisioned` until a local instance exists,
/// so the per-bucket verdict is `unprovisioned` — an external comparison cannot
/// be claimed without the external system, and no "beats Sourcegraph" claim can
/// be derived from a half-captured row.
#[derive(Clone, Copy, Debug)]
pub struct SourcegraphOverlapBucket {
    /// Canonical overlap-bucket name (the contract's minimum set).
    pub bucket: &'static str,
    /// Human query-family label.
    pub query_family: &'static str,
    /// Exact query text issued to both surfaces.
    pub query: &'static str,
    /// Query syntax dialect.
    pub syntax: BenchSyntax,
    /// The Sourcegraph search surface the comparison would use.
    pub sourcegraph_surface: &'static str,
}

/// The six minimum overlap buckets (`COMMAND_AND_ARTIFACT_CONTRACT`).
///
/// Each is pinned to a query that exercises the seeded relevance corpus so the
/// quanta-index half is a real captured ordering rather than prose. The
/// Sourcegraph syntax forms (`"phrase"`, `/regex/`, `repo:`, `path:`) are the
/// dialects the live lexical route already executes.
pub const SOURCEGRAPH_OVERLAP_BUCKETS: &[SourcegraphOverlapBucket] = &[
    SourcegraphOverlapBucket {
        bucket: "keyword",
        query_family: "literal keyword match",
        query: "parse_config",
        syntax: BenchSyntax::Native,
        sourcegraph_surface: "sourcegraph lexical (literal pattern)",
    },
    SourcegraphOverlapBucket {
        bucket: "phrase",
        query_family: "contiguous multi-token phrase",
        query: "\"Parse the on-disk config\"",
        syntax: BenchSyntax::Sourcegraph,
        sourcegraph_surface: "sourcegraph lexical (literal phrase)",
    },
    SourcegraphOverlapBucket {
        bucket: "regex",
        query_family: "regex pattern",
        query: "patterntype:regexp conn[a-z]+",
        syntax: BenchSyntax::Sourcegraph,
        sourcegraph_surface: "sourcegraph lexical (patterntype:regexp)",
    },
    SourcegraphOverlapBucket {
        bucket: "path-constrained content",
        query_family: "path-scoped content match",
        query: "path:src/config/parser.rs parse_config",
        syntax: BenchSyntax::Sourcegraph,
        sourcegraph_surface: "sourcegraph lexical (path: + content)",
    },
    SourcegraphOverlapBucket {
        bucket: "repo metadata",
        query_family: "repo-scoped content match",
        query: "repo:repo-relevance connect",
        syntax: BenchSyntax::Sourcegraph,
        sourcegraph_surface: "sourcegraph lexical (repo: predicate)",
    },
    SourcegraphOverlapBucket {
        bucket: "symbol name",
        query_family: "symbol-name identifier (lexical surface stands in until a seeded symbol-route corpus exists)",
        query: "connect",
        syntax: BenchSyntax::Sourcegraph,
        sourcegraph_surface: "sourcegraph lexical (symbol-name token)",
    },
];
