use super::{
    MAX_REPOMAP_RANK_QUERY_BYTES, MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES, NativeRepoMapQuery,
    native_repomap_rank_policy,
};
use crate::{
    CompiledRepoMapProjectionEntryV1, FileId, RepoMapDocType, RepoMapNodeRef, RepoRelativePath,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn certified_projection_row_is_closed_and_range_checked() -> TestResult {
    let row = CompiledRepoMapProjectionEntryV1 {
        subject: RepoMapNodeRef::File(FileId::new("file-1")),
        doc_type: RepoMapDocType::File,
        owner_path: RepoRelativePath::new("src/lib.rs"),
        search_text: "file-1 src/lib.rs".into(),
        final_score_millis: 975,
    };
    let wire = serde_json::to_string(&row)?;
    if wire
        != r#"{"subject":{"File":"file-1"},"doc_type":"File","owner_path":"src/lib.rs","search_text":"file-1 src/lib.rs","final_score_millis":975}"#
        || serde_json::from_str::<CompiledRepoMapProjectionEntryV1>(&wire)? != row
    {
        return Err("compiled row wire changed".into());
    }
    for invalid in [
        wire.replace("\"doc_type\":\"File\"", "\"doc_type\":\"Symbol\""),
        wire.replace("\"final_score_millis\":975", "\"final_score_millis\":1001"),
        wire.replace("\"final_score_millis\":975", "\"final_score_millis\":-1"),
        wire.replace("\"owner_path\":\"src/lib.rs\",", ""),
        wire.replace(
            "\"search_text\":",
            "\"search_text\":\"duplicate\",\"search_text\":",
        ),
        wire.replace("\"subject\":", "\"unexpected\":0,\"subject\":"),
        wire.replace(
            "\"search_text\":\"file-1 src/lib.rs\"",
            &format!(
                "\"search_text\":\"{}\"",
                "x".repeat(MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES.saturating_add(1))
            ),
        ),
        wire.replace(
            "\"subject\":{\"File\":\"file-1\"}",
            &format!(
                "\"subject\":{{\"File\":\"{}\"}}",
                "x".repeat(super::MAX_REPOMAP_RANK_SUBJECT_BYTES.saturating_add(1))
            ),
        ),
    ] {
        if serde_json::from_str::<CompiledRepoMapProjectionEntryV1>(&invalid).is_ok() {
            return Err("invalid certified row was accepted".into());
        }
    }
    Ok(())
}

#[test]
fn rank_query_is_closed_bounded_and_keeps_unresolvable_typed_focus() -> TestResult {
    let wire = r#"{"query_text":"needle","focus_subjects":[{"subject_identity":"module-1","subject_doc_type":"Module"}],"top_k":5,"token_budget":64}"#;
    let request: NativeRepoMapQuery = serde_json::from_str(wire)?;
    if request.focus_subjects.len() != 1 || serde_json::to_string(&request)? != wire {
        return Err("native RepoMap query wire changed".into());
    }
    for invalid in [
        wire.replace("\"query_text\":\"needle\"", "\"query_text\":\"   \""),
        wire.replace("\"top_k\":5", "\"top_k\":0"),
        wire.replace("\"token_budget\":64", "\"token_budget\":0"),
        wire.replace("\"focus_subjects\":", "\"unknown\":0,\"focus_subjects\":"),
        wire.replace(
            "\"query_text\":\"needle\"",
            &format!(
                "\"query_text\":\"{}\"",
                "x".repeat(MAX_REPOMAP_RANK_QUERY_BYTES.saturating_add(1))
            ),
        ),
    ] {
        if serde_json::from_str::<NativeRepoMapQuery>(&invalid).is_ok() {
            return Err("invalid rank query was accepted".into());
        }
    }
    if native_repomap_rank_policy().0
        != [
            0x7e, 0xa4, 0x2a, 0xfe, 0x0d, 0xd0, 0xc9, 0xff, 0x2a, 0x08, 0xdc, 0xcd, 0x0e, 0x8d,
            0xc1, 0x7a, 0xae, 0x65, 0x46, 0xef, 0x88, 0x46, 0xd4, 0x12, 0xe5, 0x40, 0x14, 0xe9,
            0xfb, 0x84, 0x0f, 0x21,
        ]
    {
        return Err("rank policy identity changed".into());
    }
    Ok(())
}

#[test]
fn rank_hit_and_result_refuse_unbound_or_malformed_output() -> TestResult {
    use super::{NativeRepoMapHit, NativeRepoMapResult};
    use crate::index::{
        CollectionId, Digest, IndexInstanceId, SlotId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };
    let wire = r#"{"subject":{"File":"file-1"},"owner_path":"src/lib.rs","final_score_millis":975,"rank":1}"#;
    let hit: NativeRepoMapHit = serde_json::from_str(wire)?;
    if serde_json::to_string(&hit)? != wire {
        return Err("rank hit wire changed".into());
    }
    for invalid in [
        wire.replace("\"rank\":1", "\"rank\":0"),
        wire.replace("\"rank\":1", "\"rank\":1,\"rank\":1"),
        wire.replace("\"rank\":1", "\"rank\":1,\"unknown\":0"),
        wire.replace("\"final_score_millis\":975", "\"final_score_millis\":1001"),
        wire.replace("\"File\"", "\"Module\""),
    ] {
        if serde_json::from_str::<NativeRepoMapHit>(&invalid).is_ok() {
            return Err("invalid rank hit accepted".into());
        }
    }
    let mut result = NativeRepoMapResult {
        view: ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 7,
            sequence: 9,
            digest: Digest([8; 32]),
        },
        instance: IndexInstanceId([5; 16]),
        policy: native_repomap_rank_policy(),
        hits: vec![hit],
        dropped_entries_count: 2,
        drop_reason_codes: vec!["top_k_exhausted".into()],
        degraded_reason_codes: Vec::new(),
        exhaustive: true,
    };
    let value = serde_json::to_value(&result)?;
    if serde_json::from_value::<NativeRepoMapResult>(value.clone())? != result {
        return Err("view-bound rank result changed".into());
    }
    for field in ["view", "policy", "exhaustive"] {
        let mut invalid = value.clone();
        let _removed = invalid
            .as_object_mut()
            .ok_or("rank result must be an object")?
            .remove(field);
        if serde_json::from_value::<NativeRepoMapResult>(invalid).is_ok() {
            return Err("unbound rank result accepted".into());
        }
    }
    result.policy = Digest([0; 32]);
    if serde_json::to_vec(&result).is_ok() {
        return Err("unrecognized rank result policy accepted".into());
    }
    result.policy = native_repomap_rank_policy();
    result.hits.first_mut().ok_or("rank hit absent")?.rank = 2;
    if serde_json::to_vec(&result).is_ok() {
        return Err("nonconsecutive result rank accepted".into());
    }
    Ok(())
}

