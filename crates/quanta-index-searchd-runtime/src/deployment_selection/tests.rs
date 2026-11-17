//! Real filesystem/lease tests of the file publisher, not producer acceptance.
use super::*;
use crate::native_resources::{NativeResourceClass, NativeResourceLedger, NativeResourcePlan};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        WorkLimits,
        effort::{ReservationGuard, ResourceNeed},
    },
};
use std::{
    error::Error,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixDatagram,
    },
    time::Duration,
};

#[path = "../../tests/common/legacy_physical_fixture.rs"]
mod legacy_physical_fixture;

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;
fn require(ok: bool, label: &str) -> TestResult {
    if ok { Ok(()) } else { Err(label.into()) }
}
fn budget() -> TestResult<(
    WorkBudget,
    ReservationGuard<crate::native_resources::NativeClassScheduler>,
)> {
    budget_with_bytes(64 * 1024 * 1024)
}
fn budget_with_bytes(
    byte_limit: u64,
) -> TestResult<(
    WorkBudget,
    ReservationGuard<crate::native_resources::NativeClassScheduler>,
)> {
    let unit = ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024 * 1024,
        memory_bytes: 1024 * 1024,
        provider_units: 0,
    };
    let control = ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * 1024 * 1024,
        memory_bytes: 16 * 1024 * 1024,
        provider_units: 0,
    };
    let ledger = NativeResourceLedger::new(NativeResourcePlan {
        capacity: ResourceNeed {
            cpu_units: 8,
            io_bytes: 256 * 1024 * 1024,
            memory_bytes: 64 * 1024 * 1024,
            provider_units: 0,
        },
        control,
        interactive: unit,
        core: unit,
        enrichment: unit,
    })?;
    let execution =
        ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Control), control)
            .ok_or("execution admission refused")?;
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: byte_limit,
            mutations: 10_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(10)),
    );
    budget.bind_memory_scope(execution.memory_scope());
    Ok((budget, execution))
}
fn digest_text(value: Digest) -> String {
    value.0.iter().map(|b| format!("{b:02x}")).collect()
}
fn quoted(path: &Path) -> TestResult<String> {
    Ok(serde_json::to_string(path.to_str().ok_or("UTF-8 path")?)?)
}
const OPERATION: &str = "{\"epoch\":1,\"producer\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"sequence\":1,\"stream\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"stream_epoch\":1,\"workspace\":{\"collection\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"slot\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"tenant\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],\"worktree\":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]}}";
struct Fixture {
    _temp: tempfile::TempDir,
    selection: PathBuf,
    candidate: PathBuf,
    before: Vec<u8>,
    after: Vec<u8>,
}
impl Fixture {
    fn new(mode: DeploymentRecoveryModeV1) -> TestResult<Self> {
        Self::with_binary(mode, b"#!/bin/sh\nexit 0\n")
    }
    fn with_binary(mode: DeploymentRecoveryModeV1, binary_bytes: &[u8]) -> TestResult<Self> {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let root = temp.path().canonicalize()?;
        let binary = root.join("searchd");
        fs::write(&binary, binary_bytes)?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        let binary_digest = digest_text(fingerprint(&fs::read(&binary)?));
        let native = |name: &str, nonce: u8| -> TestResult<Vec<u8>> {
            let state = root.join(name);
            fs::create_dir(&state)?;
            let config = root.join(format!("{name}.config.json"));
            let config_bytes = format!("{{\"state_root\":{}}}", quoted(&state)?).into_bytes();
            fs::write(&config, &config_bytes)?;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
            // Fixed sorted schema-2 template: independent of the new codec.
            Ok(format!(concat!("{{\"binary\":{},\"binary_sha256\":\"{}\",\"environment\":{{\"LANG\":\"ko_KR.UTF-8\"}},",
                "\"mode\":\"native\",\"native_boot_receipt\":{{\"install_catalog_digest_hex\":\"{}\",",
                "\"manifest_object_hex\":\"{}\",\"manifest_operation\":{},\"root_nonce_hex\":\"{}\",\"source_cut_digest_hex\":\"{}\"}},",
                "\"native_config\":{},\"native_config_sha256\":\"{}\",\"schema_version\":2,\"state_root\":{}}}\n"),
                quoted(&binary)?,binary_digest,"11".repeat(32),"22".repeat(32),OPERATION,
                format!("{nonce:02x}").repeat(32),"33".repeat(32),quoted(&config)?,digest_text(fingerprint(&config_bytes)),quoted(&state)?).into_bytes())
        };
        let before = native("old", 1)?;
        let after = if mode == DeploymentRecoveryModeV1::RestoreForward {
            native("new", 2)?
        } else {
            let state = root.join("legacy");
            fs::create_dir(&state)?;
            format!("{{\"binary\":{},\"binary_sha256\":\"{}\",\"environment\":{{\"LANG\":\"ko_KR.UTF-8\"}},\"mode\":\"legacy\",\"schema_version\":2,\"state_root\":{}}}\n",quoted(&binary)?,binary_digest,quoted(&state)?).into_bytes()
        };
        let selection = root.join("selection.json");
        let candidate = root.join("candidate.json");
        for (path, bytes) in [(&selection, &before), (&candidate, &after)] {
            fs::write(path, bytes)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self {
            _temp: temp,
            selection,
            candidate,
            before,
            after,
        })
    }
    fn request(&self, mode: DeploymentRecoveryModeV1) -> DeploymentSelectionRequestV1<'_> {
        DeploymentSelectionRequestV1 {
            selection_path_v1: &self.selection,
            expected_current_sha256_v1: fingerprint(&self.before),
            candidate_path_v1: &self.candidate,
            expected_candidate_sha256_v1: fingerprint(&self.after),
            mode_v1: mode,
        }
    }

    fn before_document(
        &self,
        operation_id: &'static str,
    ) -> TestResult<DeploymentSelectionBeforeDocumentV1<'_>> {
        Ok(DeploymentSelectionBeforeDocumentV1 {
            operation_id_v1: operation_id,
            bytes_v1: &self.before,
            raw_sha256_v1: fingerprint(&self.before),
            file_version_v1: Some(NativeCutoverEvidenceBeforeSelectionV1::file_version_v1(
                &fs::metadata(&self.selection)?,
            )),
            retained_terminal_v1: None,
        })
    }

    fn publish_with_intent(
        &self,
        mode: DeploymentRecoveryModeV1,
        before: &DeploymentSelectionBeforeDocumentV1<'_>,
    ) -> TestResult<DeploymentSelectionPublishedV1> {
        let (mut work, _execution) = budget()?;
        Ok(with_locked_deployment_selection_reconciliation_v1(
            &self.request(mode),
            before,
            &mut work,
            |locked, work| locked.publish_v1(work),
        )?)
    }
}

