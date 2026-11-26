//! Installed producer -> retained descriptors -> one Q selection transaction.
//! Only the real signed resident's complete Original inventory is consumed.

mod control;
mod custody;
mod facts;
mod peer;
mod trust;

#[cfg(test)]
pub(crate) use control::settle_published_v1;

use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::native_cutover_evidence_v1 as wire;
use quanta_index_core::{
    RequestBudgetV1,
    index::{MemoryLifetime, WorkBudget, WorkUsage},
};
use quanta_index_searchd::cli::NativeRecoveryCommandV1;
use std::{fmt::Write as _, os::unix::net::UnixDatagram, path::Path};

use crate::native_product::{FundedInstalledHistoryV1, RestoreForwardHistoryV1};
use crate::{
    deployment_selection::{
        DeploymentRecoveryModeV1, DeploymentSelectionBeforeDocumentV1,
        DeploymentSelectionErrorV1 as Error, DeploymentSelectionPublishedV1,
        DeploymentSelectionRequestV1, DeploymentSelectionResolutionV1,
        with_locked_deployment_selection_reconciliation_v1, with_locked_published_recovery_v1,
    },
    native_product::{RestoreForwardSourceRefusalV1, with_native_recovery_control_v1},
};

/// Process entry. `0` requires an Applied or Reconciled terminal from this helper;
/// `2` refuses before rename, `3` reports possible publication. The resident
/// still authenticates the terminal and inspects selection after uncertainty.
pub fn run(command: &NativeRecoveryCommandV1, request: RequestBudgetV1) -> Result<u8> {
    // Claim optional FD7 before FD3 duplication can occupy its vacant number.
    // Keep a malformed present mapping as a bound refusal after Begin arrives.
    let history_fd7 = if command.settle_published_only {
        Ok(None)
    } else {
        quanta_explicit_fd_process::try_duplicate_optional_inherited_regular_file_v1(7)
    };
    command.validate()?;
    let _inherited_umask = quanta_index_searchd::app::harden_umask();
    let socket = quanta_explicit_fd_process::try_duplicate_inherited_unix_datagram_v1(3)?;
    let (begin, bootstrap) = control::receive_begin_v1(&socket, &request)?;
    let request = match validate_begin_v1(command, &begin)
        .and_then(|()| control::original_deadline_v1(begin.deadline_ns))
    {
        Ok(deadline) => request.capped_at(deadline),
        Err(error) => {
            let error = if command.settle_published_only {
                Error::PersistenceUncertain(error)
            } else {
                Error::RefusedBeforeRename(error)
            };
            return send_outcome(&socket, &begin, Err(error));
        }
    };
    if command.settle_published_only {
        return run_published_after_begin_v1(command, request, &socket, &begin, bootstrap);
    }
    let history_fd7 = match history_fd7 {
        Ok(history) => history,
        Err(error) => {
            return send_outcome(
                &socket,
                &begin,
                Err(Error::RefusedBeforeRename(error.into())),
            );
        }
    };
    run_after_begin(command, request, &socket, &begin, bootstrap, history_fd7)
}

