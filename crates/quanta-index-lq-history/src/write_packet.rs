//! Write-packet wire shape and trace record.
//!
//! [`WritePacket`] is the unit of authority routed from the producer plane to
//! the search plane. Hashing the packet under CBOR-canonical encoding with
//! the `b"WritePacketV1\0"` domain tag yields the trace id used by the
//! manifest ledger to detect idempotent replays.
//!
//! [`TraceRecord`] is the compact ledger-side projection that the manifest
//! retains forever to gate `apply()`.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use sha2::{Digest, Sha256};

use crate::types::{AppliedAtMs, ManifestGeneration};

/// Domain tag prepended to the CBOR body before hashing. Bumping the tag is
/// the only path to re-hash the same payload bytes into a different trace.
pub const WRITE_PACKET_DOMAIN: &[u8] = b"WritePacketV1\0";

/// One write-packet routed by the producer plane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WritePacket {
    committer_id: Box<str>,
    applied_at_ms: AppliedAtMs,
    before_gen: ManifestGeneration,
    after_gen: ManifestGeneration,
    payload: Vec<u8>,
}

impl WritePacket {
    /// Build a write-packet.
    #[must_use]
    pub fn new(
        committer_id: impl Into<Box<str>>,
        applied_at_ms: AppliedAtMs,
        before_gen: ManifestGeneration,
        after_gen: ManifestGeneration,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            committer_id: committer_id.into(),
            applied_at_ms,
            before_gen,
            after_gen,
            payload,
        }
    }

    /// Committer identity.
    #[must_use]
    pub fn committer_id(&self) -> &str {
        &self.committer_id
    }

    /// Time anchor.
    #[must_use]
    pub const fn applied_at_ms(&self) -> AppliedAtMs {
        self.applied_at_ms
    }

    /// Pre-apply generation.
    #[must_use]
    pub const fn before_gen(&self) -> ManifestGeneration {
        self.before_gen
    }

    /// Post-apply generation.
    #[must_use]
    pub const fn after_gen(&self) -> ManifestGeneration {
        self.after_gen
    }

    /// Borrow the payload bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// SHA-256 over `WRITE_PACKET_DOMAIN || cbor(self)`.
    ///
    /// The trace id is byte-stable across runs and platforms because the
    /// CBOR encoding is deterministic (manual `Serialize` impl emits fields
    /// in fixed order; `Vec<u8>` and string lengths are fixed).
    ///
    /// Returns [`crate::errors::HistoryErrorCode::HistoryTraceIncomplete`]
    /// if the CBOR serialize step fails. The codec path against `Vec<u8>`
    /// is infallible in practice, but the function propagates the typed
    /// error rather than falling back to a sentinel digest — a single
    /// packet must hash to exactly one digest under exactly one algorithm.
    pub fn hash(&self) -> Result<[u8; 32], crate::errors::HistoryError> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(self, &mut buf).map_err(|e| {
            crate::errors::HistoryError::new(
                crate::errors::HistoryErrorCode::HistoryTraceIncomplete,
                format!("cbor encode of WritePacket failed: {e}"),
            )
        })?;
        let mut h = Sha256::new();
        h.update(WRITE_PACKET_DOMAIN);
        h.update(&buf);
        let out = h.finalize();
        let mut arr = [0u8; 32];
        let mut i: usize = 0;
        while i < 32 {
            if let (Some(slot), Some(src)) = (arr.get_mut(i), out.get(i)) {
                *slot = *src;
            }
            i = i.saturating_add(1);
        }
        Ok(arr)
    }

    /// Produce the ledger-side projection.
    ///
    /// Returns the typed error from [`WritePacket::hash`] if CBOR encode
    /// fails. The `trace_record` shape is identity-bearing — never produce
    /// a `TraceRecord` with a sentinel digest.
    pub fn trace_record(&self) -> Result<TraceRecord, crate::errors::HistoryError> {
        Ok(TraceRecord {
            committer_id: self.committer_id.clone(),
            applied_at_ms: self.applied_at_ms,
            before_gen: self.before_gen,
            after_gen: self.after_gen,
            hash: self.hash()?,
        })
    }
}

