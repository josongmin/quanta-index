use super::*;
use quanta_index_contract::index::{
    NativeProtectedRepoMapHeadV1, NativeProtectedSourceHeadV1, NativeProtectedSourceRoutesV1,
    native_protected_view_marker_bytes_v1, native_protected_view_marker_ref_v1,
};

#[derive(Clone, Default)]
struct ProtectedTestMemoryV1 {
    live: std::sync::Arc<std::sync::atomic::AtomicU64>,
    deny_bytes: Option<u64>,
}

impl quanta_index_core::index::MemoryLeaseOwner for ProtectedTestMemoryV1 {
    fn release(&self, bytes: u64) {
        assert!(
            self.live
                .fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |live| live.checked_sub(bytes),
                )
                .is_ok(),
            "protected test memory released more than reserved",
        );
    }
}

impl quanta_index_core::index::MemoryScopePort for ProtectedTestMemoryV1 {
    fn reserve(
        &self,
        bytes: u64,
        _lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        const LIMIT: u64 = 64 * 1024 * 1024;
        if self.deny_bytes == Some(bytes) {
            return Err(IndexError::ResourceLimited);
        }
        let _previous_live = self
            .live
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |live| live.checked_add(bytes).filter(|next| *next <= LIMIT),
            )
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            std::sync::Arc::new(self.clone()),
        ))
    }
}

pub(super) fn protected_budget_v1() -> WorkBudget {
    let mut budget = admission_budget();
    budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(
        std::sync::Arc::new(ProtectedTestMemoryV1::default()),
    )));
    budget
}

pub(super) fn second_workspace() -> WorkspaceKey {
    WorkspaceKey {
        slot: SlotId(id(236)),
        ..workspace()
    }
}

fn third_workspace() -> WorkspaceKey {
    WorkspaceKey {
        slot: SlotId(id(237)),
        ..workspace()
    }
}

pub(super) fn in_second_workspace(mut publication: CatalogView) -> CatalogView {
    publication.identity.workspace = second_workspace();
    publication.identity.digest = publication.identity_digest().expect("second view digest");
    publication
}

pub(super) fn two_published_heads(
    root: &Path,
) -> Result<
    (
        SqliteCatalog,
        ImmutableOperation,
        CatalogView,
        ImmutableOperation,
        CatalogView,
    ),
    Box<dyn Error>,
> {
    let (catalog, first_genesis) = setup(root)?;
    let profile = object(30, vec![]);
    let bootstrap = catalog.begin_bootstrap(
        second_workspace(),
        7,
        &GenesisIdentity(profile.clone()),
        &mut admission_budget(),
    )?;
    catalog.reserve_bootstrap(bootstrap, &profile)?;
    catalog.confirm_bootstrap(bootstrap, &profile)?;
    let second_genesis = in_second_workspace(view(0, profile.reference, None));
    check_eq!(
        catalog.publish_bootstrap(bootstrap, second_genesis.clone(), &mut admission_budget(),)?,
        second_genesis.identity
    );

    let first_operation = operation(20_001, 201);
    let first_attempt = admit_new(&catalog, first_operation)?;
    let first = view(1, first_genesis.profile, None);
    check_eq!(
        publish(&catalog, first_attempt, &first_genesis, first.clone())?,
        OperationStatus::Published(first.identity)
    );

    let mut second_operation = operation(20_002, 202);
    second_operation.key.workspace = second_workspace();
    let second_attempt = admit_new(&catalog, second_operation)?;
    let second = in_second_workspace(view(1, second_genesis.profile, None));
    check_eq!(
        publish(&catalog, second_attempt, &second_genesis, second.clone())?,
        OperationStatus::Published(second.identity)
    );
    Ok((catalog, first_operation, first, second_operation, second))
}

pub(super) fn route_set(
    first_operation: ImmutableOperation,
    first: CatalogView,
    second_operation: ImmutableOperation,
    second: CatalogView,
) -> Result<NativeProtectedSourceRoutesV1, Box<dyn Error>> {
    let heads = [
        ("repo-a", "rev-a", first_operation, first),
        ("repo-b", "rev-b", second_operation, second),
    ]
    .into_iter()
    .map(
        |(repo, revision, operation, publication)| NativeProtectedSourceHeadV1 {
            repo_id: quanta_index_contract::RepoId::new(repo).expect("repo"),
            revision_id: quanta_index_contract::RevisionId::new(revision).expect("revision"),
            source_operation: operation.clone(),
            source_publication: publication.clone(),
            operation,
            publication,
        },
    )
    .collect();
    let routes = NativeProtectedSourceRoutesV1 {
        native_origin: None,
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        heads,
        repomap_heads: Vec::new(),
    };
    routes.validate()?;
    Ok(routes)
}