fn run_after_begin(
    command: &NativeRecoveryCommandV1,
    request: RequestBudgetV1,
    socket: &UnixDatagram,
    begin: &wire::NativeCutoverControlBeginV1,
    bootstrap: WorkUsage,
    history_fd7: Option<std::fs::File>,
) -> Result<u8> {
    let before = Error::RefusedBeforeRename;
    if let Err(error) = request.checkpoint("prepare native recovery Control") {
        return send_outcome(socket, begin, Err(before(error.into())));
    }
    let prepared = with_native_recovery_control_v1(
        &command.config_path,
        command.config_digest,
        request,
        move |context, budget| {
            let control_memory = budget
                .reserve_memory(128 * 1024, MemoryLifetime::Temporary)?
                .ok_or_else(|| anyhow!("recovery controls require original admitted memory"))?;
            budget.charge(bootstrap)?;
            let custody = custody::InheritedCustodyV1::import_v1(budget, history_fd7)?;
            let mut installed_history = None;
            let result = (|| -> std::result::Result<DeploymentSelectionPublishedV1, Error> {
                let parent = peer::observe_parent_v1(socket, budget).map_err(before)?;
                control::with_cancel_monitor_v1(socket, begin, budget, |budget, signal| {
                    trust::with_installed_parent_v1(&parent, command, budget, |budget| {
                        let evidence = custody
                            .read_evidence_v1(begin.evidence_length, begin.evidence_sha256, budget)
                            .map_err(before)?;
                        let evidence = &evidence.facts;
                        validate_bindings_v1(command, begin, evidence).map_err(before)?;
                        let sdk_path = Path::new(&evidence.header.sdk_lock_path);
                        let mut checkpoint = |budget: &mut WorkBudget| {
                            if signal.load(std::sync::atomic::Ordering::Acquire) {
                                budget.request().cancel_handle().cancel();
                            }
                            custody.revalidate_locks_v1(evidence, sdk_path, budget)
                        };
                        let mut require_history_fence = |budget: &mut WorkBudget| {
                            custody.require_history_writer_fence_v1(
                                Path::new(&evidence.header.writer_state_root),
                                budget,
                            )
                        };
                        checkpoint(budget).map_err(before)?;
                        let selection = selection_request_v1(command);
                        let retained_terminal =
                            decode_retained_terminal_v1(evidence, begin, budget).map_err(before)?;
                        let original_before = DeploymentSelectionBeforeDocumentV1 {
                            operation_id_v1: &begin.operation_id,
                            bytes_v1: &evidence.before_selection.document.0,
                            raw_sha256_v1: evidence.before_selection.raw_sha256,
                            file_version_v1: evidence.before_selection.file_version,
                            retained_terminal_v1: retained_terminal.as_ref(),
                        };
                        with_locked_deployment_selection_reconciliation_v1(
                            &selection,
                            &original_before,
                            budget,
                            |locked, budget| {
                                let joined = facts::join_source_and_host_v1(evidence, budget)
                                    .map_err(before)?;
                                facts::require_fresh_source_parity_v1(&joined, budget)
                                    .map_err(before)?;
                                match command.mode {
                                    wire::NativeCutoverRecoveryModeV1::RollbackLegacy => {
                                        let helper = locked.current_v1().binary_sha256_v1();
                                        let _running_image =
                                            peer::retain_running_image_v1(helper, budget)
                                                .map_err(before)?;
                                        locked
                                            .bind_recovery_witness_v1(
                                                begin,
                                                command.config_digest,
                                                helper,
                                            )
                                            .map_err(before)?;
                                        let native = locked
                                            .current_v1()
                                            .native_command_v1()
                                            .cloned()
                                            .ok_or_else(|| {
                                                before(anyhow!(
                                                    "rollback current selection is not Native"
                                                ))
                                            })?;
                                        context.publish_rollback_v1(
                                            &native,
                                            evidence,
                                            &joined,
                                            locked,
                                            budget,
                                            &mut checkpoint,
                                        )
                                    }
                                    wire::NativeCutoverRecoveryModeV1::RestoreForward => {
                                        let candidate = locked.candidate_v1();
                                        let helper = candidate.binary_sha256_v1();
                                        let _running_image =
                                            peer::retain_running_image_v1(helper, budget)
                                                .map_err(before)?;
                                        let native = candidate
                                            .native_command_v1()
                                            .cloned()
                                            .ok_or_else(|| {
                                                before(anyhow!(
                                                    "restore-forward candidate is not Native"
                                                ))
                                            })?;
                                        locked
                                            .bind_recovery_witness_v1(
                                                begin,
                                                command.config_digest,
                                                helper,
                                            )
                                            .map_err(before)?;
                                        context.publish_restore_forward_v1(
                                            &native,
                                            evidence,
                                            &joined,
                                            locked,
                                            budget,
                                            &mut checkpoint,
                                            &mut require_history_fence,
                                            RestoreForwardHistoryV1 {
                                                retained_terminal: retained_terminal.as_ref(),
                                                selected: &mut installed_history,
                                            },
                                        )
                                    }
                                }
                            },
                        )
                    })
                })
            })();
            // Transport errors must propagate without sending a second
            // terminal. Publication may already have happened.
            let sent = send_outcome_with_history(socket, begin, result, installed_history);
            drop(control_memory);
            Ok(sent)
        },
    );
    match prepared {
        Ok(sent) => sent,
        Err(error) => send_outcome(socket, begin, Err(before(error))),
    }
}

