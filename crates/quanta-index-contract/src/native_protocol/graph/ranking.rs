//! One certified projection row and the view-bound `Native` `RepoMap` rank wire.
//!
//! The producer stores the existing compiled projection entry as an immutable
//! leaf. The graph output and supporter refer to that leaf; neither adjacency
//! nor this query wire carries an invented legacy snapshot envelope.

use crate::{
    CompiledRepoMapProjectionEntryV1, RepoMapDocType, RepoMapFocusSubjectDto, RepoMapNodeRef,
    RepoRelativePath,
    index::{Digest, IndexInstanceId, ObjectRef, ViewRef},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};
use std::fmt;

/// The existing compiler permits one MiB of preview text per projection row.
pub const MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES: usize = 1 << 20;
pub const MAX_REPOMAP_RANK_PATH_BYTES: usize = 4 << 10;
pub const MAX_REPOMAP_RANK_SUBJECT_BYTES: usize = 2 << 10;
pub const MAX_REPOMAP_RANK_QUERY_BYTES: usize = 16 << 10;
/// JSON can escape each UTF-8 byte of a one-MiB search preview. This caps
/// the persisted encoded leaf; the decoded search text retains its one-MiB cap.
pub const MAX_REPOMAP_RANK_PROJECTION_LEAF_BYTES: usize = 8 << 20;
pub const MAX_REPOMAP_RANK_FOCUS_SUBJECTS: usize = 10_000;
pub const MAX_REPOMAP_RANK_HITS: usize = 10_000;
const MAX_REASON_CODES: usize = 8;
const MAX_REASON_BYTES: usize = 64;

/// A recognized Index policy, not a digest supplied by an arbitrary producer.
/// The producer projection policy is checked separately for compatibility.
pub const NATIVE_REPOMAP_RANK_POLICY_DESCRIPTOR: &str = "quanta-index/native-repomap-rank/v1{compiled-projection=v1;score-millis=0..1000;token-hint=64;query-tokenizer=lq-text-normalizer-2.0-folded;match=unicode-folded-substring;order=exact-focus,owner-path-focus,match-count,final-score,raw-identity;first-row-budget-floor=true;drop=legacy-v1}";

#[must_use]
pub fn native_repomap_rank_policy() -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/native-repomap-rank-policy/v1\0");
    hash.update(NATIVE_REPOMAP_RANK_POLICY_DESCRIPTOR.as_bytes());
    Digest(hash.finalize().into())
}

/// Sorted by `subject` in one certified owner output. `row` is the canonical
/// `CompiledRepoMapProjectionEntryV1` leaf and must decode to this subject.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphProjectionRow {
    pub subject: RepoMapNodeRef,
    pub row: ObjectRef,
}
crate::native_serde_struct!(GraphProjectionRow {
    subject: RepoMapNodeRef,
    row: ObjectRef
});

fn valid_subject_type(subject: &RepoMapNodeRef, doc_type: RepoMapDocType) -> bool {
    matches!(
        (subject, doc_type),
        (RepoMapNodeRef::File(_), RepoMapDocType::File)
            | (RepoMapNodeRef::Symbol(_), RepoMapDocType::Symbol)
    )
}

fn valid_rank_subject(subject: &RepoMapNodeRef) -> bool {
    let raw = match subject {
        RepoMapNodeRef::File(id) => id.as_str(),
        RepoMapNodeRef::Symbol(id) => id.as_str(),
        RepoMapNodeRef::Module(_) | RepoMapNodeRef::Chunk(_) => return false,
    };
    !raw.is_empty() && raw.len() <= MAX_REPOMAP_RANK_SUBJECT_BYTES
}

