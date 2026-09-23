//! P09: real supervised daemon and control UDS process-readiness proof.

use std::error::Error;
use std::time::Duration;

use quanta_index_contract::{ProcessReadinessReasonV1, ProcessReadinessV1};
use quanta_index_searchd_harness::E2eRuntime;

use crate::fail_closed_wait::{RealTicker, WaitError, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

fn wait_until_ready(rt: &mut E2eRuntime) -> Result<ProcessReadinessV1, Box<dyn Error>> {
    match wait_for(
        &RealTicker::new(),
        Duration::from_secs(5),
        Duration::from_millis(10),
        "supervised process readiness",
        || rt.process_readiness(),
        |report| report.ready,
        |_| false,
    ) {
        Ok(report) => Ok(report),
        Err(WaitError::Terminal(error)) => Err(error.into()),
        Err(WaitError::Timeout(timeout)) => Err(Box::new(timeout)),
    }
}

#[test]
fn zero_active_repositories_are_ready_only_with_all_supervised_children() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let report = wait_until_ready(&mut rt)?;
    assert_eq!(report.active_repositories, 0);
    assert_eq!(report.active_candidate_integrity, None);
    assert!(report.components.query_plane);
    assert!(report.components.control_plane);
    assert!(report.components.ingest_plane);
    assert!(report.components.maintenance_heartbeat);
    assert!(report.not_ready_reasons.is_empty());
    Ok(())
}

#[test]
fn active_repository_requires_physical_candidate_proof() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-readiness", "src/ready.rs", "readiness probe")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let report = wait_until_ready(&mut rt)?;
    assert_eq!(report.active_repositories, 1);
    assert_eq!(report.active_candidate_integrity, Some(true));
    assert!(
        !report
            .not_ready_reasons
            .contains(&ProcessReadinessReasonV1::ActiveCandidateIntegrityFailed)
    );
    Ok(())
}
