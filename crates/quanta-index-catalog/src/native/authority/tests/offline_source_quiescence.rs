use super::*;
use super::offline_protected_routes::protected_budget_v1;

#[test]
fn policy_only_boot_seal_preserves_empty_source_routes_and_rejects_extra_ingress() -> TestResult {
    use quanta_index_contract::index::RetiredProjectionPolicyRefV1;

    for extra in [false, true] {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let primary = operation(32_090, 209);
        let attempt = admit_new(&catalog, primary)?;
        let published = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, published.clone())?,
            OperationStatus::Published(published.identity)
        );
        let ingress = catalog.open_ingress(operation(32_091, 210).key)?;
        let policy_object = object(241, vec![]);
        catalog.reserve_inputs(ingress, std::slice::from_ref(&policy_object))?;
        catalog.confirm_inputs(ingress, std::slice::from_ref(&policy_object))?;
        if extra {
            let unexpected = object(242, vec![]);
            catalog.reserve_inputs(ingress, std::slice::from_ref(&unexpected))?;
            catalog.confirm_inputs(ingress, std::slice::from_ref(&unexpected))?;
        }
        let policy = RetiredProjectionPolicyRefV1 {
            object: policy_object.reference,
            sha256: Digest([243; 32]),
            identity_count: 1,
        };
        let outcome = catalog.select_native_boot_manifest_with_retired_policy_ingress_if_current(
            primary.key, ingress, policy, &mut protected_budget_v1(),
        );
        if extra {
            check!(outcome.is_err());
            check_eq!(catalog.native_boot_manifest(&mut protected_budget_v1())?, None);
        } else {
            let selected = outcome?;
            check_eq!(selected.protected_source_routes, None);
            check_eq!(selected.retired_projection_policy, Some(policy));
            check_eq!(selected.format_version, NativeBootManifest::FORMAT_VERSION_V2);
            check_eq!(catalog.native_boot_manifest(&mut protected_budget_v1())?, Some(selected));
        }
    }
    Ok(())
}

fn selected_catalog(root: &Path) -> Result<(SqliteCatalog, NativeBootManifest), Box<dyn Error>> {
    let (catalog, genesis) = setup(root)?;
    let original = operation(32_001, 201);
    let attempt = admit_new(&catalog, original)?;
    let first = view(1, genesis.profile, None);
    check_eq!(
        publish(&catalog, attempt, &genesis, first.clone())?,
        OperationStatus::Published(first.identity)
    );
    let selected = catalog.select_native_boot_manifest(original.key, &mut admission_budget())?;
    Ok((catalog, selected))
}