#[test]
fn saved_repomap_policy_is_bound_to_the_separate_root_and_exact_control() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RollbackLegacy;
    let fixture = Fixture::new(mode)?;
    let parent = fixture.selection.parent().ok_or("missing deployment directory")?;
    let legacy_root = parent.join("legacy");
    let repomap_root = parent.join("legacy-repomap");
    fs::create_dir(&repomap_root)?;
    let saved = parent.join("repomap.saved.json");
    let mut environment = legacy_physical_fixture::legacy_physical_env()
        .collect::<std::collections::BTreeMap<_, _>>();
    let _prior = environment.insert("LANG", "ko_KR.UTF-8".into());
    let bytes = String::from_utf8(fixture.after.clone())?
        .replace(&quoted(&legacy_root)?, &quoted(&repomap_root)?)
        .replace(r#"{"LANG":"ko_KR.UTF-8"}"#, &serde_json::to_string(&environment)?)
        .into_bytes();
    fs::write(&saved, &bytes)?;
    fs::set_permissions(&saved, fs::Permissions::from_mode(0o600))?;
    let expected = fingerprint(&bytes);
    let (mut budget, _execution) = budget()?;
    let result = with_locked_deployment_selection_v1(
        &fixture.request(mode),
        &mut budget,
        |locked, budget| {
            (|| -> Result<()> {
                let selected = locked.read_legacy_repomap_selection_v1(
                    &saved, expected, &repomap_root, budget,
                )?;
                ensure!(selected.state_root_v1() == repomap_root);
                ensure!(selected.legacy_physical_policy_v1(budget)?
                    == quanta_index_searchd::app::config::TEST_STATE_ROOT_PHYSICAL_POLICY);
                drop(selected);
                ensure!(
                    locked.read_legacy_repomap_selection_v1(&saved, expected, &legacy_root, budget).is_err()
                );
                ensure!(
                    locked.read_legacy_repomap_selection_v1(&saved, Digest([1; 32]), &repomap_root, budget).is_err()
                );
                Ok(())
            })().map_err(DeploymentSelectionErrorV1::RefusedBeforeRename)?;
            locked.publish_with_custody_v1(budget, &mut |budget| {
                let replacement = String::from_utf8(bytes.clone())?.replace("ko_KR.UTF-8", "C");
                fs::write(&saved, replacement)?;
                LockedDeploymentSelectionV1::require_legacy_repomap_selection_v1(&saved, expected, budget)
            })
        },
    );
    require(
        matches!(result, Err(DeploymentSelectionErrorV1::RefusedBeforeRename(error))
            if error.to_string() == "saved RepoMap selection changed during publication"),
        "changed saved RepoMap selection was not refused",
    )?;
    require(fs::read(&fixture.selection)? == fixture.before, "refusal changed selection")
}

#[test]
fn recovery_settlement_refuses_queued_control_after_durable_publication() -> TestResult {
    use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverControlCancelV1;

    let mut cancel = Vec::new();
    ciborium::into_writer(
        &NativeCutoverControlCancelV1 {
            format_version: 1,
            operation_id: "queued-recovery-cancel".into(),
            nonce: Digest([3; 32]),
        },
        &mut cancel,
    )?;
    for frame in [None, Some(cancel.as_slice()), Some(b"invalid"), Some(b"")] {
        let mode = DeploymentRecoveryModeV1::RollbackLegacy;
        let fixture = Fixture::new(mode)?;
        let (mut budget, _execution) = budget()?;
        let published = with_locked_deployment_selection_v1(
            &fixture.request(mode),
            &mut budget,
            |locked, budget| locked.publish_v1(budget),
        )?;
        let (resident, helper) = UnixDatagram::pair()?;
        if let Some(bytes) = frame {
            require(
                resident.send(bytes)? == bytes.len(),
                "incomplete control datagram",
            )?;
        }
        let settled = crate::native_recovery::settle_published_v1(&helper, published);
        match frame {
            None => require(
                settled?.after_sha256_v1() == fingerprint(&fixture.after),
                "an empty control queue must preserve the durable publication",
            )?,
            Some(_) => require(
                matches!(
                    settled,
                    Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
                ),
                "queued control must force reconciliation",
            )?,
        }
        require(
            fs::read(&fixture.selection)? == fixture.after,
            "control settlement must preserve the exact durable selection",
        )?;
    }
    Ok(())
}

#[test]
fn canonical_payload_matches_python_ascii_u64_golden() -> TestResult {
    let value = codec::unique_json(
        "{\"z\":\"한글😀\\u007f\\n\",\"a\":{\"x\":18446744073709551615,\"b\":[0,255]}}".as_bytes(),
    )?;
    let expected=b"{\"a\":{\"b\":[0,255],\"x\":18446744073709551615},\"z\":\"\\ud55c\\uae00\\ud83d\\ude00\\u007f\\n\"}\n";
    let actual = codec::canonical_payload(&value)?;
    require(actual == expected, "Python byte golden differs")?;
    require(
        digest_text(fingerprint(&actual))
            == "c0491b9cfe1e419e115e7d4b6a210bcab3790d6eab31382be01443bbad9a6349",
        "Python SHA golden differs",
    )
}
#[test]
fn canonical_codec_rejects_nested_duplicate_fields() -> TestResult {
    for wire in [
        r#"{"a":1,"a":2}"#,
        r#"{"a":{"x":0,"x":0}}"#,
        r#"{"a":0} trailing"#,
    ] {
        require(
            codec::unique_json(wire.as_bytes()).is_err(),
            "malformed JSON accepted",
        )?;
    }
    Ok(())
}
#[test]
fn publisher_holds_actual_lock_and_publishes_both_modes() -> TestResult {
    for mode in [
        DeploymentRecoveryModeV1::RollbackLegacy,
        DeploymentRecoveryModeV1::RestoreForward,
    ] {
        let fixture = Fixture::new(mode)?;
        let (mut work, _execution) = budget()?;
        let outcome = with_locked_deployment_selection_v1(
            &fixture.request(mode),
            &mut work,
            |locked, work| {
                let contender = File::open(
                    fixture
                        .selection
                        .parent()
                        .ok_or_else(|| {
                            DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                                "parent missing"
                            ))
                        })?
                        .join(LOCK_NAME),
                )
                .map_err(|e| DeploymentSelectionErrorV1::RefusedBeforeRename(e.into()))?;
                if rustix::fs::flock(
                    &contender,
                    rustix::fs::FlockOperation::NonBlockingLockExclusive,
                )
                .is_ok()
                {
                    return Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                        "actual lock contender entered"
                    )));
                }
                locked.publish_v1(work)
            },
        )?;
        require(
            outcome.before_sha256_v1 == fingerprint(&fixture.before)
                && outcome.after_sha256_v1 == fingerprint(&fixture.after),
            "published fingerprints",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.after,
            "published wrong selection bytes",
        )?;
        require(
            fs::metadata(&fixture.selection)?.mode() & 0o777 == 0o600,
            "selection mode differs",
        )?;
        let contender = File::open(fixture.selection.parent().ok_or("parent")?.join(LOCK_NAME))?;
        rustix::fs::flock(
            &contender,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )?;
    }
    Ok(())
}
#[test]
fn callback_success_without_publication_is_refused() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RollbackLegacy;
    let fixture = Fixture::new(mode)?;
    let original_inode = fs::metadata(&fixture.selection)?.ino();
    let (mut work, _execution) = budget()?;
    let outcome =
        with_locked_deployment_selection_v1(&fixture.request(mode), &mut work, |locked, _work| {
            Ok(DeploymentSelectionPublishedV1 {
                resolution_v1: DeploymentSelectionResolutionV1::Published,
                before_sha256_v1: locked.current_v1().fingerprint_v1(),
                after_sha256_v1: locked.candidate_v1().fingerprint_v1(),
            })
        });
    require(
        matches!(
            outcome,
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
        ),
        "callback success without publication was accepted",
    )?;
    require(
        fs::read(&fixture.selection)? == fixture.before
            && fs::metadata(&fixture.selection)?.ino() == original_inode,
        "callback changed selection without publication",
    )
}

