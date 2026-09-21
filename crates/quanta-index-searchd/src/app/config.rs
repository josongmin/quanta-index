use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use quanta_index_core::{
    EMBEDDING_CACHE_LEDGER_BYTES_PER_ENTRY, IngestResourcePolicy, IntegrityScrubPolicyV1,
    LexicalExecutionBudgetV1, LexicalWriterPolicy, MAX_EMBEDDING_DIMENSION,
    ProcessMemoryEnvelopeV1, RegexMatchCachePolicy, SemanticStreamWindowPolicy,
};
use quanta_index_embed::{
    DEFAULT_CONCURRENCY, DEFAULT_MAX_BATCH, DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST,
    DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT, EmbeddingCacheRetentionPolicy, MAX_CONCURRENCY,
    OpenAiProviderConfig,
};
use quanta_index_ipc::ServerAdmissionPolicy;
use quanta_index_search_plane::readiness::SearchCorpusHistoryRetentionPolicyV1;
use quanta_index_search_plane::{
    ResponsePayloadBudget, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotRegistryPolicy,
};

use crate::app::socket_access::SocketAccessPolicies;

/// Default `OpenAI` embedding model and dimension when the `openai` profile is
/// selected without explicit overrides.
const DEFAULT_OPENAI_MODEL: &str = "text-embedding-3-small";
const DEFAULT_OPENAI_DIMENSION: usize = 1536;

/// Operational knobs for the `OpenAI` embedder, surfaced as daemon env.
///
/// They can be tuned without a code change. Defaults mirror the provider's own
/// defaults (single source of truth re-exported from `quanta_index_embed`); the
/// embedding cache is on by default to control cost on rebuild / incremental.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenAiEmbedderTuning {
    /// Max inputs per `/v1/embeddings` request (`QUANTA_INDEX_EMBED_BATCH`).
    pub max_batch: usize,
    /// Conservative estimated-token budget per request
    /// (`QUANTA_INDEX_EMBED_MAX_EST_TOKENS`).
    pub max_estimated_tokens_per_request: usize,
    /// Bounded retry count for transient failures (`QUANTA_INDEX_EMBED_MAX_RETRIES`).
    pub max_retries: u32,
    /// Per-request HTTP timeout (`QUANTA_INDEX_EMBED_TIMEOUT_SECS`).
    pub timeout: Duration,
    /// Whether the on-disk embedding cache is used (`QUANTA_INDEX_EMBED_CACHE`).
    pub cache_enabled: bool,
    /// How much the on-disk embedding cache may hold (QI-BB-009):
    /// `QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES`, `QUANTA_INDEX_EMBED_CACHE_MAX_BYTES`
    /// and `QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES`.
    pub cache_retention: EmbeddingCacheRetentionPolicy,
    /// Embedding requests dispatched concurrently per batch
    /// (`QUANTA_INDEX_EMBED_CONCURRENCY`; 1 = sequential, at most
    /// [`MAX_CONCURRENCY`]).
    pub concurrency: usize,
}

impl Default for OpenAiEmbedderTuning {
    fn default() -> Self {
        Self {
            max_batch: DEFAULT_MAX_BATCH,
            max_estimated_tokens_per_request: DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
            cache_enabled: true,
            cache_retention: EmbeddingCacheRetentionPolicy::DEFAULT,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

impl OpenAiEmbedderTuning {
    /// Build the provider config for `model`/`dimension`/`api_key`, threading
    /// every tuning knob (except `cache_enabled`, which gates the cache wrapper at
    /// the composition root) onto its config setter. Owning this mapping here —
    /// instead of inline at the composition root — makes the knob->config wiring
    /// unit-testable, so a swapped or dropped field fails the test.
    #[must_use]
    pub fn provider_config(
        &self,
        model: String,
        model_revision: String,
        dimension: usize,
        api_key: String,
    ) -> OpenAiProviderConfig {
        OpenAiProviderConfig::new(api_key, model, model_revision, dimension)
            .with_max_batch(self.max_batch)
            .with_max_estimated_tokens_per_request(self.max_estimated_tokens_per_request)
            .with_max_retries(self.max_retries)
            .with_timeout(self.timeout)
            .with_concurrency(self.concurrency)
    }
}

/// The env selector for the development hash embedder (QI-BB-007): the
/// name says what it is, so no deployment picks it by omission.
pub const DEV_HASH_EMBEDDER_SELECTOR: &str = "hash-dev";
/// The env knob that lets an unset `QUANTA_INDEX_EMBEDDER` resolve to the
/// development hash embedder instead of refusing boot (QI-BB-007).
///
/// The harness and the test rails set it; a deployment names its embedder.
pub const ALLOW_DEV_EMBEDDER_ENV: &str = "QUANTA_INDEX_ALLOW_DEV_EMBEDDER";

/// How `searchd` resolves the semantic embedder for both query and corpus paths.
///
/// One profile drives both, so the two sides can never disagree on model
/// identity (the query-time model-identity gate then holds).
#[derive(Clone, Eq, PartialEq)]
pub enum SemanticEmbedderProfile {
    /// The deterministic FNV-1a hash embedder — a **development and test
    /// profile only** (QI-BB-007).
    ///
    /// It hashes tokens into slots and carries no learned semantics, so a
    /// semantic query under it is token overlap, not meaning. It is
    /// selected by name (`hash-dev`) or, with
    /// `QUANTA_INDEX_ALLOW_DEV_EMBEDDER=1`, by an unset selector; the
    /// daemon logs a boot warning and reports
    /// `boot_semantic_profile_is_dev` whenever it serves under it.
    Hash { dimension: usize },
    /// Network-backed `OpenAI` embeddings. `api_key` is held here but redacted in
    /// `Debug` (R-SEC-01) and never logged. `tuning` carries the env-resolved
    /// operational knobs threaded into the provider/cache at the composition root.
    OpenAi {
        model: String,
        /// The operator-pinned revision of `model` (QI-BB-028). `OpenAI`
        /// does not expose one, so the operator names it and rotates it
        /// when the served model changes; cache namespaces, sealed
        /// generations and the query gate all key on it.
        model_revision: String,
        dimension: usize,
        api_key: String,
        tuning: OpenAiEmbedderTuning,
    },
    /// No query-time embedder is configured: semantic/hybrid queries fail closed
    /// (`SEM_PROVIDER_UNAVAILABLE`) while the corpus still hash-derives so the
    /// generation materializes — the deliberate degraded-config contract.
    Unavailable,
}

impl std::fmt::Debug for SemanticEmbedderProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hash { dimension } => f
                .debug_struct("Hash")
                .field("dimension", dimension)
                .finish(),
            Self::OpenAi {
                model,
                model_revision,
                dimension,
                tuning,
                ..
            } => f
                .debug_struct("OpenAi")
                .field("model", model)
                .field("model_revision", model_revision)
                .field("dimension", dimension)
                .field("api_key", &"<redacted>")
                .field("tuning", tuning)
                .finish(),
            Self::Unavailable => f.write_str("Unavailable"),
        }
    }
}

impl SemanticEmbedderProfile {
    /// Whether this profile is a development/test embedder rather than a
    /// learned one (QI-BB-007).
    #[must_use]
    pub const fn is_dev(&self) -> bool {
        matches!(self, Self::Hash { .. })
    }

    /// The selector name that resolves this profile from env.
    #[must_use]
    pub const fn selector(&self) -> &'static str {
        match self {
            Self::Hash { .. } => DEV_HASH_EMBEDDER_SELECTOR,
            Self::OpenAi { .. } => "openai",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The builder default is the development hash embedder.
///
/// The harness and the in-process test rails build configs directly and
/// need a key-free, network-free embedder. A deployment resolves its
/// profile from env, where an unset selector is refused unless the
/// operator opted into the development embedder by name.
impl Default for SemanticEmbedderProfile {
    fn default() -> Self {
        Self::Hash {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        }
    }
}

/// The two process-wide memory ceilings (QI-BB-016): what every declared
/// resident byte policy must fit under together, and the resident-memory
/// level above which no new lexical writer is opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessMemoryCeilings {
    ceiling_bytes: u64,
    rss_ceiling_bytes: Option<u64>,
}

impl ProcessMemoryCeilings {
    /// The default envelope ceiling and no resident-memory gate.
    pub const DEFAULT: Self = Self {
        ceiling_bytes: ProcessMemoryEnvelopeV1::DEFAULT_CEILING_BYTES,
        rss_ceiling_bytes: None,
    };

    pub fn new(ceiling_bytes: u64, rss_ceiling_bytes: Option<u64>) -> Result<Self> {
        if ceiling_bytes == 0 {
            return Err(anyhow::anyhow!("the process memory ceiling must be at least one byte"));
        }
        if rss_ceiling_bytes == Some(0) {
            return Err(anyhow::anyhow!(
                "the resident-memory ceiling must be at least one byte when set"
            ));
        }
        Ok(Self {
            ceiling_bytes,
            rss_ceiling_bytes,
        })
    }

    #[must_use]
    pub const fn ceiling_bytes(self) -> u64 {
        self.ceiling_bytes
    }

    #[must_use]
    pub const fn rss_ceiling_bytes(self) -> Option<u64> {
        self.rss_ceiling_bytes
    }
}

/// How often the composition root's maintenance timer ticks: the idle
/// writer sweep and the per-track disk-usage refresh both run on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaintenancePolicy {
    tick: Duration,
}

impl MaintenancePolicy {
    /// Every five seconds: an idle writer is released within one tick of
    /// its idle interval, and the disk walk is far from any hot path.
    pub const DEFAULT: Self = Self {
        tick: Duration::from_secs(5),
    };

    pub fn new(tick: Duration) -> Result<Self> {
        if tick.is_zero() {
            return Err(anyhow::anyhow!("the maintenance tick must be non-zero"));
        }
        Ok(Self { tick })
    }

    #[must_use]
    pub const fn tick(self) -> Duration {
        self.tick
    }
}

/// Resolved runtime paths and policies for one `searchd` instance.
///
/// There is one way a deployment reaches this config from env: the
/// `ENV_POLICY_FAMILIES` chain (crate-private), which [`Self::from_env`] and
/// [`Self::from_env_with_state_root`] both run in full, so `--state-root`
/// and the env-resolved state root can never disagree on a policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchdConfig {
    state_root: PathBuf,
    query_socket_path: PathBuf,
    control_socket_path: PathBuf,
    /// QI-RT-01: typed ingest socket. Producer-side SDK publishes typed
    /// batches here; searchd's ingest dispatcher applies them through the
    /// direct authority path.
    ingest_socket_path: PathBuf,
    semantic_embedder_profile: SemanticEmbedderProfile,
    search_corpus_history_retention_policy: Option<SearchCorpusHistoryRetentionPolicyV1>,
    /// Residency limits for opened sealed generations (QI-BB-001). Optional
    /// operator tuning with a documented default; both env knobs must be
    /// given together and neither may be zero.
    snapshot_registry_policy: SnapshotRegistryPolicy,
    /// Most candidates one lexical execution may materialize (QI-BB-005).
    /// Optional operator tuning with a documented default; zero is refused.
    lexical_execution_budget: LexicalExecutionBudgetV1,
    /// Ingress and dispatch limits for the query socket (QI-BB-002). The
    /// control and ingest sockets are not tunable: their mutations must not
    /// interleave, so they always run [`ServerAdmissionPolicy::SERIAL_DISPATCH`].
    query_admission_policy: ServerAdmissionPolicy,
    /// Bounds for the lexical regex match cache (QI-BB-024): entries,
    /// resident bytes, and the widest match set one entry may hold.
    regex_match_cache_policy: RegexMatchCachePolicy,
    /// The resource envelope one search-corpus batch may ask the plane to
    /// hold (QI-BB-021): records, embedded text bytes and vector bytes.
    ingest_resource_policy: IngestResourcePolicy,
    /// The window the semantic track embeds and appends a batch in
    /// (QI-BB-021): owner scopes and vector bytes resident at once.
    semantic_stream_window_policy: SemanticStreamWindowPolicy,
    /// The heap every open lexical generation writer may hold together, the
    /// heap one takes, and how long an idle one is kept (QI-BB-016).
    lexical_writer_policy: LexicalWriterPolicy,
    /// Who may connect to each socket (QI-BB-014). Every socket is private
    /// unless its own knob opens it; the state root stays owner-only
    /// regardless.
    socket_access_policies: SocketAccessPolicies,
    /// The process memory ceilings every resident byte policy is validated
    /// against at boot (QI-BB-016).
    process_memory_ceilings: ProcessMemoryCeilings,
    /// The maintenance timer's cadence (QI-BB-016, QI-BB-015).
    maintenance_policy: MaintenancePolicy,
    /// How many encoded bytes one ranked lexical page may take before it
    /// is cut and continued by its cursor (QI-BB-005 보완 #5).
    query_response_budget: ResponsePayloadBudget,
    /// How the integrity scrub is paced as maintenance (QI-BB-017): at most
    /// one bounded step per interval, on the maintenance timer.
    integrity_scrub_policy: IntegrityScrubPolicyV1,
}

