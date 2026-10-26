use super::*;
use quanta_index_contract::index::{
    Digest, MapRoot, ObjectRef, ProducerId, ProjectionBinding, ProjectionId, SchemaId, SourceCut,
    ViewRef,
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