#[test]
fn foreign_published_outcome_after_real_rename_is_uncertain() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RollbackLegacy;
    let previous = Fixture::new(mode)?;
    let (mut previous_work, _previous_execution) = budget()?;
    let prior_outcome = with_locked_deployment_selection_v1(
        &previous.request(mode),
        &mut previous_work,
        |locked, work| locked.publish_v1(work),
    )?;
    let current = Fixture::new(mode)?;
    let (mut current_work, _current_execution) = budget()?;
    let outcome = with_locked_deployment_selection_v1(
        &current.request(mode),
        &mut current_work,
        |locked, work| {
            let _actual = locked.publish_v1(work)?;
            Ok(prior_outcome)
        },
    );
    require(
        matches!(
            outcome,
            Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
        ),
        "foreign published outcome was accepted after real rename",
    )?;
    require(
        fs::read(&current.selection)? == current.after,
        "foreign outcome test did not execute the real rename",
    )
}

#[test]
fn stale_cas_and_changed_candidate_refuse_before_rename() -> TestResult {
    for change_candidate in [false, true] {
        let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
            &mut work,
            |locked, work| {
                locked.publish_inner(work, &mut |stage| {
                    if matches!(stage, PublishStage::AfterFileFsync) {
                        fs::write(
                            if change_candidate {
                                &fixture.candidate
                            } else {
                                &fixture.selection
                            },
                            b"{}\n",
                        )?;
                    }
                    Ok(())
                })
            },
        );
        require(
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
            ),
            "changed CAS/candidate did not refuse",
        )?;
        require(
            fs::read(&fixture.selection)?
                == if change_candidate {
                    fixture.before.clone()
                } else {
                    b"{}\n".to_vec()
                },
            "refusal overwrote concurrent selection",
        )?;
    }
    Ok(())
}
#[test]
fn postrename_faults_report_uncertainty_with_actual_target_bytes() -> TestResult {
    for fault_after_directory in [false, true] {
        let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
            &mut work,
            |locked, work| {
                locked.publish_inner(work, &mut |stage| {
                    if matches!(
                        (stage, fault_after_directory),
                        (PublishStage::AfterRename, false)
                            | (PublishStage::AfterDirectoryFsync, true)
                    ) {
                        return Err(anyhow!("injected terminal fault"));
                    }
                    Ok(())
                })
            },
        );
        require(
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            ),
            "postrename failure reported ordinary refusal",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.after,
            "rename did not publish actual target",
        )?;
    }
    Ok(())
}

#[test]
fn replaced_temporary_after_custody_checkpoint_refuses_before_rename() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    let fixture = Fixture::new(mode)?;
    let (mut work, _execution) = budget()?;
    let mut replaced = None;
    let result =
        with_locked_deployment_selection_v1(&fixture.request(mode), &mut work, |locked, work| {
            locked.publish_with_custody_v1(work, &mut |_| {
                let parent = fixture
                    .selection
                    .parent()
                    .ok_or_else(|| anyhow!("fixture parent missing"))?;
                for entry in fs::read_dir(parent)? {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".selection.json.tmp-")
                    {
                        let path = entry.path();
                        fs::remove_file(&path)?;
                        fs::write(&path, &fixture.before)?;
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                        replaced = Some(path);
                        return Ok(());
                    }
                }
                Ok(())
            })
        });
    let replaced = replaced.ok_or("temporary replacement did not execute")?;
    require(
        matches!(
            result,
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
        ),
        "replaced temporary was not refused before rename",
    )?;
    require(
        fs::read(&fixture.selection)? == fixture.before,
        "selection changed",
    )?;
    require(
        fs::read(replaced)? == fixture.before,
        "refusal deleted the replacement",
    )
}

#[test]
fn changed_selection_or_lock_after_directory_sync_is_uncertain() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    for change in ["bytes", "inode", "lock"] {
        let fixture = Fixture::new(mode)?;
        let (mut work, _execution) = budget()?;
        let mut changed = false;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(mode),
            &mut work,
            |locked, work| {
                locked.publish_inner(work, &mut |stage| {
                    if matches!(stage, PublishStage::AfterDirectoryFsync) {
                        match change {
                            "bytes" => fs::write(&fixture.selection, &fixture.before)?,
                            "inode" => {
                                fs::remove_file(&fixture.selection)?;
                                fs::write(&fixture.selection, &fixture.after)?;
                                fs::set_permissions(
                                    &fixture.selection,
                                    fs::Permissions::from_mode(0o600),
                                )?;
                            }
                            "lock" => {
                                let lock = fixture.selection.with_file_name(LOCK_NAME);
                                fs::remove_file(&lock)?;
                                fs::write(&lock, b"")?;
                                fs::set_permissions(&lock, fs::Permissions::from_mode(0o600))?;
                            }
                            _ => return Err(anyhow!("unknown post-sync fixture change")),
                        }
                        changed = true;
                    }
                    Ok(())
                })
            },
        );
        require(changed, "post-sync replacement did not execute")?;
        require(
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            ),
            "post-sync replacement was not classified as uncertain",
        )?;
    }
    Ok(())
}

#[test]
fn binary_changed_during_final_custody_refuses_before_rename() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    let fixture = Fixture::new(mode)?;
    let binary = fixture.selection.with_file_name("searchd");
    let (mut work, _execution) = budget()?;
    let result =
        with_locked_deployment_selection_v1(&fixture.request(mode), &mut work, |locked, work| {
            locked.publish_with_custody_v1(work, &mut |_| {
                fs::write(&binary, b"#!/bin/sh\nexit 123\n")?;
                Ok(())
            })
        });
    require(
        matches!(result, Err(DeploymentSelectionErrorV1::RefusedBeforeRename(cause))
            if cause.to_string() == "selected binary changed after hashing"),
        "changed executable was not detected before rename",
    )?;
    require(
        fs::read(&fixture.selection)? == fixture.before,
        "selection changed",
    )
}

#[test]
fn selected_binary_or_config_change_after_sync_is_uncertain() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    for binary in [false, true] {
        let fixture = Fixture::new(mode)?;
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(mode),
            &mut work,
            |locked, work| {
                locked.publish_inner(work, &mut |stage| {
                    if matches!(stage, PublishStage::AfterDirectoryFsync) {
                        if binary {
                            fs::write(
                                fixture.selection.with_file_name("searchd"),
                                b"changed image",
                            )?;
                        } else {
                            fs::write(fixture.selection.with_file_name("new.config.json"), b"{}")?;
                        }
                    }
                    Ok(())
                })
            },
        );
        let expected = if binary {
            "selected binary changed after hashing"
        } else {
            "native config SHA-256 changed"
        };
        require(
            matches!(result, Err(DeploymentSelectionErrorV1::PersistenceUncertain(cause))
                if cause.to_string() == expected),
            "changed selected artifact was not detected after sync",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.after,
            "rename did not execute",
        )?;
    }
    Ok(())
}

