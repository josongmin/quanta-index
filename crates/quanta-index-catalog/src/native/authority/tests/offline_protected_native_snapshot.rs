use super::offline_protected_routes::{
    in_second_workspace, protected_budget_v1, route_set, second_workspace, stage_routes,
    two_published_heads,
};
use super::*;

#[test]
fn selected_native_snapshot_exports_terminal_ledger_and_survives_retired_request() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, primary_operation, primary, prior_source_operation, prior_source) =
        two_published_heads(root.path())?;

    let mut source_operation = operation(20_010, 210);
    source_operation.key.workspace = second_workspace();
    source_operation.source_mode = SourceOperationMode::Advance;
    let event = SourceEvent {
        producer: source_operation.key.producer,
        stream: source_operation.key.stream,
        stream_epoch: source_operation.key.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: digest_id(211),
    };
    let attempt = admit_new_events(&catalog, source_operation, &[event])?;
    let frontier = object(212, vec![]);
    reserve_confirm(&catalog, attempt, std::slice::from_ref(&frontier))?;
    let mut current = in_second_workspace(view(2, prior_source.profile, None));
    current.source.frontiers = MapRoot(Some(frontier.reference));
    current.identity.digest = current.identity_digest()?;
    check_eq!(
        commit_empty(
            &catalog,
            PreparedPublication {
                attempt,
                expected: prior_source.identity,
                next: current.clone(),
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: vec![event],
                projection_events: vec![],
                expected_source: Some(prior_source.source),
                source_mode: SourceOperationMode::Advance,
            },
        )?,
        OperationStatus::Published(current.identity)
    );
    let mut refused = operation(20_011, 213);
    refused.key.workspace = second_workspace();
    refused.source_mode = SourceOperationMode::Advance;
    let refused_events = (20_u8..70)
        .map(|stream| SourceEvent {
            producer: refused.key.producer,
            stream: StreamId(id(stream)),
            stream_epoch: refused.key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(stream),
        })
        .collect::<Vec<_>>();
    let refused_attempt = admit_new_events(&catalog, refused, &refused_events)?;
    check_eq!(
        catalog.refuse(refused_attempt, Refusal::InvalidInput)?,
        OperationStatus::Refused(Refusal::InvalidInput)
    );
    let direct = Connection::open(catalog.path())?;
    let retained_row_bytes: i64 = direct.query_row(
        "SELECT length(payload) FROM native_operation_v1 WHERE operation=?1",
        params![operation_key(refused.key)?],
        |row| row.get(0),
    )?;
    check!(retained_row_bytes > 4096);
    check_eq!(
        direct.execute(
            "DELETE FROM native_operation_object_v1 WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND object=?2",
            params![operation_key(refused.key)?, object_key(refused.request)],
        )?,
        1
    );
    check_eq!(
        direct.execute(
            "DELETE FROM native_object_v1 WHERE object=?1",
            params![object_key(refused.request)],
        )?,
        1
    );
    drop(direct);

    let mut routes = route_set(
        primary_operation,
        primary.clone(),
        source_operation,
        current,
    )?;
    let _primary_outside_protected_set = routes.heads.remove(0);
    routes.validate()?;
    let (ingress, route_ref) = stage_routes(&catalog, &routes, operation(20_012, 214).key)?;
    let selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
        primary_operation.key,
        route_ref,
        &routes,
        ingress,
        &mut protected_budget_v1(),
    )?;
    let root_nonce = catalog.native_root_nonce(&mut protected_budget_v1())?;
    let guard = catalog.lock()?;
    check_eq!(
        read_only::verify_native_published_view_connection_v1(
            &guard,
            prior_source.identity,
            Some(&routes),
            &mut protected_budget_v1(),
        )?,
        prior_source
    );
    check!(
        read_only::verify_native_published_view_connection_v1(
            &guard,
            prior_source.identity,
            None,
            &mut protected_budget_v1(),
        )
        .is_err()
    );
    check!(matches!(
        read_only::visit_native_protected_snapshot_connection_v1(
            &guard,
            root_nonce,
            &selected,
            &routes,
            &mut admission_budget(),
            |_, _| Ok(()),
        ),
        Err(IndexError::ResourceLimited)
    ));
    read_only::visit_native_protected_snapshot_connection_v1(
        &guard,
        root_nonce,
        &selected,
        &routes,
        &mut protected_budget_v1(),
        |snapshot, budget| {
            budget.checkpoint()?;
            if snapshot.source_root_nonce != Digest(root_nonce)
                || snapshot.source_routes != route_ref
                || snapshot.operation_rows.len() != 3
                || snapshot.event_claims.len() != 51
                || snapshot.frontiers.len() != 1
                || snapshot.source_authorities.len() != 1
                || !snapshot.roots.contains(&route_ref)
                || snapshot.roots.contains(&refused.request)
            {
                return Err(IndexError::DigestConflict);
            }
            let keys = snapshot
                .operation_rows
                .iter()
                .map(|bytes| decode::<OperationRow>(bytes).map(|row| row.immutable.key))
                .collect::<Result<Vec<_>, _>>()?;
            if !keys.contains(&prior_source_operation.key)
                || !keys.contains(&source_operation.key)
                || !keys.contains(&refused.key)
            {
                return Err(IndexError::DigestConflict);
            }
            Ok(())
        },
    )?;
    drop(guard);

    let guard = catalog.lock()?;
    let _changed = guard.execute(
        "UPDATE native_frontier_v1 SET row_digest=zeroblob(32) WHERE workspace=?1",
        params![workspace_key(second_workspace())?],
    )?;
    check!(matches!(
        read_only::visit_native_protected_snapshot_connection_v1(
            &guard,
            root_nonce,
            &selected,
            &routes,
            &mut protected_budget_v1(),
            |_, _| Ok(()),
        ),
        Err(IndexError::DigestConflict)
    ));
    Ok(())
}
