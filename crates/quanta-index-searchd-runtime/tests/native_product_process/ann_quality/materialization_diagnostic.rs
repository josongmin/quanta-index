//! Explicitly selected phase diagnosis through the existing product path.
//!
//! These phase fixtures never qualify original-4K ANN acceptance. A successful
//! fixed-source quiet-host Off/Record pair can measure only Source overhead.

use super::*;

#[test]
#[ignore = "phase diagnosis only; requires QIDX_NATIVE_ANN_TRACE_DIR"]
fn diagnostic_native_ann_128_owner_materialization_phases() -> TestResult {
    run_phase_diagnostic(128, true, None)
}

#[test]
#[ignore = "source phase diagnosis only; requires QIDX_NATIVE_ANN_TRACE_DIR"]
fn diagnostic_native_ann_4096_source_advance_phases() -> TestResult {
    run_phase_diagnostic(4_096, false, None)
}

#[test]
#[ignore = "4K source and exact 128-owner completion prefix diagnosis only; requires trace directory and explicit 256 MiB Record cap"]
fn diagnostic_native_ann_4096_source_128_completed_prefix() -> TestResult {
    require(
        matches!(
            std::env::var("QIDX_NATIVE_ANN_DIAGNOSTIC_TRACE_MODE").as_deref(),
            Ok("record")
        ),
        "4K prefix diagnosis requires explicit Record mode",
    )?;
    require(
        matches!(
            std::env::var("QIDX_NATIVE_TRACE_MAX_OUTPUT_BYTES").as_deref(),
            Ok("268435456")
        ),
        "4K prefix diagnosis requires explicit 256 MiB trace cap",
    )?;
    run_phase_diagnostic(4_096, true, Some(128))
}

