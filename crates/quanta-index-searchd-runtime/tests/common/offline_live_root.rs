//! Disposable current-format root with catalog rows and no root manifest.

use std::fs;
use std::path::Path;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;

pub(crate) const BUSY: Duration = Duration::from_secs(2);

pub(crate) fn build_live_root(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for directory in [
        "indexes/lexical/generation-v1-aa/g1",
        "indexes/semantic",
        "authorities",
        "activations",
        // A real seal leaves its object directory behind for the store to sync.
        "repo-map/objects/sha256/08/08",
        "repo-map/objects/sha256/12/12",
        "repo-map/objects/sha256/1c/1c",
    ] {
        fs::create_dir_all(root.join(directory))?;
    }
    fs::write(
        root.join("indexes/lexical/generation-v1-aa/g1/manifest.cbor"),
        b"lexical-generation-1",
    )?;
    fs::write(root.join("authorities/history.cbor"), b"history-authority")?;
    let catalog = SqliteCatalog::open(root, BUSY)?;
    let _sealed = catalog.seal_repomap_candidate(
        "repo-a",
        "rev-a",
        1,
        &[7_u8; 32],
        &[8_u8; 32],
        &[9_u8; 32],
        128,
        "projection-a",
    )?;
    let _sealed = catalog.seal_repomap_candidate(
        "repo-a",
        "rev-a",
        2,
        &[17_u8; 32],
        &[18_u8; 32],
        &[19_u8; 32],
        256,
        "projection-b",
    )?;
    drop(catalog);
    Ok(())
}
