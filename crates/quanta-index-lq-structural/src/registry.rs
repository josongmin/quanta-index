//! Lang-keyed registry of [`StructuralMatcher`] implementations.
//!
//! Missing-language lookup fails closed with
//! [`crate::errors::StructuralErrorCode::StrLangNotSupported`]; there is
//! no silent skip path. A registered matcher that returns zero candidates
//! is promoted to [`crate::errors::StructuralErrorCode::StrLangResolutionEmpty`]
//! **only** when the pattern carries metavariables — pure-literal
//! patterns may legitimately return zero matches.
//!
//! D18 — no proc-macro derives.

use std::collections::BTreeMap;

use crate::binding::StructuralCandidate;
use crate::errors::StructuralError;
use crate::matcher::StructuralMatcher;
use crate::pattern::StructuralPattern;
use crate::types::LangId;

/// Per-language matcher table.
pub struct MatcherRegistry {
    by_lang: BTreeMap<LangId, Box<dyn StructuralMatcher>>,
}

impl MatcherRegistry {
    /// Construct an empty registry. Every language is unsupported until
    /// [`Self::register`] lands its matcher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_lang: BTreeMap::new(),
        }
    }

    /// Register `matcher` against `lang`. Returns `true` if a prior
    /// registration was displaced.
    pub fn register(&mut self, lang: LangId, matcher: Box<dyn StructuralMatcher>) -> bool {
        self.by_lang.insert(lang, matcher).is_some()
    }

    /// Borrow the matcher registered for `lang`, or `None` if absent.
    #[must_use]
    pub fn get(&self, lang: LangId) -> Option<&dyn StructuralMatcher> {
        self.by_lang.get(&lang).map(AsRef::as_ref)
    }

    /// `true` if `lang` has a registered matcher.
    #[must_use]
    pub fn is_supported(&self, lang: LangId) -> bool {
        self.by_lang.contains_key(&lang)
    }

    /// All languages with a registered matcher, in declaration order.
    #[must_use]
    pub fn supported_langs(&self) -> Vec<LangId> {
        self.by_lang.keys().copied().collect()
    }

    /// Run the registered matcher for `pattern.lang()` over `source`.
    ///
    /// - Returns `STR_LANG_NOT_SUPPORTED{lang}` if the language is not
    ///   registered.
    /// - Returns `STR_LANG_RESOLUTION_EMPTY{lang}` if the matcher
    ///   returned zero candidates **and** the pattern carries
    ///   metavariables (an empty match for a metavariable pattern is
    ///   structurally suspect per §8.5 step 3).
    /// - Forwards any matcher-side error verbatim.
    pub fn match_pattern(
        &self,
        pattern: &StructuralPattern,
        source: &[u8],
    ) -> Result<Vec<StructuralCandidate>, StructuralError> {
        let lang = pattern.lang();
        let Some(m) = self.get(lang) else {
            return Err(StructuralError::lang_not_supported(lang.as_code_str()));
        };
        let out = m.match_pattern(pattern, source)?;
        if out.is_empty() && pattern.has_metavars() {
            return Err(StructuralError::lang_resolution_empty(lang.as_code_str()));
        }
        Ok(out)
    }
}

impl Default for MatcherRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Convenience constructor attached to StructuralError, kept here next to
// the registry that emits it.
impl StructuralError {
    /// Convenience: `STR_LANG_RESOLUTION_EMPTY{lang}`.
    #[must_use]
    pub fn lang_resolution_empty(lang_code: &str) -> Self {
        Self::new(
            crate::errors::StructuralErrorCode::StrLangResolutionEmpty,
            format!("STR_LANG_RESOLUTION_EMPTY{{lang=\"{lang_code}\"}}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::MatcherRegistry;
    use crate::binding::{StructuralBinding, StructuralCandidate};
    use crate::errors::StructuralErrorCode;
    use crate::matcher::MockStructuralMatcher;
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

    fn one_candidate() -> StructuralCandidate {
        StructuralCandidate::new(DocId(1), span(0, 3), StructuralBinding::empty())
    }

    #[test]
    fn empty_registry_unsupported() {
        let r = MatcherRegistry::new();
        for l in LangId::all() {
            assert!(!r.is_supported(*l));
            assert!(r.get(*l).is_none());
        }
        assert!(r.supported_langs().is_empty());
    }

    #[test]
    fn register_then_match() {
        let mut r = MatcherRegistry::new();
        let displaced = r.register(
            LangId::Rust,
            Box::new(MockStructuralMatcher::new(vec![one_candidate()])),
        );
        assert!(!displaced);
        let Ok(pat) = parse_pattern("hi", LangId::Rust) else {
            assert!(false, "parse");
            return;
        };
        let got = match r.match_pattern(&pat, b"hi") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got, vec![one_candidate()]);
    }

    #[test]
    fn re_register_displaces() {
        let mut r = MatcherRegistry::new();
        let first = r.register(
            LangId::Rust,
            Box::new(MockStructuralMatcher::new(Vec::new())),
        );
        assert!(!first);
        let displaced = r.register(
            LangId::Rust,
            Box::new(MockStructuralMatcher::new(vec![one_candidate()])),
        );
        assert!(displaced);
    }

    #[test]
    fn unsupported_lang_fails_with_typed_code() {
        let r = MatcherRegistry::new();
        let Ok(pat) = parse_pattern("hi", LangId::Python) else {
            assert!(false, "parse");
            return;
        };
        match r.match_pattern(&pat, b"") {
            Ok(_) => assert!(false, "must fail closed"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::StrLangNotSupported);
                assert!(e.detail.contains("PYTHON"));
            }
        }
    }

    #[test]
    fn empty_result_on_metavar_pattern_promotes_to_resolution_empty() {
        let mut r = MatcherRegistry::new();
        let _r = r.register(
            LangId::Rust,
            Box::new(MockStructuralMatcher::new(Vec::new())),
        );
        let Ok(pat) = parse_pattern("$X", LangId::Rust) else {
            assert!(false, "parse");
            return;
        };
        match r.match_pattern(&pat, b"") {
            Ok(_) => assert!(false, "metavar pattern with empty matches must promote"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::StrLangResolutionEmpty);
                assert!(e.detail.contains("RUST"));
            }
        }
    }

    #[test]
    fn empty_result_on_literal_pattern_is_success() {
        let mut r = MatcherRegistry::new();
        let _r = r.register(
            LangId::Rust,
            Box::new(MockStructuralMatcher::new(Vec::new())),
        );
        let Ok(pat) = parse_pattern("hi", LangId::Rust) else {
            assert!(false, "parse");
            return;
        };
        match r.match_pattern(&pat, b"") {
            Ok(v) => assert!(v.is_empty()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn supported_langs_lists_registered() {
        let mut r = MatcherRegistry::new();
        let _a = r.register(
            LangId::Rust,
            Box::new(MockStructuralMatcher::new(Vec::new())),
        );
        let _b = r.register(LangId::Go, Box::new(MockStructuralMatcher::new(Vec::new())));
        let langs = r.supported_langs();
        assert!(langs.contains(&LangId::Rust));
        assert!(langs.contains(&LangId::Go));
        assert_eq!(langs.len(), 2);
    }
}
