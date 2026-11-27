//! Explicitly selected phase diagnosis through the existing product path.
//!
//! These phase fixtures never qualify original-4K ANN acceptance. A successful
//! fixed-source quiet-host Off/Record pair can measure only Source overhead.

use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

type CandidateSources = BTreeMap<Vec<u8>, (OwnerKey, ObjectRef)>;

// The live Catalog is only an index into the public work-status API. Its
// terminal columns cannot qualify a completion without the exact SDK receipt.
fn expected_candidate_sources(
    sources: &[(OwnerKey, ObjectRef)],
    source_view: ViewRef,
    spec: &InstanceSpec,
) -> TestResult<CandidateSources> {
    let mut expected = BTreeMap::new();
    for &(owner, source) in sources {
        let intent = work_status::owner_intent(
            source_view.workspace,
            source_view.epoch,
            spec,
            owner,
            source,
        );
        let key = intent.identity_digest()?.0.to_vec();
        require(
            expected.insert(key, (owner, source)).is_none(),
            "4K prefix source inventory has duplicate work identities",
        )?;
    }
    Ok(expected)
}

fn select_completed_candidates<E: std::error::Error + 'static>(
    rows: impl IntoIterator<Item = Result<(Vec<u8>, Vec<u8>, i64), E>>,
    expected: &CandidateSources,
    source_operation: &[u8],
    target: usize,
) -> TestResult<Vec<(OwnerKey, ObjectRef)>> {
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    selected.try_reserve_exact(target)?;
    for row in rows {
        let (key, operation, status) = row?;
        require(
            seen.insert(key.clone()),
            "4K prefix Catalog returned a duplicate work identity",
        )?;
        require(
            seen.len() <= expected.len(),
            "4K prefix Catalog created-view cohort exceeds source inventory",
        )?;
        let &(owner, source) = expected
            .get(&key)
            .ok_or("4K prefix Catalog returned a foreign work identity")?;
        require_equal(
            &operation.as_slice(),
            &source_operation,
            "4K prefix Catalog work creator",
        )?;
        match status {
            2 => {
                if selected.len() < target {
                    selected.push((owner, source));
                }
            }
            3 => return Err(format!("4K prefix owner {owner:?} was superseded").into()),
            4 => return Err(format!("4K prefix owner {owner:?} was blocked").into()),
            _ => {
                return Err(
                    format!("4K prefix Catalog terminal status is invalid: {status}").into(),
                );
            }
        }
    }
    Ok(selected)
}

