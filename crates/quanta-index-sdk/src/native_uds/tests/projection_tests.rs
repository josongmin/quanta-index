use super::*;
use quanta_index_contract::index::{
    Digest, MapRoot, ObjectRef, ProducerId, ProjectionBinding, ProjectionId, SchemaId, SourceCut,
    ViewRef,
};
use quanta_index_contract::{
    ManifestGeneration, RepoId, RevisionId, RuntimeCatalogIngestBatch, RuntimeSnapshotRecord,
    native_protocol::{
        NATIVE_DSL_PRODUCER_V1, NATIVE_DSL_RUNTIME_CATALOG_MAX_BYTES_V1, NativeDslKindV1,
        NativeDslRuntimeCatalogV1, NativeDslScopeV1,
    },
};

fn projection_fixture(view: ViewRef, projection: ProjectionId) -> NativeProjectionRead {
    NativeProjectionRead {
        view,
        binding: Some(ProjectionBinding {
            projection,
            producer: ProducerId([11; 16]),
            owners: MapRoot(Some(ObjectRef(Digest([12; 32])))),
            source: SourceCut {
                owners: MapRoot(Some(ObjectRef(Digest([13; 32])))),
                paths: MapRoot(Some(ObjectRef(Digest([14; 32])))),
                frontiers: MapRoot(Some(ObjectRef(Digest([15; 32])))),
            },
            schema: SchemaId([16; 16]),
            producer_policy: Digest([17; 32]),
            dependency_context: Digest([18; 32]),
            certificate: ObjectRef(Digest([19; 32])),
        }),
    }
}

#[test]
fn sdk_projection_read_distinguishes_first_full_from_bound_delta()
-> Result<(), Box<dyn std::error::Error>> {
    let view = ViewRef {
        workspace: key(1).workspace,
        epoch: 1,
        sequence: 7,
        digest: Digest([21; 32]),
    };
    let projection = ProjectionId([22; 16]);
    let absent = NativeProjectionRead {
        view,
        binding: None,
    };
    let first = scripted_reply(NativeIndexResponse::Projection(absent), |client, budget| {
        client.read_projection_binding(ViewSelection::Exact(view), projection, budget)
    })?;
    if first != absent {
        return Err("first-FULL projection read invented a binding or changed its view".into());
    }

    let bound = projection_fixture(view, projection);
    for selection in [
        ViewSelection::Exact(view),
        ViewSelection::Latest {
            workspace: view.workspace,
            epoch: view.epoch,
        },
    ] {
        let actual = scripted_reply(NativeIndexResponse::Projection(bound), |client, budget| {
            client.read_projection_binding(selection, projection, budget)
        })?;
        if actual != bound {
            return Err("projection read lost the selected CAS context".into());
        }
    }
    Ok(())
}

#[test]
fn sdk_projection_read_refuses_foreign_view_and_projection_key()
-> Result<(), Box<dyn std::error::Error>> {
    let view = ViewRef {
        workspace: key(2).workspace,
        epoch: 1,
        sequence: 8,
        digest: Digest([23; 32]),
    };
    let projection = ProjectionId([24; 16]);
    let foreign_view = projection_fixture(
        ViewRef {
            sequence: 9,
            ..view
        },
        projection,
    );
    let foreign_key = projection_fixture(view, ProjectionId([25; 16]));
    for reply in [foreign_view, foreign_key] {
        let observed = scripted_reply(NativeIndexResponse::Projection(reply), |client, budget| {
            client.read_projection_binding(ViewSelection::Exact(view), projection, budget)
        });
        if !matches!(observed, Err(NativeUdsError::Protocol(_))) {
            return Err("projection read accepted a foreign view or key".into());
        }
    }
    let wrong_kind = scripted_reply(
        NativeIndexResponse::Staged(ObjectRef(Digest([29; 32]))),
        |client, budget| {
            client.read_projection_binding(ViewSelection::Exact(view), projection, budget)
        },
    );
    if !matches!(wrong_kind, Err(NativeUdsError::Protocol(_))) {
        return Err("projection read accepted a foreign response kind".into());
    }
    Ok(())
}

#[test]
fn admitted_projection_reply_preserves_context_until_owner_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let view = ViewRef {
        workspace: key(3).workspace,
        epoch: 1,
        sequence: 10,
        digest: Digest([26; 32]),
    };
    let projection = ProjectionId([27; 16]);
    let bound = projection_fixture(view, projection);
    let (memory, active, admissions) = publication_scope(false);
    let reply = scripted_reply(NativeIndexResponse::Projection(bound), |client, budget| {
        client.request_publication_admitted(
            NativeIndexRequest::ReadProjection {
                selection: ViewSelection::Exact(view),
                projection,
            },
            budget,
            &memory,
        )
    })?;
    if reply.response() != &NativeIndexResponse::Projection(bound) {
        return Err("admitted projection reply lost binding or CAS context".into());
    }
    if admissions.load(AtomicOrdering::SeqCst) == 0 || active.load(AtomicOrdering::SeqCst) == 0 {
        return Err("projection reply lacked admitted response ownership".into());
    }
    drop(reply);
    if active.load(AtomicOrdering::SeqCst) != 0 {
        return Err("projection reply retained backing after owner drop".into());
    }
    Ok(())
}