fn selection_request_v1(command: &NativeRecoveryCommandV1) -> DeploymentSelectionRequestV1<'_> {
    DeploymentSelectionRequestV1 {
        selection_path_v1: &command.selection_path,
        expected_current_sha256_v1: command.expected_current_sha256,
        candidate_path_v1: &command.candidate_path,
        expected_candidate_sha256_v1: command.expected_candidate_sha256,
        mode_v1: match command.mode {
            wire::NativeCutoverRecoveryModeV1::RollbackLegacy => {
                DeploymentRecoveryModeV1::RollbackLegacy
            }
            wire::NativeCutoverRecoveryModeV1::RestoreForward => {
                DeploymentRecoveryModeV1::RestoreForward
            }
        },
    }
}

fn run_published_after_begin_v1(
    command: &NativeRecoveryCommandV1,
    request: RequestBudgetV1,
    socket: &UnixDatagram,
    begin: &wire::NativeCutoverControlBeginV1,
    bootstrap: WorkUsage,
) -> Result<u8> {
    let uncertain = Error::PersistenceUncertain;
    let prepared = with_native_recovery_control_v1(
        &command.config_path,
        command.config_digest,
        request,
        |context, budget| {
            let _control_memory = budget
                .reserve_memory(128 * 1024, MemoryLifetime::Temporary)?
                .ok_or_else(|| anyhow!("published recovery requires admitted Control memory"))?;
            budget.charge(bootstrap)?;
            let parent = peer::observe_parent_v1(socket, budget)?;
            let mut installed_history = None;
            let result =
                control::with_cancel_monitor_v1(socket, begin, budget, |budget, signal| {
                    trust::with_installed_parent_v1(&parent, command, budget, |budget| {
                        let (before, _memory) = custody::read_published_before_v1(
                            begin.evidence_length,
                            begin.evidence_sha256,
                            budget,
                        )
                        .map_err(uncertain)?;
                        let original = DeploymentSelectionBeforeDocumentV1 {
                            operation_id_v1: &begin.operation_id,
                            bytes_v1: &before.document.0,
                            raw_sha256_v1: before.raw_sha256,
                            file_version_v1: before.file_version,
                            retained_terminal_v1: None,
                        };
                        let settled = with_locked_published_recovery_v1(
                            &selection_request_v1(command),
                            &original,
                            command.config_digest,
                            budget,
                            |locked, helper_sha256, witness, budget, sync| {
                                let _running_image =
                                    peer::retain_running_image_v1(helper_sha256, budget)?;
                                context.with_verified_settlement_root_v1(
                                    locked,
                                    witness,
                                    budget,
                                    |budget| {
                                        sync(budget, &mut |budget| {
                                            if signal.load(std::sync::atomic::Ordering::Acquire) {
                                                budget.request().cancel_handle().cancel();
                                            }
                                            Ok(budget.checkpoint()?)
                                        })
                                    },
                                )
                            },
                        )?;
                        installed_history = settled.history.map(|value| FundedInstalledHistoryV1 {
                            value,
                            _memory: Some(settled.memory),
                        });
                        Ok(settled.publication)
                    })
                })
                .map_err(|error| match error {
                    Error::RefusedBeforeRename(cause) => uncertain(cause),
                    error => error,
                });
            Ok(send_outcome_with_history(
                socket,
                begin,
                result,
                installed_history,
            ))
        },
    );
    match prepared {
        Ok(sent) => sent,
        Err(error) => send_outcome(socket, begin, Err(uncertain(error))),
    }
}

fn send_outcome(
    socket: &UnixDatagram,
    begin: &wire::NativeCutoverControlBeginV1,
    result: std::result::Result<DeploymentSelectionPublishedV1, Error>,
) -> Result<u8> {
    send_outcome_with_history(socket, begin, result, None)
}

