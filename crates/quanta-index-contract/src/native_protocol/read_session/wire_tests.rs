//! Literal public wire gold; expected keys are independent of Rust DTO fields.

use super::*;
use crate::native_protocol::NativeIndexRequest;

fn hybrid_wire() -> serde_json::Value {
    serde_json::json!({"ReadSession":{"SearchHybridFile":{
        "session":vec![7_u8;32],"text_instance":vec![8_u8;16],"semantic_instance":vec![9_u8;16],
        "text_query":{"syntax":"native","query_text":"needle","constraints":{"language_any_of":[]},"top_k":2},
        "semantic_query":{"query_text":"meaning","constraints":{"language_any_of":[]},"top_k":2},"file_top_k":1
    }}})
}

#[test]
fn native_session_hybrid_has_fixed_json_and_cbor_authority_shape()
-> Result<(), Box<dyn std::error::Error>> {
    let wire = hybrid_wire();
    let request: NativeIndexRequest = serde_json::from_value(wire.clone())?;
    assert_eq!(serde_json::to_value(&request)?, wire);
    let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
        text_query,
        semantic_query,
        file_top_k,
        ..
    }) = &request
    else {
        return Err("fixed Hybrid wire decoded as another operation".into());
    };
    validate_native_hybrid_file_query(text_query, semantic_query, *file_top_k)?;
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&request, &mut cbor)?;
    assert_eq!(
        ciborium::de::from_reader::<NativeIndexRequest, _>(cbor.as_slice())?,
        request
    );
    Ok(())
}

#[test]
fn native_session_wire_refuses_missing_unknown_and_cross_lane_authority()
-> Result<(), Box<dyn std::error::Error>> {
    for missing in [
        "session",
        "text_instance",
        "semantic_instance",
        "file_top_k",
    ] {
        let mut wire = hybrid_wire();
        assert!(
            wire["ReadSession"]["SearchHybridFile"]
                .as_object_mut()
                .ok_or("Hybrid wire object absent")?
                .remove(missing)
                .is_some()
        );
        assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    }
    let mut wire = hybrid_wire();
    wire["ReadSession"]["SearchHybridFile"]["operator_uid"] = serde_json::json!(0);
    assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    let request: NativeIndexRequest = serde_json::from_value(hybrid_wire())?;
    let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
        text_query,
        mut semantic_query,
        file_top_k,
        ..
    }) = request
    else {
        return Err("Hybrid wire decoded as another operation".into());
    };
    semantic_query.lexical_scope = Some(text_query.clone());
    assert!(validate_native_hybrid_file_query(&text_query, &semantic_query, file_top_k).is_err());
    let page = serde_json::json!({"ReadSession":{"SearchTextPage":{"session":vec![7_u8;32],"instance":vec![8_u8;16],
        "query":{"syntax":"native","query_text":"needle","constraints":{"language_any_of":[]},"top_k":2},"after":null}}});
    assert!(serde_json::from_value::<NativeIndexRequest>(page.clone()).is_ok());
    let mut missing = page;
    assert!(
        missing["ReadSession"]["SearchTextPage"]
            .as_object_mut()
            .ok_or("page wire absent")?
            .remove("after")
            .is_some()
    );
    assert!(serde_json::from_value::<NativeIndexRequest>(missing).is_err());
    Ok(())
}

fn profile_wire_gold() -> serde_json::Value {
    serde_json::json!({"ReadSessionProfile": {
        "session": vec![7_u8; 32],
        "view": {
            "identity": {
                "workspace": {"tenant": vec![1_u8; 16], "collection": vec![2_u8; 16],
                    "worktree": vec![3_u8; 16], "slot": vec![4_u8; 16]},
                "epoch": 1, "sequence": 9, "digest": vec![5_u8; 32]
            },
            "profile": vec![6_u8; 32],
            "source": {"owners": null, "paths": null, "frontiers": null},
            "projections": null, "members": null, "coverage": null
        },
        "profile": {"instances": [{
            "instance": vec![8_u8; 16], "kind": vec![9_u8; 16],
            "group": vec![10_u8; 16], "activation": "CoreRequired",
            "input_domain": "SourceLocal", "schema": vec![11_u8; 16],
            "policy": vec![12_u8; 32], "format": vec![13_u8; 16]
        }]}
    }})
}

