use crate::errors::{LexNormError, LexNormErrorCode};
use crate::lang::LangId;
use crate::nfc::normalize_nfc;
use crate::patterntype::PatternType;

use super::{Token, TokenKind};

/// Maximum supported chunk size, in bytes. Inputs above this fail with
/// [`LexNormErrorCode::OversizedChunk`].
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;

/// Tokenize `input` per `lang` and `pt`. See module-level docs for the
/// per-`PatternType` branching contract.
pub fn tokenize_text(
    input: &str,
    lang: LangId,
    pt: PatternType,
) -> Result<Vec<Token>, LexNormError> {
    if input.len() > MAX_CHUNK_BYTES {
        return Err(LexNormError::new(
            LexNormErrorCode::OversizedChunk,
            u32_from_usize_clamped(MAX_CHUNK_BYTES),
            "input exceeds 64 KiB cap",
        ));
    }
    if !lang.is_supported() {
        return Err(LexNormError::new(
            LexNormErrorCode::NormalizerUnknownLang,
            0,
            "lang not in v1 ship set",
        ));
    }
    // NFC pass at input boundary. ASCII-only input is byte-identical to
    // its NFC form by definition, so we skip the allocation in that case.
    // Non-ASCII inputs are normalized before tokenization; emitted token
    // byte offsets index into this normalized form (see module header).
    let normalized: String;
    let work: &str = if input.is_ascii() {
        input
    } else {
        normalized = normalize_nfc(input);
        &normalized
    };
    // NFC composition can shrink the input (e.g. decomposed `e + ́`
    // becomes single `é` codepoint), never enlarge it past the original
    // byte length cap in practical terms, but a future spec change might
    // expand combining-mark sequences. Recheck the cap after NFC to keep
    // the contract honest.
    if work.len() > MAX_CHUNK_BYTES {
        return Err(LexNormError::new(
            LexNormErrorCode::OversizedChunk,
            u32_from_usize_clamped(MAX_CHUNK_BYTES),
            "NFC-normalized input exceeds 64 KiB cap",
        ));
    }
    match pt {
        PatternType::Literal => tokenize_literal(work),
        PatternType::Keyword | PatternType::Standard => tokenize_identifier_aware(work),
        PatternType::Regexp => Err(LexNormError::new(
            LexNormErrorCode::UnknownPatternType,
            0,
            "regexp tokenization deferred to LEX-04",
        )),
        PatternType::Structural => Err(LexNormError::new(
            LexNormErrorCode::UnknownPatternType,
            0,
            "structural tokenization deferred to STR-01",
        )),
    }
}

pub(super) fn tokenize_literal(input: &str) -> Result<Vec<Token>, LexNormError> {
    let mut out: Vec<Token> = Vec::new();
    let bytes = input.as_bytes();
    let mut i: usize = 0;
    while i < bytes.len() {
        let Some(b) = bytes.get(i) else {
            break;
        };
        if is_ascii_ws(*b) {
            i = i.saturating_add(1);
            continue;
        }
        let start = i;
        while let Some(b2) = bytes.get(i) {
            if is_ascii_ws(*b2) {
                break;
            }
            i = i.saturating_add(1);
        }
        let end = i;
        let slice = input
            .get(start..end)
            .ok_or_else(|| invalid_slice_err(start))?;
        out.push(Token {
            surface: slice.to_owned().into_boxed_str(),
            lowered: slice.to_ascii_lowercase().into_boxed_str(),
            kind: TokenKind::Word,
            byte_start: u32_from_usize_clamped(start),
            byte_end: u32_from_usize_clamped(end),
        });
    }
    Ok(out)
}

fn tokenize_identifier_aware(input: &str) -> Result<Vec<Token>, LexNormError> {
    let mut out: Vec<Token> = Vec::new();
    let bytes = input.as_bytes();
    let mut i: usize = 0;
    while i < bytes.len() {
        let Some(b) = bytes.get(i) else {
            break;
        };
        if is_ascii_ws(*b) {
            i = i.saturating_add(1);
            continue;
        }
        let start = i;
        let is_word = is_word_byte(*b);
        if is_word {
            while let Some(b2) = bytes.get(i) {
                if !is_word_byte(*b2) {
                    break;
                }
                i = i.saturating_add(1);
            }
        } else {
            i = i.saturating_add(1);
        }
        let end = i;
        let slice = input
            .get(start..end)
            .ok_or_else(|| invalid_slice_err(start))?;
        if is_word {
            push_word_and_parts(&mut out, slice, start, end);
        } else {
            out.push(Token {
                surface: slice.to_owned().into_boxed_str(),
                lowered: slice.to_ascii_lowercase().into_boxed_str(),
                kind: TokenKind::Punct,
                byte_start: u32_from_usize_clamped(start),
                byte_end: u32_from_usize_clamped(end),
            });
        }
    }
    Ok(out)
}