impl CompiledRepoMapProjectionEntryV1 {
    /// The persisted rank row does not silently clamp a corrupt score or infer
    /// a File/Symbol kind from a mismatched `doc_type`.
    #[must_use]
    pub fn valid_native_rank_row(&self) -> bool {
        valid_subject_type(&self.subject, self.doc_type)
            && valid_rank_subject(&self.subject)
            && (0..=1_000).contains(&self.final_score_millis)
            && !self.owner_path.as_str().is_empty()
            && self.owner_path.as_str().len() <= MAX_REPOMAP_RANK_PATH_BYTES
            && self.search_text.len() <= MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES
    }
}

impl Serialize for CompiledRepoMapProjectionEntryV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.valid_native_rank_row() {
            return Err(serde::ser::Error::custom(
                "invalid native RepoMap projection row",
            ));
        }
        let mut fields = serializer.serialize_struct("CompiledRepoMapProjectionEntryV1", 5)?;
        fields.serialize_field("subject", &self.subject)?;
        fields.serialize_field("doc_type", &self.doc_type)?;
        fields.serialize_field("owner_path", &self.owner_path)?;
        fields.serialize_field("search_text", &self.search_text)?;
        fields.serialize_field("final_score_millis", &self.final_score_millis)?;
        fields.end()
    }
}

/// Checks decoded length before making a retained `String` copy.
///
/// Borrowed UTF-8 is checked before the copy. The containing object/request
/// has a separate byte cap for readers that own their input buffer.
struct LimitedText<const MAX: usize>(String);

impl<'de, const MAX: usize> Deserialize<'de> for LimitedText<MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text<const MAX: usize>;
        impl<const MAX: usize> Visitor<'_> for Text<MAX> {
            type Value = LimitedText<MAX>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("bounded UTF-8 text")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() > MAX {
                    return Err(E::custom("native RepoMap text exceeds byte bound"));
                }
                let mut text = String::new();
                text.try_reserve_exact(value.len())
                    .map_err(|_error| E::custom("native RepoMap text allocation refused"))?;
                text.push_str(value);
                Ok(LimitedText(text))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                if value.len() > MAX {
                    return Err(E::custom("native RepoMap text exceeds byte bound"));
                }
                Ok(LimitedText(value))
            }
        }
        deserializer.deserialize_str(Text::<MAX>)
    }
}

impl<'de> Deserialize<'de> for CompiledRepoMapProjectionEntryV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Row;
        impl<'de> Visitor<'de> for Row {
            type Value = CompiledRepoMapProjectionEntryV1;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a certified native RepoMap projection row")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut subject, mut doc_type, mut owner_path, mut search_text, mut score) =
                    (None, None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "subject" if subject.is_none() => subject = Some(map.next_value()?),
                        "doc_type" if doc_type.is_none() => doc_type = Some(map.next_value()?),
                        "owner_path" if owner_path.is_none() => {
                            owner_path = Some(
                                map.next_value::<LimitedText<MAX_REPOMAP_RANK_PATH_BYTES>>()?
                                    .0,
                            );
                        }
                        "search_text" if search_text.is_none() => {
                            search_text = Some(
                                map.next_value::<LimitedText<MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES>>(
                                )?
                                .0,
                            );
                        }
                        "final_score_millis" if score.is_none() => score = Some(map.next_value()?),
                        "subject" => return Err(de::Error::duplicate_field("subject")),
                        "doc_type" => return Err(de::Error::duplicate_field("doc_type")),
                        "owner_path" => return Err(de::Error::duplicate_field("owner_path")),
                        "search_text" => return Err(de::Error::duplicate_field("search_text")),
                        "final_score_millis" => {
                            return Err(de::Error::duplicate_field("final_score_millis"));
                        }
                        _ => return Err(de::Error::unknown_field(&field, PROJECTION_FIELDS)),
                    }
                }
                let row = CompiledRepoMapProjectionEntryV1 {
                    subject: subject.ok_or_else(|| de::Error::missing_field("subject"))?,
                    doc_type: doc_type.ok_or_else(|| de::Error::missing_field("doc_type"))?,
                    owner_path: RepoRelativePath::new(
                        owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
                    ),
                    search_text: search_text
                        .ok_or_else(|| de::Error::missing_field("search_text"))?,
                    final_score_millis: score
                        .ok_or_else(|| de::Error::missing_field("final_score_millis"))?,
                };
                if !row.valid_native_rank_row() {
                    return Err(de::Error::custom("invalid native RepoMap projection row"));
                }
                Ok(row)
            }
        }
        deserializer.deserialize_struct("CompiledRepoMapProjectionEntryV1", PROJECTION_FIELDS, Row)
    }
}

