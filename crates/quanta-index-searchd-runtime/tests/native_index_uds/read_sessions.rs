//! Product read-session custody through the real SDK, UDS, Catalog, and pump.

use super::*;
use quanta_index_contract::native_protocol::{NativeReadSessionState, NativeReadSettlementState};
use quanta_index_searchd_runtime::native::{
    NativeReadSessionConfig, NativeRetentionConfig, NativeRetentionScope,
};

fn read_session_resources(lease: Duration) -> NativeServiceResources {
    let mut resources = test_resources();
    resources.read_ticket_lease = lease;
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    resources
}

fn read_session_service(
    root: &std::path::Path,
    socket: &std::path::Path,
    lease: Duration,
) -> TestResult<NativeIndexService> {
    let resources = read_session_resources(lease);
    let recipe = TextRecipe::new(Digest([31; 32]));
    let spec = native_text_spec();
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let mut capabilities = reclaim_capabilities(None)?;
    capabilities
        .builders
        .register(descriptor, NativeTextIndex::new(recipe), |delta| {
            Ok(delta.clone())
        })?;
    let read_need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    capabilities.text.register(
        descriptor,
        read_need,
        NativeTextReader::new(recipe, Bm25Policy { k1: 1.2, b: 0.75 }, read_need)?,
    )?;
    let retention = NativeRetentionConfig {
        scopes: vec![NativeRetentionScope {
            workspace: workspace(),
            epoch: 1,
        }],
        sequence_window: 1,
        page_limit: 8,
        interval: Duration::from_millis(20),
        round_timeout: Duration::from_secs(2),
        work_limits: work_limits(),
        head_probe_every: 2,
    };
    let service = NativeIndexService::bind_with_retention(
        root,
        socket,
        work_limits(),
        capabilities,
        resources,
        Some(test_reclaim_config()),
        Some(retention),
    )?;
    service.configure_read_sessions(NativeReadSessionConfig::default())?;
    Ok(service)
}

fn create_text_workspace(
    client: &NativeUdsClient,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("read-session genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn publish_text_owner(
    client: &NativeUdsClient,
    expected: quanta_index_contract::index::ViewRef,
    sequence: u64,
    stable_id: u8,
    path: &str,
    text: &str,
) -> TestResult<(
    quanta_index_contract::index::ViewRef,
    quanta_index_contract::index::ObjectRef,
)> {
    let ingress = client.begin_input(operation(sequence), &budget())?;
    let staged = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: path.into(),
            bytes: text.as_bytes().to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget(),
    )?;
    let [source] = staged.as_slice() else {
        return Err("read-session source fixture did not stage one source".into());
    };
    let prior = client.read_view(ViewSelection::Exact(expected), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: OwnerKey {
                producer: operation(sequence).producer,
                stable_id: StableOwnerId([stable_id; 16]),
            },
            expected: None,
            next: Some(*source),
        }],
        &budget(),
    )?;
    let event_sequence = sequence
        .checked_sub(1)
        .ok_or("read-session event sequence underflow")?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(sequence).producer,
        stream: operation(sequence).stream,
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("read-session event predecessor underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    let view = published(&client.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: expected,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    Ok((view, *source))
}

fn view_pin_count(root: &std::path::Path, sequence: u64) -> TestResult<i64> {
    let connection = rusqlite::Connection::open_with_flags(
        root.join("catalog/catalog-v1.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let sequence = i64::try_from(sequence)?;
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM native_pin_v1 WHERE epoch=1 AND sequence=?1",
        [sequence],
        |row| row.get(0),
    )?)
}