#[test]
fn control_file_symlink_hardlink_and_permissions_refuse() -> TestResult {
    for case in 0..3 {
        let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
        if case == 0 {
            fs::remove_file(&fixture.candidate)?;
            symlink(&fixture.selection, &fixture.candidate)?;
        }
        if case == 1 {
            fs::hard_link(
                &fixture.candidate,
                fixture.candidate.with_extension("alias"),
            )?;
        }
        if case == 2 {
            fs::set_permissions(&fixture.candidate, fs::Permissions::from_mode(0o644))?;
        }
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
            &mut work,
            |locked, work| locked.publish_v1(work),
        );
        require(
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
            ),
            "unsafe control accepted",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.before,
            "refusal changed selection",
        )?;
    }
    Ok(())
}
#[test]
fn absent_memory_authority_refuses_before_lock_creation() -> TestResult {
    let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1024 * 1024,
            mutations: 100,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(2)),
    );
    let result = with_locked_deployment_selection_v1(
        &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
        &mut work,
        |locked, work| locked.publish_v1(work),
    );
    require(
        matches!(
            result,
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
        ),
        "missing memory authority accepted",
    )?;
    require(
        !fixture
            .selection
            .parent()
            .ok_or("parent")?
            .join(LOCK_NAME)
            .exists(),
        "unfunded operation created lock",
    )
}

#[test]
fn deployment_lock_cannot_be_used_as_a_selection_document() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    for active in [false, true] {
        let mut fixture = Fixture::new(mode)?;
        let lock = fixture.selection.with_file_name(LOCK_NAME);
        if active {
            fs::rename(&fixture.selection, &lock)?;
            fixture.selection = lock.clone();
        } else {
            fs::rename(&fixture.candidate, &lock)?;
            fixture.candidate = lock.clone();
        }
        let before = fs::read(&lock)?;
        let inode = fs::metadata(&lock)?.ino();
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(mode),
            &mut work,
            |locked, work| locked.publish_v1(work),
        );
        require(
            matches!(result, Err(DeploymentSelectionErrorV1::RefusedBeforeRename(cause))
                if cause.to_string() == "selection path aliases deployment lock"),
            "deployment lock was accepted as a selection document",
        )?;
        require(fs::read(&lock)? == before, "lock contents changed")?;
        require(fs::metadata(&lock)?.ino() == inode, "lock inode changed")?;
    }
    Ok(())
}

#[test]
fn cancellation_is_refusal_before_rename_and_uncertain_after_rename() -> TestResult {
    for after_rename in [false, true] {
        let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
        let (mut work, _execution) = budget()?;
        let cancel = work.request().cancel_handle();
        let result = with_locked_deployment_selection_v1(
            &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
            &mut work,
            |locked, work| {
                locked.publish_inner(work, &mut |stage| {
                    if matches!(
                        (stage, after_rename),
                        (PublishStage::AfterFileFsync, false) | (PublishStage::AfterRename, true)
                    ) {
                        cancel.cancel();
                    }
                    Ok(())
                })
            },
        );
        let correct = if after_rename {
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            )
        } else {
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
            )
        };
        require(correct, "cancellation misclassified rename boundary")?;
        require(
            fs::read(&fixture.selection)?
                == if after_rename {
                    fixture.after
                } else {
                    fixture.before
                },
            "cancellation selection bytes",
        )?;
    }
    Ok(())
}
#[test]
fn replaced_lock_inode_does_not_authorize_publication() -> TestResult {
    let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
    let (mut work, _execution) = budget()?;
    let result = with_locked_deployment_selection_v1(
        &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
        &mut work,
        |locked, work| {
            locked.publish_inner(work, &mut |stage| {
                if matches!(stage, PublishStage::AfterFileFsync) {
                    let lock = fixture
                        .selection
                        .parent()
                        .ok_or_else(|| anyhow!("parent"))?
                        .join(LOCK_NAME);
                    fs::remove_file(&lock)?;
                    fs::write(&lock, b"")?;
                    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600))?;
                }
                Ok(())
            })
        },
    );
    require(
        matches!(
            result,
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
        ),
        "replaced lock was accepted",
    )?;
    require(
        fs::read(&fixture.selection)? == fixture.before,
        "replaced lock changed selection",
    )
}

#[test]
fn callback_failure_after_durable_publish_is_never_before_rename() -> TestResult {
    let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
    let (mut work, _execution) = budget()?;
    let result = with_locked_deployment_selection_v1(
        &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
        &mut work,
        |locked, work| {
            let _published = locked.publish_v1(work)?;
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                "coordinator failed after the actual directory fsync"
            )))
        },
    );
    // Observe the real mutation before judging the reported failure phase.
    require(
        fs::read(&fixture.selection)? == fixture.after,
        "fixture did not actually publish",
    )?;
    require(
        matches!(
            result,
            Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
        ),
        "a transaction that changed selection reported no mutation",
    )
}

#[test]
fn same_binary_three_phase_verification_fits_fixed_work_budget() -> TestResult {
    let mut executable = vec![b'#'; 512 * 1024];
    let header = b"#!/bin/sh\nexit 0\n";
    executable[..header.len()].copy_from_slice(header);
    let fixture = Fixture::with_binary(DeploymentRecoveryModeV1::RestoreForward, &executable)?;
    // Fixed caller budget, not an implementation-derived expected read count.
    let (mut work, _execution) = budget_with_bytes(3 * 1024 * 1024)?;
    let result = with_locked_deployment_selection_v1(
        &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
        &mut work,
        |locked, work| locked.publish_v1(work),
    );
    let _published = result.map_err(|error| {
        format!("512KiB same-binary recovery exceeded fixed 3MiB work budget: {error}")
    })?;
    require(
        fs::read(&fixture.selection)? == fixture.after,
        "bounded publisher changed wrong selection",
    )
}

#[test]
fn unresolved_publish_cannot_be_swallowed_or_retried_in_same_transaction() -> TestResult {
    let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
    let (mut work, _execution) = budget()?;
    let result = with_locked_deployment_selection_v1(
        &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
        &mut work,
        |locked, work| {
            let first = locked.publish_inner(work, &mut |stage| {
                if matches!(stage, PublishStage::AfterRename) {
                    return Err(anyhow!("failure before directory fsync"));
                }
                Ok(())
            });
            if !matches!(
                first,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            ) {
                return Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                    "wrong first failure"
                )));
            }
            let second = locked.publish_v1(work);
            if !matches!(
                second,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            ) {
                return Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                    "uncertain transaction was reusable"
                )));
            }
            // An erroneous coordinator swallowing the error must not turn the
            // outer transaction into success or reset its original phase.
            Ok(DeploymentSelectionPublishedV1 {
                resolution_v1: DeploymentSelectionResolutionV1::Published,
                before_sha256_v1: locked.current_v1().fingerprint_v1(),
                after_sha256_v1: locked.candidate_v1().fingerprint_v1(),
            })
        },
    );
    require(
        fs::read(&fixture.selection)? == fixture.after,
        "rename did not execute",
    )?;
    require(
        matches!(
            result,
            Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
        ),
        "unresolved publication escaped as success",
    )
}

