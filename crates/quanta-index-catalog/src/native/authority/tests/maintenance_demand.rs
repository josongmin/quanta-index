//! Every committed change that makes maintenance due records it for the pump.

use super::*;

#[test]
fn held_ticket_requests_maintenance_only_after_last_pin() -> TestResult {
    use quanta_index_core::index::{CatalogReadSettlementPort, NativeReadSettlementScope};

    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let session = catalog.open_read_session()?;
    let scope = NativeReadSettlementScope {
        workspace: workspace(),
        epoch: 7,
        target: genesis.identity,
    };
    let acquire = || {
        acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(scope.target),
            Duration::from_secs(30),
        )
    };
    let first = acquire()?;
    let last = acquire()?;
    let release = |ticket| {
        catalog.release_held_ticket(session.as_ref(), ticket, scope, &mut admission_budget())
    };

    check!(release(first.lease)?.has_remaining_target_pins);
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    check!(!release(last.lease)?.has_remaining_target_pins);
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            released_views: vec![ReleasedViewRange {
                workspace: scope.workspace,
                epoch: scope.epoch,
                first_sequence: scope.target.sequence,
                last_sequence: scope.target.sequence,
            }],
            ..MaintenanceDemand::default()
        }
    );
    Ok(())
}

#[test]
fn releases_abandonment_recovery_and_retirement_record_maintenance_demand() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let next = publish_plain_next(&catalog, &genesis, 1, 241)?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    let released = vec![ReleasedViewRange {
        workspace: workspace(),
        epoch: 7,
        first_sequence: genesis.identity.sequence,
        last_sequence: genesis.identity.sequence,
    }];
    let lease = Duration::from_secs(30);

    // An explicit release of a pinned view releases its scope.
    let live = catalog.open_read_session()?;
    let pinned = acquire_ticket_with_test_budget(
        &catalog,
        live.as_ref(),
        ViewSelection::Exact(genesis.identity),
        lease,
    )?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    catalog.release_ticket(live.as_ref(), pinned.lease)?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            released_views: released.clone(),
            ..MaintenanceDemand::default()
        }
    );
    // Taking the demand clears it.
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );

    // An abandoned pin makes read recovery due; its settlement releases the scope.
    let abandoned = acquire_ticket_with_test_budget(
        &catalog,
        live.as_ref(),
        ViewSelection::Exact(genesis.identity),
        lease,
    )?;
    catalog.abandon_ticket(Arc::clone(&live), abandoned.lease)?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            read_recovery: true,
            ..MaintenanceDemand::default()
        }
    );
    let _round = catalog.recover_read_sessions(None, 16, &mut admission_budget())?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            released_views: released.clone(),
            ..MaintenanceDemand::default()
        }
    );

    // Dead-session recovery releases the dead session's pinned scope.
    let dead = catalog.open_read_session()?;
    let _dead_pin = acquire_ticket_with_test_budget(
        &catalog,
        dead.as_ref(),
        ViewSelection::Exact(genesis.identity),
        lease,
    )?;
    drop(dead);
    let recovered = catalog.recover_read_sessions(None, 16, &mut admission_budget())?;
    check_eq!((recovered.recovered, recovered.pins_released), (1, 1));
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            released_views: released,
            ..MaintenanceDemand::default()
        }
    );

    // Retiring a view queues reclaim work; a page that retires nothing does not.
    check_eq!(
        catalog.advance_exact_history_floor(
            workspace(),
            7,
            next.identity.sequence,
            &mut admission_budget()
        )?,
        next.identity.sequence
    );
    let retired =
        catalog.retire_exact_history_page(workspace(), 7, None, 8, &mut admission_budget())?;
    check_eq!(retired.retired, 1);
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            reclaim: true,
            ..MaintenanceDemand::default()
        }
    );
    let idle =
        catalog.retire_exact_history_page(workspace(), 7, None, 8, &mut admission_budget())?;
    check_eq!(idle.retired, 0);
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    drop(live);
    Ok(())
}

#[test]
fn distant_released_pins_stay_separate_in_reverse_release_order() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let first = publish_plain_next(&catalog, &genesis, 1, 241)?;
    let last = publish_plain_next(&catalog, &first, 2, 242)?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );

    let live = catalog.open_read_session()?;
    let lease = Duration::from_secs(30);
    let oldest = acquire_ticket_with_test_budget(
        &catalog,
        live.as_ref(),
        ViewSelection::Exact(genesis.identity),
        lease,
    )?;
    let newest = acquire_ticket_with_test_budget(
        &catalog,
        live.as_ref(),
        ViewSelection::Exact(last.identity),
        lease,
    )?;
    // Release in descending order. The published middle view has no pin
    // deletion, so it must not be included in either demand range.
    catalog.release_ticket(live.as_ref(), newest.lease)?;
    catalog.release_ticket(live.as_ref(), oldest.lease)?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            released_views: vec![
                ReleasedViewRange {
                    workspace: workspace(),
                    epoch: 7,
                    first_sequence: genesis.identity.sequence,
                    last_sequence: genesis.identity.sequence,
                },
                ReleasedViewRange {
                    workspace: workspace(),
                    epoch: 7,
                    first_sequence: last.identity.sequence,
                    last_sequence: last.identity.sequence,
                },
            ],
            ..MaintenanceDemand::default()
        }
    );
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    drop(live);
    Ok(())
}

