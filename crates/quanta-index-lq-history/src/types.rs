//! Stable shared types used across the LEX-07 history engine.
//!
//! [`CommitSha`] is a `[u8; 20]` newtype displayed as 40-hex. [`AppliedAtMs`]
//! and [`ManifestGeneration`] are `u64` newtypes that keep the write-packet
//! time anchor and the manifest linearization point typewise distinct from
//! ad-hoc integers.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{HistoryError, HistoryErrorCode};

/// Default `parent:` walk depth cap per LEX-07 § 9.
pub const HISTORY_PARENT_DEPTH_MAX_DEFAULT: u32 = 64;

/// Default `revisions:<range>` capacity cap per LEX-07 § 9.
pub const HISTORY_REVISIONS_MAX_DEFAULT: u32 = 10_000;

/// Tag-pattern regex NFA cap mirroring `dsl.md` § 3.4.
pub const HISTORY_TAG_REGEX_NFA_MAX: u32 = 100_000;

/// 20-byte commit SHA. Displayed as 40-hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitSha([u8; 20]);

impl CommitSha {
    /// Construct from a 20-byte buffer.
    #[must_use]
    pub const fn from_bytes(b: [u8; 20]) -> Self {
        Self(b)
    }

    /// Borrow the raw 20 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Parse a 40-character lowercase-hex string.
    pub fn parse_hex(s: &str) -> Result<Self, HistoryError> {
        if s.len() != 40 {
            return Err(HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                format!("commit sha must be 40 hex characters, got {}", s.len()),
            ));
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 20];
        let mut i: usize = 0;
        while i < 20 {
            let hi_idx = i.saturating_mul(2);
            let lo_idx = hi_idx.saturating_add(1);
            let hi = bytes.get(hi_idx).copied().ok_or_else(|| {
                HistoryError::new(HistoryErrorCode::HistoryRefNotFound, "sha hex out of range")
            })?;
            let lo = bytes.get(lo_idx).copied().ok_or_else(|| {
                HistoryError::new(HistoryErrorCode::HistoryRefNotFound, "sha hex out of range")
            })?;
            let h = hex_nibble(hi)?;
            let l = hex_nibble(lo)?;
            let slot = out.get_mut(i).ok_or_else(|| {
                HistoryError::new(HistoryErrorCode::HistoryRefNotFound, "sha buf out of range")
            })?;
            *slot = (h << 4) | l;
            i = i.saturating_add(1);
        }
        Ok(Self(out))
    }
}

fn hex_nibble(b: u8) -> Result<u8, HistoryError> {
    match b {
        b'0'..=b'9' => Ok(b.saturating_sub(b'0')),
        b'a'..=b'f' => Ok(b.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => Ok(b.saturating_sub(b'A').saturating_add(10)),
        _ => Err(HistoryError::new(
            HistoryErrorCode::HistoryRefNotFound,
            format!("invalid hex byte 0x{b:02x}"),
        )),
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl serde::Serialize for CommitSha {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for CommitSha {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = CommitSha;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("CommitSha 20-byte buffer")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<CommitSha, E> {
                if v.len() != 20 {
                    return Err(E::custom(format!(
                        "CommitSha must be 20 bytes, got {}",
                        v.len()
                    )));
                }
                let mut out = [0u8; 20];
                for (i, b) in v.iter().enumerate() {
                    let slot = out
                        .get_mut(i)
                        .ok_or_else(|| E::custom("sha out of range"))?;
                    *slot = *b;
                }
                Ok(CommitSha(out))
            }
            fn visit_borrowed_bytes<E: serde::de::Error>(
                self,
                v: &'d [u8],
            ) -> Result<CommitSha, E> {
                self.visit_bytes(v)
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<CommitSha, E> {
                self.visit_bytes(v.as_slice())
            }
            fn visit_seq<A: serde::de::SeqAccess<'d>>(
                self,
                mut seq: A,
            ) -> Result<CommitSha, A::Error> {
                let mut out = [0u8; 20];
                let mut i: usize = 0;
                while let Some(b) = seq.next_element::<u8>()? {
                    let slot = out
                        .get_mut(i)
                        .ok_or_else(|| serde::de::Error::custom("sha out of range"))?;
                    *slot = b;
                    i = i.saturating_add(1);
                }
                if i != 20 {
                    return Err(serde::de::Error::custom(format!(
                        "CommitSha must be 20 bytes, got {i}"
                    )));
                }
                Ok(CommitSha(out))
            }
        }
        de.deserialize_bytes(V)
    }
}

/// Repo-relative path newtype.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoRelativePath(Box<str>);

impl RepoRelativePath {
    /// Wrap a path string.
    #[must_use]
    pub fn new(s: impl Into<Box<str>>) -> Self {
        Self(s.into())
    }

    /// Borrow as `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for RepoRelativePath {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RepoRelativePath {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = RepoRelativePath;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("RepoRelativePath string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<RepoRelativePath, E> {
                Ok(RepoRelativePath(Box::from(v)))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<RepoRelativePath, E> {
                Ok(RepoRelativePath(v.into_boxed_str()))
            }
        }
        de.deserialize_str(V)
    }
}