impl serde::Serialize for WritePacket {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("WritePacket", 5)?;
        st.serialize_field("committer_id", self.committer_id.as_ref())?;
        st.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        st.serialize_field("before_gen", &self.before_gen)?;
        st.serialize_field("after_gen", &self.after_gen)?;
        // Serialize payload as a CBOR byte-string (not an array of ints) via
        // a borrowing newtype.
        st.serialize_field("payload", &PayloadWire(&self.payload))?;
        st.end()
    }
}

/// Borrowing wrapper that serializes a `&[u8]` as a CBOR byte string.
struct PayloadWire<'a>(&'a [u8]);

impl serde::Serialize for PayloadWire<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for WritePacket {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            CommitterId,
            AppliedAtMs,
            BeforeGen,
            AfterGen,
            Payload,
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
                        f.write_str("WritePacket field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "committer_id" => Ok(Field::CommitterId),
                            "applied_at_ms" => Ok(Field::AppliedAtMs),
                            "before_gen" => Ok(Field::BeforeGen),
                            "after_gen" => Ok(Field::AfterGen),
                            "payload" => Ok(Field::Payload),
                            other => Err(E::unknown_field(
                                other,
                                &[
                                    "committer_id",
                                    "applied_at_ms",
                                    "before_gen",
                                    "after_gen",
                                    "payload",
                                ],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct PV;
        impl<'d> serde::de::Visitor<'d> for PV {
            type Value = WritePacket;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("WritePacket struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<WritePacket, A::Error> {
                let mut committer_id: Option<String> = None;
                let mut applied_at_ms: Option<AppliedAtMs> = None;
                let mut before_gen: Option<ManifestGeneration> = None;
                let mut after_gen: Option<ManifestGeneration> = None;
                let mut payload: Option<Vec<u8>> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::CommitterId => {
                            if committer_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("committer_id"));
                            }
                            committer_id = Some(map.next_value()?);
                        }
                        Field::AppliedAtMs => {
                            if applied_at_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("applied_at_ms"));
                            }
                            applied_at_ms = Some(map.next_value()?);
                        }
                        Field::BeforeGen => {
                            if before_gen.is_some() {
                                return Err(serde::de::Error::duplicate_field("before_gen"));
                            }
                            before_gen = Some(map.next_value()?);
                        }
                        Field::AfterGen => {
                            if after_gen.is_some() {
                                return Err(serde::de::Error::duplicate_field("after_gen"));
                            }
                            after_gen = Some(map.next_value()?);
                        }
                        Field::Payload => {
                            if payload.is_some() {
                                return Err(serde::de::Error::duplicate_field("payload"));
                            }
                            let bytes: ByteBuf = map.next_value()?;
                            payload = Some(bytes.0);
                        }
                    }
                }
                let committer_id =
                    committer_id.ok_or_else(|| serde::de::Error::missing_field("committer_id"))?;
                let applied_at_ms = applied_at_ms
                    .ok_or_else(|| serde::de::Error::missing_field("applied_at_ms"))?;
                let before_gen =
                    before_gen.ok_or_else(|| serde::de::Error::missing_field("before_gen"))?;
                let after_gen =
                    after_gen.ok_or_else(|| serde::de::Error::missing_field("after_gen"))?;
                let payload = payload.ok_or_else(|| serde::de::Error::missing_field("payload"))?;
                Ok(WritePacket {
                    committer_id: committer_id.into_boxed_str(),
                    applied_at_ms,
                    before_gen,
                    after_gen,
                    payload,
                })
            }
        }

        de.deserialize_struct(
            "WritePacket",
            &[
                "committer_id",
                "applied_at_ms",
                "before_gen",
                "after_gen",
                "payload",
            ],
            PV,
        )
    }
}

/// Local wrapper used to coerce CBOR byte-strings back to `Vec<u8>`.
struct ByteBuf(Vec<u8>);

