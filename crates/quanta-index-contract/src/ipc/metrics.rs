//! The metrics snapshot a daemon serves over its control socket, and the
//! request that asks for it (QI-BB-015).
//!
//! One scrape returns every counter, gauge and histogram the process has
//! aggregated since it started, plus how many diagnostic samples and
//! errors its bounded rings kept and dropped. Names are the closed metric
//! names the search plane registers; there are no free-text labels and no
//! per-repo or per-query series, so the snapshot's size is bounded by the
//! number of registered metrics, never by traffic.
//!
//! Every number on the wire is finite: a histogram carries its finite
//! bucket bounds and its total `count`, which is what the `+Inf` bucket
//! would hold, so the snapshot round-trips through JSON as well as CBOR.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// A monotonic counter.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricCounterV1 {
    pub name: String,
    pub value: u64,
}

/// A point-in-time gauge.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricGaugeV1 {
    pub name: String,
    pub value: f64,
}

/// One cumulative histogram bucket: observations at or below `le`.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricBucketV1 {
    /// Finite upper bound of the bucket.
    pub le: f64,
    pub count: u64,
}

/// A distribution: count, sum, extremes and cumulative finite buckets.
///
/// `count` is every observation, which is what a `+Inf` bucket would
/// hold; renderers that need one derive it from `count` rather than
/// carrying an infinity on the wire.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricHistogramV1 {
    pub name: String,
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub buckets: Vec<MetricBucketV1>,
}

/// What the diagnostic rings kept and dropped.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetricsDiagnosticsV1 {
    pub samples_recorded: u64,
    pub samples_dropped: u64,
    pub errors_recorded: u64,
    pub errors_dropped: u64,
}

/// Everything one scrape returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricsSnapshotV1 {
    pub counters: Vec<MetricCounterV1>,
    pub gauges: Vec<MetricGaugeV1>,
    pub histograms: Vec<MetricHistogramV1>,
    pub diagnostics: MetricsDiagnosticsV1,
}

/// Ask the daemon for its metrics snapshot.
///
/// A scrape takes no parameters; the struct exists so the request has a
/// typed payload that decodes fail-closed like every other one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetricsSnapshotRequest;

const METRICS_SNAPSHOT_REQUEST_FIELDS: &[&str] = &[];

impl Serialize for MetricsSnapshotRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer
            .serialize_struct("MetricsSnapshotRequest", 0)?
            .end()
    }
}

struct MetricsSnapshotRequestVisitor;

impl<'de> Visitor<'de> for MetricsSnapshotRequestVisitor {
    type Value = MetricsSnapshotRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an empty MetricsSnapshotRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        if let Some(key) = map.next_key::<String>()? {
            return Err(de::Error::unknown_field(&key, METRICS_SNAPSHOT_REQUEST_FIELDS));
        }
        Ok(MetricsSnapshotRequest)
    }
}

impl<'de> Deserialize<'de> for MetricsSnapshotRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "MetricsSnapshotRequest",
            METRICS_SNAPSHOT_REQUEST_FIELDS,
            MetricsSnapshotRequestVisitor,
        )
    }
}

/// A metric name the snapshot accepts: `[a-z0-9_]+`, starting with a letter,
/// so every name is also a valid Prometheus metric name.
#[must_use]
pub fn is_metric_name_v1(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn reject_bad_name<E: de::Error>(name: &str) -> Result<(), E> {
    if is_metric_name_v1(name) {
        Ok(())
    } else {
        Err(E::custom(format!("metric name `{name}` is not [a-z][a-z0-9_]*")))
    }
}

/// Serialize one struct with the given `(field name, value)` pairs, then
/// decode it back field by field, refusing duplicates, unknown and missing
/// fields — the one codec shape every type below uses.
///
/// The default form runs the type's `validate_wire` after decoding; the
/// `unvalidated` form is for a type whose fields admit every value.
macro_rules! metric_struct_serde {
    ($ty:ident, $visitor:ident, $fields:ident, [$($field:ident : $field_ty:ty),+ $(,)?]) => {
        metric_struct_serde!(@codec $ty, $visitor, $fields, [$($field : $field_ty),+]);

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let decoded = deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)?;
                decoded.validate_wire::<D::Error>()?;
                Ok(decoded)
            }
        }
    };
    ($ty:ident, $visitor:ident, $fields:ident, [$($field:ident : $field_ty:ty),+ $(,)?], unvalidated) => {
        metric_struct_serde!(@codec $ty, $visitor, $fields, [$($field : $field_ty),+]);

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
    (@codec $ty:ident, $visitor:ident, $fields:ident, [$($field:ident : $field_ty:ty),+ $(,)?]) => {
        const $fields: &[&str] = &[$(stringify!($field)),+];

        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), $fields.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                $(let mut $field: Option<$field_ty> = None;)+
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        $(
                            stringify!($field) => {
                                if $field.is_some() {
                                    return Err(de::Error::duplicate_field(stringify!($field)));
                                }
                                $field = Some(map.next_value()?);
                            }
                        )+
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                Ok($ty {
                    $($field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,)+
                })
            }
        }
    };
}

