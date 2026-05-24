//! Inbound `apply_changes` payloads and outcomes per RT-01 § 4.3 / § 5.5.
//!
//! [`DirtyEntry`] is the producer-published mark-dirty record routed through
//! the write-coordinator-gated channel; [`ApplyOutcome`] is the typed verdict
//! returned by [`crate::buffer::DirtyBuffer::apply`].
//!
//! D18 — every wire shape is hand-rolled serde.

use core::fmt;

use crate::errors::RuntimeErrorCode;
use crate::types::{ApplyTimeMs, DocId, ManifestGeneration, RepoId, TenantId};

/// Size of the payload identity hash carried by [`DirtyEntry`].
pub const PAYLOAD_HASH_LEN: usize = 32;

/// One producer-marked dirty record.
///
/// Identity tuple `(tenant_id, repo_id, doc_id, generation)` is keyed against
/// the receiving buffer; mismatches yield typed rejects. `payload_hash` lets
/// the buffer recognise idempotent re-apply attempts without rebuilding the
/// document body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyEntry {
    /// Tenant scope this entry belongs to.
    pub tenant_id: TenantId,
    /// Repository scope this entry belongs to.
    pub repo_id: RepoId,
    /// Document identity (manifest-aligned).
    pub doc_id: DocId,
    /// Generation the producer believes is currently active.
    pub generation: ManifestGeneration,
    /// Caller-supplied apply-time anchor in milliseconds.
    pub applied_at_ms: ApplyTimeMs,
    /// 32-byte payload identity hash.
    pub payload_hash: [u8; PAYLOAD_HASH_LEN],
}

/// CBOR field names for [`DirtyEntry`].
const DIRTY_ENTRY_FIELDS: &[&str] = &[
    "tenant_id",
    "repo_id",
    "doc_id",
    "generation",
    "applied_at_ms",
    "payload_hash",
];

impl serde::Serialize for DirtyEntry {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        let mut st = ser.serialize_struct("DirtyEntry", 6)?;
        st.serialize_field("tenant_id", &self.tenant_id)?;
        st.serialize_field("repo_id", &self.repo_id)?;
        st.serialize_field("doc_id", &self.doc_id)?;
        st.serialize_field("generation", &self.generation)?;
        st.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        st.serialize_field("payload_hash", &PayloadHashRef(&self.payload_hash))?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for DirtyEntry {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = DirtyEntry;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("DirtyEntry struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<DirtyEntry, A::Error> {
                let mut tenant_id: Option<TenantId> = None;
                let mut repo_id: Option<RepoId> = None;
                let mut doc_id: Option<DocId> = None;
                let mut generation: Option<ManifestGeneration> = None;
                let mut applied_at_ms: Option<ApplyTimeMs> = None;
                let mut payload_hash: Option<[u8; PAYLOAD_HASH_LEN]> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tenant_id" => {
                            if tenant_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("tenant_id"));
                            }
                            tenant_id = Some(map.next_value::<TenantId>()?);
                        }
                        "repo_id" => {
                            if repo_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("repo_id"));
                            }
                            repo_id = Some(map.next_value::<RepoId>()?);
                        }
                        "doc_id" => {
                            if doc_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("doc_id"));
                            }
                            doc_id = Some(map.next_value::<DocId>()?);
                        }
                        "generation" => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value::<ManifestGeneration>()?);
                        }
                        "applied_at_ms" => {
                            if applied_at_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("applied_at_ms"));
                            }
                            applied_at_ms = Some(map.next_value::<ApplyTimeMs>()?);
                        }
                        "payload_hash" => {
                            if payload_hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("payload_hash"));
                            }
                            let w = map.next_value::<PayloadHashOwned>()?;
                            payload_hash = Some(w.0);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, DIRTY_ENTRY_FIELDS));
                        }
                    }
                }
                let tenant_id =
                    tenant_id.ok_or_else(|| serde::de::Error::missing_field("tenant_id"))?;
                let repo_id = repo_id.ok_or_else(|| serde::de::Error::missing_field("repo_id"))?;
                let doc_id = doc_id.ok_or_else(|| serde::de::Error::missing_field("doc_id"))?;
                let generation =
                    generation.ok_or_else(|| serde::de::Error::missing_field("generation"))?;
                let applied_at_ms = applied_at_ms
                    .ok_or_else(|| serde::de::Error::missing_field("applied_at_ms"))?;
                let payload_hash =
                    payload_hash.ok_or_else(|| serde::de::Error::missing_field("payload_hash"))?;
                Ok(DirtyEntry {
                    tenant_id,
                    repo_id,
                    doc_id,
                    generation,
                    applied_at_ms,
                    payload_hash,
                })
            }
        }
        de.deserialize_struct("DirtyEntry", DIRTY_ENTRY_FIELDS, V)
    }
}

