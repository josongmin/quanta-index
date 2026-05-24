//! Typed span tree shapes per OBS-01 § 4.1.
//!
//! Root: [`ROOT_SPAN`] (`lq.query`). Every applicable child stage emits
//! exactly one [`SpanEvent`] carrying [`SpanKind`], timings, optional
//! `error_code`, and the carried-through `budget_remaining_ms`.
//!
//! D18 — hand-rolled serde, no proc-macro derives.

use core::fmt;
use std::collections::BTreeMap;

/// Root span name for every LQ request. The 14 child kinds in [`SpanKind`]
/// are emitted exactly once each when the corresponding pipeline stage runs.
pub const ROOT_SPAN: &str = "lq.query";

/// Closed enumeration of the 14 child span types. Wire form is `snake_case`
/// — see [`SpanKind::as_code_str`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SpanKind {
    /// `lq.query.parse` — parser entry → AST.
    Parse,
    /// `lq.query.normalize` — desugar + alias resolve + canonicalize.
    Normalize,
    /// `lq.query.plan` — planner.
    Plan,
    /// `lq.query.lexical_fanout` — lexical content/path/symbol fanout.
    LexicalFanout,
    /// `lq.query.lexical_trigram` — trigram-index probe.
    LexicalTrigram,
    /// `lq.query.lexical_positions` — position-list intersection.
    LexicalPositions,
    /// `lq.query.lexical_regex` — regex post-filter.
    LexicalRegex,
    /// `lq.query.lexical_symbol` — symbol-table lookup.
    LexicalSymbol,
    /// `lq.query.lexical_rank` — lexical-lane ranking.
    LexicalRank,
    /// `lq.query.semantic_fanout` — semantic-lane shard fanout.
    SemanticFanout,
    /// `lq.query.semantic_ann` — semantic ANN probe.
    SemanticAnn,
    /// `lq.query.hybrid_merge` — lexical+semantic merge.
    HybridMerge,
    /// `lq.query.hybrid_rank` — hybrid-lane ranking.
    HybridRank,
    /// `lq.query.render` — result envelope assembly.
    Render,
}

impl SpanKind {
    /// `snake_case` wire form. Unique across the closed set.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Normalize => "normalize",
            Self::Plan => "plan",
            Self::LexicalFanout => "lexical_fanout",
            Self::LexicalTrigram => "lexical_trigram",
            Self::LexicalPositions => "lexical_positions",
            Self::LexicalRegex => "lexical_regex",
            Self::LexicalSymbol => "lexical_symbol",
            Self::LexicalRank => "lexical_rank",
            Self::SemanticFanout => "semantic_fanout",
            Self::SemanticAnn => "semantic_ann",
            Self::HybridMerge => "hybrid_merge",
            Self::HybridRank => "hybrid_rank",
            Self::Render => "render",
        }
    }

    /// Inverse of [`SpanKind::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "parse" => Self::Parse,
            "normalize" => Self::Normalize,
            "plan" => Self::Plan,
            "lexical_fanout" => Self::LexicalFanout,
            "lexical_trigram" => Self::LexicalTrigram,
            "lexical_positions" => Self::LexicalPositions,
            "lexical_regex" => Self::LexicalRegex,
            "lexical_symbol" => Self::LexicalSymbol,
            "lexical_rank" => Self::LexicalRank,
            "semantic_fanout" => Self::SemanticFanout,
            "semantic_ann" => Self::SemanticAnn,
            "hybrid_merge" => Self::HybridMerge,
            "hybrid_rank" => Self::HybridRank,
            "render" => Self::Render,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for SpanKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for SpanKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for SpanKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SpanKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SpanKind snake_case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SpanKind, E> {
                SpanKind::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<SpanKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Typed span emission record. Every child span attaches `error_code` and
/// `budget_remaining_ms` per OBS-01 § 4.1, plus a free-form `attributes`
/// map whose keys must come from the per-stage label registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanEvent {
    /// Closed-set child span kind.
    pub kind: SpanKind,
    /// Span start time, milliseconds since UNIX epoch. Never `now()` —
    /// callers anchor against the request's monotonic clock snapshot.
    pub start_ms: u64,
    /// Span duration in milliseconds.
    pub duration_ms: u64,
    /// `Some` when the stage emitted a typed error; `None` on success.
    pub error_code: Option<Box<str>>,
    /// Carried-through budget remaining at span exit. Used to reconstruct
    /// partial-failure traces without log correlation.
    pub budget_remaining_ms: u64,
    /// Per-stage labels. Keys must be registered in the per-stage label set
    /// (enforced by the emit boundary, not this struct).
    pub attributes: BTreeMap<Box<str>, Box<str>>,
}

impl SpanEvent {
    /// Minimal constructor; attributes start empty.
    #[must_use]
    pub fn new(kind: SpanKind, start_ms: u64, duration_ms: u64, budget_remaining_ms: u64) -> Self {
        Self {
            kind,
            start_ms,
            duration_ms,
            error_code: None,
            budget_remaining_ms,
            attributes: BTreeMap::new(),
        }
    }
}