fn visit_quiescent_publications(
    connection: &Connection,
    selected: &NativeBootManifest,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&ImmutableOperation, &CatalogView, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<CatalogView, IndexError> {
    let head = read_only::check_source_root_quiescence(connection, selected, workspace(), budget)?;
    read_only::visit_quiescent_published_rows_v1(connection, workspace(), budget, &mut visit)?;
    Ok(head)
}

#[test]
fn source_root_publication_census_visits_actual_middle_row_and_refuses_orphan() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let middle = publish_plain_next(&catalog, &selected.publication, 2, 202)?;
    let latest = publish_plain_next(&catalog, &middle, 3, 203)?;
    let guard = catalog.lock()?;
    let mut seen = Vec::new();
    check_eq!(
        visit_quiescent_publications(
            &guard,
            &selected,
            &mut admission_budget(),
            |operation, publication, budget| {
                budget.checkpoint()?;
                seen.push((operation.key, publication.identity));
                Ok(())
            }
        )?,
        latest
    );
    check_eq!(seen.len(), 3);
    check_eq!(
        seen[0],
        (selected.operation.key, selected.publication.identity)
    );
    check_eq!(seen[1], (operation(922, 202).key, middle.identity));
    check_eq!(seen[2], (operation(923, 203).key, latest.identity));

    // A Source inventory missing the middle operation must refuse even when
    // its highest key and the actual Catalog head agree.
    check!(matches!(
        visit_quiescent_publications(
            &guard,
            &selected,
            &mut admission_budget(),
            |published, _, _| {
                if published.key == operation(922, 202).key {
                    Err(IndexError::DigestConflict)
                } else {
                    Ok(())
                }
            }
        ),
        Err(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn source_root_quiescence_allows_actual_post_selection_publication() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let later = publish_plain_next(&catalog, &selected.publication, 2, 202)?;
    let guard = catalog.lock()?;
    check_eq!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        )?,
        later
    );
    // Forward recovery can observe a later settled head. Backward rollback
    // still requires the exact selected baseline and must refuse it.
    check!(matches!(
        read_only::check_rollback(&guard, &selected, &mut admission_budget()),
        Err(IndexError::Conflict)
    ));
    check_eq!(boot_record::read_stored(&guard)?, Some(selected));
    Ok(())
}

#[test]
fn single_workspace_boot_and_rollback_refuse_orphan_view_root() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let guard = catalog.lock()?;
    let workspace_blob = workspace_key(workspace())?;
    let epoch = int(selected.publication.identity.epoch)?;
    let sequence = int(selected.publication.identity.sequence + 1)?;
    let object = object_key(selected.publication.profile);
    let digest = fields_digest(
        b"qidx.native.viewroot.v1",
        &(&workspace_blob, epoch, sequence, &object),
    )?;
    let foreign_keys: bool = guard.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    guard.pragma_update(None, "foreign_keys", false)?;
    let inserted = guard.execute(
        "INSERT INTO native_view_root_v1(workspace,epoch,sequence,object,row_digest)
         VALUES (?1,?2,?3,?4,?5)",
        params![&workspace_blob, epoch, sequence, &object, &digest],
    );
    guard.pragma_update(None, "foreign_keys", foreign_keys)?;
    check_eq!(inserted?, 1);

    check_eq!(
        boot_quiescence::check_quiescent_rows(
            &guard,
            workspace(),
            selected.publication.identity,
            &mut admission_budget(),
        ),
        Err(IndexError::DigestConflict)
    );
    check_eq!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget(),
        ),
        Err(IndexError::DigestConflict)
    );
    check_eq!(
        read_only::check_rollback(&guard, &selected, &mut admission_budget()),
        Err(IndexError::DigestConflict)
    );
    check_eq!(boot_record::read_stored(&guard)?, Some(selected.clone()));
    let retained: i64 = guard.query_row(
        "SELECT count(*) FROM native_view_root_v1
         WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
        params![&workspace_blob, epoch, sequence],
        |row| row.get(0),
    )?;
    check_eq!(retained, 1);

    check_eq!(
        guard.execute(
            "DELETE FROM native_view_root_v1
             WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
        )?,
        1
    );
    check_eq!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget(),
        )?,
        selected.publication
    );
    read_only::check_rollback(&guard, &selected, &mut admission_budget())?;
    Ok(())
}

#[test]
fn source_root_quiescence_refuses_ingress_pending_operation_and_pending_work() -> TestResult {
    for kind in 0..3 {
        let root = tempfile::tempdir()?;
        let (catalog, selected) = selected_catalog(root.path())?;
        match kind {
            0 => {
                let _ingress = catalog.open_ingress(operation(32_002, 203).key)?;
            }
            1 => {
                let _pending = admit_new(&catalog, operation(32_003, 204))?;
            }
            _ => {
                let work = owner_work(205);
                let _next =
                    publish_work_next(&catalog, &selected.publication, 2, &work, 32_004, 206)?;
            }
        }
        let guard = catalog.lock()?;
        check!(matches!(
            read_only::check_source_root_quiescence(
                &guard,
                &selected,
                workspace(),
                &mut admission_budget()
            ),
            Err(IndexError::Conflict)
        ));
        let mut visited = 0;
        check!(matches!(
            visit_quiescent_publications(&guard, &selected, &mut admission_budget(), |_, _, _| {
                visited += 1;
                Ok(())
            }),
            Err(IndexError::Conflict)
        ));
        check_eq!(visited, 0);
        check_eq!(boot_record::read_stored(&guard)?, Some(selected));
    }
    Ok(())
}