#[test]
fn binary_hash_is_checked_at_all_three_publication_phases() -> TestResult {
    for changed_at in 0..3 {
        let fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
        let binary = fixture.selection.parent().ok_or("parent")?.join("searchd");
        if changed_at == 0 {
            fs::write(&binary, b"changed executable")?;
        }
        let mut entered = false;
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
            &mut work,
            |locked, work| {
                entered = true;
                if changed_at == 1 {
                    fs::write(&binary, b"changed executable").map_err(|error| {
                        DeploymentSelectionErrorV1::RefusedBeforeRename(error.into())
                    })?;
                }
                locked.publish_inner(work, &mut |stage| {
                    if changed_at == 2 && matches!(stage, PublishStage::AfterFileFsync) {
                        fs::write(&binary, b"changed executable")?;
                    }
                    Ok(())
                })
            },
        );
        require(
            fs::read(&fixture.selection)? == fixture.before,
            "changed binary was published",
        )?;
        require(
            entered == (changed_at != 0),
            "binary check ran at wrong phase",
        )?;
        match result {
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(cause)) => require(
                cause
                    .to_string()
                    .contains("selected binary SHA-256 changed"),
                "binary test failed for another reason",
            )?,
            _ => return Err("changed binary did not refuse before rename".into()),
        }
    }
    Ok(())
}

#[test]
fn invalid_selection_identity_reports_the_exact_rejection() -> TestResult {
    for case in [
        "nonce",
        "workspace",
        "zero_source",
        "unknown_field",
        "boolean_id",
        "writable_parent",
    ] {
        let mut fixture = Fixture::new(DeploymentRecoveryModeV1::RestoreForward)?;
        let mut wire: serde_json::Value = serde_json::from_slice(&fixture.after)?;
        let expected = match case {
            "nonce" => {
                wire["native_boot_receipt"]["root_nonce_hex"] = serde_json::json!("01".repeat(32));
                "independent Native incarnations"
            }
            "workspace" => {
                wire["native_boot_receipt"]["manifest_operation"]["workspace"]["tenant"] =
                    serde_json::to_value([2_u8; 16])?;
                "another workspace"
            }
            "zero_source" => {
                wire["native_boot_receipt"]["source_cut_digest_hex"] =
                    serde_json::json!("00".repeat(32));
                "zero identity digest"
            }
            "unknown_field" => {
                wire["ignored_authority"] = serde_json::json!(true);
                "selection fields differ"
            }
            "boolean_id" => {
                wire["native_boot_receipt"]["manifest_operation"]["producer"][0] =
                    serde_json::json!(true);
                "exactly 16 bytes"
            }
            _ => {
                fs::set_permissions(
                    fixture.selection.parent().ok_or("parent")?,
                    fs::Permissions::from_mode(0o770),
                )?;
                "not group/world writable"
            }
        };
        fixture.after = serde_json::to_vec(&wire)?;
        fixture.after.push(b'\n');
        fs::write(&fixture.candidate, &fixture.after)?;
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(DeploymentRecoveryModeV1::RestoreForward),
            &mut work,
            |locked, work| locked.publish_v1(work),
        );
        require(
            fs::read(&fixture.selection)? == fixture.before,
            "invalid identity changed selection",
        )?;
        match result {
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(cause)) => require(
                cause.to_string().contains(expected),
                "invalid identity failed for another reason",
            )?,
            _ => return Err("invalid selection identity did not refuse".into()),
        }
    }
    Ok(())
}

#[test]
fn reconciliation_revalidates_original_and_syncs_without_another_rename() -> TestResult {
    for mode in [
        DeploymentRecoveryModeV1::RollbackLegacy,
        DeploymentRecoveryModeV1::RestoreForward,
    ] {
        let fixture = Fixture::new(mode)?;
        let original = fixture.before_document("original-intent")?;
        let (mut publish_work, _publish_execution) = budget()?;
        let first = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &original,
            &mut publish_work,
            |locked, work| {
                locked.publish_inner(work, &mut |stage| {
                    if matches!(stage, PublishStage::AfterRename) {
                        return Err(anyhow!("first helper stopped after rename"));
                    }
                    Ok(())
                })
            },
        );
        require(
            matches!(first, Err(DeploymentSelectionErrorV1::PersistenceUncertain(cause))
                if cause.to_string() == "first helper stopped after rename"),
            "first helper did not stop after a real rename",
        )?;
        let original_inode = fs::metadata(&fixture.selection)?.ino();
        // Independent raw encoding differs from the fixed canonical fixture.
        let mut archived = b" \n".to_vec();
        archived.extend_from_slice(&fixture.before);
        require(
            fingerprint(&archived) != fingerprint(&fixture.before),
            "raw/canonical fixture did not differ",
        )?;
        let before = DeploymentSelectionBeforeDocumentV1 {
            bytes_v1: &archived,
            raw_sha256_v1: fingerprint(&archived),
            ..original
        };
        let (mut work, _execution) = budget()?;
        let mut file_syncs = 0;
        let mut directory_syncs = 0;
        let result = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &before,
            &mut work,
            |locked, work| {
                if locked.current_v1().native_command_v1().is_none()
                    || locked.current_v1().fingerprint_v1() != fingerprint(&fixture.before)
                {
                    return Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                        "lost original Native binding"
                    )));
                }
                locked.publish_inner(work, &mut |stage| {
                    match stage {
                        PublishStage::AfterFileFsync => file_syncs += 1,
                        PublishStage::AfterDirectoryFsync => directory_syncs += 1,
                        PublishStage::AfterRename => {
                            return Err(anyhow!("reconciliation repeated rename"));
                        }
                    }
                    Ok(())
                })
            },
        )?;
        require(
            result.resolution_v1 == DeploymentSelectionResolutionV1::Reconciled
                && result.before_sha256_v1 == fingerprint(&fixture.before)
                && result.after_sha256_v1 == fingerprint(&fixture.after),
            "reconciliation identities",
        )?;
        require(
            file_syncs == 1 && directory_syncs == 1,
            "reconciliation skipped durability",
        )?;
        require(
            fs::metadata(&fixture.selection)?.ino() == original_inode,
            "reconciliation replaced the current inode",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.after,
            "reconciliation changed payload",
        )?;
    }
    Ok(())
}

