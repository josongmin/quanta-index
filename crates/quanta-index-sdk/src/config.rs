use std::path::PathBuf;

use crate::SdkError;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectOptions {
    state_root: Option<PathBuf>,
    query_socket: Option<PathBuf>,
    control_socket: Option<PathBuf>,
    /// QI-SDK-01: typed ingest socket override. Defaults to
    /// `state_root/search-plane/ingest.sock`.
    ingest_socket: Option<PathBuf>,
}

impl ConnectOptions {
    #[must_use]
    pub fn from_state_root(path: impl Into<PathBuf>) -> Self {
        Self {
            state_root: Some(path.into()),
            query_socket: None,
            control_socket: None,
            ingest_socket: None,
        }
    }

    #[must_use]
    pub fn with_query_socket(mut self, path: impl Into<PathBuf>) -> Self {
        self.query_socket = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_control_socket(mut self, path: impl Into<PathBuf>) -> Self {
        self.control_socket = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_ingest_socket(mut self, path: impl Into<PathBuf>) -> Self {
        self.ingest_socket = Some(path.into());
        self
    }

    pub(crate) fn resolve(self) -> Result<(Option<PathBuf>, PathBuf, PathBuf, PathBuf), SdkError> {
        let state_root = self.resolve_state_root()?;
        let query_socket = match (self.query_socket, &state_root) {
            (Some(path), _) => path,
            (None, Some(root)) => root.join("search-plane").join("query.sock"),
            (None, None) => {
                return Err(SdkError::Usage(
                    "query socket unresolved: set state root or explicit query socket".to_string(),
                ));
            }
        };
        let control_socket = match (self.control_socket, &state_root) {
            (Some(path), _) => path,
            (None, Some(root)) => root.join("search-plane").join("control.sock"),
            (None, None) => {
                return Err(SdkError::Usage(
                    "control socket unresolved: set state root or explicit control socket"
                        .to_string(),
                ));
            }
        };
        let ingest_socket = match (self.ingest_socket, &state_root) {
            (Some(path), _) => path,
            (None, Some(root)) => root.join("search-plane").join("ingest.sock"),
            (None, None) => {
                return Err(SdkError::Usage(
                    "ingest socket unresolved: set state root or explicit ingest socket"
                        .to_string(),
                ));
            }
        };
        Ok((state_root, query_socket, control_socket, ingest_socket))
    }

    fn resolve_state_root(&self) -> Result<Option<PathBuf>, SdkError> {
        if let Some(root) = &self.state_root {
            return Ok(Some(root.clone()));
        }
        if self.query_socket.is_some()
            || self.control_socket.is_some()
            || self.ingest_socket.is_some()
        {
            return Ok(None);
        }
        if let Ok(explicit) = std::env::var("QUANTA_INDEX_STATE_ROOT") {
            return Ok(Some(PathBuf::from(explicit)));
        }
        if let Ok(cache_root) = std::env::var("QUANTA_INDEX_CACHE_ROOT") {
            return Ok(Some(PathBuf::from(cache_root).join("state")));
        }
        let home = std::env::var("HOME").map_err(|_err| {
            SdkError::Usage(
                "cannot resolve default state root: set state root, HOME, or QUANTA_INDEX_*"
                    .to_string(),
            )
        })?;
        #[cfg(target_os = "macos")]
        let root = PathBuf::from(home).join("Library/Caches/quanta-index/state");
        #[cfg(not(target_os = "macos"))]
        let root = PathBuf::from(home).join(".cache/quanta-index/state");
        Ok(Some(root))
    }
}