fn catalog_terminal_candidates(
    connection: &rusqlite::Connection,
    expected: &CandidateSources,
    source_view: ViewRef,
    source_operation: &[u8],
    target: usize,
) -> TestResult<Vec<(OwnerKey, ObjectRef)>> {
    // The created-view index limits this scan to the fixed 4K Source cohort.
    // A 4,097th terminal row is a scope error, not a reason to truncate it.
    let mut statement = connection.prepare(
        "SELECT work,operation,status FROM native_work_v1 INDEXED BY native_work_created_view_idx_v1
         WHERE workspace=?1 AND epoch=?2 AND sequence=?3 AND status IN (2,3,4)
         ORDER BY work LIMIT ?4",
    )?;
    let workspace = serde_json::to_vec(&source_view.workspace)?;
    let epoch = i64::try_from(source_view.epoch)?;
    let sequence = i64::try_from(source_view.sequence)?;
    let limit = i64::try_from(
        expected
            .len()
            .checked_add(1)
            .ok_or("4K prefix cohort overflow")?,
    )?;
    let selected = select_completed_candidates(
        statement.query_map(
            rusqlite::params![workspace, epoch, sequence, limit],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?,
        expected,
        source_operation,
        target,
    )?;
    drop(statement);
    Ok(selected)
}

#[test]
fn prefix_catalog_finds_late_completed_candidates_but_requires_public_receipts() -> TestResult {
    let source_view = ViewRef {
        workspace: workspace(),
        epoch: 1,
        sequence: 5,
        digest: Digest([5; 32]),
    };
    let spec = openai_semantic_spec_dimension("http://127.0.0.1:1", 64)?;
    let sources = (0..4_096_u64)
        .map(|seed| {
            let source_bytes = seed.to_le_bytes();
            let mut source_digest = [0_u8; 32];
            source_digest[..source_bytes.len()].copy_from_slice(&source_bytes);
            Ok((
                quality_owner(seed, &AnnRailConfig::DEFAULT)?,
                ObjectRef(Digest(source_digest)),
            ))
        })
        .collect::<TestResult<Vec<_>>>()?;
    let expected = expected_candidate_sources(&sources, source_view, &spec)?;
    require_equal(&expected.len(), &4_096, "test candidate identity count")?;
    let mut connection = rusqlite::Connection::open_in_memory()?;
    connection.execute_batch(
        "CREATE TABLE native_work_v1 (work BLOB PRIMARY KEY, operation BLOB NOT NULL,
            workspace BLOB NOT NULL, epoch INTEGER NOT NULL, sequence INTEGER NOT NULL,
            status INTEGER NOT NULL) WITHOUT ROWID;
         CREATE INDEX native_work_created_view_idx_v1
            ON native_work_v1(workspace,epoch,sequence,work);",
    )?;
    let operation_key = serde_json::to_vec(&operation(2))?;
    let workspace_key = serde_json::to_vec(&source_view.workspace)?;
    let tx = connection.transaction()?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO native_work_v1(work,operation,workspace,epoch,sequence,status)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        for (index, &(owner, source)) in sources.iter().enumerate() {
            let key = work_status::owner_intent(
                source_view.workspace,
                source_view.epoch,
                &spec,
                owner,
                source,
            )
            .identity_digest()?
            .0
            .to_vec();
            let status = if index < 4_096 - 128 { 0_i64 } else { 2_i64 };
            let inserted = insert.execute(rusqlite::params![
                key,
                &operation_key,
                &workspace_key,
                1_i64,
                5_i64,
                status
            ])?;
            require_equal(&inserted, &1, "test work row insertion")?;
        }
    }
    tx.commit()?;
    for (key, workspace, epoch, sequence) in [
        (vec![0_u8; 33], vec![0_u8], 1_i64, 5_i64),
        (vec![1_u8; 33], workspace_key.clone(), 2_i64, 5_i64),
        (vec![2_u8; 33], workspace_key.clone(), 1_i64, 6_i64),
    ] {
        let inserted = connection.execute(
            "INSERT INTO native_work_v1(work,operation,workspace,epoch,sequence,status)
             VALUES (?1,?2,?3,?4,?5,2)",
            rusqlite::params![key, &operation_key, workspace, epoch, sequence],
        )?;
        require_equal(&inserted, &1, "test foreign scope insertion")?;
    }
    let selected =
        catalog_terminal_candidates(&connection, &expected, source_view, &operation_key, 128)?;
    require_equal(&selected.len(), &128, "late prefix candidate count")?;
    let selected_owners: BTreeSet<_> = selected.iter().map(|(owner, _)| *owner).collect();
    let late_owners: BTreeSet<_> = sources[4_096 - 128..]
        .iter()
        .map(|(owner, _)| *owner)
        .collect();
    require_equal(
        &selected_owners,
        &late_owners,
        "late prefix exact owner scope",
    )?;
    let mut public = work_status::CompletionProgress::new(&selected, source_view, &spec)?;
    require(
        !public.poll_next(|_| Ok(quanta_index_contract::index::WorkStatus::Pending))?,
        "Catalog-only candidate was promoted to public completion",
    )?;
    require_equal(
        &public.completed(),
        &0,
        "Catalog-only public completion count",
    )
}