metric_struct_serde!(MetricCounterV1, MetricCounterVisitor, METRIC_COUNTER_FIELDS, [name: String, value: u64]);
metric_struct_serde!(MetricGaugeV1, MetricGaugeVisitor, METRIC_GAUGE_FIELDS, [name: String, value: f64]);
metric_struct_serde!(MetricBucketV1, MetricBucketVisitor, METRIC_BUCKET_FIELDS, [le: f64, count: u64]);
metric_struct_serde!(
    MetricHistogramV1,
    MetricHistogramVisitor,
    METRIC_HISTOGRAM_FIELDS,
    [name: String, count: u64, sum: f64, min: f64, max: f64, buckets: Vec<MetricBucketV1>]
);
metric_struct_serde!(
    MetricsDiagnosticsV1,
    MetricsDiagnosticsVisitor,
    METRICS_DIAGNOSTICS_FIELDS,
    [samples_recorded: u64, samples_dropped: u64, errors_recorded: u64, errors_dropped: u64],
    unvalidated
);
metric_struct_serde!(
    MetricsSnapshotV1,
    MetricsSnapshotVisitor,
    METRICS_SNAPSHOT_FIELDS,
    [
        counters: Vec<MetricCounterV1>,
        gauges: Vec<MetricGaugeV1>,
        histograms: Vec<MetricHistogramV1>,
        diagnostics: MetricsDiagnosticsV1
    ]
);

impl MetricCounterV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        reject_bad_name(&self.name)
    }
}

impl MetricGaugeV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        reject_bad_name(&self.name)?;
        if !self.value.is_finite() {
            return Err(E::custom(format!("gauge `{}` is not finite", self.name)));
        }
        Ok(())
    }
}

impl MetricBucketV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        if !self.le.is_finite() {
            return Err(E::custom("histogram bucket bound is not finite"));
        }
        Ok(())
    }
}

impl MetricHistogramV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        reject_bad_name(&self.name)?;
        if !self.sum.is_finite() || !self.min.is_finite() || !self.max.is_finite() {
            return Err(E::custom(format!(
                "histogram `{}` carries a non-finite summary",
                self.name
            )));
        }
        let mut previous: Option<&MetricBucketV1> = None;
        for bucket in &self.buckets {
            if previous
                .is_some_and(|earlier| bucket.le <= earlier.le || bucket.count < earlier.count)
            {
                return Err(E::custom(format!(
                    "histogram `{}` buckets are not cumulative in ascending bound order",
                    self.name
                )));
            }
            previous = Some(bucket);
        }
        if previous.is_some_and(|last| last.count > self.count) {
            return Err(E::custom(format!(
                "histogram `{}` has a bucket holding more than its count",
                self.name
            )));
        }
        Ok(())
    }
}

impl MetricsSnapshotV1 {
    fn validate_wire<E: de::Error>(&self) -> Result<(), E> {
        let mut names = std::collections::BTreeSet::new();
        for name in self
            .counters
            .iter()
            .map(|counter| counter.name.as_str())
            .chain(self.gauges.iter().map(|gauge| gauge.name.as_str()))
            .chain(
                self.histograms
                    .iter()
                    .map(|histogram| histogram.name.as_str()),
            )
        {
            if !names.insert(name) {
                return Err(E::custom(format!("metric `{name}` appears twice")));
            }
        }
        Ok(())
    }
}
