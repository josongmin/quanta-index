//! Structural matcher trait surface and the [`MockStructuralMatcher`]
//! test-only implementation.
//!
//! ## Tree-sitter deferral
//!
//! Per LEX-05 precedent and STR-01 §4.8, the real per-language matchers
//! are tree-sitter-backed. The C transitive build dragged cold-build time
//! past the spec's 60 s budget, so this landing ships the trait surface
//! plus a deterministic mock. Real per-language matchers wire in at the
//! lexical / structural integration ticket. Until then,
//! [`MockStructuralMatcher`] is the only implementation — kept `pub` so
//! caller integration tests can exercise the registry.
//!
//! D18 — no proc-macro derives.

use crate::binding::StructuralCandidate;
use crate::errors::StructuralError;
use crate::pattern::StructuralPattern;

/// Per-language matcher port.
///
/// Implementations walk `source` against `pattern` and emit one
/// [`StructuralCandidate`] per match. Failures must surface as typed
/// [`StructuralError`] — no `panic!`/`unwrap` on the production path.
pub trait StructuralMatcher: Send + Sync {
    /// Match `pattern` against `source`. Returns every successful match;
    /// the empty vector is a valid "no match" success (but registries
    /// promote certain empty cases to `STR_LANG_RESOLUTION_EMPTY` —
    /// see [`crate::registry::MatcherRegistry::match_pattern`]).
    fn match_pattern(
        &self,
        pattern: &StructuralPattern,
        source: &[u8],
    ) -> Result<Vec<StructuralCandidate>, StructuralError>;
}

/// Deterministic test-only matcher. Returns a fixed list of candidates
/// regardless of `pattern` / `source`.
///
/// Public so caller integration tests (e.g. registry round-trips, fixture
/// goldens) can use the same shape without re-implementing the trait.
#[derive(Clone, Debug)]
pub struct MockStructuralMatcher {
    fixed: Vec<StructuralCandidate>,
}

impl MockStructuralMatcher {
    /// Construct a mock that always returns `fixed` on match.
    #[must_use]
    pub const fn new(fixed: Vec<StructuralCandidate>) -> Self {
        Self { fixed }
    }

    /// Borrow the fixed candidate list.
    #[must_use]
    pub fn fixed(&self) -> &[StructuralCandidate] {
        &self.fixed
    }
}

impl StructuralMatcher for MockStructuralMatcher {
    fn match_pattern(
        &self,
        _pattern: &StructuralPattern,
        _source: &[u8],
    ) -> Result<Vec<StructuralCandidate>, StructuralError> {
        Ok(self.fixed.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{MockStructuralMatcher, StructuralMatcher};
    use crate::binding::{StructuralBinding, StructuralCandidate};
    use crate::pattern::parse_pattern;
    use crate::types::{ByteSpan, DocId, LangId};

    fn span(a: u32, b: u32) -> ByteSpan {
        match ByteSpan::new(a, b) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                std::process::abort();
            }
        }
    }

    #[test]
    fn mock_returns_fixed_candidates() {
        let cand = StructuralCandidate::new(DocId(1), span(0, 3), StructuralBinding::empty());
        let m = MockStructuralMatcher::new(vec![cand.clone()]);
        let Ok(pat) = parse_pattern("hi", LangId::Rust) else {
            assert!(false, "parse");
            return;
        };
        let got = match m.match_pattern(&pat, b"hi") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got, vec![cand]);
    }

    #[test]
    fn mock_is_input_independent() {
        let m = MockStructuralMatcher::new(Vec::new());
        let Ok(pat) = parse_pattern("hi", LangId::Rust) else {
            assert!(false, "parse");
            return;
        };
        let a = match m.match_pattern(&pat, b"abc") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let b = match m.match_pattern(&pat, b"") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(a, b);
    }

    #[test]
    fn mock_fixed_accessor() {
        let cand = StructuralCandidate::new(DocId(2), span(0, 1), StructuralBinding::empty());
        let m = MockStructuralMatcher::new(vec![cand.clone()]);
        assert_eq!(m.fixed(), &[cand]);
    }
}
