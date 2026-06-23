use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use quanta_index_embed::{DEFAULT_MAX_BATCH, DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT};
use quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION;

/// Default `OpenAI` embedding model and dimension when the `openai` profile is
/// selected without explicit overrides.
const DEFAULT_OPENAI_MODEL: &str = "text-embedding-3-small";
const DEFAULT_OPENAI_DIMENSION: usize = 1536;

/// Operational knobs for the `OpenAI` embedder, surfaced as daemon env so they can
/// be tuned without a code change. Defaults mirror the provider's own defaults
/// (single source of truth re-exported from `quanta_index_embed`); the embedding
/// cache is on by default to control cost on rebuild / incremental.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenAiEmbedderTuning {
    /// Max inputs per `/v1/embeddings` request (`QUANTA_INDEX_EMBED_BATCH`).
    pub max_batch: usize,
    /// Bounded retry count for transient failures (`QUANTA_INDEX_EMBED_MAX_RETRIES`).
    pub max_retries: u32,
    /// Per-request HTTP timeout (`QUANTA_INDEX_EMBED_TIMEOUT_SECS`).
    pub timeout: Duration,
    /// Whether the on-disk embedding cache is used (`QUANTA_INDEX_EMBED_CACHE`).
    pub cache_enabled: bool,
}

impl Default for OpenAiEmbedderTuning {
    fn default() -> Self {
        Self {
            max_batch: DEFAULT_MAX_BATCH,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
            cache_enabled: true,
        }
    }
}

/// How `searchd` resolves the semantic embedder for BOTH the query path and
/// corpus derivation. One profile drives both, so the two sides can never
/// disagree on model identity (the query-time model-identity gate then holds).
#[derive(Clone, Eq, PartialEq)]
pub enum SemanticEmbedderProfile {
    /// Deterministic FNV-1a hash embedder (default; no network, free).
    Hash { dimension: usize },
    /// Network-backed `OpenAI` embeddings. `api_key` is held here but redacted in
    /// `Debug` (R-SEC-01) and never logged. `tuning` carries the env-resolved
    /// operational knobs threaded into the provider/cache at the composition root.
    OpenAi {
        model: String,
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
            Self::Hash { dimension } => {
                f.debug_struct("Hash").field("dimension", dimension).finish()
            }
            Self::OpenAi {
                model,
                dimension,
                tuning,
                ..
            } => f
                .debug_struct("OpenAi")
                .field("model", model)
                .field("dimension", dimension)
                .field("api_key", &"<redacted>")
                .field("tuning", tuning)
                .finish(),
            Self::Unavailable => f.write_str("Unavailable"),
        }
    }
}

impl Default for SemanticEmbedderProfile {
    fn default() -> Self {
        Self::Hash {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        }
    }
}

