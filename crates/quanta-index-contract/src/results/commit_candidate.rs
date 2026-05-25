use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::CommitSha;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitCandidate {
    pub sha: CommitSha,
    pub parent_ids: Vec<CommitSha>,
    pub committed_at_unix_s: i64,
    pub author: String,
    pub committer: String,
    pub message: String,
    pub is_merge: bool,
    pub tags: Vec<String>,
}

const COMMIT_CANDIDATE_FIELDS: &[&str] = &[
    "sha",
    "parent_ids",
    "committed_at_unix_s",
    "author",
    "committer",
    "message",
    "is_merge",
    "tags",
];

impl Serialize for CommitCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CommitCandidate", 8)?;
        state.serialize_field("sha", &self.sha)?;
        state.serialize_field("parent_ids", &self.parent_ids)?;
        state.serialize_field("committed_at_unix_s", &self.committed_at_unix_s)?;
        state.serialize_field("author", &self.author)?;
        state.serialize_field("committer", &self.committer)?;
        state.serialize_field("message", &self.message)?;
        state.serialize_field("is_merge", &self.is_merge)?;
        state.serialize_field("tags", &self.tags)?;
        state.end()
    }
}

struct CommitCandidateVisitor;

impl<'de> Visitor<'de> for CommitCandidateVisitor {
    type Value = CommitCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CommitCandidate map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut sha: Option<CommitSha> = None;
        let mut parent_ids: Option<Vec<CommitSha>> = None;
        let mut committed_at_unix_s: Option<i64> = None;
        let mut author: Option<String> = None;
        let mut committer: Option<String> = None;
        let mut message: Option<String> = None;
        let mut is_merge: Option<bool> = None;
        let mut tags: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "sha" => sha = Some(map.next_value()?),
                "parent_ids" => parent_ids = Some(map.next_value()?),
                "committed_at_unix_s" => committed_at_unix_s = Some(map.next_value()?),
                "author" => author = Some(map.next_value()?),
                "committer" => committer = Some(map.next_value()?),
                "message" => message = Some(map.next_value()?),
                "is_merge" => is_merge = Some(map.next_value()?),
                "tags" => tags = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, COMMIT_CANDIDATE_FIELDS)),
            }
        }
        Ok(CommitCandidate {
            sha: sha.ok_or_else(|| de::Error::missing_field("sha"))?,
            parent_ids: parent_ids.ok_or_else(|| de::Error::missing_field("parent_ids"))?,
            committed_at_unix_s: committed_at_unix_s
                .ok_or_else(|| de::Error::missing_field("committed_at_unix_s"))?,
            author: author.ok_or_else(|| de::Error::missing_field("author"))?,
            committer: committer.ok_or_else(|| de::Error::missing_field("committer"))?,
            message: message.ok_or_else(|| de::Error::missing_field("message"))?,
            is_merge: is_merge.ok_or_else(|| de::Error::missing_field("is_merge"))?,
            tags: tags.ok_or_else(|| de::Error::missing_field("tags"))?,
        })
    }
}

impl<'de> Deserialize<'de> for CommitCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "CommitCandidate",
            COMMIT_CANDIDATE_FIELDS,
            CommitCandidateVisitor,
        )
    }
}
