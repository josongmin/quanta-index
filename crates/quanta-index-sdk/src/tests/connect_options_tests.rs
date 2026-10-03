use super::*;

#[test]
fn connect_options_from_state_root_resolve_default_sockets() {
    let resolved = ok_or_fail!(ConnectOptions::from_state_root("/tmp/qi-state").resolve());
    assert_eq!(resolved.state_root, Some(PathBuf::from("/tmp/qi-state")));
    assert_eq!(
        resolved.query_socket,
        PathBuf::from("/tmp/qi-state/search-plane/query.sock")
    );
    assert_eq!(
        resolved.control_socket,
        Some(PathBuf::from("/tmp/qi-state/search-plane/control.sock"))
    );
    assert_eq!(
        resolved.ingest_socket,
        Some(PathBuf::from("/tmp/qi-state/search-plane/ingest.sock")),
        "QI-SDK-01: ingest socket resolves to state_root/search-plane/ingest.sock"
    );
    assert_eq!(
        resolved.io_policy,
        quanta_index_ipc::ClientIoPolicy::default()
    );
}

#[test]
fn connect_options_preserve_explicit_request_io_timeout() {
    let timeout = std::time::Duration::from_millis(125);
    let resolved = ok_or_fail!(
        ConnectOptions::from_state_root("/tmp/qi-state")
            .with_request_io_timeout(timeout)
            .resolve()
    );
    assert_eq!(resolved.io_policy.request_timeout(), timeout);
}

#[test]
fn connect_options_preserve_absolute_request_io_deadline() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let resolved = ok_or_fail!(
        ConnectOptions::from_state_root("/tmp/qi-state")
            .with_request_io_deadline(deadline)
            .resolve()
    );
    assert_eq!(resolved.io_policy.absolute_deadline(), Some(deadline));
}

#[test]
fn connect_options_reject_elapsed_request_io_deadline() {
    let deadline = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(1))
        .expect("monotonic clock must represent an instant 1ms in the past");
    let result = ConnectOptions::from_state_root("/tmp/qi-state")
        .with_request_io_deadline(deadline)
        .resolve();
    assert!(
        matches!(result, Err(crate::SdkError::Usage(message)) if message.contains("deadline elapsed"))
    );
}

#[test]
fn connect_options_reject_zero_request_io_timeout() {
    let result = ConnectOptions::from_state_root("/tmp/qi-state")
        .with_request_io_timeout(std::time::Duration::ZERO)
        .resolve();
    assert!(
        matches!(result, Err(crate::SdkError::Usage(message)) if message.contains("greater than zero"))
    );
}

/// Scripted environment for the state-root precedence matrix (TOPT-01 /
/// PO-2): no process environment is read or mutated, so these tests run
/// safely in parallel with no global env mutex.
struct MapEnv {
    vars: BTreeMap<&'static str, Result<String, EnvLookupError>>,
}