fn run_phase_diagnostic(rows: u64, materialize: bool, prefix_target: Option<usize>) -> TestResult {
    require(
        prefix_target.is_none() || (rows == 4_096 && materialize && prefix_target == Some(128)),
        "unsupported phase diagnostic completion prefix",
    )?;
    let record = match std::env::var("QIDX_NATIVE_ANN_DIAGNOSTIC_TRACE_MODE") {
        Ok(mode) if mode == "record" => true,
        Ok(mode) if mode == "off" => false,
        Err(std::env::VarError::NotPresent) => true,
        _ => return Err("diagnostic trace mode must be exactly off or record".into()),
    };
    let trace_directory = PathBuf::from(
        std::env::var_os("QIDX_NATIVE_ANN_TRACE_DIR")
            .ok_or("phase diagnosis requires an existing external trace directory")?,
    );
    require(trace_directory.is_dir(), "trace directory is absent")?;
    let config = AnnRailConfig {
        rows,
        ..AnnRailConfig::DEFAULT
    };
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-ann-phase-diagnostic.json");
    let stub = EmbeddingStub::start_ann_quality()?;
    let base_url = stub.base_url();
    let mut configuration =
        config_with_openai_semantic_dimension(&root, &socket, u32::try_from(config.width)?)?;
    set_fixture_json(
        &mut configuration,
        "/semantic/read/max_owners",
        serde_json::json!(QUALITY_MAX_OWNERS),
    )?;
    configure_semantic_endpoint(&mut configuration, &base_url)?;
    configure_owned_semantic(&mut configuration)?;
    configure_effort(&mut configuration)?;
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let semantic = openai_semantic_spec_dimension(&base_url, u32::try_from(config.width)?)?;
    let trace = trace_directory.join(if prefix_target.is_some() {
        "ann-materialization-4096-prefix128.qtrace"
    } else if materialize {
        "ann-materialization-128.qtrace"
    } else {
        "ann-source-4096.qtrace"
    });
    writeln!(
        std::io::stderr().lock(),
        "native-ann-phase rows={rows} materialize={materialize} trace_record={record}"
    )?;
    let mut daemon = Daemon::start_with_api_key_and_trace(
        &config_path,
        &socket,
        Some("test-key"),
        record.then_some(trace.as_path()),
    )?;
    let client = NativeUdsClient::new(socket);
    let phase = Cell::new("diagnostic workspace creation");
    let result = (|| -> TestResult {
        let NativeIndexResponse::Created(genesis) = client.request(
            NativeIndexRequest::CreateWorkspace {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?
        else {
            return Err("diagnostic workspace creation returned another variant".into());
        };
        phase.set("diagnostic profile staging");
        let ingress = client.begin_input(operation(1), &budget())?;
        let profile = client.stage_profile(
            ingress,
            IndexProfile {
                instances: vec![text_spec(), semantic.clone()],
            },
            &budget(),
        )?;
        let configured = published(&client.submit(
            UpdateRequest {
                operation: operation(1),
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
            &budget(),
        )?)?;
        // A shorter diagnostic watchdog cannot extend any original request or
        // assignment. All production numeric/SDK/worker policies stay unchanged.
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(180))
            .ok_or("diagnostic watchdog overflow")?;
        phase.set("diagnostic source advance");
        let staged = stage_quality_sources(&client, configured, &config, deadline, |seed| {
            format!("row-{seed}")
        })?;
        if let Some(target) = prefix_target {
            phase.set("diagnostic exact owner completion prefix");
            require_equal(
                &staged.sources.len(),
                &usize::try_from(config.rows)?,
                "4K prefix source inventory count",
            )?;
            // The Source view may retire while the unchanged pump publishes
            // materializations. Reconcile its durable original-key receipt;
            // no historical read-view pin or partial coverage claim is needed.
            let status = client.status(operation(2), &budget().capped_at(deadline))?;
            require_equal(
                &status,
                &OperationStatus::Published(staged.view),
                "4K prefix original Source publication receipt",
            )?;
            writeln!(
                std::io::stderr().lock(),
                "native-ann-prefix source_rows={} target_completed={target} source_published=true full_quality=false",
                staged.sources.len(),
            )?;
            let completed = wait_for_completed_prefix(
                &client,
                &mut daemon,
                work_status::CompletionProgress::new(&staged.sources, staged.view, &semantic)?,
                target,
                deadline,
            )?;
            require(
                completed >= target,
                "4K prefix returned without exact public completion receipts",
            )?;
            return Ok(());
        }
        if !materialize {
            phase.set("diagnostic source publication receipt");
            // This fixture measures SourceAdvance, whose immediate view can be
            // retired by the unchanged pump. Reconcile the durable original-key
            // receipt instead of reading an unleased historical view. Complete
            // inventory and ANN quality remain the original product rail's oracle.
            let status = client.status(operation(2), &budget().capped_at(deadline))?;
            require_equal(
                &status,
                &OperationStatus::Published(staged.view),
                "diagnostic original source publication receipt",
            )?;
            return Ok(());
        }
        phase.set("diagnostic materialization");
        let selected = wait_for_complete(
            &client,
            &mut daemon,
            staged.view,
            work_status::CompletionProgress::new(&staged.sources, staged.view, &semantic)?,
            &config,
            Some(config.rows),
            deadline,
        )?;
        phase.set("diagnostic selected source inventory");
        verify_selected_sources(&client, selected, &staged.sources, deadline)?;
        Ok(())
    })();
    if let Err(error) = result {
        report_provider_from_stub("diagnostic failure prefix", &stub, 0);
        return Err(quality_failure(phase.get(), error.as_ref(), daemon));
    }
    quality_stop("diagnostic daemon shutdown", daemon)?;
    report_provider_from_stub("diagnostic through confirmed shutdown", &stub, 0);
    stub.finish()
}

fn wait_for_completed_prefix(
    client: &NativeUdsClient,
    daemon: &mut Daemon,
    mut completion: work_status::CompletionProgress<'_>,
    target: usize,
    deadline: Instant,
) -> TestResult<usize> {
    require(
        target > 0 && target < completion.total(),
        "diagnostic completion prefix is outside the source inventory",
    )?;
    let mut pace = work_status::ReadinessPace::new();
    let mut cost = work_status::ReadinessCost::new("4096-prefix128");
    let mut previous_completed = 0;
    let mut next_report = 16;
    loop {
        require(
            Instant::now() < deadline,
            format!(
                "4K prefix watchdog expired with {}/{} exact completed receipts",
                completion.completed(),
                target,
            ),
        )?;
        if let Some(status) = daemon.0.try_wait()? {
            return Err(format!(
                "4K prefix daemon exited {status} with {}/{} exact completed receipts",
                completion.completed(),
                target,
            )
            .into());
        }
        let polled = completion.poll_next(|intent| {
            require(
                Instant::now() < deadline,
                "4K prefix watchdog expired before work-status read",
            )?;
            let started = Instant::now();
            let reply = client.request(
                NativeIndexRequest::WorkStatus(intent),
                &budget().capped_at(deadline),
            );
            cost.record_rpc(started.elapsed());
            let reply = reply?;
            let NativeIndexResponse::WorkStatus(status) = reply else {
                return Err(format!("4K prefix work-status reply differs: {reply:?}").into());
            };
            Ok(status)
        });
        let completed = completion.completed();
        cost.observe_completed(completed);
        let _all_completed = polled?;
        require(
            Instant::now() < deadline,
            format!(
                "4K prefix watchdog expired during work-status page with {completed}/{target} receipts"
            ),
        )?;
        if completed >= next_report {
            writeln!(
                std::io::stderr().lock(),
                "native-ann-prefix-progress source_rows={} actual_completed={completed} target_completed={target}",
                completion.total(),
            )?;
            next_report = completed
                .checked_div(16)
                .and_then(|groups| groups.checked_add(1))
                .and_then(|groups| groups.checked_mul(16))
                .ok_or("4K prefix progress report overflow")?;
        }
        if completed >= target {
            cost.mark_success();
            writeln!(
                std::io::stderr().lock(),
                "native-ann-prefix-complete source_rows={} actual_completed={completed} target_completed={target} full_quality=false",
                completion.total(),
            )?;
            return Ok(completed);
        }
        let advanced = completed > previous_completed;
        previous_completed = completed;
        thread::sleep(
            pace.after_page(advanced, deadline.saturating_duration_since(Instant::now())),
        );
    }
}