const PROJECTION_FIELDS: &[&str] = &[
    "subject",
    "doc_type",
    "owner_path",
    "search_text",
    "final_score_millis",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapQuery {
    pub query_text: String,
    pub focus_subjects: Vec<RepoMapFocusSubjectDto>,
    pub top_k: u32,
    pub token_budget: u32,
}

impl NativeRepoMapQuery {
    #[must_use]
    pub fn valid_native_rank_query(&self) -> bool {
        !(self.query_text.trim().is_empty()
            || self.query_text.len() > MAX_REPOMAP_RANK_QUERY_BYTES
            || self.focus_subjects.len() > MAX_REPOMAP_RANK_FOCUS_SUBJECTS
            || self.focus_subjects.iter().any(|focus| {
                focus.subject_identity.is_empty()
                    || focus.subject_identity.len() > MAX_REPOMAP_RANK_SUBJECT_BYTES
            })
            || self.top_k == 0
            || self.top_k > crate::PUBLIC_TOP_K_MAX
            || !usize::try_from(self.top_k).is_ok_and(|top_k| top_k <= MAX_REPOMAP_RANK_HITS)
            || self.token_budget == 0)
    }
}

impl Serialize for NativeRepoMapQuery {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.valid_native_rank_query() {
            return Err(serde::ser::Error::custom(
                "invalid native RepoMap rank query",
            ));
        }
        let mut fields = serializer.serialize_struct("NativeRepoMapQuery", 4)?;
        fields.serialize_field("query_text", &self.query_text)?;
        fields.serialize_field("focus_subjects", &self.focus_subjects)?;
        fields.serialize_field("top_k", &self.top_k)?;
        fields.serialize_field("token_budget", &self.token_budget)?;
        fields.end()
    }
}

struct LimitedFocusSubjects(Vec<RepoMapFocusSubjectDto>);

struct LimitedFocusSubject(RepoMapFocusSubjectDto);
impl<'de> Deserialize<'de> for LimitedFocusSubject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Focus;
        impl<'de> Visitor<'de> for Focus {
            type Value = LimitedFocusSubject;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a typed RepoMap focus subject")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut identity, mut doc_type) = (None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "subject_identity" if identity.is_none() => {
                            identity = Some(
                                map.next_value::<LimitedText<MAX_REPOMAP_RANK_SUBJECT_BYTES>>()?
                                    .0,
                            );
                        }
                        "subject_doc_type" if doc_type.is_none() => {
                            doc_type = Some(map.next_value()?);
                        }
                        "subject_identity" => {
                            return Err(de::Error::duplicate_field("subject_identity"));
                        }
                        "subject_doc_type" => {
                            return Err(de::Error::duplicate_field("subject_doc_type"));
                        }
                        _ => return Err(de::Error::unknown_field(&field, FOCUS_FIELDS)),
                    }
                }
                let subject_identity: String =
                    identity.ok_or_else(|| de::Error::missing_field("subject_identity"))?;
                let subject_doc_type =
                    doc_type.ok_or_else(|| de::Error::missing_field("subject_doc_type"))?;
                if subject_identity.is_empty() {
                    return Err(de::Error::custom("invalid native RepoMap focus subject"));
                }
                Ok(LimitedFocusSubject(RepoMapFocusSubjectDto {
                    subject_identity,
                    subject_doc_type,
                }))
            }
        }
        deserializer.deserialize_struct("RepoMapFocusSubjectDto", FOCUS_FIELDS, Focus)
    }
}
const FOCUS_FIELDS: &[&str] = &["subject_identity", "subject_doc_type"];