/// One env-driven policy family: the knobs it reads and the setter that
/// applies what they resolve to.
///
/// Every family is applied by both config entry points, in table order,
/// and the table is the only place a family is named — a new env knob
/// that is not in a family here does not reach the daemon by any path,
/// which the config tests enforce against the source.
pub(crate) struct EnvPolicyFamily {
    pub(crate) name: &'static str,
    /// Every env var the family reads, for the source fence.
    pub(crate) env_vars: &'static [&'static str],
    pub(crate) apply: fn(SearchdConfig, &EnvLookup<'_>) -> Result<SearchdConfig>,
}

/// An env lookup, injected so the chain is provable without process env.
pub(crate) type EnvLookup<'a> = dyn Fn(&str) -> Result<Option<String>> + 'a;

/// Every env policy family, in the order both entry points apply them.
pub(crate) const ENV_POLICY_FAMILIES: &[EnvPolicyFamily] = &[
    EnvPolicyFamily {
        name: "search-corpus history retention",
        env_vars: &[
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
        ],
        apply: |config, lookup| {
            Ok(config.with_search_corpus_history_retention_policy_v1(
                search_corpus_history_retention_policy_from_lookup_v1(lookup)?,
            ))
        },
    },
    EnvPolicyFamily {
        name: "semantic embedder",
        env_vars: &[
            "QUANTA_INDEX_EMBEDDER",
            ALLOW_DEV_EMBEDDER_ENV,
            "QUANTA_INDEX_EMBED_DIM",
            "QUANTA_INDEX_EMBED_MODEL",
            "QUANTA_INDEX_EMBED_MODEL_REVISION",
            "OPENAI_API_KEY",
            "QUANTA_INDEX_EMBED_BATCH",
            "QUANTA_INDEX_EMBED_MAX_EST_TOKENS",
            "QUANTA_INDEX_EMBED_MAX_RETRIES",
            "QUANTA_INDEX_EMBED_TIMEOUT_SECS",
            "QUANTA_INDEX_EMBED_CACHE",
            "QUANTA_INDEX_EMBED_CONCURRENCY",
            "QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES",
            "QUANTA_INDEX_EMBED_CACHE_MAX_BYTES",
            "QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES",
            "QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS",
            "QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES",
        ],
        apply: |config, lookup| {
            Ok(config
                .with_semantic_embedder_profile(semantic_embedder_profile_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "snapshot registry",
        env_vars: &[
            "QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES",
            "QUANTA_INDEX_SNAPSHOT_MAX_RESIDENT_BYTES",
        ],
        apply: |config, lookup| {
            Ok(config.with_snapshot_registry_policy(snapshot_registry_policy_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "lexical execution budget",
        env_vars: &["QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES"],
        apply: |config, lookup| {
            Ok(config.with_lexical_execution_budget(lexical_execution_budget_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "query admission",
        env_vars: &[
            "QUANTA_INDEX_QUERY_MAX_CONNECTIONS",
            "QUANTA_INDEX_QUERY_DISPATCH_SLOTS",
            "QUANTA_INDEX_QUERY_MAX_IN_FLIGHT_PER_REPO",
            "QUANTA_INDEX_QUERY_QUEUE_WAIT_MS",
            "QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS",
        ],
        apply: |config, lookup| {
            Ok(config.with_query_admission_policy(query_admission_policy_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "regex match cache",
        env_vars: &[
            "QUANTA_INDEX_REGEX_CACHE_MAX_ENTRIES",
            "QUANTA_INDEX_REGEX_CACHE_MAX_RESIDENT_BYTES",
            "QUANTA_INDEX_REGEX_CACHE_MAX_MATCHES_PER_ENTRY",
        ],
        apply: |config, lookup| {
            Ok(config.with_regex_match_cache_policy(regex_match_cache_policy_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "ingest resource envelope",
        env_vars: &[
            "QUANTA_INDEX_INGEST_MAX_RECORDS",
            "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
        ],
        apply: |config, lookup| {
            Ok(config.with_ingest_resource_policy(ingest_resource_policy_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "semantic stream window",
        env_vars: &[
            "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES",
            "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES",
        ],
        apply: |config, lookup| {
            Ok(config.with_semantic_stream_window_policy(
                semantic_stream_window_policy_from_lookup(lookup)?,
            ))
        },
    },
    EnvPolicyFamily {
        name: "lexical writer envelope",
        env_vars: &[
            "QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES",
            "QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES",
            "QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS",
        ],
        apply: |config, lookup| {
            Ok(config.with_lexical_writer_policy(lexical_writer_policy_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "socket access",
        env_vars: &[
            "QUANTA_INDEX_QUERY_SOCKET_ACCESS",
            "QUANTA_INDEX_CONTROL_SOCKET_ACCESS",
            "QUANTA_INDEX_INGEST_SOCKET_ACCESS",
        ],
        apply: |config, lookup| {
            Ok(config.with_socket_access_policies(
                crate::app::socket_access::socket_access_policies_from_lookup(
                    lookup,
                    &crate::app::socket_access::SystemPrincipals,
                )?,
            ))
        },
    },
    EnvPolicyFamily {
        name: "process memory ceilings",
        env_vars: &[
            "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES",
            "QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES",
        ],
        apply: |config, lookup| {
            Ok(config.with_process_memory_ceilings(process_memory_ceilings_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "maintenance",
        env_vars: &["QUANTA_INDEX_MAINTENANCE_TICK_MS"],
        apply: |config, lookup| {
            Ok(config.with_maintenance_policy(maintenance_policy_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "query response budget",
        env_vars: &["QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES"],
        apply: |config, lookup| {
            Ok(config.with_query_response_budget(query_response_budget_from_lookup(lookup)?))
        },
    },
    EnvPolicyFamily {
        name: "integrity scrub",
        env_vars: &[
            "QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS",
            "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP",
        ],
        apply: |config, lookup| {
            Ok(config.with_integrity_scrub_policy(integrity_scrub_policy_from_lookup(lookup)?))
        },
    },
];

/// Env vars the state-root resolution reads, outside every policy family.
pub(crate) const STATE_ROOT_ENV_VARS: &[&str] =
    &["QUANTA_INDEX_STATE_ROOT", "QUANTA_INDEX_CACHE_ROOT"];

impl SearchdConfig {
    #[must_use]
    pub fn from_state_root(state_root: PathBuf) -> Self {
        let socket_dir = state_root.join("search-plane");
        Self {
            state_root,
            query_socket_path: socket_dir.join("query.sock"),
            control_socket_path: socket_dir.join("control.sock"),
            ingest_socket_path: socket_dir.join("ingest.sock"),
            semantic_embedder_profile: SemanticEmbedderProfile::default(),
            search_corpus_history_retention_policy: None,
            snapshot_registry_policy: SnapshotRegistryPolicy::DEFAULT,
            lexical_execution_budget: LexicalExecutionBudgetV1::DEFAULT,
            query_admission_policy: ServerAdmissionPolicy::DEFAULT,
            regex_match_cache_policy: RegexMatchCachePolicy::DEFAULT,
            ingest_resource_policy: IngestResourcePolicy::DEFAULT,
            semantic_stream_window_policy: SemanticStreamWindowPolicy::DEFAULT,
            lexical_writer_policy: LexicalWriterPolicy::DEFAULT,
            socket_access_policies: SocketAccessPolicies::PRIVATE,
            process_memory_ceilings: ProcessMemoryCeilings::DEFAULT,
            maintenance_policy: MaintenancePolicy::DEFAULT,
            query_response_budget: ResponsePayloadBudget::DEFAULT,
            integrity_scrub_policy: IntegrityScrubPolicyV1::DEFAULT,
        }
    }

    /// The deployment config: the state root from env, then every policy
    /// family from env.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(None, &optional_env)
    }

    /// The deployment config with an explicit state root (`--state-root`),
    /// and every policy family from env — the same chain as
    /// [`Self::from_env`], so the two entry points cannot drift.
    pub fn from_env_with_state_root(state_root: PathBuf) -> Result<Self> {
        Self::from_lookup(Some(state_root), &optional_env)
    }

    /// The one config chain: the state root (given, or resolved from the
    /// lookup), then [`ENV_POLICY_FAMILIES`] in order, then the required
    /// policies checked. Both public entry points are this function.
    pub(crate) fn from_lookup(state_root: Option<PathBuf>, lookup: &EnvLookup<'_>) -> Result<Self> {
        let state_root = match state_root {
            Some(state_root) => state_root,
            None => Self::resolve_state_root_from_lookup(lookup)?,
        };
        let mut config = Self::from_state_root(state_root);
        for family in ENV_POLICY_FAMILIES {
            config = (family.apply)(config, lookup).map_err(|error| {
                anyhow::anyhow!(
                    "{} policy (env {}): {error}",
                    family.name,
                    family.env_vars.join(", ")
                )
            })?;
        }
        let _validated = config.search_corpus_history_retention_policy()?;
        let _validated = config.process_memory_envelope()?;
        Ok(config)
    }

    fn resolve_state_root_from_lookup(lookup: &EnvLookup<'_>) -> Result<PathBuf> {
        if let Some(explicit) = lookup("QUANTA_INDEX_STATE_ROOT")? {
            return Ok(PathBuf::from(explicit));
        }
        if let Some(cache) = lookup("QUANTA_INDEX_CACHE_ROOT")? {
            return Ok(PathBuf::from(cache).join("state"));
        }
        let home = std::env::var("HOME").map_err(|_err| {
            anyhow::anyhow!(
                "cannot resolve state_root: HOME unset and none of {} set",
                STATE_ROOT_ENV_VARS.join(", ")
            )
        })?;
        let home_path = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        let default_root = home_path.join("Library/Caches/quanta-index/state");
        #[cfg(not(target_os = "macos"))]
        let default_root = home_path.join(".cache/quanta-index/state");
        Ok(default_root)
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[must_use]
    pub fn query_socket_path(&self) -> &Path {
        &self.query_socket_path
    }

    #[must_use]
    pub fn control_socket_path(&self) -> &Path {
        &self.control_socket_path
    }

    #[must_use]
    pub fn ingest_socket_path(&self) -> &Path {
        &self.ingest_socket_path
    }

    #[must_use]
    pub fn semantic_embedder_profile(&self) -> &SemanticEmbedderProfile {
        &self.semantic_embedder_profile
    }

    #[must_use]
    pub const fn snapshot_registry_policy(&self) -> SnapshotRegistryPolicy {
        self.snapshot_registry_policy
    }

    #[must_use]
    pub const fn with_snapshot_registry_policy(mut self, policy: SnapshotRegistryPolicy) -> Self {
        self.snapshot_registry_policy = policy;
        self
    }

    #[must_use]
    pub const fn lexical_execution_budget(&self) -> LexicalExecutionBudgetV1 {
        self.lexical_execution_budget
    }

    #[must_use]
    pub const fn with_lexical_execution_budget(mut self, budget: LexicalExecutionBudgetV1) -> Self {
        self.lexical_execution_budget = budget;
        self
    }

    #[must_use]
    pub const fn query_admission_policy(&self) -> ServerAdmissionPolicy {
        self.query_admission_policy
    }

    #[must_use]
    pub const fn with_query_admission_policy(mut self, policy: ServerAdmissionPolicy) -> Self {
        self.query_admission_policy = policy;
        self
    }

    #[must_use]
    pub const fn regex_match_cache_policy(&self) -> RegexMatchCachePolicy {
        self.regex_match_cache_policy
    }

    #[must_use]
    pub const fn with_regex_match_cache_policy(mut self, policy: RegexMatchCachePolicy) -> Self {
        self.regex_match_cache_policy = policy;
        self
    }

    #[must_use]
    pub const fn lexical_writer_policy(&self) -> LexicalWriterPolicy {
        self.lexical_writer_policy
    }

    #[must_use]
    pub const fn with_lexical_writer_policy(mut self, policy: LexicalWriterPolicy) -> Self {
        self.lexical_writer_policy = policy;
        self
    }

    #[must_use]
    pub const fn ingest_resource_policy(&self) -> IngestResourcePolicy {
        self.ingest_resource_policy
    }

    #[must_use]
    pub const fn socket_access_policies(&self) -> &SocketAccessPolicies {
        &self.socket_access_policies
    }

    #[must_use]
    pub fn with_socket_access_policies(mut self, policies: SocketAccessPolicies) -> Self {
        self.socket_access_policies = policies;
        self
    }

    #[must_use]
    pub const fn with_ingest_resource_policy(mut self, policy: IngestResourcePolicy) -> Self {
        self.ingest_resource_policy = policy;
        self
    }

    #[must_use]
    pub const fn process_memory_ceilings(&self) -> ProcessMemoryCeilings {
        self.process_memory_ceilings
    }

    #[must_use]
    pub const fn with_process_memory_ceilings(mut self, ceilings: ProcessMemoryCeilings) -> Self {
        self.process_memory_ceilings = ceilings;
        self
    }

    #[must_use]
    pub const fn maintenance_policy(&self) -> MaintenancePolicy {
        self.maintenance_policy
    }

    #[must_use]
    pub const fn with_maintenance_policy(mut self, policy: MaintenancePolicy) -> Self {
        self.maintenance_policy = policy;
        self
    }

    #[must_use]
    pub const fn query_response_budget(&self) -> ResponsePayloadBudget {
        self.query_response_budget
    }

    #[must_use]
    pub const fn with_query_response_budget(mut self, budget: ResponsePayloadBudget) -> Self {
        self.query_response_budget = budget;
        self
    }

    #[must_use]
    pub const fn integrity_scrub_policy(&self) -> IntegrityScrubPolicyV1 {
        self.integrity_scrub_policy
    }

    #[must_use]
    pub const fn with_integrity_scrub_policy(mut self, policy: IntegrityScrubPolicyV1) -> Self {
        self.integrity_scrub_policy = policy;
        self
    }

    /// The one process memory envelope every resident byte policy of this
    /// config declares under (QI-BB-016), validated.
    ///
    /// The sum must fit the configured ceiling, typed
    /// `PROCESS_MEMORY_ENVELOPE_EXCEEDED` when it does not.
    pub fn process_memory_envelope(&self) -> Result<ProcessMemoryEnvelopeV1> {
        let embedding_cache_ledger_bytes = match &self.semantic_embedder_profile {
            SemanticEmbedderProfile::OpenAi { tuning, .. } if tuning.cache_enabled => tuning
                .cache_retention
                .max_entries()
                .saturating_mul(EMBEDDING_CACHE_LEDGER_BYTES_PER_ENTRY),
            SemanticEmbedderProfile::OpenAi { .. }
            | SemanticEmbedderProfile::Hash { .. }
            | SemanticEmbedderProfile::Unavailable => 0,
        };
        let envelope = ProcessMemoryEnvelopeV1 {
            lexical_writer_bytes: self.lexical_writer_policy.envelope_bytes(),
            snapshot_registry_bytes: self.snapshot_registry_policy.max_resident_bytes(),
            regex_match_cache_bytes: self.regex_match_cache_policy.max_resident_bytes(),
            embedding_cache_ledger_bytes,
            semantic_stream_window_bytes: self.semantic_stream_window_policy.max_vector_bytes(),
            ingest_batch_bytes: self
                .ingest_resource_policy
                .max_text_bytes()
                .saturating_add(self.ingest_resource_policy.max_vector_bytes()),
            ceiling: self.process_memory_ceilings.ceiling_bytes(),
            rss_ceiling: self.process_memory_ceilings.rss_ceiling_bytes(),
        };
        envelope.validate().map_err(anyhow::Error::from)?;
        Ok(envelope)
    }

    #[must_use]
    pub const fn semantic_stream_window_policy(&self) -> SemanticStreamWindowPolicy {
        self.semantic_stream_window_policy
    }

    #[must_use]
    pub const fn with_semantic_stream_window_policy(
        mut self,
        policy: SemanticStreamWindowPolicy,
    ) -> Self {
        self.semantic_stream_window_policy = policy;
        self
    }

    pub fn search_corpus_history_retention_policy(
        &self,
    ) -> Result<SearchCorpusHistoryRetentionPolicyV1> {
        let Some(policy) = self.search_corpus_history_retention_policy else {
            return Err(anyhow::anyhow!(
                "search-corpus history retention policy is required; configure max_generations, max_bytes, max_revision_pairs, and max_total_bytes"
            ));
        };
        Ok(policy)
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        self.query_socket_path()
    }

    /// Override query / control socket paths. Ingest socket retains its
    /// default `state_root/search-plane/ingest.sock` location; use
    /// [`Self::with_ingest_socket_override`] to override that as well.
    #[must_use]
    pub fn with_socket_overrides(mut self, query_socket: PathBuf, control_socket: PathBuf) -> Self {
        self.query_socket_path = query_socket;
        self.control_socket_path = control_socket;
        self
    }

    /// Override the ingest socket path independently. Test rails that need
    /// per-instance ingest socket paths use this; the production path is the
    /// default in [`Self::from_state_root`].
    #[must_use]
    pub fn with_ingest_socket_override(mut self, ingest_socket: PathBuf) -> Self {
        self.ingest_socket_path = ingest_socket;
        self
    }

    #[must_use]
    pub fn with_semantic_embedder_profile(mut self, profile: SemanticEmbedderProfile) -> Self {
        self.semantic_embedder_profile = profile;
        self
    }

    pub fn try_with_search_corpus_history_retention_limits(
        mut self,
        max_generations: usize,
        max_bytes: u64,
        max_revision_pairs: usize,
        max_total_bytes: u64,
    ) -> Result<Self> {
        self.search_corpus_history_retention_policy = Some(
            SearchCorpusHistoryRetentionPolicyV1::new(
                max_generations,
                max_bytes,
                max_revision_pairs,
                max_total_bytes,
            )
            .map_err(anyhow::Error::from)?,
        );
        Ok(self)
    }

    #[must_use]
    pub(crate) fn with_search_corpus_history_retention_policy_v1(
        mut self,
        policy: SearchCorpusHistoryRetentionPolicyV1,
    ) -> Self {
        self.search_corpus_history_retention_policy = Some(policy);
        self
    }

    #[must_use]
    pub fn with_provider_unavailable_query_text_embedder(self) -> Self {
        self.with_semantic_embedder_profile(SemanticEmbedderProfile::Unavailable)
    }
}

/// Read an optional daemon env var, distinguishing "unset" from "unreadable".
///
/// An unset variable is the ordinary absent case. A variable that is set but
/// not valid UTF-8 is an operator error and is surfaced, not collapsed into
/// "absent" where it would silently select a default the operator did not ask
/// for.
pub(crate) fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err @ std::env::VarError::NotUnicode(_)) => {
            Err(anyhow::anyhow!("{name} is set but not valid UTF-8: {err}"))
        }
    }
}

fn search_corpus_history_retention_policy_from_lookup_v1(
    lookup: &EnvLookup<'_>,
) -> Result<SearchCorpusHistoryRetentionPolicyV1> {
    let max_generations = required_positive_raw_usize(
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
        lookup("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS")?,
    )?;
    let max_bytes = required_positive_raw_u64(
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
        lookup("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES")?,
    )?;
    let max_revision_pairs = required_positive_raw_usize(
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
        lookup("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS")?,
    )?;
    let max_total_bytes = required_positive_raw_u64(
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
        lookup("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES")?,
    )?;
    SearchCorpusHistoryRetentionPolicyV1::new(
        max_generations,
        max_bytes,
        max_revision_pairs,
        max_total_bytes,
    )
    .map_err(anyhow::Error::from)
}

/// Resolve the snapshot registry limits from env.
///
/// `QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES` and
/// `QUANTA_INDEX_SNAPSHOT_MAX_RESIDENT_BYTES` are optional as a pair: neither
/// set selects [`SnapshotRegistryPolicy::DEFAULT`]; one without the other is
/// an operator error rather than a half-applied override.
fn snapshot_registry_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<SnapshotRegistryPolicy> {
    const ENTRIES: &str = "QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES";
    const BYTES: &str = "QUANTA_INDEX_SNAPSHOT_MAX_RESIDENT_BYTES";
    match (lookup(ENTRIES)?, lookup(BYTES)?) {
        (None, None) => Ok(SnapshotRegistryPolicy::DEFAULT),
        (Some(entries), Some(bytes)) => SnapshotRegistryPolicy::new(
            required_positive_raw_usize(ENTRIES, Some(entries))?,
            required_positive_raw_u64(BYTES, Some(bytes))?,
        )
        .map_err(anyhow::Error::from),
        (Some(_), None) => Err(anyhow::anyhow!("{ENTRIES} is set but {BYTES} is not")),
        (None, Some(_)) => Err(anyhow::anyhow!("{BYTES} is set but {ENTRIES} is not")),
    }
}

/// Resolve the lexical examined-candidate budget from env.
///
/// `QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES` unset selects
/// [`LexicalExecutionBudgetV1::DEFAULT`]; set, it must be a positive integer.
fn lexical_execution_budget_from_lookup(
    lookup: &EnvLookup<'_>,
) -> Result<LexicalExecutionBudgetV1> {
    const NAME: &str = "QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES";
    match lookup(NAME)? {
        None => Ok(LexicalExecutionBudgetV1::DEFAULT),
        Some(raw) => LexicalExecutionBudgetV1::new(required_positive_raw_usize(NAME, Some(raw))?)
            .map_err(anyhow::Error::from),
    }
}

/// Resolve the query socket's admission policy from env (QI-BB-002).
///
/// Each knob is optional and, unset, takes the matching field of
/// [`ServerAdmissionPolicy::DEFAULT`]; the combination is then validated as
/// one policy, so a zero limit or more dispatch slots than connections is
/// refused at boot rather than half-applied. The per-operation I/O timeout
/// is not an operator knob.
///
/// - `QUANTA_INDEX_QUERY_MAX_CONNECTIONS` — live connections the accept loop
///   admits; past it a connection is closed, not queued.
/// - `QUANTA_INDEX_QUERY_DISPATCH_SLOTS` — requests executing concurrently.
/// - `QUANTA_INDEX_QUERY_MAX_IN_FLIGHT_PER_REPO` — slots one repository may
///   hold at once; past it a request for that repository answers
///   `SERVER_OVERLOADED` naming the repository after the queue wait.
/// - `QUANTA_INDEX_QUERY_QUEUE_WAIT_MS` — how long a request waits for a slot
///   before `SERVER_OVERLOADED`; zero means refuse immediately.
/// - `QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS` — the deadline every dispatch
///   runs under; a route past it answers `REQUEST_DEADLINE_EXCEEDED`.
fn query_admission_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<ServerAdmissionPolicy> {
    const CONNECTIONS: &str = "QUANTA_INDEX_QUERY_MAX_CONNECTIONS";
    const SLOTS: &str = "QUANTA_INDEX_QUERY_DISPATCH_SLOTS";
    const PER_REPO: &str = "QUANTA_INDEX_QUERY_MAX_IN_FLIGHT_PER_REPO";
    const QUEUE_WAIT: &str = "QUANTA_INDEX_QUERY_QUEUE_WAIT_MS";
    const BUDGET: &str = "QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS";
    let defaults = ServerAdmissionPolicy::DEFAULT;
    let max_connections = match lookup(CONNECTIONS)? {
        None => defaults.max_connections(),
        Some(raw) => required_positive_raw_usize(CONNECTIONS, Some(raw))?,
    };
    let dispatch_slots = match lookup(SLOTS)? {
        None => defaults.dispatch_slots(),
        Some(raw) => required_positive_raw_usize(SLOTS, Some(raw))?,
    };
    // Unset, the per-repository cap follows the slot count down so a
    // narrower slot policy is not refused by a default it never named.
    let max_in_flight_per_repo = match lookup(PER_REPO)? {
        None => defaults.max_in_flight_per_repo().min(dispatch_slots),
        Some(raw) => required_positive_raw_usize(PER_REPO, Some(raw))?,
    };
    let queue_wait = match lookup(QUEUE_WAIT)? {
        None => defaults.queue_wait(),
        Some(raw) => Duration::from_millis(raw_u64(QUEUE_WAIT, &raw)?),
    };
    let dispatch_budget = match lookup(BUDGET)? {
        None => defaults.dispatch_budget(),
        Some(raw) => Duration::from_millis(required_positive_raw_u64(BUDGET, Some(raw))?),
    };
    ServerAdmissionPolicy::new(
        max_connections,
        dispatch_slots,
        max_in_flight_per_repo,
        queue_wait,
        dispatch_budget,
        defaults.io_timeout(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "{CONNECTIONS}={max_connections} {SLOTS}={dispatch_slots} {PER_REPO}={max_in_flight_per_repo} {QUEUE_WAIT}={}ms {BUDGET}={}ms is not a valid admission policy: {error}",
            queue_wait.as_millis(),
            dispatch_budget.as_millis()
        )
    })
}

/// Resolve the regex match cache bounds from env (QI-BB-024).
///
/// Each knob is optional and, unset, takes the matching field of
/// [`RegexMatchCachePolicy::DEFAULT`]; zero is refused.
///
/// - `QUANTA_INDEX_REGEX_CACHE_MAX_ENTRIES`
/// - `QUANTA_INDEX_REGEX_CACHE_MAX_RESIDENT_BYTES`
/// - `QUANTA_INDEX_REGEX_CACHE_MAX_MATCHES_PER_ENTRY`
fn regex_match_cache_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<RegexMatchCachePolicy> {
    const ENTRIES: &str = "QUANTA_INDEX_REGEX_CACHE_MAX_ENTRIES";
    const BYTES: &str = "QUANTA_INDEX_REGEX_CACHE_MAX_RESIDENT_BYTES";
    const MATCHES: &str = "QUANTA_INDEX_REGEX_CACHE_MAX_MATCHES_PER_ENTRY";
    let defaults = RegexMatchCachePolicy::DEFAULT;
    let max_entries = match lookup(ENTRIES)? {
        None => defaults.max_entries(),
        Some(raw) => required_positive_raw_usize(ENTRIES, Some(raw))?,
    };
    let max_resident_bytes = match lookup(BYTES)? {
        None => defaults.max_resident_bytes(),
        Some(raw) => required_positive_raw_u64(BYTES, Some(raw))?,
    };
    let max_matches_per_entry = match lookup(MATCHES)? {
        None => defaults.max_matches_per_entry(),
        Some(raw) => required_positive_raw_usize(MATCHES, Some(raw))?,
    };
    RegexMatchCachePolicy::new(max_entries, max_resident_bytes, max_matches_per_entry)
        .map_err(anyhow::Error::from)
}

/// Resolve the lexical writer envelope from env (QI-BB-016).
///
/// Each knob is optional and, unset, takes the matching field of
/// [`LexicalWriterPolicy::DEFAULT`]; the policy refuses a heap below the
/// writer's minimum, an envelope smaller than one writer, and a zero idle
/// interval.
///
/// - `QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES`
/// - `QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES`
/// - `QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS`
fn lexical_writer_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<LexicalWriterPolicy> {
    const ENVELOPE: &str = "QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES";
    const HEAP: &str = "QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES";
    const IDLE_SECS: &str = "QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS";
    let defaults = LexicalWriterPolicy::DEFAULT;
    let envelope_bytes = match lookup(ENVELOPE)? {
        None => defaults.envelope_bytes(),
        Some(raw) => required_positive_raw_u64(ENVELOPE, Some(raw))?,
    };
    let writer_heap_bytes = match lookup(HEAP)? {
        None => defaults.writer_heap_bytes(),
        Some(raw) => required_positive_raw_u64(HEAP, Some(raw))?,
    };
    let idle_after = match lookup(IDLE_SECS)? {
        None => defaults.idle_after(),
        Some(raw) => Duration::from_secs(required_positive_raw_u64(IDLE_SECS, Some(raw))?),
    };
    LexicalWriterPolicy::new(envelope_bytes, writer_heap_bytes, idle_after)
        .map_err(anyhow::Error::from)
}

/// Resolve the ingest resource envelope from env (QI-BB-021).
///
/// Each knob is optional and, unset, takes the matching field of
/// [`IngestResourcePolicy::DEFAULT`]; zero is refused.
///
/// - `QUANTA_INDEX_INGEST_MAX_RECORDS`
/// - `QUANTA_INDEX_INGEST_MAX_TEXT_BYTES`
/// - `QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES`
fn ingest_resource_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<IngestResourcePolicy> {
    const RECORDS: &str = "QUANTA_INDEX_INGEST_MAX_RECORDS";
    const TEXT_BYTES: &str = "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES";
    const VECTOR_BYTES: &str = "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES";
    let defaults = IngestResourcePolicy::DEFAULT;
    let max_records = match lookup(RECORDS)? {
        None => defaults.max_records(),
        Some(raw) => required_positive_raw_usize(RECORDS, Some(raw))?,
    };
    let max_text_bytes = match lookup(TEXT_BYTES)? {
        None => defaults.max_text_bytes(),
        Some(raw) => required_positive_raw_u64(TEXT_BYTES, Some(raw))?,
    };
    let max_vector_bytes = match lookup(VECTOR_BYTES)? {
        None => defaults.max_vector_bytes(),
        Some(raw) => required_positive_raw_u64(VECTOR_BYTES, Some(raw))?,
    };
    IngestResourcePolicy::new(max_records, max_text_bytes, max_vector_bytes)
        .map_err(anyhow::Error::from)
}

/// Resolve the semantic stream window from env (QI-BB-021).
///
/// Each knob is optional and, unset, takes the matching field of
/// [`SemanticStreamWindowPolicy::DEFAULT`]; zero is refused.
///
/// - `QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES`
/// - `QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES`
fn semantic_stream_window_policy_from_lookup(
    lookup: &EnvLookup<'_>,
) -> Result<SemanticStreamWindowPolicy> {
    const SCOPES: &str = "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES";
    const VECTOR_BYTES: &str = "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES";
    let defaults = SemanticStreamWindowPolicy::DEFAULT;
    let max_owner_scopes = match lookup(SCOPES)? {
        None => defaults.max_owner_scopes(),
        Some(raw) => required_positive_raw_usize(SCOPES, Some(raw))?,
    };
    let max_vector_bytes = match lookup(VECTOR_BYTES)? {
        None => defaults.max_vector_bytes(),
        Some(raw) => required_positive_raw_u64(VECTOR_BYTES, Some(raw))?,
    };
    SemanticStreamWindowPolicy::new(max_owner_scopes, max_vector_bytes).map_err(anyhow::Error::from)
}

/// Resolve the embedding cache retention from an injected lookup
/// (QI-BB-009).
///
/// Each knob is optional and, unset, takes the matching field of
/// [`EmbeddingCacheRetentionPolicy::DEFAULT`]; zero is refused, and the
/// total ceiling must hold one namespace.
///
/// - `QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES`
/// - `QUANTA_INDEX_EMBED_CACHE_MAX_BYTES`
/// - `QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES`
/// - `QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS` — oldest an entry may be,
///   from its write
/// - `QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES` — every namespace
///   together
fn embedding_cache_retention_policy_from_lookup(
    lookup: &EnvLookup<'_>,
) -> Result<EmbeddingCacheRetentionPolicy> {
    const ENTRIES: &str = "QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES";
    const BYTES: &str = "QUANTA_INDEX_EMBED_CACHE_MAX_BYTES";
    const NAMESPACES: &str = "QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES";
    const AGE_SECS: &str = "QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS";
    const TOTAL_BYTES: &str = "QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES";
    let defaults = EmbeddingCacheRetentionPolicy::DEFAULT;
    let max_entries = match lookup(ENTRIES)? {
        None => defaults.max_entries(),
        Some(raw) => required_positive_raw_u64(ENTRIES, Some(raw))?,
    };
    let max_resident_bytes = match lookup(BYTES)? {
        None => defaults.max_resident_bytes(),
        Some(raw) => required_positive_raw_u64(BYTES, Some(raw))?,
    };
    let max_namespaces = match lookup(NAMESPACES)? {
        None => defaults.max_namespaces(),
        Some(raw) => required_positive_raw_usize(NAMESPACES, Some(raw))?,
    };
    let max_entry_age = match lookup(AGE_SECS)? {
        None => defaults.max_entry_age(),
        Some(raw) => Duration::from_secs(required_positive_raw_u64(AGE_SECS, Some(raw))?),
    };
    let max_total_bytes = match lookup(TOTAL_BYTES)? {
        None => defaults.max_total_bytes(),
        Some(raw) => required_positive_raw_u64(TOTAL_BYTES, Some(raw))?,
    };
    EmbeddingCacheRetentionPolicy::new(
        max_entries,
        max_resident_bytes,
        max_namespaces,
        max_entry_age,
        max_total_bytes,
    )
    .map_err(anyhow::Error::from)
}

/// Resolve the process memory ceilings from env (QI-BB-016).
///
/// - `QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES` — what every declared
///   resident byte policy must fit under together; unset selects
///   [`ProcessMemoryEnvelopeV1::DEFAULT_CEILING_BYTES`].
/// - `QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES` — the resident-memory level
///   above which no new lexical writer is opened; unset disables the
///   gate, which the boot log says.
fn process_memory_ceilings_from_lookup(lookup: &EnvLookup<'_>) -> Result<ProcessMemoryCeilings> {
    const CEILING: &str = "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES";
    const RSS: &str = "QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES";
    let ceiling_bytes = match lookup(CEILING)? {
        None => ProcessMemoryCeilings::DEFAULT.ceiling_bytes(),
        Some(raw) => required_positive_raw_u64(CEILING, Some(raw))?,
    };
    let rss_ceiling_bytes = match lookup(RSS)? {
        None => None,
        Some(raw) => Some(required_positive_raw_u64(RSS, Some(raw))?),
    };
    ProcessMemoryCeilings::new(ceiling_bytes, rss_ceiling_bytes)
}

/// Resolve the maintenance timer's cadence from env.
///
/// `QUANTA_INDEX_MAINTENANCE_TICK_MS` unset selects
/// [`MaintenancePolicy::DEFAULT`]; zero is refused.
fn maintenance_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<MaintenancePolicy> {
    const TICK: &str = "QUANTA_INDEX_MAINTENANCE_TICK_MS";
    match lookup(TICK)? {
        None => Ok(MaintenancePolicy::DEFAULT),
        Some(raw) => MaintenancePolicy::new(Duration::from_millis(required_positive_raw_u64(
            TICK,
            Some(raw),
        )?)),
    }
}

/// Resolve the ranked page byte budget from env (QI-BB-005 보완 #5).
///
/// `QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES` unset selects
/// [`ResponsePayloadBudget::DEFAULT`] (a frame less the envelope
/// reserve); zero or more than that is refused.
fn query_response_budget_from_lookup(lookup: &EnvLookup<'_>) -> Result<ResponsePayloadBudget> {
    const BYTES: &str = "QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES";
    match lookup(BYTES)? {
        None => Ok(ResponsePayloadBudget::DEFAULT),
        Some(raw) => ResponsePayloadBudget::new(required_positive_raw_u64(BYTES, Some(raw))?)
            .map_err(anyhow::Error::from),
    }
}

/// Resolve the integrity scrub pacing from env (QI-BB-017).
///
/// `QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS` and
/// `QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP` are optional as a
/// pair: neither set selects [`IntegrityScrubPolicyV1::DEFAULT`]; one
/// without the other is an operator error rather than a half-applied
/// override, and zero is refused.
fn integrity_scrub_policy_from_lookup(lookup: &EnvLookup<'_>) -> Result<IntegrityScrubPolicyV1> {
    const INTERVAL: &str = "QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS";
    const BYTES: &str = "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP";
    match (lookup(INTERVAL)?, lookup(BYTES)?) {
        (None, None) => Ok(IntegrityScrubPolicyV1::DEFAULT),
        (Some(interval), Some(bytes)) => IntegrityScrubPolicyV1::new(
            required_positive_raw_u64(INTERVAL, Some(interval))?,
            required_positive_raw_u64(BYTES, Some(bytes))?,
        )
        .map_err(anyhow::Error::from),
        (Some(_), None) => Err(anyhow::anyhow!("{INTERVAL} is set but {BYTES} is not")),
        (None, Some(_)) => Err(anyhow::anyhow!("{BYTES} is set but {INTERVAL} is not")),
    }
}

fn raw_u64(name: &str, raw: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("invalid {name} `{raw}`: {error}"))
}

fn required_positive_raw_usize(name: &str, raw: Option<String>) -> Result<usize> {
    let raw = raw.ok_or_else(|| anyhow::anyhow!("{name} is required"))?;
    let value = raw
        .trim()
        .parse::<usize>()
        .map_err(|error| anyhow::anyhow!("invalid {name} `{raw}`: {error}"))?;
    if value == 0 {
        return Err(anyhow::anyhow!("{name} must be non-zero"));
    }
    Ok(value)
}

fn required_positive_raw_u64(name: &str, raw: Option<String>) -> Result<u64> {
    let raw = raw.ok_or_else(|| anyhow::anyhow!("{name} is required"))?;
    let value = raw
        .trim()
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("invalid {name} `{raw}`: {error}"))?;
    if value == 0 {
        return Err(anyhow::anyhow!("{name} must be non-zero"));
    }
    Ok(value)
}

/// Resolve the semantic embedder profile from an injected lookup
/// (QI-BB-007).
///
/// `QUANTA_INDEX_EMBEDDER` names the profile: `openai` (a learned
/// provider), `unavailable` (queries fail closed), or `hash-dev` (the
/// development hash embedder, by name). Unset, it is refused — a
/// deployment that names no embedder does not silently serve token
/// overlap as semantics — unless `QUANTA_INDEX_ALLOW_DEV_EMBEDDER=1`,
/// which the harness and the test rails set explicitly. An unknown
/// selector, the retired `hash` spelling, or a missing `OpenAI` key fails
/// closed. Applied by the one config chain, so `--state-root` and the
/// env-resolved root honor it alike.
fn semantic_embedder_profile_from_lookup(
    lookup: &EnvLookup<'_>,
) -> Result<SemanticEmbedderProfile> {
    match lookup("QUANTA_INDEX_EMBEDDER")?.as_deref() {
        None | Some("") => {
            if !dev_embedder_allowed(lookup)? {
                return Err(anyhow::anyhow!(
                    "QUANTA_INDEX_EMBEDDER is unset: name the embedder (openai|unavailable|{DEV_HASH_EMBEDDER_SELECTOR}); the development hash embedder is served only by name or with {ALLOW_DEV_EMBEDDER_ENV}=1"
                ));
            }
            Ok(SemanticEmbedderProfile::Hash {
                dimension: embed_dim_from_lookup(lookup, SEARCH_OWNED_SEMANTIC_DIMENSION)?,
            })
        }
        Some(DEV_HASH_EMBEDDER_SELECTOR) => Ok(SemanticEmbedderProfile::Hash {
            dimension: embed_dim_from_lookup(lookup, SEARCH_OWNED_SEMANTIC_DIMENSION)?,
        }),
        Some("hash") => Err(anyhow::anyhow!(
            "QUANTA_INDEX_EMBEDDER=hash is retired: the hash embedder is a development profile and is selected as {DEV_HASH_EMBEDDER_SELECTOR}"
        )),
        Some("unavailable") => Ok(SemanticEmbedderProfile::Unavailable),
        Some("openai") => {
            let Some(api_key) = lookup("OPENAI_API_KEY")? else {
                return Err(anyhow::anyhow!(
                    "QUANTA_INDEX_EMBEDDER=openai requires OPENAI_API_KEY to be set"
                ));
            };
            if api_key.trim().is_empty() {
                return Err(anyhow::anyhow!("OPENAI_API_KEY is set but empty"));
            }
            let model = lookup("QUANTA_INDEX_EMBED_MODEL")?
                .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
            let model_revision = openai_model_revision_from_lookup(lookup)?;
            Ok(SemanticEmbedderProfile::OpenAi {
                model,
                model_revision,
                dimension: embed_dim_from_lookup(lookup, DEFAULT_OPENAI_DIMENSION)?,
                api_key,
                tuning: openai_tuning_from_env_with(lookup)?,
            })
        }
        Some(other) => Err(anyhow::anyhow!(
            "unknown QUANTA_INDEX_EMBEDDER '{other}' (expected openai|unavailable|{DEV_HASH_EMBEDDER_SELECTOR})"
        )),
    }
}

/// Whether the operator opted an unset selector into the development
/// embedder: `QUANTA_INDEX_ALLOW_DEV_EMBEDDER=1`; any other value is a
/// misspelling, refused.
fn dev_embedder_allowed(lookup: &EnvLookup<'_>) -> Result<bool> {
    match lookup(ALLOW_DEV_EMBEDDER_ENV)?.as_deref() {
        None => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(anyhow::anyhow!(
            "{ALLOW_DEV_EMBEDDER_ENV} must be `1` to opt into the development embedder, got `{other}`"
        )),
    }
}

fn embed_dim_from_lookup(lookup: &EnvLookup<'_>, default: usize) -> Result<usize> {
    parse_embed_dim(lookup("QUANTA_INDEX_EMBED_DIM")?.as_deref(), default)
}

/// The operator-pinned `OpenAI` model revision (QI-BB-028).
///
/// `QUANTA_INDEX_EMBED_MODEL_REVISION` is required for the `openai` profile:
/// the provider cannot name its own revision, and without one a served-model
/// change would mix embedding spaces in one cache namespace and one sealed
/// generation. There is no default; an unset or blank revision fails boot.
fn openai_model_revision_from_lookup(lookup: &EnvLookup<'_>) -> Result<String> {
    const NAME: &str = "QUANTA_INDEX_EMBED_MODEL_REVISION";
    let Some(raw) = lookup(NAME)? else {
        return Err(anyhow::anyhow!(
            "QUANTA_INDEX_EMBEDDER=openai requires {NAME}: name the model revision you are pinning (rotate it when the served model changes)"
        ));
    };
    let revision = raw.trim();
    if revision.is_empty() || !revision.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(anyhow::anyhow!(
            "{NAME} must be a non-empty printable ASCII token, got `{raw}`"
        ));
    }
    Ok(revision.to_string())
}

/// Resolve the `OpenAI` embedder operational knobs from an injected
/// `name -> value` lookup.
///
/// Any unset knob falls back to the provider default. Splitting the lookup
/// from the real `std::env::var` call keeps the env-var-NAME -> field
/// binding unit-testable without mutating process env (this crate forbids
/// `unsafe`, so `std::env::set_var` is unavailable in tests).
fn openai_tuning_from_env_with(lookup: &EnvLookup<'_>) -> Result<OpenAiEmbedderTuning> {
    let mut tuning = openai_tuning_from_raw(
        lookup("QUANTA_INDEX_EMBED_BATCH")?.as_deref(),
        lookup("QUANTA_INDEX_EMBED_MAX_EST_TOKENS")?.as_deref(),
        lookup("QUANTA_INDEX_EMBED_MAX_RETRIES")?.as_deref(),
        lookup("QUANTA_INDEX_EMBED_TIMEOUT_SECS")?.as_deref(),
        lookup("QUANTA_INDEX_EMBED_CACHE")?.as_deref(),
        lookup("QUANTA_INDEX_EMBED_CONCURRENCY")?.as_deref(),
    )?;
    tuning.cache_retention = embedding_cache_retention_policy_from_lookup(lookup)?;
    Ok(tuning)
}

/// Pure assembler: maps the raw provider knob strings onto their tuning
/// fields; the cache retention policy is resolved separately.
///
/// Any unset knob falls back to the provider default. Extracted from
/// [`openai_tuning_from_env`] so the env-var-name -> field mapping (not just
/// the individual leaf parsers) is unit-testable without mutating process env
/// — a cross-wired field or typo'd binding fails the test instead of shipping
/// green.
fn openai_tuning_from_raw(
    batch: Option<&str>,
    max_estimated_tokens: Option<&str>,
    max_retries: Option<&str>,
    timeout_secs: Option<&str>,
    cache: Option<&str>,
    concurrency: Option<&str>,
) -> Result<OpenAiEmbedderTuning> {
    let defaults = OpenAiEmbedderTuning::default();
    Ok(OpenAiEmbedderTuning {
        max_batch: parse_embed_batch(batch, defaults.max_batch)?,
        max_estimated_tokens_per_request: parse_embed_max_estimated_tokens(
            max_estimated_tokens,
            defaults.max_estimated_tokens_per_request,
        )?,
        max_retries: parse_embed_max_retries(max_retries, defaults.max_retries)?,
        timeout: parse_embed_timeout(timeout_secs, defaults.timeout)?,
        cache_enabled: parse_embed_cache_enabled(cache, defaults.cache_enabled)?,
        cache_retention: defaults.cache_retention,
        concurrency: parse_embed_concurrency(concurrency, defaults.concurrency)?,
    })
}

fn parse_embed_concurrency(raw: Option<&str>, default: usize) -> Result<usize> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => {
            let parsed = value.trim().parse::<usize>().map_err(|err| {
                anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_CONCURRENCY '{value}': {err}")
            })?;
            if parsed == 0 || parsed > MAX_CONCURRENCY {
                return Err(anyhow::anyhow!(
                    "QUANTA_INDEX_EMBED_CONCURRENCY must be within 1..={MAX_CONCURRENCY}, got {parsed}"
                ));
            }
            Ok(parsed)
        }
    }
}

/// The embedding dimension knob, bounded (QI-BB-021).
///
/// Refused outside `1..=MAX_EMBEDDING_DIMENSION`: the dimension multiplies
/// every batch's vector residency and every cache entry, so a stray value
/// is a boot-time defect.
fn parse_embed_dim(raw: Option<&str>, default: usize) -> Result<usize> {
    let dimension = raw.map_or(Ok(default), |value| {
        value
            .trim()
            .parse::<usize>()
            .map_err(|err| anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_DIM '{value}': {err}"))
    })?;
    if dimension == 0 || dimension > MAX_EMBEDDING_DIMENSION {
        return Err(anyhow::anyhow!(
            "QUANTA_INDEX_EMBED_DIM must be within 1..={MAX_EMBEDDING_DIMENSION}, got {dimension}"
        ));
    }
    Ok(dimension)
}

fn parse_embed_batch(raw: Option<&str>, default: usize) -> Result<usize> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => {
            let parsed = value.trim().parse::<usize>().map_err(|err| {
                anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_BATCH '{value}': {err}")
            })?;
            if parsed == 0 {
                return Err(anyhow::anyhow!("QUANTA_INDEX_EMBED_BATCH must be >= 1, got 0"));
            }
            Ok(parsed)
        }
    }
}

fn parse_embed_max_estimated_tokens(raw: Option<&str>, default: usize) -> Result<usize> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => {
            let parsed = value.trim().parse::<usize>().map_err(|err| {
                anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_MAX_EST_TOKENS '{value}': {err}")
            })?;
            if parsed == 0 {
                return Err(anyhow::anyhow!(
                    "QUANTA_INDEX_EMBED_MAX_EST_TOKENS must be >= 1, got 0"
                ));
            }
            Ok(parsed)
        }
    }
}

fn parse_embed_max_retries(raw: Option<&str>, default: u32) -> Result<u32> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => value.trim().parse::<u32>().map_err(|err| {
            anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_MAX_RETRIES '{value}': {err}")
        }),
    }
}

fn parse_embed_timeout(raw: Option<&str>, default: Duration) -> Result<Duration> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => {
            let secs = value.trim().parse::<u64>().map_err(|err| {
                anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_TIMEOUT_SECS '{value}': {err}")
            })?;
            if secs == 0 {
                return Err(anyhow::anyhow!("QUANTA_INDEX_EMBED_TIMEOUT_SECS must be >= 1, got 0"));
            }
            Ok(Duration::from_secs(secs))
        }
    }
}

fn parse_embed_cache_enabled(raw: Option<&str>, default: bool) -> Result<bool> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Ok(true),
            "0" | "false" | "off" | "no" => Ok(false),
            other => Err(anyhow::anyhow!(
                "invalid QUANTA_INDEX_EMBED_CACHE '{other}' (expected true|false|on|off|1|0)"
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn default_profile_is_hash_at_search_owned_dimension() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"));
        assert_eq!(
            config.semantic_embedder_profile(),
            &SemanticEmbedderProfile::Hash {
                dimension: SEARCH_OWNED_SEMANTIC_DIMENSION
            }
        );
        assert!(config.semantic_embedder_profile().is_dev());
        assert!(!SemanticEmbedderProfile::Unavailable.is_dev());
    }

    /// One non-default value for every env knob every family reads, so a
    /// family that silently drops a knob, or a knob that reaches neither
    /// entry point, changes the resolved config in a way the fence sees.
    fn every_knob_non_default() -> BTreeMap<&'static str, &'static str> {
        BTreeMap::from([
            ("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS", "3"),
            ("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES", "4096"),
            ("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS", "17"),
            ("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES", "65536"),
            ("QUANTA_INDEX_EMBEDDER", "openai"),
            (ALLOW_DEV_EMBEDDER_ENV, "1"),
            ("QUANTA_INDEX_EMBED_DIM", "256"),
            ("QUANTA_INDEX_EMBED_MODEL", "text-embedding-3-large"),
            ("QUANTA_INDEX_EMBED_MODEL_REVISION", "rev-2026-09"),
            ("OPENAI_API_KEY", "sk-fence"),
            ("QUANTA_INDEX_EMBED_BATCH", "7"),
            ("QUANTA_INDEX_EMBED_MAX_EST_TOKENS", "1234"),
            ("QUANTA_INDEX_EMBED_MAX_RETRIES", "2"),
            ("QUANTA_INDEX_EMBED_TIMEOUT_SECS", "11"),
            ("QUANTA_INDEX_EMBED_CACHE", "on"),
            ("QUANTA_INDEX_EMBED_CONCURRENCY", "5"),
            ("QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES", "77"),
            ("QUANTA_INDEX_EMBED_CACHE_MAX_BYTES", "4096"),
            ("QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES", "2"),
            ("QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS", "3600"),
            ("QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES", "8192"),
            ("QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES", "4"),
            ("QUANTA_INDEX_SNAPSHOT_MAX_RESIDENT_BYTES", "1024"),
            ("QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES", "12"),
            ("QUANTA_INDEX_QUERY_MAX_CONNECTIONS", "9"),
            ("QUANTA_INDEX_QUERY_DISPATCH_SLOTS", "3"),
            ("QUANTA_INDEX_QUERY_MAX_IN_FLIGHT_PER_REPO", "2"),
            ("QUANTA_INDEX_QUERY_QUEUE_WAIT_MS", "250"),
            ("QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS", "5000"),
            ("QUANTA_INDEX_REGEX_CACHE_MAX_ENTRIES", "5"),
            ("QUANTA_INDEX_REGEX_CACHE_MAX_RESIDENT_BYTES", "2048"),
            ("QUANTA_INDEX_REGEX_CACHE_MAX_MATCHES_PER_ENTRY", "6"),
            ("QUANTA_INDEX_INGEST_MAX_RECORDS", "12"),
            ("QUANTA_INDEX_INGEST_MAX_TEXT_BYTES", "3456"),
            ("QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES", "789"),
            ("QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES", "8"),
            ("QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES", "4096"),
            ("QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES", "45000000"),
            ("QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES", "15000001"),
            ("QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS", "7"),
            ("QUANTA_INDEX_QUERY_SOCKET_ACCESS", "shared:uid=0"),
            ("QUANTA_INDEX_CONTROL_SOCKET_ACCESS", "shared:uid=0"),
            ("QUANTA_INDEX_INGEST_SOCKET_ACCESS", "shared:uid=0"),
            ("QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES", "3221225472"),
            ("QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES", "4294967296"),
            ("QUANTA_INDEX_MAINTENANCE_TICK_MS", "1500"),
            ("QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES", "1048576"),
            ("QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS", "750"),
            ("QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP", "4096"),
        ])
    }

    /// An env knob name: uppercase, digits and underscores, not ending in
    /// an underscore (a bare prefix is this fence's own text).
    fn is_env_knob_name(literal: &str) -> bool {
        !literal.ends_with('_')
            && literal
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    }

    /// Every `QUANTA_INDEX_*` (and `OPENAI_*`) literal the config sources
    /// name, read from the sources themselves.
    fn env_literals_in_sources() -> BTreeSet<String> {
        let sources = [include_str!("config.rs"), include_str!("socket_access.rs")];
        let mut found = BTreeSet::new();
        for source in sources {
            for prefix in ["\"QUANTA_INDEX_", "\"OPENAI_"] {
                for (start, _) in source.match_indices(prefix) {
                    let rest = source
                        .get(start.saturating_add(1)..)
                        .expect("a match starts at a char boundary");
                    let literal = rest.split('"').next().expect("a closed literal");
                    // Prose and this fence's own bare prefixes are not knobs.
                    if is_env_knob_name(literal) {
                        let _seen = found.insert(literal.to_string());
                    }
                }
            }
        }
        found
    }

    /// The one-chain proof (QI-BB-014, QI-BB-016).
    ///
    /// 1. Every env knob named anywhere in the config sources is read by
    ///    exactly one family in [`ENV_POLICY_FAMILIES`] (or resolves the
    ///    state root): a knob added outside the table fails here.
    /// 2. Every family, given every knob at a non-default value, changes
    ///    the config it is applied to: a family whose setter drops what
    ///    it resolved fails here.
    /// 3. Both entry points — the env-resolved state root and an explicit
    ///    one — resolve the same policies from the same env; only the
    ///    state root and the socket paths under it differ.
    #[test]
    fn every_env_knob_belongs_to_one_family_and_both_entry_points_apply_every_family() {
        let knobs = every_knob_non_default();
        let lookup = |name: &str| -> Result<Option<String>> {
            Ok(knobs.get(name).map(|value| (*value).to_string()))
        };

        // (1) The source fence.
        let mut declared: BTreeMap<&str, &str> = BTreeMap::new();
        for family in ENV_POLICY_FAMILIES {
            assert!(!family.env_vars.is_empty(), "{} names its knobs", family.name);
            for var in family.env_vars {
                assert!(
                    declared.insert(var, family.name).is_none(),
                    "{var} is read by two families"
                );
                assert!(
                    knobs.contains_key(var),
                    "{var} ({}) has no non-default fixture value; add one so the chain proof covers it",
                    family.name
                );
            }
        }
        for var in STATE_ROOT_ENV_VARS {
            assert!(declared.insert(var, "state root").is_none());
        }
        for literal in env_literals_in_sources() {
            assert!(
                declared.contains_key(literal.as_str()),
                "env knob `{literal}` is named in the config sources but belongs to no family in ENV_POLICY_FAMILIES; a knob outside the table reaches neither entry point"
            );
        }

        // (2) Every family applies what it resolves.
        let root = PathBuf::from("/tmp/quanta-index-chain-fence");
        for family in ENV_POLICY_FAMILIES {
            let base = SearchdConfig::from_state_root(root.clone());
            let applied = match (family.apply)(base.clone(), &lookup) {
                Ok(applied) => applied,
                Err(error) => panic!("{} applies its knobs: {error}", family.name),
            };
            assert_ne!(
                applied, base,
                "{} left the config unchanged with every knob non-default",
                family.name
            );
        }

        // (3) Both entry points agree.
        let explicit_root = PathBuf::from("/tmp/quanta-index-chain-explicit");
        let with_state_root = SearchdConfig::from_lookup(Some(explicit_root.clone()), &lookup)
            .expect("the explicit-root entry point resolves every family");
        let mut env_knobs = knobs.clone();
        let _fresh = env_knobs.insert("QUANTA_INDEX_STATE_ROOT", "/tmp/quanta-index-chain-env");
        let env_lookup = |name: &str| -> Result<Option<String>> {
            Ok(env_knobs.get(name).map(|value| (*value).to_string()))
        };
        let from_env = SearchdConfig::from_lookup(None, &env_lookup)
            .expect("the env entry point resolves every family");
        assert_eq!(with_state_root.state_root(), explicit_root.as_path());
        assert_eq!(from_env.state_root(), Path::new("/tmp/quanta-index-chain-env"));
        // Same policies from the same env, whichever way the root came.
        let normalized = from_env
            .clone()
            .with_socket_overrides(
                with_state_root.query_socket_path().to_path_buf(),
                with_state_root.control_socket_path().to_path_buf(),
            )
            .with_ingest_socket_override(with_state_root.ingest_socket_path().to_path_buf());
        let mut expected = with_state_root.clone();
        expected.state_root = from_env.state_root().to_path_buf();
        assert_eq!(normalized, expected);
        // And the policies are the fixture's, not the defaults.
        assert_eq!(with_state_root.query_admission_policy().dispatch_slots(), 3);
        assert_eq!(
            with_state_root
                .query_admission_policy()
                .max_in_flight_per_repo(),
            2
        );
        assert_eq!(with_state_root.lexical_writer_policy().idle_after(), Duration::from_secs(7));
        assert_ne!(with_state_root.socket_access_policies(), &SocketAccessPolicies::PRIVATE);
        assert_eq!(with_state_root.process_memory_ceilings().ceiling_bytes(), 3_221_225_472);
        assert_eq!(with_state_root.maintenance_policy().tick(), Duration::from_millis(1500));
        assert_eq!(with_state_root.query_response_budget().max_payload_bytes(), 1_048_576);
        let SemanticEmbedderProfile::OpenAi {
            model,
            model_revision,
            dimension,
            tuning,
            ..
        } = with_state_root.semantic_embedder_profile()
        else {
            panic!("the fixture selects openai");
        };
        assert_eq!(
            (model.as_str(), model_revision.as_str(), *dimension),
            ("text-embedding-3-large", "rev-2026-09", 256)
        );
        assert_eq!(tuning.cache_retention.max_entry_age(), Duration::from_secs(3600));
        assert_eq!(tuning.cache_retention.max_total_bytes(), 8192);
    }

    /// The ranked page byte budget: unset is a frame less the envelope
    /// reserve, and a budget of zero or more than that is refused.
    #[test]
    fn the_query_response_budget_is_bounded_by_the_frame() {
        const BYTES: &str = "QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES";
        assert_eq!(
            query_response_budget_from_lookup(&|_name| Ok(None)).expect("unset"),
            ResponsePayloadBudget::DEFAULT
        );
        let at_default = ResponsePayloadBudget::DEFAULT
            .max_payload_bytes()
            .to_string();
        for (raw, admitted) in [
            ("1", true),
            (at_default.as_str(), true),
            ("0", false),
            ("16777216", false),
        ] {
            let lookup = |name: &str| -> Result<Option<String>> {
                Ok((name == BYTES).then(|| raw.to_string()))
            };
            assert_eq!(
                query_response_budget_from_lookup(&lookup).is_ok(),
                admitted,
                "{BYTES}={raw}"
            );
        }
    }

    /// The development label (QI-BB-007): an unset selector is refused
    /// unless the operator opts in by name; `hash-dev` is the hash
    /// embedder by name; the retired `hash` spelling is refused.
    #[test]
    fn an_unset_embedder_is_refused_unless_the_dev_embedder_is_allowed_by_name() {
        let unset = semantic_embedder_profile_from_lookup(&|_name| Ok(None))
            .expect_err("an unset embedder does not silently serve the hash profile");
        assert!(unset.to_string().contains(ALLOW_DEV_EMBEDDER_ENV), "{unset}");
        let allowed = semantic_embedder_profile_from_lookup(&|name| {
            Ok((name == ALLOW_DEV_EMBEDDER_ENV).then(|| "1".to_string()))
        })
        .expect("the opt-in resolves the dev embedder");
        assert!(allowed.is_dev());
        assert_eq!(allowed.selector(), DEV_HASH_EMBEDDER_SELECTOR);
        let misspelt = semantic_embedder_profile_from_lookup(&|name| {
            Ok((name == ALLOW_DEV_EMBEDDER_ENV).then(|| "yes".to_string()))
        })
        .expect_err("only `1` opts in");
        assert!(misspelt.to_string().contains("must be `1`"), "{misspelt}");
        let by_name = semantic_embedder_profile_from_lookup(&|name| {
            Ok((name == "QUANTA_INDEX_EMBEDDER").then(|| DEV_HASH_EMBEDDER_SELECTOR.to_string()))
        })
        .expect("hash-dev by name needs no opt-in");
        assert!(by_name.is_dev());
        let retired = semantic_embedder_profile_from_lookup(&|name| {
            Ok((name == "QUANTA_INDEX_EMBEDDER").then(|| "hash".to_string()))
        })
        .expect_err("the retired spelling is refused");
        assert!(retired.to_string().contains("retired"), "{retired}");
        let unavailable = semantic_embedder_profile_from_lookup(&|name| {
            Ok((name == "QUANTA_INDEX_EMBEDDER").then(|| "unavailable".to_string()))
        })
        .expect("unavailable by name");
        assert!(!unavailable.is_dev());
    }

    /// The one envelope (QI-BB-016): the config's resident byte policies
    /// sum under the ceiling, and a ceiling below their sum is refused
    /// typed by both entry points.
    #[test]
    fn the_process_memory_envelope_sums_the_policies_and_a_low_ceiling_is_refused_typed() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-envelope"));
        let envelope = config.process_memory_envelope().expect("the defaults fit");
        assert_eq!(envelope.lexical_writer_bytes, LexicalWriterPolicy::DEFAULT.envelope_bytes());
        assert_eq!(
            envelope.snapshot_registry_bytes,
            SnapshotRegistryPolicy::DEFAULT.max_resident_bytes()
        );
        assert_eq!(
            envelope.regex_match_cache_bytes,
            RegexMatchCachePolicy::DEFAULT.max_resident_bytes()
        );
        assert_eq!(envelope.embedding_cache_ledger_bytes, 0, "no cache under the hash profile");
        assert_eq!(
            envelope.semantic_stream_window_bytes,
            SemanticStreamWindowPolicy::DEFAULT.max_vector_bytes()
        );
        assert_eq!(
            envelope.ingest_batch_bytes,
            IngestResourcePolicy::DEFAULT.max_text_bytes()
                + IngestResourcePolicy::DEFAULT.max_vector_bytes()
        );
        assert!(envelope.declared_bytes() <= ProcessMemoryEnvelopeV1::DEFAULT_CEILING_BYTES);
        assert_eq!(envelope.rss_ceiling, None);

        let too_low = config
            .with_process_memory_ceilings(
                ProcessMemoryCeilings::new(envelope.declared_bytes() - 1, None).expect("ceiling"),
            )
            .process_memory_envelope()
            .expect_err("a ceiling below the declared sum is refused");
        assert!(
            too_low
                .to_string()
                .contains(quanta_index_core::PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE.as_wire_str()),
            "{too_low}"
        );
        // The chain refuses it too, naming the family.
        let mut knobs = every_knob_non_default();
        let _was = knobs.insert("QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES", "1");
        let _was = knobs.insert("QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES", "1");
        let lookup = |name: &str| -> Result<Option<String>> {
            Ok(knobs.get(name).map(|value| (*value).to_string()))
        };
        let refused = SearchdConfig::from_lookup(Some(PathBuf::from("/tmp/x")), &lookup)
            .expect_err("the chain validates the envelope");
        assert!(
            refused
                .to_string()
                .contains(quanta_index_core::PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE.as_wire_str()),
            "{refused}"
        );
        // An OpenAI profile with the cache declares the ledger's bound.
        let cached = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-envelope"))
            .with_semantic_embedder_profile(SemanticEmbedderProfile::OpenAi {
                model: "m".to_string(),
                model_revision: "r".to_string(),
                dimension: 8,
                api_key: "sk".to_string(),
                tuning: OpenAiEmbedderTuning::default(),
            })
            .process_memory_envelope()
            .expect("fits");
        assert_eq!(
            cached.embedding_cache_ledger_bytes,
            EmbeddingCacheRetentionPolicy::DEFAULT.max_entries()
                * EMBEDDING_CACHE_LEDGER_BYTES_PER_ENTRY
        );
    }

    #[test]
    fn socket_access_defaults_to_private_everywhere_and_the_builder_sets_it() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"));
        assert_eq!(config.socket_access_policies(), &SocketAccessPolicies::PRIVATE);
        let shared = SocketAccessPolicies::new(
            quanta_index_ipc::SocketAccessPolicy::Shared(
                quanta_index_ipc::SharedSocketAccess::new(
                    Some(2000),
                    std::collections::BTreeSet::new(),
                ),
            ),
            quanta_index_ipc::SocketAccessPolicy::Private,
            quanta_index_ipc::SocketAccessPolicy::Private,
        );
        let config = config.with_socket_access_policies(shared.clone());
        assert_eq!(config.socket_access_policies(), &shared);
    }

    #[test]
    fn provider_unavailable_builder_sets_unavailable_profile() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"))
            .with_provider_unavailable_query_text_embedder();
        assert_eq!(config.semantic_embedder_profile(), &SemanticEmbedderProfile::Unavailable);
    }

    #[test]
    fn search_corpus_history_retention_is_required_and_validated() {
        let missing = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"));
        assert!(missing.search_corpus_history_retention_policy().is_err());

        let too_small = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"))
            .try_with_search_corpus_history_retention_limits(1, 1024, 8, 8192);
        assert!(too_small.is_err());

        let configured =
            SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"))
                .try_with_search_corpus_history_retention_limits(3, 4096, 17, 65_536)
                .expect("valid explicit retention limits");
        let policy = configured
            .search_corpus_history_retention_policy()
            .expect("valid explicit retention policy");
        assert_eq!(policy.max_generations(), 3);
        assert_eq!(policy.max_bytes(), 4096);
        assert_eq!(policy.max_revision_pairs(), 17);
        assert_eq!(policy.max_total_bytes(), 65_536);
    }

    #[test]
    fn search_corpus_history_retention_env_binding_requires_all_four_knobs() {
        const NAMES: [&str; 4] = [
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
        ];
        for missing in NAMES {
            let result = search_corpus_history_retention_policy_from_lookup_v1(&|name| {
                if name == missing {
                    return Ok(None);
                }
                Ok(match name {
                    "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS" => Some("3".to_string()),
                    "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES" => Some("4096".to_string()),
                    "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS" => {
                        Some("17".to_string())
                    }
                    "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES" => {
                        Some("65536".to_string())
                    }
                    _ => None,
                })
            });
            let error = result.expect_err("missing required retention knob must fail closed");
            assert!(error.to_string().contains(missing));
        }

        let policy = search_corpus_history_retention_policy_from_lookup_v1(&|name| {
            Ok(match name {
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS" => Some("3".to_string()),
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES" => Some("4096".to_string()),
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS" => Some("17".to_string()),
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES" => Some("65536".to_string()),
                _ => None,
            })
        })
        .expect("all required retention env bindings");
        assert_eq!(policy.max_generations(), 3);
        assert_eq!(policy.max_bytes(), 4096);
        assert_eq!(policy.max_revision_pairs(), 17);
        assert_eq!(policy.max_total_bytes(), 65_536);
    }

    #[test]
    fn snapshot_registry_env_binding_is_all_or_nothing_and_nonzero() {
        const ENTRIES: &str = "QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES";
        const BYTES: &str = "QUANTA_INDEX_SNAPSHOT_MAX_RESIDENT_BYTES";
        let unset = snapshot_registry_policy_from_lookup(&|_name| Ok(None))
            .expect("no knobs selects the default");
        assert_eq!(unset, SnapshotRegistryPolicy::DEFAULT);

        let half = snapshot_registry_policy_from_lookup(&|name| {
            Ok((name == ENTRIES).then(|| "4".to_string()))
        })
        .expect_err("one knob without the other must fail closed");
        assert!(half.to_string().contains(BYTES));

        let zero = snapshot_registry_policy_from_lookup(&|name| {
            Ok(match name {
                ENTRIES => Some("0".to_string()),
                BYTES => Some("1024".to_string()),
                _ => None,
            })
        })
        .expect_err("zero entries must fail closed");
        assert!(zero.to_string().contains(ENTRIES));

        let explicit = snapshot_registry_policy_from_lookup(&|name| {
            Ok(match name {
                ENTRIES => Some("4".to_string()),
                BYTES => Some("1024".to_string()),
                _ => None,
            })
        })
        .expect("both knobs bind");
        assert_eq!(explicit.max_entries(), 4);
        assert_eq!(explicit.max_resident_bytes(), 1_024);
    }

    #[test]
    fn lexical_execution_budget_env_binding_defaults_and_refuses_zero() {
        const NAME: &str = "QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES";
        let unset =
            lexical_execution_budget_from_lookup(&|_name| Ok(None)).expect("unset selects default");
        assert_eq!(unset, LexicalExecutionBudgetV1::DEFAULT);
        let zero = lexical_execution_budget_from_lookup(&|name| {
            Ok((name == NAME).then(|| "0".to_string()))
        })
        .expect_err("zero must fail closed");
        assert!(zero.to_string().contains(NAME));
        let explicit = lexical_execution_budget_from_lookup(&|name| {
            Ok((name == NAME).then(|| "12".to_string()))
        })
        .expect("explicit budget binds");
        assert_eq!(explicit.max_examined_candidates(), 12);
    }

    #[test]
    fn query_admission_env_binding_layers_over_the_default_and_validates_as_one_policy() {
        const SLOTS: &str = "QUANTA_INDEX_QUERY_DISPATCH_SLOTS";
        const CONNECTIONS: &str = "QUANTA_INDEX_QUERY_MAX_CONNECTIONS";
        const QUEUE_WAIT: &str = "QUANTA_INDEX_QUERY_QUEUE_WAIT_MS";
        const BUDGET: &str = "QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS";
        let unset =
            query_admission_policy_from_lookup(&|_name| Ok(None)).expect("unset selects default");
        assert_eq!(unset, ServerAdmissionPolicy::DEFAULT);

        let slots_only =
            query_admission_policy_from_lookup(
                &|name| Ok((name == SLOTS).then(|| "2".to_string())),
            )
            .expect("one knob layers over the default");
        assert_eq!(slots_only.dispatch_slots(), 2);
        assert_eq!(slots_only.max_connections(), ServerAdmissionPolicy::DEFAULT.max_connections());
        assert_eq!(slots_only.dispatch_budget(), ServerAdmissionPolicy::DEFAULT.dispatch_budget());

        let zero_wait = query_admission_policy_from_lookup(&|name| {
            Ok((name == QUEUE_WAIT).then(|| "0".to_string()))
        })
        .expect("a zero queue wait is a valid immediate-refusal policy");
        assert_eq!(zero_wait.queue_wait(), Duration::ZERO);

        let zero_budget = query_admission_policy_from_lookup(&|name| {
            Ok((name == BUDGET).then(|| "0".to_string()))
        })
        .expect_err("a zero dispatch budget must fail closed");
        assert!(zero_budget.to_string().contains(BUDGET));

        let inverted = query_admission_policy_from_lookup(&|name| {
            Ok(match name {
                CONNECTIONS => Some("2".to_string()),
                SLOTS => Some("3".to_string()),
                _ => None,
            })
        })
        .expect_err("more slots than connections is refused as one policy");
        assert!(
            inverted
                .to_string()
                .contains("not a valid admission policy")
        );

        let garbage = query_admission_policy_from_lookup(&|name| {
            Ok((name == QUEUE_WAIT).then(|| "soon".to_string()))
        })
        .expect_err("non-numeric wait fails closed");
        assert!(garbage.to_string().contains(QUEUE_WAIT));
    }

    #[test]
    fn openai_model_revision_is_required_and_must_be_a_token() {
        let unset = openai_model_revision_from_lookup(&|_name| Ok(None))
            .expect_err("an unset revision must fail closed");
        assert!(
            unset
                .to_string()
                .contains("QUANTA_INDEX_EMBED_MODEL_REVISION"),
            "{unset}"
        );
        let blank = openai_model_revision_from_lookup(&|_name| Ok(Some("   ".to_string())))
            .expect_err("a blank revision must fail closed");
        assert!(blank.to_string().contains("non-empty"), "{blank}");
        let spaced = openai_model_revision_from_lookup(&|_name| Ok(Some("2024 01".to_string())))
            .expect_err("whitespace inside the revision must fail closed");
        assert!(spaced.to_string().contains("printable"), "{spaced}");
        let pinned = openai_model_revision_from_lookup(&|_name| Ok(Some(" 2024-01 ".to_string())))
            .expect("a trimmed token binds");
        assert_eq!(pinned, "2024-01");
    }

    #[test]
    fn regex_match_cache_env_binding_layers_over_the_default_and_refuses_zero() {
        const BYTES: &str = "QUANTA_INDEX_REGEX_CACHE_MAX_RESIDENT_BYTES";
        let unset =
            regex_match_cache_policy_from_lookup(&|_name| Ok(None)).expect("unset selects default");
        assert_eq!(unset, RegexMatchCachePolicy::DEFAULT);
        let bytes_only = regex_match_cache_policy_from_lookup(&|name| {
            Ok((name == BYTES).then(|| "4096".to_string()))
        })
        .expect("one knob layers over the default");
        assert_eq!(bytes_only.max_resident_bytes(), 4096);
        assert_eq!(bytes_only.max_entries(), RegexMatchCachePolicy::DEFAULT.max_entries());
        let zero = regex_match_cache_policy_from_lookup(&|name| {
            Ok((name == BYTES).then(|| "0".to_string()))
        })
        .expect_err("zero must fail closed");
        assert!(zero.to_string().contains(BYTES));
    }

    #[test]
    fn lexical_writer_env_binding_layers_over_the_default_and_refuses_bad_envelopes() {
        const ENVELOPE: &str = "QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES";
        const HEAP: &str = "QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES";
        const IDLE_SECS: &str = "QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS";
        let unset =
            lexical_writer_policy_from_lookup(&|_name| Ok(None)).expect("unset selects default");
        assert_eq!(unset, LexicalWriterPolicy::DEFAULT);
        assert_eq!(unset.max_writers(), 16);
        // A wider per-writer heap under the same envelope means fewer writers.
        let wider = lexical_writer_policy_from_lookup(&|name| {
            Ok((name == HEAP).then(|| "60000000".to_string()))
        })
        .expect("one knob layers over the default");
        assert_eq!(wider.max_writers(), 4);
        assert_eq!(wider.idle_after(), LexicalWriterPolicy::DEFAULT.idle_after());
        let idle = lexical_writer_policy_from_lookup(&|name| {
            Ok((name == IDLE_SECS).then(|| "5".to_string()))
        })
        .expect("idle layers over the default");
        assert_eq!(idle.idle_after(), Duration::from_secs(5));
        // A heap below the writer's minimum, an envelope smaller than one
        // writer, and a zero idle interval all fail closed.
        let small_heap = lexical_writer_policy_from_lookup(&|name| {
            Ok((name == HEAP).then(|| "1000".to_string()))
        })
        .expect_err("a heap below the minimum must fail closed");
        assert!(small_heap.to_string().contains("outside"));
        let small_envelope = lexical_writer_policy_from_lookup(&|name| {
            Ok((name == ENVELOPE).then(|| "1".to_string()))
        })
        .expect_err("an envelope smaller than one writer must fail closed");
        assert!(
            small_envelope
                .to_string()
                .contains("cannot hold one writer")
        );
        let zero_idle = lexical_writer_policy_from_lookup(&|name| {
            Ok((name == IDLE_SECS).then(|| "0".to_string()))
        })
        .expect_err("zero idle must fail closed");
        assert!(zero_idle.to_string().contains(IDLE_SECS));
    }

    #[test]
    fn unset_tuning_falls_back_to_provider_defaults() {
        // None for every knob -> exactly the provider defaults (no drift).
        assert_eq!(parse_embed_batch(None, DEFAULT_MAX_BATCH).expect("ok"), DEFAULT_MAX_BATCH);
        assert_eq!(
            parse_embed_max_retries(None, DEFAULT_MAX_RETRIES).expect("ok"),
            DEFAULT_MAX_RETRIES
        );
        assert_eq!(parse_embed_timeout(None, DEFAULT_TIMEOUT).expect("ok"), DEFAULT_TIMEOUT);
        assert!(parse_embed_cache_enabled(None, true).expect("ok"));
        // Empty string is treated as unset, not an error.
        assert_eq!(parse_embed_batch(Some(""), 256).expect("ok"), 256);
        assert_eq!(
            parse_embed_max_estimated_tokens(None, DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST)
                .expect("ok"),
            DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST
        );
    }

    #[test]
    fn batch_knob_parses_and_rejects_zero_and_garbage() {
        assert_eq!(parse_embed_batch(Some("32"), 256).expect("ok"), 32);
        assert_eq!(parse_embed_batch(Some("  8 "), 256).expect("trim"), 8);
        assert!(parse_embed_batch(Some("0"), 256).is_err(), "0 batch rejected");
        assert!(parse_embed_batch(Some("nope"), 256).is_err(), "garbage rejected");
    }

    #[test]
    fn estimated_token_knob_parses_and_rejects_zero_and_garbage() {
        assert_eq!(parse_embed_max_estimated_tokens(Some("2048"), 4096).expect("ok"), 2048);
        assert_eq!(parse_embed_max_estimated_tokens(Some("  512 "), 4096).expect("trim"), 512);
        assert!(parse_embed_max_estimated_tokens(Some("0"), 4096).is_err());
        assert!(parse_embed_max_estimated_tokens(Some("x"), 4096).is_err());
    }

    #[test]
    fn max_retries_knob_parses_zero_through_n() {
        // 0 is valid here (disables retry); negatives / garbage fail closed.
        assert_eq!(parse_embed_max_retries(Some("0"), 3).expect("ok"), 0);
        assert_eq!(parse_embed_max_retries(Some("5"), 3).expect("ok"), 5);
        assert!(parse_embed_max_retries(Some("-1"), 3).is_err());
        assert!(parse_embed_max_retries(Some("x"), 3).is_err());
    }

    #[test]
    fn timeout_knob_parses_seconds_and_rejects_zero() {
        assert_eq!(
            parse_embed_timeout(Some("10"), DEFAULT_TIMEOUT).expect("ok"),
            Duration::from_secs(10)
        );
        assert!(parse_embed_timeout(Some("0"), DEFAULT_TIMEOUT).is_err(), "0s rejected");
        assert!(parse_embed_timeout(Some("abc"), DEFAULT_TIMEOUT).is_err());
    }

    #[test]
    fn tuning_raw_mapper_threads_estimated_token_budget_into_tuning() {
        let tuning = openai_tuning_from_raw(
            Some("8"),
            Some("1024"),
            Some("5"),
            Some("10"),
            Some("false"),
            Some("3"),
        )
        .expect("raw tuning parses");
        assert_eq!(tuning.max_batch, 8);
        assert_eq!(tuning.max_estimated_tokens_per_request, 1024);
        assert_eq!(tuning.max_retries, 5);
        assert_eq!(tuning.timeout, Duration::from_secs(10));
        assert!(!tuning.cache_enabled);
        assert_eq!(tuning.concurrency, 3);
    }

    #[test]
    fn cache_knob_parses_truthy_and_falsy_forms() {
        for truthy in ["1", "true", "TRUE", "on", "yes"] {
            assert!(parse_embed_cache_enabled(Some(truthy), false).expect("ok"), "{truthy}");
        }
        for falsy in ["0", "false", "OFF", "no"] {
            assert!(!parse_embed_cache_enabled(Some(falsy), true).expect("ok"), "{falsy}");
        }
        assert!(parse_embed_cache_enabled(Some("maybe"), true).is_err());
    }

    #[test]
    fn dim_knob_parses_and_rejects_garbage() {
        assert_eq!(parse_embed_dim(Some("1536"), 64).expect("ok"), 1536);
        assert_eq!(parse_embed_dim(None, 64).expect("default"), 64);
        assert!(parse_embed_dim(Some("big"), 64).is_err());
        assert!(parse_embed_dim(Some("0"), 64).is_err(), "zero is refused");
        assert!(
            parse_embed_dim(Some(&(MAX_EMBEDDING_DIMENSION + 1).to_string()), 64).is_err(),
            "past the ceiling is refused"
        );
        assert_eq!(
            parse_embed_dim(Some(&MAX_EMBEDDING_DIMENSION.to_string()), 64).expect("at ceiling"),
            MAX_EMBEDDING_DIMENSION
        );
    }

    #[test]
    fn tuning_assembler_maps_each_env_knob_to_its_own_field() {
        // DISTINCT values per knob so a cross-wire (e.g. binding BATCH into
        // max_retries) cannot pass: each field must equal its own source.
        let tuning = openai_tuning_from_raw(
            Some("7"),
            Some("1024"),
            Some("2"),
            Some("11"),
            Some("off"),
            Some("5"),
        )
        .expect("assembles");
        assert_eq!(tuning.max_batch, 7, "BATCH knob -> max_batch");
        assert_eq!(
            tuning.max_estimated_tokens_per_request, 1024,
            "MAX_EST_TOKENS knob -> max_estimated_tokens_per_request"
        );
        assert_eq!(tuning.max_retries, 2, "MAX_RETRIES knob -> max_retries");
        assert_eq!(tuning.timeout, Duration::from_secs(11), "TIMEOUT_SECS knob -> timeout");
        assert!(!tuning.cache_enabled, "CACHE=off -> cache_enabled false");
        assert_eq!(tuning.concurrency, 5, "CONCURRENCY knob -> concurrency");
    }

    #[test]
    fn tuning_assembler_unset_knobs_fall_back_to_defaults() {
        let tuning = openai_tuning_from_raw(None, None, None, None, None, None).expect("assembles");
        assert_eq!(tuning, OpenAiEmbedderTuning::default());
    }

    #[test]
    fn tuning_assembler_propagates_a_bad_knob_as_error() {
        // A single garbage knob fails closed (no silent default substitution).
        assert!(openai_tuning_from_raw(Some("nope"), None, None, None, None, None).is_err());
        assert!(openai_tuning_from_raw(None, Some("0"), None, None, None, None).is_err());
        // concurrency=0 is rejected (would mean "no dispatch"), and so is a
        // pool wider than the provider's ceiling.
        assert!(openai_tuning_from_raw(None, None, None, None, None, Some("0")).is_err());
        let over = (MAX_CONCURRENCY + 1).to_string();
        assert!(openai_tuning_from_raw(None, None, None, None, None, Some(&over)).is_err());
        let at = MAX_CONCURRENCY.to_string();
        assert_eq!(
            openai_tuning_from_raw(None, None, None, None, None, Some(&at))
                .expect("at the ceiling")
                .concurrency,
            MAX_CONCURRENCY
        );
    }

    #[test]
    fn cache_retention_env_binds_each_knob_and_refuses_zero() {
        let lookup = |name: &str| -> Result<Option<String>> {
            Ok(match name {
                "QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES" => Some("77".to_string()),
                "QUANTA_INDEX_EMBED_CACHE_MAX_BYTES" => Some("4096".to_string()),
                "QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES" => Some("2".to_string()),
                _ => None,
            })
        };
        let tuning = openai_tuning_from_env_with(&lookup).expect("assembles");
        assert_eq!(tuning.cache_retention.max_entries(), 77);
        assert_eq!(tuning.cache_retention.max_resident_bytes(), 4096);
        assert_eq!(tuning.cache_retention.max_namespaces(), 2);
        let unset = |_name: &str| -> Result<Option<String>> { Ok(None) };
        assert_eq!(
            openai_tuning_from_env_with(&unset)
                .expect("defaults")
                .cache_retention,
            EmbeddingCacheRetentionPolicy::DEFAULT
        );
        let zero = |name: &str| -> Result<Option<String>> {
            Ok((name == "QUANTA_INDEX_EMBED_CACHE_MAX_BYTES").then(|| "0".to_string()))
        };
        assert!(openai_tuning_from_env_with(&zero).is_err(), "zero is refused");
    }

    #[test]
    fn ingest_resource_env_binds_each_knob_and_refuses_zero() {
        let lookup = |name: &str| -> Result<Option<String>> {
            Ok(match name {
                "QUANTA_INDEX_INGEST_MAX_RECORDS" => Some("12".to_string()),
                "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES" => Some("3456".to_string()),
                "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES" => Some("789".to_string()),
                _ => None,
            })
        };
        let policy = ingest_resource_policy_from_lookup(&lookup).expect("assembles");
        assert_eq!(policy.max_records(), 12);
        assert_eq!(policy.max_text_bytes(), 3456);
        assert_eq!(policy.max_vector_bytes(), 789);
        let unset = |_name: &str| -> Result<Option<String>> { Ok(None) };
        assert_eq!(
            ingest_resource_policy_from_lookup(&unset).expect("defaults"),
            IngestResourcePolicy::DEFAULT
        );
        let zero = |name: &str| -> Result<Option<String>> {
            Ok((name == "QUANTA_INDEX_INGEST_MAX_RECORDS").then(|| "0".to_string()))
        };
        assert!(ingest_resource_policy_from_lookup(&zero).is_err(), "zero is refused");
    }

    #[test]
    fn semantic_stream_window_env_binds_each_knob_and_refuses_zero() {
        let lookup = |name: &str| -> Result<Option<String>> {
            Ok(match name {
                "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES" => Some("8".to_string()),
                "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES" => Some("2048".to_string()),
                _ => None,
            })
        };
        let policy = semantic_stream_window_policy_from_lookup(&lookup).expect("assembles");
        assert_eq!(policy.max_owner_scopes(), 8);
        assert_eq!(policy.max_vector_bytes(), 2048);
        let unset = |_name: &str| -> Result<Option<String>> { Ok(None) };
        assert_eq!(
            semantic_stream_window_policy_from_lookup(&unset).expect("defaults"),
            SemanticStreamWindowPolicy::DEFAULT
        );
        let zero = |name: &str| -> Result<Option<String>> {
            Ok((name == "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES")
                .then(|| "0".to_string()))
        };
        assert!(semantic_stream_window_policy_from_lookup(&zero).is_err(), "zero is refused");
        let garbage = |name: &str| -> Result<Option<String>> {
            Ok((name == "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES").then(|| "many".to_string()))
        };
        assert!(
            semantic_stream_window_policy_from_lookup(&garbage).is_err(),
            "a non-numeric knob is refused"
        );
    }

    #[test]
    fn from_env_binds_each_env_var_name_to_its_own_tuning_field() {
        // The assembler tests prove positional args map to fields; this proves the
        // ENV-VAR-NAME -> position binding in openai_tuning_from_env_with. Distinct
        // values (7 / 1234 / 2 / 11 / off) so a name<->field cross-wire (e.g. reading
        // MAX_RETRIES into the batch slot, or a typo'd env name returning None ->
        // default) is caught. Uses an injected lookup — no process-env mutation.
        let lookup = |name: &str| -> Result<Option<String>> {
            Ok(match name {
                "QUANTA_INDEX_EMBED_BATCH" => Some("7".to_string()),
                "QUANTA_INDEX_EMBED_MAX_EST_TOKENS" => Some("1234".to_string()),
                "QUANTA_INDEX_EMBED_MAX_RETRIES" => Some("2".to_string()),
                "QUANTA_INDEX_EMBED_TIMEOUT_SECS" => Some("11".to_string()),
                "QUANTA_INDEX_EMBED_CACHE" => Some("off".to_string()),
                "QUANTA_INDEX_EMBED_CONCURRENCY" => Some("5".to_string()),
                _ => None,
            })
        };
        let tuning =
            openai_tuning_from_env_with(&lookup).expect("env tuning assembles from valid knobs");
        assert_eq!(tuning.max_batch, 7, "QUANTA_INDEX_EMBED_BATCH -> max_batch");
        assert_eq!(
            tuning.max_estimated_tokens_per_request, 1234,
            "QUANTA_INDEX_EMBED_MAX_EST_TOKENS -> max_estimated_tokens_per_request"
        );
        assert_eq!(tuning.max_retries, 2, "QUANTA_INDEX_EMBED_MAX_RETRIES -> max_retries");
        assert_eq!(
            tuning.timeout,
            Duration::from_secs(11),
            "QUANTA_INDEX_EMBED_TIMEOUT_SECS -> timeout"
        );
        assert!(!tuning.cache_enabled, "QUANTA_INDEX_EMBED_CACHE=off -> cache_enabled=false");
        assert_eq!(tuning.concurrency, 5, "QUANTA_INDEX_EMBED_CONCURRENCY -> concurrency");
    }

    #[test]
    fn provider_config_threads_every_tuning_knob_to_its_setter() {
        // Distinct, non-default values so a swapped/dropped setter is caught: the
        // produced OpenAiProviderConfig must carry exactly this tuning.
        let tuning = OpenAiEmbedderTuning {
            max_batch: 13,
            max_estimated_tokens_per_request: 8192,
            max_retries: 4,
            timeout: Duration::from_secs(9),
            cache_enabled: false,
            cache_retention: EmbeddingCacheRetentionPolicy::DEFAULT,
            concurrency: 6,
        };
        let config = tuning.provider_config(
            "text-embedding-3-large".to_string(),
            "2025-02".to_string(),
            3072,
            "sk-unit-test".to_string(),
        );
        assert_eq!(config.model_revision, "2025-02");
        assert_eq!(config.max_batch, 13);
        assert_eq!(config.max_estimated_tokens_per_request, 8192);
        assert_eq!(config.max_retries, 4);
        assert_eq!(config.timeout, Duration::from_secs(9));
        assert_eq!(config.concurrency, 6);
        assert_eq!(config.model, "text-embedding-3-large");
        assert_eq!(config.dimension, 3072);
        assert_eq!(config.api_key, "sk-unit-test");
    }

    #[test]
    fn integrity_scrub_env_binding_is_all_or_nothing_and_nonzero() {
        const INTERVAL: &str = "QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS";
        const BYTES: &str = "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP";
        let unset = integrity_scrub_policy_from_lookup(&|_name: &str| Ok(None))
            .expect("no knobs selects the default");
        assert_eq!(unset, IntegrityScrubPolicyV1::DEFAULT);

        let half = integrity_scrub_policy_from_lookup(&|name: &str| {
            Ok((name == INTERVAL).then(|| "250".to_string()))
        })
        .expect_err("one knob without the other must fail closed");
        assert!(half.to_string().contains(BYTES));

        let zero = integrity_scrub_policy_from_lookup(&|name: &str| {
            Ok(match name {
                INTERVAL => Some("250".to_string()),
                BYTES => Some("0".to_string()),
                _ => None,
            })
        })
        .expect_err("a zero byte budget must fail closed");
        assert!(zero.to_string().contains(BYTES));

        let explicit = integrity_scrub_policy_from_lookup(&|name: &str| {
            Ok(match name {
                INTERVAL => Some("250".to_string()),
                BYTES => Some("1024".to_string()),
                _ => None,
            })
        })
        .expect("both knobs set is an explicit policy");
        assert_eq!(explicit, IntegrityScrubPolicyV1::new(250, 1024).expect("a positive policy"));
    }
}
