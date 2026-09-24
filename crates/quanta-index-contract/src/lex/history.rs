//! `CommitSha` newtype + `CommitRecord`.
//!
//! Wire shape: [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! §3.1.1.
//!
//! `CommitSha` is a 20-byte git SHA-1 with a fixed 40-hex-char canonical
//! display. Construction is fallible via [`CommitSha::from_hex`]; on the wire
//! it serializes as the lower-hex 40-character string (round-trip stable).

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

const COMMIT_SHA_HEX_LEN: usize = 40;
const COMMIT_SHA_BYTE_LEN: usize = 20;

/// Hex-parse failure for [`CommitSha::from_hex`].
///
/// Plain enum (no `thiserror`) to keep the contract crate's dep graph minimal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitShaParseError {
    /// Length was not exactly 40 chars.
    BadLength { observed: usize },
    /// Non-hex character at the given position.
    NonHexChar { position: usize },
}

impl fmt::Display for CommitShaParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadLength { observed } => {
                write!(
                    formatter,
                    "CommitSha hex must be exactly 40 chars; observed {observed}",
                )
            }
            Self::NonHexChar { position } => {
                write!(
                    formatter,
                    "CommitSha hex has non-hex char at position {position}"
                )
            }
        }
    }
}

impl core::error::Error for CommitShaParseError {}

/// Git SHA-1 commit identity. 20 raw bytes; canonical wire form is the
/// 40-char lower-hex string.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitSha([u8; COMMIT_SHA_BYTE_LEN]);

impl CommitSha {
    /// Zero SHA — convenience for tests / sentinel use only; not a valid
    /// producer-emitted commit identity.
    pub const ZERO: Self = Self([0u8; COMMIT_SHA_BYTE_LEN]);

    /// Construct from raw 20 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; COMMIT_SHA_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    /// Borrow the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; COMMIT_SHA_BYTE_LEN] {
        &self.0
    }

    /// Parse a 40-char lower-or-upper-hex string. No silent truncation, no
    /// alternate form; failure surfaces a typed [`CommitShaParseError`].
    pub fn from_hex(value: &str) -> Result<Self, CommitShaParseError> {
        if value.len() != COMMIT_SHA_HEX_LEN {
            return Err(CommitShaParseError::BadLength {
                observed: value.len(),
            });
        }
        let mut bytes = [0u8; COMMIT_SHA_BYTE_LEN];
        let raw = value.as_bytes();
        let mut idx = 0usize;
        while idx < COMMIT_SHA_BYTE_LEN {
            let Some(hi_idx) = idx.checked_mul(2) else {
                return Err(CommitShaParseError::BadLength {
                    observed: value.len(),
                });
            };
            let Some(lo_idx) = hi_idx.checked_add(1) else {
                return Err(CommitShaParseError::BadLength {
                    observed: value.len(),
                });
            };
            // SAFETY-equivalent comment: indexing is bounded by the explicit
            // length check above; we surface a typed error rather than slicing.
            let Some(&hi) = raw.get(hi_idx) else {
                return Err(CommitShaParseError::BadLength {
                    observed: value.len(),
                });
            };
            let Some(&lo) = raw.get(lo_idx) else {
                return Err(CommitShaParseError::BadLength {
                    observed: value.len(),
                });
            };
            let Some(hi_v) = decode_nibble(hi) else {
                return Err(CommitShaParseError::NonHexChar { position: hi_idx });
            };
            let Some(lo_v) = decode_nibble(lo) else {
                return Err(CommitShaParseError::NonHexChar { position: lo_idx });
            };
            let Some(byte_ref) = bytes.get_mut(idx) else {
                return Err(CommitShaParseError::BadLength {
                    observed: value.len(),
                });
            };
            // `hi_v << 4` cannot overflow: `hi_v < 16`, shift is 4, result
            // fits in u8.
            *byte_ref = (hi_v << 4) | lo_v;
            idx = idx.saturating_add(1);
        }
        Ok(Self(bytes))
    }

    /// Render as 40 lower-hex chars.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(COMMIT_SHA_HEX_LEN);
        for byte in self.0 {
            // `byte >> 4` masks to `< 16`; `byte & 0x0f` masks to `< 16`.
            // Both indices are guaranteed in-range for the HEX table.
            let hi = usize::from(byte >> 4);
            let lo = usize::from(byte & 0x0f);
            if let Some(&hi_c) = HEX_LOWER.get(hi) {
                out.push(char::from(hi_c));
            }
            if let Some(&lo_c) = HEX_LOWER.get(lo) {
                out.push(char::from(lo_c));
            }
        }
        out
    }
}

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte.saturating_sub(b'0')),
        b'a'..=b'f' => Some(byte.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => Some(byte.saturating_sub(b'A').saturating_add(10)),
        _ => None,
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.to_hex().as_str())
    }
}