impl serde::Serialize for SpanEvent {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("SpanEvent", 6)?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("start_ms", &self.start_ms)?;
        st.serialize_field("duration_ms", &self.duration_ms)?;
        match &self.error_code {
            Some(c) => st.serialize_field("error_code", c.as_ref())?,
            None => st.serialize_field("error_code", &Option::<&str>::None)?,
        }
        st.serialize_field("budget_remaining_ms", &self.budget_remaining_ms)?;
        let attrs: BTreeMap<&str, &str> = self
            .attributes
            .iter()
            .map(|(k, v)| (k.as_ref(), v.as_ref()))
            .collect();
        st.serialize_field("attributes", &attrs)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for SpanEvent {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            Kind,
            StartMs,
            DurationMs,
            ErrorCode,
            BudgetRemainingMs,
            Attributes,
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
                        f.write_str("SpanEvent field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "kind" => Ok(Field::Kind),
                            "start_ms" => Ok(Field::StartMs),
                            "duration_ms" => Ok(Field::DurationMs),
                            "error_code" => Ok(Field::ErrorCode),
                            "budget_remaining_ms" => Ok(Field::BudgetRemainingMs),
                            "attributes" => Ok(Field::Attributes),
                            other => Err(E::unknown_field(
                                other,
                                &[
                                    "kind",
                                    "start_ms",
                                    "duration_ms",
                                    "error_code",
                                    "budget_remaining_ms",
                                    "attributes",
                                ],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct SV;
        impl<'d> serde::de::Visitor<'d> for SV {
            type Value = SpanEvent;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SpanEvent struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<SpanEvent, A::Error> {
                let mut kind: Option<SpanKind> = None;
                let mut start_ms: Option<u64> = None;
                let mut duration_ms: Option<u64> = None;
                let mut error_code: Option<Option<Box<str>>> = None;
                let mut budget_remaining_ms: Option<u64> = None;
                let mut attributes: Option<BTreeMap<Box<str>, Box<str>>> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::Kind => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        Field::StartMs => {
                            if start_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("start_ms"));
                            }
                            start_ms = Some(map.next_value()?);
                        }
                        Field::DurationMs => {
                            if duration_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("duration_ms"));
                            }
                            duration_ms = Some(map.next_value()?);
                        }
                        Field::ErrorCode => {
                            if error_code.is_some() {
                                return Err(serde::de::Error::duplicate_field("error_code"));
                            }
                            let v: Option<String> = map.next_value()?;
                            error_code = Some(v.map(String::into_boxed_str));
                        }
                        Field::BudgetRemainingMs => {
                            if budget_remaining_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field(
                                    "budget_remaining_ms",
                                ));
                            }
                            budget_remaining_ms = Some(map.next_value()?);
                        }
                        Field::Attributes => {
                            if attributes.is_some() {
                                return Err(serde::de::Error::duplicate_field("attributes"));
                            }
                            let v: BTreeMap<String, String> = map.next_value()?;
                            let mut out: BTreeMap<Box<str>, Box<str>> = BTreeMap::new();
                            for (k, val) in v {
                                let _prev: Option<Box<str>> =
                                    out.insert(k.into_boxed_str(), val.into_boxed_str());
                            }
                            attributes = Some(out);
                        }
                    }
                }
                Ok(SpanEvent {
                    kind: kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?,
                    start_ms: start_ms
                        .ok_or_else(|| serde::de::Error::missing_field("start_ms"))?,
                    duration_ms: duration_ms
                        .ok_or_else(|| serde::de::Error::missing_field("duration_ms"))?,
                    error_code: error_code.unwrap_or(None),
                    budget_remaining_ms: budget_remaining_ms
                        .ok_or_else(|| serde::de::Error::missing_field("budget_remaining_ms"))?,
                    attributes: attributes.unwrap_or_default(),
                })
            }
        }

        de.deserialize_struct(
            "SpanEvent",
            &[
                "kind",
                "start_ms",
                "duration_ms",
                "error_code",
                "budget_remaining_ms",
                "attributes",
            ],
            SV,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ROOT_SPAN, SpanEvent, SpanKind};

    const ALL: &[SpanKind] = &[
        SpanKind::Parse,
        SpanKind::Normalize,
        SpanKind::Plan,
        SpanKind::LexicalFanout,
        SpanKind::LexicalTrigram,
        SpanKind::LexicalPositions,
        SpanKind::LexicalRegex,
        SpanKind::LexicalSymbol,
        SpanKind::LexicalRank,
        SpanKind::SemanticFanout,
        SpanKind::SemanticAnn,
        SpanKind::HybridMerge,
        SpanKind::HybridRank,
        SpanKind::Render,
    ];

    #[test]
    fn root_span_name_is_lq_query() {
        assert_eq!(ROOT_SPAN, "lq.query");
    }

    #[test]
    fn fourteen_kinds_total() {
        assert_eq!(ALL.len(), 14);
    }

    #[test]
    fn code_strs_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for k in ALL {
            let s = k.as_code_str();
            assert!(!seen.contains(&s), "duplicate: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn code_strs_roundtrip() {
        for k in ALL {
            assert_eq!(SpanKind::from_code_str(k.as_code_str()), Some(*k));
        }
    }

    #[test]
    fn unknown_code_returns_none() {
        assert!(SpanKind::from_code_str("").is_none());
        assert!(SpanKind::from_code_str("not_a_kind").is_none());
    }

    #[test]
    fn span_event_serde_roundtrip() {
        let mut e = SpanEvent::new(SpanKind::Plan, 1_000, 5, 42);
        let _prev: Option<Box<str>> = e
            .attributes
            .insert(Box::from("engine_routed"), Box::from("lexical_content"));
        e.error_code = Some(Box::from("OBS_INVALID_METRIC"));
        let buf = match serde_json::to_vec(&e) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "ser: {err}");
                return;
            }
        };
        match serde_json::from_slice::<SpanEvent>(&buf) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "de: {err}"),
        }
    }

    #[test]
    fn span_event_serde_roundtrip_no_error() {
        let e = SpanEvent::new(SpanKind::HybridMerge, 0, 0, 0);
        let buf = match serde_json::to_vec(&e) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "ser: {err}");
                return;
            }
        };
        match serde_json::from_slice::<SpanEvent>(&buf) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "de: {err}"),
        }
    }
}
