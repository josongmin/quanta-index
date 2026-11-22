//! Allocation-free CBOR walk before server request materialization.

use ciborium_ll::{Decoder, Header};

use crate::error::IpcError;

// A retained String occupies its inline header. During Vec growth, the old
// allocation and a double-capacity replacement can briefly coexist. CBOR
// text bytes are already charged as request buffers by legacy ingress.
const TEXT_VALUE_STORAGE_BYTES: usize = 3 * std::mem::size_of::<String>();

#[derive(Default)]
struct StorageBudget {
    text_headers: usize,
    scalar_backing_peak: usize,
    guarded: bool,
}

impl StorageBudget {
    fn add_scalar(&mut self, content_bytes: usize, definite: bool) -> Result<(), IpcError> {
        if !self.guarded || content_bytes == 0 {
            return Ok(());
        }
        // Guarded slice decode gives definite scalars one exact allocation.
        // Indefinite scalars may hold old and replacement buffers during growth.
        let peak = if definite {
            content_bytes
        } else {
            content_bytes
                .checked_mul(3)
                .map(|bytes| bytes.max(8))
                .ok_or_else(|| IpcError::Decode("CBOR request storage budget overflow".into()))?
        };
        self.scalar_backing_peak = self
            .scalar_backing_peak
            .checked_add(peak)
            .ok_or_else(|| IpcError::Decode("CBOR request storage budget overflow".into()))?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn retained_text_budget(bytes: &[u8]) -> Result<usize, IpcError> {
    retained_text_budget_reader(bytes, bytes.len())
}

pub(crate) fn retained_text_budget_reader<R: std::io::Read>(
    reader: R,
    input_len: usize,
) -> Result<usize, IpcError> {
    Ok(scan_budget_reader(reader, input_len, false)?.text_headers)
}

pub(crate) fn guarded_request_storage_budget(bytes: &[u8]) -> Result<usize, IpcError> {
    guarded_storage_budget(scan_budget_reader(bytes, bytes.len(), true)?)
}

fn guarded_storage_budget(budget: StorageBudget) -> Result<usize, IpcError> {
    budget
        .scalar_backing_peak
        .checked_add(budget.text_headers)
        .ok_or_else(|| IpcError::Decode("CBOR request storage budget overflow".into()))
}

fn scan_budget_reader<R: std::io::Read>(
    reader: R,
    input_len: usize,
    guarded: bool,
) -> Result<StorageBudget, IpcError> {
    let mut decoder = Decoder::from(reader);
    let mut budget = StorageBudget {
        guarded,
        ..StorageBudget::default()
    };
    // One scratch buffer for the entire bounded walk. Per-depth 4 KiB buffers
    // exhausted a normal worker/test stack before reaching the nesting guard.
    let mut scratch = [0_u8; 4096];
    let header = decoder.pull().map_err(|error| decode_error(&error))?;
    scan(
        header,
        &mut decoder,
        input_len,
        0,
        false,
        &mut budget,
        &mut scratch,
    )?;
    if decoder.offset() != input_len {
        return Err(IpcError::Decode(
            "CBOR request contains trailing bytes".to_string(),
        ));
    }
    Ok(budget)
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
    budget: &mut StorageBudget,
    scratch: &mut [u8; 4096],
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
            budget,
            scratch,
        )?,
        Header::Bytes(len) => {
            let mut segments = decoder.bytes(len);
            let mut content_bytes = 0_usize;
            while let Some(mut segment) = segments.pull().map_err(|error| decode_error(&error))? {
                while let Some(chunk) = segment
                    .pull(scratch)
                    .map_err(|error| decode_error(&error))?
                {
                    if budget.guarded {
                        content_bytes =
                            content_bytes.checked_add(chunk.len()).ok_or_else(|| {
                                IpcError::Decode("CBOR request storage budget overflow".into())
                            })?;
                    }
                }
            }
            budget.add_scalar(content_bytes, len.is_some())?;
        }
        Header::Text(len) => {
            if !map_key {
                budget.text_headers = budget
                    .text_headers
                    .checked_add(TEXT_VALUE_STORAGE_BYTES)
                    .ok_or_else(|| IpcError::Decode("CBOR text budget overflow".into()))?;
            }
            let mut segments = decoder.text(len);
            let mut content_bytes = 0_usize;
            while let Some(mut segment) = segments.pull().map_err(|error| decode_error(&error))? {
                while let Some(chunk) = segment
                    .pull(scratch)
                    .map_err(|error| decode_error(&error))?
                {
                    if budget.guarded {
                        content_bytes =
                            content_bytes.checked_add(chunk.len()).ok_or_else(|| {
                                IpcError::Decode("CBOR request storage budget overflow".into())
                            })?;
                    }
                }
            }
            budget.add_scalar(content_bytes, len.is_some())?;
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
                    budget,
                    scratch,
                )?;
            }
        }
        Header::Array(None) => loop {
            let child = next(decoder)?;
            if child == Header::Break {
                break;
            }
            scan(
                child,
                decoder,
                input_len,
                child_depth,
                false,
                budget,
                scratch,
            )?;
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
                    budget,
                    scratch,
                )?;
                scan(
                    next(decoder)?,
                    decoder,
                    input_len,
                    child_depth,
                    false,
                    budget,
                    scratch,
                )?;
            }
        }
        Header::Map(None) => loop {
            let key = next(decoder)?;
            if key == Header::Break {
                break;
            }
            scan(key, decoder, input_len, child_depth, true, budget, scratch)?;
            scan(
                next(decoder)?,
                decoder,
                input_len,
                child_depth,
                false,
                budget,
                scratch,
            )?;
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        StorageBudget, TEXT_VALUE_STORAGE_BYTES, guarded_request_storage_budget,
        guarded_storage_budget, retained_text_budget,
    };

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
    fn guarded_counts_definite_text_and_bytes_content() {
        // ["abc", h'646566']; each scalar needs one 3-byte backing.
        let body = b"\x82\x63abc\x43def";
        assert_eq!(
            guarded_request_storage_budget(body).expect("valid CBOR"),
            TEXT_VALUE_STORAGE_BYTES + 6
        );
        assert_eq!(
            retained_text_budget(body).expect("valid CBOR"),
            TEXT_VALUE_STORAGE_BYTES
        );
    }

    #[test]
    fn guarded_counts_indefinite_text_and_bytes_segments() {
        // [(_ "hi", "!"), (_ h'61', h'6263')]
        let body = b"\x82\x7f\x62hi\x61!\xff\x5f\x41a\x42bc\xff";
        assert_eq!(
            guarded_request_storage_budget(body).expect("valid CBOR"),
            TEXT_VALUE_STORAGE_BYTES + 18
        );
        assert_eq!(
            retained_text_budget(body).expect("valid CBOR"),
            TEXT_VALUE_STORAGE_BYTES
        );
    }

    #[test]
    fn guarded_counts_utf8_bytes_instead_of_characters() {
        // "€" is one character encoded as three UTF-8 bytes.
        let body = b"\x63\xe2\x82\xac";
        assert_eq!(
            guarded_request_storage_budget(body).expect("valid CBOR"),
            TEXT_VALUE_STORAGE_BYTES + 3
        );
    }

    #[test]
    fn guarded_counts_map_keys_without_legacy_text_header_charge() {
        // {"name": h'0102'}; key content is owned even though its header is
        // excluded from the legacy retained-text budget.
        let body = b"\xa1\x64name\x42\x01\x02";
        assert_eq!(guarded_request_storage_budget(body).expect("valid CBOR"), 6);
        assert_eq!(retained_text_budget(body).expect("valid CBOR"), 0);
    }

    #[test]
    fn guarded_funds_indefinite_minimum_capacity_and_skips_empty_scalars() {
        assert_eq!(
            guarded_request_storage_budget(b"\x80").expect("empty array"),
            0
        );
        assert_eq!(
            guarded_request_storage_budget(b"\x40").expect("empty bytes"),
            0
        );
        assert_eq!(
            guarded_request_storage_budget(b"\x60").expect("empty text"),
            TEXT_VALUE_STORAGE_BYTES
        );
        assert_eq!(
            guarded_request_storage_budget(b"\x5f\x41a\xff").expect("one segmented byte"),
            8
        );
        assert_eq!(
            retained_text_budget(b"\x60").expect("empty text"),
            TEXT_VALUE_STORAGE_BYTES
        );
    }

    #[test]
    fn guarded_rejects_scalar_peak_and_total_overflow() {
        let mut budget = StorageBudget {
            guarded: true,
            ..StorageBudget::default()
        };
        assert!(budget.add_scalar(usize::MAX, false).is_err());
        budget.scalar_backing_peak = usize::MAX;
        budget.text_headers = 1;
        assert!(guarded_storage_budget(budget).is_err());
    }

    #[test]
    fn rejects_truncated_arrays_and_trailing_values() {
        assert!(retained_text_budget(b"\x82\x60").is_err());
        assert!(retained_text_budget(b"\x01\x02").is_err());
    }

    #[test]
    fn nesting_guard_refuses_without_exhausting_the_worker_stack() {
        let mut bytes = vec![0x81; 257];
        bytes.push(0);
        let error = retained_text_budget(&bytes).expect_err("nested input exceeds contract");
        assert!(error.to_string().contains("nesting limit"));
    }
}
