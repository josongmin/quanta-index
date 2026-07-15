#![forbid(unsafe_code)]

use std::io::Cursor;

use quanta_index_contract::SearchPlaneIpcError;
use quanta_index_ipc::{IpcError, MAX_FRAME_BODY_BYTES, decode_response, encode_response};

type TestRes = Result<(), Box<dyn std::error::Error>>;

const HISTORICAL_SEARCH_PLANE_IPC_ERROR_V1: &str =
    include_str!("fixtures/search_plane_ipc_error_v1.cbor.hex");

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn historical_frame_v1() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut frame = Vec::new();
    let mut high_nibble = None;
    let mut in_comment = false;

    for byte in HISTORICAL_SEARCH_PLANE_IPC_ERROR_V1.bytes() {
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

fn expected_error_v1() -> SearchPlaneIpcError {
    SearchPlaneIpcError {
        code: "BAD_REQUEST".to_owned(),
        message: "bad request".to_owned(),
        repair: None,
    }
}

#[test]
fn historical_v1_cbor_frame_decodes_and_reencodes_byte_identically() -> TestRes {
    let historical = historical_frame_v1()?;
    let decoded: SearchPlaneIpcError = decode_response(&mut Cursor::new(&historical))?;
    if decoded != expected_error_v1() {
        return Err(format!("historical V1 decode drifted: {decoded:?}").into());
    }

    let reencoded = encode_response(&decoded)?;
    if reencoded != historical {
        return Err(format!(
            "V1 canonical re-encode drifted from checked-in historical frame: \
             expected={historical:02x?}, actual={reencoded:02x?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn historical_v1_cbor_frame_rejects_truncated_header_and_body() -> TestRes {
    let historical = historical_frame_v1()?;
    let header_truncated: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(&historical[..3]));
    if !matches!(header_truncated, Err(IpcError::Truncated)) {
        return Err(
            format!("expected truncated header rejection, got {header_truncated:?}").into(),
        );
    }

    let body_truncated: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(&historical[..historical.len() - 1]));
    if !matches!(body_truncated, Err(IpcError::Truncated)) {
        return Err(format!("expected truncated body rejection, got {body_truncated:?}").into());
    }
    Ok(())
}

#[test]
fn historical_v1_cbor_frame_rejects_malformed_declared_lengths() -> TestRes {
    let historical = historical_frame_v1()?;
    let mut too_short = historical.clone();
    let declared = u32::from_le_bytes([too_short[0], too_short[1], too_short[2], too_short[3]]);
    let malformed = declared
        .checked_sub(1)
        .ok_or("historical fixture must have a nonzero body length")?;
    too_short[..4].copy_from_slice(&malformed.to_le_bytes());
    let short_result: Result<SearchPlaneIpcError, IpcError> =
        decode_response(&mut Cursor::new(&too_short));
    if !matches!(short_result, Err(IpcError::Decode(_))) {
        return Err(
            format!("expected short declared length rejection, got {short_result:?}").into(),
        );
    }

    let oversized = u32::try_from(MAX_FRAME_BODY_BYTES)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or("MAX_FRAME_BODY_BYTES must fit in u32 for this wire test")?;
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