impl<'de> Deserialize<'de> for LimitedFocusSubjects {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Focuses;
        impl<'de> Visitor<'de> for Focuses {
            type Value = LimitedFocusSubjects;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded list of RepoMap focus subjects")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<LimitedFocusSubject>()? {
                    if values.len() == MAX_REPOMAP_RANK_FOCUS_SUBJECTS {
                        return Err(de::Error::custom("too many RepoMap focus subjects"));
                    }
                    values
                        .try_reserve(1)
                        .map_err(|_error| de::Error::custom("focus allocation refused"))?;
                    values.push(value.0);
                }
                Ok(LimitedFocusSubjects(values))
            }
        }
        deserializer.deserialize_seq(Focuses)
    }
}

impl<'de> Deserialize<'de> for NativeRepoMapQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Query;
        impl<'de> Visitor<'de> for Query {
            type Value = NativeRepoMapQuery;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded native RepoMap rank query")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut query_text, mut focus_subjects, mut top_k, mut token_budget) =
                    (None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "query_text" if query_text.is_none() => {
                            query_text = Some(
                                map.next_value::<LimitedText<MAX_REPOMAP_RANK_QUERY_BYTES>>()?
                                    .0,
                            );
                        }
                        "focus_subjects" if focus_subjects.is_none() => {
                            focus_subjects = Some(map.next_value::<LimitedFocusSubjects>()?.0);
                        }
                        "top_k" if top_k.is_none() => top_k = Some(map.next_value()?),
                        "token_budget" if token_budget.is_none() => {
                            token_budget = Some(map.next_value()?);
                        }
                        "query_text" => return Err(de::Error::duplicate_field("query_text")),
                        "focus_subjects" => {
                            return Err(de::Error::duplicate_field("focus_subjects"));
                        }
                        "top_k" => return Err(de::Error::duplicate_field("top_k")),
                        "token_budget" => return Err(de::Error::duplicate_field("token_budget")),
                        _ => return Err(de::Error::unknown_field(&field, QUERY_FIELDS)),
                    }
                }
                let query = NativeRepoMapQuery {
                    query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
                    focus_subjects: focus_subjects
                        .ok_or_else(|| de::Error::missing_field("focus_subjects"))?,
                    top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
                    token_budget: token_budget
                        .ok_or_else(|| de::Error::missing_field("token_budget"))?,
                };
                if !query.valid_native_rank_query() {
                    return Err(de::Error::custom("invalid native RepoMap rank query"));
                }
                Ok(query)
            }
        }
        deserializer.deserialize_struct("NativeRepoMapQuery", QUERY_FIELDS, Query)
    }
}
const QUERY_FIELDS: &[&str] = &["query_text", "focus_subjects", "top_k", "token_budget"];

