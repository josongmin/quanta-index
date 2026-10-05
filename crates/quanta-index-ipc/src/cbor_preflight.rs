//! Allocation-free CBOR walk before server request materialization.

use ciborium_ll::{Decoder, Header};

use crate::error::IpcError;

// A retained String occupies its inline header. During Vec growth, the old
// allocation and a double-capacity replacement can briefly coexist. CBOR
// text bytes are already charged as request buffers by ingress admission.
const TEXT_VALUE_STORAGE_BYTES: usize = 3 * std::mem::size_of::<String>();

pub(crate) fn retained_text_budget(bytes: &[u8]) -> Result<usize, IpcError> {
    retained_text_budget_reader(bytes, bytes.len())
}

pub(crate) fn retained_text_budget_reader<R: std::io::Read>(reader: R, input_len: usize) -> Result<usize, IpcError> {
    let mut decoder = Decoder::from(reader);
    let mut storage_bytes = 0_usize;
    let header = decoder.pull().map_err(|error| decode_error(&error))?;
    scan(
        header,
        &mut decoder,
        input_len,
        0,
        false,
        &mut storage_bytes,
    )?;
    if decoder.offset() != input_len {
        return Err(IpcError::Decode(
            "CBOR request contains trailing bytes".to_string(),
        ));
    }
    Ok(storage_bytes)
}

fn decode_error(error: &ciborium_ll::Error<std::io::Error>) -> IpcError {
    IpcError::Decode(format!("CBOR preflight failed: {error:?}"))
}

fn next<R: std::io::Read>(decoder: &mut Decoder<R>) -> Result<Header, IpcError> {
    decoder.pull().map_err(|error| decode_error(&error))
}

fn scan<R: std::io::Read>(
    header: Header,
    decoder: &mut Decoder<R>,
    input_len: usize,
    depth: usize,
    map_key: bool,
    storage_bytes: &mut usize,
) -> Result<(), IpcError> {
    if depth >= 256 {
        return Err(IpcError::Decode(
            "CBOR request exceeds nesting limit".into(),
        ));
    }
    let child_depth = depth.saturating_add(1);
    match header {
        Header::Positive(_) | Header::Negative(_) | Header::Float(_) | Header::Simple(_) => {}
        Header::Break => return Err(IpcError::Decode("CBOR break outside container".into())),
        Header::Tag(_) => scan(
            next(decoder)?,
            decoder,
            input_len,
            child_depth,
            map_key,
            storage_bytes,
        )?,
        Header::Bytes(len) => {
            let mut segments = decoder.bytes(len);
            let mut scratch = [0_u8; 4096];
            while let Some(mut segment) = segments.pull().map_err(|error| decode_error(&error))? {
                while segment
                    .pull(&mut scratch)
                    .map_err(|error| decode_error(&error))?
                    .is_some()
                {}
            }
        }
        Header::Text(len) => {
            if !map_key {
                *storage_bytes = storage_bytes
                    .checked_add(TEXT_VALUE_STORAGE_BYTES)
                    .ok_or_else(|| IpcError::Decode("CBOR text budget overflow".into()))?;
            }
            let mut segments = decoder.text(len);
            let mut scratch = [0_u8; 4096];
            while let Some(mut segment) = segments.pull().map_err(|error| decode_error(&error))? {
                while segment
                    .pull(&mut scratch)
                    .map_err(|error| decode_error(&error))?
                    .is_some()
                {}
            }
        }
        Header::Array(Some(len)) => {
            if len > input_len {
                return Err(IpcError::Decode(
                    "CBOR array length exceeds request bytes".into(),
                ));
            }
            for _ in 0..len {
                scan(
                    next(decoder)?,
                    decoder,
                    input_len,
                    child_depth,
                    false,
                    storage_bytes,
                )?;
            }
        }
        Header::Array(None) => loop {
            let child = next(decoder)?;
            if child == Header::Break {
                break;
            }
            scan(child, decoder, input_len, child_depth, false, storage_bytes)?;
        },
        Header::Map(Some(len)) => {
            if len > input_len >> 1 {
                return Err(IpcError::Decode(
                    "CBOR map length exceeds request bytes".into(),
                ));
            }
            for _ in 0..len {
                scan(
                    next(decoder)?,
                    decoder,
                    input_len,
                    child_depth,
                    true,
                    storage_bytes,
                )?;
                scan(
                    next(decoder)?,
                    decoder,
                    input_len,
                    child_depth,
                    false,
                    storage_bytes,
                )?;
            }
        }
        Header::Map(None) => loop {
            let key = next(decoder)?;
            if key == Header::Break {
                break;
            }
            scan(key, decoder, input_len, child_depth, true, storage_bytes)?;
            scan(
                next(decoder)?,
                decoder,
                input_len,
                child_depth,
                false,
                storage_bytes,
            )?;
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TEXT_VALUE_STORAGE_BYTES, retained_text_budget};

    #[test]
    fn counts_retained_text_values_without_charging_field_names_or_byte_arrays() {
        // {"owners": ["", "x"], "bytes": [0, 1]}
        let body = b"\xa2\x66owners\x82\x60\x61x\x65bytes\x82\x00\x01";
        assert_eq!(
            retained_text_budget(body).expect("valid CBOR"),
            2 * TEXT_VALUE_STORAGE_BYTES
        );
    }

    #[test]
    fn rejects_truncated_arrays_and_trailing_values() {
        assert!(retained_text_budget(b"\x82\x60").is_err());
        assert!(retained_text_budget(b"\x01\x02").is_err());
    }
}