fn wire_view() -> crate::index::ViewRef {
    use crate::index::{CollectionId, Digest, SlotId, TenantId, ViewRef, WorkspaceKey, WorktreeId};
    ViewRef {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch: 7,
        sequence: 9,
        digest: Digest([8; 32]),
    }
}

#[test]
fn rank_request_envelope_roundtrips_canonical_wire() -> TestResult {
    use crate::{
        index::{IndexInstanceId, ViewSelection},
        native_protocol::{NativeIndexRequest, NativeIndexRequestEnvelope},
    };
    let envelope = NativeIndexRequestEnvelope {
        request_id: 17,
        payload: NativeIndexRequest::QueryRepoMap {
            selection: ViewSelection::Exact(wire_view()),
            instance: IndexInstanceId([5; 16]),
            query: NativeRepoMapQuery {
                query_text: "needle".into(),
                focus_subjects: Vec::new(),
                top_k: 5,
                token_budget: 64,
            },
        },
    };
    let json = serde_json::to_vec(&envelope)?;
    if !String::from_utf8(json.clone())?.contains("\"QueryRepoMap\"")
        || serde_json::from_slice::<NativeIndexRequestEnvelope>(&json)? != envelope
    {
        return Err("native RepoMap request envelope changed".into());
    }
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&envelope, &mut cbor)?;
    if ciborium::de::from_reader::<NativeIndexRequestEnvelope, _>(cbor.as_slice())? != envelope {
        return Err("native RepoMap request CBOR changed".into());
    }
    Ok(())
}

#[test]
fn borrowed_rank_response_matches_owned_wire() -> TestResult {
    use crate::{
        index::IndexInstanceId,
        native_protocol::{
            NativeIndexResponse, NativeRepoMapHit, NativeRepoMapResult, NativeRepoMapResultRef,
        },
    };
    use serde::{Serialize, Serializer};

    struct Borrowed<'a>(NativeRepoMapResultRef<'a>);
    impl Serialize for Borrowed<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_repomap_ranked_ref(&self.0, serializer)
        }
    }

    let result = NativeRepoMapResult {
        view: wire_view(),
        instance: IndexInstanceId([5; 16]),
        policy: native_repomap_rank_policy(),
        hits: vec![NativeRepoMapHit {
            subject: RepoMapNodeRef::File(FileId::new("file-1")),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            final_score_millis: 975,
            rank: 1,
        }],
        dropped_entries_count: 2,
        drop_reason_codes: vec!["top_k_exhausted".into()],
        degraded_reason_codes: Vec::new(),
        exhaustive: true,
    };
    let owned = NativeIndexResponse::RepoMapRanked(result.clone());
    let borrowed = Borrowed(NativeRepoMapResultRef::from(&result));
    let owned_json = serde_json::to_vec(&owned)?;
    if owned_json != serde_json::to_vec(&borrowed)?
        || serde_json::from_slice::<NativeIndexResponse>(&owned_json)? != owned
    {
        return Err("borrowed RepoMap response JSON differs from owned wire".into());
    }
    let mut owned_cbor = Vec::new();
    ciborium::ser::into_writer(&owned, &mut owned_cbor)?;
    let mut borrowed_cbor = Vec::new();
    ciborium::ser::into_writer(&borrowed, &mut borrowed_cbor)?;
    if owned_cbor != borrowed_cbor
        || ciborium::de::from_reader::<NativeIndexResponse, _>(owned_cbor.as_slice())? != owned
    {
        return Err("borrowed RepoMap response CBOR differs from owned wire".into());
    }
    Ok(())
}

