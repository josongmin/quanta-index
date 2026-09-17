//! QI-BB-015 — the metrics snapshot and its request on the control wire.
//!
//! The snapshot round-trips through CBOR byte-for-byte as a value, rides the
//! adjacent-tagged control envelope under its own kind, and every shape that
//! would make a scrape lie is refused at decode: a name Prometheus cannot
//! take, a NaN, buckets that are not cumulative or do not end in `+Inf`, a
//! name used twice, an unknown or missing field, and a request that carries
//! anything at all.

#![forbid(unsafe_code)]

use ciborium::value::Value;
use quanta_index_contract::ipc::{
    MetricBucketV1, MetricCounterV1, MetricGaugeV1, MetricHistogramV1, MetricsDiagnosticsV1,
    MetricsSnapshotRequest, MetricsSnapshotV1, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, is_metric_name_v1,
};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn decode<T>(bytes: &[u8]) -> Result<T, ciborium::de::Error<std::io::Error>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    ciborium::de::from_reader(bytes)
}

fn snapshot() -> MetricsSnapshotV1 {
    MetricsSnapshotV1 {
        counters: vec![
            MetricCounterV1 {
                name: "ipc_query_requests_dispatched_total".to_string(),
                value: 13,
            },
            MetricCounterV1 {
                name: "lq_route_lexical_served_total".to_string(),
                value: u64::MAX,
            },
        ],
        gauges: vec![MetricGaugeV1 {
            name: "ipc_query_connections_live".to_string(),
            value: 0.5,
        }],
        histograms: vec![MetricHistogramV1 {
            name: "lq_route_lexical_latency_ms".to_string(),
            count: 3,
            sum: 8.5,
            min: 0.5,
            max: 7.0,
            buckets: vec![
                MetricBucketV1 { le: 1.0, count: 2 },
                MetricBucketV1 { le: 2.5, count: 2 },
                MetricBucketV1 { le: 10.0, count: 3 },
                MetricBucketV1 {
                    le: f64::INFINITY,
                    count: 3,
                },
            ],
        }],
        diagnostics: MetricsDiagnosticsV1 {
            samples_recorded: 40,
            samples_dropped: 1,
            errors_recorded: 2,
            errors_dropped: 0,
        },
    }
}

/// The fixture as a generic CBOR value the test can bend into shapes the
/// typed constructors refuse to build.
fn forged(edit: impl FnOnce(&mut Value) -> Option<()>) -> Result<Value, String> {
    let mut value = Value::serialized(&snapshot()).map_err(|err| err.to_string())?;
    edit(&mut value).ok_or_else(|| "the fixture has the path the forgery edits".to_string())?;
    Ok(value)
}

/// Decode a forged CBOR value as a snapshot.
fn decode_forged(value: &Value) -> Result<MetricsSnapshotV1, String> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(value, &mut buf).map_err(|err| err.to_string())?;
    decode::<MetricsSnapshotV1>(&buf).map_err(|err| err.to_string())
}

/// The entry `key` of a CBOR map.
fn field<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    value
        .as_map_mut()?
        .iter_mut()
        .find(|(name, _)| name.as_text() == Some(key))
        .map(|(_, entry)| entry)
}

/// Element `index` of a CBOR array.
fn item(value: &mut Value, index: usize) -> Option<&mut Value> {
    value.as_array_mut()?.get_mut(index)
}

/// Walk `path` of alternating field names and array positions.
fn at<'a>(value: &'a mut Value, path: &[Step]) -> Option<&'a mut Value> {
    let mut cursor = value;
    for step in path {
        cursor = match step {
            Step::Field(key) => field(cursor, key)?,
            Step::Item(index) => item(cursor, *index)?,
        };
    }
    Some(cursor)
}

