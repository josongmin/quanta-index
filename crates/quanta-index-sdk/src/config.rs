use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::SdkError;
use quanta_index_ipc::ClientIoPolicy;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectOptions {
    state_root: Option<PathBuf>,
    query_socket: Option<PathBuf>,
    control_socket: Option<PathBuf>,
    /// QI-SDK-01: typed ingest socket override. Defaults to
    /// `state_root/search-plane/ingest.sock`.
    ingest_socket: Option<PathBuf>,
    request_io_timeout: Duration,
    request_io_deadline: Option<Instant>,
}

/// Which transports a client configures (S21-07). The query-only
/// profile is least privilege: it requires and constructs only the query
/// transport, and never fabricates dummy control or ingest transports.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClientProfile {
    /// Query, control and ingest transports, all required.
    #[default]
    Full,
    /// Query transport only; control and ingest calls fail with a typed
    /// [`crate::SdkError::PlaneUnavailable`].
    QueryOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedConnectOptions {
    pub(crate) state_root: Option<PathBuf>,
    pub(crate) query_socket: PathBuf,
    pub(crate) control_socket: Option<PathBuf>,
    pub(crate) ingest_socket: Option<PathBuf>,
    pub(crate) io_policy: ClientIoPolicy,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            state_root: None,
            query_socket: None,
            control_socket: None,
            ingest_socket: None,
            request_io_timeout: ClientIoPolicy::default().request_timeout(),
            request_io_deadline: None,
        }
    }
}

impl ConnectOptions {
    #[must_use]
    pub fn from_state_root(path: impl Into<PathBuf>) -> Self {
        Self {
            state_root: Some(path.into()),
            ..Self::default()
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

    #[must_use]
    pub fn with_request_io_timeout(mut self, timeout: Duration) -> Self {
        self.request_io_timeout = timeout;
        self.request_io_deadline = None;
        self
    }

    #[must_use]
    pub fn with_request_io_deadline(mut self, deadline: Instant) -> Self {
        self.request_io_deadline = Some(deadline);
        self
    }

    pub(crate) fn resolve(self) -> Result<ResolvedConnectOptions, SdkError> {
        self.resolve_profile(ClientProfile::Full)
    }

    /// Resolve under an explicit profile (S21-07): `QueryOnly` leaves
    /// the control and ingest sockets unresolved instead of failing or
    /// inventing defaults for transports the profile never constructs.
    pub(crate) fn resolve_profile(
        self,
        profile: ClientProfile,
    ) -> Result<ResolvedConnectOptions, SdkError> {
        let io_policy = match self.request_io_deadline {
            Some(deadline) => ClientIoPolicy::try_with_deadline(deadline),
            None => ClientIoPolicy::try_new(self.request_io_timeout),
        }
        .map_err(|error| SdkError::Usage(error.to_string()))?;
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
        let optional_socket = |explicit: Option<PathBuf>, name: &str| -> Option<PathBuf> {
            match (explicit, &state_root) {
                (Some(path), _) => Some(path),
                (None, Some(root)) => Some(root.join("search-plane").join(name)),
                (None, None) => None,
            }
        };
        let (control_socket, ingest_socket) = match profile {
            ClientProfile::Full => {
                let control_socket = optional_socket(self.control_socket, "control.sock")
                    .ok_or_else(|| {
                        SdkError::Usage(
                            "control socket unresolved: set state root or explicit control socket"
                                .to_string(),
                        )
                    })?;
                let ingest_socket =
                    optional_socket(self.ingest_socket, "ingest.sock").ok_or_else(|| {
                        SdkError::Usage(
                            "ingest socket unresolved: set state root or explicit ingest socket"
                                .to_string(),
                        )
                    })?;
                (Some(control_socket), Some(ingest_socket))
            }
            ClientProfile::QueryOnly => (None, None),
        };
        Ok(ResolvedConnectOptions {
            state_root,
            query_socket,
            control_socket,
            ingest_socket,
            io_policy,
        })
    }

    fn resolve_state_root(&self) -> Result<Option<PathBuf>, SdkError> {
        self.resolve_state_root_with(&SystemEnv)
    }

    /// Deterministic state-root resolution over an explicit environment
    /// view (TOPT-01 / PO-2). Precedence, exactly as before: explicit
    /// option, then no-root when any socket is pinned, then the
    /// state-root variable, the cache-root variable, then the platform
    /// default under `HOME`. A missing or non-Unicode `HOME` is a usage
    /// error; a missing or non-Unicode `QUANTA_INDEX_*` variable falls
    /// through to the next source.
    pub(crate) fn resolve_state_root_with(
        &self,
        env: &dyn EnvLookup,
    ) -> Result<Option<PathBuf>, SdkError> {
        if let Some(root) = &self.state_root {
            return Ok(Some(root.clone()));
        }
        if self.query_socket.is_some()
            || self.control_socket.is_some()
            || self.ingest_socket.is_some()
        {
            return Ok(None);
        }
        if let Ok(explicit) = env.get("QUANTA_INDEX_STATE_ROOT") {
            return Ok(Some(PathBuf::from(explicit)));
        }
        if let Ok(cache_root) = env.get("QUANTA_INDEX_CACHE_ROOT") {
            return Ok(Some(PathBuf::from(cache_root).join("state")));
        }
        let home = env.get("HOME").map_err(|_err| {
            SdkError::Usage(
                "cannot resolve default state root: set state root, HOME, or QUANTA_INDEX_*"
                    .to_string(),
            )
        })?;
        Ok(Some(default_state_root_from_home(&home)))
    }
}

/// The platform default state root under `home`, isolated from
/// precedence so the fallback is testable without the environment.
fn default_state_root_from_home(home: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    let root = PathBuf::from(home).join("Library/Caches/quanta-index/state");
    #[cfg(not(target_os = "macos"))]
    let root = PathBuf::from(home).join(".cache/quanta-index/state");
    root
}

/// Read-only environment view for state-root resolution.
pub(crate) trait EnvLookup {
    fn get(&self, name: &str) -> Result<String, EnvLookupError>;
}

/// Why an environment lookup failed: absent, or present but not Unicode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvLookupError {
    Missing,
    NotUnicode,
}

/// The production environment view: the real process environment at the
/// composition edge.
pub(crate) struct SystemEnv;

impl EnvLookup for SystemEnv {
    fn get(&self, name: &str) -> Result<String, EnvLookupError> {
        match std::env::var(name) {
            Ok(value) => Ok(value),
            Err(std::env::VarError::NotPresent) => Err(EnvLookupError::Missing),
            Err(std::env::VarError::NotUnicode(_)) => Err(EnvLookupError::NotUnicode),
        }
    }
}

#[cfg(all(test, not(unix)))]
mod non_unix_tests {
    use super::{ClientProfile, ConnectOptions};
    use crate::SdkError;

    #[test]
    fn all_profiles_refuse_unavailable_native_transport() {
        for profile in [ClientProfile::Full, ClientProfile::QueryOnly] {
            let result = ConnectOptions::from_state_root("state")
                .with_query_socket("query.sock")
                .resolve_profile(profile);
            assert!(
                matches!(result, Err(SdkError::Usage(message)) if message.contains("native IPC transport is unavailable")),
                "profile {profile:?} must refuse Unix socket fallback"
            );
        }
    }
}