fn send_outcome_with_history(
    socket: &UnixDatagram,
    begin: &wire::NativeCutoverControlBeginV1,
    result: std::result::Result<DeploymentSelectionPublishedV1, Error>,
    installed_history: Option<FundedInstalledHistoryV1>,
) -> Result<u8> {
    let (history, _memory) =
        installed_history.map_or((None, None), |proof| (Some(proof.value), proof._memory));
    let (terminal, status) = terminal_v1(begin, &result, history);
    control::send_terminal_v1(socket, &terminal)?;
    Ok(status)
}

fn validate_begin_v1(
    command: &NativeRecoveryCommandV1,
    begin: &wire::NativeCutoverControlBeginV1,
) -> Result<()> {
    begin.validate_v1()?;
    ensure!(
        begin.mode == command.mode
            && begin.current_selection_sha256 == command.expected_current_sha256
            && begin.candidate_selection_sha256 == command.expected_candidate_sha256,
        "recovery Begin differs from the explicit expected identities"
    );
    Ok(())
}

fn validate_bindings_v1(
    command: &NativeRecoveryCommandV1,
    begin: &wire::NativeCutoverControlBeginV1,
    evidence: &wire::NativeCutoverEvidenceV1,
) -> Result<()> {
    let header = &evidence.header;
    ensure!(
        header.current_selection_sha256 == begin.current_selection_sha256
            && header.candidate_selection_sha256 == begin.candidate_selection_sha256
            && Path::new(&header.config_path) == command.config_path
            && header.config_sha256 == command.config_digest,
        "Original evidence differs from the recovery selection/configuration"
    );
    for path in [
        &header.canonical_workspace_root,
        &header.writer_state_root,
        &header.canonical_snapshot_namespace,
        &header.sdk_lock_path,
        &header.host.namespace_root,
    ] {
        ensure!(
            Path::new(path).is_absolute(),
            "Original evidence has an unbound relative namespace"
        );
    }
    ensure!(
        !header.repo_id.is_empty() && !header.contract_key.is_empty(),
        "Original evidence has no repository/contract identity"
    );
    Ok(())
}

fn decode_retained_terminal_v1(
    evidence: &wire::NativeCutoverEvidenceV1,
    begin: &wire::NativeCutoverControlBeginV1,
    budget: &mut WorkBudget,
) -> Result<Option<wire::NativeCutoverControlTerminalV1>> {
    let Some(bytes) = &evidence.retained_terminal else {
        return Ok(None);
    };
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes.0.len())?,
        ..WorkUsage::default()
    })?;
    let terminal = wire::decode_native_cutover_control_terminal_v1(&bytes.0)?;
    use wire::NativeCutoverControlTerminalV1 as Terminal;
    let operation = match &terminal {
        Terminal::Applied {
            operation_id,
            before_sha256,
            after_sha256,
            ..
        }
        | Terminal::Reconciled {
            operation_id,
            before_sha256,
            after_sha256,
            ..
        }
        | Terminal::AppliedHistory {
            operation_id,
            before_sha256,
            after_sha256,
            ..
        }
        | Terminal::ReconciledHistory {
            operation_id,
            before_sha256,
            after_sha256,
            ..
        } => {
            ensure!(
                *before_sha256 == begin.current_selection_sha256
                    && *after_sha256 == begin.candidate_selection_sha256,
                "retained terminal describes another selection transition"
            );
            operation_id
        }
        Terminal::PersistenceUncertain {
            operation_id,
            before_sha256,
            after_sha256,
            ..
        } => {
            ensure!(
                *before_sha256 == begin.current_selection_sha256
                    && after_sha256.is_none_or(|digest| digest == begin.candidate_selection_sha256),
                "retained uncertainty describes another selection transition"
            );
            operation_id
        }
        Terminal::RefusedBeforeRename { operation_id, .. } => operation_id,
    };
    ensure!(
        operation == &begin.operation_id,
        "retained terminal belongs to another operation"
    );
    ensure!(
        terminal.installed_history_v1().is_none()
            || begin.mode == wire::NativeCutoverRecoveryModeV1::RestoreForward,
        "retained History witness requires restore-forward"
    );
    // The first response's nonce is historical. This attempt's Begin nonce
    // authenticates its new terminal; neither nonce is a publication permit.
    budget.checkpoint()?;
    Ok(Some(terminal))
}