pub(super) fn stage_routes(
    catalog: &SqliteCatalog,
    routes: &NativeProtectedSourceRoutesV1,
    seal_key: OperationKey,
) -> Result<(IngressRef, ObjectRef), Box<dyn Error>> {
    let ingress = catalog.open_ingress(seal_key)?;
    let mut markers = Vec::new();
    for head in &routes.heads {
        let payload = native_protected_view_marker_bytes_v1(head.publication.identity)?;
        let reference = native_protected_view_marker_ref_v1(head.publication.identity)?;
        markers.push(RegisteredObject::unscoped(
            reference,
            u64::try_from(payload.len() + 16)?,
            Vec::new(),
        ));
    }
    for head in &routes.repomap_heads {
        let payload = native_protected_view_marker_bytes_v1(head.publication.identity)?;
        let reference = native_protected_view_marker_ref_v1(head.publication.identity)?;
        markers.push(RegisteredObject::unscoped(
            reference,
            u64::try_from(payload.len() + 16)?,
            Vec::new(),
        ));
        if head.source_operation.is_none() {
            let payload = native_protected_view_marker_bytes_v1(head.source_publication.identity)?;
            let reference = native_protected_view_marker_ref_v1(head.source_publication.identity)?;
            markers.push(RegisteredObject::unscoped(
                reference,
                u64::try_from(payload.len() + 16)?,
                Vec::new(),
            ));
        }
    }
    markers.sort_unstable_by_key(|marker| marker.reference);
    markers.dedup_by_key(|marker| marker.reference);
    catalog.reserve_inputs(ingress, &markers)?;
    catalog.confirm_inputs(ingress, &markers)?;
    let route_ref = routes.object_ref()?;
    let payload = serde_json::to_vec(routes)?;
    let children = routes.children()?;
    let route = RegisteredObject::unscoped(
        route_ref,
        u64::try_from(payload.len() + 16 + children.len() * 32)?,
        children,
    );
    catalog.reserve_inputs(ingress, std::slice::from_ref(&route))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&route))?;
    Ok((ingress, route_ref))
}