#[test]
fn source_root_quiescence_refuses_a_second_encoding_of_a_published_key() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let guard = catalog.lock()?;
    let key = operation_key(selected.operation.key)?;
    let mut alias = key.clone();
    alias.push(b' ');
    check_eq!(decode::<OperationKey>(&alias)?, selected.operation.key);
    check_eq!(
        guard.execute(
            "INSERT INTO native_operation_v1(operation,workspace,epoch,payload,row_digest)
             SELECT ?1,workspace,epoch,payload,row_digest FROM native_operation_v1 WHERE operation=?2",
            params![&alias, &key],
        )?,
        1
    );
    let mut visited = 0;
    check!(matches!(
        visit_quiescent_publications(&guard, &selected, &mut admission_budget(), |_, _, _| {
            visited += 1;
            Ok(())
        }),
        Err(IndexError::DigestConflict)
    ));
    check_eq!(visited, 0);
    check_eq!(
        guard.execute(
            "DELETE FROM native_operation_v1 WHERE operation=?1",
            params![&alias],
        )?,
        1
    );
    check_eq!(
        visit_quiescent_publications(&guard, &selected, &mut admission_budget(), |_, _, _| {
            visited += 1;
            Ok(())
        })?,
        selected.publication
    );
    check_eq!(visited, 1);
    Ok(())
}

#[test]
fn source_root_quiescence_refuses_work_payload_at_another_key() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let intent = owner_work(209);
    let head = publish_work_next(&catalog, &selected.publication, 2, &intent, 32_002, 210)?;
    set_terminal_work_status(&catalog, &intent, &WorkStatus::Superseded)?;
    let guard = catalog.lock()?;
    let key = work_key(&intent)?;
    let other_key = work_key(&owner_work(210))?;
    check_ne!(other_key, key);
    check_eq!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        )?,
        head
    );
    check_eq!(
        guard.execute(
            "INSERT INTO native_work_v1(work,operation,workspace,epoch,sequence,status,owner_mode,
                                      ready_since_ms,lease_until_ms,payload,row_digest)
             SELECT ?1,operation,workspace,epoch,sequence,status,owner_mode,
                    ready_since_ms,lease_until_ms,payload,row_digest FROM native_work_v1 WHERE work=?2",
            params![&other_key, &key],
        )?,
        1
    );
    check!(matches!(
        read_only::check_source_root_quiescence(&guard, &selected, workspace(), &mut admission_budget()),
        Err(IndexError::Storage(reason)) if reason == "native work row identity mismatch"
    ));
    Ok(())
}

#[test]
fn source_root_quiescence_refuses_foreign_scope_and_preserves_cancellation() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let guard = catalog.lock()?;
    let foreign = WorkspaceKey {
        slot: SlotId(id(207)),
        ..workspace()
    };
    check!(matches!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            foreign,
            &mut admission_budget()
        ),
        Err(IndexError::DigestConflict)
    ));
    let mut cancelled = admission_budget();
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        read_only::check_source_root_quiescence(&guard, &selected, workspace(), &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    check_eq!(boot_record::read_stored(&guard)?, Some(selected));
    Ok(())
}

#[test]
fn source_root_quiescence_refuses_foreign_workspace_even_when_local_head_is_valid() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let guard = catalog.lock()?;
    let foreign = WorkspaceKey {
        slot: SlotId(id(208)),
        ..workspace()
    };
    check_eq!(
        guard.execute(
            "INSERT INTO native_workspace_v1(workspace,epoch,sequence,payload,row_digest) \
         SELECT ?1,epoch,sequence,payload,row_digest FROM native_workspace_v1 WHERE workspace=?2",
            params![workspace_key(foreign)?, workspace_key(workspace())?]
        )?,
        1
    );
    check!(matches!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        ),
        Err(IndexError::Conflict)
    ));
    let mut visited = 0;
    check!(matches!(
        visit_quiescent_publications(&guard, &selected, &mut admission_budget(), |_, _, _| {
            visited += 1;
            Ok(())
        }),
        Err(IndexError::Conflict)
    ));
    check_eq!(visited, 0);
    check_eq!(
        current_view(&guard, workspace())?,
        Some(selected.publication)
    );
    Ok(())
}