fn push_word_and_parts(out: &mut Vec<Token>, slice: &str, start: usize, end: usize) {
    let kind = if slice.bytes().all(|b| b.is_ascii_digit()) {
        TokenKind::Number
    } else {
        TokenKind::Word
    };
    out.push(Token {
        surface: slice.to_owned().into_boxed_str(),
        lowered: slice.to_ascii_lowercase().into_boxed_str(),
        kind,
        byte_start: u32_from_usize_clamped(start),
        byte_end: u32_from_usize_clamped(end),
    });
    let parts = split_identifier(slice);
    if parts.len() <= 1 {
        return;
    }
    let mut local_off: usize = 0;
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let plen = part.len();
        let pstart = start.saturating_add(local_off);
        let pend = pstart.saturating_add(plen);
        local_off = local_off.saturating_add(plen);
        if pstart == start && pend == end {
            continue;
        }
        let pkind = if part.bytes().all(|b| b.is_ascii_digit()) {
            TokenKind::Number
        } else {
            TokenKind::IdentifierPart
        };
        out.push(Token {
            surface: part.to_owned().into_boxed_str(),
            lowered: part.to_ascii_lowercase().into_boxed_str(),
            kind: pkind,
            byte_start: u32_from_usize_clamped(pstart),
            byte_end: u32_from_usize_clamped(pend),
        });
    }
}

/// Split an identifier into camelCase / `snake_case` / kebab-case / digit
/// boundary parts.
///
/// Returns the slice as-is when no boundary fires (so the caller can detect
/// "single-part" and skip emitting duplicates). Boundary rules:
///
/// 1. Separator bytes (`_`, `-`) terminate a part and are dropped.
/// 2. ASCII digit run is its own part.
/// 3. lower→Upper boundary (`getName` → `get` | `Name`).
/// 4. Upper-run + lower boundary (`HTTPServer` → `HTTP` | `Server`): when an
///    uppercase run is followed by a lowercase letter, the last uppercase
///    starts the new part.
pub(crate) fn split_identifier(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = Vec::new();
    let mut start: usize = 0;
    let mut i: usize = 0;
    while i < bytes.len() {
        let Some(b) = bytes.get(i) else {
            break;
        };
        if *b == b'_' || *b == b'-' {
            if start < i
                && let Some(p) = s.get(start..i)
            {
                parts.push(p);
            }
            i = i.saturating_add(1);
            start = i;
            continue;
        }
        let is_upper = b.is_ascii_uppercase();
        let is_digit = b.is_ascii_digit();
        let prev = if i == 0 {
            None
        } else {
            bytes.get(i.saturating_sub(1))
        };
        let next = bytes.get(i.saturating_add(1));
        let cut_here = prev.is_some_and(|pb| {
            let prev_lower = pb.is_ascii_lowercase();
            let prev_upper = pb.is_ascii_uppercase();
            let prev_digit = pb.is_ascii_digit();
            let lower_to_upper = prev_lower && is_upper;
            let acronym_break =
                prev_upper && is_upper && matches!(next, Some(nb) if nb.is_ascii_lowercase());
            let digit_boundary = (prev_digit && !is_digit) || (!prev_digit && is_digit);
            lower_to_upper || acronym_break || digit_boundary
        });
        if cut_here
            && start < i
            && let Some(p) = s.get(start..i)
        {
            parts.push(p);
            start = i;
        }
        i = i.saturating_add(1);
    }
    if start < bytes.len()
        && let Some(p) = s.get(start..bytes.len())
    {
        parts.push(p);
    }
    parts
}

const fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

const fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (b >= 0x80)
}

fn invalid_slice_err(at: usize) -> LexNormError {
    LexNormError::new(
        LexNormErrorCode::InvalidUtf8,
        u32_from_usize_clamped(at),
        "slice does not align to a UTF-8 boundary",
    )
}

fn u32_from_usize_clamped(v: usize) -> u32 {
    u32::try_from(v).map_or(u32::MAX, |n| n)
}
