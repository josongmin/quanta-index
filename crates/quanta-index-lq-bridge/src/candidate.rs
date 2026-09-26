//! [`BridgeCandidate`] — the bridge wire shape for an
//! already-translated Sourcegraph query.
//!
//! Per the current Sourcegraph compatibility boundary
//! (`docs/adr/JUN-06-001-sourcegraph-compatibility-boundary.md`),
//! every bridge output carries:
//!
//! - `source_syntax`: the original Sourcegraph input string (retained
//!   for explainability; never trusted as authority).
//! - `translator_version`: stamped from [`crate::version::TRANSLATOR_VERSION`]
//!   so a downstream consumer can correlate any refusal with the
//!   translator version that emitted it (§ 5.4 step 4 — translator
//!   version stamped on every output).
//! - `translated`: the lowered canonical `LqQuery`.
//!
//! Construction is via [`BridgeCandidate::new`], which is the only
//! supported entry point and which stamps `translator_version`
//! automatically from the const so callers cannot drift it.
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::version::TRANSLATOR_VERSION;
use quanta_index_contract::LqQuery;

/// Stable bridge candidate envelope.
///
/// Mirrors a subset of `BridgeCandidatePacket` (owned by
/// `quanta-index-contract`) so the translator crate can produce
/// envelopes without depending on the contract crate; the
/// integration ticket maps this onto the full packet shape.
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeCandidate {
    pub source_syntax: Box<str>,
    pub translator_version: Box<str>,
    pub translated: LqQuery,
}

impl BridgeCandidate {
    /// Construct a `BridgeCandidate` from the original Sourcegraph
    /// input string + the lowered directive. Stamps
    /// [`TRANSLATOR_VERSION`] from the crate const.
    #[must_use]
    pub fn new(source_syntax: impl Into<Box<str>>, translated: LqQuery) -> Self {
        Self {
            source_syntax: source_syntax.into(),
            translator_version: Box::<str>::from(TRANSLATOR_VERSION),
            translated,
        }
    }

    /// Constructor variant that allows pinning a non-default
    /// translator version. Intended for cross-version skew tests in
    /// the integration ticket; the const path is the production
    /// surface.
    #[must_use]
    pub fn with_translator_version(
        source_syntax: impl Into<Box<str>>,
        translator_version: impl Into<Box<str>>,
        translated: LqQuery,
    ) -> Self {
        Self {
            source_syntax: source_syntax.into(),
            translator_version: translator_version.into(),
            translated,
        }
    }
}

impl serde::Serialize for BridgeCandidate {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        m.serialize_entry("source_syntax", self.source_syntax.as_ref())?;
        m.serialize_entry("translator_version", self.translator_version.as_ref())?;
        m.serialize_entry("translated", &self.translated)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for BridgeCandidate {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = BridgeCandidate;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("BridgeCandidate map (source_syntax, translator_version, translated)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<BridgeCandidate, M::Error> {
                let mut source_syntax: Option<String> = None;
                let mut translator_version: Option<String> = None;
                let mut translated: Option<LqQuery> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "source_syntax" => {
                            if source_syntax.is_some() {
                                return Err(serde::de::Error::duplicate_field("source_syntax"));
                            }
                            source_syntax = Some(map.next_value()?);
                        }
                        "translator_version" => {
                            if translator_version.is_some() {
                                return Err(serde::de::Error::duplicate_field(
                                    "translator_version",
                                ));
                            }
                            translator_version = Some(map.next_value()?);
                        }
                        "translated" => {
                            if translated.is_some() {
                                return Err(serde::de::Error::duplicate_field("translated"));
                            }
                            translated = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["source_syntax", "translator_version", "translated"],
                            ));
                        }
                    }
                }
                let source_syntax = source_syntax
                    .ok_or_else(|| serde::de::Error::missing_field("source_syntax"))?;
                let translator_version = translator_version
                    .ok_or_else(|| serde::de::Error::missing_field("translator_version"))?;
                let translated =
                    translated.ok_or_else(|| serde::de::Error::missing_field("translated"))?;
                Ok(BridgeCandidate {
                    source_syntax: source_syntax.into_boxed_str(),
                    translator_version: translator_version.into_boxed_str(),
                    translated,
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::BridgeCandidate;
    use crate::syntax::parse_sourcegraph;
    use crate::translate_query;
    use crate::version::{SourcegraphVersionTag, TRANSLATOR_VERSION};
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};

    fn empty_query() -> LqQuery {
        LqQuery::empty(LqSpan::eof(0))
    }

    fn translate_for(sg: &str) -> LqQuery {
        let q = match parse_sourcegraph(sg) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse `{sg}`: {e}");
                return empty_query();
            }
        };
        let v = match SourcegraphVersionTag::supported() {
            Ok(t) => t,
            Err(e) => {
                assert!(false, "supported pin must parse: {e}");
                return empty_query();
            }
        };
        match translate_query(q, &v, sg.len()) {
            Ok(d) => d,
            Err(e) => {
                assert!(false, "translate `{sg}`: {e}");
                empty_query()
            }
        }
    }

    #[test]
    fn new_stamps_translator_version_from_const() {
        let lq = translate_for("repo:acme foo");
        let c = BridgeCandidate::new("repo:acme foo", lq);
        assert_eq!(c.source_syntax.as_ref(), "repo:acme foo");
        assert_eq!(c.translator_version.as_ref(), TRANSLATOR_VERSION);
    }

    #[test]
    fn with_translator_version_overrides() {
        let c = BridgeCandidate::with_translator_version(
            "foo",
            "lq-bridge-v9999",
            LqQuery {
                expr: LqExpr::Leaf(LqLeaf::Keyword("foo".to_string())),
                ..empty_query()
            },
        );
        assert_eq!(c.translator_version.as_ref(), "lq-bridge-v9999");
    }

    #[test]
    fn serde_roundtrip_via_ciborium() {
        let lq = translate_for("lang:rust foo");
        let mut c = BridgeCandidate::new("lang:rust foo", lq);
        c.translated.source_span = LqSpan::eof(0);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<BridgeCandidate, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, c),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn deserialize_rejects_missing_field() {
        // Round-trip a map missing `translated`.
        let mut buf: Vec<u8> = Vec::new();
        let bad: std::collections::BTreeMap<String, String> = [
            ("source_syntax".to_string(), "x".to_string()),
            ("translator_version".to_string(), "v1".to_string()),
        ]
        .into_iter()
        .collect();
        if let Err(e) = ciborium::ser::into_writer(&bad, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<BridgeCandidate, _> = ciborium::de::from_reader(buf.as_slice());
        assert!(got.is_err(), "missing field must reject");
    }
}
