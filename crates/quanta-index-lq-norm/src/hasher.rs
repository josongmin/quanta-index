//! Canonical hash for [`crate::ast::LqNormalizedQuery`].
//!
//! Pipeline: CBOR-encode the normalized query → prepend a fixed domain
//! tag → SHA-256. The domain tag binds this hash to the PRE-NORM canonical
//! shape so collisions across other CBOR consumers are infeasible.
//!
//! The CBOR encoder is the off-the-shelf `ciborium` writer; canonical key
//! ordering is the responsibility of the serde impls in
//! [`crate::ast`] (which use deterministic insertion order).
//!
//! D18 — every wire shape is hand-rolled serde; the hasher imports the
//! serializer but never sees the leaf representations directly.

use sha2::Digest as _;

use crate::ast::LqNormalizedQuery;
use crate::errors::{LqParseError, LqParseErrorCode, LqSpan};

/// Domain-separator tag baked into every PRE-NORM canonical hash.
///
/// Bumping this constant invalidates downstream cache keys; reserved for
/// shape-changing migrations only.
pub const DOMAIN_TAG: &[u8] = b"LqCanonicalHashV1\0";

/// 32-byte SHA-256 digest of the canonical CBOR encoding of `q`,
/// prefixed by [`DOMAIN_TAG`].
///
/// Errors only on CBOR encoder failure — which itself indicates a
/// programming error in the serde impls. Returns a typed
/// [`LqParseError`] tagged [`LqParseErrorCode::SyntaxError`] so callers
/// see a uniform failure surface.
pub fn canonical_hash(q: &LqNormalizedQuery) -> Result<[u8; 32], LqParseError> {
    let mut cbor: Vec<u8> = Vec::new();
    if let Err(e) = ciborium::ser::into_writer(q, &mut cbor) {
        return Err(LqParseError::new(
            LqParseErrorCode::SyntaxError,
            LqSpan::synthetic(0),
            format!("canonical CBOR encode failed: {e}"),
        ));
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(DOMAIN_TAG);
    hasher.update(&cbor);
    let out = hasher.finalize();
    let mut digest = [0u8; 32];
    digest.copy_from_slice(out.as_slice());
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::canonical_hash;
    use crate::normalizer::normalize;
    use crate::parser::parse;
    use crate::tokenizer::tokenize;

    fn hash_of(input: &str) -> [u8; 32] {
        let toks = match tokenize(input) {
            Ok(t) => t,
            Err(e) => {
                assert!(false, "tokenize failed: {e}");
                return [0u8; 32];
            }
        };
        let q = match parse(&toks, input) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse failed: {e}");
                return [0u8; 32];
            }
        };
        let q = match normalize(q) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "normalize failed: {e}");
                return [0u8; 32];
            }
        };
        match canonical_hash(&q) {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "hash failed: {e}");
                [0u8; 32]
            }
        }
    }

    #[test]
    fn hash_is_deterministic_across_runs() {
        let h1 = hash_of("foo bar");
        let h2 = hash_of("foo bar");
        assert_eq!(h1, h2);
    }

    #[test]
    fn equivalent_normalize_yields_same_hash() {
        // `foo bar` and `bar foo` differ only in source order; the
        // normalizer sorts AND children so both should hash identically.
        let h1 = hash_of("foo bar");
        let h2 = hash_of("bar foo");
        assert_eq!(h1, h2);
    }

    #[test]
    fn distinct_queries_yield_distinct_hashes() {
        let h1 = hash_of("foo");
        let h2 = hash_of("bar");
        assert_ne!(h1, h2);
    }

    #[test]
    fn domain_tag_is_v1() {
        assert_eq!(super::DOMAIN_TAG, b"LqCanonicalHashV1\0");
    }
}