#[test]
fn independently_selected_candidate_cannot_complete_a_retained_intent() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RollbackLegacy;
    let fixture = Fixture::new(mode)?;
    let before = fixture.before_document("original-intent")?;
    for legacy_intent in [false, true] {
        // A separate publisher can select exactly the same configuration.
        let replacement = fixture.selection.with_extension("replacement");
        fs::write(&replacement, &fixture.after)?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        fs::rename(&replacement, &fixture.selection)?;
        let inode = fs::metadata(&fixture.selection)?.ino();
        let retained = DeploymentSelectionBeforeDocumentV1 {
            file_version_v1: before.file_version_v1.filter(|_| !legacy_intent),
            ..before
        };
        let (mut work, _execution) = budget()?;
        let mut entered = false;
        let result = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode), &retained, &mut work, |locked, work| {
                entered = true;
                locked.publish_v1(work)
            },
        );
        require(
            matches!(result, Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))) && !entered,
            "independently selected candidate completed a retained intent",
        )?;
        require(
            fs::metadata(&fixture.selection)?.ino() == inode
                && fs::read(&fixture.selection)? == fixture.after,
            "rejected reconciliation changed selection",
        )?;
    }
    Ok(())
}

#[test]
fn reconciliation_refuses_wrong_raw_or_canonical_before_and_third_state() -> TestResult {
    for corruption in 0..5 {
        let mode = DeploymentRecoveryModeV1::RollbackLegacy;
        let fixture = Fixture::new(mode)?;
        let mut archived = fixture.before.clone();
        if corruption == 1 {
            archived = String::from_utf8(archived)?
                .replace("ko_KR.UTF-8", "C")
                .into_bytes();
        }
        match corruption {
            0 | 1 => fs::write(&fixture.selection, &fixture.after)?,
            2 => fs::write(
                &fixture.selection,
                String::from_utf8(fixture.after.clone())?.replace("ko_KR.UTF-8", "C"),
            )?,
            3 => fs::write(&fixture.selection, b"{")?,
            _ => fs::remove_file(&fixture.selection)?,
        }
        let unchanged = fs::read(&fixture.selection).ok();
        let before = DeploymentSelectionBeforeDocumentV1 {
            operation_id_v1: "original-intent",
            bytes_v1: &archived,
            raw_sha256_v1: if corruption == 0 {
                Digest([0; 32])
            } else {
                fingerprint(&archived)
            },
            file_version_v1: None,
            retained_terminal_v1: None,
        };
        let (mut work, _execution) = budget()?;
        let mut entered = false;
        let result = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &before,
            &mut work,
            |locked, work| {
                entered = true;
                locked.publish_v1(work)
            },
        );
        require(
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            ) && !entered,
            &format!("corrupt reconciliation case {corruption} did not report uncertainty"),
        )?;
        require(
            fs::read(&fixture.selection).ok() == unchanged,
            "corrupt reconciliation changed selection",
        )?;
    }
    Ok(())
}

#[test]
fn observed_candidate_preflight_failure_stays_uncertain() -> TestResult {
    for failure in ["missing candidate", "changed config", "changed binary"] {
        for candidate_visible in [false, true] {
            let mode = DeploymentRecoveryModeV1::RestoreForward;
            let fixture = Fixture::new(mode)?;
            let original = fixture.before_document("fixed-original-intent")?;
            let active = if candidate_visible {
                let _published = fixture.publish_with_intent(mode, &original)?;
                &fixture.after
            } else {
                &fixture.before
            };
            match failure {
                "missing candidate" => fs::remove_file(&fixture.candidate)?,
                "changed config" => {
                    fs::write(fixture.selection.with_file_name("new.config.json"), b"{}")?
                }
                "changed binary" => fs::write(
                    fixture.selection.with_file_name("searchd"),
                    b"changed executable",
                )?,
                _ => return Err("unknown preflight failure fixture".into()),
            }
            let terminal = unfinished_terminal_data(&fixture, true);
            let before = DeploymentSelectionBeforeDocumentV1 {
                retained_terminal_v1: Some(&terminal),
                ..original
            };
            let (mut work, _execution) = budget()?;
            let mut entered = false;
            let result = with_locked_deployment_selection_reconciliation_v1(
                &fixture.request(mode),
                &before,
                &mut work,
                |locked, work| {
                    entered = true;
                    locked.publish_v1(work)
                },
            );
            let expected_phase = if candidate_visible {
                matches!(
                    result,
                    Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
                )
            } else {
                matches!(
                    result,
                    Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
                )
            };
            require(
                expected_phase && !entered,
                "preflight failure reported the wrong phase",
            )?;
            require(
                fs::read(&fixture.selection)? == *active,
                "preflight changed selection",
            )?;
        }
    }
    Ok(())
}

#[test]
fn reconciliation_rejects_a_raw_rewrite_with_the_same_canonical_selection() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RollbackLegacy;
    let fixture = Fixture::new(mode)?;
    let before = fixture.before_document("original-intent")?;
    let _published = fixture.publish_with_intent(mode, &before)?;
    let inode = fs::metadata(&fixture.selection)?.ino();
    let mut rewritten = b" \n".to_vec();
    rewritten.extend_from_slice(&fixture.after);
    let (mut work, _execution) = budget()?;
    let result = with_locked_deployment_selection_reconciliation_v1(
        &fixture.request(mode),
        &before,
        &mut work,
        |locked, work| {
            locked.publish_inner(work, &mut |stage| {
                if matches!(stage, PublishStage::AfterDirectoryFsync) {
                    fs::write(&fixture.selection, &rewritten)?;
                }
                Ok(())
            })
        },
    );
    require(
        matches!(result, Err(DeploymentSelectionErrorV1::PersistenceUncertain(cause))
            if cause.to_string() == "current selection file version changed"),
        "unsynced bytes were reported as reconciled",
    )?;
    require(
        fs::read(&fixture.selection)? == rewritten
            && fs::metadata(&fixture.selection)?.ino() == inode,
        "reconciliation did not preserve the observed inode",
    )
}

#[test]
fn reconciliation_without_durability_or_with_late_failure_stays_uncertain() -> TestResult {
    for late_failure in [false, true] {
        let mode = DeploymentRecoveryModeV1::RollbackLegacy;
        let fixture = Fixture::new(mode)?;
        let before = fixture.before_document("original-intent")?;
        let _published = fixture.publish_with_intent(mode, &before)?;
        let mut entered = false;
        let (mut work, _execution) = budget()?;
        let result = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &before,
            &mut work,
            |locked, work| {
                entered = true;
                if late_failure {
                    let _published = locked.publish_v1(work)?;
                    Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                        "late domain failure"
                    )))
                } else {
                    Ok(DeploymentSelectionPublishedV1 {
                        resolution_v1: DeploymentSelectionResolutionV1::Reconciled,
                        before_sha256_v1: fingerprint(&fixture.before),
                        after_sha256_v1: fingerprint(&fixture.after),
                    })
                }
            },
        );
        require(
            matches!(
                result,
                Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
            ) && entered,
            "unfinished/late reconciliation became success or pre-rename refusal",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.after,
            "uncertain reconciliation changed payload",
        )?;
    }
    Ok(())
}

