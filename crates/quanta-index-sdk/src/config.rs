use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::SdkError;
use quanta_index_ipc::ClientIoPolicy;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectOptions<P = PathBuf> {
    pub(crate) state_root: Option<P>,
    pub(crate) query_socket: Option<P>,
    pub(crate) control_socket: Option<P>,
    /// QI-SDK-01: typed ingest socket override. Defaults to
    /// `state_root/search-plane/ingest.sock`.
    pub(crate) ingest_socket: Option<P>,
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

    pub(crate) fn resolve(self) -> Result<ResolvedConnectOptions, SdkError> {
        self.resolve_profile(ClientProfile::Full)
    }

    /// Resolve under an explicit profile (S21-07): `QueryOnly` leaves
    /// the control and ingest sockets unresolved instead of failing or
    /// inventing defaults for transports the profile never constructs.
    pub(crate) fn resolve_profile(
        mut self,
        profile: ClientProfile,
    ) -> Result<ResolvedConnectOptions, SdkError> {
        let io_policy = self
            .io_policy_v1()
            .map_err(|error| SdkError::Usage(error.to_string()))?;
        let state_root = self.resolve_state_root()?;
        let query_socket = resolve_owned_socket_v1(
            state_root.as_deref(),
            self.query_socket,
            SocketKindV1::Query,
        )?;
        let (control_socket, ingest_socket) = match profile {
            ClientProfile::Full => {
                let control_socket = resolve_owned_socket_v1(
                    state_root.as_deref(),
                    self.control_socket,
                    SocketKindV1::Control,
                )?;
                let ingest_socket = resolve_owned_socket_v1(
                    state_root.as_deref(),
                    self.ingest_socket,
                    SocketKindV1::Ingest,
                )?;
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

    fn resolve_state_root(&mut self) -> Result<Option<PathBuf>, SdkError> {
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
        &mut self,
        env: &dyn EnvLookup,
    ) -> Result<Option<PathBuf>, SdkError> {
        match self.root_source_v1() {
            RootSourceV1::Explicit => return Ok(self.state_root.take()),
            RootSourceV1::PinnedSockets => return Ok(None),
            RootSourceV1::Environment => {}
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

impl<P> ConnectOptions<P> {
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

    pub(crate) fn io_policy_v1(&self) -> Result<ClientIoPolicy, quanta_index_ipc::IpcError> {
        self.request_io_deadline.map_or_else(
            || ClientIoPolicy::try_new(self.request_io_timeout),
            ClientIoPolicy::try_with_deadline,
        )
    }

    pub(crate) fn root_source_v1(&self) -> RootSourceV1 {
        if self.state_root.is_some() {
            RootSourceV1::Explicit
        } else if self.query_socket.is_some()
            || self.control_socket.is_some()
            || self.ingest_socket.is_some()
        {
            RootSourceV1::PinnedSockets
        } else {
            RootSourceV1::Environment
        }
    }
}

impl<'input> ConnectOptions<&'input Path> {
    /// Borrow the same options without constructing a `PathBuf` before admission.
    #[must_use]
    pub fn borrow_state_root_v1(path: &'input Path) -> Self {
        Self {
            state_root: Some(path),
            query_socket: None,
            control_socket: None,
            ingest_socket: None,
            request_io_timeout: ClientIoPolicy::default().request_timeout(),
            request_io_deadline: None,
        }
    }

    /// Configure an explicitly pinned query socket without environment lookup.
    #[must_use]
    pub fn borrow_query_socket_v1(path: &'input Path) -> Self {
        Self {
            state_root: None,
            query_socket: Some(path),
            control_socket: None,
            ingest_socket: None,
            request_io_timeout: ClientIoPolicy::default().request_timeout(),
            request_io_deadline: None,
        }
    }

    #[must_use]
    pub fn with_borrowed_query_socket_v1(mut self, path: &'input Path) -> Self {
        self.query_socket = Some(path);
        self
    }
    #[must_use]
    pub fn with_borrowed_control_socket_v1(mut self, path: &'input Path) -> Self {
        self.control_socket = Some(path);
        self
    }
    #[must_use]
    pub fn with_borrowed_ingest_socket_v1(mut self, path: &'input Path) -> Self {
        self.ingest_socket = Some(path);
        self
    }
}

pub(crate) enum RootSourceV1 {
    Explicit,
    PinnedSockets,
    Environment,
}

#[derive(Clone, Copy)]
pub(crate) enum SocketKindV1 {
    Query,
    Control,
    Ingest,
}

impl SocketKindV1 {
    pub(crate) const fn suffix_v1(self) -> &'static str {
        match self {
            Self::Query => "search-plane/query.sock",
            Self::Control => "search-plane/control.sock",
            Self::Ingest => "search-plane/ingest.sock",
        }
    }
    pub(crate) const fn unresolved_v1(self) -> &'static str {
        match self {
            Self::Query => "query socket unresolved: set state root or explicit query socket",
            Self::Control => "control socket unresolved: set state root or explicit control socket",
            Self::Ingest => "ingest socket unresolved: set state root or explicit ingest socket",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SocketSourceV1 {
    Explicit,
    StateRoot,
}

pub(crate) fn socket_source_v1(has_root: bool, has_explicit: bool) -> Option<SocketSourceV1> {
    if has_explicit {
        Some(SocketSourceV1::Explicit)
    } else if has_root {
        Some(SocketSourceV1::StateRoot)
    } else {
        None
    }
}

/// One physical `PathBuf` producer for ordinary and admitted construction.
/// Native callers reserve the complete capacity and prepay copying before entry.
pub(crate) fn fill_socket_path_v1(
    input: &Path,
    source: SocketSourceV1,
    kind: SocketKindV1,
    output: &mut PathBuf,
) {
    output.push(input);
    if matches!(source, SocketSourceV1::StateRoot) {
        output.push(kind.suffix_v1());
    }
}

pub(crate) fn socket_path_capacity_v1(
    input: &Path,
    source: SocketSourceV1,
    kind: SocketKindV1,
) -> Option<usize> {
    let suffix = match source {
        SocketSourceV1::Explicit => 0,
        SocketSourceV1::StateRoot => kind.suffix_v1().len().checked_add(1)?,
    };
    input
        .as_os_str()
        .as_encoded_bytes()
        .len()
        .checked_add(suffix)
}

fn resolve_owned_socket_v1(
    root: Option<&Path>,
    explicit: Option<PathBuf>,
    kind: SocketKindV1,
) -> Result<PathBuf, SdkError> {
    match socket_source_v1(root.is_some(), explicit.is_some()) {
        Some(SocketSourceV1::Explicit) => {
            explicit.ok_or_else(|| SdkError::Usage(kind.unresolved_v1().to_string()))
        }
        Some(source @ SocketSourceV1::StateRoot) => {
            let input = root.ok_or_else(|| SdkError::Usage(kind.unresolved_v1().to_string()))?;
            let bytes = socket_path_capacity_v1(input, source, kind)
                .ok_or_else(|| SdkError::Usage("socket path capacity overflow".to_string()))?;
            let mut output = PathBuf::with_capacity(bytes);
            fill_socket_path_v1(input, source, kind, &mut output);
            Ok(output)
        }
        None => Err(SdkError::Usage(kind.unresolved_v1().to_string())),
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
