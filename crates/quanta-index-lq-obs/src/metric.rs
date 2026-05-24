//! Typed metric sample shapes per OBS-01 § 4.3.
//!
//! Every metric carries a [`MetricKind`] and the closed
//! [`crate::dim::Dimensions`] label set. No free-text labels. New labels
//! require an `lq_version` minor bump (RFC § Migration and Versioning
//! Policy) plus a coordinated edit to [`crate::dim::Dimensions`].
//!
//! D18 — hand-rolled serde, no proc-macro derives.

use core::fmt;

use crate::dim::Dimensions;

/// Closed metric-kind enumeration aligned with OpenTelemetry / Prometheus
/// instrument types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetricKind {
    /// Monotonically-increasing counter (resets only on process restart).
    Counter,
    /// Point-in-time gauge.
    Gauge,
    /// Distribution histogram.
    Histogram,
}

impl MetricKind {
    /// `snake_case` wire form.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }

    /// Inverse of [`MetricKind::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "counter" => Self::Counter,
            "gauge" => Self::Gauge,
            "histogram" => Self::Histogram,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for MetricKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for MetricKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for MetricKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = MetricKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("MetricKind snake_case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<MetricKind, E> {
                MetricKind::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<MetricKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Typed metric sample. The pair `(name, kind)` keys the registered metric;
/// `dimensions` is the closed label set carried verbatim.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSample {
    /// Registered metric name (e.g. `lq_query_latency_ms`).
    pub name: Box<str>,
    /// Counter / Gauge / Histogram.
    pub kind: MetricKind,
    /// Observed value. `f64` covers both integer counters and float
    /// histograms; the registered metric kind dictates the interpretation.
    pub value: f64,
    /// Closed-label dimension set per OBS-01 § 4.3.
    pub dimensions: Dimensions,
}

impl MetricSample {
    /// Minimal constructor.
    #[must_use]
    pub fn new(
        name: impl Into<Box<str>>,
        kind: MetricKind,
        value: f64,
        dimensions: Dimensions,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            value,
            dimensions,
        }
    }
}

impl serde::Serialize for MetricSample {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("MetricSample", 4)?;
        st.serialize_field("name", self.name.as_ref())?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("value", &self.value)?;
        st.serialize_field("dimensions", &self.dimensions)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for MetricSample {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            Name,
            Kind,
            Value,
            Dimensions,
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
                        f.write_str("MetricSample field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "name" => Ok(Field::Name),
                            "kind" => Ok(Field::Kind),
                            "value" => Ok(Field::Value),
                            "dimensions" => Ok(Field::Dimensions),
                            other => Err(E::unknown_field(
                                other,
                                &["name", "kind", "value", "dimensions"],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct MV;
        impl<'d> serde::de::Visitor<'d> for MV {
            type Value = MetricSample;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("MetricSample struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<MetricSample, A::Error> {
                let mut name: Option<Box<str>> = None;
                let mut kind: Option<MetricKind> = None;
                let mut value: Option<f64> = None;
                let mut dimensions: Option<Dimensions> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::Name => {
                            if name.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            let v: String = map.next_value()?;
                            name = Some(v.into_boxed_str());
                        }
                        Field::Kind => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        Field::Value => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        Field::Dimensions => {
                            if dimensions.is_some() {
                                return Err(serde::de::Error::duplicate_field("dimensions"));
                            }
                            dimensions = Some(map.next_value()?);
                        }
                    }
                }
                Ok(MetricSample {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    kind: kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?,
                    value: value.ok_or_else(|| serde::de::Error::missing_field("value"))?,
                    dimensions: dimensions
                        .ok_or_else(|| serde::de::Error::missing_field("dimensions"))?,
                })
            }
        }

        de.deserialize_struct("MetricSample", &["name", "kind", "value", "dimensions"], MV)
    }
}

#[cfg(test)]
mod tests {
    use super::{MetricKind, MetricSample};
    use crate::dim::Dimensions;

    const ALL_KINDS: &[MetricKind] = &[
        MetricKind::Counter,
        MetricKind::Gauge,
        MetricKind::Histogram,
    ];

    #[test]
    fn metric_kind_code_strs_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for k in ALL_KINDS {
            let s = k.as_code_str();
            assert!(!seen.contains(&s), "duplicate: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn metric_kind_roundtrip() {
        for k in ALL_KINDS {
            assert_eq!(MetricKind::from_code_str(k.as_code_str()), Some(*k));
        }
    }

    #[test]
    fn metric_kind_unknown_returns_none() {
        assert!(MetricKind::from_code_str("summary").is_none());
        assert!(MetricKind::from_code_str("").is_none());
    }

    fn sample() -> MetricSample {
        MetricSample::new(
            "lq_query_latency_ms",
            MetricKind::Histogram,
            42.5,
            Dimensions::new("OBS-01", "8", "t1", "r1", 7),
        )
    }

    #[test]
    fn metric_sample_serde_roundtrip() {
        let s = sample();
        let buf = match serde_json::to_vec(&s) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "ser: {e}");
                return;
            }
        };
        match serde_json::from_slice::<MetricSample>(&buf) {
            Ok(got) => assert_eq!(got, s),
            Err(e) => assert!(false, "de: {e}"),
        }
    }
}
