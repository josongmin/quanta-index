//! Own the Native benchmark daemon and its private control files together.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use serde_json::Value;
use tempfile::TempDir;

use super::{NativeDaemon, NativeExecutable};

/// A real Native daemon whose private session directory is removed after its child exits.
/// A state root supplied by the caller remains the caller's responsibility.
pub struct NativeSession {
    daemon: Option<NativeDaemon>,
    owned_session_dir: Option<TempDir>,
    state_root: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    config_bytes: Vec<u8>,
}

impl NativeSession {
    /// Start the registered daemon binary in a private, session-owned directory.
    pub fn start_owned(
        binary: &Path,
        make_config: impl FnOnce(&Path, &Path) -> Result<Value>,
    ) -> Result<Self> {
        let executable = NativeExecutable::capture(binary)?;
        Self::start_owned_pinned(&executable, make_config)
    }

    pub(super) fn start_owned_pinned(
        executable: &NativeExecutable,
        make_config: impl FnOnce(&Path, &Path) -> Result<Value>,
    ) -> Result<Self> {
        let session_dir = Self::private_session_dir()?;
        let state_root = session_dir.path().join("state");
        Self::start(executable, state_root, session_dir, make_config)
    }

    /// Reopen a caller-owned state root with fresh private control files.
    pub fn start_in(
        binary: &Path,
        directory: &Path,
        make_config: impl FnOnce(&Path, &Path) -> Result<Value>,
    ) -> Result<Self> {
        let executable = NativeExecutable::capture(binary)?;
        ensure!(directory.is_dir(), "Native session directory is absent");
        let session_dir = Self::private_session_dir()?;
        Self::start(
            &executable,
            directory.join("state"),
            session_dir,
            make_config,
        )
    }

    fn private_session_dir() -> Result<TempDir> {
        // Keep the UDS path independent of the caller's potentially deep state root.
        Ok(tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?)
    }

    fn start(
        executable: &NativeExecutable,
        state_root: PathBuf,
        session_dir: TempDir,
        make_config: impl FnOnce(&Path, &Path) -> Result<Value>,
    ) -> Result<Self> {
        let socket = session_dir.path().join("native.sock");
        let config = session_dir.path().join("native.json");
        let stderr = session_dir.path().join("daemon.stderr");
        let value = make_config(&state_root, &socket)?;
        let state_root_text = state_root
            .to_str()
            .context("Native state root is not UTF-8")?;
        let socket_text = socket.to_str().context("Native socket path is not UTF-8")?;
        ensure!(
            value.get("state_root").and_then(Value::as_str) == Some(state_root_text)
                && value.get("socket_path").and_then(Value::as_str) == Some(socket_text),
            "Native session config must bind the supplied state root and socket"
        );
        let config_bytes = serde_json::to_vec(&value)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config)
            .context("create exclusive Native session config")?;
        file.write_all(&config_bytes)?;
        file.flush()?;
        drop(file);
        let daemon = NativeDaemon::start(executable, &config, &socket, None, 0, &stderr)?;
        Ok(Self {
            daemon: Some(daemon),
            owned_session_dir: Some(session_dir),
            state_root,
            socket,
            config,
            config_bytes,
        })
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// Bytes serialized before daemon startup, for post-drain integrity checks.
    #[must_use]
    pub(super) fn config_bytes(&self) -> &[u8] {
        &self.config_bytes
    }

    pub(super) fn linux_peak_rss_bytes(&self) -> Result<Option<u64>> {
        self.daemon
            .as_ref()
            .context("Native session daemon already stopped")?
            .linux_peak_rss_bytes()
    }

    /// Drain the child, inspect its final files, then close the private session directory.
    pub(super) fn stop_with<T>(
        mut self,
        after_drain: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        let result = (|| {
            self.daemon
                .take()
                .context("Native session daemon already stopped")?
                .stop(&mut None)?;
            after_drain(&self.config)
        })();
        let cleanup = match self.owned_session_dir.take() {
            Some(root) => root
                .close()
                .context("remove private Native session directory"),
            None => Ok(()),
        };
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => Err(error.context(format!(
                "Native session directory cleanup also failed: {cleanup:#}"
            ))),
        }
    }

    /// Drain the child and remove its private files; preserve a borrowed state root.
    pub fn stop(self) -> Result<()> {
        self.stop_with(|_| Ok(()))
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        drop(self.daemon.take());
        drop(self.owned_session_dir.take());
    }
}
