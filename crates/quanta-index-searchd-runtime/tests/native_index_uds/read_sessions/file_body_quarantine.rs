//! Selected Native FileBody reads through the public SDK.

use super::*;
use quanta_index_contract::native_protocol::NativeTextPreviewEvidence;
use quanta_index_core::index::{SourceRecordOwner, WorkBudgetDenial, WorkUsage};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;

#[derive(Clone, Copy)]
enum FileBodyFault {
    Modified,
    Missing,
    Symlink,
}

fn file_body_service(
    root: &std::path::Path,
    socket: &std::path::Path,
) -> TestResult<NativeIndexService> {
    // FileBody verification owns quarantine and boot recovery, not asynchronous
    // history retirement. Keep the exact views stable; Catalog retirement has
    // separate coverage for handing body incidents across view deletion.
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

fn publish_file_body_owner<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
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
    let (request, source) = prepare_text_owner_in_repository(
        client,
        expected,
        ingress,
        stable_id,
        path,
        text,
        Some(quanta_index_contract::RepoId::try_from("file-body")?),
    )?;
    Ok((published(&client.submit(request, &budget())?)?, source))
}

fn scrub_source_body(
    maintenance: &quanta_index_searchd_runtime::native::NativeIndexMaintenance,
    view: quanta_index_contract::index::ViewRef,
    owner: quanta_index_contract::index::OwnerKey,
) -> Result<Option<quanta_index_contract::index::ObjectRef>, IndexError> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or(IndexError::ResourceLimited)?;
    let mut work = WorkBudget::with_request(work_limits(), RequestBudgetV1::until(deadline));
    scrub_source_body_with_work(maintenance, view, owner, deadline, &mut work)
}

