//! Selected Native member-root reproof through the production maintenance port.

use super::*;
use quanta_index_core::index::ports::{ImmutableObjectPort, PersistentMapPort};
use quanta_index_core::index::{WorkUsage, decode_member_binding, member_map_key};
use std::fs;

fn member_artifact_service(
    root: &std::path::Path,
    socket: &std::path::Path,
) -> TestResult<NativeIndexService> {
    let mut reclaim = test_reclaim_config();
    reclaim.interval = Duration::from_secs(1);
    read_session_service_with_reclaim(
        root,
        socket,
        Duration::from_secs(300),
        None,
        work_limits(),
        reclaim,
    )
}

fn scrub_member_artifact(
    maintenance: &quanta_index_searchd_runtime::native::NativeIndexMaintenance,
    view: quanta_index_contract::index::ViewRef,
    instance: IndexInstanceId,
) -> Result<Option<ObjectRef>, IndexError> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or(IndexError::ResourceLimited)?;
    let mut work = WorkBudget::with_request(work_limits(), RequestBudgetV1::until(deadline));
    loop {
        match maintenance.scrub_source_member_artifact(view, instance, &mut work) {
            Err(IndexError::ResourceLimited)
                if work.first_denial().is_none()
                    && work.usage() == WorkUsage::default()
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => return result,
        }
    }
}

fn assert_exact_view_refused(
    client: &NativeUdsClient<&std::path::Path>,
    selected: quanta_index_contract::index::ViewRef,
    label: &str,
) -> TestResult {
    let result = client.search_text(
        ViewSelection::Exact(selected),
        native_text_spec().instance,
        native_text_request("rootquarantine", 1),
        &budget(),
    );
    assert!(
        matches!(result, Err(NativeUdsError::Refused(ref error))
            if error.code == NativeIndexErrorCode::CorruptObject),
        "{label}: {result:?}"
    );
    Ok(())
}

#[test]
fn scrub_reproves_a_second_member_root_in_an_already_quarantined_view() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let bind = || member_artifact_service(root.path(), &socket);
    let writer = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(socket.as_path());
    let NativeIndexResponse::Created(genesis) = client
        .request(
            NativeIndexRequest::CreateWorkspace {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )
        .map_err(|error| format!("create two-member workspace: {error}"))?
    else {
        return Err("two-member workspace genesis reply differs".into());
    };
    let ingress = client
        .begin_input(operation(1), &budget())
        .map_err(|error| format!("begin two-member profile: {error}"))?;
    let profile = client
        .stage_profile(
            ingress,
            IndexProfile {
                instances: vec![literal_spec(), native_text_spec()],
            },
            &budget(),
        )
        .map_err(|error| format!("stage two-member profile: {error}"))?;
    let configured = published(
        &client
            .submit(
                UpdateRequest {
                    operation: operation(1),
                    expected_view: genesis,
                    kind: UpdateKind::Reconfigure { profile },
                },
                &budget(),
            )
            .map_err(|error| format!("submit two-member profile: {error}"))?,
    )
    .map_err(|error| format!("publish two-member profile: {error}"))?;
    let (selected, _source) = publish_text_owner(
        &client,
        configured,
        2,
        1,
        "src/two-roots.rs",
        "two-root-witness",
    )
    .map_err(|error| format!("publish two-member Source: {error}"))?;
    let view = client.read_view(ViewSelection::Exact(selected), &budget())?;
    writer.stop()?;

    let object_root = root.path().join("native-objects");
    let (first, second) = {
        let store = FileObjectStore::open(&object_root)?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let artifact =
            |instance: IndexInstanceId, work: &mut WorkBudget| -> TestResult<ObjectRef> {
                let member_ref = store
                    .lookup_object(view.members, &member_map_key(instance), work)?
                    .ok_or("selected member absent")?;
                let member = store.get(member_ref, work)?;
                Ok(decode_member_binding(&member.payload, work)?.artifact)
            };
        (
            artifact(literal_spec().instance, &mut work)?,
            artifact(native_text_spec().instance, &mut work)?,
        )
    };
    assert_ne!(first, second);
    let damage = |reference| -> TestResult<(std::path::PathBuf, Vec<u8>)> {
        let path = native_object_files::object_file(&object_root, reference)?;
        let original = fs::read(&path)?;
        let mut changed = original.clone();
        *changed.last_mut().ok_or("empty member root")? ^= 1;
        fs::write(&path, changed)?;
        Ok((path, original))
    };

    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let running = NativeServiceGuard::start(service);
    let (first_path, first_wire) = damage(first)?;
    assert_eq!(
        scrub_member_artifact(&maintenance, selected, literal_spec().instance)?,
        Some(first)
    );
    assert_exact_view_refused(&client, selected, "first root did not fence normal reads")?;
    let (second_path, second_wire) = damage(second)?;
    assert_eq!(
        scrub_member_artifact(&maintenance, selected, native_text_spec().instance)?,
        Some(second)
    );
    drop(maintenance);
    running.stop()?;

    fs::write(first_path, first_wire)?;
    fs::write(second_path, second_wire)?;
    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let reopened = NativeServiceGuard::start(service);
    assert_eq!(
        scrub_member_artifact(&maintenance, selected, native_text_spec().instance)?,
        Some(second),
        "restoring bytes erased the second durable object incident"
    );
    drop(maintenance);
    reopened.stop()?;
    Ok(())
}

