//! Stable shared types for the RT-01 runtime metadata + `dirty:` channel.
//!
//! Every wire shape is hand-rolled serde per D18 — no proc-macro
//! `derive(Serialize)` / `derive(Deserialize)` is used in this crate.
//! Newtype layout keeps the per-tenant per-repo buffer scope typewise
//! distinct from raw integers and strings.
//!
//! Time is caller-supplied via [`ApplyTimeMs`]. The runtime crate never reads
//! the wall clock; TTL sweeps anchor against the `now_ms` value passed by the
//! caller (write-coordinator / sweeper). This keeps tests deterministic and
//! lets the IPC layer be the authoritative time source.

use core::fmt;

use crate::errors::{RuntimeError, RuntimeErrorCode};

/// Default per-(tenant, repo) dirty-buffer capacity per RT-01 § 4.6.
pub const DEFAULT_BUFFER_CAPACITY: u32 = 10_000;

/// Default per-entry TTL per RT-01 § 4.6 (300 seconds, in milliseconds).
pub const DEFAULT_TTL_MS: u64 = 300_000;

/// Tenant identifier newtype. Non-empty.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantId(Box<str>);

impl TenantId {
    /// Construct a tenant identifier; rejects the empty string with
    /// [`RuntimeErrorCode::DirtyBadIdentity`].
    pub fn new(s: impl Into<Box<str>>) -> Result<Self, RuntimeError> {
        let v: Box<str> = s.into();
        if v.is_empty() {
            return Err(RuntimeError::new(
                RuntimeErrorCode::DirtyBadIdentity,
                "tenant_id must be non-empty",
            ));
        }
        Ok(Self(v))
    }

    /// Borrow as `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for TenantId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for TenantId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = TenantId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("non-empty TenantId string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<TenantId, E> {
                if v.is_empty() {
                    return Err(E::custom("tenant_id must be non-empty"));
                }
                Ok(TenantId(Box::from(v)))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<TenantId, E> {
                if v.is_empty() {
                    return Err(E::custom("tenant_id must be non-empty"));
                }
                Ok(TenantId(v.into_boxed_str()))
            }
        }
        de.deserialize_str(V)
    }
}

/// Repository identifier newtype. Non-empty.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoId(Box<str>);

impl RepoId {
    /// Construct a repository identifier; rejects the empty string with
    /// [`RuntimeErrorCode::DirtyBadIdentity`].
    pub fn new(s: impl Into<Box<str>>) -> Result<Self, RuntimeError> {
        let v: Box<str> = s.into();
        if v.is_empty() {
            return Err(RuntimeError::new(
                RuntimeErrorCode::DirtyBadIdentity,
                "repo_id must be non-empty",
            ));
        }
        Ok(Self(v))
    }

    /// Borrow as `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for RepoId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RepoId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = RepoId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("non-empty RepoId string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<RepoId, E> {
                if v.is_empty() {
                    return Err(E::custom("repo_id must be non-empty"));
                }
                Ok(RepoId(Box::from(v)))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<RepoId, E> {
                if v.is_empty() {
                    return Err(E::custom("repo_id must be non-empty"));
                }
                Ok(RepoId(v.into_boxed_str()))
            }
        }
        de.deserialize_str(V)
    }
}

/// Document identifier newtype. Authority is the manifest row identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub u64);

