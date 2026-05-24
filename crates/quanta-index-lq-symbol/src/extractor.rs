//! Symbol-extractor trait surface.
//!
//! [`SymbolExtractor`] is the per-language abstraction that a registry
//! looks up by [`crate::types::LangId`]. Each language implementation owns
//! the lookup from grammar capture-names (eg. `tags.scm` tags when the
//! real tree-sitter wire-up lands) to [`crate::SymbolKind`].
//!
//! ## Tree-sitter deferral (per LEX-05 §12 / spec caveat)
//!
//! The LEX-05 spec sheet calls for `tree-sitter` + `tags.scm` as the
//! authoritative extractor. We have not pulled `tree-sitter` in this
//! landing because the C-library transitive build dragged cold-build time
//! past the 60 s budget the spec sheet's caveat allows. The trait surface
//! and registry are stable so the real per-language extractors can drop
//! in without re-shaping callers. Until then, [`MockExtractor`] provides
//! a fully-deterministic test-only implementation.
//!
//! D18 — no proc-macro derives.

use crate::errors::SymbolError;
use crate::types::Symbol;

/// Per-language extractor port.
///
/// Implementations parse `source` and emit every captured definition (and
/// local reference, when the language grammar supports it) as a
/// [`Symbol`]. Failures must be typed via [`SymbolError`]; no
/// `panic!`/`unwrap` on the production path.
pub trait SymbolExtractor: Send + Sync {
    /// Run the extractor over `source` bytes.
    fn extract(&self, source: &[u8]) -> Result<Vec<Symbol>, SymbolError>;
}

/// Test-only deterministic extractor. Returns a fixed `Vec<Symbol>`
/// regardless of input. Used to exercise the trait surface and the
/// registry routing before the real per-language extractors land.
#[derive(Clone, Debug)]
pub struct MockExtractor {
    pub fixed_symbols: Vec<Symbol>,
}

impl MockExtractor {
    /// Construct a mock that always returns `fixed_symbols` on extract.
    #[must_use]
    pub const fn new(fixed_symbols: Vec<Symbol>) -> Self {
        Self { fixed_symbols }
    }
}

impl SymbolExtractor for MockExtractor {
    fn extract(&self, _source: &[u8]) -> Result<Vec<Symbol>, SymbolError> {
        Ok(self.fixed_symbols.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{MockExtractor, SymbolExtractor};
    use crate::symbol_kind::SymbolKind;
    use crate::types::{ByteSpan, DocId, LangId, Symbol};

    #[test]
    fn mock_extractor_returns_fixed_symbols() {
        let span = match ByteSpan::new(0, 3) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let sym = Symbol::new(
            "foo",
            SymbolKind::Function,
            DocId(1),
            span,
            LangId::Rust,
            None,
        );
        let m = MockExtractor::new(vec![sym.clone()]);
        let got = match m.extract(b"") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got, vec![sym]);
    }

    #[test]
    fn mock_extractor_is_input_independent() {
        let m = MockExtractor::new(Vec::new());
        let a = match m.extract(b"abc") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let b = match m.extract(b"") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(a, b);
    }
}