#[test]
fn offline_head_checks_charge_current_and_history_payloads_before_decode() -> TestResult {
    for historical in [false, true] {
        let root = tempfile::tempdir()?;
        let (catalog, selected) = selected_catalog(root.path())?;
        let guard = catalog.lock()?;
        let workspace_blob = workspace_key(workspace())?;
        let replace_payload =
            |size: i64| {
                if historical {
                    guard
                        .execute(
                            "UPDATE native_view_v1 SET payload=zeroblob(?1)
                     WHERE workspace=?2 AND epoch=?3 AND sequence=?4",
                            params![
                                size,
                                &workspace_blob,
                                int(selected.publication.identity.epoch)?,
                                int(selected.publication.identity.sequence)?
                            ],
                        )
                        .map_err(|error| sql(&error))
                } else {
                    guard.execute(
                    "UPDATE native_workspace_v1 SET payload=zeroblob(?1) WHERE workspace=?2",
                    params![size, &workspace_blob],
                ).map_err(|error| sql(&error))
                }
            };
        let budget = |bytes| {
            WorkBudget::with_request(
                WorkLimits {
                    reads: 100_000,
                    writes: 0,
                    bytes,
                    mutations: 0,
                },
                quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
            )
        };
        let source = |budget: &mut WorkBudget| {
            read_only::check_source_root_quiescence(&guard, &selected, workspace(), budget)
                .map(|_| ())
        };
        let rollback =
            |budget: &mut WorkBudget| read_only::check_rollback(&guard, &selected, budget);
        let checks: [&dyn Fn(&mut WorkBudget) -> Result<(), IndexError>; 2] = [&source, &rollback];
        for check in checks.into_iter().take(if historical { 2 } else { 1 }) {
            // Both corrupt payloads pass the width preflight. Keeping the other
            // row fields fixed isolates the charge for the payload itself.
            check_eq!(replace_payload(1)?, 1);
            let mut small = budget(u64::MAX);
            let result = check(&mut small);
            check!(
                matches!(result, Err(IndexError::Storage(ref reason)) if reason.contains("digest mismatch")),
                "small corrupt payload reached an unexpected boundary: {result:?}"
            );

            check_eq!(replace_payload(MAX_VIEW_PAYLOAD_BYTES)?, 1);
            let mut large = budget(u64::MAX);
            let result = check(&mut large);
            check!(
                matches!(result, Err(IndexError::Storage(ref reason)) if reason.contains("digest mismatch")),
                "large corrupt payload reached an unexpected boundary: {result:?}"
            );
            let payload_delta = u64::try_from(MAX_VIEW_PAYLOAD_BYTES - 1)?;
            check_eq!(large.usage().bytes, small.usage().bytes + payload_delta);

            let result = check(&mut budget(small.usage().bytes));
            check!(
                matches!(result, Err(IndexError::ResourceLimited)),
                "the larger payload was not refused at admission: {result:?}"
            );
        }
        let size: i64 = if historical {
            guard.query_row(
                "SELECT length(payload) FROM native_view_v1
                 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
                params![
                    &workspace_blob,
                    int(selected.publication.identity.epoch)?,
                    int(selected.publication.identity.sequence)?
                ],
                |row| row.get(0),
            )?
        } else {
            guard.query_row(
                "SELECT length(payload) FROM native_workspace_v1 WHERE workspace=?1",
                params![&workspace_blob],
                |row| row.get(0),
            )?
        };
        check_eq!(size, MAX_VIEW_PAYLOAD_BYTES);
    }
    Ok(())
}