fn terminal_v1(
    begin: &wire::NativeCutoverControlBeginV1,
    result: &std::result::Result<DeploymentSelectionPublishedV1, Error>,
    history: Option<wire::NativeCutoverInstalledHistoryV1>,
) -> (wire::NativeCutoverControlTerminalV1, u8) {
    use wire::NativeCutoverControlTerminalV1 as Terminal;
    match result {
        Ok(published) => (
            match (published.resolution_v1(), history) {
                (DeploymentSelectionResolutionV1::Published, None) => Terminal::Applied {
                    operation_id: begin.operation_id.clone(),
                    nonce: begin.nonce,
                    before_sha256: published.before_sha256_v1(),
                    after_sha256: published.after_sha256_v1(),
                },
                (DeploymentSelectionResolutionV1::Reconciled, None) => Terminal::Reconciled {
                    operation_id: begin.operation_id.clone(),
                    nonce: begin.nonce,
                    before_sha256: published.before_sha256_v1(),
                    after_sha256: published.after_sha256_v1(),
                },
                (DeploymentSelectionResolutionV1::Published, Some(history)) => {
                    Terminal::AppliedHistory {
                        operation_id: begin.operation_id.clone(),
                        nonce: begin.nonce,
                        before_sha256: published.before_sha256_v1(),
                        after_sha256: published.after_sha256_v1(),
                        history,
                    }
                }
                (DeploymentSelectionResolutionV1::Reconciled, Some(history)) => {
                    Terminal::ReconciledHistory {
                        operation_id: begin.operation_id.clone(),
                        nonce: begin.nonce,
                        before_sha256: published.before_sha256_v1(),
                        after_sha256: published.after_sha256_v1(),
                        history,
                    }
                }
            },
            0,
        ),
        Err(Error::RefusedBeforeRename(error)) => (
            Terminal::RefusedBeforeRename {
                operation_id: begin.operation_id.clone(),
                nonce: begin.nonce,
                code: if error.is::<facts::FreshSourceParityRefusalV1>() {
                    "FRESH_SOURCE_PARITY_REFUSED"
                } else if let Some(reason) = error.downcast_ref::<facts::RetainedHostRefusalV1>() {
                    match reason {
                        facts::RetainedHostRefusalV1::PendingRecoveryOrdinal => {
                            "PENDING_RETAINED_RECOVERY_ORDINAL"
                        }
                        _ => "RETAINED_HOST_EVIDENCE_REFUSED",
                    }
                } else if let Some(reason) = error.downcast_ref::<RestoreForwardSourceRefusalV1>() {
                    match reason {
                        RestoreForwardSourceRefusalV1::MissingCurrentLegacyPublication => {
                            "MISSING_CURRENT_LEGACY_PUBLICATION"
                        }
                        RestoreForwardSourceRefusalV1::LegacyHistoryBootstrapRequired => {
                            "LEGACY_HISTORY_BOOTSTRAP_REQUIRED"
                        }
                        RestoreForwardSourceRefusalV1::CandidateIsNotFreshFull => {
                            "CANDIDATE_IS_NOT_FRESH_FULL"
                        }
                        RestoreForwardSourceRefusalV1::MissingPreviousNativeSourceAssociation => {
                            "MISSING_PREVIOUS_NATIVE_SOURCE_ASSOCIATION"
                        }
                        RestoreForwardSourceRefusalV1::MissingPreviousNativeSourceCoverage => {
                            "MISSING_PREVIOUS_NATIVE_SOURCE_COVERAGE"
                        }
                        RestoreForwardSourceRefusalV1::PreviousNativeSourceCoverageMismatch => {
                            "PREVIOUS_NATIVE_SOURCE_COVERAGE_MISMATCH"
                        }
                        RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable => {
                            "NATIVE_REPOMAP_VERIFICATION_UNAVAILABLE"
                        }
                    }
                } else {
                    "REFUSED_BEFORE_RENAME"
                }
                .into(),
                message: bounded_message_v1(error),
            },
            2,
        ),
        Err(Error::PersistenceUncertain(error)) => (
            Terminal::PersistenceUncertain {
                operation_id: begin.operation_id.clone(),
                nonce: begin.nonce,
                before_sha256: begin.current_selection_sha256,
                after_sha256: None,
                code: "PERSISTENCE_UNCERTAIN".into(),
                message: bounded_message_v1(error),
            },
            3,
        ),
    }
}

