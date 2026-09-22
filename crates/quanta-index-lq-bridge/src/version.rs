//! Sourcegraph version pin + translator version constants.
//!
//! Per BRIDGE-01 §10 (Risks: BR-R1 Sourcegraph version drift), every
//! `translate` call is anchored to a concrete Sourcegraph release tag.
//! Floating references are not allowed. The pin shape is
//! `sg-<MAJOR>.<MINOR>.<PATCH>`; anything else is rejected with
//! `BRIDGE_VERSION_PIN`.
//!
//! [`TRANSLATOR_VERSION`] is stamped on every `BridgeCandidate` so a
//! downstream consumer can correlate a refusal with the translator
//! version that emitted it (per
//! [BRIDGE-01 § 4 deliverable 3 / § 5.4 step 4](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md)).
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::BridgeError;

/// Currently-supported Sourcegraph reference release.
///
/// Bump policy: every 6 months by default; emergency bumps require an
/// RFC amendment per [BRIDGE-01 § 10 BR-R1](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md).
pub const SUPPORTED_SG_VERSION: &str = "sg-5.5.0";

/// Stable translator version stamped on every `BridgeCandidate`. Format
/// is `lq-bridge-v<MAJOR>`; bumps follow LEX-01 canonical-hash version
/// policy and are tied to RFC § Migration and Versioning Policy.
pub const TRANSLATOR_VERSION: &str = "lq-bridge-v1";

/// Pinned Sourcegraph release tag for a `translate` call.
///
/// Constructed via [`SourcegraphVersionTag::new`], which validates the
/// `sg-X.Y.Z` shape (digits-only major/minor/patch). Anything else
/// returns `BRIDGE_VERSION_PIN`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourcegraphVersionTag(Box<str>);

impl SourcegraphVersionTag {
    /// Validate and wrap a Sourcegraph release tag of the form
    /// `sg-MAJOR.MINOR.PATCH` where each component is one or more
    /// decimal digits.
    pub fn new(raw: &str) -> Result<Self, BridgeError> {
        if !Self::is_valid_shape(raw) {
            return Err(BridgeError::version_pin(
                raw,
                format!(
                    "expected `sg-MAJOR.MINOR.PATCH` (digits-only), got `{raw}`; \
                     supported pin is `{SUPPORTED_SG_VERSION}`"
                ),
            ));
        }
        Ok(Self(Box::<str>::from(raw)))
    }

    /// Convenience constructor for [`SUPPORTED_SG_VERSION`]. Cannot
    /// fail because the constant is validated by the same predicate;
    /// returns a `BridgeError` only if the constant is ever set to a
    /// malformed value (which the unit test below catches at build
    /// time).
    pub fn supported() -> Result<Self, BridgeError> {
        Self::new(SUPPORTED_SG_VERSION)
    }

    /// Borrow the validated tag string (e.g. `"sg-5.5.0"`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` iff `raw` is `sg-<digits>.<digits>.<digits>`.
    fn is_valid_shape(raw: &str) -> bool {
        let Some(rest) = raw.strip_prefix("sg-") else {
            return false;
        };
        // Split into exactly three dot-separated digit-only components.
        let parts: Vec<&str> = rest.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    }
}

impl fmt::Display for SourcegraphVersionTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for SourcegraphVersionTag {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for SourcegraphVersionTag {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SourcegraphVersionTag;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SourcegraphVersionTag `sg-MAJOR.MINOR.PATCH` string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SourcegraphVersionTag, E> {
                SourcegraphVersionTag::new(v).map_err(|err| E::custom(format!("{err}")))
            }
        }
        de.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{SUPPORTED_SG_VERSION, SourcegraphVersionTag, TRANSLATOR_VERSION};
    use crate::errors::BridgeErrorCode;

    #[test]
    fn supported_constant_parses() {
        match SourcegraphVersionTag::new(SUPPORTED_SG_VERSION) {
            Ok(t) => assert_eq!(t.as_str(), SUPPORTED_SG_VERSION),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn translator_version_constant_shape() {
        // The translator version must be stable, prefixed `lq-bridge-`.
        assert!(TRANSLATOR_VERSION.starts_with("lq-bridge-"));
    }

    #[test]
    fn accepts_well_formed_pin() {
        for raw in &["sg-0.0.0", "sg-1.2.3", "sg-5.5.0", "sg-12.34.567"] {
            match SourcegraphVersionTag::new(raw) {
                Ok(t) => assert_eq!(t.as_str(), *raw),
                Err(e) => assert!(false, "rejected well-formed `{raw}`: {e}"),
            }
        }
    }

    #[test]
    fn rejects_missing_prefix() {
        match SourcegraphVersionTag::new("5.5.0") {
            Ok(_) => assert!(false, "must reject missing `sg-` prefix"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_wrong_prefix() {
        match SourcegraphVersionTag::new("zoekt-5.5.0") {
            Ok(_) => assert!(false, "must reject non-sg prefix"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_too_few_components() {
        match SourcegraphVersionTag::new("sg-5.5") {
            Ok(_) => assert!(false, "must reject two-component version"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_too_many_components() {
        match SourcegraphVersionTag::new("sg-5.5.0.1") {
            Ok(_) => assert!(false, "must reject four-component version"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_non_digit_component() {
        match SourcegraphVersionTag::new("sg-5.5.beta") {
            Ok(_) => assert!(false, "must reject non-digit component"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_empty_component() {
        match SourcegraphVersionTag::new("sg-5..0") {
            Ok(_) => assert!(false, "must reject empty component"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_empty_string() {
        match SourcegraphVersionTag::new("") {
            Ok(_) => assert!(false, "must reject empty string"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn rejects_leading_whitespace() {
        match SourcegraphVersionTag::new(" sg-5.5.0") {
            Ok(_) => assert!(false, "must reject leading whitespace"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeVersionPin),
        }
    }

    #[test]
    fn serde_roundtrip_via_ciborium() {
        let tag = match SourcegraphVersionTag::new("sg-5.5.0") {
            Ok(t) => t,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&tag, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<SourcegraphVersionTag, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, tag),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn serde_rejects_malformed_during_deserialize() {
        // Round-trip a plain string that doesn't match the shape.
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&"not-a-version".to_string(), &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<SourcegraphVersionTag, _> = ciborium::de::from_reader(buf.as_slice());
        assert!(
            got.is_err(),
            "deserialize must fail closed on malformed pin"
        );
    }
}
