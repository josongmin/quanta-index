//! Optional excerpt evidence for the selected native text ranking.
//!
//! The wire shape checks its own ranges and row bindings. The selected-view
//! reader must separately prove the emitted bytes against each source object.

use super::{NativeTextResult, ObjectRef, OwnerKey};
use crate::{
    PreviewByteRange, PreviewUnavailableReason, bounded_cluster_members::BoundedVecV1,
    results::HighlightSpan,
};
use quanta_index_contract_base::ClosedFieldKeySeedV1;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Error as _, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct,
};

pub const MAX_NATIVE_TEXT_PREVIEW_BYTES: usize = 240;
pub const MAX_NATIVE_TEXT_PREVIEW_HIGHLIGHTS: usize = 32;
const MAX_NATIVE_TEXT_PREVIEW_ROWS: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextExcerpt {
    pub text: String,
    /// Half-open byte range in the selected immutable source.
    pub original_focus: PreviewByteRange,
    /// Half-open byte range in the selected immutable source; its bytes form `text`.
    pub original_context: PreviewByteRange,
    /// Half-open byte range in the normalized text used for ranking.
    pub normalized_focus: PreviewByteRange,
    /// Ordered, unique, UTF-8-aligned byte ranges in `text`; overlaps are allowed.
    pub highlights: Vec<HighlightSpan>,
}

impl NativeTextExcerpt {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.text.len() > MAX_NATIVE_TEXT_PREVIEW_BYTES {
            return Err("native text excerpt exceeds 240 UTF-8 bytes");
        }
        if self.highlights.len() > MAX_NATIVE_TEXT_PREVIEW_HIGHLIGHTS {
            return Err("native text excerpt exceeds 32 highlights");
        }
        self.original_focus.validate()?;
        self.original_context.validate()?;
        self.normalized_focus.validate()?;
        if self.original_context.start > self.original_focus.start
            || self.original_context.end < self.original_focus.end
        {
            return Err("native text context must contain original focus");
        }
        let context_len = self
            .original_context
            .end
            .checked_sub(self.original_context.start)
            .ok_or("native text context byte range underflow")?;
        let text_len = u64::try_from(self.text.len())
            .map_err(|_width| "native text excerpt length overflows u64")?;
        if context_len != text_len {
            return Err("native text context length disagrees with excerpt");
        }
        let focus_start = self
            .original_focus
            .start
            .checked_sub(self.original_context.start)
            .ok_or("native text focus begins before context")?;
        let focus_end = self
            .original_focus
            .end
            .checked_sub(self.original_context.start)
            .ok_or("native text focus ends before context")?;
        let focus_len = self
            .original_focus
            .end
            .checked_sub(self.original_focus.start)
            .ok_or("native text focus byte range underflow")?;
        let local_focus_start = usize::try_from(focus_start)
            .map_err(|_width| "native text focus start overflows usize")?;
        let local_focus_end =
            usize::try_from(focus_end).map_err(|_width| "native text focus end overflows usize")?;
        if self.text.get(local_focus_start..local_focus_end).is_none() {
            return Err("native text focus is not UTF-8 aligned");
        }
        let mut previous = None;
        for span in &self.highlights {
            let start = usize::try_from(span.start)
                .map_err(|_width| "native text highlight start overflows usize")?;
            let len = usize::try_from(span.len)
                .map_err(|_width| "native text highlight length overflows usize")?;
            let end = start
                .checked_add(len)
                .ok_or("native text highlight end overflows usize")?;
            if len == 0 || self.text.get(start..end).is_none() {
                return Err("native text highlight must cover nonempty UTF-8 bytes");
            }
            let key = (span.start, span.len);
            if previous.is_some_and(|prior| prior >= key) {
                return Err("native text highlights must be sorted and unique");
            }
            previous = Some(key);
        }
        let primary = self
            .highlights
            .first()
            .ok_or("native text focus has no highlight")?;
        if u64::from(primary.start) != focus_start || u64::from(primary.len) != focus_len {
            return Err("native text focus disagrees with primary highlight");
        }
        Ok(())
    }
}

struct NativeTextExcerptWire {
    text: String,
    original_focus: PreviewByteRange,
    original_context: PreviewByteRange,
    normalized_focus: PreviewByteRange,
    highlights: BoundedVecV1<HighlightSpan, MAX_NATIVE_TEXT_PREVIEW_HIGHLIGHTS>,
}