/// The selected rank facts. Legacy global snapshot metadata is intentionally
/// absent; its compatibility bridge is a separate endpoint decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapHit {
    pub subject: RepoMapNodeRef,
    pub owner_path: RepoRelativePath,
    pub final_score_millis: u32,
    pub rank: u32,
}
impl Serialize for NativeRepoMapHit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !valid_rank_subject(&self.subject)
            || self.owner_path.as_str().is_empty()
            || self.owner_path.as_str().len() > MAX_REPOMAP_RANK_PATH_BYTES
            || self.final_score_millis > 1_000
            || self.rank == 0
        {
            return Err(serde::ser::Error::custom("invalid native RepoMap rank hit"));
        }
        let mut fields = serializer.serialize_struct("NativeRepoMapHit", 4)?;
        fields.serialize_field("subject", &self.subject)?;
        fields.serialize_field("owner_path", &self.owner_path)?;
        fields.serialize_field("final_score_millis", &self.final_score_millis)?;
        fields.serialize_field("rank", &self.rank)?;
        fields.end()
    }
}
impl<'de> Deserialize<'de> for NativeRepoMapHit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Hit;
        impl<'de> Visitor<'de> for Hit {
            type Value = NativeRepoMapHit;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a typed native RepoMap rank hit")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut subject, mut owner_path, mut score, mut rank) = (None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "subject" if subject.is_none() => subject = Some(map.next_value()?),
                        "owner_path" if owner_path.is_none() => {
                            owner_path = Some(
                                map.next_value::<LimitedText<MAX_REPOMAP_RANK_PATH_BYTES>>()?
                                    .0,
                            );
                        }
                        "final_score_millis" if score.is_none() => score = Some(map.next_value()?),
                        "rank" if rank.is_none() => rank = Some(map.next_value()?),
                        "subject" => return Err(de::Error::duplicate_field("subject")),
                        "owner_path" => return Err(de::Error::duplicate_field("owner_path")),
                        "final_score_millis" => {
                            return Err(de::Error::duplicate_field("final_score_millis"));
                        }
                        "rank" => return Err(de::Error::duplicate_field("rank")),
                        _ => return Err(de::Error::unknown_field(&field, HIT_FIELDS)),
                    }
                }
                let hit = NativeRepoMapHit {
                    subject: subject.ok_or_else(|| de::Error::missing_field("subject"))?,
                    owner_path: RepoRelativePath::new(
                        owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
                    ),
                    final_score_millis: score
                        .ok_or_else(|| de::Error::missing_field("final_score_millis"))?,
                    rank: rank.ok_or_else(|| de::Error::missing_field("rank"))?,
                };
                if !valid_rank_subject(&hit.subject)
                    || hit.owner_path.as_str().is_empty()
                    || hit.final_score_millis > 1_000
                    || hit.rank == 0
                {
                    return Err(de::Error::custom("invalid native RepoMap rank hit"));
                }
                Ok(hit)
            }
        }
        deserializer.deserialize_struct("NativeRepoMapHit", HIT_FIELDS, Hit)
    }
}
const HIT_FIELDS: &[&str] = &["subject", "owner_path", "final_score_millis", "rank"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapResult {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub policy: Digest,
    pub hits: Vec<NativeRepoMapHit>,
    pub dropped_entries_count: u32,
    pub drop_reason_codes: Vec<String>,
    pub degraded_reason_codes: Vec<String>,
    /// Complete within the selected view's certified producer frontier.
    pub exhaustive: bool,
}

struct LimitedHits(Vec<NativeRepoMapHit>);
impl<'de> Deserialize<'de> for LimitedHits {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Hits;
        impl<'de> Visitor<'de> for Hits {
            type Value = LimitedHits;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded native RepoMap rank result")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<NativeRepoMapHit>()? {
                    if values.len() == MAX_REPOMAP_RANK_HITS {
                        return Err(de::Error::custom("too many RepoMap rank hits"));
                    }
                    values
                        .try_reserve(1)
                        .map_err(|_error| de::Error::custom("rank hit allocation refused"))?;
                    values.push(value);
                }
                Ok(LimitedHits(values))
            }
        }
        deserializer.deserialize_seq(Hits)
    }
}

struct LimitedReasons(Vec<String>);
impl<'de> Deserialize<'de> for LimitedReasons {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Reasons;
        impl<'de> Visitor<'de> for Reasons {
            type Value = LimitedReasons;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("bounded RepoMap reason codes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<LimitedText<MAX_REASON_BYTES>>()? {
                    if values.len() == MAX_REASON_CODES {
                        return Err(de::Error::custom("too many RepoMap reason codes"));
                    }
                    values
                        .try_reserve(1)
                        .map_err(|_error| de::Error::custom("reason allocation refused"))?;
                    values.push(value.0);
                }
                Ok(LimitedReasons(values))
            }
        }
        deserializer.deserialize_seq(Reasons)
    }
}

