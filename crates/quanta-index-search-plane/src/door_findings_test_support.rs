//! A test double for the port activation and rollback hand a door's
//! content defect to (QI-BB-026).
//!
//! It records every generation it is asked about and answers with the
//! outcome the test scripted, so a test can assert which track's adapter
//! was asked — and that no other was.

use std::path::PathBuf;
use std::sync::Mutex;

use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::{
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, GenerationQuarantineReasonV1,
    QuarantinedGenerationV1,
};

/// What the double answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScriptedFinding {
    /// The adapter re-proved the defect and wrote the receipt.
    Quarantines,
    /// The adapter's re-proof admitted the generation.
    DoesNotReproduce,
    /// The adapter could not re-prove (the directory is gone, say).
    Fails,
}

#[derive(Debug)]
pub(crate) struct RecordingDoorFindings {
    script: ScriptedFinding,
    asked: Mutex<Vec<GenerationSnapshot>>,
}

impl RecordingDoorFindings {
    pub(crate) const fn new(script: ScriptedFinding) -> Self {
        Self {
            script,
            asked: Mutex::new(Vec::new()),
        }
    }

    /// Every generation the double was asked about, in order.
    pub(crate) fn asked(&self) -> Result<Vec<GenerationSnapshot>, CoreError> {
        self.asked
            .lock()
            .map(|asked| asked.clone())
            .map_err(|error| CoreError::Storage(format!("door findings double poisoned: {error}")))
    }
}

/// The path the double reports a quarantine at, for `generation`.
pub(crate) fn scripted_quarantine_path(generation: &GenerationSnapshot) -> PathBuf {
    PathBuf::from(format!(
        "/scripted/{:?}/g{}",
        generation.track,
        generation.manifest_generation.get()
    ))
}

impl DoorFindingQuarantinePort for RecordingDoorFindings {
    fn quarantine_door_finding(
        &self,
        generation: &GenerationSnapshot,
    ) -> Result<DoorFindingOutcome, CoreError> {
        self.asked
            .lock()
            .map_err(|error| CoreError::Storage(format!("door findings double poisoned: {error}")))?
            .push(generation.clone());
        match self.script {
            ScriptedFinding::Quarantines => Ok(DoorFindingOutcome::Quarantined {
                quarantined: QuarantinedGenerationV1 {
                    track: generation.track,
                    path: scripted_quarantine_path(generation),
                    reason: GenerationQuarantineReasonV1::ContentCorrupt,
                    detail: "scripted".to_string(),
                },
            }),
            ScriptedFinding::DoesNotReproduce => Ok(DoorFindingOutcome::NotReproduced),
            ScriptedFinding::Fails => Err(CoreError::NotFound(
                "scripted: the generation directory is gone".to_string(),
            )),
        }
    }
}