/// Typed verdict from [`crate::buffer::DirtyBuffer::apply`].
///
/// No silent path: every reject names a [`RuntimeErrorCode`] sub-cause; every
/// idempotent re-apply echoes the matched hash so the caller can confirm the
/// payload identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Entry accepted into the buffer.
    Buffered,
    /// Slot already held an equivalent entry; second submit was a no-op.
    Idempotent {
        /// The hash that matched (mirrors the incoming `payload_hash`).
        matched_hash: [u8; PAYLOAD_HASH_LEN],
    },
    /// Typed rejection — never silent.
    Rejected {
        /// The taxonomy code that fired.
        reason: RuntimeErrorCode,
    },
}

const APPLY_OUTCOME_TAGS: &[&str] = &["Buffered", "Idempotent", "Rejected"];

impl serde::Serialize for ApplyOutcome {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        match self {
            Self::Buffered => {
                let mut st = ser.serialize_struct("ApplyOutcome", 1)?;
                st.serialize_field("tag", "Buffered")?;
                st.end()
            }
            Self::Idempotent { matched_hash } => {
                let mut st = ser.serialize_struct("ApplyOutcome", 2)?;
                st.serialize_field("tag", "Idempotent")?;
                st.serialize_field("matched_hash", &PayloadHashRef(matched_hash))?;
                st.end()
            }
            Self::Rejected { reason } => {
                let mut st = ser.serialize_struct("ApplyOutcome", 2)?;
                st.serialize_field("tag", "Rejected")?;
                st.serialize_field("reason", reason)?;
                st.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for ApplyOutcome {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = ApplyOutcome;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ApplyOutcome tagged struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<ApplyOutcome, A::Error> {
                let mut tag: Option<String> = None;
                let mut matched_hash: Option<[u8; PAYLOAD_HASH_LEN]> = None;
                let mut reason: Option<RuntimeErrorCode> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => {
                            if tag.is_some() {
                                return Err(serde::de::Error::duplicate_field("tag"));
                            }
                            tag = Some(map.next_value::<String>()?);
                        }
                        "matched_hash" => {
                            if matched_hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("matched_hash"));
                            }
                            let w = map.next_value::<PayloadHashOwned>()?;
                            matched_hash = Some(w.0);
                        }
                        "reason" => {
                            if reason.is_some() {
                                return Err(serde::de::Error::duplicate_field("reason"));
                            }
                            reason = Some(map.next_value::<RuntimeErrorCode>()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["tag", "matched_hash", "reason"],
                            ));
                        }
                    }
                }
                let tag = tag.ok_or_else(|| serde::de::Error::missing_field("tag"))?;
                match tag.as_str() {
                    "Buffered" => Ok(ApplyOutcome::Buffered),
                    "Idempotent" => {
                        let h = matched_hash
                            .ok_or_else(|| serde::de::Error::missing_field("matched_hash"))?;
                        Ok(ApplyOutcome::Idempotent { matched_hash: h })
                    }
                    "Rejected" => {
                        let r = reason.ok_or_else(|| serde::de::Error::missing_field("reason"))?;
                        Ok(ApplyOutcome::Rejected { reason: r })
                    }
                    other => Err(serde::de::Error::unknown_variant(other, APPLY_OUTCOME_TAGS)),
                }
            }
        }
        de.deserialize_map(V)
    }
}

