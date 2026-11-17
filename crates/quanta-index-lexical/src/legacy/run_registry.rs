//! Canonical persisted run-registry DATA and decoder; no ObjectStore is opened.
use super::object_codec::{MapRoot, ObjectRef};
use quanta_index_core::{CoreError, lexical_maintenance::TargetCounters};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
const REGISTRY_WIRE_VERSION: u8 = 2;
pub(crate) fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical run registry: {message}"))
}
#[cfg(feature = "legacy-tantivy")]
pub(crate) fn corrupt(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("lexical run registry corrupt: {message}"),
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunRegistry {
    pub(crate) runs: MapRoot,
    pub(crate) segment_owners: MapRoot,
    pub(crate) next_run_id: u64,
    pub(crate) active_index_encoded_bytes: u64,
    pub(crate) selection_progress: RunSelectionProgress,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunSelectionProgress {
    pub(crate) next_scan_run_id: u64,
    pub(crate) scan_high_water_run_id: u64,
    pub(crate) pending_run: Option<u64>,
}
impl Default for RunSelectionProgress {
    fn default() -> Self {
        Self {
            next_scan_run_id: 1,
            scan_high_water_run_id: 0,
            pending_run: None,
        }
    }
}
impl Default for RunRegistry {
    fn default() -> Self {
        Self {
            runs: MapRoot::default(),
            segment_owners: MapRoot::default(),
            next_run_id: 1,
            active_index_encoded_bytes: 0,
            selection_progress: RunSelectionProgress::default(),
        }
    }
}
impl Serialize for RunRegistry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            REGISTRY_WIRE_VERSION,
            &self.runs,
            &self.segment_owners,
            self.next_run_id,
            self.active_index_encoded_bytes,
            self.selection_progress.next_scan_run_id,
            self.selection_progress.scan_high_water_run_id,
            self.selection_progress.pending_run,
        )
            .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for RunRegistry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            version,
            runs,
            segment_owners,
            next_run_id,
            active_index_encoded_bytes,
            next_scan_run_id,
            scan_high_water_run_id,
            pending_run,
        ) = <(u8, MapRoot, MapRoot, u64, u64, u64, u64, Option<u64>)>::deserialize(deserializer)?;
        if version != REGISTRY_WIRE_VERSION {
            return Err(serde::de::Error::custom(
                "unknown lexical run registry version",
            ));
        }
        let registry = Self {
            runs,
            segment_owners,
            next_run_id,
            active_index_encoded_bytes,
            selection_progress: RunSelectionProgress {
                next_scan_run_id,
                scan_high_water_run_id,
                pending_run,
            },
        };
        registry.validate().map_err(serde::de::Error::custom)?;
        Ok(registry)
    }
}
impl RunRegistry {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.next_run_id == 0 || self.runs.entries >= self.next_run_id {
            return Err(invalid("next run ID or run count differs"));
        }
        let progress = self.selection_progress;
        if progress.next_scan_run_id == 0
            || progress.next_scan_run_id > self.next_run_id
            || progress.scan_high_water_run_id >= self.next_run_id
            || progress
                .pending_run
                .is_some_and(|id| id == 0 || id > progress.scan_high_water_run_id)
        {
            return Err(invalid("selection progress is outside issued run IDs"));
        }
        let _counters = self.snapshot()?;
        Ok(())
    }
    pub(crate) fn snapshot(&self) -> Result<TargetCounters, CoreError> {
        TargetCounters::new(
            self.runs.entries,
            self.segment_owners.entries,
            self.active_index_encoded_bytes,
        )
    }
    pub(crate) fn object_roots(&self) -> Vec<ObjectRef> {
        self.runs
            .page
            .iter()
            .chain(self.segment_owners.page.iter())
            .map(|page| page.object.clone())
            .collect()
    }
}