const NATIVE_TEXT_EXCERPT_FIELDS: &[&str] = &[
    "text",
    "original_focus",
    "original_context",
    "normalized_focus",
    "highlights",
];

impl<'de> Deserialize<'de> for NativeTextExcerptWire {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExcerptVisitor;
        impl<'de> Visitor<'de> for ExcerptVisitor {
            type Value = NativeTextExcerptWire;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a complete NativeTextExcerpt record")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut text = None;
                let mut original_focus = None;
                let mut original_context = None;
                let mut normalized_focus = None;
                let mut highlights = None;
                while let Some(field) =
                    map.next_key_seed(ClosedFieldKeySeedV1(NATIVE_TEXT_EXCERPT_FIELDS))?
                {
                    match field {
                        "text" => {
                            if text.is_some() {
                                return Err(de::Error::duplicate_field("text"));
                            }
                            text = Some(map.next_value()?);
                        }
                        "original_focus" => {
                            if original_focus.is_some() {
                                return Err(de::Error::duplicate_field("original_focus"));
                            }
                            original_focus = Some(map.next_value()?);
                        }
                        "original_context" => {
                            if original_context.is_some() {
                                return Err(de::Error::duplicate_field("original_context"));
                            }
                            original_context = Some(map.next_value()?);
                        }
                        "normalized_focus" => {
                            if normalized_focus.is_some() {
                                return Err(de::Error::duplicate_field("normalized_focus"));
                            }
                            normalized_focus = Some(map.next_value()?);
                        }
                        "highlights" => {
                            if highlights.is_some() {
                                return Err(de::Error::duplicate_field("highlights"));
                            }
                            highlights = Some(map.next_value()?);
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(NativeTextExcerptWire {
                    text: text.ok_or_else(|| de::Error::missing_field("text"))?,
                    original_focus: original_focus
                        .ok_or_else(|| de::Error::missing_field("original_focus"))?,
                    original_context: original_context
                        .ok_or_else(|| de::Error::missing_field("original_context"))?,
                    normalized_focus: normalized_focus
                        .ok_or_else(|| de::Error::missing_field("normalized_focus"))?,
                    highlights: highlights.ok_or_else(|| de::Error::missing_field("highlights"))?,
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let text = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("text"))?;
                let original_focus = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("original_focus"))?;
                let original_context = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("original_context"))?;
                let normalized_focus = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("normalized_focus"))?;
                let highlights = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("highlights"))?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(6, &self));
                }
                Ok(NativeTextExcerptWire {
                    text,
                    original_focus,
                    original_context,
                    normalized_focus,
                    highlights,
                })
            }
        }
        deserializer.deserialize_struct(
            "NativeTextExcerpt",
            NATIVE_TEXT_EXCERPT_FIELDS,
            ExcerptVisitor,
        )
    }
}

impl Serialize for NativeTextExcerpt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("NativeTextExcerpt", 5)?;
        state.serialize_field("text", &self.text)?;
        state.serialize_field("original_focus", &self.original_focus)?;
        state.serialize_field("original_context", &self.original_context)?;
        state.serialize_field("normalized_focus", &self.normalized_focus)?;
        state.serialize_field("highlights", &self.highlights)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for NativeTextExcerpt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = NativeTextExcerptWire::deserialize(deserializer)?;
        let value = Self {
            text: wire.text,
            original_focus: wire.original_focus,
            original_context: wire.original_context,
            normalized_focus: wire.normalized_focus,
            highlights: wire.highlights.into_inner(),
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeTextPreviewEvidence {
    Excerpt(NativeTextExcerpt),
    Unavailable(PreviewUnavailableReason),
    /// Display the bound ranked hit's path without copying it into evidence.
    Path,
}

impl Serialize for NativeTextPreviewEvidence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Excerpt(excerpt) => serializer.serialize_newtype_variant(
                "NativeTextPreviewEvidence",
                0,
                "Excerpt",
                excerpt,
            ),
            Self::Unavailable(reason) => serializer.serialize_newtype_variant(
                "NativeTextPreviewEvidence",
                1,
                "Unavailable",
                reason,
            ),
            Self::Path => serializer.serialize_unit_variant("NativeTextPreviewEvidence", 2, "Path"),
        }
    }
}