/// Write-packet time anchor (NEVER `now()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppliedAtMs(u64);

impl AppliedAtMs {
    /// Wrap an unsigned millisecond value.
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the raw millisecond value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AppliedAtMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for AppliedAtMs {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for AppliedAtMs {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = AppliedAtMs;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("AppliedAtMs unsigned 64-bit integer")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<AppliedAtMs, E> {
                Ok(AppliedAtMs(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<AppliedAtMs, E> {
                Ok(AppliedAtMs(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<AppliedAtMs, E> {
                if v < 0 {
                    return Err(E::custom("AppliedAtMs must be non-negative"));
                }
                let u = u64::try_from(v)
                    .map_err(|err| E::custom(format!("AppliedAtMs out of u64 range: {err}")))?;
                Ok(AppliedAtMs(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Manifest generation newtype. Strictly monotonic per `(repo, rev)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManifestGeneration(u64);

impl ManifestGeneration {
    /// Wrap a non-zero generation. Returns `INVALID_GENERATION` on `0`.
    pub fn new(v: u64) -> Result<Self, HistoryError> {
        if v == 0 {
            return Err(HistoryError::new(
                HistoryErrorCode::InvalidGeneration,
                "manifest generation must be non-zero",
            ));
        }
        Ok(Self(v))
    }

    /// Construct without the non-zero check. Used by deserializers that
    /// surface their own typed error.
    #[must_use]
    pub const fn from_raw(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the wrapped value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ManifestGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for ManifestGeneration {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ManifestGeneration {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = ManifestGeneration;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ManifestGeneration unsigned 64-bit integer")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ManifestGeneration, E> {
                Ok(ManifestGeneration(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<ManifestGeneration, E> {
                Ok(ManifestGeneration(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ManifestGeneration, E> {
                if v < 0 {
                    return Err(E::custom("ManifestGeneration must be non-negative"));
                }
                let u = u64::try_from(v).map_err(|err| {
                    E::custom(format!("ManifestGeneration out of u64 range: {err}"))
                })?;
                Ok(ManifestGeneration(u))
            }
        }
        de.deserialize_u64(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppliedAtMs, CommitSha, HISTORY_PARENT_DEPTH_MAX_DEFAULT, HISTORY_REVISIONS_MAX_DEFAULT,
        HISTORY_TAG_REGEX_NFA_MAX, ManifestGeneration, RepoRelativePath,
    };
    use crate::errors::HistoryErrorCode;

    #[test]
    fn caps_match_spec() {
        assert_eq!(HISTORY_PARENT_DEPTH_MAX_DEFAULT, 64);
        assert_eq!(HISTORY_REVISIONS_MAX_DEFAULT, 10_000);
        assert_eq!(HISTORY_TAG_REGEX_NFA_MAX, 100_000);
    }

    #[test]
    fn commit_sha_parse_roundtrip() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let sha = match CommitSha::parse_hex(hex) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(format!("{sha}"), hex);
    }

    #[test]
    fn commit_sha_rejects_wrong_length() {
        match CommitSha::parse_hex("abc") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn commit_sha_rejects_invalid_hex() {
        let bad = "g".repeat(40);
        match CommitSha::parse_hex(&bad) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn commit_sha_serde_roundtrip() {
        let sha = CommitSha::from_bytes([7u8; 20]);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&sha, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<CommitSha, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, sha),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn applied_at_ms_get() {
        assert_eq!(AppliedAtMs::new(42).get(), 42);
    }

    #[test]
    fn manifest_generation_zero_rejected() {
        match ManifestGeneration::new(0) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::InvalidGeneration),
        }
    }

    #[test]
    fn manifest_generation_nonzero_accepted() {
        match ManifestGeneration::new(1) {
            Ok(g) => assert_eq!(g.get(), 1),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn repo_relative_path_roundtrip() {
        let p = RepoRelativePath::new("src/lib.rs");
        assert_eq!(p.as_str(), "src/lib.rs");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&p, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<RepoRelativePath, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, p),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
