//! Shared legacy RepoMap fixture for offline and pinned-daemon refusals.

use std::fs;
use std::path::Path;

pub(crate) fn build_legacy_root(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("repo-map/activations"))?;
    fs::create_dir_all(root.join("repo-map/snapshots"))?;
    fs::write(
        root.join("repo-map/activations/repo-a--rev-a.json"),
        b"{\"legacy\":\"activation\"}",
    )?;
    fs::write(
        root.join("repo-map/snapshots/repo-a--rev-a.json"),
        b"{\"legacy\":\"snapshot\"}",
    )?;
    Ok(())
}
