//! QI-BB-026 — the quarantine inventory, target and discard on the control
//! wire.
//!
//! Every DTO round-trips through CBOR and JSON as a value and rides the
//! adjacent-tagged control envelope under its own kind; every shape that
//! would let a discard name something the inventory never listed is
//! refused at decode: an empty path, reason or file name, a file name that
//! is not one path segment, an entry filed under the wrong track, an
//! unknown target or outcome variant, a payload before its kind, and an
//! inventory request that carries anything at all.

#![forbid(unsafe_code)]

use ciborium::value::Value;
use quanta_index_contract::ipc::{
    QuarantineDiscardAck, QuarantineDiscardOutcomeDtoV1, QuarantineDiscardRequest,
    QuarantineInventoryRequest, QuarantineInventoryV1, QuarantineTargetV1,
    QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneTrackKind,
};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn decode<T>(bytes: &[u8]) -> Result<T, ciborium::de::Error<std::io::Error>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    ciborium::de::from_reader(bytes)
}

fn lexical_entry() -> QuarantinedGenerationEntryV1 {
    QuarantinedGenerationEntryV1 {
        track: SearchPlaneTrackKind::Lexical,
        path: "/state/indexes/lexical/repo/rev/g4".to_string(),
        reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
        detail: "identity file does not decode".to_string(),
    }
}

fn semantic_entry() -> QuarantinedGenerationEntryV1 {
    QuarantinedGenerationEntryV1 {
        track: SearchPlaneTrackKind::Semantic,
        path: "/state/indexes/semantic/repo-legacy".to_string(),
        reason: "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT".to_string(),
        detail: String::new(),
    }
}

fn repo_map_entry() -> QuarantinedRepoMapFileEntryV1 {
    QuarantinedRepoMapFileEntryV1 {
        file_name: "stale--marker.json".to_string(),
        reason: "snapshot does not decode".to_string(),
    }
}

fn inventory() -> QuarantineInventoryV1 {
    QuarantineInventoryV1 {
        lexical: vec![lexical_entry()],
        semantic: vec![semantic_entry()],
        repo_map: vec![repo_map_entry()],
    }
}

/// A fixture as a generic CBOR value bent into a shape the typed
/// constructors refuse to build; `None` from `edit` means the fixture lacks
/// the path the forgery edits.
fn forged<T: serde::Serialize>(
    fixture: &T,
    edit: impl FnOnce(&mut Value) -> Option<()>,
) -> Result<Vec<u8>, String> {
    let mut value = Value::serialized(fixture).map_err(|err| err.to_string())?;
    edit(&mut value).ok_or_else(|| "the fixture has the path the forgery edits".to_string())?;
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(&value, &mut buf).map_err(|err| err.to_string())?;
    Ok(buf)
}

/// The entry `key` of a CBOR map.
fn field<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    value
        .as_map_mut()?
        .iter_mut()
        .find(|(name, _)| name.as_text() == Some(key))
        .map(|(_, entry)| entry)
}

/// Element `index` of a CBOR array.
fn item(value: &mut Value, index: usize) -> Option<&mut Value> {
    value.as_array_mut()?.get_mut(index)
}

/// Overwrite the text `value` with `text`; `None` when the fixture holds
/// something other than text there, which no forgery here intends.
fn set_text(value: &mut Value, text: &str) -> Option<()> {
    let Value::Text(current) = value else {
        return None;
    };
    *current = text.to_string();
    Some(())
}

fn expect_refused<T>(what: &str, bytes: &[u8]) -> TestRes
where
    T: for<'de> serde::Deserialize<'de> + std::fmt::Debug,
{
    decode::<T>(bytes).map_or_else(
        |_| Ok(()),
        |decoded| Err(format!("{what} must be refused at decode, got {decoded:?}").into()),
    )
}

