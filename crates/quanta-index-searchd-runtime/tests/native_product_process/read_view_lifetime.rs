use super::*;
use quanta_index_contract::{
    index::{ObjectRef, ViewRef},
    native_protocol::NativeReadSettlementState,
};
use quanta_index_searchd_harness::product_xfs::ProductXfsProfile;

pub(super) fn check_product_daemon_publish_query_and_reopen(
    retention: bool,
    witness_file: &str,
    witness_test: &str,
) -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let config_path = temp.path().join("native-config.json");
    let selected = selected_test_daemon_binary(
        "P04 requires the release daemon path in QUANTA_INDEX_SEARCHD_BIN",
        "P04 daemon path must be absolute",
    )?;
    let binary = selected.path.as_path();
    let socket = if selected.product {
        ProductXfsProfile::service_socket(temp.path(), "uds")?
    } else {
        temp.path().join("native.sock")
    };
    let profile = selected
        .product
        .then(ProductXfsProfile::from_env)
        .transpose()?;
    let product_root = profile
        .as_ref()
        .map(ProductXfsProfile::tempdir)
        .transpose()?;
    let root = product_root.as_ref().map_or_else(
        || temp.path().join("state"),
        |directory| directory.path().join("state"),
    );
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    // Read sessions require funded automatic maintenance. This functional
    // fixture uses its existing maintenance-capable resource profile for both
    // retention modes; it does not qualify the minimal product defaults.
    let mut configuration = config_with_reclaim(&root, &socket)?;
    // One retained SDK reader is live at a time. Use the existing bounded
    // product read-session configuration without changing work/resource caps.
    let _previous_read_sessions = configuration
        .as_object_mut()
        .ok_or("invalid product config fixture")?
        .insert(
            "read_sessions".into(),
            serde_json::json!({
                "global_limit": 1,
                "per_uid_limit": 1,
                "terminal_ttl_millis": 300_000_u64,
                "sweep_limit": 1
            }),
        );
    if retention {
        release_bounded_maintenance(&mut configuration)?;
        let object = configuration
            .as_object_mut()
            .ok_or("invalid product config fixture")?;
        let _previous = object.insert("retention".into(), serde_json::json!({
            "scopes": [{ "workspace": workspace(), "epoch": 1 }],
            "sequence_window": 2, "page_limit": 1,
            "interval_ms": 20, "round_timeout_ms": 2000, "head_probe_every": 2,
            "work_limits": {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000}
        }));
    }
    let sqlite_temp = if let Some(profile) = profile.as_ref() {
        profile.configure(&mut configuration)?;
        Some(profile.prepare_root(&root, &socket)?)
    } else {
        None
    };
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    if let Some(sqlite_temp) = sqlite_temp.as_ref() {
        ProductXfsProfile::preflight(binary, &config_path, sqlite_temp)?;
    }
    let start_daemon = || {
        if let Some(sqlite_temp) = sqlite_temp.as_ref() {
            Daemon::start_command_before(
                ProductXfsProfile::command(binary, sqlite_temp),
                &config_path,
                &socket,
                None,
                None,
                None,
            )
        } else {
            Daemon::start_binary(binary, &config_path, &socket, None, None)
        }
    };
    let daemon = start_daemon()?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec(), text_spec()],
        },
        &budget(),
    )?;
    let configured = submit_and_settle(
        &client,
        &UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
    )?;
    let (view, source) = publish_text(&client, configured, 1, 1, None, "a.rs", "needle needle")?;
    let query = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 2,
        cursor: None,
    };
    let ranked = client.search_text(
        ViewSelection::Exact(view),
        text_spec().instance,
        query.clone(),
        &budget(),
    )?;
    require_equal(&ranked.view, &view, "native query view")?;
    require_equal(&ranked.matched, &1, "native query match count")?;
    let [hit] = ranked.hits.as_slice() else {
        return Err("native text query must return exactly one hit".into());
    };
    require_equal(&hit.path.as_str(), &"a.rs", "native query hit path")?;
    require_equal(&hit.source, &source, "native query hit source")?;
    // The maintained ordinary SDK front door must reach this Native daemon,
    // without a Legacy query/control/ingest listener or a generation mapping.
    let sdk = quanta_index_sdk::QuantaIndex::connect_query_only(
        quanta_index_sdk::ConnectOptions::from_native_socket(&socket, ViewSelection::Exact(view)),
    )?;
    require(
        matches!(
            sdk.producer().native(),
            Err(quanta_index_sdk::SdkError::PlaneUnavailable {
                plane: "native-write"
            })
        ),
        "query-only SDK exposed the Native write sender",
    )?;
    let native_reader = sdk.reader().native()?;
    let mut sdk_session = native_reader.open([81; 32], &budget())?;
    let _sdk_profile = sdk_session.load_profile(&budget())?;
    require_equal(
        &sdk_session.text(&text_spec(), query.clone(), &budget())?,
        &ranked,
        "ordinary SDK Native Text DTO",
    )?;
    let sdk_page = sdk_session.text_page(&text_spec(), query.clone(), None, &budget())?;
    require_equal(
        &sdk_page.hits,
        &ranked.hits,
        "ordinary SDK Native file page",
    )?;
    require_equal(&sdk_page.view, &view, "ordinary SDK captured view")?;
    let workspace_id = format!(
        "{}{}{}{}",
        "01".repeat(16),
        "02".repeat(16),
        "03".repeat(16),
        "04".repeat(16)
    );
    let cli_view = native_cli(vec![
        "read-view".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_id.clone(),
        "--epoch".into(),
        "1".into(),
        "--output".into(),
        "json".into(),
    ])?;
    let cli_view: quanta_index_contract::index::CatalogView = serde_json::from_slice(&cli_view)?;
    require_equal(&cli_view.identity, &view, "CLI read-view identity")?;
    let cli_ranked = native_cli(vec![
        "search-text".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_id,
        "--epoch".into(),
        "1".into(),
        "--instance".into(),
        "47".repeat(16),
        "--query-text".into(),
        "needle".into(),
        "--top-k".into(),
        "2".into(),
        "--output".into(),
        "json".into(),
    ])?;
    let cli_ranked: NativeTextResult = serde_json::from_slice(&cli_ranked)?;
    require_equal(&cli_ranked, &ranked, "CLI native search result")?;
    let (current_view, current_ranked) = if retention {
        let (replacement, replacement_source) = publish_text(
            &client,
            view,
            2,
            1,
            Some(source),
            "replacement.rs",
            "needle",
        )?;
        let (current, _) =
            publish_text(&client, replacement, 3, 2, None, "other.rs", "distractor")?;
        // Observe the live daemon without opening a second Catalog writer or
        // running constructor recovery/maintenance against its owned root.
        let observer = rusqlite::Connection::open_with_flags(
            quanta_index_catalog::catalog_dir(&root).join(quanta_index_catalog::CATALOG_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        observer.busy_timeout(Duration::from_secs(2))?;
        let workspace_key = serde_json::to_vec(&workspace())?;
        let expected_floor = i64::try_from(current.sequence - 1)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("product retention deadline overflow")?;
        loop {
            let floor: i64 = observer.query_row(
                "SELECT COALESCE((SELECT retain_from_sequence FROM native_exact_history_floor_v1 WHERE workspace=?1 AND epoch=1),0)",
                rusqlite::params![&workspace_key], |row| row.get(0),
            )?;
            if floor == expected_floor {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "product retention did not advance: floor={floor}, expected={expected_floor}"
                )
                .into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        observer.close().map_err(|(_connection, error)| error)?;
        require(
            matches!(
                client.read_view(ViewSelection::Exact(view), &budget()),
                Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable
            ),
            "product retained new Exact admission below its automatic floor",
        )?;
        require_equal(
            &sdk_session.text(&text_spec(), query.clone(), &budget())?,
            &ranked,
            "retained SDK reader below Exact admission floor",
        )?;
        require(
            !sdk_session.release(&budget())?,
            "retired view still has target pins",
        )?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let settlement = sdk_session.settlement(&budget())?;
            require_equal(&settlement.view, &view, "settled retired view")?;
            require_equal(&settlement.head, &current, "retention settlement head")?;
            require(
                settlement.exact_floor > view.sequence && !settlement.has_remaining_target_pins,
                "retirement settlement lost its floor or regained a pin",
            )?;
            if settlement.state == NativeReadSettlementState::SettledAtSnapshot {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "retired reader GC did not settle: state={:?}, exact_floor={}, target_pins={}",
                    settlement.state, settlement.exact_floor, settlement.has_remaining_target_pins,
                )
                .into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        let fresh = client.search_text(
            ViewSelection::Exact(current),
            text_spec().instance,
            query.clone(),
            &budget(),
        )?;
        require_equal(&fresh.view, &current, "current query view after retirement")?;
        require_equal(
            &fresh.matched,
            &1,
            "current query match count after retirement",
        )?;
        let [fresh_hit] = fresh.hits.as_slice() else {
            return Err("current view must return exactly one replacement hit".into());
        };
        require_equal(
            &fresh_hit.path.as_str(),
            &"replacement.rs",
            "current hit path",
        )?;
        require_equal(&fresh_hit.source, &replacement_source, "current hit source")?;
        (current, fresh)
    } else {
        (view, ranked)
    };
    let first = read_view_daemon_witness::observe(daemon.0.id())?;
    daemon.stop()?;

    let reopened = start_daemon()?;
    let restored = NativeUdsClient::new(socket.clone());
    let reopened_view = restored.read_view(ViewSelection::Exact(current_view), &budget())?;
    require_equal(
        &reopened_view.identity,
        &current_view,
        "reopened view identity",
    )?;
    if retention {
        require(
            matches!(
                restored.read_view(ViewSelection::Exact(view), &budget()),
                Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable
            ),
            "reopened daemon admitted an Exact view below the persistent retention floor",
        )?;
    } else {
        require(
            matches!(
                sdk_session.text(&text_spec(), query.clone(), &budget()),
                Err(quanta_index_sdk::NativeQueryError::Transport(NativeUdsError::Refused(error)))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable
            ),
            "ordinary SDK reused a pre-restart session",
        )?;
    }
    let reopened_sdk = quanta_index_sdk::QuantaIndex::connect_query_only(
        quanta_index_sdk::ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(current_view),
        ),
    )?;
    let mut reopened_session = reopened_sdk.reader().native()?.open([82; 32], &budget())?;
    let _reopened_sdk_profile = reopened_session.load_profile(&budget())?;
    require_equal(
        &reopened_session.text(&text_spec(), query.clone(), &budget())?,
        &current_ranked,
        "ordinary SDK after real daemon restart",
    )?;
    let _has_remaining_target_pins = reopened_session.release(&budget())?;
    let after = restored.search_text(
        ViewSelection::Exact(current_view),
        text_spec().instance,
        query,
        &budget(),
    )?;
    require_equal(&after, &current_ranked, "reopened native search result")?;
    let second = read_view_daemon_witness::observe(reopened.0.id())?;
    reopened.stop()?;
    read_view_daemon_witness::write([first, second], witness_file, witness_test, None)
}

pub(super) fn publish_text(
    client: &NativeUdsClient,
    previous: ViewRef,
    sequence: u64,
    owner: u8,
    expected: Option<ObjectRef>,
    path: &str,
    text: &str,
) -> TestResult<(ViewRef, ObjectRef)> {
    let (request, [source]) = stage_text_update(
        client,
        previous,
        sequence,
        [TextOwnerUpdate {
            owner,
            expected,
            path,
            text,
        }],
    )?;
    Ok((submit_and_settle(client, &request)?, source))
}

pub(super) struct TextOwnerUpdate<'a> {
    pub owner: u8,
    pub expected: Option<ObjectRef>,
    pub path: &'a str,
    pub text: &'a str,
}