impl MapEnv {
    fn new(vars: &[(&'static str, Result<String, EnvLookupError>)]) -> Self {
        Self {
            vars: vars.iter().cloned().collect(),
        }
    }

    fn missing() -> Self {
        Self {
            vars: BTreeMap::new(),
        }
    }
}

impl EnvLookup for MapEnv {
    fn get(&self, name: &str) -> Result<String, EnvLookupError> {
        self.vars
            .get(name)
            .cloned()
            .unwrap_or(Err(EnvLookupError::Missing))
    }
}

/// An environment that must never be consulted: any lookup is a test
/// failure, proving the short-circuit paths skip the environment.
struct ForbiddenEnv;

impl EnvLookup for ForbiddenEnv {
    fn get(&self, name: &str) -> Result<String, EnvLookupError> {
        forbidden_env_lookup(name)
    }
}

/// Any lookup is itself the test failure: returning `Err` would let
/// tolerant paths pass silently, so this diverges instead of answering.
fn forbidden_env_lookup(name: &str) -> ! {
    panic!("environment must not be consulted, got lookup for {name}");
}

#[test]
fn state_root_explicit_option_beats_every_environment_source() {
    let env = MapEnv::new(&[
        ("QUANTA_INDEX_STATE_ROOT", Ok("/env/state".to_string())),
        ("QUANTA_INDEX_CACHE_ROOT", Ok("/env/cache".to_string())),
        ("HOME", Ok("/env/home".to_string())),
    ]);
    let resolved = ok_or_fail!(
        ConnectOptions::from_state_root("/explicit/state").resolve_state_root_with(&env)
    );
    assert_eq!(resolved, Some(PathBuf::from("/explicit/state")));
}

#[test]
fn state_root_short_circuits_skip_the_environment_entirely() {
    let explicit = ok_or_fail!(
        ConnectOptions::from_state_root("/explicit/state").resolve_state_root_with(&ForbiddenEnv)
    );
    assert_eq!(explicit, Some(PathBuf::from("/explicit/state")));
    let pinned = ok_or_fail!(
        ConnectOptions::default()
            .with_query_socket("/pinned/query.sock")
            .resolve_state_root_with(&ForbiddenEnv)
    );
    assert_eq!(pinned, None);
}

#[test]
fn state_root_environment_precedence_matrix() {
    // State-root variable wins over everything below it.
    let env = MapEnv::new(&[
        ("QUANTA_INDEX_STATE_ROOT", Ok("/env/state".to_string())),
        ("QUANTA_INDEX_CACHE_ROOT", Ok("/env/cache".to_string())),
        ("HOME", Ok("/env/home".to_string())),
    ]);
    let resolved = ok_or_fail!(ConnectOptions::default().resolve_state_root_with(&env));
    assert_eq!(resolved, Some(PathBuf::from("/env/state")));

    // Cache-root variable appends `state` when the state-root variable
    // is missing.
    let env = MapEnv::new(&[
        ("QUANTA_INDEX_CACHE_ROOT", Ok("/env/cache".to_string())),
        ("HOME", Ok("/env/home".to_string())),
    ]);
    let resolved = ok_or_fail!(ConnectOptions::default().resolve_state_root_with(&env));
    assert_eq!(resolved, Some(PathBuf::from("/env/cache/state")));

    // HOME fallback when both QUANTA_INDEX_* variables are missing.
    let env = MapEnv::new(&[("HOME", Ok("/env/home".to_string()))]);
    let resolved = ok_or_fail!(ConnectOptions::default().resolve_state_root_with(&env));
    #[cfg(target_os = "macos")]
    let expected = PathBuf::from("/env/home/Library/Caches/quanta-index/state");
    #[cfg(not(target_os = "macos"))]
    let expected = PathBuf::from("/env/home/.cache/quanta-index/state");
    assert_eq!(resolved, Some(expected));
}

#[test]
fn state_root_missing_and_non_unicode_inputs() {
    // A missing HOME is a usage error naming the remedy.
    let missing_home = ConnectOptions::default().resolve_state_root_with(&MapEnv::missing());
    assert!(
        matches!(missing_home, Err(crate::SdkError::Usage(message)) if message.contains("HOME"))
    );

    // A non-Unicode HOME is the same usage error, never a guess.
    let env = MapEnv::new(&[("HOME", Err(EnvLookupError::NotUnicode))]);
    let bad_home = ConnectOptions::default().resolve_state_root_with(&env);
    assert!(matches!(bad_home, Err(crate::SdkError::Usage(message)) if message.contains("HOME")));

    // A non-Unicode state-root variable falls through to the cache root.
    let env = MapEnv::new(&[
        ("QUANTA_INDEX_STATE_ROOT", Err(EnvLookupError::NotUnicode)),
        ("QUANTA_INDEX_CACHE_ROOT", Ok("/env/cache".to_string())),
        ("HOME", Ok("/env/home".to_string())),
    ]);
    let resolved = ok_or_fail!(ConnectOptions::default().resolve_state_root_with(&env));
    assert_eq!(resolved, Some(PathBuf::from("/env/cache/state")));

    // A non-Unicode cache-root variable falls through to HOME.
    let env = MapEnv::new(&[
        ("QUANTA_INDEX_CACHE_ROOT", Err(EnvLookupError::NotUnicode)),
        ("HOME", Ok("/env/home".to_string())),
    ]);
    let resolved = ok_or_fail!(ConnectOptions::default().resolve_state_root_with(&env));
    assert!(
        resolved
            .as_ref()
            .is_some_and(|root| root.ends_with("quanta-index/state")),
        "falls through to the HOME default, got {resolved:?}"
    );
}
