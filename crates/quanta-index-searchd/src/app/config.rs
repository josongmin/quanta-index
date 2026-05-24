use std::path::{Path, PathBuf};

use crate::cli::ServeOptions;

#[derive(Clone, Debug)]
pub struct SearchdConfig {
    pub state_root: PathBuf,
    pub control_plane_path: PathBuf,
    pub socket_path: PathBuf,
}

impl SearchdConfig {
    #[must_use]
    pub fn from_env() -> Self {
        if let Some(state_root) = std::env::var_os("QUANTA_INDEX_STATE_ROOT") {
            return Self::from_state_root(PathBuf::from(state_root));
        }

        let cache_root = default_cache_root();
        Self::from_cache_root(&cache_root)
    }

    #[must_use]
    pub fn from_state_root(state_root: PathBuf) -> Self {
        let control_plane_path = state_root.join("control-plane.sqlite3");
        let socket_path = state_root.join("search-plane").join("searchd.sock");

        Self {
            state_root,
            control_plane_path,
            socket_path,
        }
    }

    #[must_use]
    pub fn from_cache_root(cache_root: &Path) -> Self {
        Self::from_state_root(cache_root.join("state"))
    }

    /// Layer operator overrides on top of an environment-derived config.
    ///
    /// CLI flag values win over `QUANTA_INDEX_STATE_ROOT` / cache-root
    /// defaults: `--state-root` re-derives all paths; `--socket-path`
    /// overrides only the socket without touching the control-plane DB path.
    #[must_use]
    pub fn with_overrides(self, overrides: &ServeOptions) -> Self {
        let base = overrides
            .state_root
            .as_ref()
            .map_or(self, |root| Self::from_state_root(root.clone()));
        let socket_path = match overrides.socket_path.as_ref() {
            Some(p) => p.clone(),
            None => base.socket_path.clone(),
        };
        Self {
            socket_path,
            ..base
        }
    }
}

fn default_cache_root() -> PathBuf {
    if let Some(root) = std::env::var_os("QUANTA_INDEX_CACHE_ROOT") {
        return PathBuf::from(root);
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);

    #[cfg(target_os = "macos")]
    {
        home.map_or_else(
            || PathBuf::from("./state"),
            |path| path.join("Library/Caches/quanta-index"),
        )
    }

    #[cfg(not(target_os = "macos"))]
    {
        return std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|path| path.join(".cache")))
            .map(|path| path.join("quanta-index"))
            .unwrap_or_else(|| PathBuf::from("./state"));
    }
}

#[cfg(test)]
mod tests {
    use super::SearchdConfig;
    use std::path::{Path, PathBuf};

    #[test]
    fn builds_paths_from_state_root() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-state"));

        assert_eq!(config.state_root, PathBuf::from("/tmp/quanta-index-state"));
        assert_eq!(
            config.control_plane_path,
            PathBuf::from("/tmp/quanta-index-state/control-plane.sqlite3")
        );
        assert_eq!(
            config.socket_path,
            PathBuf::from("/tmp/quanta-index-state/search-plane/searchd.sock")
        );
    }

    #[test]
    fn builds_paths_from_cache_root() {
        let config = SearchdConfig::from_cache_root(Path::new("/tmp/quanta-index-cache"));

        assert_eq!(
            config.state_root,
            PathBuf::from("/tmp/quanta-index-cache/state")
        );
        assert_eq!(
            config.control_plane_path,
            PathBuf::from("/tmp/quanta-index-cache/state/control-plane.sqlite3")
        );
    }
}
