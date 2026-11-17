use super::*;
use crate::index::{CollectionId, Digest, SlotId, TenantId, WorktreeId};

fn fixture() -> Result<NativeRootScopedViewV1, Box<dyn std::error::Error>> {
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 9,
            sequence: 17,
            digest: Digest([0; 32]),
        },
        profile: ObjectRef(Digest([11; 32])),
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members: MapRoot(None),
        coverage: MapRoot(None),
    };
    view.identity.digest = view.identity_digest()?;
    Ok(NativeRootScopedViewV1 {
        view,
        root_nonce: Digest([31; 32]),
        root_device: 0x1234,
        root_inode: 0x5678,
    })
}

#[test]
fn root_scoped_wire_keeps_named_request_and_all_four_metadata_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?;
    let request = NativeIndexRequest::ObserveRootView(ViewSelection::Exact(expected.view.identity));
    let response = NativeIndexResponse::RootScopedView(expected.clone());
    let request_json = serde_json::to_value(&request)?;
    assert!(request_json.get("ObserveRootView").is_some());
    let response_json = serde_json::to_value(&response)?;
    let fields = response_json
        .get("RootScopedView")
        .and_then(serde_json::Value::as_object)
        .ok_or("root view fields missing")?;
    assert_eq!(fields.len(), 4);
    assert_eq!(fields.get("root_device"), Some(&serde_json::json!(0x1234)));
    assert_eq!(fields.get("root_inode"), Some(&serde_json::json!(0x5678)));
    for value in [&request_json, &response_json] {
        assert!(value.is_object());
    }
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&response, &mut bytes)?;
    assert_eq!(
        ciborium::de::from_reader::<NativeIndexResponse, _>(bytes.as_slice())?,
        response
    );
    assert_eq!(
        serde_json::from_value::<NativeIndexResponse>(response_json)?,
        response
    );
    Ok(())
}

#[test]
fn root_scoped_wire_refuses_missing_and_invented_metadata_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_value(fixture()?)?;
    for field in ["view", "root_nonce", "root_device", "root_inode"] {
        let mut bad = json.clone();
        let removed = bad.as_object_mut()
            .ok_or("fixture not an object")?
            .remove(field);
        assert!(removed.is_some());
        assert!(serde_json::from_value::<NativeRootScopedViewV1>(bad).is_err());
    }
    let mut bad = json;
    let previous = bad.as_object_mut()
        .ok_or("fixture not an object")?
        .insert("peer_pid".into(), serde_json::json!(123));
    assert!(previous.is_none());
    assert!(serde_json::from_value::<NativeRootScopedViewV1>(bad).is_err());
    Ok(())
}