fn bounded_message_v1(error: &anyhow::Error) -> String {
    struct Text {
        bytes: [u8; 4096],
        length: usize,
    }
    impl std::fmt::Write for Text {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            let available = self.bytes.len() - self.length;
            let mut count = value.len().min(available);
            while !value.is_char_boundary(count) {
                count -= 1;
            }
            self.bytes[self.length..self.length + count]
                .copy_from_slice(&value.as_bytes()[..count]);
            self.length += count;
            Ok(())
        }
    }
    let mut text = Text {
        bytes: [0; 4096],
        length: 0,
    };
    let _formatted = write!(&mut text, "{error}");
    String::from_utf8_lossy(&text.bytes[..text.length]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::Digest;
    use sha2::{Digest as _, Sha256};
    use std::{fs, time::Duration};

    #[test]
    fn startup_config_failure_returns_bound_refusal() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config = root.path().join("native.json");
        let selection = root.path().join("selection.json");
        fs::write(&config, b"{\"unrecognized\":true}")?;
        fs::write(&selection, b"selection fixture")?;
        let (resident, helper) = UnixDatagram::pair()?;
        resident.set_read_timeout(Some(Duration::from_secs(1)))?;
        let expected = wire::NativeCutoverControlBeginV1 {
            format_version: wire::NativeCutoverControlBeginV1::FORMAT_VERSION_V1,
            operation_id: "config-refusal".into(),
            mode: wire::NativeCutoverRecoveryModeV1::RestoreForward,
            current_selection_sha256: Digest([1; 32]),
            candidate_selection_sha256: Digest([2; 32]),
            deadline_ns: 1,
            evidence_length: 1,
            evidence_sha256: Digest([3; 32]),
            nonce: Digest([4; 32]),
        };
        let mut frame = Vec::new();
        ciborium::ser::into_writer(&expected, &mut frame)?;
        ensure!(resident.send(&frame)? == frame.len());
        let request = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let (begin, work) = control::receive_begin_v1(&helper, &request)?;
        ensure!(begin == expected);
        let command = NativeRecoveryCommandV1 {
            selection_path: selection.clone(),
            expected_current_sha256: begin.current_selection_sha256,
            candidate_path: root.path().join("candidate.json"),
            expected_candidate_sha256: begin.candidate_selection_sha256,
            mode: begin.mode,
            config_path: config,
            config_digest: Digest(Sha256::digest(b"{}").into()),
            parent_executable: root.path().join("resident"),
            parent_executable_sha256: Digest([5; 32]),
            settle_published_only: false,
        };
        validate_begin_v1(&command, &begin)?;
        ensure!(run_after_begin(&command, request, &helper, &begin, work, None)? == 2);
        let mut response = [0; wire::MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1];
        let size = resident.recv(&mut response)?;
        let terminal = wire::decode_native_cutover_control_terminal_v1(
            response
                .get(..size)
                .ok_or_else(|| anyhow!("response exceeds frame"))?,
        )?;
        ensure!(matches!(
            terminal,
            wire::NativeCutoverControlTerminalV1::RefusedBeforeRename {
                operation_id, nonce, code, message,
            } if operation_id == expected.operation_id
                && nonce == expected.nonce
                && code == "REFUSED_BEFORE_RENAME"
                && message == quanta_index_core::index::IndexError::DigestConflict.to_string()
        ));
        ensure!(fs::read(&selection)? == b"selection fixture");
        Ok(())
    }
}