#[test]
fn every_dto_round_trips_and_rides_the_control_envelope() -> TestRes {
    let value = inventory();
    let decoded: QuarantineInventoryV1 = decode(&encode(&value)?)?;
    if decoded != value {
        return Err(format!("inventory round trip drifted: {decoded:?}").into());
    }
    let decoded: QuarantineInventoryV1 = serde_json::from_str(&serde_json::to_string(&value)?)?;
    if decoded != value {
        return Err(format!("inventory JSON round trip drifted: {decoded:?}").into());
    }
    let empty = QuarantineInventoryV1::default();
    let decoded: QuarantineInventoryV1 = decode(&encode(&empty)?)?;
    if decoded != empty {
        return Err("an empty inventory round-trips as empty".into());
    }

    for (target, outcome) in [
        (
            QuarantineTargetV1::Generation(lexical_entry()),
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes: u64::MAX },
        ),
        (
            QuarantineTargetV1::RepoMapFile(repo_map_entry()),
            QuarantineDiscardOutcomeDtoV1::Absent,
        ),
    ] {
        let request = QuarantineDiscardRequest {
            target: target.clone(),
        };
        let decoded: QuarantineDiscardRequest = decode(&encode(&request)?)?;
        if decoded != request {
            return Err(format!("discard request round trip drifted: {decoded:?}").into());
        }
        let ack = QuarantineDiscardAck { target, outcome };
        let decoded: QuarantineDiscardAck = decode(&encode(&ack)?)?;
        if decoded != ack {
            return Err(format!("discard ack round trip drifted: {decoded:?}").into());
        }
        let decoded: QuarantineDiscardAck = serde_json::from_str(&serde_json::to_string(&ack)?)?;
        if decoded != ack {
            return Err(format!("discard ack JSON round trip drifted: {decoded:?}").into());
        }
        // The target and outcome are adjacent-tagged under `kind`, so a
        // peer without a variant refuses it as unknown rather than
        // mis-decoding it.
        let json = serde_json::to_value(&ack)?;
        let target_kind = json
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(serde_json::Value::as_str);
        let outcome_kind = json
            .get("outcome")
            .and_then(|outcome| outcome.get("kind"))
            .and_then(serde_json::Value::as_str);
        let expected = match ack.target {
            QuarantineTargetV1::Generation(_) => ("Generation", "Discarded"),
            QuarantineTargetV1::RepoMapFile(_) => ("RepoMapFile", "Absent"),
        };
        if (target_kind, outcome_kind) != (Some(expected.0), Some(expected.1)) {
            return Err(format!("ack kind tags: {json}").into());
        }
    }

    let request = SearchPlaneControlIpcRequestEnvelope {
        request_id: 91,
        payload: SearchPlaneControlIpcRequest::QuarantineInventory(QuarantineInventoryRequest),
    };
    let decoded: SearchPlaneControlIpcRequestEnvelope = decode(&encode(&request)?)?;
    if decoded != request {
        return Err(format!("inventory request envelope drifted: {decoded:?}").into());
    }
    let json = serde_json::to_value(&request.payload)?;
    if json.get("kind").and_then(serde_json::Value::as_str) != Some("QuarantineInventory") {
        return Err(format!("inventory request kind tag: {json}").into());
    }
    let request = SearchPlaneControlIpcRequestEnvelope {
        request_id: 92,
        payload: SearchPlaneControlIpcRequest::QuarantineDiscard(QuarantineDiscardRequest {
            target: QuarantineTargetV1::Generation(semantic_entry()),
        }),
    };
    let decoded: SearchPlaneControlIpcRequestEnvelope = decode(&encode(&request)?)?;
    if decoded != request {
        return Err(format!("discard request envelope drifted: {decoded:?}").into());
    }
    let json = serde_json::to_value(&request.payload)?;
    if json.get("kind").and_then(serde_json::Value::as_str) != Some("QuarantineDiscard") {
        return Err(format!("discard request kind tag: {json}").into());
    }
    let response = SearchPlaneControlIpcResponseEnvelope {
        request_id: 91,
        payload: SearchPlaneControlIpcResponse::QuarantineInventory(inventory()),
    };
    let decoded: SearchPlaneControlIpcResponseEnvelope = decode(&encode(&response)?)?;
    if decoded != response {
        return Err(format!("inventory response envelope drifted: {decoded:?}").into());
    }
    let json = serde_json::to_value(&response.payload)?;
    if json.get("kind").and_then(serde_json::Value::as_str) != Some("QuarantineInventory") {
        return Err(format!("inventory response kind tag: {json}").into());
    }
    let response = SearchPlaneControlIpcResponseEnvelope {
        request_id: 92,
        payload: SearchPlaneControlIpcResponse::QuarantineDiscardAck(QuarantineDiscardAck {
            target: QuarantineTargetV1::Generation(semantic_entry()),
            outcome: QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 0 },
        }),
    };
    let decoded: SearchPlaneControlIpcResponseEnvelope = decode(&encode(&response)?)?;
    if decoded != response {
        return Err(format!("discard ack envelope drifted: {decoded:?}").into());
    }
    let json = serde_json::to_value(&response.payload)?;
    if json.get("kind").and_then(serde_json::Value::as_str) != Some("QuarantineDiscardAck") {
        return Err(format!("discard ack kind tag: {json}").into());
    }
    Ok(())
}