#[test]
fn selected_two_workspace_route_survives_exact_retirement_and_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, first_operation, first, second_operation, second) =
        two_published_heads(root.path())?;
    let routes = route_set(
        first_operation,
        first.clone(),
        second_operation,
        second.clone(),
    )?;
    let seal_key = operation(20_003, 203).key;
    let (ingress, route_ref) = stage_routes(&catalog, &routes, seal_key)?;
    assert_canonical_route_readback(&routes, route_ref)?;
    let wrong_fence = IngressRef {
        fence: ingress.fence + 1,
        ..ingress
    };
    check!(matches!(
        catalog.select_native_boot_manifest_with_protected_routes_if_current(
            first_operation.key,
            route_ref,
            &routes,
            wrong_fence,
            &mut admission_budget(),
        ),
        Err(IndexError::FenceLost)
    ));
    check_eq!(catalog.native_boot_manifest(&mut admission_budget())?, None);
    check!(
        catalog
            .select_native_boot_manifest_with_protected_routes_if_current(
                first_operation.key,
                ObjectRef(digest_id(254)),
                &routes,
                ingress,
                &mut admission_budget(),
            )
            .is_err()
    );
    check_eq!(catalog.native_boot_manifest(&mut admission_budget())?, None);

    let selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
        first_operation.key,
        route_ref,
        &routes,
        ingress,
        &mut admission_budget(),
    )?;
    check_eq!(
        selected.protected_source_routes,
        Some(boot_manifest::NativeBootSourceRouteRefV1 {
            object: route_ref,
            count: 2
        })
    );
    check_eq!(fixture_read_ingress(&catalog, seal_key)?, None);

    let guard = catalog.lock()?;
    read_only::check_quiescent_protected_routes(
        &guard,
        first.identity,
        &routes,
        None,
        &mut admission_budget(),
    )?;
    let mut published = Vec::new();
    read_only::visit_quiescent_published_rows_with_routes_v1(
        &guard,
        first.identity.workspace,
        Some(&routes),
        &mut admission_budget(),
        |operation, view, _| {
            published.push((operation.key, view.identity));
            Ok(())
        },
    )?;
    check_eq!(published.len(), 2);
    check!(published.contains(&(first_operation.key, first.identity)));
    check!(published.contains(&(second_operation.key, second.identity)));
    drop(guard);

    let mut successor = operation(20_004, 204);
    successor.key.workspace = second_workspace();
    let successor_attempt = admit_new(&catalog, successor)?;
    let next = in_second_workspace(view(2, second.profile, None));
    check_eq!(
        publish(&catalog, successor_attempt, &second, next.clone())?,
        OperationStatus::Published(next.identity)
    );
    check_eq!(
        catalog.advance_exact_history_floor(second_workspace(), 7, 2, &mut admission_budget(),)?,
        2
    );
    let retired = catalog.retire_exact_history_page(
        second_workspace(),
        7,
        None,
        8,
        &mut admission_budget(),
    )?;
    check_eq!(retired.retired, 1);
    check_eq!(retired.blocked, 1);
    let row_count: i64 = catalog.lock()?.query_row(
        "SELECT count(*) FROM native_view_v1 WHERE workspace=?1 AND epoch=7 AND sequence=1",
        params![workspace_key(second_workspace())?],
        |row| row.get(0),
    )?;
    check_eq!(row_count, 1);
    drop(catalog);
    let catalog = open(root.path())?;
    check_eq!(
        catalog.native_boot_manifest(&mut admission_budget())?,
        Some(selected.clone())
    );
    let guard = catalog.lock()?;
    check!(matches!(
        boot_manifest::validate_protected_routes(
            &guard,
            first_operation.key,
            route_ref,
            &routes,
            true,
            None,
            &mut admission_budget(),
        ),
        Err(IndexError::Conflict)
    ));
    check_eq!(
        boot_manifest::validate_protected_routes(
            &guard,
            first_operation.key,
            route_ref,
            &routes,
            false,
            None,
            &mut admission_budget(),
        )?
        .count,
        2
    );
    Ok(())
}