#[test]
fn prefix_catalog_rejects_foreign_and_terminal_scope_rows() -> TestResult {
    let owner = quality_owner(0, &AnnRailConfig::DEFAULT)?;
    let source = ObjectRef(Digest([7; 32]));
    let key = vec![9_u8; 32];
    let expected = BTreeMap::from([(key.clone(), (owner, source))]);
    let operation_key = serde_json::to_vec(&operation(2))?;
    let row = |key: Vec<u8>, operation: Vec<u8>, status: i64| {
        Ok::<_, std::io::Error>((key, operation, status))
    };
    require(
        select_completed_candidates(
            vec![row(vec![8; 32], operation_key.clone(), 2)],
            &expected,
            &operation_key,
            1,
        )
        .is_err(),
        "foreign candidate was accepted",
    )?;
    require(
        select_completed_candidates(
            vec![row(key.clone(), vec![0; operation_key.len()], 2)],
            &expected,
            &operation_key,
            1,
        )
        .is_err(),
        "foreign creator was accepted",
    )?;
    for status in [3_i64, 4_i64] {
        require(
            select_completed_candidates(
                vec![row(key.clone(), operation_key.clone(), status)],
                &expected,
                &operation_key,
                1,
            )
            .is_err(),
            "terminal failed work was accepted",
        )?;
    }
    require(
        select_completed_candidates(
            vec![
                row(key.clone(), operation_key.clone(), 2),
                row(key, operation_key.clone(), 2),
            ],
            &expected,
            &operation_key,
            1,
        )
        .is_err(),
        "duplicate candidate was accepted",
    )
}

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
        "native-ann-phase rows={rows} materialize={materialize} trace_record={record} trace_path={:?}",
        record.then_some(trace.as_path()),
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
                instances: vec![literal_spec(), text_spec(), semantic.clone()],
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
                &root,
                &staged.sources,
                staged.view,
                &semantic,
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
    state_root: &Path,
    sources: &[(OwnerKey, ObjectRef)],
    source_view: ViewRef,
    spec: &InstanceSpec,
    target: usize,
    deadline: Instant,
) -> TestResult<usize> {
    require(
        target > 0 && target < sources.len(),
        "diagnostic completion prefix is outside the source inventory",
    )?;
    let expected = expected_candidate_sources(sources, source_view, spec)?;
    require_equal(
        &expected.len(),
        &sources.len(),
        "4K prefix distinct source intents",
    )?;
    let source_operation = serde_json::to_vec(&operation(2))?;
    let mut pace = work_status::ReadinessPace::new();
    let selection_started = Instant::now();
    let selected = {
        // Opening the live database read-only skips Catalog constructor recovery
        // and never claims or funds work. Close it before any SDK census.
        let connection = rusqlite::Connection::open_with_flags(
            quanta_index_catalog::catalog_dir(state_root)
                .join(quanta_index_catalog::CATALOG_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let mut previous_candidates = 0;
        let selected = loop {
            require(
                Instant::now() < deadline,
                format!(
                    "4K prefix watchdog expired with {previous_candidates}/{target} Catalog candidates and no public completion claim"
                ),
            )?;
            if let Some(status) = daemon.0.try_wait()? {
                return Err(format!(
                    "4K prefix daemon exited {status} with {previous_candidates}/{target} Catalog candidates"
                )
                .into());
            }
            connection.busy_timeout(
                Duration::from_secs(2).min(deadline.saturating_duration_since(Instant::now())),
            )?;
            let candidates = catalog_terminal_candidates(
                &connection,
                &expected,
                source_view,
                &source_operation,
                target,
            )?;
            require(
                Instant::now() < deadline,
                "4K prefix watchdog expired during Catalog candidate scan",
            )?;
            if candidates.len() == target {
                break candidates;
            }
            let advanced = candidates.len() > previous_candidates;
            previous_candidates = candidates.len();
            thread::sleep(
                pace.after_page(advanced, deadline.saturating_duration_since(Instant::now())),
            );
        };
        connection.close().map_err(|(_connection, error)| error)?;
        selected
    };
    writeln!(
        std::io::stderr().lock(),
        "native-ann-prefix-candidates selected={} elapsed_ms={}",
        selected.len(),
        selection_started.elapsed().as_millis(),
    )?;
    // The Catalog selected a bounded subset, but only exact public per-intent
    // receipts can make the diagnostic succeed. This also checks distinct
    // completion sequences and rejects a foreign view/claim.
    let mut completion = work_status::CompletionProgress::new(&selected, source_view, spec)?;
    let mut cost = work_status::ReadinessCost::new("4096-prefix128-public-receipts");
    let mut previous_completed = 0;
    let mut next_report = 16;
    let mut pace = work_status::ReadinessPace::new();
    loop {
        require(
            Instant::now() < deadline,
            format!(
                "4K prefix watchdog expired with {}/{} exact public completed receipts",
                completion.completed(),
                target,
            ),
        )?;
        if let Some(status) = daemon.0.try_wait()? {
            return Err(format!(
                "4K prefix daemon exited {status} with {}/{} exact public completed receipts",
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
        let observed_at = Instant::now();
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
                sources.len(),
            )?;
            next_report = completed
                .checked_div(16)
                .and_then(|groups| groups.checked_add(1))
                .and_then(|groups| groups.checked_mul(16))
                .ok_or("4K prefix progress report overflow")?;
        }
        if completed >= target {
            cost.observe_receipts_complete(observed_at);
            cost.mark_success();
            writeln!(
                std::io::stderr().lock(),
                "native-ann-prefix-complete source_rows={} actual_completed={completed} target_completed={target} full_quality=false",
                sources.len(),
            )?;
            return Ok(completed);
        }
        let advanced = completed > previous_completed;
        previous_completed = completed;
        cost.pause_before_receipts(
            pace.after_page(advanced, deadline.saturating_duration_since(Instant::now())),
        );
    }
}