impl<'de> serde::Deserialize<'de> for ByteBuf {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = ByteBuf;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("byte string")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<ByteBuf, E> {
                Ok(ByteBuf(v.to_vec()))
            }
            fn visit_borrowed_bytes<E: serde::de::Error>(self, v: &'d [u8]) -> Result<ByteBuf, E> {
                Ok(ByteBuf(v.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<ByteBuf, E> {
                Ok(ByteBuf(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'d>>(
                self,
                mut seq: A,
            ) -> Result<ByteBuf, A::Error> {
                let mut out: Vec<u8> = Vec::new();
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(ByteBuf(out))
            }
        }
        de.deserialize_bytes(V)
    }
}

/// Manifest-ledger projection of [`WritePacket`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraceRecord {
    /// Committer identity copied from the packet.
    pub committer_id: Box<str>,
    /// Time anchor copied from the packet.
    pub applied_at_ms: AppliedAtMs,
    /// Pre-apply generation copied from the packet.
    pub before_gen: ManifestGeneration,
    /// Post-apply generation copied from the packet.
    pub after_gen: ManifestGeneration,
    /// Domain-tagged SHA-256 trace id.
    pub hash: [u8; 32],
}

impl serde::Serialize for TraceRecord {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("TraceRecord", 5)?;
        st.serialize_field("committer_id", self.committer_id.as_ref())?;
        st.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        st.serialize_field("before_gen", &self.before_gen)?;
        st.serialize_field("after_gen", &self.after_gen)?;
        // Serialize as byte-string to keep the wire compact.
        st.serialize_field("hash", &HashWire(self.hash))?;
        st.end()
    }
}

struct HashWire([u8; 32]);

impl serde::Serialize for HashWire {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for TraceRecord {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            CommitterId,
            AppliedAtMs,
            BeforeGen,
            AfterGen,
            Hash,
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
                        f.write_str("TraceRecord field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "committer_id" => Ok(Field::CommitterId),
                            "applied_at_ms" => Ok(Field::AppliedAtMs),
                            "before_gen" => Ok(Field::BeforeGen),
                            "after_gen" => Ok(Field::AfterGen),
                            "hash" => Ok(Field::Hash),
                            other => Err(E::unknown_field(
                                other,
                                &[
                                    "committer_id",
                                    "applied_at_ms",
                                    "before_gen",
                                    "after_gen",
                                    "hash",
                                ],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct TV;
        impl<'d> serde::de::Visitor<'d> for TV {
            type Value = TraceRecord;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("TraceRecord struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<TraceRecord, A::Error> {
                let mut committer_id: Option<String> = None;
                let mut applied_at_ms: Option<AppliedAtMs> = None;
                let mut before_gen: Option<ManifestGeneration> = None;
                let mut after_gen: Option<ManifestGeneration> = None;
                let mut hash: Option<[u8; 32]> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::CommitterId => {
                            if committer_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("committer_id"));
                            }
                            committer_id = Some(map.next_value()?);
                        }
                        Field::AppliedAtMs => {
                            if applied_at_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("applied_at_ms"));
                            }
                            applied_at_ms = Some(map.next_value()?);
                        }
                        Field::BeforeGen => {
                            if before_gen.is_some() {
                                return Err(serde::de::Error::duplicate_field("before_gen"));
                            }
                            before_gen = Some(map.next_value()?);
                        }
                        Field::AfterGen => {
                            if after_gen.is_some() {
                                return Err(serde::de::Error::duplicate_field("after_gen"));
                            }
                            after_gen = Some(map.next_value()?);
                        }
                        Field::Hash => {
                            if hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("hash"));
                            }
                            let bb: ByteBuf = map.next_value()?;
                            if bb.0.len() != 32 {
                                return Err(serde::de::Error::custom(format!(
                                    "TraceRecord hash must be 32 bytes, got {}",
                                    bb.0.len()
                                )));
                            }
                            let mut arr = [0u8; 32];
                            let mut i: usize = 0;
                            for b in &bb.0 {
                                if let Some(slot) = arr.get_mut(i) {
                                    *slot = *b;
                                }
                                i = i.saturating_add(1);
                            }
                            hash = Some(arr);
                        }
                    }
                }
                let committer_id =
                    committer_id.ok_or_else(|| serde::de::Error::missing_field("committer_id"))?;
                let applied_at_ms = applied_at_ms
                    .ok_or_else(|| serde::de::Error::missing_field("applied_at_ms"))?;
                let before_gen =
                    before_gen.ok_or_else(|| serde::de::Error::missing_field("before_gen"))?;
                let after_gen =
                    after_gen.ok_or_else(|| serde::de::Error::missing_field("after_gen"))?;
                let hash = hash.ok_or_else(|| serde::de::Error::missing_field("hash"))?;
                Ok(TraceRecord {
                    committer_id: committer_id.into_boxed_str(),
                    applied_at_ms,
                    before_gen,
                    after_gen,
                    hash,
                })
            }
        }

        de.deserialize_struct(
            "TraceRecord",
            &[
                "committer_id",
                "applied_at_ms",
                "before_gen",
                "after_gen",
                "hash",
            ],
            TV,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{TraceRecord, WRITE_PACKET_DOMAIN, WritePacket};
    use crate::types::{AppliedAtMs, ManifestGeneration};

    fn mg(v: u64) -> ManifestGeneration {
        // Tests only ever pass non-zero generations; the constructor cannot
        // fail. Surface any future regression explicitly.
        match ManifestGeneration::new(v) {
            Ok(g) => g,
            Err(e) => {
                assert!(false, "test helper mg({v}) failed: {e}");
                ManifestGeneration::from_raw(v)
            }
        }
    }

    fn must_hash(p: &WritePacket) -> [u8; 32] {
        match p.hash() {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "test helper must_hash failed: {e}");
                [0u8; 32]
            }
        }
    }

    fn must_trace(p: &WritePacket) -> TraceRecord {
        match p.trace_record() {
            Ok(tr) => tr,
            Err(e) => {
                assert!(false, "test helper must_trace failed: {e}");
                TraceRecord {
                    committer_id: "".into(),
                    applied_at_ms: AppliedAtMs::new(0),
                    before_gen: mg(1),
                    after_gen: mg(2),
                    hash: [0u8; 32],
                }
            }
        }
    }

    #[test]
    fn hash_is_stable_for_same_payload() {
        let p = WritePacket::new("alice", AppliedAtMs::new(1), mg(1), mg(2), vec![1, 2, 3]);
        assert_eq!(must_hash(&p), must_hash(&p));
    }

    #[test]
    fn hash_changes_with_payload() {
        let p1 = WritePacket::new("alice", AppliedAtMs::new(1), mg(1), mg(2), vec![1, 2, 3]);
        let p2 = WritePacket::new("alice", AppliedAtMs::new(1), mg(1), mg(2), vec![4, 5, 6]);
        assert_ne!(must_hash(&p1), must_hash(&p2));
    }

    #[test]
    fn hash_changes_with_committer() {
        let p1 = WritePacket::new("alice", AppliedAtMs::new(1), mg(1), mg(2), vec![1]);
        let p2 = WritePacket::new("bob", AppliedAtMs::new(1), mg(1), mg(2), vec![1]);
        assert_ne!(must_hash(&p1), must_hash(&p2));
    }

    #[test]
    fn domain_tag_is_distinct() {
        // Bumping the domain tag in the future must invalidate prior hashes.
        // This guard locks the v1 value.
        assert_eq!(WRITE_PACKET_DOMAIN, b"WritePacketV1\0");
    }

    #[test]
    fn trace_record_carries_hash() {
        let p = WritePacket::new("alice", AppliedAtMs::new(1), mg(1), mg(2), vec![9]);
        let tr = must_trace(&p);
        assert_eq!(tr.hash, must_hash(&p));
        assert_eq!(tr.committer_id.as_ref(), "alice");
        assert_eq!(tr.applied_at_ms, AppliedAtMs::new(1));
        assert_eq!(tr.before_gen, mg(1));
        assert_eq!(tr.after_gen, mg(2));
    }

    #[test]
    fn write_packet_cbor_roundtrip() {
        let p = WritePacket::new(
            "alice",
            AppliedAtMs::new(123),
            mg(1),
            mg(2),
            vec![0xde, 0xad, 0xbe, 0xef],
        );
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&p, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<WritePacket, _>(buf.as_slice()) {
            Ok(back) => assert_eq!(back, p),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn trace_record_cbor_roundtrip() {
        let p = WritePacket::new("alice", AppliedAtMs::new(1), mg(1), mg(2), vec![1, 2]);
        let tr: TraceRecord = must_trace(&p);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&tr, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<TraceRecord, _>(buf.as_slice()) {
            Ok(back) => assert_eq!(back, tr),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
