//! Stable semantic linked-inode census under generation lifecycle custody.

use crate::SemanticAdapter;
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::domains::generation::TrackPhysicalByteSession;
use quanta_index_core::{CoreError, DiagnosticContextV1, DiagnosticStageV1, RequestBudgetV1};
use std::sync::{RwLockWriteGuard, TryLockError};

pub(crate) struct SemanticPhysicalSession<'a> {
    adapter: &'a SemanticAdapter,
    _lifecycle: RwLockWriteGuard<'a, ()>,
}

impl<'a> SemanticPhysicalSession<'a> {
    pub(crate) fn begin(adapter: &'a SemanticAdapter) -> Result<Self, CoreError> {
        let lifecycle = adapter.directory_lifecycle.try_write().map_err(|error| match error {
            TryLockError::WouldBlock => CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                message: "semantic physical snapshot: generation lifecycle is busy; retry measurement".into(),
            },
            TryLockError::Poisoned(error) => CoreError::Storage(format!(
                "semantic physical snapshot lifecycle poisoned: {error}"
            )),
        })?;
        Ok(Self {
            adapter,
            _lifecycle: lifecycle,
        })
    }
}

impl TrackPhysicalByteSession for SemanticPhysicalSession<'_> {
    fn measure(
        &self,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<u64, CoreError> {
        crate::observe_result(diagnostic, DiagnosticStageV1::PhysicalInodeCensus, |_| {
            budget.checkpoint("semantic-physical-snapshot:entry")?;
            match std::fs::symlink_metadata(&self.adapter.state_root) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                Err(error) => {
                    return Err(CoreError::Storage(format!(
                        "semantic physical snapshot inspect track {}: {error}",
                        self.adapter.state_root.display()
                    )));
                }
                Ok(_) => {}
            }
            quanta_index_scratch_sqlite::physical::measure_physical_inode_tree_bytes(
                &self.adapter.state_root,
                &|_name| false,
                self.adapter
                    .retention_measurement_policy
                    .max_main_file_bytes(),
                budget,
                diagnostic,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("fixture assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if left != right => {
                    return Err(format!(
                        "fixture equality failed: {} = {left:?}, {} = {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )
                    .into());
                }
                _ => {}
            }
        }};
    }

    #[test]
    fn snapshot_refuses_active_writes_and_counts_fixed_hardlinked_fixture()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let adapter = SemanticAdapter::with_state_root(root.path().to_path_buf())?;
        std::fs::write(root.path().join("one"), b"12345")?;
        std::fs::hard_link(root.path().join("one"), root.path().join("shared"))?;
        std::fs::write(root.path().join("two"), b"1234567")?;
        let writer = adapter
            .directory_lifecycle
            .read()
            .map_err(|error| format!("fixture lifecycle read: {error}"))?;
        check!(matches!(
            SemanticPhysicalSession::begin(&adapter),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        // Live diagnostics must not acquire the writer's lifecycle boundary.
        check_eq!(
            quanta_index_core::TrackDiskUsagePort::track_disk_bytes(
                &adapter,
                &RequestBudgetV1::unbounded()
            )?,
            12
        );
        drop(writer);
        let snapshot = SemanticPhysicalSession::begin(&adapter)?;
        check!(matches!(
            adapter.directory_lifecycle.try_read(),
            Err(TryLockError::WouldBlock)
        ));
        check_eq!(
            snapshot.measure(
                &adapter
                    .retention_measurement_policy
                    .physical_scan_budget()?,
                None
            )?,
            12
        );
        drop(snapshot);
        drop(
            adapter
                .directory_lifecycle
                .try_read()
                .map_err(|error| format!("fixture lifecycle retry: {error}"))?,
        );
        Ok(())
    }
}
