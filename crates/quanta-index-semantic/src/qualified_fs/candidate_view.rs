//! Generation-level reader custody for the persistent memory URI candidate.
//!
//! Lance's cloud reader has no per-reader file descriptor lease. The adapter
//! therefore refuses physical reclamation while one of its searchers owns a
//! generation. This changes direct reclaim into a retryable operation; it does
//! not reproduce the local reader's read-after-unlink contract.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use quanta_index_core::CoreError;

// Direct adapter users bypass Search Plane's bounded snapshot registry.
// Bound both cardinality and key width for this process-global registry.
const MAX_LIVE_GENERATIONS: usize = 1024;
const MAX_KEY_BYTES: usize = 4096;

#[derive(Debug, Default)]
pub(crate) struct CandidateViews {
    live: Mutex<BTreeMap<PathBuf, usize>>,
    lifecycle: RwLock<()>,
}

impl CandidateViews {
    pub(crate) fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<CandidateViews>> = OnceLock::new();
        Arc::clone(SHARED.get_or_init(|| Arc::new(Self::default())))
    }

    pub(crate) fn read_lifecycle(&self) -> Result<RwLockReadGuard<'_, ()>, CoreError> {
        self.lifecycle.read().map_err(|error| {
            CoreError::Storage(format!("semantic candidate lifecycle poisoned: {error}"))
        })
    }

    pub(crate) fn write_lifecycle(&self) -> Result<RwLockWriteGuard<'_, ()>, CoreError> {
        self.lifecycle.write().map_err(|error| {
            CoreError::Storage(format!("semantic candidate lifecycle poisoned: {error}"))
        })
    }

    pub(crate) fn acquire(
        self: &Arc<Self>,
        generation_dir: PathBuf,
    ) -> Result<CandidateView, CoreError> {
        let generation_dir = std::fs::canonicalize(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "semantic candidate view cannot identify {}: {error}",
                generation_dir.display()
            ))
        })?;
        if generation_dir.as_os_str().as_bytes().len() > MAX_KEY_BYTES {
            return Err(CoreError::Storage(format!(
                "semantic candidate view key exceeds {MAX_KEY_BYTES} bytes"
            )));
        }
        let mut live = self.live.lock().map_err(|error| {
            CoreError::Storage(format!(
                "semantic candidate view registry poisoned: {error}"
            ))
        })?;
        if !live.contains_key(&generation_dir) && live.len() >= MAX_LIVE_GENERATIONS {
            return Err(CoreError::Storage(format!(
                "semantic candidate view registry reached {MAX_LIVE_GENERATIONS} live generations; retry after readers close"
            )));
        }
        let count = live.entry(generation_dir.clone()).or_default();
        *count = count
            .checked_add(1)
            .ok_or_else(|| CoreError::Storage("semantic candidate view count overflow".into()))?;
        Ok(CandidateView {
            owner: Arc::clone(self),
            generation_dir,
        })
    }

    pub(crate) fn refuse_reclaim(&self, directory: &Path) -> Result<(), CoreError> {
        let directory = match std::fs::canonicalize(directory) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "semantic candidate reclaim cannot identify {}: {error}",
                    directory.display()
                )));
            }
        };
        let live = self.live.lock().map_err(|error| {
            CoreError::Storage(format!(
                "semantic candidate view registry poisoned: {error}"
            ))
        })?;
        let holders: usize = live
            .iter()
            .filter(|(generation_dir, _)| generation_dir.starts_with(&directory))
            .map(|(_, holders)| *holders)
            .fold(0_usize, usize::saturating_add);
        if holders > 0 {
            return Err(CoreError::Storage(format!(
                "semantic candidate generation {} has {holders} live reader view(s); retry reclaim after they close",
                directory.display()
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct CandidateView {
    owner: Arc<CandidateViews>,
    generation_dir: PathBuf,
}

impl Drop for CandidateView {
    fn drop(&mut self) {
        // Drop must release the view even after an unrelated panic poisoned
        // the mutex. No filesystem I/O occurs in this destructor.
        let mut live = match self.owner.live.lock() {
            Ok(live) => live,
            Err(error) => error.into_inner(),
        };
        if let Some(count) = live.get_mut(&self.generation_dir) {
            if *count == 1 {
                let _removed = live.remove(&self.generation_dir);
            } else {
                *count -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::CandidateViews;

    #[test]
    fn shared_views_refuse_reclaim_through_directory_alias_until_last_drop()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let generation = root.path().join("generation");
        fs::create_dir(&generation)?;
        let alias = root.path().join("alias");
        symlink(root.path(), &alias)?;

        let opener = CandidateViews::shared();
        let reclaimer = CandidateViews::shared();
        let view = opener.acquire(generation.clone())?;
        assert!(reclaimer.refuse_reclaim(&alias.join("generation")).is_err());
        drop(view);
        reclaimer.refuse_reclaim(&alias.join("generation"))?;
        Ok(())
    }
}