impl From<u64> for DocId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for DocId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DocId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DocId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("DocId unsigned 64-bit integer")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<DocId, E> {
                Ok(DocId(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<DocId, E> {
                Ok(DocId(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<DocId, E> {
                if v < 0 {
                    return Err(E::custom("DocId must be non-negative"));
                }
                let u = u64::try_from(v)
                    .map_err(|err| E::custom(format!("DocId out of u64 range: {err}")))?;
                Ok(DocId(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Manifest-generation newtype pinning a runtime read or buffer entry to a
/// specific linearization point on the producer's publish timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManifestGeneration(pub u64);

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

/// Apply-time anchor in unsigned milliseconds.
///
/// The runtime crate never reads the wall clock; this value is
/// **caller-supplied** by the write coordinator or sweep driver. Keeping time
/// as a typed parameter guarantees tests are deterministic and TTL math has
/// a single source of truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApplyTimeMs(pub u64);

impl fmt::Display for ApplyTimeMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for ApplyTimeMs {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ApplyTimeMs {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = ApplyTimeMs;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ApplyTimeMs unsigned 64-bit integer")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ApplyTimeMs, E> {
                Ok(ApplyTimeMs(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<ApplyTimeMs, E> {
                Ok(ApplyTimeMs(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ApplyTimeMs, E> {
                if v < 0 {
                    return Err(E::custom("ApplyTimeMs must be non-negative"));
                }
                let u = u64::try_from(v)
                    .map_err(|err| E::custom(format!("ApplyTimeMs out of u64 range: {err}")))?;
                Ok(ApplyTimeMs(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Configuration parameters for a [`crate::buffer::DirtyBuffer`]:
/// per-(tenant, repo) capacity and per-entry TTL in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferConfig {
    capacity: u32,
    ttl_ms: u64,
}

impl BufferConfig {
    /// Construct a config; rejects `capacity == 0` with
    /// [`RuntimeErrorCode::InvalidBufferConfig`]. A zero capacity would
    /// silently disable the capacity cap and is therefore a typed reject.
    pub fn new(capacity: u32, ttl_ms: u64) -> Result<Self, RuntimeError> {
        if capacity == 0 {
            return Err(RuntimeError::new(
                RuntimeErrorCode::InvalidBufferConfig,
                "buffer capacity must be > 0",
            ));
        }
        Ok(Self { capacity, ttl_ms })
    }

    /// Capacity cap (entries per (tenant, repo) buffer).
    #[must_use]
    pub const fn capacity(self) -> u32 {
        self.capacity
    }

    /// Per-entry TTL in milliseconds.
    #[must_use]
    pub const fn ttl_ms(self) -> u64 {
        self.ttl_ms
    }
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_BUFFER_CAPACITY,
            ttl_ms: DEFAULT_TTL_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyTimeMs, BufferConfig, DEFAULT_BUFFER_CAPACITY, DEFAULT_TTL_MS, DocId,
        ManifestGeneration, RepoId, TenantId,
    };
    use crate::errors::RuntimeErrorCode;

    #[test]
    fn defaults_match_spec() {
        assert_eq!(DEFAULT_BUFFER_CAPACITY, 10_000);
        assert_eq!(DEFAULT_TTL_MS, 300_000);
        let d = BufferConfig::default();
        assert_eq!(d.capacity(), DEFAULT_BUFFER_CAPACITY);
        assert_eq!(d.ttl_ms(), DEFAULT_TTL_MS);
    }

    #[test]
    fn tenant_id_rejects_empty() {
        match TenantId::new("") {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, RuntimeErrorCode::DirtyBadIdentity),
        }
    }

    #[test]
    fn tenant_id_accepts_nonempty() {
        match TenantId::new("acme") {
            Ok(t) => assert_eq!(t.as_str(), "acme"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn repo_id_rejects_empty() {
        match RepoId::new("") {
            Ok(_) => assert!(false, "must reject"),
            Err(e) => assert_eq!(e.code, RuntimeErrorCode::DirtyBadIdentity),
        }
    }

    #[test]
    fn buffer_config_rejects_zero_capacity() {
        match BufferConfig::new(0, 1_000) {
            Ok(_) => assert!(false, "must reject zero capacity"),
            Err(e) => assert_eq!(e.code, RuntimeErrorCode::InvalidBufferConfig),
        }
    }

    #[test]
    fn buffer_config_accepts_nonzero_capacity() {
        match BufferConfig::new(1, 0) {
            Ok(c) => {
                assert_eq!(c.capacity(), 1);
                assert_eq!(c.ttl_ms(), 0);
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn docid_display_and_from() {
        let d: DocId = DocId::from(42_u64);
        assert_eq!(d.0, 42);
        assert_eq!(format!("{d}"), "42");
    }

    #[test]
    fn newtypes_cbor_roundtrip() {
        let tenant = match TenantId::new("t1") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&tenant, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<TenantId, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, tenant),
            Err(e) => assert!(false, "{e}"),
        }

        let repo = match RepoId::new("r1") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&repo, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<RepoId, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, repo),
            Err(e) => assert!(false, "{e}"),
        }

        let doc = DocId(7);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&doc, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<DocId, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, doc),
            Err(e) => assert!(false, "{e}"),
        }

        let gen_ = ManifestGeneration(3);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&gen_, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<ManifestGeneration, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, gen_),
            Err(e) => assert!(false, "{e}"),
        }

        let at = ApplyTimeMs(123_456);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&at, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<ApplyTimeMs, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, at),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn empty_tenant_str_via_serde_rejected() {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&"", &mut buf) {
            assert!(false, "{e}");
        }
        if let Ok(v) = ciborium::de::from_reader::<TenantId, _>(buf.as_slice()) {
            assert!(false, "must reject empty, got {v:?}");
        }
    }
}