fn assert_canonical_route_readback(
    routes: &NativeProtectedSourceRoutesV1,
    route_ref: ObjectRef,
) -> TestResult {
    use quanta_index_core::index::{
        StoredObject, native_immutable_object_ref_v1, with_native_protected_source_routes_v1,
    };

    let bytes = serde_json::to_vec(routes)?;
    let children = routes.children()?;
    let stored = |reference, payload: Vec<u8>, children: Vec<ObjectRef>| {
        let wire_bytes = payload.len() + 16 + children.len() * 32;
        Ok::<_, std::num::TryFromIntError>(StoredObject::new(
            RegisteredObject::unscoped(reference, u64::try_from(wire_bytes)?, children),
            payload,
            None,
        ))
    };
    let canonical = stored(route_ref, bytes.clone(), children.clone())?;
    with_native_protected_source_routes_v1(
        route_ref,
        Some(2),
        &canonical,
        &mut protected_budget_v1(),
        |actual, _budget| {
            assert_eq!(actual, routes);
            Ok(())
        },
    )?;

    let mut noncanonical = bytes.clone();
    noncanonical.push(b' ');
    let raw_ref =
        native_immutable_object_ref_v1(&noncanonical, &children).ok_or("route reference")?;
    check!(raw_ref != route_ref);
    for reference in [route_ref, raw_ref] {
        let stored = stored(reference, noncanonical.clone(), children.clone())?;
        check!(matches!(
            with_native_protected_source_routes_v1::<()>(
                reference,
                Some(2),
                &stored,
                &mut protected_budget_v1(),
                |_, _| panic!("noncanonical route reached the visitor"),
            ),
            Err(IndexError::DigestConflict)
        ));
    }
    let mut wrong_children = children;
    let _removed = wrong_children.pop().ok_or("route child missing")?;
    let wrong_ref =
        native_immutable_object_ref_v1(&bytes, &wrong_children).ok_or("wrong route reference")?;
    let wrong = stored(wrong_ref, bytes, wrong_children)?;
    check!(matches!(
        with_native_protected_source_routes_v1::<()>(
            wrong_ref,
            Some(2),
            &wrong,
            &mut protected_budget_v1(),
            |_, _| panic!("route with wrong children reached the visitor"),
        ),
        Err(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn selected_primary_can_be_outside_complete_preserved_legacy_heads() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, primary_operation, _primary, old_operation, old_view) =
        two_published_heads(root.path())?;
    let mut routes = route_set(primary_operation, _primary, old_operation, old_view)?;
    let _removed = routes.heads.remove(0);
    routes.validate()?;
    let ingress_key = operation(20_005, 205).key;
    let (ingress, route_ref) = stage_routes(&catalog, &routes, ingress_key)?;
    let selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
        primary_operation.key,
        route_ref,
        &routes,
        ingress,
        &mut admission_budget(),
    )?;
    check_eq!(
        selected.protected_source_routes,
        Some(boot_manifest::NativeBootSourceRouteRefV1 {
            object: route_ref,
            count: 1
        })
    );
    let guard = catalog.lock()?;
    check_eq!(
        boot_manifest::validate_protected_routes(
            &guard,
            primary_operation.key,
            route_ref,
            &routes,
            true,
            None,
            &mut admission_budget(),
        )?
        .count,
        1
    );
    Ok(())
}

#[test]
fn selected_repomap_only_workspace_is_protected_without_source_route() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, primary_operation, _primary, _old_source_operation, old_source_view) =
        two_published_heads(root.path())?;
    let mut imported_operation = operation(20_006, 206);
    imported_operation.key.workspace = second_workspace();
    let attempt = admit_new(&catalog, imported_operation)?;
    let member = object(52, vec![]);
    reserve_confirm(&catalog, attempt, std::slice::from_ref(&member))?;
    let imported_view =
        in_second_workspace(view(2, old_source_view.profile, Some(member.reference)));
    check_eq!(
        publish(&catalog, attempt, &old_source_view, imported_view.clone())?,
        OperationStatus::Published(imported_view.identity)
    );
    let routes = NativeProtectedSourceRoutesV1 {
        native_origin: None,
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        heads: Vec::new(),
        repomap_heads: vec![NativeProtectedRepoMapHeadV1 {
            repo_id: quanta_index_contract::RepoId::new("repo-b")?,
            revision_id: quanta_index_contract::RevisionId::new("rev-b")?,
            active: quanta_index_contract::RepoMapExpectedActiveV2::new(
                std::num::NonZeroU64::new(1).ok_or("epoch absent")?,
                quanta_index_contract::CandidateCommitmentV1::from_bytes([53; 32]),
            ),
            source_operation: None,
            source_publication: in_second_workspace(view(0, old_source_view.profile, None)),
            operation: imported_operation,
            publication: imported_view.clone(),
            instance: IndexInstanceId(id(40)),
            member_binding: member.reference,
        }],
    };
    routes.validate()?;
    let mut false_genesis = routes.clone();
    false_genesis.repomap_heads[0].source_publication = old_source_view;
    check!(false_genesis.validate().is_err());
    let (ingress, route_ref) = stage_routes(&catalog, &routes, operation(20_007, 207).key)?;
    let memory = ProtectedTestMemoryV1 {
        // One route object, its RepoMap view marker and its genesis marker.
        deny_bytes: Some(3 * 32),
        ..Default::default()
    };
    let mut denied = admission_budget();
    denied.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(
        std::sync::Arc::new(memory.clone()),
    )));
    check!(matches!(
        catalog.select_native_boot_manifest_with_protected_routes_if_current(
            primary_operation.key,
            route_ref,
            &routes,
            ingress,
            &mut denied,
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(memory.live.load(std::sync::atomic::Ordering::Acquire), 0);
    check_eq!(catalog.native_boot_manifest(&mut admission_budget())?, None);
    let selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
        primary_operation.key,
        route_ref,
        &routes,
        ingress,
        &mut admission_budget(),
    )?;
    check_eq!(
        selected.protected_source_routes,
        Some(boot_manifest::NativeBootSourceRouteRefV1 {
            object: route_ref,
            count: 0
        })
    );
    let guard = catalog.lock()?;
    check_eq!(
        boot_manifest::validate_protected_routes(
            &guard,
            primary_operation.key,
            route_ref,
            &routes,
            true,
            None,
            &mut admission_budget(),
        )?
        .count,
        0
    );
    read_only::check_quiescent_protected_routes(
        &guard,
        selected.publication.identity,
        &routes,
        None,
        &mut admission_budget(),
    )?;
    drop(guard);
    check_eq!(
        catalog.advance_exact_history_floor(second_workspace(), 7, 2, &mut admission_budget(),)?,
        2
    );
    let retired = catalog.retire_exact_history_page(
        second_workspace(),
        7,
        None,
        8,
        &mut admission_budget(),
    )?;
    check_eq!(retired.retired, 1);
    check_eq!(retired.blocked, 1);
    let repo = quanta_index_contract::RepoId::new("repo-b")?;
    let revision = quanta_index_contract::RevisionId::new("rev-b")?;
    check_eq!(
        catalog.resolve_selected_native_protected_repomap_head_v1(
            &selected,
            &routes,
            &repo,
            &revision,
            &mut admission_budget(),
        )?,
        imported_view
    );
    let foreign = quanta_index_contract::RepoId::new("foreign")?;
    check!(matches!(
        catalog.resolve_selected_native_protected_repomap_head_v1(
            &selected,
            &routes,
            &foreign,
            &revision,
            &mut admission_budget(),
        ),
        Err(IndexError::SnapshotUnavailable)
    ));
    Ok(())
}

#[test]
fn selected_routes_refuse_pending_or_unknown_workspace() -> TestResult {
    for unknown_workspace in [false, true] {
        let root = tempfile::tempdir()?;
        let (catalog, first_operation, first, second_operation, second) =
            two_published_heads(root.path())?;
        let routes = route_set(first_operation, first.clone(), second_operation, second)?;
        let (ingress, route_ref) = stage_routes(&catalog, &routes, operation(20_008, 208).key)?;
        let _selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
            first_operation.key,
            route_ref,
            &routes,
            ingress,
            &mut admission_budget(),
        )?;
        if unknown_workspace {
            let profile = object(31, vec![]);
            let stage = catalog.begin_bootstrap(
                third_workspace(),
                7,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            )?;
            catalog.reserve_bootstrap(stage, &profile)?;
            catalog.confirm_bootstrap(stage, &profile)?;
            let mut genesis = view(0, profile.reference, None);
            genesis.identity.workspace = third_workspace();
            genesis.identity.digest = genesis.identity_digest()?;
            let _published = catalog.publish_bootstrap(stage, genesis, &mut admission_budget())?;
        } else {
            let mut pending = operation(20_009, 209);
            pending.key.workspace = second_workspace();
            let _pending = admit_new(&catalog, pending)?;
        }
        let guard = catalog.lock()?;
        check!(matches!(
            read_only::check_quiescent_protected_routes(
                &guard,
                first.identity,
                &routes,
                None,
                &mut admission_budget(),
            ),
            Err(IndexError::Conflict)
        ));
    }
    Ok(())
}

