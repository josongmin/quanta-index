#![forbid(unsafe_code)]

use std::io::Cursor;

use quanta_index_contract::SearchPlaneIpcError;
use quanta_index_ipc::{IpcError, MAX_FRAME_BODY_BYTES, decode_response};

type TestRes = Result<(), Box<dyn std::error::Error>>;

const HISTORICAL_SEARCH_PLANE_IPC_ERROR_V1: &str =
    include_str!("fixtures/search_plane_ipc_error_v1.cbor.hex");
const RETIRED_CODE_IN_COMPLETE_V2_SHAPE: &str =
    include_str!("fixtures/search_plane_ipc_error_retired_code_v2_shape.cbor.hex");

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte
            .checked_sub(b'a')
            .and_then(|offset| offset.checked_add(10)),
        b'A'..=b'F' => byte
            .checked_sub(b'A')
            .and_then(|offset| offset.checked_add(10)),
        _ => None,
    }
}

fn frame_from_hex_fixture(fixture: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut frame = Vec::new();
    let mut high_nibble = None;
    let mut in_comment = false;

    for byte in fixture.bytes() {
        if byte == b'#' {
            in_comment = true;
            continue;
        }
        if byte == b'\n' {
            in_comment = false;
            continue;
        }
        if in_comment || byte.is_ascii_whitespace() {
            continue;
        }

        let nibble =
            hex_nibble(byte).ok_or_else(|| format!("fixture contains non-hex byte `{byte}`"))?;
        if let Some(high) = high_nibble.take() {
            frame.push((high << 4) | nibble);
        } else {
            high_nibble = Some(nibble);
        }
    }

    if high_nibble.is_some() {
        return Err("fixture has an odd number of hex digits".into());
    }
    Ok(frame)
}

fn historical_frame_v1() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    frame_from_hex_fixture(HISTORICAL_SEARCH_PLANE_IPC_ERROR_V1)
}

#[test]
fn historical_v1_bad_request_code_is_rejected_by_the_closed_v2_decoder() -> TestRes {
    // The legacy fixture also omits the V2-required repair field. Use a
    // complete V2-shaped frame so only the retired code can cause refusal.
    let historical = frame_from_hex_fixture(RETIRED_CODE_IN_COMPLETE_V2_SHAPE)?;
    let decoded: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(&historical));
    if !matches!(&decoded, Err(IpcError::Decode(message)) if message.contains("BAD_REQUEST")) {
        return Err(
            format!("retired BAD_REQUEST code was not rejected for its code: {decoded:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn historical_v1_cbor_frame_rejects_truncated_header_and_body() -> TestRes {
    let historical = historical_frame_v1()?;
    let header_prefix = historical
        .get(..3)
        .ok_or("historical fixture must be at least 3 bytes")?;
    let header_truncated: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(header_prefix));
    if !matches!(header_truncated, Err(IpcError::Truncated)) {
        return Err(
            format!("expected truncated header rejection, got {header_truncated:?}").into(),
        );
    }

    let body_prefix_len = historical
        .len()
        .checked_sub(1)
        .ok_or("historical fixture must be nonempty")?;
    let body_prefix = historical
        .get(..body_prefix_len)
        .ok_or("historical fixture prefix must be in range")?;
    let body_truncated: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(body_prefix));
    if !matches!(body_truncated, Err(IpcError::Truncated)) {
        return Err(format!("expected truncated body rejection, got {body_truncated:?}").into());
    }
    Ok(())
}

#[test]
fn historical_v1_cbor_frame_rejects_malformed_declared_lengths() -> TestRes {
    let mut too_short = historical_frame_v1()?;
    let declared_bytes: [u8; 4] = too_short
        .get(..4)
        .ok_or("historical fixture must carry a 4-byte length prefix")?
        .try_into()
        .map_err(|err| format!("historical length prefix is not 4 bytes: {err}"))?;
    let declared = u32::from_le_bytes(declared_bytes);
    let malformed = declared
        .checked_sub(1)
        .ok_or("historical fixture must have a nonzero body length")?;
    too_short
        .get_mut(..4)
        .ok_or("historical fixture must carry a 4-byte length prefix")?
        .copy_from_slice(&malformed.to_le_bytes());
    let short_result: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(&too_short));
    if !matches!(short_result, Err(IpcError::Decode(_))) {
        return Err(
            format!("expected short declared length rejection, got {short_result:?}").into(),
        );
    }

    let frame_body_limit = u32::try_from(MAX_FRAME_BODY_BYTES)
        .map_err(|err| format!("MAX_FRAME_BODY_BYTES must fit in u32 for this wire test: {err}"))?;
    let oversized = frame_body_limit
        .checked_add(1)
        .ok_or("MAX_FRAME_BODY_BYTES + 1 must fit in u32 for this wire test")?;
    let oversized_result: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(oversized.to_le_bytes()));
    if !matches!(oversized_result, Err(IpcError::Oversized(length)) if length == u64::from(oversized))
    {
        return Err(
            format!("expected oversized declaration rejection, got {oversized_result:?}").into(),
        );
    }
    Ok(())
}
