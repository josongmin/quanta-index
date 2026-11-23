//! One certified projection row and the view-bound `Native` `RepoMap` rank wire.
//!
//! The producer stores the existing compiled projection entry as an immutable
//! leaf. The graph output and supporter refer to that leaf; neither adjacency
//! nor this query wire carries an invented legacy snapshot envelope.

use crate::{
    CompiledRepoMapProjectionEntryV1, RepoMapDocType, RepoMapNodeRef, RepoRelativePath,
    index::{Digest, ObjectRef},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};
use std::fmt;

/// The existing compiler permits one MiB of preview text per projection row.
pub const MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES: usize = 1 << 20;
pub const MAX_REPOMAP_RANK_PATH_BYTES: usize = 4 << 10;
pub const MAX_REPOMAP_RANK_SUBJECT_BYTES: usize = 2 << 10;
/// JSON can escape each UTF-8 byte of a one-MiB search preview. This caps
/// the persisted encoded leaf; the decoded search text retains its one-MiB cap.
pub const MAX_REPOMAP_RANK_PROJECTION_LEAF_BYTES: usize = 8 << 20;

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

/// Checks decoded length before retaining UTF-8 text.
///
/// Borrowed UTF-8 is checked before copying; owned text is validated before
/// adoption. The containing object/request separately caps its input buffer.
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
        deserializer.deserialize_string(Text::<MAX>)
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

#[cfg(test)]
mod retired_rank_wire_fixture;

#[cfg(test)]
pub(crate) use retired_rank_wire_fixture::{
    MAX_REPOMAP_RANK_QUERY_BYTES, NativeRepoMapHit, NativeRepoMapQuery, NativeRepoMapResult,
    NativeRepoMapResultRef,
};

#[cfg(test)]
mod tests;