#[test]
fn protected_native_import_is_atomic_exact_and_retryable() -> TestResult {
    use quanta_index_contract::index::{
        NativeProtectedNativeOriginV1, NativeProtectedNativeSnapshotV1,
    };

    let old_root = tempfile::tempdir()?;
    let (old, primary, _first, source_operation, source_view) =
        two_published_heads(old_root.path())?;
    let map_workspace = third_workspace();
    let profile = object(30, vec![]);
    let stage = old.begin_bootstrap(
        map_workspace,
        7,
        &GenesisIdentity(profile.clone()),
        &mut protected_budget_v1(),
    )?;
    old.reserve_bootstrap(stage, &profile)?;
    old.confirm_bootstrap(stage, &profile)?;
    let mut map_genesis = view(0, profile.reference, None);
    map_genesis.identity.workspace = map_workspace;
    map_genesis.identity.digest = map_genesis.identity_digest()?;
    check_eq!(
        old.publish_bootstrap(stage, map_genesis.clone(), &mut protected_budget_v1())?,
        map_genesis.identity
    );
    let mut map_operation = operation(20_020, 220);
    map_operation.key.workspace = map_workspace;
    let attempt = admit_new(&old, map_operation)?;
    let member = object(52, vec![]);
    reserve_confirm(&old, attempt, std::slice::from_ref(&member))?;
    let mut map_view = view(1, profile.reference, Some(member.reference));
    map_view.identity.workspace = map_workspace;
    map_view.identity.digest = map_view.identity_digest()?;
    check_eq!(
        publish(&old, attempt, &map_genesis, map_view.clone())?,
        OperationStatus::Published(map_view.identity)
    );
    let mut old_routes = route_set(primary, _first, source_operation, source_view.clone())?;
    let _removed_primary = old_routes.heads.remove(0);
    old_routes.repomap_heads.push(NativeProtectedRepoMapHeadV1 {
        repo_id: quanta_index_contract::RepoId::new("repo-map")?,
        revision_id: quanta_index_contract::RevisionId::new("rev-map")?,
        active: quanta_index_contract::RepoMapExpectedActiveV2::new(
            std::num::NonZeroU64::new(1).ok_or("map epoch absent")?,
            quanta_index_contract::CandidateCommitmentV1::from_bytes([53; 32]),
        ),
        source_operation: None,
        source_publication: map_genesis.clone(),
        operation: map_operation,
        publication: map_view.clone(),
        instance: IndexInstanceId(id(40)),
        member_binding: member.reference,
    });
    old_routes.validate()?;
    let (old_ingress, old_route_ref) = stage_routes(&old, &old_routes, operation(20_021, 221).key)?;
    let _old_selected = old.select_native_boot_manifest_with_protected_routes_if_current(
        primary.key,
        old_route_ref,
        &old_routes,
        old_ingress,
        &mut protected_budget_v1(),
    )?;
    let nonce = {
        let guard = old.lock()?;
        Digest(capture_format::verify_stamp(&*guard, old_root.path())?)
    };
    let mut keys = [source_operation.key, map_operation.key];
    keys.sort_unstable();
    let operation_rows = keys
        .into_iter()
        .map(|key| {
            let guard = old.lock().map_err(|error| error.to_string())?;
            let row = read_operation(&*guard, key)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "old operation absent".to_owned())?;
            encode(&row).map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let roots = vec![old_route_ref];
    let snapshot = NativeProtectedNativeSnapshotV1 {
        wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
        source_root_nonce: nonce,
        source_routes: old_route_ref,
        operation_rows,
        genesis_views: vec![map_genesis.clone()],
        frontiers: Vec::new(),
        source_authorities: Vec::new(),
        event_claims: Vec::new(),
        roots,
    };
    snapshot.validate_shape_v1()?;
    let snapshot_ref = snapshot.object_ref_v1()?;
    let mut routes = old_routes.clone();
    routes.native_origin = Some(NativeProtectedNativeOriginV1 {
        source_root_nonce: nonce,
        source_routes: old_route_ref,
        snapshot: snapshot_ref,
    });
    routes.validate()?;

    let destination = tempfile::tempdir()?;
    let new = open(destination.path())?;
    let primary_workspace = WorkspaceKey {
        slot: SlotId(id(238)),
        ..workspace()
    };
    let primary_profile = object(30, vec![]);
    let stage = new.begin_bootstrap(
        primary_workspace,
        7,
        &GenesisIdentity(primary_profile.clone()),
        &mut protected_budget_v1(),
    )?;
    new.reserve_bootstrap(stage, &primary_profile)?;
    new.confirm_bootstrap(stage, &primary_profile)?;
    let mut primary_genesis = view(0, primary_profile.reference, None);
    primary_genesis.identity.workspace = primary_workspace;
    primary_genesis.identity.digest = primary_genesis.identity_digest()?;
    check_eq!(
        new.publish_bootstrap(stage, primary_genesis.clone(), &mut protected_budget_v1())?,
        primary_genesis.identity
    );
    let mut new_primary = operation(20_022, 222);
    new_primary.key.workspace = primary_workspace;
    let attempt = admit_new(&new, new_primary)?;
    let mut primary_view = view(1, primary_profile.reference, None);
    primary_view.identity.workspace = primary_workspace;
    primary_view.identity.digest = primary_view.identity_digest()?;
    check_eq!(
        publish(&new, attempt, &primary_genesis, primary_view.clone())?,
        OperationStatus::Published(primary_view.identity)
    );
    let ingress_key = OperationKey {
        workspace: primary_workspace,
        ..operation(20_023, 223).key
    };
    let ingress = new.open_ingress(ingress_key)?;
    let route_ref = routes.object_ref()?;
    let mut childless = old_routes
        .children()?
        .into_iter()
        .map(|reference| RegisteredObject::unscoped(reference, 1, Vec::new()))
        .collect::<Vec<_>>();
    childless.push(RegisteredObject::unscoped(
        primary_profile.reference,
        1,
        Vec::new(),
    ));
    childless.sort_unstable_by_key(|object| object.reference);
    childless.dedup_by_key(|object| object.reference);
    new.reserve_inputs(ingress, &childless)?;
    new.confirm_inputs(ingress, &childless)?;
    let old_route = RegisteredObject::unscoped(old_route_ref, 1, old_routes.children()?);
    new.reserve_inputs(ingress, std::slice::from_ref(&old_route))?;
    new.confirm_inputs(ingress, std::slice::from_ref(&old_route))?;
    let snapshot_object = RegisteredObject::unscoped(snapshot_ref, 1, snapshot.roots.clone());
    new.reserve_inputs(ingress, std::slice::from_ref(&snapshot_object))?;
    new.confirm_inputs(ingress, std::slice::from_ref(&snapshot_object))?;
    let route_object = RegisteredObject::unscoped(route_ref, 1, routes.children()?);
    new.reserve_inputs(ingress, std::slice::from_ref(&route_object))?;
    new.confirm_inputs(ingress, std::slice::from_ref(&route_object))?;

    check_eq!(
        new.restore_protected_native_heads_v1(
            new_primary.key,
            ingress,
            &snapshot,
            &routes,
            &mut admission_budget(),
        ),
        Err(IndexError::ResourceLimited)
    );
    let mut bad = snapshot.clone();
    check!(bad.operation_rows.pop().is_some());
    let bad_snapshot_ref = bad.object_ref_v1()?;
    let mut bad_routes = routes.clone();
    bad_routes
        .native_origin
        .as_mut()
        .ok_or("Native origin absent")?
        .snapshot = bad_snapshot_ref;
    // Bind and confirm this closure so the refusal exercises the missing
    // operation row instead of a mismatched snapshot reference.
    let bad_snapshot = RegisteredObject::unscoped(bad_snapshot_ref, 1, bad.roots.clone());
    new.reserve_inputs(ingress, std::slice::from_ref(&bad_snapshot))?;
    new.confirm_inputs(ingress, std::slice::from_ref(&bad_snapshot))?;
    let bad_route = RegisteredObject::unscoped(bad_routes.object_ref()?, 1, bad_routes.children()?);
    new.reserve_inputs(ingress, std::slice::from_ref(&bad_route))?;
    new.confirm_inputs(ingress, std::slice::from_ref(&bad_route))?;
    check_eq!(
        new.restore_protected_native_heads_v1(
            new_primary.key,
            ingress,
            &bad,
            &bad_routes,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    );
    {
        let guard = new.lock()?;
        check_eq!(current_view(&*guard, second_workspace())?, None);
        check_eq!(current_view(&*guard, map_workspace)?, None);
    }
    new.restore_protected_native_heads_v1(
        new_primary.key,
        ingress,
        &snapshot,
        &routes,
        &mut protected_budget_v1(),
    )?;
    new.restore_protected_native_heads_v1(
        new_primary.key,
        ingress,
        &snapshot,
        &routes,
        &mut protected_budget_v1(),
    )?;
    {
        let guard = new.lock()?;
        check_eq!(
            current_view(&*guard, second_workspace())?,
            Some(source_view)
        );
        check_eq!(
            current_view(&*guard, map_workspace)?,
            Some(map_view.clone())
        );
    }
    let conflicting = OperationKey {
        workspace: map_workspace,
        ..operation(20_024, 224).key
    };
    let _unexpected = admit_new(
        &new,
        ImmutableOperation {
            key: conflicting,
            request: ObjectRef(digest_id(224)),
            source_mode: SourceOperationMode::Unchanged,
        },
    )?;
    check!(
        new.restore_protected_native_heads_v1(
            new_primary.key,
            ingress,
            &snapshot,
            &routes,
            &mut protected_budget_v1(),
        )
        .is_err()
    );
    // An imported workspace must not expose an undeclared Exact view even
    // when every declared route and the terminal journal still match.
    let mut orphan = map_view.clone();
    orphan.identity.sequence = orphan
        .identity
        .sequence
        .checked_add(1)
        .ok_or("view sequence")?;
    orphan.identity.digest = orphan.identity_digest()?;
    {
        let guard = new.lock()?;
        write_view(&*guard, &orphan)?;
        check_eq!(
            read_only::verify_imported_exact_views_v1(&*guard, &routes, &mut protected_budget_v1()),
            Err(IndexError::DigestConflict)
        );
    }
    Ok(())
}
