//! Lang-id keyed registry of [`SymbolExtractor`] implementations.
//!
//! Missing-language lookup fails closed with the locked
//! `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` shape from
//! [`crate::errors::SymbolError::lang_unsupported`]. There is no silent
//! skip path; callers that need to ignore unsupported languages must
//! catch the typed error explicitly.
//!
//! D18 — no proc-macro derives.

use std::collections::BTreeMap;

use crate::errors::SymbolError;
use crate::extractor::SymbolExtractor;
use crate::types::{LangId, Symbol};

/// Per-language extractor table.
pub struct ExtractorRegistry {
    by_lang: BTreeMap<LangId, Box<dyn SymbolExtractor>>,
}

impl ExtractorRegistry {
    /// Construct an empty registry. Every language is `unsupported` until
    /// a call to [`Self::register`] lands its extractor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_lang: BTreeMap::new(),
        }
    }

    /// Register `extractor` against `lang`. Replaces any prior
    /// registration for the same language and returns whether a prior
    /// registration was displaced.
    pub fn register(&mut self, lang: LangId, extractor: Box<dyn SymbolExtractor>) -> bool {
        self.by_lang.insert(lang, extractor).is_some()
    }

    /// Borrow the extractor registered for `lang`, or `None` if absent.
    #[must_use]
    pub fn get(&self, lang: LangId) -> Option<&dyn SymbolExtractor> {
        self.by_lang.get(&lang).map(AsRef::as_ref)
    }

    /// `true` if `lang` has a registered extractor.
    #[must_use]
    pub fn is_supported(&self, lang: LangId) -> bool {
        self.by_lang.contains_key(&lang)
    }

    /// All languages with a registered extractor, in declaration order.
    #[must_use]
    pub fn supported_langs(&self) -> Vec<LangId> {
        self.by_lang.keys().copied().collect()
    }

    /// Run the registered extractor for `lang` over `source`.
    ///
    /// Returns `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` (typed,
    /// not panic) if `lang` is not registered. Any extractor-side failure
    /// propagates verbatim.
    pub fn extract(&self, lang: LangId, source: &[u8]) -> Result<Vec<Symbol>, SymbolError> {
        self.get(lang).map_or_else(
            || Err(SymbolError::lang_unsupported(lang.as_code_str())),
            |ex| ex.extract(source),
        )
    }
}

impl Default for ExtractorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ExtractorRegistry;
    use crate::errors::SymbolErrorCode;
    use crate::extractor::MockExtractor;
    use crate::symbol_kind::SymbolKind;
    use crate::types::{ByteSpan, DocId, LangId, Symbol};

    fn one_symbol() -> Symbol {
        let Ok(span) = ByteSpan::new(0, 3) else {
            std::process::abort();
        };
        Symbol::new(
            "foo",
            SymbolKind::Function,
            DocId(1),
            span,
            LangId::Rust,
            None,
        )
    }

    #[test]
    fn empty_registry_reports_no_support() {
        let r = ExtractorRegistry::new();
        for l in LangId::all() {
            assert!(!r.is_supported(*l));
            assert!(r.get(*l).is_none());
        }
        assert!(r.supported_langs().is_empty());
    }

    #[test]
    fn register_then_get_returns_extractor() {
        let mut r = ExtractorRegistry::new();
        let displaced = r.register(
            LangId::Rust,
            Box::new(MockExtractor::new(vec![one_symbol()])),
        );
        assert!(!displaced);
        assert!(r.is_supported(LangId::Rust));
        let Some(ex) = r.get(LangId::Rust) else {
            assert!(false, "expected extractor");
            return;
        };
        let out = match ex.extract(b"") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(out, vec![one_symbol()]);
    }

    #[test]
    fn re_register_displaces_prior() {
        let mut r = ExtractorRegistry::new();
        let first: bool = r.register(LangId::Rust, Box::new(MockExtractor::new(Vec::new())));
        assert!(!first);
        let displaced = r.register(
            LangId::Rust,
            Box::new(MockExtractor::new(vec![one_symbol()])),
        );
        assert!(displaced);
    }

    #[test]
    fn extract_unsupported_lang_fails_closed() {
        let r = ExtractorRegistry::new();
        match r.extract(LangId::Python, b"") {
            Ok(_) => assert!(false, "must fail closed"),
            Err(e) => {
                assert_eq!(e.code, SymbolErrorCode::StateNotReady);
                assert!(e.is_lang_unsupported());
                assert!(e.detail.contains("PYTHON"));
            }
        }
    }

    #[test]
    fn extract_supported_lang_routes_to_registered() {
        let mut r = ExtractorRegistry::new();
        let _registered: bool = r.register(
            LangId::Python,
            Box::new(MockExtractor::new(vec![one_symbol()])),
        );
        let got = match r.extract(LangId::Python, b"") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got, vec![one_symbol()]);
    }

    #[test]
    fn supported_langs_lists_registered_langs() {
        let mut r = ExtractorRegistry::new();
        let _rs: bool = r.register(LangId::Rust, Box::new(MockExtractor::new(Vec::new())));
        let _go: bool = r.register(LangId::Go, Box::new(MockExtractor::new(Vec::new())));
        let langs = r.supported_langs();
        assert!(langs.contains(&LangId::Rust));
        assert!(langs.contains(&LangId::Go));
        assert_eq!(langs.len(), 2);
    }
}