impl<'de> Deserialize<'de> for NativeTextPreviewEvidence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const PAYLOAD_VARIANTS: &[&str] = &["Excerpt", "Unavailable"];
        const VARIANTS: &[&str] = &["Excerpt", "Unavailable", "Path"];
        struct EvidenceVisitor {
            require_definite_map: bool,
        }
        impl<'de> Visitor<'de> for EvidenceVisitor {
            type Value = NativeTextPreviewEvidence;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a Path unit or an Excerpt/Unavailable payload")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "Path" {
                    Ok(NativeTextPreviewEvidence::Path)
                } else {
                    Err(E::unknown_variant(value, VARIANTS))
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                if self.require_definite_map && map.size_hint() != Some(1) {
                    return Err(de::Error::custom(
                        "native text evidence requires a definite one-entry binary map",
                    ));
                }
                let variant = map
                    .next_key_seed(ClosedFieldKeySeedV1(PAYLOAD_VARIANTS))?
                    .ok_or_else(|| de::Error::custom("native text evidence variant missing"))?;
                let value = match variant {
                    "Excerpt" => NativeTextPreviewEvidence::Excerpt(map.next_value()?),
                    "Unavailable" => NativeTextPreviewEvidence::Unavailable(map.next_value()?),
                    _ => return Err(de::Error::unknown_variant(variant, PAYLOAD_VARIANTS)),
                };
                if map.next_key::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        "native text evidence has multiple variants",
                    ));
                }
                Ok(value)
            }
        }
        // CBOR unit-variant access can leave a map payload unread. Dispatch
        // on the value shape and preserve the definite map used by binary
        // Excerpt and Unavailable variants.
        let require_definite_map = !deserializer.is_human_readable();
        deserializer.deserialize_any(EvidenceVisitor {
            require_definite_map,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPreview {
    pub owner: OwnerKey,
    pub source: ObjectRef,
    pub evidence: NativeTextPreviewEvidence,
}
quanta_index_contract_base::native_serde_struct!(NativeTextPreview {
    owner: OwnerKey,
    source: ObjectRef,
    evidence: NativeTextPreviewEvidence,
});

/// One preview row per ranked hit, in the ranking's order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPreviewResult {
    pub ranking: NativeTextResult,
    pub previews: Vec<NativeTextPreview>,
}

impl NativeTextPreviewResult {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_native_text_previews(&self.ranking, &self.previews)
    }
}

pub(super) fn validate_native_text_previews(
    ranking: &NativeTextResult,
    previews: &[NativeTextPreview],
) -> Result<(), &'static str> {
    if previews.len() > MAX_NATIVE_TEXT_PREVIEW_ROWS {
        return Err("native text preview exceeds 1024 rows");
    }
    if ranking.hits.len() != previews.len() {
        return Err("native text preview count disagrees with ranked hits");
    }
    for (hit, preview) in ranking.hits.iter().zip(previews) {
        if hit.owner != preview.owner || hit.source != preview.source {
            return Err("native text preview identity disagrees with ranked hit");
        }
        match &preview.evidence {
            NativeTextPreviewEvidence::Excerpt(excerpt) => excerpt.validate()?,
            NativeTextPreviewEvidence::Path => {
                if hit.path.is_empty() || hit.path.len() > MAX_NATIVE_TEXT_PREVIEW_BYTES {
                    return Err("native text path preview must contain 1..=240 UTF-8 bytes");
                }
            }
            NativeTextPreviewEvidence::Unavailable(_) => {}
        }
    }
    Ok(())
}

struct NativeTextPreviewResultWire {
    ranking: NativeTextResult,
    previews: BoundedVecV1<NativeTextPreview, MAX_NATIVE_TEXT_PREVIEW_ROWS>,
}

const NATIVE_TEXT_PREVIEW_RESULT_FIELDS: &[&str] = &["ranking", "previews"];