impl Serialize for CommitSha {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_hex().as_str())
    }
}

struct CommitShaVisitor;

impl Visitor<'_> for CommitShaVisitor {
    type Value = CommitSha;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a 40-char hex CommitSha string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        CommitSha::from_hex(value).map_err(|parse_err| de::Error::custom(parse_err.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for CommitSha {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(CommitShaVisitor)
    }
}

/// Producer-authored commit record per
/// [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
/// §3.1.1.
///
/// `parents` carries the full DAG view at integration time; empty for root
/// commits. `is_merge` is producer-set and is **not** recomputed by the search
/// side (the producer is the authority; per `CLAUDE.md` we do not invent a
/// heuristic when the authority is present).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitRecord {
    pub wire_version: u32,
    pub sha: CommitSha,
    pub parents: Vec<CommitSha>,
    pub author_time_ms: u64,
    pub committer_time_ms: u64,
    pub applied_at_ms: u64,
    pub author: Box<str>,
    pub author_name: Option<Box<str>>,
    pub author_email: Option<Box<str>>,
    pub committer: Box<str>,
    pub committer_name: Option<Box<str>>,
    pub committer_email: Option<Box<str>>,
    pub message: Box<str>,
    pub is_merge: bool,
    pub tags: Vec<Box<str>>,
}

const COMMIT_RECORD_FIELDS: &[&str] = &[
    "wire_version",
    "sha",
    "parents",
    "author_time_ms",
    "committer_time_ms",
    "applied_at_ms",
    "author",
    "author_name",
    "author_email",
    "committer",
    "committer_name",
    "committer_email",
    "message",
    "is_merge",
    "tags",
];

struct TagsSer<'a> {
    items: &'a [Box<str>],
}

impl Serialize for TagsSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut state = serializer.serialize_seq(Some(self.items.len()))?;
        for tag in self.items {
            state.serialize_element(tag.as_ref())?;
        }
        state.end()
    }
}

impl Serialize for CommitRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CommitRecord", 15)?;
        state.serialize_field("wire_version", &self.wire_version)?;
        state.serialize_field("sha", &self.sha)?;
        state.serialize_field("parents", &self.parents)?;
        state.serialize_field("author_time_ms", &self.author_time_ms)?;
        state.serialize_field("committer_time_ms", &self.committer_time_ms)?;
        state.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        state.serialize_field("author", self.author.as_ref())?;
        state.serialize_field("author_name", &self.author_name)?;
        state.serialize_field("author_email", &self.author_email)?;
        state.serialize_field("committer", self.committer.as_ref())?;
        state.serialize_field("committer_name", &self.committer_name)?;
        state.serialize_field("committer_email", &self.committer_email)?;
        state.serialize_field("message", self.message.as_ref())?;
        state.serialize_field("is_merge", &self.is_merge)?;
        state.serialize_field("tags", &TagsSer { items: &self.tags })?;
        state.end()
    }
}

struct CommitRecordVisitor;

