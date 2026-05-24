use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub struct SearchdTestPaths {
    _temp: TempDir,
    state_root: PathBuf,
    control_plane_path: PathBuf,
    socket_path: PathBuf,
}

impl SearchdTestPaths {
    pub fn new() -> std::io::Result<Self> {
        let temp = tempfile::tempdir()?;
        let state_root = temp.path().join("state");
        let control_plane_path = state_root.join("control-plane.sqlite3");
        let socket_path = state_root.join("search-plane").join("searchd.sock");

        Ok(Self {
            _temp: temp,
            state_root,
            control_plane_path,
            socket_path,
        })
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[must_use]
    pub fn control_plane_path(&self) -> &Path {
        &self.control_plane_path
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}