fn scrub_source_body_with_work(
    maintenance: &quanta_index_searchd_runtime::native::NativeIndexMaintenance,
    view: quanta_index_contract::index::ViewRef,
    owner: quanta_index_contract::index::OwnerKey,
    deadline: Instant,
    work: &mut WorkBudget,
) -> Result<Option<quanta_index_contract::index::ObjectRef>, IndexError> {
    loop {
        match maintenance.scrub_source_body(view, owner, work) {
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

#[test]
fn selected_file_body_modified_is_quarantined_across_restart() -> TestResult {
    assert_file_body_quarantine(FileBodyFault::Modified)
}

#[test]
fn selected_file_body_missing_is_quarantined_across_restart() -> TestResult {
    assert_file_body_quarantine(FileBodyFault::Missing)
}

#[test]
fn selected_file_body_symlink_is_quarantined_across_restart() -> TestResult {
    assert_file_body_quarantine(FileBodyFault::Symlink)
}

fn assert_file_body_quarantine(fault: FileBodyFault) -> TestResult {
    const NEEDLE: &str = "filebodywitness";
    const PATH: &str = "src/filebody.rs";
    const HEALTHY: &str = "healthysuccessor";

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let bind = || file_body_service(root.path(), &socket);
    let start_with_scrub = || -> TestResult<_> {
        let service = bind()?;
        let maintenance = service.maintenance_handle();
        // On failure, release the maintenance handle before joining the service.
        Ok((NativeServiceGuard::start(service), maintenance))
    };
    let writer = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(&socket);
    let configured = create_text_workspace(&client)?;
    let (selected, source) = publish_file_body_owner(&client, configured, 2, 1, PATH, NEEDLE)?;
    let query = || {
        client.search_text_preview(
            ViewSelection::Exact(selected),
            native_text_spec().instance,
            native_text_request(NEEDLE, 2),
            &budget(),
        )
    };
    let original = query()?;
    assert_eq!(original.ranking.view, selected);
    assert_eq!(original.ranking.matched, 1);
    let [hit] = original.previews.as_slice() else {
        return Err("healthy selected view did not return one preview".into());
    };
    assert_eq!(hit.source, source);
    assert!(matches!(
        hit.evidence,
        NativeTextPreviewEvidence::Excerpt(_)
    ));
    writer.stop()?;

    let store_root = root.path().join("native-objects");
    let body = {
        let store = FileObjectStore::open(&store_root)?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let stored = store.get(source, &mut work)?;
        let record =
            SourceRecordOwner::decode_json(&stored.payload, &mut work, MemoryLifetime::Retained)?;
        assert_eq!(record.path, PATH);
        assert_eq!(
            store.get(record.bytes, &mut work)?.payload,
            NEEDLE.as_bytes()
        );
        record.bytes
    };
    let object = native_object_files::object_file(&store_root, body)?;
    let original_permissions = fs::metadata(&object)?.permissions();
    let original_bytes = fs::read(&object)?;
    let outside = tempfile::tempdir()?;
    let target = outside.path().join("original-object");
    fs::write(&target, &original_bytes)?;

    let (cold, maintenance) = start_with_scrub()?;
    let held = client
        .open_read_session(ViewSelection::Exact(selected), [55; 32], &budget())
        .map_err(|error| format!("hold selected view before corruption: {error}"))?;
    let ingress = client.begin_input(operation(3), &budget())?;
    let next = stage_text_source_in_ingress(
        &client,
        ingress,
        PATH,
        HEALTHY,
        Some(quanta_index_contract::RepoId::try_from("file-body")?),
    )?;
    let update = prepare_text_owner_mutation_in_ingress(
        &client,
        selected,
        ingress,
        1,
        Some(source),
        Some(next),
    )?;
    let successor = published(&client.submit(update, &budget())?)?;
    assert_eq!(query()?.ranking.view, selected);
    let held_text = client
        .search_text_in_read_session(
            &held,
            native_text_spec().instance,
            native_text_request(NEEDLE, 2),
            &budget(),
        )
        .map_err(|error| format!("held text before damage: {error}"))?;
    assert_eq!(held_text.view, selected);
    assert_eq!(held_text.matched, 1);
    let path_query = || {
        client.search_text_paths_in_read_session(
            &held,
            native_text_spec().instance,
            native_text_request("select:path filebody", 2),
            &budget(),
        )
    };
    assert_eq!(
        path_query()
            .map_err(|error| format!("held path before damage: {error}"))?
            .ranked
            .ranking
            .matched,
        1
    );
    let parent = object.parent().ok_or("FileBody object has no parent")?;
    match fault {
        FileBodyFault::Modified => {
            let mut damaged = original_bytes.clone();
            *damaged.last_mut().ok_or("empty FileBody object")? ^= 1;
            let mut replacement = tempfile::NamedTempFile::new_in(parent)?;
            replacement.write_all(&damaged)?;
            replacement
                .as_file()
                .set_permissions(original_permissions.clone())?;
            let _replaced = replacement.persist(&object)?;
            assert_eq!(fs::read(&object)?, damaged);
        }
        FileBodyFault::Missing => fs::remove_file(&object)?,
        FileBodyFault::Symlink => {
            let replacement_dir = tempfile::tempdir_in(parent)?;
            let replacement = replacement_dir.path().join("replacement-link");
            symlink(&target, &replacement)?;
            fs::rename(replacement, &object)?;
            assert!(fs::symlink_metadata(&object)?.is_symlink());
        }
    }
    let result = query();
    assert!(
        matches!(result, Err(NativeUdsError::Refused(ref error))
            if error.code == NativeIndexErrorCode::CorruptObject),
        "the SDK did not report selected FileBody corruption: {result:?}"
    );
    assert_eq!(fs::read(&target)?, original_bytes);
    assert_eq!(
        path_query()
            .map_err(|error| format!("held path before scrub: {error}"))?
            .ranked
            .ranking
            .matched,
        1
    );
    assert_eq!(
        scrub_source_body(&maintenance, selected, owner(1))?,
        Some(body),
        "scrub did not persist the selected FileBody identity"
    );
    let held_result = path_query();
    assert!(
        matches!(held_result, Err(NativeUdsError::Refused(ref error))
            if error.code == NativeIndexErrorCode::CorruptObject),
        "the already held view escaped quarantine: {held_result:?}"
    );
    match client.release_read_session(&held, &budget()) {
        Ok((status, remaining)) => {
            assert_eq!(status.state, NativeReadSessionState::Released);
            assert!(!remaining);
        }
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
        {
            // The service can release the failed ticket before this request.
            assert_eq!(view_pin_count(root.path(), selected.sequence)?, 0);
        }
        Err(error) => return Err(format!("release quarantined view: {error}").into()),
    }
    drop(maintenance);
    cold.stop()?;

    if !matches!(fault, FileBodyFault::Missing) {
        fs::remove_file(&object)?;
    }
    fs::write(&object, &original_bytes)?;
    fs::set_permissions(&object, original_permissions)?;
    assert!(fs::symlink_metadata(&object)?.is_file());
    assert_eq!(fs::read(&object)?, original_bytes);
    assert_eq!(fs::read(target)?, original_bytes);
    {
        let store = FileObjectStore::open(&store_root)?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        assert_eq!(store.get(body, &mut work)?.payload, NEEDLE.as_bytes());
    }

    let (restarted, maintenance) = start_with_scrub()?;
    let fenced = query();
    assert!(
        matches!(fenced, Err(NativeUdsError::Refused(ref error))
            if error.code == NativeIndexErrorCode::CorruptObject),
        "the SDK served the restored but quarantined view: {fenced:?}"
    );
    assert_eq!(
        scrub_source_body(&maintenance, selected, owner(1))?,
        Some(body),
        "restored bytes erased the durable quarantine receipt"
    );
    assert_eq!(scrub_source_body(&maintenance, successor, owner(1))?, None);
    let healthy = client.search_text_preview(
        ViewSelection::Exact(successor),
        native_text_spec().instance,
        native_text_request(HEALTHY, 2),
        &budget(),
    )?;
    assert_eq!(healthy.ranking.matched, 1);
    assert_eq!(healthy.ranking.view, successor);
    assert_eq!(healthy.ranking.hits[0].source, next);
    drop(maintenance);
    restarted.stop()?;
    Ok(())
}

#[test]
fn shared_file_body_fault_fences_every_referencing_native_view() -> TestResult {
    const NEEDLE: &str = "sharedfilebodywitness";
    const PATH: &str = "src/shared.rs";
    const HEALTHY: &str = "unrelatedbodywitness";
    const POST_FAULT: &str = "postfaultbuildwitness";

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let bind = || file_body_service(root.path(), &socket);
    let writer = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(&socket);
    let configured = create_text_workspace(&client)?;
    let (origin, source) = publish_file_body_owner(&client, configured, 2, 1, PATH, NEEDLE)?;

    // A delta on another path inherits the original Source record and FileBody.
    let (sharing, _) = publish_file_body_owner(&client, origin, 3, 2, "src/other.rs", "otherbody")?;

    // Replace the shared body in the successor so its healthy read is independent.
    let ingress = client.begin_input(operation(4), &budget())?;
    let healthy_source = stage_text_source_in_ingress(
        &client,
        ingress,
        PATH,
        HEALTHY,
        Some(quanta_index_contract::RepoId::try_from("file-body")?),
    )?;
    assert_ne!(healthy_source, source);
    let update = prepare_text_owner_mutation_in_ingress(
        &client,
        sharing,
        ingress,
        1,
        Some(source),
        Some(healthy_source),
    )?;
    let unrelated = published(&client.submit(update, &budget())?)?;
    writer.stop()?;

    let store_root = root.path().join("native-objects");
    let body = {
        let store = FileObjectStore::open(&store_root)?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let stored = store.get(source, &mut work)?;
        let record =
            SourceRecordOwner::decode_json(&stored.payload, &mut work, MemoryLifetime::Retained)?;
        assert_eq!(record.path, PATH);
        assert_eq!(
            store.get(record.bytes, &mut work)?.payload,
            NEEDLE.as_bytes()
        );
        record.bytes
    };
    let object = native_object_files::object_file(&store_root, body)?;
    let original_bytes = fs::read(&object)?;
    let original_permissions = fs::metadata(&object)?.permissions();

    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let (cold, maintenance) = (NativeServiceGuard::start(service), maintenance);
    let origin_held =
        client.open_read_session(ViewSelection::Exact(origin), [61; 32], &budget())?;
    let sharing_held =
        client.open_read_session(ViewSelection::Exact(sharing), [62; 32], &budget())?;
    for held in [&origin_held, &sharing_held] {
        let ranking = client
            .search_text_paths_in_read_session(
                held,
                native_text_spec().instance,
                native_text_request("select:path shared", 2),
                &budget(),
            )?
            .ranked
            .ranking;
        let [hit] = ranking.hits.as_slice() else {
            return Err("the retained view did not serve exactly one shared path".into());
        };
        assert_eq!(hit.source, source);
    }

    let mut damaged = original_bytes.clone();
    *damaged.last_mut().ok_or("empty shared FileBody object")? ^= 1;
    let mut replacement =
        tempfile::NamedTempFile::new_in(object.parent().ok_or("no object parent")?)?;
    replacement.write_all(&damaged)?;
    replacement
        .as_file()
        .set_permissions(original_permissions.clone())?;
    let _replaced = replacement.persist(&object)?;

    // The public content door must independently observe the physical fault.
    // Scrub is the maintenance authority that turns that finding into the
    // retained-view fence; a failed receipt write cannot be inferred from the
    // maintenance call alone.
    for view in [origin, sharing] {
        let damaged = client.search_text_preview(
            ViewSelection::Exact(view),
            native_text_spec().instance,
            native_text_request(NEEDLE, 2),
            &budget(),
        );
        assert!(
            matches!(damaged, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "public content read did not report the shared FileBody damage: {damaged:?}"
        );
    }

    // The body incident and pending format marker consume the first two writes.
    // Refuse the first per-view receipt through the original WorkBudget, without
    // bypassing the service's managed SQLite VFS or mutating its tracked inode.
    let mut fanout_limits = work_limits();
    fanout_limits.writes = 2;
    fanout_limits.mutations = 2;
    let fanout_deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or(IndexError::ResourceLimited)?;
    let mut fanout_work =
        WorkBudget::with_request(fanout_limits, RequestBudgetV1::until(fanout_deadline));
    let failure = scrub_source_body_with_work(
        &maintenance,
        origin,
        owner(1),
        fanout_deadline,
        &mut fanout_work,
    );
    assert!(
        matches!(failure, Err(IndexError::ResourceLimited)),
        "{failure:?}"
    );
    assert_eq!(fanout_work.first_denial(), Some(WorkBudgetDenial::Writes));
    assert_eq!(fanout_work.usage().writes, 2);
    let catalog = rusqlite::Connection::open_with_flags(
        root.path().join("catalog/catalog-v1.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let receipts: i64 = catalog.query_row(
        "SELECT count(*) FROM native_view_source_body_quarantine_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(receipts, 0, "partial quarantine survived a write failure");
    let pending: i64 = catalog.query_row(
        "SELECT pending FROM native_source_body_incident_v1 WHERE object=?1",
        [body.0.0.as_slice()],
        |row| row.get(0),
    )?;
    assert_eq!(
        pending, 1,
        "receipt failure discarded the durable body incident"
    );
    for held in [&origin_held, &sharing_held] {
        let result = client.search_text_paths_in_read_session(
            held,
            native_text_spec().instance,
            native_text_request("select:path shared", 2),
            &budget(),
        );
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "held view escaped the transient FileBody fence: {result:?}"
        );
    }
    for view in [origin, sharing] {
        let result = client.open_read_session(ViewSelection::Exact(view), [66; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "new exact pin escaped the transient FileBody fence: {result:?}"
        );
    }
    let healthy_before_retry = client.search_text_preview(
        ViewSelection::Exact(unrelated),
        native_text_spec().instance,
        native_text_request(HEALTHY, 2),
        &budget(),
    )?;
    assert_eq!(healthy_before_retry.ranking.view, unrelated);
    assert_eq!(healthy_before_retry.ranking.matched, 1);
    assert_eq!(healthy_before_retry.ranking.hits[0].source, healthy_source);

    // The quarantine finding is scoped to the damaged body, not the writer.
    // Prove a new Source publication and its content-bearing exact read while
    // the failed receipt leaves a durable pending incident and a process-local fence.
    let (post_fault, post_fault_source) =
        publish_file_body_owner(&client, unrelated, 5, 3, "src/post-fault.rs", POST_FAULT)?;
    let healthy_build = client.search_text_preview(
        ViewSelection::Exact(post_fault),
        native_text_spec().instance,
        native_text_request(POST_FAULT, 2),
        &budget(),
    )?;
    assert_eq!(healthy_build.ranking.view, post_fault);
    assert_eq!(healthy_build.ranking.matched, 1);
    assert_eq!(healthy_build.ranking.hits[0].source, post_fault_source);
    for view in [origin, sharing] {
        let result = client.open_read_session(ViewSelection::Exact(view), [68; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "unrelated publication cleared the transient FileBody fence: {result:?}"
        );
    }

    assert_eq!(
        scrub_source_body(&maintenance, origin, owner(1))?,
        Some(body)
    );
    let warm_receipts: i64 = catalog.query_row(
        "SELECT count(*) FROM native_view_source_body_quarantine_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        warm_receipts, 2,
        "the same-process retry did not complete both shared-view receipts"
    );
    let warm_pending: i64 = catalog.query_row(
        "SELECT pending FROM native_source_body_incident_v1 WHERE object=?1",
        [body.0.0.as_slice()],
        |row| row.get(0),
    )?;
    assert_eq!(
        warm_pending, 0,
        "the same-process retry left the body incident pending for boot"
    );
    drop(catalog);

    // The earlier transient-fence read may retire the failed ticket. A live
    // ticket must still observe the durable fence; a retired one is unavailable.
    for held in [&origin_held, &sharing_held] {
        let result = client.search_text_paths_in_read_session(
            held,
            native_text_spec().instance,
            native_text_request("select:path shared", 2),
            &budget(),
        );
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject
                    || error.code == NativeIndexErrorCode::SnapshotUnavailable),
            "held view escaped shared FileBody quarantine: {result:?}"
        );
    }
    for view in [origin, sharing] {
        let result = client.open_read_session(ViewSelection::Exact(view), [63; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "new exact pin escaped shared FileBody quarantine: {result:?}"
        );
    }
    for held in [&origin_held, &sharing_held] {
        match client.release_read_session(held, &budget()) {
            Ok((status, remaining)) => {
                assert_eq!(status.state, NativeReadSessionState::Released);
                assert!(!remaining);
            }
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable => {}
            Err(error) => return Err(format!("release quarantined shared view: {error}").into()),
        }
    }
    drop(maintenance);
    cold.stop()?;

    // Restoring the physical bytes must not erase either durable refusal.
    fs::remove_file(&object)?;
    fs::write(&object, &original_bytes)?;
    fs::set_permissions(&object, original_permissions)?;
    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let (restarted, maintenance) = (NativeServiceGuard::start(service), maintenance);
    for view in [origin, sharing] {
        let result = client.open_read_session(ViewSelection::Exact(view), [64; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "restarted exact pin escaped shared FileBody quarantine: {result:?}"
        );
    }
    let healthy = client.search_text_preview(
        ViewSelection::Exact(post_fault),
        native_text_spec().instance,
        native_text_request(POST_FAULT, 2),
        &budget(),
    )?;
    assert_eq!(healthy.ranking.view, post_fault);
    assert_eq!(healthy.ranking.matched, 1);
    assert_eq!(healthy.ranking.hits[0].source, post_fault_source);
    assert_eq!(scrub_source_body(&maintenance, post_fault, owner(3))?, None);
    drop(maintenance);
    restarted.stop()?;
    Ok(())
}

#[test]
fn shared_file_body_fanout_failure_boots_fail_closed_without_retry() -> TestResult {
    const NEEDLE: &str = "sharedbootfailurewitness";
    const PATH: &str = "src/shared-boot.rs";
    const HEALTHY: &str = "healthybootfailurewitness";

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let bind = || file_body_service(root.path(), &socket);
    let writer = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(&socket);
    let configured = create_text_workspace(&client)?;
    let (origin, source) = publish_file_body_owner(&client, configured, 2, 1, PATH, NEEDLE)?;
    let (sharing, _) = publish_file_body_owner(&client, origin, 3, 2, "src/other.rs", "otherbody")?;
    let ingress = client.begin_input(operation(4), &budget())?;
    let healthy_source = stage_text_source_in_ingress(
        &client,
        ingress,
        PATH,
        HEALTHY,
        Some(quanta_index_contract::RepoId::try_from("file-body")?),
    )?;
    let update = prepare_text_owner_mutation_in_ingress(
        &client,
        sharing,
        ingress,
        1,
        Some(source),
        Some(healthy_source),
    )?;
    let unrelated = published(&client.submit(update, &budget())?)?;
    writer.stop()?;

    let store_root = root.path().join("native-objects");
    let body = {
        let store = FileObjectStore::open(&store_root)?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let stored = store.get(source, &mut work)?;
        SourceRecordOwner::decode_json(&stored.payload, &mut work, MemoryLifetime::Temporary)?.bytes
    };
    let object = native_object_files::object_file(&store_root, body)?;
    let mut damaged = fs::read(&object)?;
    *damaged.last_mut().ok_or("empty shared FileBody object")? ^= 1;
    let permissions = fs::metadata(&object)?.permissions();
    let mut replacement =
        tempfile::NamedTempFile::new_in(object.parent().ok_or("no object parent")?)?;
    replacement.write_all(&damaged)?;
    replacement.as_file().set_permissions(permissions)?;
    let _replaced = replacement.persist(&object)?;

    let catalog_path = root.path().join("catalog/catalog-v1.sqlite");
    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let (running, maintenance) = (NativeServiceGuard::start(service), maintenance);
    for view in [origin, sharing] {
        let damaged = client.search_text_preview(
            ViewSelection::Exact(view),
            native_text_spec().instance,
            native_text_request(NEEDLE, 2),
            &budget(),
        );
        assert!(
            matches!(damaged, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "public content read did not report damage before fan-out failure: {damaged:?}"
        );
    }
    // Commit the durable incident and its capture-format marker, then refuse
    // the first per-view receipt through the original managed write path.
    let mut fanout_limits = work_limits();
    fanout_limits.writes = 2;
    fanout_limits.mutations = 2;
    let fanout_deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or(IndexError::ResourceLimited)?;
    let mut fanout_work =
        WorkBudget::with_request(fanout_limits, RequestBudgetV1::until(fanout_deadline));
    let failure = scrub_source_body_with_work(
        &maintenance,
        origin,
        owner(1),
        fanout_deadline,
        &mut fanout_work,
    );
    assert!(
        matches!(failure, Err(IndexError::ResourceLimited)),
        "{failure:?}"
    );
    assert_eq!(fanout_work.first_denial(), Some(WorkBudgetDenial::Writes));
    assert_eq!(fanout_work.usage().writes, 2);
    let catalog = rusqlite::Connection::open_with_flags(
        &catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let receipts: i64 = catalog.query_row(
        "SELECT count(*) FROM native_view_source_body_quarantine_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        receipts, 0,
        "a failed receipt fan-out committed a partial view fence"
    );
    let pending: i64 = catalog.query_row(
        "SELECT pending FROM native_source_body_incident_v1 WHERE object=?1",
        [body.0.0.as_slice()],
        |row| row.get(0),
    )?;
    assert_eq!(
        pending, 1,
        "the durable body incident did not survive receipt failure"
    );
    drop(catalog);
    drop(maintenance);
    running.stop()?;

    // No maintenance retry occurs. Bind must complete the durable seed before
    // exposing the listener, including when the catalog already used the
    // current capture format before this incident.
    let restarted = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(&socket);
    for view in [origin, sharing] {
        let result = client.open_read_session(ViewSelection::Exact(view), [67; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "restart without a maintenance retry exposed a shared damaged view: {result:?}"
        );
    }
    let healthy = client.search_text_preview(
        ViewSelection::Exact(unrelated),
        native_text_spec().instance,
        native_text_request(HEALTHY, 2),
        &budget(),
    )?;
    assert_eq!(healthy.ranking.view, unrelated);
    assert_eq!(healthy.ranking.matched, 1);
    assert_eq!(healthy.ranking.hits[0].source, healthy_source);
    restarted.stop()?;

    let catalog = rusqlite::Connection::open(catalog_path)?;
    let receipts: i64 = catalog.query_row(
        "SELECT count(*) FROM native_view_source_body_quarantine_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        receipts, 2,
        "boot did not complete both shared-view receipts"
    );
    Ok(())
}

#[test]
fn scrub_proves_a_second_owner_in_a_quarantined_view() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let bind = || file_body_service(root.path(), &socket);
    let running = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(&socket);
    let configured = create_text_workspace(&client)?;
    let (a_only, source_a) =
        publish_file_body_owner(&client, configured, 2, 1, "src/a.rs", "firstbodywitness")?;
    let (shared, source_b) =
        publish_file_body_owner(&client, a_only, 3, 2, "src/b.rs", "secondbodywitness")?;
    let ingress = client.begin_input(operation(4), &budget())?;
    let remove_a =
        prepare_text_owner_mutation_in_ingress(&client, shared, ingress, 1, Some(source_a), None)?;
    let b_only = published(&client.submit(remove_a, &budget())?)?;
    let ingress = client.begin_input(operation(5), &budget())?;
    let remove_b =
        prepare_text_owner_mutation_in_ingress(&client, b_only, ingress, 2, Some(source_b), None)?;
    let healthy = published(&client.submit(remove_b, &budget())?)?;
    running.stop()?;

    let object_root = root.path().join("native-objects");
    let damage = |source| -> TestResult<_> {
        let store = FileObjectStore::open(&object_root)?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let record = store.get(source, &mut work)?;
        let body =
            SourceRecordOwner::decode_json(&record.payload, &mut work, MemoryLifetime::Temporary)?
                .bytes;
        let path = native_object_files::object_file(&object_root, body)?;
        let mut bytes = fs::read(&path)?;
        *bytes.last_mut().ok_or("empty FileBody fixture")? ^= 1;
        let mut replacement =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("no object parent")?)?;
        replacement.write_all(&bytes)?;
        replacement
            .as_file()
            .set_permissions(fs::metadata(&path)?.permissions())?;
        let _persisted = replacement.persist(path)?;
        Ok(body)
    };
    let body_a = damage(source_a)?;
    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let (running, maintenance) = (NativeServiceGuard::start(service), maintenance);
    assert_eq!(
        scrub_source_body(&maintenance, shared, owner(1))?,
        Some(body_a)
    );

    assert_eq!(scrub_source_body(&maintenance, shared, owner(2))?, None);
    drop(maintenance);
    running.stop()?;

    let body_b = damage(source_b)?;
    let service = bind()?;
    let maintenance = service.maintenance_handle();
    let (running, maintenance) = (NativeServiceGuard::start(service), maintenance);
    let b_pin = client.open_read_session(ViewSelection::Exact(b_only), [68; 32], &budget())?;
    assert_eq!(
        scrub_source_body(&maintenance, shared, owner(2))?,
        Some(body_b)
    );
    let held = client.search_text_paths_in_read_session(
        &b_pin,
        native_text_spec().instance,
        native_text_request("select:path b.rs", 2),
        &budget(),
    );
    assert!(
        matches!(held, Err(NativeUdsError::Refused(ref error))
        if error.code == NativeIndexErrorCode::CorruptObject),
        "{held:?}"
    );
    drop(maintenance);
    running.stop()?;

    let restarted = NativeServiceGuard::start(bind()?);
    let client = NativeUdsClient::new(&socket);
    for damaged in [a_only, shared, b_only] {
        let result = client.open_read_session(ViewSelection::Exact(damaged), [69; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
            if error.code == NativeIndexErrorCode::CorruptObject),
            "{result:?}"
        );
    }
    let healthy_pin =
        client.open_read_session(ViewSelection::Exact(healthy), [70; 32], &budget())?;
    let _released = client.release_read_session(&healthy_pin, &budget())?;
    restarted.stop()?;
    Ok(())
}

#[test]
fn legacy_shared_file_body_receipt_fans_out_before_first_request() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let writer = NativeServiceGuard::start(file_body_service(root.path(), &socket)?);
    let client = NativeUdsClient::new(&socket);
    let configured = create_text_workspace(&client)?;
    let (origin, source) =
        publish_file_body_owner(&client, configured, 2, 1, "src/shared.rs", "sharedbootbody")?;
    let (sibling, _) =
        publish_file_body_owner(&client, origin, 3, 2, "src/other.rs", "otherbootbody")?;
    writer.stop()?;

    let body = {
        let store = FileObjectStore::open(root.path().join("native-objects"))?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let source = store.get(source, &mut work)?;
        SourceRecordOwner::decode_json(&source.payload, &mut work, MemoryLifetime::Temporary)?.bytes
    };
    let path = root.path().join("catalog/catalog-v1.sqlite");
    let connection = rusqlite::Connection::open(&path)?;
    let legacy_format: (i64, i64, i64, Vec<u8>, i64, Vec<u8>) = connection.query_row(
        "SELECT singleton,state_version,capture_version,hash_domain,registration_codec,root_nonce
         FROM native_capture_format_v1 WHERE singleton=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    let mut format_digest = Sha256::new();
    format_digest.update(b"qidx.native.capture-format.view-quarantine.v1\0");
    format_digest.update(serde_json::to_vec(&legacy_format)?);
    let legacy_format_digest: [u8; 32] = format_digest.finalize().into();
    let mut import_format = Sha256::new();
    import_format.update(b"qidx.native.capture-format.body-incident.import.v1\0");
    import_format.update(serde_json::to_vec(&legacy_format)?);
    let import_format_digest: [u8; 32] = import_format.finalize().into();
    connection.execute_batch(
        "CREATE TABLE native_view_source_body_quarantine_v1 (
            workspace BLOB NOT NULL,
            epoch INTEGER NOT NULL CHECK(epoch >= 0),
            sequence INTEGER NOT NULL CHECK(sequence >= 0),
            identity_digest BLOB NOT NULL CHECK(typeof(identity_digest) = 'blob' AND length(identity_digest) = 32),
            object BLOB NOT NULL CHECK(typeof(object) = 'blob' AND length(object) = 32),
            row_digest BLOB NOT NULL CHECK(typeof(row_digest) = 'blob' AND length(row_digest) = 32),
            PRIMARY KEY(workspace, epoch, sequence),
            FOREIGN KEY(workspace, epoch, sequence) REFERENCES native_view_v1(workspace, epoch, sequence) ON DELETE CASCADE
        ) WITHOUT ROWID;",
    )?;
    assert_eq!(
        connection.execute(
            "UPDATE native_capture_format_v1 SET row_digest=?1 WHERE singleton=1",
            [legacy_format_digest.as_slice()],
        )?,
        1
    );
    let workspace_bytes = serde_json::to_vec(&origin.workspace)?;
    let epoch = i64::try_from(origin.epoch)?;
    let sequence = i64::try_from(origin.sequence)?;
    let mut receipt_digest = Sha256::new();
    receipt_digest.update(b"qidx.native.view-source-body-quarantine.v1\0");
    receipt_digest.update(serde_json::to_vec(&(
        &workspace_bytes,
        epoch,
        sequence,
        origin.digest.0,
        body.0.0,
    ))?);
    let legacy_receipt: [u8; 32] = receipt_digest.finalize().into();
    assert_eq!(
        connection.execute(
            "INSERT INTO native_view_source_body_quarantine_v1
             (workspace,epoch,sequence,identity_digest,object,row_digest)
             VALUES (?1,?2,?3,?4,?5,zeroblob(32))",
            rusqlite::params![
                workspace_bytes,
                epoch,
                sequence,
                origin.digest.0.as_slice(),
                body.0.0.as_slice()
            ],
        )?,
        1
    );
    drop(connection);

    // A defective legacy receipt must fail the bind without exposing a listener.
    let next_socket = root.path().join("reopened.sock");
    let Err(error) = file_body_service(root.path(), &next_socket) else {
        return Err("boot accepted an invalid legacy receipt".into());
    };
    assert!(
        error
            .to_string()
            .contains("complete native Source body quarantine")
    );
    assert!(!next_socket.exists());

    let connection = rusqlite::Connection::open(&path)?;
    let failed_format_digest: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_capture_format_v1 WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(failed_format_digest, import_format_digest);
    let incidents: i64 = connection.query_row(
        "SELECT count(*) FROM native_source_body_incident_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(incidents, 0, "failed migration left a body incident");
    assert_eq!(
        connection.execute(
            "UPDATE native_view_source_body_quarantine_v1 SET row_digest=?1
             WHERE workspace=?2 AND epoch=?3 AND sequence=?4",
            rusqlite::params![legacy_receipt.as_slice(), workspace_bytes, epoch, sequence],
        )?,
        1
    );
    drop(connection);
    let reopened = NativeServiceGuard::start(file_body_service(root.path(), &next_socket)?);
    let client = NativeUdsClient::new(&next_socket);
    for view in [origin, sibling] {
        let result = client.open_read_session(ViewSelection::Exact(view), [65; 32], &budget());
        assert!(
            matches!(result, Err(NativeUdsError::Refused(ref error))
                if error.code == NativeIndexErrorCode::CorruptObject),
            "first SDK call after boot did not enforce the legacy receipt: {result:?}"
        );
    }
    reopened.stop()?;
    Ok(())
}