/// Borrowed canonical wire view; allocation custody remains with its owner.
#[derive(Clone, Copy)]
pub struct NativeRepoMapResultRef<'a> {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub policy: Digest,
    pub hits: &'a [NativeRepoMapHit],
    pub dropped_entries_count: u32,
    pub drop_reason_codes: &'a [String],
    pub degraded_reason_codes: &'a [String],
    pub exhaustive: bool,
}

impl<'a> From<&'a NativeRepoMapResult> for NativeRepoMapResultRef<'a> {
    fn from(result: &'a NativeRepoMapResult) -> Self {
        Self {
            view: result.view,
            instance: result.instance,
            policy: result.policy,
            hits: &result.hits,
            dropped_entries_count: result.dropped_entries_count,
            drop_reason_codes: &result.drop_reason_codes,
            degraded_reason_codes: &result.degraded_reason_codes,
            exhaustive: result.exhaustive,
        }
    }
}

impl Serialize for NativeRepoMapResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        NativeRepoMapResultRef::from(self).serialize(serializer)
    }
}

impl Serialize for NativeRepoMapResultRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.valid_native_rank_result() {
            return Err(serde::ser::Error::custom(
                "invalid native RepoMap rank result",
            ));
        }
        let mut fields = serializer.serialize_struct("NativeRepoMapResult", 8)?;
        fields.serialize_field("view", &self.view)?;
        fields.serialize_field("instance", &self.instance)?;
        fields.serialize_field("policy", &self.policy)?;
        fields.serialize_field("hits", self.hits)?;
        fields.serialize_field("dropped_entries_count", &self.dropped_entries_count)?;
        fields.serialize_field("drop_reason_codes", self.drop_reason_codes)?;
        fields.serialize_field("degraded_reason_codes", self.degraded_reason_codes)?;
        fields.serialize_field("exhaustive", &self.exhaustive)?;
        fields.end()
    }
}

impl NativeRepoMapResultRef<'_> {
    #[must_use]
    pub fn valid_native_rank_result(&self) -> bool {
        self.policy == native_repomap_rank_policy()
            && self.hits.len() <= MAX_REPOMAP_RANK_HITS
            && self.drop_reason_codes.iter().all(|reason| {
                matches!(
                    reason.as_str(),
                    "token_budget_exhausted" | "top_k_exhausted"
                )
            })
            && self.degraded_reason_codes.iter().all(|reason| {
                matches!(
                    reason.as_str(),
                    "query_terms_unmatched" | "token_budget_floor_applied"
                )
            })
            && self
                .drop_reason_codes
                .windows(2)
                .all(|pair| matches!(pair, [left, right] if left < right))
            && self
                .degraded_reason_codes
                .windows(2)
                .all(|pair| matches!(pair, [left, right] if left < right))
            && self.drop_reason_codes.len() <= MAX_REASON_CODES
            && self.degraded_reason_codes.len() <= MAX_REASON_CODES
            && self
                .drop_reason_codes
                .iter()
                .chain(self.degraded_reason_codes)
                .all(|reason| !reason.is_empty() && reason.len() <= MAX_REASON_BYTES)
            && self.hits.iter().enumerate().all(|(index, hit)| {
                u32::try_from(index)
                    .is_ok_and(|zero_based| zero_based.checked_add(1) == Some(hit.rank))
                    && valid_rank_subject(&hit.subject)
                    && hit.final_score_millis <= 1_000
                    && !hit.owner_path.as_str().is_empty()
                    && hit.owner_path.as_str().len() <= MAX_REPOMAP_RANK_PATH_BYTES
            })
    }
}