#[test]
fn retained_profile_read_has_fixed_json_and_cbor_shape() -> Result<(), Box<dyn std::error::Error>> {
    let gold = serde_json::json!({"ReadSession": {"ReadProfile": {"session": vec![7_u8; 32]}}});
    let request: NativeIndexRequest = serde_json::from_value(gold.clone())?;
    assert_eq!(serde_json::to_value(&request)?, gold);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&request, &mut bytes)?;
    // Inspect literal keys independently of round-tripping the same DTO.
    assert_eq!(
        ciborium::de::from_reader::<serde_json::Value, _>(bytes.as_slice())?,
        gold
    );
    assert_eq!(
        ciborium::de::from_reader::<NativeIndexRequest, _>(bytes.as_slice())?,
        request
    );
    Ok(())
}

#[test]
fn retained_profile_reply_preserves_full_canonical_profile_and_view()
-> Result<(), Box<dyn std::error::Error>> {
    let gold = profile_wire_gold();
    let reply: crate::native_protocol::NativeIndexResponse = serde_json::from_value(gold.clone())?;
    assert_eq!(serde_json::to_value(&reply)?, gold);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&reply, &mut bytes)?;
    assert_eq!(
        ciborium::de::from_reader::<serde_json::Value, _>(bytes.as_slice())?,
        gold
    );
    assert_eq!(
        ciborium::de::from_reader::<crate::native_protocol::NativeIndexResponse, _>(
            bytes.as_slice()
        )?,
        reply
    );
    Ok(())
}

#[test]
fn retained_profile_wire_rejects_missing_authority_and_unknown_fields()
-> Result<(), Box<dyn std::error::Error>> {
    for wire in [
        serde_json::json!({"ReadSession": {"ReadProfile": {}}}),
        serde_json::json!({"ReadSession": {"ReadProfile": {"session": vec![7_u8; 32], "operator_uid": 0}}}),
    ] {
        assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    }
    for field in ["session", "view", "profile"] {
        let mut wire = profile_wire_gold();
        assert!(
            wire["ReadSessionProfile"]
                .as_object_mut()
                .ok_or("missing profile object")?
                .remove(field)
                .is_some()
        );
        assert!(
            serde_json::from_value::<crate::native_protocol::NativeIndexResponse>(wire).is_err()
        );
    }
    let mut wire = profile_wire_gold();
    wire["ReadSessionProfile"]["selected_instance"] = serde_json::json!(vec![8_u8; 16]);
    assert!(serde_json::from_value::<crate::native_protocol::NativeIndexResponse>(wire).is_err());
    Ok(())
}

#[test]
fn retained_profile_borrowed_serializer_preserves_fixed_wire()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::native_protocol::NativeIndexResponse;
    let gold = profile_wire_gold();
    let reply: NativeIndexResponse = serde_json::from_value(gold.clone())?;
    let NativeIndexResponse::ReadSessionProfile(profile) = reply else {
        return Err("profile gold decoded as another response".into());
    };
    struct Borrowed<'a>(&'a NativeReadSessionProfile);
    impl serde::Serialize for Borrowed<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_read_session_profile_ref(
                self.0.session,
                &self.0.view,
                &self.0.profile,
                serializer,
            )
        }
    }
    let borrowed = Borrowed(&profile);
    assert_eq!(serde_json::to_value(&borrowed)?, gold);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&borrowed, &mut bytes)?;
    assert_eq!(
        ciborium::de::from_reader::<serde_json::Value, _>(bytes.as_slice())?,
        gold
    );
    assert_eq!(
        ciborium::de::from_reader::<NativeIndexResponse, _>(bytes.as_slice())?,
        NativeIndexResponse::ReadSessionProfile(profile)
    );
    Ok(())
}

