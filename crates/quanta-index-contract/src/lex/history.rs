//! `CommitSha` newtype + `CommitRecord`.
//!
//! This file defines the commit wire shape;
//! `docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md` owns
//! ingress boundaries.
//!
//! `CommitSha` is a 20-byte git SHA-1 with a fixed 40-hex-char canonical
//! display. Construction is fallible via [`CommitSha::from_hex`]; on the wire
//! it serializes as the lower-hex 40-character string (round-trip stable).

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::{Error as _, SerializeStruct},
};

mod admitted;
use admitted::{CommitDecodePolicy, Ordinary};

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
        for byte in self.lower_hex_bytes() {
            out.push(char::from(byte));
        }
        out
    }

    fn lower_hex_bytes(&self) -> [u8; COMMIT_SHA_HEX_LEN] {
        let mut hex = [0u8; COMMIT_SHA_HEX_LEN];
        for (pair, byte) in hex.chunks_exact_mut(2).zip(self.0.iter().copied()) {
            for (digit, nibble) in pair.iter_mut().zip([byte >> 4, byte & 0x0f]) {
                *digit = encode_nibble(nibble);
            }
        }
        hex
    }
}

fn encode_nibble(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0'.saturating_add(nibble)
    } else {
        b'a'.saturating_add(nibble.saturating_sub(10))
    }
}

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
        let hex = self.lower_hex_bytes();
        let text = core::str::from_utf8(&hex).map_err(|_error| fmt::Error)?;
        formatter.write_str(text)
    }
}

impl Serialize for CommitSha {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex = self.lower_hex_bytes();
        let text = core::str::from_utf8(&hex)
            .map_err(|_error| S::Error::custom("CommitSha hex formatter produced non-UTF-8"))?;
        serializer.serialize_str(text)
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

/// Producer-authored commit record under the ingress ownership decision in
/// `docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md`.
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

struct CommitRecordVisitor<P>(P);

impl<'de, P: CommitDecodePolicy<'de>> Visitor<'de> for CommitRecordVisitor<P> {
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
        let mut author: Option<Box<str>> = None;
        let mut author_name: Option<Option<Box<str>>> = None;
        let mut author_email: Option<Option<Box<str>>> = None;
        let mut committer: Option<Box<str>> = None;
        let mut committer_name: Option<Option<Box<str>>> = None;
        let mut committer_email: Option<Option<Box<str>>> = None;
        let mut message: Option<Box<str>> = None;
        let mut is_merge: Option<bool> = None;
        let mut tags: Option<Vec<Box<str>>> = None;
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
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
                    sha = Some(self.0.next_sha(&mut map)?);
                }
                "parents" => {
                    if parents.is_some() {
                        return Err(de::Error::duplicate_field("parents"));
                    }
                    parents = Some(self.0.next_parents(&mut map)?);
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
                    author = Some(self.0.next_text(&mut map)?);
                }
                "author_name" => {
                    if author_name.is_some() {
                        return Err(de::Error::duplicate_field("author_name"));
                    }
                    author_name = Some(self.0.next_optional_text(&mut map)?);
                }
                "author_email" => {
                    if author_email.is_some() {
                        return Err(de::Error::duplicate_field("author_email"));
                    }
                    author_email = Some(self.0.next_optional_text(&mut map)?);
                }
                "committer" => {
                    if committer.is_some() {
                        return Err(de::Error::duplicate_field("committer"));
                    }
                    committer = Some(self.0.next_text(&mut map)?);
                }
                "committer_name" => {
                    if committer_name.is_some() {
                        return Err(de::Error::duplicate_field("committer_name"));
                    }
                    committer_name = Some(self.0.next_optional_text(&mut map)?);
                }
                "committer_email" => {
                    if committer_email.is_some() {
                        return Err(de::Error::duplicate_field("committer_email"));
                    }
                    committer_email = Some(self.0.next_optional_text(&mut map)?);
                }
                "message" => {
                    if message.is_some() {
                        return Err(de::Error::duplicate_field("message"));
                    }
                    message = Some(self.0.next_text(&mut map)?);
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
                    tags = Some(self.0.next_tags(&mut map)?);
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
            author,
            author_name: author_name.ok_or_else(|| de::Error::missing_field("author_name"))?,
            author_email: author_email.ok_or_else(|| de::Error::missing_field("author_email"))?,
            committer,
            committer_name: committer_name
                .ok_or_else(|| de::Error::missing_field("committer_name"))?,
            committer_email: committer_email
                .ok_or_else(|| de::Error::missing_field("committer_email"))?,
            message,
            is_merge,
            tags,
        })
    }
}

impl<'de> Deserialize<'de> for CommitRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "CommitRecord",
            COMMIT_RECORD_FIELDS,
            CommitRecordVisitor(Ordinary),
        )
    }
}

#[cfg(test)]
mod fixed_sha_wire_tests {
    use super::CommitSha;

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed contract gold and typed refusal assertions are independent test oracles"
    )]
    fn fixed_sha_json_and_cbor_are_exact_lower_hex() -> Result<(), Box<dyn std::error::Error>> {
        const LOWER: &str = "abababababababababababababababababababab";
        let sha = CommitSha::from_bytes([0xab; 20]);
        assert_eq!(sha.to_hex(), LOWER);
        assert_eq!(format!("{sha}"), LOWER);
        assert_eq!(serde_json::to_string(&sha)?, format!("\"{LOWER}\""));

        let mut encoded = Vec::new();
        ciborium::into_writer(&sha, &mut encoded)?;
        let mut expected = vec![0x78, 0x28]; // definite CBOR text length 40
        expected.extend_from_slice(LOWER.as_bytes());
        assert_eq!(encoded, expected);
        Ok(())
    }
}