#[test]
fn retained_terminal_cannot_repeat_rollback_after_a_forward_cycle() -> TestResult {
    use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverControlTerminalV1;

    let mode = DeploymentRecoveryModeV1::RollbackLegacy;
    let fixture = Fixture::new(mode)?;
    let original_before = fixture.before_document("fixed-original-intent")?;
    let (mut work, _execution) = budget()?;
    let first = fixture.publish_with_intent(mode, &original_before)?;
    require(
        fs::read(&fixture.selection)? == fixture.after,
        "first rollback payload",
    )?;
    let terminal_applied = NativeCutoverControlTerminalV1::Applied {
        operation_id: "fixed-original-intent".into(),
        nonce: Digest([7; 32]),
        before_sha256: first.before_sha256_v1,
        after_sha256: first.after_sha256_v1,
    };
    let actual_reconciliation = with_locked_deployment_selection_reconciliation_v1(
        &fixture.request(mode),
        &original_before,
        &mut work,
        |locked, work| locked.publish_v1(work),
    )?;
    require(
        actual_reconciliation.resolution_v1 == DeploymentSelectionResolutionV1::Reconciled,
        "completed reconciliation setup did not reconcile",
    )?;
    let terminal_reconciled = NativeCutoverControlTerminalV1::Reconciled {
        operation_id: "fixed-original-intent".into(),
        nonce: Digest([7; 32]),
        before_sha256: actual_reconciliation.before_sha256_v1,
        after_sha256: actual_reconciliation.after_sha256_v1,
    };
    // Exercise the reverse mechanical transition with the real publisher.
    // These callbacks still issue no producer/Source recovery authority.
    fs::write(&fixture.candidate, &fixture.before)?;
    let forward = DeploymentSelectionRequestV1 {
        selection_path_v1: &fixture.selection,
        expected_current_sha256_v1: fingerprint(&fixture.after),
        candidate_path_v1: &fixture.candidate,
        expected_candidate_sha256_v1: fingerprint(&fixture.before),
        mode_v1: DeploymentRecoveryModeV1::RestoreForward,
    };
    let _forward = with_locked_deployment_selection_v1(&forward, &mut work, |locked, work| {
        locked.publish_v1(work)
    })?;
    require(
        fs::read(&fixture.selection)? == fixture.before,
        "forward cycle payload",
    )?;
    fs::write(&fixture.candidate, &fixture.after)?;
    for version in [original_before.file_version_v1, None] {
        let before = DeploymentSelectionBeforeDocumentV1 {
            file_version_v1: version,
            ..original_before
        };
        let mut entered = false;
        let repeated = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &before,
            &mut work,
            |locked, work| {
                entered = true;
                locked.publish_v1(work)
            },
        );
        require(
            matches!(repeated, Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))) && !entered,
            "retained intent without terminal repeated rollback after forward",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.before,
            "retained intent without terminal changed current head",
        )?;
    }
    for terminal in [&terminal_applied, &terminal_reconciled] {
        let before = DeploymentSelectionBeforeDocumentV1 {
            retained_terminal_v1: Some(terminal),
            ..original_before
        };
        let mut entered = false;
        let repeated = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &before,
            &mut work,
            |locked, work| {
                entered = true;
                locked.publish_v1(work)
            },
        );
        require(
            matches!(
                repeated,
                Err(DeploymentSelectionErrorV1::RefusedBeforeRename(_))
            ) && !entered,
            "old completed intent repeated rollback after forward",
        )?;
        require(
            fs::read(&fixture.selection)? == fixture.before,
            "old terminal changed current head",
        )?;
    }
    // A later Q publication can carry the same request ID and B bytes.
    let later_before = fixture.before_document("fixed-original-intent")?;
    let _later = fixture.publish_with_intent(mode, &later_before)?;
    for terminal in [None, Some(&terminal_applied)] {
        let before = DeploymentSelectionBeforeDocumentV1 {
            retained_terminal_v1: terminal,
            ..original_before
        };
        let mut entered = false;
        let repeated = with_locked_deployment_selection_reconciliation_v1(
            &fixture.request(mode),
            &before,
            &mut work,
            |locked, work| {
                entered = true;
                locked.publish_v1(work)
            },
        );
        require(
            matches!(repeated, Err(DeploymentSelectionErrorV1::PersistenceUncertain(cause))
                if cause.to_string() == "selection was published by another recovery operation")
                && !entered,
            "later Q publication satisfied an earlier retained intent",
        )?;
    }
    Ok(())
}

// Mechanical publisher DATA only. These fixtures issue no Source permission.
fn unfinished_terminal_data(
    fixture: &Fixture,
    uncertain: bool,
) -> quanta_index_contract::native_cutover_evidence_v1::NativeCutoverControlTerminalV1 {
    use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverControlTerminalV1 as Terminal;
    if uncertain {
        Terminal::PersistenceUncertain {
            operation_id: "fixed-original-intent".into(),
            nonce: Digest([7; 32]),
            before_sha256: fingerprint(&fixture.before),
            after_sha256: Some(fingerprint(&fixture.after)),
            code: "PERSISTENCE_UNCERTAIN".into(),
            message: "first attempt did not establish completion".into(),
        }
    } else {
        Terminal::RefusedBeforeRename {
            operation_id: "fixed-original-intent".into(),
            nonce: Digest([7; 32]),
            code: "REFUSED_BEFORE_RENAME".into(),
            message: "first attempt refused".into(),
        }
    }
}

