//! Canonical JSON and digest rules.
//!
//! "Canonical" here is a wire contract, not a convenience: the Python writer
//! in `tools/benchmark/evidence.py` must produce the same bytes for the same
//! in-memory document. The rules are:
//!
//! - object keys sorted by code point;
//! - no insignificant whitespace;
//! - strings escaped exactly like Python's `json.dumps(..., ensure_ascii=False)`
//!   (`"` , `\`, `\b`, `\f`, `\n`, `\r`, `\t`, and `\u00xx` for the remaining
//!   control characters; every other scalar is emitted raw);
//! - numbers emitted in shortest round-trip form; integers as integers, and
//!   floats always with an explicit fractional part and never in exponent
//!   notation (`1.0`, `0.00001`, `10000000000000000.0`), so that both writers
//!   can agree without depending on a language-specific `repr` policy;
//! - `null`/`true`/`false` lowercase.
//!
//! A document whose numbers cannot be represented by both writers is refused
//! by the Python side rather than silently hashed differently.

use serde_json::Value;
use sha2::Digest as _;

use crate::error::ProtocolError;

/// Prefix every digest in the contract carries.
pub const DIGEST_PREFIX: &str = "sha256:";

/// Lowercase hex encoding of the SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in &digest {
        push_hex_byte(&mut out, *byte);
    }
    out
}

/// `sha256:<hex>` identity of `bytes`.
#[must_use]
pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("{DIGEST_PREFIX}{}", sha256_hex(bytes))
}

/// `sha256:<hex>` identity of the canonical JSON form of `value`.
#[must_use]
pub fn digest_of(value: &Value) -> String {
    digest_bytes(canonical_json(value).as_bytes())
}

/// Canonical JSON bytes for `value`, as a `String`.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

/// Convert a byte length, refusing (never defaulting) an impossible conversion.
pub fn byte_len(len: usize) -> Result<u64, ProtocolError> {
    u64::try_from(len).map_err(|error| {
        ProtocolError::semantic(format!("byte length {len} does not fit in u64: {error}"))
    })
}

/// True when `value` is `sha256:<64 lowercase hex>`.
#[must_use]
pub fn is_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(DIGEST_PREFIX) else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Refuse a malformed digest field.
pub fn require_digest(field: &str, value: &str) -> Result<(), ProtocolError> {
    if is_digest(value) {
        Ok(())
    } else {
        Err(ProtocolError::InvalidDigest {
            field: field.to_owned(),
            value: value.to_owned(),
        })
    }
}

const HEX_DIGITS: [u8; 16] = *b"0123456789abcdef";

fn push_hex_byte(out: &mut String, byte: u8) {
    let high = HEX_DIGITS
        .get(usize::from(byte >> 4))
        .copied()
        .unwrap_or(b'0');
    let low = HEX_DIGITS
        .get(usize::from(byte & 0x0f))
        .copied()
        .unwrap_or(b'0');
    out.push(char::from(high));
    out.push(char::from(low));
}

/// Shortest round-trip plain decimal form with an explicit fractional part.
fn write_number(out: &mut String, number: &serde_json::Number) {
    if let Some(value) = number.as_i64() {
        out.push_str(&value.to_string());
        return;
    }
    if let Some(value) = number.as_u64() {
        out.push_str(&value.to_string());
        return;
    }
    if let Some(value) = number.as_f64() {
        let text = format!("{value}");
        out.push_str(&text);
        if !text.contains('.') {
            out.push_str(".0");
        }
        return;
    }
    out.push_str(&number.to_string());
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, number),
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                if let Some(item) = map.get(key.as_str()) {
                    write_value(out, item);
                } else {
                    out.push_str("null");
                }
            }
            out.push('}');
        }
    }
}

#[expect(
    clippy::expect_used,
    reason = "formatting into a String is infallible; the expect documents that"
)]
fn write_string(out: &mut String, text: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(control))
                    .expect("writing into a String cannot fail");
            }
            other => out.push(other),
        }
    }
    out.push('"');
}