impl<'de> Visitor<'de> for CommitRecordVisitor {
    type Value = CommitRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CommitRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut wire_version: Option<u32> = None;
        let mut sha: Option<CommitSha> = None;
        let mut parents: Option<Vec<CommitSha>> = None;
        let mut author_time_ms: Option<u64> = None;
        let mut committer_time_ms: Option<u64> = None;
        let mut applied_at_ms: Option<u64> = None;
        let mut author: Option<String> = None;
        let mut author_name: Option<Option<String>> = None;
        let mut author_email: Option<Option<String>> = None;
        let mut committer: Option<String> = None;
        let mut committer_name: Option<Option<String>> = None;
        let mut committer_email: Option<Option<String>> = None;
        let mut message: Option<String> = None;
        let mut is_merge: Option<bool> = None;
        let mut tags: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "wire_version" => {
                    if wire_version.is_some() {
                        return Err(de::Error::duplicate_field("wire_version"));
                    }
                    wire_version = Some(map.next_value()?);
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(de::Error::duplicate_field("sha"));
                    }
                    sha = Some(map.next_value()?);
                }
                "parents" => {
                    if parents.is_some() {
                        return Err(de::Error::duplicate_field("parents"));
                    }
                    parents = Some(map.next_value()?);
                }
                "author_time_ms" => {
                    if author_time_ms.is_some() {
                        return Err(de::Error::duplicate_field("author_time_ms"));
                    }
                    author_time_ms = Some(map.next_value()?);
                }
                "committer_time_ms" => {
                    if committer_time_ms.is_some() {
                        return Err(de::Error::duplicate_field("committer_time_ms"));
                    }
                    committer_time_ms = Some(map.next_value()?);
                }
                "applied_at_ms" => {
                    if applied_at_ms.is_some() {
                        return Err(de::Error::duplicate_field("applied_at_ms"));
                    }
                    applied_at_ms = Some(map.next_value()?);
                }
                "author" => {
                    if author.is_some() {
                        return Err(de::Error::duplicate_field("author"));
                    }
                    author = Some(map.next_value()?);
                }
                "author_name" => {
                    if author_name.is_some() {
                        return Err(de::Error::duplicate_field("author_name"));
                    }
                    author_name = Some(map.next_value()?);
                }
                "author_email" => {
                    if author_email.is_some() {
                        return Err(de::Error::duplicate_field("author_email"));
                    }
                    author_email = Some(map.next_value()?);
                }
                "committer" => {
                    if committer.is_some() {
                        return Err(de::Error::duplicate_field("committer"));
                    }
                    committer = Some(map.next_value()?);
                }
                "committer_name" => {
                    if committer_name.is_some() {
                        return Err(de::Error::duplicate_field("committer_name"));
                    }
                    committer_name = Some(map.next_value()?);
                }
                "committer_email" => {
                    if committer_email.is_some() {
                        return Err(de::Error::duplicate_field("committer_email"));
                    }
                    committer_email = Some(map.next_value()?);
                }
                "message" => {
                    if message.is_some() {
                        return Err(de::Error::duplicate_field("message"));
                    }
                    message = Some(map.next_value()?);
                }
                "is_merge" => {
                    if is_merge.is_some() {
                        return Err(de::Error::duplicate_field("is_merge"));
                    }
                    is_merge = Some(map.next_value()?);
                }
                "tags" => {
                    if tags.is_some() {
                        return Err(de::Error::duplicate_field("tags"));
                    }
                    tags = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, COMMIT_RECORD_FIELDS)),
            }
        }
        let wire_version = wire_version.ok_or_else(|| de::Error::missing_field("wire_version"))?;
        let sha = sha.ok_or_else(|| de::Error::missing_field("sha"))?;
        let parents = parents.ok_or_else(|| de::Error::missing_field("parents"))?;
        let author_time_ms =
            author_time_ms.ok_or_else(|| de::Error::missing_field("author_time_ms"))?;
        let committer_time_ms =
            committer_time_ms.ok_or_else(|| de::Error::missing_field("committer_time_ms"))?;
        let applied_at_ms =
            applied_at_ms.ok_or_else(|| de::Error::missing_field("applied_at_ms"))?;
        let author = author.ok_or_else(|| de::Error::missing_field("author"))?;
        let committer = committer.ok_or_else(|| de::Error::missing_field("committer"))?;
        let message = message.ok_or_else(|| de::Error::missing_field("message"))?;
        let is_merge = is_merge.ok_or_else(|| de::Error::missing_field("is_merge"))?;
        let tags = tags.ok_or_else(|| de::Error::missing_field("tags"))?;
        Ok(CommitRecord {
            wire_version,
            sha,
            parents,
            author_time_ms,
            committer_time_ms,
            applied_at_ms,
            author: author.into_boxed_str(),
            author_name: author_name
                .ok_or_else(|| de::Error::missing_field("author_name"))?
                .map(String::into_boxed_str),
            author_email: author_email
                .ok_or_else(|| de::Error::missing_field("author_email"))?
                .map(String::into_boxed_str),
            committer: committer.into_boxed_str(),
            committer_name: committer_name
                .ok_or_else(|| de::Error::missing_field("committer_name"))?
                .map(String::into_boxed_str),
            committer_email: committer_email
                .ok_or_else(|| de::Error::missing_field("committer_email"))?
                .map(String::into_boxed_str),
            message: message.into_boxed_str(),
            is_merge,
            tags: tags.into_iter().map(String::into_boxed_str).collect(),
        })
    }
}

impl<'de> Deserialize<'de> for CommitRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("CommitRecord", COMMIT_RECORD_FIELDS, CommitRecordVisitor)
    }
}