#[test]
fn retained_text_session_survives_floor_and_releases_original_catalog_pin() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult<quanta_index_contract::index::ViewRef> {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let (held_view, held_source) =
            publish_text_owner(&client, configured, 2, 1, "held.rs", "alpha alpha")?;
        let opened =
            client.open_read_session(ViewSelection::Exact(held_view), [9; 32], &budget())?;
        assert_eq!(opened.view, held_view);
        assert_eq!(opened.state, NativeReadSessionState::Active);
        assert_eq!(opened.revision, 1);
        assert_eq!(
            client.open_read_session(ViewSelection::Exact(held_view), [9; 32], &budget())?,
            opened
        );
        let renewed = client.renew_read_session(&opened, &budget())?;
        assert_eq!(renewed.revision, 2);
        assert_eq!(client.renew_read_session(&opened, &budget())?, renewed);
        assert_eq!(client.read_session_status(&opened, &budget())?, renewed);
        let other =
            client.open_read_session(ViewSelection::Exact(held_view), [10; 32], &budget())?;

        let prior = client.search_text(
            ViewSelection::Exact(held_view),
            native_text_spec().instance,
            native_text_request("alpha", 2),
            &budget(),
        )?;
        assert_eq!(prior.view, held_view);
        assert_eq!(prior.matched, 1);
        let [prior_hit] = prior.hits.as_slice() else {
            return Err("pre-retention text query did not return one fixed hit".into());
        };
        assert_eq!(prior_hit.owner.stable_id, StableOwnerId([1; 16]));
        assert_eq!(prior_hit.source, held_source);
        assert_eq!(prior_hit.path, "held.rs");

        let (current, _next_source) =
            publish_text_owner(&client, held_view, 3, 2, "new.rs", "beta")?;
        assert_ne!(current, held_view);
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("read-session floor deadline overflow")?;
        loop {
            if Instant::now() >= deadline {
                return Err(format!(
                    "retention did not make the unpinned Exact view unavailable: held={}, current={}",
                    held_view.sequence, current.sequence,
                )
                .into());
            }
            let outcome = client.search_text(
                ViewSelection::Exact(held_view),
                native_text_spec().instance,
                native_text_request("alpha", 2),
                &RequestBudgetV1::until(deadline),
            );
            if Instant::now() >= deadline {
                return Err("retention Exact refusal arrived after the original deadline".into());
            }
            match outcome {
                Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
                {
                    break;
                }
                Ok(result) => {
                    assert_eq!(result.view, held_view);
                    assert_eq!(result.matched, 1);
                    let [hit] = result.hits.as_slice() else {
                        return Err("pre-retention text query lost its fixed hit".into());
                    };
                    assert_eq!(hit.owner.stable_id, StableOwnerId([1; 16]));
                    assert_eq!(hit.source, held_source);
                    assert_eq!(hit.path, "held.rs");
                }
                Err(error) => return Err(error.into()),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let held = client.search_text_in_read_session(
            &renewed,
            native_text_spec().instance,
            native_text_request("alpha", 2),
            &budget(),
        )?;
        assert_eq!(held.view, held_view);
        assert_eq!(held.matched, 1);
        let [hit] = held.hits.as_slice() else {
            return Err("retained text query did not return exactly one fixed hit".into());
        };
        assert_eq!(hit.owner.stable_id, StableOwnerId([1; 16]));
        assert_eq!(hit.source, held_source);
        assert_eq!(hit.path, "held.rs");

        let (released, remaining) = client.release_read_session(&renewed, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        assert!(remaining);
        let waiting = client.read_session_settlement(&released, &budget())?;
        assert_eq!(waiting.state, NativeReadSettlementState::Pending);
        assert!(waiting.has_remaining_target_pins);
        let settlement_deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("read-session settlement deadline overflow")?;
        let (last_released, last_remaining) = client.release_read_session(&other, &budget())?;
        assert_eq!(last_released.state, NativeReadSessionState::Released);
        assert!(!last_remaining);
        // Release replay retains its committed observation; settlement below
        // observes the later Catalog snapshot after the other pin was removed.
        assert_eq!(
            client.release_read_session(&renewed, &budget())?,
            (released, true)
        );
        let after_release = client.search_text_in_read_session(
            &renewed,
            native_text_spec().instance,
            native_text_request("alpha", 2),
            &budget(),
        );
        match after_release {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable => {}
            Err(error) => return Err(error.into()),
            Ok(result) => {
                return Err(format!("released read session query succeeded: {result:?}").into());
            }
        }
        loop {
            let observed = client
                .read_session_settlement(&released, &RequestBudgetV1::until(settlement_deadline))?;
            assert_eq!(observed.session, released.session);
            assert_eq!(observed.view, held_view);
            assert_eq!(observed.head, current);
            assert!(observed.exact_floor > held_view.sequence);
            assert!(!observed.has_remaining_target_pins);
            if Instant::now() >= settlement_deadline {
                return Err(format!(
                    "real retention/reclaim did not settle after the last pin: {:?}",
                    observed.state
                )
                .into());
            }
            if observed.state == NativeReadSettlementState::SettledAtSnapshot {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(held_view)
    })();
    let terminal = running.stop();
    match (primary, terminal) {
        (Ok(held_view), Ok(())) => {
            assert_eq!(view_pin_count(root.path(), held_view.sequence)?, 0);
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_held_view), Err(error)) => Err(error),
        (Err(primary), Err(terminal)) => Err(format!(
            "read-session primary failure: {primary}; service stop failure: {terminal}"
        )
        .into()),
    }
}

#[test]
fn expired_and_shutdown_read_sessions_release_real_pins() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(1),
    )?);
    let primary = (|| -> TestResult<quanta_index_contract::index::ViewRef> {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let expired =
            client.open_read_session(ViewSelection::Exact(configured), [1; 32], &budget())?;
        assert_eq!(expired.state, NativeReadSessionState::Active);
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(8))
            .ok_or("read-session expiry deadline overflow")?;
        let swept = loop {
            let status = client.read_session_status(&expired, &RequestBudgetV1::until(deadline))?;
            if Instant::now() >= deadline {
                return Err(format!("expired read session was not swept: {status:?}").into());
            }
            if status.state == NativeReadSessionState::Released {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // Expiry cleanup removed the pin, but it did not commit this client's
        // Release. Neither an ACK replay nor settlement may invent that event.
        match client.release_read_session(&expired, &budget()) {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable => {}
            Err(error) => return Err(error.into()),
            Ok(reply) => {
                return Err(
                    format!("expiry cleanup became a public Release ACK: {reply:?}").into(),
                );
            }
        }
        match client.read_session_settlement(&swept, &budget()) {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable => {}
            Err(error) => return Err(error.into()),
            Ok(reply) => {
                return Err(format!("expiry cleanup became public settlement: {reply:?}").into());
            }
        }
        let after_expiry = client.search_text_in_read_session(
            &expired,
            native_text_spec().instance,
            native_text_request("alpha", 2),
            &budget(),
        );
        match after_expiry {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable => {}
            Err(error) => return Err(error.into()),
            Ok(result) => {
                return Err(format!("expired read session query succeeded: {result:?}").into());
            }
        }
        let probe =
            client.open_read_session(ViewSelection::Exact(configured), [2; 32], &budget())?;
        assert_eq!(probe.state, NativeReadSessionState::Active);
        let (probe_released, probe_remaining) = client.release_read_session(&probe, &budget())?;
        assert_eq!(probe_released.state, NativeReadSessionState::Released);
        assert!(!probe_remaining);
        let active =
            client.open_read_session(ViewSelection::Exact(configured), [3; 32], &budget())?;
        assert_eq!(active.state, NativeReadSessionState::Active);
        Ok(configured)
    })();
    let terminal = running.stop();
    match (primary, terminal) {
        (Ok(configured), Ok(())) => {
            assert_eq!(view_pin_count(root.path(), configured.sequence)?, 0);
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_configured), Err(error)) => Err(error),
        (Err(primary), Err(terminal)) => Err(format!(
            "read-session primary failure: {primary}; service stop failure: {terminal}"
        )
        .into()),
    }
}
