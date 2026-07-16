use std::path::PathBuf;

use anyhow::Result;
use quanta_index_search_plane::readiness::SearchCorpusHistoryRetentionPolicyV1;

use crate::app::config::{SearchdConfig, SemanticEmbedderProfile};

/// CLI subcommand surface. Today: `serve [--state-root PATH]`.
#[derive(Clone, Debug)]
pub struct SearchdCommand {
    state_root_override: Option<PathBuf>,
}

impl SearchdCommand {
    pub fn from_env() -> Result<Self> {
        let mut state_root_override: Option<PathBuf> = None;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => {}
                "--state-root" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--state-root requires a path"))?;
                    state_root_override = Some(PathBuf::from(next));
                }
                other => return Err(anyhow::anyhow!("unknown argument: {other}")),
            }
        }
        Ok(Self {
            state_root_override,
        })
    }

    pub fn into_config(self) -> Result<SearchdConfig> {
        self.into_config_with_v1(
            crate::app::config::search_corpus_history_retention_policy_from_env,
            crate::app::config::semantic_embedder_profile_from_env,
        )
    }

    fn into_config_with_v1<Retention, Embedder>(
        self,
        retention_from_env: Retention,
        embedder_from_env: Embedder,
    ) -> Result<SearchdConfig>
    where
        Retention: FnOnce() -> Result<SearchCorpusHistoryRetentionPolicyV1>,
        Embedder: FnOnce() -> Result<SemanticEmbedderProfile>,
    {
        if let Some(root) = self.state_root_override {
            // The semantic embedder profile is env-driven regardless of how the
            // state root was resolved, so `--state-root` still honors
            // QUANTA_INDEX_EMBEDDER (otherwise an explicit state root would
            // silently force the hash embedder).
            let retention = retention_from_env()?;
            return Ok(SearchdConfig::from_state_root(root)
                .with_search_corpus_history_retention_policy_v1(retention)
                .with_semantic_embedder_profile(embedder_from_env()?));
        }
        SearchdConfig::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_root_override_preserves_all_required_retention_authority() {
        let command = SearchdCommand {
            state_root_override: Some(PathBuf::from("/tmp/quanta-index-cli-retention")),
        };
        let config = command
            .into_config_with_v1(
                || {
                    SearchCorpusHistoryRetentionPolicyV1::new(3, 4096, 17, 65_536)
                        .map_err(anyhow::Error::from)
                },
                || Ok(SemanticEmbedderProfile::Unavailable),
            )
            .expect("explicit state-root config");
        let policy = config
            .search_corpus_history_retention_policy()
            .expect("launcher must preserve required retention policy");
        assert_eq!(policy.max_generations(), 3);
        assert_eq!(policy.max_bytes(), 4096);
        assert_eq!(policy.max_revision_pairs(), 17);
        assert_eq!(policy.max_total_bytes(), 65_536);
    }
}