fn history_session_wire_gold(order: &str) -> serde_json::Value {
    serde_json::json!({"ReadSession":{"SearchHistory":{
        "session":vec![7_u8;32],"instance":vec![8_u8;16],
        "query":{
            "text_query":{"syntax":"native","query_text":"type:commit fix author:alice",
                "constraints":{"language_any_of":[]},"top_k":2},
            "order":order
        },
        "after":null
    }}})
}

#[test]
fn native_history_cut_read_keeps_new_session_wire_tag_and_exact_reply_identity()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::native_protocol::NativeIndexResponse;
    use crate::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
    let request_gold = serde_json::json!({"ReadSession":{"ReadHistoryCut":{
        "session":vec![7_u8;32], "instance":vec![8_u8;16]
    }}});
    let request: NativeIndexRequest = serde_json::from_value(request_gold.clone())?;
    assert_eq!(serde_json::to_value(&request)?, request_gold);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&request, &mut bytes)?;
    assert_eq!(
        ciborium::de::from_reader::<NativeIndexRequest, _>(bytes.as_slice())?,
        request
    );

    let view: CatalogView =
        serde_json::from_value(profile_wire_gold()["ReadSessionProfile"]["view"].clone())?;
    let reply = NativeIndexResponse::HistoryCut(NativeReadSessionHistoryCutV1 {
        session: NativeReadSessionId([7; 32]),
        view: view.identity,
        instance: IndexInstanceId([8; 16]),
        member: ObjectRef(Digest([9; 32])),
        source_input: ObjectRef(Digest([12; 32])),
        cut: HistorySourceCutV1 {
            generation: crate::index::AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("repo")?,
                revision_id: RevisionId::new("revision")?,
                generation: ManifestGeneration::new(4),
            },
            durable_epoch: AuxEpochV1::new(2),
            snapshot_digest: Digest([10; 32]),
            replay_start: Digest([11; 32]),
        },
    });
    let wire = serde_json::to_value(&reply)?;
    assert_eq!(
        wire["HistoryCut"]["source_input"],
        serde_json::to_value(ObjectRef(Digest([12; 32])))?
    );
    let mut old_wire = wire;
    let _removed = old_wire["HistoryCut"]
        .as_object_mut()
        .ok_or("History cut response is not a record")?
        .remove("source_input");
    assert!(serde_json::from_value::<NativeIndexResponse>(old_wire).is_err());
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&reply, &mut bytes)?;
    assert_eq!(
        ciborium::de::from_reader::<NativeIndexResponse, _>(bytes.as_slice())?,
        reply
    );
    Ok(())
}

#[test]
fn native_history_read_session_accepts_existing_history_request_for_both_orders()
-> Result<(), Box<dyn std::error::Error>> {
    for order in ["recency", "relevance"] {
        let gold = history_session_wire_gold(order);
        let request: NativeIndexRequest = serde_json::from_value(gold.clone())?;
        assert_eq!(serde_json::to_value(&request)?, gold);
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&request, &mut bytes)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(bytes.as_slice())?,
            request
        );
    }
    Ok(())
}

#[test]
fn native_history_read_session_wire_refuses_missing_authority_and_unknown_fields()
-> Result<(), Box<dyn std::error::Error>> {
    for field in ["session", "instance", "query", "after"] {
        let mut wire = history_session_wire_gold("recency");
        assert!(
            wire["ReadSession"]["SearchHistory"]
                .as_object_mut()
                .ok_or("History request object missing")?
                .remove(field)
                .is_some()
        );
        assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    }
    let mut wire = history_session_wire_gold("recency");
    wire["ReadSession"]["SearchHistory"]["operator_uid"] = serde_json::json!(0);
    assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    let wire = history_session_wire_gold("oldest");
    assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    Ok(())
}