#[test]
fn adjacent_release_points_coalesce_and_overflow_keeps_oldest_hints() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, _genesis) = setup(root.path())?;
    let workspace_bytes = workspace_key(workspace())?;
    // Exact duplicate hints do not spend capacity. An older arrival replaces
    // the newest entry when the 256-point scope allowance is full; a later
    // arrival waits for the ordinary durable Views sweep.
    for sequence in 1..=256 {
        catalog.record_released_pin(&workspace_bytes, 7, sequence);
    }
    catalog.record_released_pin(&workspace_bytes, 7, 256);
    catalog.record_released_pin(&workspace_bytes, 7, 1_000);
    catalog.record_released_pin(&workspace_bytes, 7, 0);
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand {
            released_views: vec![ReleasedViewRange {
                workspace: workspace(),
                epoch: 7,
                first_sequence: 0,
                last_sequence: 255,
            }],
            ..MaintenanceDemand::default()
        }
    );
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    Ok(())
}

/// Drain one unlinked parent whose child edges are its only references and
/// return the drain page with the demand it recorded.
fn drain_parent(
    children: &[RegisteredObject],
    parent: u8,
) -> Result<
    (
        quanta_index_core::index::ReclaimDrainPage,
        MaintenanceDemand,
    ),
    Box<dyn Error>,
> {
    use quanta_index_core::index::ports::{CatalogReclaimPort, ManagedObjectAuthority};
    use quanta_index_core::index::{
        ObjectNamespaceBinding, PhysicalIncarnation, ReclaimTarget, ReclaimWork,
    };

    let root = tempfile::tempdir()?;
    let binding = ObjectNamespaceBinding {
        root_device: 1,
        root_inode: 2,
        objects_device: 1,
        objects_inode: 3,
        lease_device: 1,
        lease_inode: 4,
    };
    let catalog = open(root.path())?;
    catalog.bind_namespace(binding)?;
    let parent = object(
        parent,
        children.iter().map(|child| child.reference).collect(),
    );
    {
        let mut guard = catalog.lock().map_err(|error| error.to_string())?;
        let tx = begin(&mut guard)?;
        for registered in children.iter().chain(std::iter::once(&parent)) {
            reclaim::reserve_object(&tx, registered.reference)?;
            object_install::reserve(&tx, registered)?;
            register_confirmed(&tx, std::slice::from_ref(registered))?;
            reclaim::confirm_object(&tx, registered.reference)?;
        }
        check!(reclaim::enqueue(&tx, parent.reference)?);
        tx.commit()?;
        drop(guard);
    }
    let page = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
    let Some(ReclaimWork::Candidate(candidate)) = page.items.first().cloned() else {
        return Err("parent is not the reclaim candidate".into());
    };
    check_eq!(candidate.object, parent.reference);
    let retiring = catalog
        .retire_candidate(candidate, &mut admission_budget())?
        .ok_or("parent unexpectedly retained")?;
    let permit = catalog.enter_reclaiming(
        retiring,
        ReclaimTarget::Present {
            incarnation: PhysicalIncarnation {
                namespace: binding,
                shard_device: 1,
                shard_inode: 5,
                device: 1,
                inode: 6,
                changed_nanos: 1,
                bytes: parent.bytes,
            },
            registration: parent,
        },
        &mut admission_budget(),
    )?;
    catalog.validate_reclaiming(permit.clone(), &mut admission_budget())?;
    catalog.mark_unlinked(permit.clone(), &mut admission_budget())?;
    check_eq!(
        catalog.take_maintenance_demand()?,
        MaintenanceDemand::default()
    );
    let drained = catalog.drain_unlinked(permit, 32, &mut admission_budget())?;
    let demand = catalog.take_maintenance_demand()?;
    Ok((drained, demand))
}

#[test]
fn drain_that_queues_freed_children_records_reclaim_demand() -> TestResult {
    let (drained, demand) = drain_parent(&[object(250, vec![]), object(251, vec![])], 252)?;
    check_eq!(drained.enqueued_children, 2);
    check!(!drained.has_more);
    check_eq!(
        demand,
        MaintenanceDemand {
            reclaim: true,
            ..MaintenanceDemand::default()
        }
    );
    // A leaf drain queues nothing, so it leaves the reclaim schedule alone.
    let (leaf, demand) = drain_parent(&[], 253)?;
    check_eq!(leaf.enqueued_children, 0);
    check_eq!(demand, MaintenanceDemand::default());
    Ok(())
}