#[test]
fn the_inventory_request_is_empty_and_refuses_any_field() -> TestRes {
    let decoded: QuarantineInventoryRequest = decode(&encode(&QuarantineInventoryRequest)?)?;
    if decoded != QuarantineInventoryRequest {
        return Err("the empty request round-trips".into());
    }
    let mut forged: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(&serde_json::json!({ "track": "lexical" }), &mut forged)?;
    expect_refused::<QuarantineInventoryRequest>("a request carrying a field", &forged)
}

#[test]
fn every_shape_that_could_name_the_unlisted_is_refused_at_decode() -> TestRes {
    // An entry with an empty path or reason names nothing the inventory
    // could have reported.
    let entry = lexical_entry();
    expect_refused::<QuarantinedGenerationEntryV1>(
        "an empty generation path",
        &forged(&entry, |value| set_text(field(value, "path")?, ""))?,
    )?;
    expect_refused::<QuarantinedGenerationEntryV1>(
        "an empty generation reason",
        &forged(&entry, |value| set_text(field(value, "reason")?, ""))?,
    )?;
    // The detail is informational and may be empty; the adapters match on
    // path and reason.
    let decoded: QuarantinedGenerationEntryV1 = decode(&forged(&entry, |value| {
        set_text(field(value, "detail")?, "")
    })?)?;
    if !decoded.detail.is_empty() {
        return Err("an empty detail decodes as empty".into());
    }
    expect_refused::<QuarantinedGenerationEntryV1>(
        "a generation entry with a field the wire does not carry",
        &forged(&entry, |value| {
            value
                .as_map_mut()?
                .push((Value::Text("bytes".to_string()), Value::Integer(7.into())));
            Some(())
        })?,
    )?;
    expect_refused::<QuarantinedGenerationEntryV1>(
        "a generation entry missing its reason",
        &forged(&entry, |value| {
            value
                .as_map_mut()?
                .retain(|(key, _)| key.as_text() != Some("reason"));
            Some(())
        })?,
    )?;
    expect_refused::<QuarantinedGenerationEntryV1>(
        "a generation entry with a duplicate path",
        &forged(&entry, |value| {
            value.as_map_mut()?.push((
                Value::Text("path".to_string()),
                Value::Text("/elsewhere".to_string()),
            ));
            Some(())
        })?,
    )?;

    // A repo-map file name is one path segment; anything that could walk
    // out of the quarantine directory is refused before it reaches a port.
    let file = repo_map_entry();
    for file_name in [
        "",
        ".",
        "..",
        "../snapshots/live.json",
        "nested/x.json",
        "/abs.json",
    ] {
        expect_refused::<QuarantinedRepoMapFileEntryV1>(
            &format!("a repo-map file name {file_name:?}"),
            &forged(&file, |value| {
                set_text(field(value, "file_name")?, file_name)
            })?,
        )?;
    }
    let decoded: QuarantinedRepoMapFileEntryV1 = decode(&forged(&file, |value| {
        set_text(field(value, "reason")?, "")
    })?)?;
    if !decoded.reason.is_empty() {
        return Err("an empty repo-map reason decodes as empty".into());
    }

    // The per-track lists carry only their own track; the target inside
    // a discard is validated the same way through the entry it wraps.
    let value = inventory();
    expect_refused::<QuarantineInventoryV1>(
        "a semantic entry filed under lexical",
        &forged(&value, |value| {
            set_text(
                field(item(field(value, "lexical")?, 0)?, "track")?,
                "semantic",
            )
        })?,
    )?;
    expect_refused::<QuarantineInventoryV1>(
        "a lexical entry filed under semantic",
        &forged(&value, |value| {
            set_text(
                field(item(field(value, "semantic")?, 0)?, "track")?,
                "lexical",
            )
        })?,
    )?;
    expect_refused::<QuarantineInventoryV1>(
        "an inventory missing its repo_map list",
        &forged(&value, |value| {
            value
                .as_map_mut()?
                .retain(|(key, _)| key.as_text() != Some("repo_map"));
            Some(())
        })?,
    )?;
    let request = QuarantineDiscardRequest {
        target: QuarantineTargetV1::RepoMapFile(repo_map_entry()),
    };
    expect_refused::<QuarantineDiscardRequest>(
        "a discard of a nested repo-map file name",
        &forged(&request, |value| {
            set_text(
                field(field(field(value, "target")?, "payload")?, "file_name")?,
                "../x",
            )
        })?,
    )?;
    expect_refused::<QuarantineDiscardRequest>(
        "a discard with an empty generation path",
        &forged(
            &QuarantineDiscardRequest {
                target: QuarantineTargetV1::Generation(lexical_entry()),
            },
            |value| {
                set_text(
                    field(field(field(value, "target")?, "payload")?, "path")?,
                    "",
                )
            },
        )?,
    )?;

    // The adjacent tag is closed: an unknown kind, a missing payload, a
    // payload ahead of its kind and a stray field are all refused.
    let target = QuarantineTargetV1::Generation(lexical_entry());
    expect_refused::<QuarantineTargetV1>(
        "an unknown target kind",
        &forged(&target, |value| set_text(field(value, "kind")?, "Snapshot"))?,
    )?;
    expect_refused::<QuarantineTargetV1>(
        "a target kind with the other variant's payload",
        &forged(&target, |value| {
            set_text(field(value, "kind")?, "RepoMapFile")
        })?,
    )?;
    expect_refused::<QuarantineTargetV1>(
        "a target without its payload",
        &forged(&target, |value| {
            value
                .as_map_mut()?
                .retain(|(key, _)| key.as_text() != Some("payload"));
            Some(())
        })?,
    )?;
    expect_refused::<QuarantineTargetV1>(
        "a target whose payload precedes its kind",
        &forged(&target, |value| {
            value.as_map_mut()?.reverse();
            Some(())
        })?,
    )?;
    expect_refused::<QuarantineTargetV1>(
        "a target with a stray field",
        &forged(&target, |value| {
            value
                .as_map_mut()?
                .push((Value::Text("force".to_string()), Value::Bool(true)));
            Some(())
        })?,
    )?;

    let discarded = QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 3 };
    expect_refused::<QuarantineDiscardOutcomeDtoV1>(
        "an unknown outcome kind",
        &forged(&discarded, |value| {
            set_text(field(value, "kind")?, "Skipped")
        })?,
    )?;
    expect_refused::<QuarantineDiscardOutcomeDtoV1>(
        "a Discarded outcome without its bytes",
        &forged(&discarded, |value| {
            value
                .as_map_mut()?
                .retain(|(key, _)| key.as_text() != Some("bytes"));
            Some(())
        })?,
    )?;
    expect_refused::<QuarantineDiscardOutcomeDtoV1>(
        "an Absent outcome carrying bytes",
        &forged(&discarded, |value| {
            set_text(field(value, "kind")?, "Absent")
        })?,
    )?;
    expect_refused::<QuarantineDiscardOutcomeDtoV1>(
        "negative bytes",
        &forged(&discarded, |value| {
            *field(value, "bytes")? = Value::Integer((-1).into());
            Some(())
        })?,
    )?;
    Ok(())
}
