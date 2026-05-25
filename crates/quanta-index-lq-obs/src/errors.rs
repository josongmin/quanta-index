//! Typed errors for the LQ observability surface.
//!
//! Every failure across dimension validation, span/metric emission, the
//! cardinality guard, and the audit sink maps to exactly one
//! [`ObsErrorCode`] variant. No silent fallback; no silent default.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of observability-surface failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[expect(
    clippy::enum_variant_names,
    reason = "OBS-01 § 8 locks every variant to start with the `Obs` failure-namespace prefix"
)]
pub enum ObsErrorCode {
    /// Cardinality guard rejected an emit. The offending dimension is carried
    /// on the surrounding [`ObsError`].
    ObsCardinalityGuard,
    /// A span event failed structural validation at emit time.
    ObsInvalidSpan,
    /// A metric sample failed structural validation at emit time.
    ObsInvalidMetric,
    /// An audit entry was missing a required field.
    ObsAuditMissingField,
}

impl ObsErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::ObsCardinalityGuard => "OBS_CARDINALITY_GUARD",
            Self::ObsInvalidSpan => "OBS_INVALID_SPAN",
            Self::ObsInvalidMetric => "OBS_INVALID_METRIC",
            Self::ObsAuditMissingField => "OBS_AUDIT_MISSING_FIELD",
        }
    }

    /// Inverse of [`ObsErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "OBS_CARDINALITY_GUARD" => Self::ObsCardinalityGuard,
            "OBS_INVALID_SPAN" => Self::ObsInvalidSpan,
            "OBS_INVALID_METRIC" => Self::ObsInvalidMetric,
            "OBS_AUDIT_MISSING_FIELD" => Self::ObsAuditMissingField,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for ObsErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for ObsErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for ObsErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = ObsErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ObsErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ObsErrorCode, E> {
                ObsErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<ObsErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete observability-surface failure.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObsError {
    /// Typed failure code.
    pub code: ObsErrorCode,
    /// Dimension name that overflowed (only set when `code` is
    /// [`ObsErrorCode::ObsCardinalityGuard`]).
    pub dim_overflow: Option<Box<str>>,
    /// Free-form detail string for operator diagnostics. Not for control flow.
    pub detail: Box<str>,
}

impl ObsError {
    /// Build a typed failure without a dimension tag.
    #[must_use]
    pub fn new(code: ObsErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            dim_overflow: None,
            detail: detail.into(),
        }
    }

    /// Build a cardinality-guard failure tagged with the overflowing dimension.
    #[must_use]
    pub fn cardinality(dim: impl Into<Box<str>>, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: ObsErrorCode::ObsCardinalityGuard,
            dim_overflow: Some(dim.into()),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ObsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dim_overflow.as_deref() {
            Some(d) => write!(f, "{}[dim_overflow={}]: {}", self.code, d, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for ObsError {}

impl serde::Serialize for ObsError {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("ObsError", 3)?;
        st.serialize_field("code", &self.code)?;
        match &self.dim_overflow {
            Some(d) => st.serialize_field("dim_overflow", d.as_ref())?,
            None => st.serialize_field("dim_overflow", &Option::<&str>::None)?,
        }
        st.serialize_field("detail", self.detail.as_ref())?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for ObsError {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            Code,
            DimOverflow,
            Detail,
        }
        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("ObsError field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "code" => Ok(Field::Code),
                            "dim_overflow" => Ok(Field::DimOverflow),
                            "detail" => Ok(Field::Detail),
                            other => {
                                Err(E::unknown_field(other, &["code", "dim_overflow", "detail"]))
                            }
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct EV;
        impl<'d> serde::de::Visitor<'d> for EV {
            type Value = ObsError;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ObsError struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<ObsError, A::Error> {
                let mut code: Option<ObsErrorCode> = None;
                let mut dim_overflow: Option<Option<Box<str>>> = None;
                let mut detail: Option<Box<str>> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::Code => {
                            if code.is_some() {
                                return Err(serde::de::Error::duplicate_field("code"));
                            }
                            code = Some(map.next_value()?);
                        }
                        Field::DimOverflow => {
                            if dim_overflow.is_some() {
                                return Err(serde::de::Error::duplicate_field("dim_overflow"));
                            }
                            let v: Option<String> = map.next_value()?;
                            dim_overflow = Some(v.map(String::into_boxed_str));
                        }
                        Field::Detail => {
                            if detail.is_some() {
                                return Err(serde::de::Error::duplicate_field("detail"));
                            }
                            let v: String = map.next_value()?;
                            detail = Some(v.into_boxed_str());
                        }
                    }
                }
                let code = code.ok_or_else(|| serde::de::Error::missing_field("code"))?;
                let dim_overflow = dim_overflow.unwrap_or(None);
                let detail = detail.ok_or_else(|| serde::de::Error::missing_field("detail"))?;
                Ok(ObsError {
                    code,
                    dim_overflow,
                    detail,
                })
            }
        }

        de.deserialize_struct("ObsError", &["code", "dim_overflow", "detail"], EV)
    }
}

#[cfg(test)]
mod tests {
    use super::{ObsError, ObsErrorCode};

    const ALL_CODES: &[ObsErrorCode] = &[
        ObsErrorCode::ObsCardinalityGuard,
        ObsErrorCode::ObsInvalidSpan,
        ObsErrorCode::ObsInvalidMetric,
        ObsErrorCode::ObsAuditMissingField,
    ];

    #[test]
    fn code_strs_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in ALL_CODES {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn code_strs_roundtrip() {
        for c in ALL_CODES {
            assert_eq!(ObsErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_unknown_returns_none() {
        assert!(ObsErrorCode::from_code_str("NOT_A_CODE").is_none());
        assert!(ObsErrorCode::from_code_str("").is_none());
    }

    #[test]
    fn display_carries_dim_overflow_when_set() {
        let e = ObsError::cardinality("tenant_id", "too many");
        let s = format!("{e}");
        assert!(s.contains("OBS_CARDINALITY_GUARD"));
        assert!(s.contains("tenant_id"));
        assert!(s.contains("too many"));
    }

    #[test]
    fn display_omits_dim_overflow_when_absent() {
        let e = ObsError::new(ObsErrorCode::ObsInvalidSpan, "bad");
        let s = format!("{e}");
        assert!(s.contains("OBS_INVALID_SPAN"));
        assert!(!s.contains("dim_overflow="));
    }

    #[test]
    fn obs_error_json_roundtrip() {
        let e = ObsError::cardinality("repo_id", "cap exceeded");
        let buf = match serde_json::to_vec(&e) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "serialize: {err}");
                return;
            }
        };
        match serde_json::from_slice::<ObsError>(&buf) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "deserialize: {err}"),
        }
    }

    #[test]
    fn obs_error_json_roundtrip_no_dim() {
        let e = ObsError::new(ObsErrorCode::ObsInvalidMetric, "bad name");
        let buf = match serde_json::to_vec(&e) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "serialize: {err}");
                return;
            }
        };
        match serde_json::from_slice::<ObsError>(&buf) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "deserialize: {err}"),
        }
    }
}