impl<'de> Deserialize<'de> for NativeTextPreviewResultWire {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ResultVisitor;
        impl<'de> Visitor<'de> for ResultVisitor {
            type Value = NativeTextPreviewResultWire;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a complete NativeTextPreviewResult record")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut ranking = None;
                let mut previews = None;
                while let Some(field) =
                    map.next_key_seed(ClosedFieldKeySeedV1(NATIVE_TEXT_PREVIEW_RESULT_FIELDS))?
                {
                    match field {
                        "ranking" => {
                            if ranking.is_some() {
                                return Err(de::Error::duplicate_field("ranking"));
                            }
                            ranking = Some(map.next_value()?);
                        }
                        "previews" => {
                            if previews.is_some() {
                                return Err(de::Error::duplicate_field("previews"));
                            }
                            previews = Some(map.next_value()?);
                        }
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(NativeTextPreviewResultWire {
                    ranking: ranking.ok_or_else(|| de::Error::missing_field("ranking"))?,
                    previews: previews.ok_or_else(|| de::Error::missing_field("previews"))?,
                })
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let ranking = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("ranking"))?;
                let previews = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::missing_field("previews"))?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(3, &self));
                }
                Ok(NativeTextPreviewResultWire { ranking, previews })
            }
        }
        deserializer.deserialize_struct(
            "NativeTextPreviewResult",
            NATIVE_TEXT_PREVIEW_RESULT_FIELDS,
            ResultVisitor,
        )
    }
}