/// Borrowed serializer wrapper that emits a `[u8; 32]` as a CBOR byte string.
struct PayloadHashRef<'a>(&'a [u8; PAYLOAD_HASH_LEN]);

impl serde::Serialize for PayloadHashRef<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(self.0)
    }
}

/// Owned deserializer wrapper that parses a CBOR byte string into `[u8; 32]`.
struct PayloadHashOwned([u8; PAYLOAD_HASH_LEN]);

impl<'de> serde::Deserialize<'de> for PayloadHashOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = PayloadHashOwned;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{PAYLOAD_HASH_LEN}-byte payload identity hash")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<PayloadHashOwned, E> {
                if v.len() != PAYLOAD_HASH_LEN {
                    return Err(E::custom(format!(
                        "payload_hash must be {PAYLOAD_HASH_LEN} bytes, got {}",
                        v.len()
                    )));
                }
                let mut out = [0u8; PAYLOAD_HASH_LEN];
                for (i, b) in v.iter().enumerate() {
                    let slot = out
                        .get_mut(i)
                        .ok_or_else(|| E::custom("payload_hash slot out of range"))?;
                    *slot = *b;
                }
                Ok(PayloadHashOwned(out))
            }
            fn visit_borrowed_bytes<E: serde::de::Error>(
                self,
                v: &'d [u8],
            ) -> Result<PayloadHashOwned, E> {
                self.visit_bytes(v)
            }
            fn visit_byte_buf<E: serde::de::Error>(
                self,
                v: Vec<u8>,
            ) -> Result<PayloadHashOwned, E> {
                self.visit_bytes(v.as_slice())
            }
            fn visit_seq<A: serde::de::SeqAccess<'d>>(
                self,
                mut seq: A,
            ) -> Result<PayloadHashOwned, A::Error> {
                let mut out = [0u8; PAYLOAD_HASH_LEN];
                let mut i: usize = 0;
                while let Some(b) = seq.next_element::<u8>()? {
                    let slot = out.get_mut(i).ok_or_else(|| {
                        serde::de::Error::custom("payload_hash slot out of range")
                    })?;
                    *slot = b;
                    i = i.saturating_add(1);
                }
                if i != PAYLOAD_HASH_LEN {
                    return Err(serde::de::Error::custom(format!(
                        "payload_hash must be {PAYLOAD_HASH_LEN} bytes, got {i}"
                    )));
                }
                Ok(PayloadHashOwned(out))
            }
        }
        de.deserialize_bytes(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplyOutcome, DirtyEntry, PAYLOAD_HASH_LEN};
    use crate::errors::RuntimeErrorCode;
    use crate::types::{ApplyTimeMs, DocId, ManifestGeneration, RepoId, TenantId};

    fn sample_entry() -> Result<DirtyEntry, crate::errors::RuntimeError> {
        let tenant = TenantId::new("t1")?;
        let repo = RepoId::new("r1")?;
        Ok(DirtyEntry {
            tenant_id: tenant,
            repo_id: repo,
            doc_id: DocId(7),
            generation: ManifestGeneration(3),
            applied_at_ms: ApplyTimeMs(1_000),
            payload_hash: [0xABu8; PAYLOAD_HASH_LEN],
        })
    }

    #[test]
    fn dirty_entry_cbor_roundtrip() {
        let e = match sample_entry() {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "{err}");
                return;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
        }
        match ciborium::de::from_reader::<DirtyEntry, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, e),
            Err(err) => assert!(false, "{err}"),
        }
    }

    #[test]
    fn apply_outcome_buffered_cbor_roundtrip() {
        let o = ApplyOutcome::Buffered;
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&o, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<ApplyOutcome, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, o),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn apply_outcome_idempotent_cbor_roundtrip() {
        let o = ApplyOutcome::Idempotent {
            matched_hash: [0x11u8; PAYLOAD_HASH_LEN],
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&o, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<ApplyOutcome, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, o),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn apply_outcome_rejected_cbor_roundtrip() {
        let o = ApplyOutcome::Rejected {
            reason: RuntimeErrorCode::DirtyStaleGen,
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&o, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<ApplyOutcome, _>(buf.as_slice()) {
            Ok(v) => assert_eq!(v, o),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