pub(super) fn stage_text_update<const N: usize>(
    client: &NativeUdsClient,
    previous: ViewRef,
    sequence: u64,
    updates: [TextOwnerUpdate<'_>; N],
) -> TestResult<(UpdateRequest, [ObjectRef; N])> {
    let operation = operation(sequence + 1);
    let ingress = client.begin_input(operation, &budget())?;
    let inputs = updates
        .iter()
        .map(|update| SourceInput {
            path: update.path.into(),
            bytes: update.text.as_bytes().to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            language: None,
        })
        .collect();
    let sources: [ObjectRef; N] = client
        .stage_sources(ingress, inputs, &budget())?
        .try_into()
        .map_err(|_| "Native staging returned a different source count")?;
    let before = client.read_view(ViewSelection::Exact(previous), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        updates
            .into_iter()
            .zip(sources)
            .map(|(update, source)| OwnerMutation {
                owner: OwnerKey {
                    producer: operation.producer,
                    stable_id: StableOwnerId([update.owner; 16]),
                },
                expected: update.expected,
                next: Some(source),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: operation.stream_epoch,
        predecessor: sequence - 1,
        sequence,
        payload: transition.owner_digest()?,
    });
    Ok((
        UpdateRequest {
            operation,
            expected_view: previous,
            kind: UpdateKind::SourceAdvance(transition),
        },
        sources,
    ))
}
