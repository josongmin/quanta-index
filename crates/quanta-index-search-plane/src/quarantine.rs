//! The quarantine control service (QI-BB-026 follow-up).
//!
//! Boot sets aside what it cannot trust and serves without it; this is the
//! operator's live view of that set and the one path that removes an
//! entry. The inventory is read from the adapters each time, never from a
//! boot snapshot, so a discard or a repair is visible on the next listing,
//! and a discard names exactly what a listing reported: the adapter
//! re-inventories and refuses anything it does not quarantine at that
//! moment.

use std::sync::Arc;

use quanta_index_contract::{
    QuarantineDiscardAck, QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1, QuarantineTargetV1,
    QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1, SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationQuarantineReasonV1, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, QuarantinedGenerationV1, QuarantinedRepoMapFileV1,
    RepoMapQuarantinePort, SealedGenerationScanPort,
};

/// The ports one [`QuarantineService`] is composed from.
pub struct QuarantineServiceParts {
    pub lexical_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub semantic_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub lexical_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub semantic_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub repo_map: Arc<dyn RepoMapQuarantinePort + Send + Sync>,
}

/// Live quarantine inventory and discard over the adapters' ports.
pub struct QuarantineService {
    lexical_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    semantic_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    lexical_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    semantic_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    repo_map: Arc<dyn RepoMapQuarantinePort + Send + Sync>,
}

impl QuarantineService {
    #[must_use]
    pub fn new(parts: QuarantineServiceParts) -> Self {
        let QuarantineServiceParts {
            lexical_scanner,
            semantic_scanner,
            lexical_discard,
            semantic_discard,
            repo_map,
        } = parts;
        Self {
            lexical_scanner,
            semantic_scanner,
            lexical_discard,
            semantic_discard,
            repo_map,
        }
    }

    /// Everything quarantined right now, per authority, as the adapters
    /// report it this instant.
    pub fn inventory(&self) -> Result<QuarantineInventoryV1, CoreError> {
        Ok(QuarantineInventoryV1 {
            lexical: track_entries(
                SearchPlaneTrackKind::Lexical,
                &self
                    .lexical_scanner
                    .inventory_sealed_generations()?
                    .quarantined,
            )?,
            semantic: track_entries(
                SearchPlaneTrackKind::Semantic,
                &self
                    .semantic_scanner
                    .inventory_sealed_generations()?
                    .quarantined,
            )?,
            repo_map: self
                .repo_map
                .quarantined_files()?
                .iter()
                .map(repo_map_entry)
                .collect(),
        })
    }

    /// Discard one entry exactly as a listing reported it.
    pub fn discard(&self, target: &QuarantineTargetV1) -> Result<QuarantineDiscardAck, CoreError> {
        let outcome = match target {
            QuarantineTargetV1::Generation(entry) => {
                let core_entry = generation_entry_from_wire(entry)?;
                let port = match entry.track {
                    SearchPlaneTrackKind::Lexical => &self.lexical_discard,
                    SearchPlaneTrackKind::Semantic => &self.semantic_discard,
                    SearchPlaneTrackKind::Structural => {
                        return Err(CoreError::InvalidContract(
                            "quarantine discard: the structural track has no generation directories to quarantine"
                                .to_string(),
                        ));
                    }
                };
                port.discard_quarantined_generation(&core_entry)?
            }
            QuarantineTargetV1::RepoMapFile(entry) => {
                self.repo_map
                    .discard_quarantined_file(&QuarantinedRepoMapFileV1 {
                        file_name: entry.file_name.clone(),
                        reason: entry.reason.clone(),
                    })?
            }
        };
        Ok(QuarantineDiscardAck {
            target: target.clone(),
            outcome: match outcome {
                QuarantineDiscardOutcomeV1::Discarded { bytes } => {
                    QuarantineDiscardOutcomeDtoV1::Discarded { bytes }
                }
                QuarantineDiscardOutcomeV1::Absent => QuarantineDiscardOutcomeDtoV1::Absent,
            },
        })
    }
}

/// One track's quarantined directories as wire entries.
///
/// An adapter that reports another track's entry under this track is a
/// defect, surfaced rather than relabelled.
fn track_entries(
    track: SearchPlaneTrackKind,
    quarantined: &[QuarantinedGenerationV1],
) -> Result<Vec<QuarantinedGenerationEntryV1>, CoreError> {
    quarantined
        .iter()
        .map(|entry| {
            if entry.track != track {
                return Err(CoreError::Storage(format!(
                    "quarantine inventory: the {track:?} scanner reported {} under track {:?}",
                    entry.path.display(),
                    entry.track
                )));
            }
            let path = entry.path.to_str().ok_or_else(|| {
                CoreError::Storage(format!(
                    "quarantine inventory: path {} is not UTF-8 and cannot be named on the wire",
                    entry.path.display()
                ))
            })?;
            Ok(QuarantinedGenerationEntryV1 {
                track: entry.track,
                path: path.to_string(),
                reason: entry.reason.as_code_str().to_string(),
                detail: entry.detail.clone(),
            })
        })
        .collect()
}

