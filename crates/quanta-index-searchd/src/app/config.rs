use std::path::{Path, PathBuf};

use anyhow::Result;
use quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION;

/// How `searchd` resolves the semantic embedder for BOTH the query path and
/// corpus derivation. One profile drives both, so the two sides can never
/// disagree on model identity (the query-time model-identity gate then holds).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticEmbedderProfile {
    /// Deterministic FNV-1a hash embedder (default; no network, free).
    Hash { dimension: usize },
    /// No query-time embedder is configured: semantic/hybrid queries fail closed
    /// (`SEM_PROVIDER_UNAVAILABLE`) AND the corpus derives no semantics — so there
    /// is never a populated-but-unqueryable semantic index.
    Unavailable,
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
/// hash embedder; an unknown selector or a not-yet-wired provider fails closed
/// (no silent fallback). `openai` is reserved for the OpenAI provider phase.
fn semantic_embedder_profile_from_env() -> Result<SemanticEmbedderProfile> {
    match std::env::var("QUANTA_INDEX_EMBEDDER").ok().as_deref() {
        None | Some("") | Some("hash") => Ok(SemanticEmbedderProfile::Hash {
            dimension: embed_dim_from_env(SEARCH_OWNED_SEMANTIC_DIMENSION)?,
        }),
        Some("unavailable") => Ok(SemanticEmbedderProfile::Unavailable),
        Some("openai") => Err(anyhow::anyhow!(
            "QUANTA_INDEX_EMBEDDER=openai is not yet wired (pending the OpenAI provider); use 'hash' or 'unavailable'"
        )),
        Some(other) => Err(anyhow::anyhow!(
            "unknown QUANTA_INDEX_EMBEDDER '{other}' (expected hash|unavailable|openai)"
        )),
    }
}

fn embed_dim_from_env(default: usize) -> Result<usize> {
    match std::env::var("QUANTA_INDEX_EMBED_DIM") {
        Ok(raw) => raw.trim().parse::<usize>().map_err(|err| {
            anyhow::anyhow!("invalid QUANTA_INDEX_EMBED_DIM '{raw}': {err}")
        }),
        Err(_) => Ok(default),
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
}