#[test]
fn offline_quiescence_refuses_terminal_receipt_for_another_view_at_the_head() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    let operation = operation(32_002, 210);
    let key = operation.key;
    let attempt = admit_new(&catalog, operation)?;
    let actual = view(2, selected.publication.profile, None);
    check_eq!(
        publish(&catalog, attempt, &selected.publication, actual.clone())?,
        OperationStatus::Published(actual.identity)
    );
    let guard = catalog.lock()?;
    let mut terminal = read_operation(&guard, key)?.ok_or("missing fixture publication")?;
    check_eq!(terminal.status, OperationStatus::Published(actual.identity));
    let alternate = view(2, actual.profile, Some(ObjectRef(digest_id(211))));
    check_ne!(actual.identity, alternate.identity);
    terminal.status = OperationStatus::Published(alternate.identity);
    terminal.published_view = Some(alternate);
    write_operation(&guard, &terminal, false)?;
    check!(matches!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        ),
        Err(IndexError::Conflict)
    ));
    let mut visited = 0;
    check!(matches!(
        visit_quiescent_publications(&guard, &selected, &mut admission_budget(), |_, _, _| {
            visited += 1;
            Ok(())
        }),
        Err(IndexError::Conflict)
    ));
    check_eq!(visited, 0);
    check_eq!(current_view(&guard, workspace())?, Some(actual));
    Ok(())
}

#[test]
fn offline_floor_scan_keeps_valid_old_epoch_and_refuses_corrupt_digest() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    check_eq!(
        catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
        1
    );
    let guard = catalog.lock()?;
    // A prior epoch may retain an exact-history floor after the current head
    // advances. Its own historical head, not the current epoch, bounds it.
    let mut old = view(1, selected.publication.profile, None);
    old.identity.epoch = 6;
    old.identity.digest = old.identity_digest()?;
    write_view(&guard, &old)?;
    let scope = workspace_key(workspace())?;
    let old_digest = fields_digest(b"qidx.native.exactfloor.v1", &(&scope, 6_i64, 1_i64))?;
    check_eq!(
        guard.execute(
            "INSERT INTO native_exact_history_floor_v1
             (workspace,epoch,retain_from_sequence,row_digest) VALUES (?1,?2,?3,?4)",
            params![&scope, 6_i64, 1_i64, old_digest]
        )?,
        1
    );
    check_eq!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        )?,
        selected.publication
    );
    read_only::check_rollback(&guard, &selected, &mut admission_budget())?;
    check_eq!(
        guard.execute(
            "UPDATE native_exact_history_floor_v1 SET row_digest=zeroblob(32)
             WHERE workspace=?1 AND epoch=6",
            params![&scope]
        )?,
        1
    );
    check!(matches!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        ),
        Err(IndexError::Storage(_))
    ));
    check!(matches!(
        read_only::check_rollback(&guard, &selected, &mut admission_budget()),
        Err(IndexError::Storage(_))
    ));
    Ok(())
}

#[test]
fn offline_floor_scan_refuses_valid_digest_beyond_epoch_head() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, selected) = selected_catalog(root.path())?;
    check_eq!(
        catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
        1
    );
    let guard = catalog.lock()?;
    let scope = workspace_key(workspace())?;
    let beyond = int(selected.publication.identity.sequence + 1)?;
    let checksum = fields_digest(b"qidx.native.exactfloor.v1", &(&scope, 7_i64, beyond))?;
    check_eq!(
        guard.execute(
            "UPDATE native_exact_history_floor_v1
             SET retain_from_sequence=?1,row_digest=?2 WHERE workspace=?3 AND epoch=7",
            params![beyond, checksum, &scope]
        )?,
        1
    );
    check!(matches!(
        read_only::check_source_root_quiescence(
            &guard,
            &selected,
            workspace(),
            &mut admission_budget()
        ),
        Err(IndexError::Storage(_))
    ));
    check!(matches!(
        read_only::check_rollback(&guard, &selected, &mut admission_budget()),
        Err(IndexError::Storage(_))
    ));
    Ok(())
}
