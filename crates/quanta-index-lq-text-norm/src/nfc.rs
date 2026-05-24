//! Unicode NFC (Normalization Form Canonical Composition) at the
//! tokenizer input boundary.
//!
//! Producer ships raw UTF-8 chunk text via `UpsertChunk.payload`; the
//! search-side tokenizer must canonicalize Unicode form before downstream
//! matching, so that the precomposed `é` (U+00E9) and the decomposed
//! `é` (U+0065 U+0301) tokenize identically.
//!
//! ASCII-only inputs are short-circuited by the caller via
//! [`str::is_ascii`] — this module is unconditionally a copy, so the
//! caller decides when to skip it.
//!
//! Invariant: NFC is idempotent — `nfc(nfc(x)) == nfc(x)`. Property-tested.
//!
//! D18 — this module exposes no wire types; the function takes/returns
//! plain `&str` / `String`.

use unicode_normalization::UnicodeNormalization as _;

/// Return the Unicode NFC (canonical composition) form of `input`.
///
/// Always allocates. Callers that need a fast path for ASCII-only input
/// should branch on [`str::is_ascii`] before calling.
#[must_use]
pub fn normalize_nfc(input: &str) -> String {
    input.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::normalize_nfc;

    #[test]
    fn ascii_is_unchanged() {
        let s = "hello world";
        assert_eq!(normalize_nfc(s), s);
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(normalize_nfc(""), "");
    }

    #[test]
    fn decomposed_e_acute_composes() {
        // U+0065 (e) + U+0301 (combining acute) -> U+00E9 (é)
        let decomposed = "e\u{0301}";
        let composed = "\u{00E9}";
        assert_eq!(normalize_nfc(decomposed), composed);
    }

    #[test]
    fn precomposed_e_acute_unchanged() {
        let composed = "\u{00E9}";
        assert_eq!(normalize_nfc(composed), composed);
    }

    #[test]
    fn precomposed_equals_decomposed_after_nfc() {
        let decomposed = "caf\u{0065}\u{0301}";
        let composed = "caf\u{00E9}";
        assert_eq!(normalize_nfc(decomposed), normalize_nfc(composed));
    }

    #[test]
    fn idempotent_on_decomposed_input() {
        let decomposed = "e\u{0301}a\u{0301}o\u{0301}";
        let once = normalize_nfc(decomposed);
        let twice = normalize_nfc(&once);
        assert_eq!(once, twice);
    }
}