enum Step {
    Field(&'static str),
    Item(usize),
}

use Step::{Field, Item};

#[test]
fn snapshot_round_trips_and_rides_the_control_envelope() -> TestRes {
    let value = snapshot();
    let decoded: MetricsSnapshotV1 = decode(&encode(&value)?)?;
    if decoded != value {
        return Err(format!("snapshot round trip drifted: {decoded:?}").into());
    }
    let response = SearchPlaneControlIpcResponseEnvelope {
        request_id: 77,
        payload: SearchPlaneControlIpcResponse::MetricsSnapshot(value),
    };
    let decoded: SearchPlaneControlIpcResponseEnvelope = decode(&encode(&response)?)?;
    if decoded != response {
        return Err(format!("response envelope drifted: {decoded:?}").into());
    }
    let request = SearchPlaneControlIpcRequestEnvelope {
        request_id: 77,
        payload: SearchPlaneControlIpcRequest::MetricsSnapshot(MetricsSnapshotRequest),
    };
    let decoded: SearchPlaneControlIpcRequestEnvelope = decode(&encode(&request)?)?;
    if decoded != request {
        return Err(format!("request envelope drifted: {decoded:?}").into());
    }
    // The kind tag is the variant's own name, so a peer without this
    // variant refuses it as unknown rather than mis-decoding it.
    let json = serde_json::to_value(&request.payload)?;
    if json.get("kind").and_then(serde_json::Value::as_str) != Some("MetricsSnapshot") {
        return Err(format!("request kind tag: {json}").into());
    }
    let json = serde_json::to_value(&response.payload)?;
    if json.get("kind").and_then(serde_json::Value::as_str) != Some("MetricsSnapshot") {
        return Err(format!("response kind tag: {json}").into());
    }
    Ok(())
}

#[test]
fn metric_names_are_prometheus_safe_by_construction() -> TestRes {
    for name in [
        "a",
        "lq_route_lexical_latency_ms",
        "ipc_query_connections_live",
        "boot_repomap_snapshots_loaded1",
    ] {
        if !is_metric_name_v1(name) {
            return Err(format!("`{name}` is a valid name").into());
        }
    }
    for name in [
        "",
        "_leading_underscore",
        "1starts_with_digit",
        "Upper",
        "has-dash",
        "has space",
        "has.dot",
        "lq_route_{route}",
        "ünïcode",
    ] {
        if is_metric_name_v1(name) {
            return Err(format!("`{name}` is not a valid name").into());
        }
    }
    Ok(())
}

#[test]
fn every_lying_shape_is_refused_at_decode() -> TestRes {
    let set = |path: &'static [Step], to: Value| {
        move |value: &mut Value| {
            *at(value, path)? = to;
            Some(())
        }
    };
    let cases: [(&str, &str, Result<Value, String>); 11] = [
        (
            "counter name",
            "is not [a-z][a-z0-9_]*",
            forged(set(
                &[Field("counters"), Item(0), Field("name")],
                Value::Text("Bad-Name".to_string()),
            )),
        ),
        (
            "gauge name",
            "is not [a-z][a-z0-9_]*",
            forged(set(
                &[Field("gauges"), Item(0), Field("name")],
                Value::Text("has space".to_string()),
            )),
        ),
        (
            "histogram name",
            "is not [a-z][a-z0-9_]*",
            forged(set(
                &[Field("histograms"), Item(0), Field("name")],
                Value::Text("1x".to_string()),
            )),
        ),
        (
            "gauge not a number",
            "expected float",
            forged(set(
                &[Field("gauges"), Item(0), Field("value")],
                Value::Null,
            )),
        ),
        (
            "buckets not ascending",
            "not cumulative in ascending bound order",
            forged(set(
                &[
                    Field("histograms"),
                    Item(0),
                    Field("buckets"),
                    Item(1),
                    Field("le"),
                ],
                Value::Float(0.5),
            )),
        ),
        (
            "buckets not cumulative",
            "not cumulative in ascending bound order",
            forged(set(
                &[
                    Field("histograms"),
                    Item(0),
                    Field("buckets"),
                    Item(1),
                    Field("count"),
                ],
                Value::Integer(1.into()),
            )),
        ),
        (
            "last bucket not +Inf",
            "must end in a +Inf bucket",
            forged(|value| {
                let buckets = at(value, &[Field("histograms"), Item(0), Field("buckets")])?;
                let _last = buckets.as_array_mut()?.pop()?;
                Some(())
            }),
        ),
        (
            "+Inf bucket not the count",
            "not cumulative in ascending bound order",
            forged(set(
                &[
                    Field("histograms"),
                    Item(0),
                    Field("buckets"),
                    Item(3),
                    Field("count"),
                ],
                Value::Integer(2.into()),
            )),
        ),
        (
            "name used twice across kinds",
            "appears twice",
            forged(set(
                &[Field("gauges"), Item(0), Field("name")],
                Value::Text("lq_route_lexical_served_total".to_string()),
            )),
        ),
        (
            "unknown field",
            "unknown field `labels`",
            forged(|value| {
                value
                    .as_map_mut()?
                    .push((Value::Text("labels".to_string()), Value::Map(Vec::new())));
                Some(())
            }),
        ),
        (
            "missing field",
            "missing field `diagnostics`",
            forged(|value| {
                let map = value.as_map_mut()?;
                let position = map
                    .iter()
                    .position(|(name, _)| name.as_text() == Some("diagnostics"))?;
                let _removed = map.remove(position);
                Some(())
            }),
        ),
    ];
    for (case, expected, forged) in cases {
        let value = forged.map_err(|err| format!("`{case}`: {err}"))?;
        match decode_forged(&value) {
            Ok(decoded) => {
                return Err(format!("`{case}` must be refused, decoded {decoded:?}").into());
            }
            Err(refused) if refused.contains(expected) => {}
            Err(refused) => {
                return Err(format!(
                    "`{case}` refused for the wrong reason: {refused} (expected `{expected}`)"
                )
                .into());
            }
        }
    }
    // The control: the unforged shape decodes to the fixture.
    let unforged = forged(|_value| Some(()))?;
    if decode_forged(&unforged)? != snapshot() {
        return Err("the unforged snapshot decodes to itself".into());
    }
    Ok(())
}