#[test]
fn rank_reason_codes_are_closed_categorized_and_canonical() -> TestResult {
    use crate::{
        index::IndexInstanceId,
        native_protocol::{NativeRepoMapResult, NativeRepoMapResultRef},
    };

    let valid = NativeRepoMapResult {
        view: wire_view(),
        instance: IndexInstanceId([5; 16]),
        policy: native_repomap_rank_policy(),
        hits: Vec::new(),
        dropped_entries_count: 2,
        drop_reason_codes: vec!["token_budget_exhausted".into(), "top_k_exhausted".into()],
        degraded_reason_codes: vec![
            "query_terms_unmatched".into(),
            "token_budget_floor_applied".into(),
        ],
        exhaustive: true,
    };
    let valid_wire = serde_json::to_value(&valid)?;
    if !NativeRepoMapResultRef::from(&valid).valid_native_rank_result()
        || serde_json::from_value::<NativeRepoMapResult>(valid_wire.clone())? != valid
    {
        return Err("canonical rank reason lists were refused".into());
    }

    for (field, reasons) in [
        ("drop_reason_codes", vec!["unknown"]),
        ("drop_reason_codes", vec!["query_terms_unmatched"]),
        (
            "drop_reason_codes",
            vec!["top_k_exhausted", "top_k_exhausted"],
        ),
        (
            "drop_reason_codes",
            vec!["top_k_exhausted", "token_budget_exhausted"],
        ),
        ("degraded_reason_codes", vec!["unknown"]),
        ("degraded_reason_codes", vec!["top_k_exhausted"]),
        (
            "degraded_reason_codes",
            vec!["query_terms_unmatched", "query_terms_unmatched"],
        ),
        (
            "degraded_reason_codes",
            vec!["token_budget_floor_applied", "query_terms_unmatched"],
        ),
    ] {
        let mut invalid = valid.clone();
        let target = match field {
            "drop_reason_codes" => &mut invalid.drop_reason_codes,
            "degraded_reason_codes" => &mut invalid.degraded_reason_codes,
            _ => return Err("unexpected reason field in test".into()),
        };
        *target = reasons.iter().map(|reason| (*reason).to_owned()).collect();
        if serde_json::to_value(&invalid).is_ok() {
            return Err("invalid rank reason list serialized".into());
        }
        let mut invalid_wire = valid_wire.clone();
        let object = invalid_wire
            .as_object_mut()
            .ok_or("rank result wire must be an object")?;
        let _prior = object.insert(field.into(), serde_json::to_value(reasons)?);
        if serde_json::from_value::<NativeRepoMapResult>(invalid_wire).is_ok() {
            return Err("invalid rank reason list deserialized".into());
        }
    }
    Ok(())
}

#[test]
fn projection_output_request_requires_explicit_children_field() -> TestResult {
    use crate::{
        index::{IngressRef, ObjectRef, OperationKey, ProducerId, StreamId},
        native_protocol::{NativeIndexRequest, NativeIndexRequestEnvelope},
    };
    let request = NativeIndexRequestEnvelope {
        request_id: 19,
        payload: NativeIndexRequest::StageProjectionOutput {
            ingress: IngressRef {
                operation: OperationKey {
                    workspace: wire_view().workspace,
                    epoch: 7,
                    producer: ProducerId([10; 16]),
                    stream: StreamId([11; 16]),
                    stream_epoch: 1,
                    sequence: 2,
                },
                fence: 3,
            },
            bytes: b"certified graph output".to_vec(),
            children: crate::native_protocol::NativeProjectionOutputChildren::new(vec![
                ObjectRef(crate::index::Digest([12; 32])),
            ])?,
        },
    };
    let mut value = serde_json::to_value(&request)?;
    let stage = value
        .get_mut("payload")
        .and_then(|payload| payload.get_mut("StageProjectionOutput"))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("projection request wire changed")?;
    let _removed = stage.remove("children");
    if serde_json::from_value::<NativeIndexRequestEnvelope>(value).is_ok() {
        return Err("projection output silently defaulted missing child custody".into());
    }
    Ok(())
}