/// Resolved runtime paths for one `searchd` instance.
#[derive(Clone, Debug)]
pub struct SearchdConfig {
    state_root: PathBuf,
    query_socket_path: PathBuf,
    control_socket_path: PathBuf,
    /// QI-RT-01: typed ingest socket. Producer-side SDK publishes typed
    /// batches here; searchd's ingest dispatcher applies them through the
    /// direct authority path.
    ingest_socket_path: PathBuf,
    semantic_embedder_profile: SemanticEmbedderProfile,
}

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
        }
    }

    pub fn from_env() -> Result<Self> {
        let base = Self::from_state_root(Self::resolve_state_root_from_env()?);
        Ok(base.with_semantic_embedder_profile(semantic_embedder_profile_from_env()?))
    }

    fn resolve_state_root_from_env() -> Result<PathBuf> {
        if let Ok(explicit) = std::env::var("QUANTA_INDEX_STATE_ROOT") {
            return Ok(PathBuf::from(explicit));
        }
        if let Ok(cache) = std::env::var("QUANTA_INDEX_CACHE_ROOT") {
            return Ok(PathBuf::from(cache).join("state"));
        }
        let home = std::env::var("HOME").map_err(|_err| {
            anyhow::anyhow!("cannot resolve state_root: HOME unset and no QUANTA_INDEX_* env vars")
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

    #[must_use]
    pub fn with_provider_unavailable_query_text_embedder(self) -> Self {
        self.with_semantic_embedder_profile(SemanticEmbedderProfile::Unavailable)
    }
}

/// Resolve the semantic embedder profile from env. Defaults to the deterministic
/// hash embedder; an unknown selector or a missing `OpenAI` key fails closed (no
/// silent fallback). Applied on BOTH config entry points (state-root override and
/// full from_env) so `QUANTA_INDEX_EMBEDDER` is honored regardless of how the
/// state root was resolved.
pub(crate) fn semantic_embedder_profile_from_env() -> Result<SemanticEmbedderProfile> {
    match std::env::var("QUANTA_INDEX_EMBEDDER").ok().as_deref() {
        None | Some("") | Some("hash") => Ok(SemanticEmbedderProfile::Hash {
            dimension: embed_dim_from_env(SEARCH_OWNED_SEMANTIC_DIMENSION)?,
        }),
        Some("unavailable") => Ok(SemanticEmbedderProfile::Unavailable),
        Some("openai") => {
            let api_key = std::env::var("OPENAI_API_KEY").map_err(|_err| {
                anyhow::anyhow!("QUANTA_INDEX_EMBEDDER=openai requires OPENAI_API_KEY to be set")
            })?;
            if api_key.trim().is_empty() {
                return Err(anyhow::anyhow!("OPENAI_API_KEY is set but empty"));
            }
            let model = std::env::var("QUANTA_INDEX_EMBED_MODEL")
                .unwrap_or_else(|_err| DEFAULT_OPENAI_MODEL.to_string());
            Ok(SemanticEmbedderProfile::OpenAi {
                model,
                dimension: embed_dim_from_env(DEFAULT_OPENAI_DIMENSION)?,
                api_key,
                tuning: openai_tuning_from_env()?,
            })
        }
        Some(other) => Err(anyhow::anyhow!(
            "unknown QUANTA_INDEX_EMBEDDER '{other}' (expected hash|unavailable|openai)"
        )),
    }
}

fn embed_dim_from_env(default: usize) -> Result<usize> {
    parse_embed_dim(std::env::var("QUANTA_INDEX_EMBED_DIM").ok().as_deref(), default)
}

/// Resolve the `OpenAI` embedder operational knobs from env, falling back to the
/// provider defaults for any unset knob. Each knob is parsed by a pure helper so
/// the env→tuning mapping is unit-testable without mutating process env.
fn openai_tuning_from_env() -> Result<OpenAiEmbedderTuning> {
    let defaults = OpenAiEmbedderTuning::default();
    Ok(OpenAiEmbedderTuning {
        max_batch: parse_embed_batch(
            std::env::var("QUANTA_INDEX_EMBED_BATCH").ok().as_deref(),
            defaults.max_batch,
        )?,
        max_retries: parse_embed_max_retries(
            std::env::var("QUANTA_INDEX_EMBED_MAX_RETRIES").ok().as_deref(),
            defaults.max_retries,
        )?,
        timeout: parse_embed_timeout(
            std::env::var("QUANTA_INDEX_EMBED_TIMEOUT_SECS").ok().as_deref(),
            defaults.timeout,
        )?,
        cache_enabled: parse_embed_cache_enabled(
            std::env::var("QUANTA_INDEX_EMBED_CACHE").ok().as_deref(),
            defaults.cache_enabled,
        )?,
    })
}

fn parse_embed_dim(raw: Option<&str>, default: usize) -> Result<usize> {
    match raw {
        None => Ok(default),
        Some(value) => value.trim().parse::<usize>().map_err(|err| {
            anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_DIM '{value}': {err}")
        }),
    }
}

fn parse_embed_batch(raw: Option<&str>, default: usize) -> Result<usize> {
    match raw {
        None | Some("") => Ok(default),
        Some(value) => {
            let parsed = value.trim().parse::<usize>().map_err(|err| {
                anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_BATCH '{value}': {err}")
            })?;
            if parsed == 0 {
                return Err(anyhow::anyhow!(
                    "QUANTA_INDEX_EMBED_BATCH must be >= 1, got 0"
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
                return Err(anyhow::anyhow!(
                    "QUANTA_INDEX_EMBED_TIMEOUT_SECS must be >= 1, got 0"
                ));
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

    #[test]
    fn default_profile_is_hash_at_search_owned_dimension() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"));
        assert_eq!(
            config.semantic_embedder_profile(),
            &SemanticEmbedderProfile::Hash {
                dimension: SEARCH_OWNED_SEMANTIC_DIMENSION
            }
        );
    }

    #[test]
    fn provider_unavailable_builder_sets_unavailable_profile() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-cfg-test"))
            .with_provider_unavailable_query_text_embedder();
        assert_eq!(
            config.semantic_embedder_profile(),
            &SemanticEmbedderProfile::Unavailable
        );
    }

    #[test]
    fn unset_tuning_falls_back_to_provider_defaults() {
        // None for every knob -> exactly the provider defaults (no drift).
        assert_eq!(
            parse_embed_batch(None, DEFAULT_MAX_BATCH).expect("ok"),
            DEFAULT_MAX_BATCH
        );
        assert_eq!(
            parse_embed_max_retries(None, DEFAULT_MAX_RETRIES).expect("ok"),
            DEFAULT_MAX_RETRIES
        );
        assert_eq!(
            parse_embed_timeout(None, DEFAULT_TIMEOUT).expect("ok"),
            DEFAULT_TIMEOUT
        );
        assert!(parse_embed_cache_enabled(None, true).expect("ok"));
        // Empty string is treated as unset, not an error.
        assert_eq!(parse_embed_batch(Some(""), 256).expect("ok"), 256);
    }

    #[test]
    fn batch_knob_parses_and_rejects_zero_and_garbage() {
        assert_eq!(parse_embed_batch(Some("32"), 256).expect("ok"), 32);
        assert_eq!(parse_embed_batch(Some("  8 "), 256).expect("trim"), 8);
        assert!(parse_embed_batch(Some("0"), 256).is_err(), "0 batch rejected");
        assert!(parse_embed_batch(Some("nope"), 256).is_err(), "garbage rejected");
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
    }
}
