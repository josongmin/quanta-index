//! Stable linked-inode census under generation, writer, and pool custody.

use std::sync::{RwLockWriteGuard, TryLockError};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::domains::generation::TrackPhysicalByteSession;
use quanta_index_core::{CoreError, DiagnosticContextV1, DiagnosticStageV1, RequestBudgetV1};

use crate::LexicalAdapter;
use crate::generation_dir::is_writer_lock_entry;
use crate::lexical_store::Gate;

pub(crate) struct LexicalPhysicalSession<'a> {
    adapter: &'a LexicalAdapter,
    _pool: Option<Gate>,
    _lifecycle: RwLockWriteGuard<'a, ()>,
}

fn busy(boundary: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
        message: format!("lexical physical snapshot: {boundary} is busy; retry measurement"),
    }
}

impl<'a> LexicalPhysicalSession<'a> {
    pub(crate) fn begin(adapter: &'a LexicalAdapter) -> Result<Self, CoreError> {
        let lifecycle = adapter
            .directory_lifecycle
            .try_write()
            .map_err(|error| match error {
                TryLockError::WouldBlock => busy("generation lifecycle"),
                TryLockError::Poisoned(error) => CoreError::Storage(format!(
                    "lexical physical snapshot lifecycle poisoned: {error}"
                )),
            })?;
        adapter
            .writers
            .try_lock()
            .map_err(|error| match error {
                TryLockError::WouldBlock => busy("writer cache"),
                TryLockError::Poisoned(error) => CoreError::Storage(format!(
                    "lexical physical snapshot writer cache poisoned: {error}"
                )),
            })?
            .assert_physical_quiescent()?;
        let pool_path = adapter.state_root.join("lexical-object-pool");
        let pool = match std::fs::symlink_metadata(&pool_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "lexical physical snapshot inspect pool {}: {error}",
                    pool_path.display()
                )));
            }
            Ok(metadata) if metadata.is_dir() => Some(
                adapter
                    .open_existing_store()?
                    .try_physical_snapshot_gate()?,
            ),
            Ok(_) => {
                return Err(CoreError::Storage(format!(
                    "lexical physical snapshot pool {} is not a directory",
                    pool_path.display()
                )));
            }
        };
        Ok(Self {
            adapter,
            _pool: pool,
            _lifecycle: lifecycle,
        })
    }
}

impl TrackPhysicalByteSession for LexicalPhysicalSession<'_> {
    fn measure(
        &self,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<u64, CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::PhysicalInodeCensus,
            |diagnostic| {
                budget.checkpoint("lexical-physical-snapshot:entry")?;
                match std::fs::symlink_metadata(&self.adapter.state_root) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                    Err(error) => {
                        return Err(CoreError::Storage(format!(
                            "lexical physical snapshot inspect track {}: {error}",
                            self.adapter.state_root.display()
                        )));
                    }
                    Ok(_) => {}
                }
                // This walker performs no inventory cleanup or nested pool lock.
                quanta_index_scratch_sqlite::physical::measure_physical_inode_tree_bytes(
                    &self.adapter.state_root,
                    &is_writer_lock_entry,
                    self.adapter
                        .retention_measurement_policy
                        .max_main_file_bytes(),
                    budget,
                    diagnostic,
                )
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::os::unix::fs::MetadataExt as _;

    #[test]
    fn snapshot_fences_generation_and_pool_mutations_and_retries()
    -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let root = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let store = crate::lexical_store::ObjectStore::open(root.path())?;
        let active_writer = adapter
            .directory_lifecycle
            .read()
            .map_err(|error| format!("fixture lifecycle read: {error}"))?;
        check!(matches!(
            LexicalPhysicalSession::begin(&adapter),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        drop(active_writer);
        let active_pool_writer = store.try_physical_snapshot_gate()?;
        check!(matches!(
            LexicalPhysicalSession::begin(&adapter),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        drop(active_pool_writer);
        let snapshot = LexicalPhysicalSession::begin(&adapter)?;
        check!(matches!(
            adapter.directory_lifecycle.try_read(),
            Err(TryLockError::WouldBlock)
        ));
        check!(matches!(
            store.try_physical_snapshot_gate(),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        let mut pending = vec![root.path().to_path_buf()];
        let mut seen = BTreeSet::new();
        let mut golden = 0_u64;
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path)? {
                    pending.push(entry?.path());
                }
            } else if metadata.is_file()
                && !is_writer_lock_entry(
                    path.file_name()
                        .ok_or("fixture file name")?
                        .to_str()
                        .ok_or("fixture UTF-8 name")?,
                )
                && seen.insert((metadata.dev(), metadata.ino()))
            {
                golden = golden
                    .checked_add(metadata.len())
                    .ok_or("fixture byte overflow")?;
            }
        }
        check_eq!(
            snapshot.measure(
                &adapter
                    .retention_measurement_policy
                    .physical_scan_budget()?,
                None
            )?,
            golden
        );
        // The diagnostic meter must not wait on the snapshot's lifecycle or
        // pool gate; the timer's sweep needs those same boundaries.
        check_eq!(
            quanta_index_core::TrackDiskUsagePort::track_disk_bytes(
                &adapter,
                &RequestBudgetV1::unbounded()
            )?,
            golden
        );
        drop(snapshot);
        drop(
            adapter
                .directory_lifecycle
                .try_read()
                .map_err(|error| format!("fixture lifecycle retry: {error}"))?,
        );
        drop(store.try_physical_snapshot_gate()?);
        Ok(())
    }
}