#[test]
fn unfinished_terminal_retries_original_only_through_fresh_domain_validation() -> TestResult {
    for mode in [
        DeploymentRecoveryModeV1::RollbackLegacy,
        DeploymentRecoveryModeV1::RestoreForward,
    ] {
        for uncertain in [false, true] {
            for domain_refuses in [false, true] {
                let fixture = Fixture::new(mode)?;
                let terminal = unfinished_terminal_data(&fixture, uncertain);
                let mut raw_before = b" \n".to_vec();
                raw_before.extend_from_slice(&fixture.before);
                let before = DeploymentSelectionBeforeDocumentV1 {
                    operation_id_v1: "fixed-original-intent",
                    bytes_v1: &raw_before,
                    raw_sha256_v1: fingerprint(&raw_before),
                    file_version_v1: Some(NativeCutoverEvidenceBeforeSelectionV1::file_version_v1(
                        &fs::metadata(&fixture.selection)?,
                    )),
                    retained_terminal_v1: Some(&terminal),
                };
                let inode = fs::metadata(&fixture.selection)?.ino();
                let (mut work, _execution) = budget()?;
                let mut entered = false;
                let (mut renames, mut file_syncs, mut directory_syncs) = (0, 0, 0);
                let result = with_locked_deployment_selection_reconciliation_v1(
                    &fixture.request(mode),
                    &before,
                    &mut work,
                    |locked, work| {
                        entered = true;
                        if locked.current_v1().fingerprint_v1() != fingerprint(&fixture.before) {
                            return Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                                "lost actual original"
                            )));
                        }
                        if domain_refuses {
                            return Err(DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                                "fresh domain refused"
                            )));
                        }
                        locked.publish_inner(work, &mut |stage| {
                            match stage {
                                PublishStage::AfterRename => renames += 1,
                                PublishStage::AfterFileFsync => file_syncs += 1,
                                PublishStage::AfterDirectoryFsync => directory_syncs += 1,
                            }
                            Ok(())
                        })
                    },
                );
                require(
                    entered,
                    "unfinished terminal prevented fresh domain validation",
                )?;
                if domain_refuses {
                    match result {
                        Err(DeploymentSelectionErrorV1::RefusedBeforeRename(error)) => require(
                            error.to_string().contains("fresh domain refused"),
                            "retry failed for another reason",
                        )?,
                        _ => return Err("unfinished terminal bypassed fresh domain refusal".into()),
                    }
                    require(
                        renames == 0 && file_syncs == 0 && directory_syncs == 0,
                        "refused domain reached the publisher",
                    )?;
                    require(
                        fs::read(&fixture.selection)? == fixture.before
                            && fs::metadata(&fixture.selection)?.ino() == inode,
                        "refused retry changed the actual original",
                    )?;
                } else {
                    let published = result?;
                    require(
                        published.resolution_v1 == DeploymentSelectionResolutionV1::Published
                            && published.before_sha256_v1 == fingerprint(&fixture.before)
                            && published.after_sha256_v1 == fingerprint(&fixture.after),
                        "unfinished retry did not report a new publication",
                    )?;
                    require(
                        renames == 1 && file_syncs == 1 && directory_syncs == 1,
                        "unfinished retry skipped or repeated publication durability",
                    )?;
                    require(
                        fs::read(&fixture.selection)? == fixture.after
                            && fs::metadata(&fixture.selection)?.ino() != inode,
                        "unfinished retry did not replace the actual original",
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[test]
fn unfinished_terminal_reconciles_actual_candidate_without_another_rename() -> TestResult {
    for mode in [
        DeploymentRecoveryModeV1::RollbackLegacy,
        DeploymentRecoveryModeV1::RestoreForward,
    ] {
        for uncertain in [false, true] {
            let fixture = Fixture::new(mode)?;
            let original = fixture.before_document("fixed-original-intent")?;
            let _published = fixture.publish_with_intent(mode, &original)?;
            let inode = fs::metadata(&fixture.selection)?.ino();
            let terminal = unfinished_terminal_data(&fixture, uncertain);
            let before = DeploymentSelectionBeforeDocumentV1 {
                retained_terminal_v1: Some(&terminal),
                ..original
            };
            let (mut work, _execution) = budget()?;
            let mut entered = false;
            let (mut file_syncs, mut directory_syncs) = (0, 0);
            let published = with_locked_deployment_selection_reconciliation_v1(
                &fixture.request(mode),
                &before,
                &mut work,
                |locked, work| {
                    entered = true;
                    locked.publish_inner(work, &mut |stage| {
                        match stage {
                            PublishStage::AfterFileFsync => file_syncs += 1,
                            PublishStage::AfterDirectoryFsync => directory_syncs += 1,
                            PublishStage::AfterRename => {
                                return Err(anyhow!("candidate reconciliation repeated rename"));
                            }
                        }
                        Ok(())
                    })
                },
            )?;
            require(
                entered && published.resolution_v1 == DeploymentSelectionResolutionV1::Reconciled,
                "unfinished candidate did not run fresh reconciliation",
            )?;
            require(
                file_syncs == 1 && directory_syncs == 1,
                "unfinished candidate skipped durability",
            )?;
            require(
                fs::read(&fixture.selection)? == fixture.after
                    && fs::metadata(&fixture.selection)?.ino() == inode,
                "unfinished candidate changed bytes or inode",
            )?;
        }
    }
    Ok(())
}

#[test]
fn live_custody_is_checked_at_the_rename_boundary_and_after_sync() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    for fail_after_rename in [false, true] {
        let fixture = Fixture::new(mode)?;
        let (mut work, _execution) = budget()?;
        let mut checkpoint_count = 0;
        let result = with_locked_deployment_selection_v1(
            &fixture.request(mode),
            &mut work,
            |locked, budget| {
                locked.publish_with_custody_v1(budget, &mut |_budget| {
                    checkpoint_count = u8::checked_add(checkpoint_count, 1)
                        .ok_or_else(|| anyhow!("checkpoint count overflow"))?;
                    let active = fs::read(&fixture.selection)?;
                    if active == fixture.before {
                        // The file must already be written and synced at the
                        // final pre-rename checkpoint, not merely preflighted.
                        let parent = fixture
                            .selection
                            .parent()
                            .ok_or_else(|| anyhow!("fixture parent absent"))?;
                        let mut temporary = false;
                        for entry in fs::read_dir(parent)? {
                            let entry = entry?;
                            if entry.file_name().to_string_lossy().contains(".tmp-") {
                                anyhow::ensure!(fs::read(entry.path())? == fixture.after);
                                temporary = true;
                            }
                        }
                        anyhow::ensure!(temporary, "checkpoint preceded temporary publication");
                        anyhow::ensure!(fail_after_rename, "custody lost before rename");
                    } else {
                        anyhow::ensure!(active == fixture.after, "published unexpected bytes");
                        anyhow::bail!("custody lost after rename");
                    }
                    Ok(())
                })
            },
        );
        require(
            checkpoint_count == if fail_after_rename { 2 } else { 1 },
            "unexpected checkpoint count",
        )?;
        match result {
            Err(DeploymentSelectionErrorV1::RefusedBeforeRename(error)) if !fail_after_rename => {
                require(
                    error.to_string() == "custody lost before rename",
                    "wrong pre-rename refusal",
                )?;
                require(
                    fs::read(&fixture.selection)? == fixture.before,
                    "refusal changed selection",
                )?;
            }
            Err(DeploymentSelectionErrorV1::PersistenceUncertain(error)) if fail_after_rename => {
                require(
                    error.to_string() == "custody lost after rename",
                    "wrong post-rename refusal",
                )?;
                require(
                    fs::read(&fixture.selection)? == fixture.after,
                    "late refusal did not execute rename",
                )?;
            }
            _ => return Err("custody failure was reported at the wrong publication phase".into()),
        }
    }
    Ok(())
}

#[test]
fn late_custody_failure_rejects_a_previous_success_for_the_same_documents() -> TestResult {
    let mode = DeploymentRecoveryModeV1::RestoreForward;
    let fixture = Fixture::new(mode)?;
    let (mut work, _execution) = budget()?;
    let previous = with_locked_deployment_selection_v1(
        &fixture.request(mode),
        &mut work,
        LockedDeploymentSelectionV1::publish_v1,
    )?;
    fs::write(&fixture.selection, &fixture.before)?;
    let (mut work, _execution) = budget()?;
    let result =
        with_locked_deployment_selection_v1(&fixture.request(mode), &mut work, |locked, budget| {
            let failed = locked.publish_with_custody_v1(budget, &mut |_| {
                anyhow::ensure!(
                    fs::read(&fixture.selection)? == fixture.before,
                    "custody lost"
                );
                Ok(())
            });
            require(
                matches!(
                    failed,
                    Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
                ),
                "second publication did not lose custody after rename",
            )
            .map_err(|error| locked.classify(anyhow!(error.to_string())))?;
            Ok(previous)
        });
    require(
        fs::read(&fixture.selection)? == fixture.after,
        "rename did not occur",
    )?;
    require(
        matches!(
            result,
            Err(DeploymentSelectionErrorV1::PersistenceUncertain(_))
        ),
        "a previous success hid the current late custody failure",
    )
}