#[test]
fn oversized_runtime_catalog_header_refuses_before_socket_dial()
-> Result<(), Box<dyn std::error::Error>> {
    let scope = NativeDslScopeV1 {
        repository: RepoId::new("repo")?,
        revision: RevisionId::new("revision")?,
    };
    let workspace = key(1).workspace;
    let mut input = NativeDslPublicationV1 {
        operation: OperationKey {
            workspace,
            epoch: 1,
            producer: NATIVE_DSL_PRODUCER_V1,
            stream: scope.stream(NativeDslKindV1::RuntimeMetadata),
            stream_epoch: 1,
            sequence: 1,
        },
        expected_view: ViewRef {
            workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([7; 32]),
        },
        event_predecessor: 0,
        authority: Digest([8; 32]),
        changes: NativeDslChangesV1::RuntimeMetadata(NativeDslRuntimeCatalogV1 {
            corpus_certificate: ObjectRef(Digest([9; 32])),
            dirty: Vec::new(),
            catalog: RuntimeCatalogIngestBatch {
                repo_id: scope.repository,
                revision_id: scope.revision,
                generation: ManifestGeneration::new(1),
                overlay_epoch_ms: 7,
                batch_digest: String::new(),
                producer_head_applied_at_ms: 5,
                generation_materialized_at_ms: 6,
                changed_entries: Vec::new(),
                facet_entries: Vec::new(),
                snapshot_entries: vec![RuntimeSnapshotRecord {
                    name: "x".repeat(64 * 1024),
                    doc_ids: Vec::new(),
                }],
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            },
        }),
    };
    let NativeDslChangesV1::RuntimeMetadata(runtime) = &mut input.changes else {
        return Err("fixture has no RuntimeMetadata header".into());
    };
    // Keep the catalog itself within the limit. Only its complete header overflows.
    let limit = NATIVE_DSL_RUNTIME_CATALOG_MAX_BYTES_V1 as usize;
    let mut catalog_bytes = Vec::new();
    ciborium::ser::into_writer(&runtime.catalog, &mut catalog_bytes)?;
    let extra = limit
        .checked_sub(catalog_bytes.len())
        .ok_or("fixture catalog exceeded its size before padding")?;
    runtime.catalog.snapshot_entries[0]
        .name
        .extend(std::iter::repeat_n('x', extra));
    catalog_bytes.clear();
    ciborium::ser::into_writer(&runtime.catalog, &mut catalog_bytes)?;
    assert_eq!(catalog_bytes.len(), limit);
    drop(catalog_bytes);

    let root = tempfile::tempdir()?;
    let client = NativeUdsClient::new(root.path().join("missing.sock"));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
    let (memory, active, _) = publication_scope(false);
    assert!(matches!(
        client.prepare_dsl_publication_v1(&input, &budget),
        Err(NativeUdsError::Protocol(reason))
            if reason.contains("Native DSL output exceeds 16 MiB")
    ));
    assert!(matches!(
        client.prepare_dsl_publication_sealed_admitted_v1(&input, &budget, &memory),
        Err(NativeUdsError::Protocol(reason))
            if reason.contains("Native DSL output exceeds 16 MiB")
    ));
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    assert!(matches!(
        client.publish_dsl_v1(&input, &budget),
        Err(NativeUdsError::Protocol(reason))
            if reason.contains("Native DSL output exceeds 16 MiB")
    ));

    use quanta_index_ipc::BudgetInterruptionV1;

    #[derive(Debug)]
    struct InterruptDuringHeaderMeasurement {
        polls: AtomicU64,
        first: BudgetInterruptionV1,
    }

    impl quanta_index_ipc::RequestExecutionObserverV1 for InterruptDuringHeaderMeasurement {
        fn interruption(&self) -> Option<BudgetInterruptionV1> {
            match self.polls.fetch_add(1, AtomicOrdering::SeqCst) {
                0 | 1 => None,
                // Let admission and measurement entry pass, then interrupt inside counting.
                2 => Some(self.first),
                _ => Some(BudgetInterruptionV1::Cancelled),
            }
        }
    }

    let observe = |first| {
        RequestBudgetV1::observing_execution_admitted(
            Arc::new(InterruptDuringHeaderMeasurement {
                polls: AtomicU64::new(0),
                first,
            }),
            &memory,
        )
    };
    let cancelled_budget = observe(BudgetInterruptionV1::Cancelled)?;
    assert!(matches!(
        client.publish_dsl_v1(&input, &cancelled_budget),
        Err(NativeUdsError::Refused(NativeIndexWireError {
            code: NativeIndexErrorCode::Cancelled,
            ..
        }))
    ));
    drop(cancelled_budget);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);

    let expired_budget = observe(BudgetInterruptionV1::DeadlineExceeded {
        over_by: Duration::ZERO,
    })?;
    assert!(matches!(
        client.publish_dsl_v1(&input, &expired_budget),
        Err(NativeUdsError::Transport(reason)) if reason.contains("deadline exceeded")
    ));
    drop(expired_budget);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);

    let NativeDslChangesV1::RuntimeMetadata(runtime) = &mut input.changes else {
        return Err("fixture has no RuntimeMetadata header".into());
    };
    runtime.catalog.snapshot_entries[0].name.truncate(1);
    assert!(matches!(
        client.prepare_dsl_publication_v1(&input, &budget),
        Err(NativeUdsError::Transport(_))
    ));
    assert!(matches!(
        client.prepare_dsl_publication_sealed_admitted_v1(&input, &budget, &memory),
        Err(NativeUdsError::Transport(_))
    ));
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    assert!(matches!(
        client.publish_dsl_v1(&input, &budget),
        Err(NativeUdsError::Transport(_) | NativeUdsError::Unknown(_))
    ));
    Ok(())
}