fn repo_map_entry(entry: &QuarantinedRepoMapFileV1) -> QuarantinedRepoMapFileEntryV1 {
    QuarantinedRepoMapFileEntryV1 {
        file_name: entry.file_name.clone(),
        reason: entry.reason.clone(),
    }
}

/// The adapter-side entry a wire entry names; a reason code the domain
/// does not know is refused before any port sees it.
fn generation_entry_from_wire(
    entry: &QuarantinedGenerationEntryV1,
) -> Result<QuarantinedGenerationV1, CoreError> {
    let reason = GenerationQuarantineReasonV1::from_code_str(&entry.reason).ok_or_else(|| {
        CoreError::InvalidContract(format!(
            "quarantine discard: unknown quarantine reason `{}`",
            entry.reason
        ))
    })?;
    Ok(QuarantinedGenerationV1 {
        track: entry.track,
        path: std::path::PathBuf::from(&entry.path),
        reason,
        detail: entry.detail.clone(),
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert_eq!` on the doubles' recorded calls; a violated expectation is a test failure, not a propagatable error"
)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use quanta_index_contract::{
        QuarantineDiscardOutcomeDtoV1, QuarantineTargetV1, QuarantinedGenerationEntryV1,
        QuarantinedRepoMapFileEntryV1, SearchPlaneTrackKind,
    };
    use quanta_index_core::{
        CoreError, GenerationQuarantineReasonV1, QUARANTINE_TARGET_NOT_QUARANTINED_CODE,
        QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort, QuarantinedGenerationV1,
        QuarantinedRepoMapFileV1, RepoMapQuarantinePort, SealedGenerationInventoryV1,
        SealedGenerationScanPort,
    };

    use super::{QuarantineService, QuarantineServiceParts};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct ScriptedScanner(Vec<QuarantinedGenerationV1>);

    impl SealedGenerationScanPort for ScriptedScanner {
        fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
            Ok(SealedGenerationInventoryV1 {
                sealed: Vec::new(),
                quarantined: self.0.clone(),
            })
        }
    }

    /// Discards what it was told is quarantined; records every call.
    pub(crate) struct ScriptedDiscard {
        quarantined: Vec<QuarantinedGenerationV1>,
        pub(crate) discarded: Mutex<Vec<QuarantinedGenerationV1>>,
    }

    impl QuarantinedGenerationDiscardPort for ScriptedDiscard {
        fn discard_quarantined_generation(
            &self,
            entry: &QuarantinedGenerationV1,
        ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
            if !self.quarantined.contains(entry) {
                return Err(CoreError::Typed {
                    code: QUARANTINE_TARGET_NOT_QUARANTINED_CODE.to_string(),
                    message: format!("not quarantined now: {}", entry.path.display()),
                });
            }
            self.discarded
                .lock()
                .map_err(|err| CoreError::Storage(err.to_string()))?
                .push(entry.clone());
            Ok(QuarantineDiscardOutcomeV1::Discarded { bytes: 42 })
        }
    }

    struct ScriptedRepoMap(Vec<QuarantinedRepoMapFileV1>);

    impl RepoMapQuarantinePort for ScriptedRepoMap {
        fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError> {
            Ok(self.0.clone())
        }

        fn discard_quarantined_file(
            &self,
            entry: &QuarantinedRepoMapFileV1,
        ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
            match self
                .0
                .iter()
                .find(|listed| listed.file_name == entry.file_name)
            {
                Some(listed) if listed == entry => {
                    Ok(QuarantineDiscardOutcomeV1::Discarded { bytes: 7 })
                }
                Some(listed) => Err(CoreError::Typed {
                    code: QUARANTINE_TARGET_NOT_QUARANTINED_CODE.to_string(),
                    message: format!(
                        "recorded as `{}` now, not `{}`",
                        listed.reason, entry.reason
                    ),
                }),
                None => Ok(QuarantineDiscardOutcomeV1::Absent),
            }
        }
    }

    pub(crate) fn quarantined(track: SearchPlaneTrackKind, path: &str) -> QuarantinedGenerationV1 {
        QuarantinedGenerationV1 {
            track,
            path: PathBuf::from(path),
            reason: GenerationQuarantineReasonV1::IdentityUnreadable,
            detail: "identity file does not decode".to_string(),
        }
    }

    /// A service over scripted ports: the lexical discard double is handed
    /// back so a test can read what it removed.
    pub(crate) fn service(
        lexical: Vec<QuarantinedGenerationV1>,
        semantic: Vec<QuarantinedGenerationV1>,
        repo_map: Vec<QuarantinedRepoMapFileV1>,
    ) -> (QuarantineService, Arc<ScriptedDiscard>) {
        let lexical_discard = Arc::new(ScriptedDiscard {
            quarantined: lexical.clone(),
            discarded: Mutex::new(Vec::new()),
        });
        let semantic_discard = Arc::new(ScriptedDiscard {
            quarantined: semantic.clone(),
            discarded: Mutex::new(Vec::new()),
        });
        let service = QuarantineService::new(QuarantineServiceParts {
            lexical_scanner: Arc::new(ScriptedScanner(lexical)),
            semantic_scanner: Arc::new(ScriptedScanner(semantic)),
            lexical_discard: lexical_discard.clone(),
            semantic_discard,
            repo_map: Arc::new(ScriptedRepoMap(repo_map)),
        });
        (service, lexical_discard)
    }

    /// The inventory is the adapters' own, per track, with reasons as their
    /// wire codes; a discard routes by track and comes back as the target
    /// named plus the adapter's outcome.
    #[test]
    fn inventory_and_discard_route_by_track_and_carry_the_adapter_outcome() -> TestResult {
        let lexical = quarantined(SearchPlaneTrackKind::Lexical, "/root/lexical/junk");
        let semantic = quarantined(SearchPlaneTrackKind::Semantic, "/root/semantic/g7");
        let (service, lexical_discard) = service(
            vec![lexical.clone()],
            vec![semantic],
            vec![QuarantinedRepoMapFileV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "did not decode".to_string(),
            }],
        );
        let inventory = service.inventory()?;
        assert_eq!(inventory.lexical.len(), 1);
        assert_eq!(inventory.semantic.len(), 1);
        assert_eq!(inventory.repo_map.len(), 1);
        let listed = inventory.lexical.first().ok_or("lexical entry")?;
        assert_eq!(
            (listed.track, listed.path.as_str(), listed.reason.as_str()),
            (
                SearchPlaneTrackKind::Lexical,
                "/root/lexical/junk",
                "GENERATION_QUARANTINE_IDENTITY_UNREADABLE"
            )
        );

        let ack = service.discard(&QuarantineTargetV1::Generation(listed.clone()))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 42 }
        );
        assert_eq!(ack.target, QuarantineTargetV1::Generation(listed.clone()));
        let discarded = lexical_discard
            .discarded
            .lock()
            .map_err(|err| err.to_string())?
            .clone();
        assert_eq!(
            discarded,
            vec![lexical],
            "the lexical port got the lexical entry"
        );

        let ack = service.discard(&QuarantineTargetV1::RepoMapFile(
            QuarantinedRepoMapFileEntryV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "did not decode".to_string(),
            },
        ))?;
        assert_eq!(
            ack.outcome,
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 7 }
        );
        Ok(())
    }

    /// A reason the domain does not know, or an entry the adapter no longer
    /// quarantines, is refused typed before or by the port.
    #[test]
    fn a_stale_or_malformed_target_is_refused_typed() -> TestResult {
        let (service, _lexical_discard) = service(
            vec![quarantined(
                SearchPlaneTrackKind::Lexical,
                "/root/lexical/junk",
            )],
            Vec::new(),
            Vec::new(),
        );
        let unknown_reason = QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: "/root/lexical/junk".to_string(),
            reason: "NOT_A_REASON".to_string(),
            detail: String::new(),
        };
        match service.discard(&QuarantineTargetV1::Generation(unknown_reason)) {
            Err(CoreError::InvalidContract(message)) if message.contains("NOT_A_REASON") => {}
            other => return Err(format!("unknown reason must be refused: {other:?}").into()),
        }
        let repaired = QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: "/root/lexical/repaired".to_string(),
            reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
            detail: String::new(),
        };
        match service.discard(&QuarantineTargetV1::Generation(repaired)) {
            Err(CoreError::Typed { code, .. })
                if code == QUARANTINE_TARGET_NOT_QUARANTINED_CODE => {}
            other => return Err(format!("a stale entry must be refused typed: {other:?}").into()),
        }
        Ok(())
    }
}