#[test]
fn scrub_quarantines_a_tampered_text_root_before_page_expansion() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let first = NativeServiceGuard::start(member_artifact_service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket.as_path());
    let configured = create_text_workspace(&client)?;
    let (selected, _source) = publish_text_owner(
        &client,
        configured,
        2,
        1,
        "src/root-quarantine.rs",
        "rootquarantine",
    )?;
    let profile = client
        .read_view(ViewSelection::Exact(selected), &budget())?
        .profile;
    let sibling = published(&client.submit(
        UpdateRequest {
            operation: operation(3),
            expected_view: selected,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let healthy = client.search_text(
        ViewSelection::Exact(selected),
        native_text_spec().instance,
        native_text_request("rootquarantine", 1),
        &budget(),
    )?;
    assert_eq!(healthy.view, selected);
    assert_eq!(healthy.matched, 1);
    let artifact = healthy.artifact;
    let sibling_healthy = client.search_text(
        ViewSelection::Exact(sibling),
        native_text_spec().instance,
        native_text_request("rootquarantine", 1),
        &budget(),
    )?;
    assert_eq!(sibling_healthy.artifact, artifact);
    assert_eq!(sibling_healthy.matched, 1);
    first.stop()?;

    let store_root = root.path().join("native-objects");
    let object = native_object_files::object_file(&store_root, artifact)?;
    let original = fs::read(&object)?;
    let mut damaged = original.clone();
    let last = damaged
        .last_mut()
        .ok_or("selected Text root object is empty")?;
    *last ^= 0x01;
    fs::write(&object, &damaged)?;

    let service = member_artifact_service(root.path(), &socket)?;
    let maintenance = service.maintenance_handle();
    let damaged_service = NativeServiceGuard::start(service);
    assert_exact_view_refused(&client, selected, "damaged root was queryable")?;
    assert_exact_view_refused(&client, sibling, "damaged shared root was queryable")?;
    assert_eq!(
        scrub_member_artifact(&maintenance, selected, native_text_spec().instance)?,
        Some(artifact)
    );
    assert_exact_view_refused(&client, selected, "scrub did not fence exact view")?;
    assert_exact_view_refused(&client, sibling, "scrub did not fence sibling view")?;

    let catalog_path = quanta_index_catalog::catalog_dir(root.path())
        .join(quanta_index_catalog::CATALOG_FILE_NAME);
    let catalog = rusqlite::Connection::open_with_flags(
        &catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (receipt_object, pending): (Vec<u8>, i64) = catalog.query_row(
        "SELECT q.object, i.pending
         FROM native_view_source_body_quarantine_v1 AS q
         JOIN native_source_body_incident_v1 AS i ON i.object=q.object
         WHERE q.workspace=?1 AND q.epoch=?2 AND q.sequence=?3",
        rusqlite::params![
            serde_json::to_vec(&selected.workspace)?,
            i64::try_from(selected.epoch)?,
            i64::try_from(selected.sequence)?
        ],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(receipt_object.as_slice(), artifact.0.0.as_slice());
    assert_eq!(pending, 0, "member-root incident fan-out stayed pending");
    let receipt_count: i64 = catalog.query_row(
        "SELECT COUNT(*)
         FROM native_view_source_body_quarantine_v1 AS q
         JOIN native_source_body_incident_v1 AS i ON i.object=q.object
         WHERE q.object=?1 AND i.pending=0",
        rusqlite::params![artifact.0.0.as_slice()],
        |row| row.get(0),
    )?;
    assert_eq!(
        receipt_count, 2,
        "shared member root did not fence both views"
    );
    drop(catalog);
    drop(maintenance);
    damaged_service.stop()?;

    fs::write(&object, &original)?;
    let reopened = NativeServiceGuard::start(member_artifact_service(root.path(), &socket)?);
    assert_exact_view_refused(
        &client,
        selected,
        "repaired root bypassed durable quarantine after restart",
    )?;
    assert_exact_view_refused(
        &client,
        sibling,
        "repaired shared root bypassed durable quarantine after restart",
    )?;
    reopened.stop()?;
    Ok(())
}