/// CBOR carries NaN as a float, so a NaN that an encoder let through is
/// refused by the decoder wherever it sits; a negative gauge is not a lie
/// and decodes.
#[test]
fn nan_anywhere_is_refused_but_a_negative_gauge_is_not() -> TestRes {
    let mut nan_gauge = snapshot();
    if let Some(gauge) = nan_gauge.gauges.first_mut() {
        gauge.value = f64::NAN;
    }
    let mut nan_sum = snapshot();
    let mut nan_min = snapshot();
    let mut nan_max = snapshot();
    let mut nan_bound = snapshot();
    for (field, target) in [
        ("sum", &mut nan_sum),
        ("min", &mut nan_min),
        ("max", &mut nan_max),
        ("le", &mut nan_bound),
    ] {
        let Some(histogram) = target.histograms.first_mut() else {
            return Err("fixture has a histogram".into());
        };
        match field {
            "sum" => histogram.sum = f64::NAN,
            "min" => histogram.min = f64::NAN,
            "max" => histogram.max = f64::NAN,
            _le => {
                if let Some(bucket) = histogram.buckets.first_mut() {
                    bucket.le = f64::NAN;
                }
            }
        }
    }
    for (case, forged) in [
        ("gauge", nan_gauge),
        ("histogram sum", nan_sum),
        ("histogram min", nan_min),
        ("histogram max", nan_max),
        ("bucket bound", nan_bound),
    ] {
        let bytes = encode(&forged)?;
        if decode::<MetricsSnapshotV1>(&bytes).is_ok() {
            return Err(format!("a NaN {case} must be refused at decode").into());
        }
    }
    let negative = forged(|value| {
        *at(value, &[Field("gauges"), Item(0), Field("value")])? = Value::Float(-3.25);
        Some(())
    })?;
    let decoded = decode_forged(&negative)?;
    if decoded.gauges.first().map(|gauge| gauge.value) != Some(-3.25) {
        return Err(format!("negative gauge decodes: {decoded:?}").into());
    }
    Ok(())
}

#[test]
fn the_scrape_request_is_empty_and_refuses_any_field() -> TestRes {
    let bytes = encode(&MetricsSnapshotRequest)?;
    let decoded: MetricsSnapshotRequest = decode(&bytes)?;
    if decoded != MetricsSnapshotRequest {
        return Err("the empty request round-trips".into());
    }
    let mut forged: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(&serde_json::json!({ "reset": true }), &mut forged)?;
    if decode::<MetricsSnapshotRequest>(&forged).is_ok() {
        return Err("a request carrying a field is refused".into());
    }
    Ok(())
}
