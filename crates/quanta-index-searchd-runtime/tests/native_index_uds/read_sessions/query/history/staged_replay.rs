use std::{
    io::Write,
    os::unix::{net::UnixStream, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::Command,
};

use quanta_index_contract::native_protocol::{
    NativeIndexRequestEnvelope, SourceStageReplayReceiptV1,
};

use super::*;

const CHILD_ROOT: &str = "QIDX_STAGED_HISTORY_CHILD_ROOT";
const CHILD_READY: &str = "QIDX_STAGED_HISTORY_CHILD_READY";
const TEST_NAME: &str = "read_sessions::query::history::staged_source_and_history_bootstrap_publishes_changed_profile_and_reopens";

pub(super) fn child_root() -> Option<PathBuf> {
    std::env::var_os(CHILD_ROOT).map(PathBuf::from)
}

pub(super) fn run_child(root: &Path) -> TestResult {
    let ready = std::env::var_os(CHILD_READY).ok_or("missing staged History readiness path")?;
    let service = history_service(root, &root.join("native.sock"))?;
    std::fs::write(ready, b"ready")?;
    service.run(Duration::from_millis(5))?;
    Ok(())
}

pub(super) fn start_child(root: &Path) -> TestResult<ForcedExitChild> {
    let readiness = tempfile::tempdir()?;
    let ready = readiness.path().join("ready");
    let mut child = ForcedExitChild(
        Command::new(std::env::current_exe()?)
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ROOT, root)
            .env(CHILD_READY, &ready)
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        if let Some(status) = child.0.try_wait()? {
            return Err(format!("staged History child exited before bind: {status}").into());
        }
        if Instant::now() >= deadline {
            return Err("staged History child bind timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(child)
}

pub(super) fn kill_child(mut child: ForcedExitChild) -> TestResult {
    child.0.kill()?;
    let status = child.0.wait()?;
    assert!(
        status.signal().is_some(),
        "expected signal exit, got {status}"
    );
    Ok(())
}

pub(super) fn publish_with_unread_reply<P: AsRef<Path>>(
    client: &NativeUdsClient<P>,
    socket: &Path,
    request: &UpdateRequest,
    receipt: SourceStageReplayReceiptV1,
) -> TestResult<(ViewRef, NativeIndexRequest)> {
    assert_eq!(
        client.status(request.operation, &budget())?,
        OperationStatus::Unknown,
    );
    assert_eq!(
        client.request(
            NativeIndexRequest::ReadSealedMutationsForReplayV1 {
                operation: request.operation,
                correlation: receipt.context.correlation,
            },
            &budget(),
        )?,
        NativeIndexResponse::SealedMutationsForReplayV1(Some(receipt)),
    );

    let staged = NativeIndexRequest::SubmitStagedSourceV1 {
        request: request.clone(),
        receipt,
    };
    let mut unread_reply = UnixStream::connect(socket)?;
    unread_reply.set_write_timeout(Some(Duration::from_secs(10)))?;
    unread_reply.write_all(&quanta_index_ipc::encode_request(
        &NativeIndexRequestEnvelope {
            request_id: 70_003,
            payload: staged.clone(),
        },
    )?)?;
    // Keep the submitting peer live until Q publishes, then discard its
    // unread reply. Closing before publication would test cancellation.
    let observation = RequestBudgetV1::for_duration(Duration::from_secs(60));
    let published = loop {
        match client.status(request.operation, &observation)? {
            OperationStatus::Published(view) => break view,
            OperationStatus::Pending(attempt) => assert_eq!(attempt.operation, request.operation),
            OperationStatus::Unknown => {}
            OperationStatus::Refused(refusal) => {
                return Err(format!("staged Source publication refused: {refusal:?}").into());
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(unread_reply);

    Ok((published, staged))
}

pub(super) fn retry_published<P: AsRef<Path>>(
    client: &NativeUdsClient<P>,
    staged: NativeIndexRequest,
    operation: OperationKey,
    published: ViewRef,
) -> TestResult {
    let NativeIndexResponse::Submitted(retry) = client.request(staged.clone(), &budget())? else {
        return Err("same-operation staged retry did not return its terminal receipt".into());
    };
    assert_eq!(retry.operation, operation);
    assert_eq!(
        retry.payload,
        SubmitOutcome::Terminal(OperationStatus::Published(published)),
    );
    let mut changed = staged;
    let NativeIndexRequest::SubmitStagedSourceV1 { request, .. } = &mut changed else {
        return Err("expected staged Source request for conflicting replay".into());
    };
    let UpdateKind::SourceHistoryBootstrap(transition) = &mut request.kind else {
        return Err("expected Source+History request for conflicting replay".into());
    };
    transition.history.next.snapshot_digest.0[0] ^= 1;
    assert!(matches!(
        client.request(changed, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    Ok(())
}