impl Serialize for NativeTextPreviewResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("NativeTextPreviewResult", 2)?;
        state.serialize_field("ranking", &self.ranking)?;
        state.serialize_field("previews", &self.previews)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for NativeTextPreviewResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = NativeTextPreviewResultWire::deserialize(deserializer)?;
        let value = Self {
            ranking: wire.ranking,
            previews: wire.previews.into_inner(),
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        NativeIndexRequest, NativeIndexResponse, NativeTextCoverage, NativeTextHit,
    };
    use super::*;
    use crate::{
        FiniteScore,
        index::{
            CollectionId, Digest, IndexInstanceId, ProducerId, SlotId, StableOwnerId, TenantId,
            ViewRef, WorkspaceKey, WorktreeId,
        },
    };
    use crate::{QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax, index::ViewSelection};

    fn excerpt() -> NativeTextExcerpt {
        NativeTextExcerpt {
            text: "aébc".into(),
            original_focus: PreviewByteRange { start: 11, end: 13 },
            original_context: PreviewByteRange { start: 10, end: 15 },
            normalized_focus: PreviewByteRange { start: 1, end: 3 },
            highlights: vec![HighlightSpan { start: 1, len: 2 }],
        }
    }

    fn result() -> NativeTextPreviewResult {
        let owner = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let source = ObjectRef(Digest([3; 32]));
        NativeTextPreviewResult {
            ranking: NativeTextResult {
                view: ViewRef {
                    workspace: WorkspaceKey {
                        tenant: TenantId([4; 16]),
                        collection: CollectionId([5; 16]),
                        worktree: WorktreeId([6; 16]),
                        slot: SlotId([7; 16]),
                    },
                    epoch: 1,
                    sequence: 2,
                    digest: Digest([8; 32]),
                },
                instance: IndexInstanceId([9; 16]),
                coverage: NativeTextCoverage::CoreRequired,
                artifact: ObjectRef(Digest([10; 32])),
                matched: 1,
                exhaustive: true,
                hits: vec![NativeTextHit {
                    owner,
                    source,
                    path: "a.rs".into(),
                    score: FiniteScore::new(1.0).expect("finite fixture score"),
                }],
            },
            previews: vec![NativeTextPreview {
                owner,
                source,
                evidence: NativeTextPreviewEvidence::Excerpt(excerpt()),
            }],
        }
    }

    #[test]
    fn native_preview_roundtrips_and_binds_ranked_hit() -> Result<(), Box<dyn std::error::Error>> {
        let value = result();
        value.validate()?;
        let json = serde_json::to_string(&value)?;
        assert_eq!(
            serde_json::from_str::<NativeTextPreviewResult>(&json)?,
            value
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&value, &mut cbor)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeTextPreviewResult, _>(cbor.as_slice())?,
            value
        );
        assert_eq!(
            serde_json::to_string(&NativeTextPreviewEvidence::Unavailable(
                PreviewUnavailableReason::WorkBudget
            ))?,
            r#"{"Unavailable":"work_budget"}"#
        );
        let unavailable =
            NativeTextPreviewEvidence::Unavailable(PreviewUnavailableReason::WorkBudget);
        let mut unavailable_cbor = Vec::new();
        ciborium::ser::into_writer(&unavailable, &mut unavailable_cbor)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeTextPreviewEvidence, _>(unavailable_cbor.as_slice())?,
            unavailable
        );
        Ok(())
    }

    #[test]
    fn native_path_preview_is_a_unit_bound_to_the_ranked_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut value = result();
        value.ranking.hits[0].path = format!("src/{}", "é".repeat(118));
        assert_eq!(
            value.ranking.hits[0].path.len(),
            MAX_NATIVE_TEXT_PREVIEW_BYTES
        );
        value.previews[0].evidence = NativeTextPreviewEvidence::Path;
        value.validate()?;
        assert_eq!(
            serde_json::to_string(&NativeTextPreviewEvidence::Path)?,
            r#""Path""#
        );
        let mut evidence_cbor = Vec::new();
        ciborium::ser::into_writer(&NativeTextPreviewEvidence::Path, &mut evidence_cbor)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeTextPreviewEvidence, _>(evidence_cbor.as_slice())?,
            NativeTextPreviewEvidence::Path
        );

        let json = serde_json::to_string(&value)?;
        assert_eq!(
            serde_json::from_str::<NativeTextPreviewResult>(&json)?,
            value
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&value, &mut cbor)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeTextPreviewResult, _>(cbor.as_slice())?,
            value
        );
        Ok(())
    }

    #[test]
    fn native_path_preview_refuses_payload_coordinates_and_invalid_path_lengths()
    -> Result<(), Box<dyn std::error::Error>> {
        for payload in [
            serde_json::json!({"Path": null}),
            serde_json::json!({"Path": "src/lib.rs"}),
            serde_json::json!({"Path": {"original_focus": {"start": 0, "end": 1}}}),
        ] {
            assert!(serde_json::from_value::<NativeTextPreviewEvidence>(payload.clone()).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&payload, &mut cbor)?;
            assert!(
                ciborium::de::from_reader::<NativeTextPreviewEvidence, _>(cbor.as_slice()).is_err()
            );
        }

        let too_long = format!("src/{}x", "é".repeat(118));
        assert_eq!(
            too_long.len(),
            MAX_NATIVE_TEXT_PREVIEW_BYTES.saturating_add(1)
        );
        let mut value = result();
        value.previews[0].evidence = NativeTextPreviewEvidence::Path;
        for path in [String::new(), too_long] {
            value.ranking.hits[0].path = path.clone();
            assert!(value.validate().is_err());
            assert!(serde_json::to_string(&value).is_err());

            let mut wire = serde_json::to_value(result())?;
            wire["previews"][0]["evidence"] =
                serde_json::to_value(&NativeTextPreviewEvidence::Path)?;
            wire["ranking"]["hits"][0]["path"] = serde_json::Value::String(path);
            assert!(serde_json::from_value::<NativeTextPreviewResult>(wire.clone()).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&wire, &mut cbor)?;
            assert!(
                ciborium::de::from_reader::<NativeTextPreviewResult, _>(cbor.as_slice()).is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn native_preview_keeps_binary_evidence_map_definite() {
        let indefinite = b"\xbf\x6bUnavailable\x6bwork_budget\xff";
        assert!(
            ciborium::de::from_reader::<NativeTextPreviewEvidence, _>(indefinite.as_slice())
                .is_err()
        );
    }

    #[test]
    fn native_preview_has_separate_opt_in_wire_variants() -> Result<(), Box<dyn std::error::Error>>
    {
        let value = result();
        let request = NativeIndexRequest::SearchTextPreview {
            selection: ViewSelection::Exact(value.ranking.view),
            instance: value.ranking.instance,
            query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "a".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 1,
                cursor: None,
            },
        };
        let request_json = serde_json::to_value(&request)?;
        assert!(request_json.get("SearchTextPreview").is_some());
        assert_eq!(
            serde_json::from_value::<NativeIndexRequest>(request_json)?,
            request
        );
        let mut request_cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut request_cbor)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(request_cbor.as_slice())?,
            request
        );

        let response = NativeIndexResponse::TextPreviewRanked(value);
        let response_json = serde_json::to_value(&response)?;
        assert!(response_json.get("TextPreviewRanked").is_some());
        assert_eq!(
            serde_json::from_value::<NativeIndexResponse>(response_json)?,
            response
        );
        let mut response_cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut response_cbor)?;
        assert_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(response_cbor.as_slice())?,
            response
        );

        struct BorrowedPreview<'a>(&'a NativeTextPreviewResult);
        impl Serialize for BorrowedPreview<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_text_preview_ranked_ref(self.0, serializer)
            }
        }
        let NativeIndexResponse::TextPreviewRanked(value) = &response else {
            unreachable!("preview response fixture")
        };
        assert_eq!(
            serde_json::to_value(BorrowedPreview(value))?,
            serde_json::to_value(&response)?
        );
        let mut borrowed_cbor = Vec::new();
        ciborium::ser::into_writer(&BorrowedPreview(value), &mut borrowed_cbor)?;
        assert_eq!(borrowed_cbor, response_cbor);
        Ok(())
    }

    #[test]
    fn native_preview_rejects_invalid_excerpt_and_row_bindings()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut invalid = excerpt();
        invalid
            .text
            .push_str(&"x".repeat(MAX_NATIVE_TEXT_PREVIEW_BYTES));
        assert!(invalid.validate().is_err());
        assert!(serde_json::to_string(&invalid).is_err());

        let mut invalid = excerpt();
        invalid.highlights.push(HighlightSpan { start: 1, len: 2 });
        assert!(invalid.validate().is_err());

        let mut invalid = excerpt();
        invalid.highlights = vec![HighlightSpan { start: 1, len: 2 }; 33];
        assert!(invalid.validate().is_err());

        let mut invalid = excerpt();
        invalid.original_focus = PreviewByteRange { start: 12, end: 13 };
        assert!(invalid.validate().is_err());

        let mut invalid = result();
        invalid.previews[0].source = ObjectRef(Digest([11; 32]));
        assert!(invalid.validate().is_err());
        assert!(serde_json::to_string(&invalid).is_err());
        let mut invalid_wire = serde_json::to_value(result())?;
        invalid_wire["previews"][0]["source"] = serde_json::to_value(ObjectRef(Digest([11; 32])))?;
        assert!(serde_json::from_value::<NativeTextPreviewResult>(invalid_wire).is_err());

        let mut invalid = result();
        invalid.previews.clear();
        assert!(invalid.validate().is_err());
        Ok(())
    }

    #[test]
    fn native_preview_wire_refuses_first_excess_row_and_highlight()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            u32::try_from(MAX_NATIVE_TEXT_PREVIEW_ROWS)?,
            super::super::MAX_NATIVE_TEXT_TOP_K
        );

        let mut excerpt_wire = serde_json::to_value(excerpt())?;
        excerpt_wire["highlights"] = serde_json::to_value(vec![
            HighlightSpan { start: 1, len: 2 };
            MAX_NATIVE_TEXT_PREVIEW_HIGHLIGHTS
                .saturating_add(1)
        ])?;
        let error = serde_json::from_value::<NativeTextExcerpt>(excerpt_wire.clone())
            .expect_err("highlight cap must reject before DTO validation");
        assert!(
            error
                .to_string()
                .contains("at most 32 bounded sequence elements")
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&excerpt_wire, &mut cbor)?;
        assert!(ciborium::de::from_reader::<NativeTextExcerpt, _>(cbor.as_slice()).is_err());

        let mut result_wire = serde_json::to_value(result())?;
        let row = result_wire["previews"][0].clone();
        result_wire["previews"] =
            serde_json::Value::Array(vec![row; MAX_NATIVE_TEXT_PREVIEW_ROWS.saturating_add(1)]);
        let error = serde_json::from_value::<NativeTextPreviewResult>(result_wire.clone())
            .expect_err("preview row cap must reject before DTO validation");
        assert!(
            error
                .to_string()
                .contains("at most 1024 bounded sequence elements")
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&result_wire, &mut cbor)?;
        assert!(ciborium::de::from_reader::<NativeTextPreviewResult, _>(cbor.as_slice()).is_err());
        Ok(())
    }
}