impl super::super::NativeIndexResponse {
    /// Serialize the canonical result without copying its retained hit buffers.
    pub fn serialize_repomap_ranked_ref<S: Serializer>(
        result: &NativeRepoMapResultRef<'_>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 30, "RepoMapRanked", result)
    }
}

impl<'de> Deserialize<'de> for NativeRepoMapResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ResultVisitor;
        impl<'de> Visitor<'de> for ResultVisitor {
            type Value = NativeRepoMapResult;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a view-bound native RepoMap rank result")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (
                    mut view,
                    mut instance,
                    mut policy,
                    mut hits,
                    mut dropped,
                    mut drop_reasons,
                    mut degraded_reasons,
                    mut exhaustive,
                ) = (None, None, None, None, None, None, None, None);
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "view" if view.is_none() => view = Some(map.next_value()?),
                        "instance" if instance.is_none() => instance = Some(map.next_value()?),
                        "policy" if policy.is_none() => policy = Some(map.next_value()?),
                        "hits" if hits.is_none() => hits = Some(map.next_value::<LimitedHits>()?.0),
                        "dropped_entries_count" if dropped.is_none() => {
                            dropped = Some(map.next_value()?);
                        }
                        "drop_reason_codes" if drop_reasons.is_none() => {
                            drop_reasons = Some(map.next_value::<LimitedReasons>()?.0);
                        }
                        "degraded_reason_codes" if degraded_reasons.is_none() => {
                            degraded_reasons = Some(map.next_value::<LimitedReasons>()?.0);
                        }
                        "exhaustive" if exhaustive.is_none() => {
                            exhaustive = Some(map.next_value()?);
                        }
                        "view" => return Err(de::Error::duplicate_field("view")),
                        "instance" => return Err(de::Error::duplicate_field("instance")),
                        "policy" => return Err(de::Error::duplicate_field("policy")),
                        "hits" => return Err(de::Error::duplicate_field("hits")),
                        "dropped_entries_count" => {
                            return Err(de::Error::duplicate_field("dropped_entries_count"));
                        }
                        "drop_reason_codes" => {
                            return Err(de::Error::duplicate_field("drop_reason_codes"));
                        }
                        "degraded_reason_codes" => {
                            return Err(de::Error::duplicate_field("degraded_reason_codes"));
                        }
                        "exhaustive" => return Err(de::Error::duplicate_field("exhaustive")),
                        _ => return Err(de::Error::unknown_field(&field, RESULT_FIELDS)),
                    }
                }
                let result = NativeRepoMapResult {
                    view: view.ok_or_else(|| de::Error::missing_field("view"))?,
                    instance: instance.ok_or_else(|| de::Error::missing_field("instance"))?,
                    policy: policy.ok_or_else(|| de::Error::missing_field("policy"))?,
                    hits: hits.ok_or_else(|| de::Error::missing_field("hits"))?,
                    dropped_entries_count: dropped
                        .ok_or_else(|| de::Error::missing_field("dropped_entries_count"))?,
                    drop_reason_codes: drop_reasons
                        .ok_or_else(|| de::Error::missing_field("drop_reason_codes"))?,
                    degraded_reason_codes: degraded_reasons
                        .ok_or_else(|| de::Error::missing_field("degraded_reason_codes"))?,
                    exhaustive: exhaustive.ok_or_else(|| de::Error::missing_field("exhaustive"))?,
                };
                if !NativeRepoMapResultRef::from(&result).valid_native_rank_result() {
                    return Err(de::Error::custom("invalid native RepoMap rank result"));
                }
                Ok(result)
            }
        }
        deserializer.deserialize_struct("NativeRepoMapResult", RESULT_FIELDS, ResultVisitor)
    }
}
const RESULT_FIELDS: &[&str] = &[
    "view",
    "instance",
    "policy",
    "hits",
    "dropped_entries_count",
    "drop_reason_codes",
    "degraded_reason_codes",
    "exhaustive",
];

#[cfg(test)]
mod tests;
