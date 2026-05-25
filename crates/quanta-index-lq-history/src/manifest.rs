//! Manifest atomicity ledger.
//!
//! [`ManifestLedger`] is the single linearization point for write-packet
//! application. `apply()` is the only mutator. It enforces:
//!
//! - generation monotonicity: `packet.before_gen == current_gen` AND
//!   `packet.after_gen > packet.before_gen`; any regression surfaces
//!   [`HistoryErrorCode::StateGenerationRegression`]
//! - idempotency: if the trace hash already lives in
//!   `applied_packets`, the call returns
//!   [`ApplyOutcome::Idempotent`] with the prior trace id
//! - otherwise: insert the trace record, bump `current_gen`, return
//!   [`ApplyOutcome::Applied`]
//!
//! D18 — manual serde for [`ManifestLedger`]; no proc-macro derives.

use core::fmt;
use std::collections::BTreeMap;

use crate::errors::{HistoryError, HistoryErrorCode};
use crate::types::ManifestGeneration;
use crate::write_packet::{TraceRecord, WritePacket};

/// Result of [`ManifestLedger::apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApplyOutcome {
    /// Packet was new and advanced the manifest.
    Applied {
        /// Generation after the apply.
        new_gen: ManifestGeneration,
    },
    /// Packet hash already lives in the ledger; no-op.
    Idempotent {
        /// Existing trace id (the duplicate packet's hash).
        matched_trace_id: [u8; 32],
    },
}

/// Append-only manifest ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestLedger {
    current_gen: ManifestGeneration,
    applied_packets: BTreeMap<[u8; 32], TraceRecord>,
}

impl ManifestLedger {
    /// New ledger starting at `initial_gen`.
    #[must_use]
    pub fn new(initial_gen: ManifestGeneration) -> Self {
        Self {
            current_gen: initial_gen,
            applied_packets: BTreeMap::new(),
        }
    }

    /// Current generation.
    #[must_use]
    pub const fn current_gen(&self) -> ManifestGeneration {
        self.current_gen
    }

    /// Borrow the underlying trace map.
    #[must_use]
    pub const fn applied_packets(&self) -> &BTreeMap<[u8; 32], TraceRecord> {
        &self.applied_packets
    }

    /// Idempotent removal of an applied write packet by its trace hash.
    ///
    /// Returns `Ok(true)` when the hash was present and removed,
    /// `Ok(false)` when no entry matched. Generation is intentionally
    /// **not** rolled back; the caller is responsible for reconciling
    /// `current_gen` with the linearization tail when reverting.
    ///
    /// Use case: revert / rollback flows that need to drop a write
    /// packet from the ledger while keeping the linearization point
    /// monotonic for the rest of the system.
    ///
    /// The `Result` return preserves a typed-failure shape consistent
    /// with the rest of the [`ManifestLedger`] API even though the
    /// current implementation has no failure path; future invariants
    /// (e.g., refusing removal during an in-flight apply) will surface
    /// via `HistoryError` without changing the signature.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "API parity with ManifestLedger::apply; future invariants (e.g., refuse-while-applying) will surface typed errors here without breaking callers"
    )]
    pub fn remove_packet(&mut self, hash: [u8; 32]) -> Result<bool, HistoryError> {
        let removed = self.applied_packets.remove(&hash);
        Ok(removed.is_some())
    }

    /// Linearization point. See module docs for the decision table.
    pub fn apply(&mut self, packet: &WritePacket) -> Result<ApplyOutcome, HistoryError> {
        let trace = packet.trace_record()?;
        if let Some(prior) = self.applied_packets.get(&trace.hash) {
            return Ok(ApplyOutcome::Idempotent {
                matched_trace_id: prior.hash,
            });
        }
        if packet.before_gen() != self.current_gen {
            return Err(HistoryError::new(
                HistoryErrorCode::StateGenerationRegression,
                format!(
                    "manifest: packet.before_gen={} but current_gen={}",
                    packet.before_gen(),
                    self.current_gen
                ),
            ));
        }
        if packet.after_gen() <= packet.before_gen() {
            return Err(HistoryError::new(
                HistoryErrorCode::StateGenerationRegression,
                format!(
                    "manifest: packet.after_gen={} must exceed packet.before_gen={}",
                    packet.after_gen(),
                    packet.before_gen()
                ),
            ));
        }
        let new_gen = packet.after_gen();
        let hash = trace.hash;
        let _prev: Option<TraceRecord> = self.applied_packets.insert(hash, trace);
        self.current_gen = new_gen;
        Ok(ApplyOutcome::Applied { new_gen })
    }
}

impl serde::Serialize for ManifestLedger {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("ManifestLedger", 2)?;
        st.serialize_field("current_gen", &self.current_gen)?;
        // Map keys are `[u8; 32]`. Serialize as Vec<(HashWire, TraceRecord)>
        // so the byte-string key encoding is explicit.
        let entries: Vec<(HashKey, &TraceRecord)> = self
            .applied_packets
            .iter()
            .map(|(k, v)| (HashKey(*k), v))
            .collect();
        st.serialize_field("applied_packets", &entries)?;
        st.end()
    }
}

struct HashKey([u8; 32]);

impl serde::Serialize for HashKey {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for HashKey {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HashKey;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("32-byte hash key")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<HashKey, E> {
                if v.len() != 32 {
                    return Err(E::custom(format!(
                        "HashKey must be 32 bytes, got {}",
                        v.len()
                    )));
                }
                let mut arr = [0u8; 32];
                let mut i: usize = 0;
                for b in v {
                    if let Some(slot) = arr.get_mut(i) {
                        *slot = *b;
                    }
                    i = i.saturating_add(1);
                }
                Ok(HashKey(arr))
            }
            fn visit_borrowed_bytes<E: serde::de::Error>(self, v: &'d [u8]) -> Result<HashKey, E> {
                self.visit_bytes(v)
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<HashKey, E> {
                self.visit_bytes(v.as_slice())
            }
        }
        de.deserialize_bytes(V)
    }
}

impl<'de> serde::Deserialize<'de> for ManifestLedger {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            CurrentGen,
            AppliedPackets,
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
                        f.write_str("ManifestLedger field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "current_gen" => Ok(Field::CurrentGen),
                            "applied_packets" => Ok(Field::AppliedPackets),
                            other => {
                                Err(E::unknown_field(other, &["current_gen", "applied_packets"]))
                            }
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct LV;
        impl<'d> serde::de::Visitor<'d> for LV {
            type Value = ManifestLedger;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ManifestLedger struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<ManifestLedger, A::Error> {
                let mut current_gen: Option<ManifestGeneration> = None;
                let mut entries: Option<Vec<(HashKey, TraceRecord)>> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::CurrentGen => {
                            if current_gen.is_some() {
                                return Err(serde::de::Error::duplicate_field("current_gen"));
                            }
                            current_gen = Some(map.next_value()?);
                        }
                        Field::AppliedPackets => {
                            if entries.is_some() {
                                return Err(serde::de::Error::duplicate_field("applied_packets"));
                            }
                            entries = Some(map.next_value()?);
                        }
                    }
                }
                let current_gen =
                    current_gen.ok_or_else(|| serde::de::Error::missing_field("current_gen"))?;
                let entries =
                    entries.ok_or_else(|| serde::de::Error::missing_field("applied_packets"))?;
                let mut applied_packets: BTreeMap<[u8; 32], TraceRecord> = BTreeMap::new();
                for (k, v) in entries {
                    let _prev: Option<TraceRecord> = applied_packets.insert(k.0, v);
                }
                Ok(ManifestLedger {
                    current_gen,
                    applied_packets,
                })
            }
        }

        de.deserialize_struct("ManifestLedger", &["current_gen", "applied_packets"], LV)
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplyOutcome, ManifestLedger};
    use crate::errors::HistoryErrorCode;
    use crate::types::{AppliedAtMs, ManifestGeneration};
    use crate::write_packet::WritePacket;

    fn mg(v: u64) -> ManifestGeneration {
        match ManifestGeneration::new(v) {
            Ok(g) => g,
            Err(e) => {
                assert!(false, "test helper mg({v}) failed: {e}");
                ManifestGeneration::from_raw(v)
            }
        }
    }

    #[test]
    fn apply_advances_generation() {
        let mut led = ManifestLedger::new(mg(1));
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(2), vec![1]);
        match led.apply(&p) {
            Ok(ApplyOutcome::Applied { new_gen }) => assert_eq!(new_gen, mg(2)),
            Ok(ApplyOutcome::Idempotent { .. }) => assert!(false, "must be Applied"),
            Err(e) => assert!(false, "{e}"),
        }
        assert_eq!(led.current_gen(), mg(2));
        assert_eq!(led.applied_packets().len(), 1);
    }

    #[test]
    fn apply_twice_returns_idempotent() {
        let mut led = ManifestLedger::new(mg(1));
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(2), vec![1]);
        let _first = match led.apply(&p) {
            Ok(o) => o,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let expected_hash = match p.hash() {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match led.apply(&p) {
            Ok(ApplyOutcome::Idempotent { matched_trace_id }) => {
                assert_eq!(matched_trace_id, expected_hash);
            }
            Ok(ApplyOutcome::Applied { .. }) => assert!(false, "must be Idempotent"),
            Err(e) => assert!(false, "{e}"),
        }
        // Generation did not advance again.
        assert_eq!(led.current_gen(), mg(2));
    }

    #[test]
    fn regression_before_gen_mismatch_fails() {
        let mut led = ManifestLedger::new(mg(5));
        // before_gen=2 doesn't match current_gen=5
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(2), mg(3), vec![1]);
        match led.apply(&p) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::StateGenerationRegression),
        }
    }

    #[test]
    fn regression_after_le_before_fails() {
        let mut led = ManifestLedger::new(mg(1));
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(1), vec![1]);
        match led.apply(&p) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::StateGenerationRegression),
        }
    }

    #[test]
    fn sequential_apply_advances_each_time() {
        let mut led = ManifestLedger::new(mg(1));
        let p1 = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(2), vec![1]);
        let p2 = WritePacket::new("alice", AppliedAtMs::new(1), mg(2), mg(3), vec![2]);
        let p3 = WritePacket::new("alice", AppliedAtMs::new(2), mg(3), mg(4), vec![3]);
        for p in [&p1, &p2, &p3] {
            match led.apply(p) {
                Ok(ApplyOutcome::Applied { .. }) => {}
                Ok(ApplyOutcome::Idempotent { .. }) => assert!(false, "must be Applied"),
                Err(e) => assert!(false, "{e}"),
            }
        }
        assert_eq!(led.current_gen(), mg(4));
        assert_eq!(led.applied_packets().len(), 3);
    }

    #[test]
    fn remove_packet_present_returns_true() {
        let mut led = ManifestLedger::new(mg(1));
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(2), vec![1]);
        match led.apply(&p) {
            Ok(_) => {}
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        let hash = match p.hash() {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match led.remove_packet(hash) {
            Ok(true) => {}
            Ok(false) => assert!(false, "must remove existing packet"),
            Err(e) => assert!(false, "{e}"),
        }
        assert!(led.applied_packets().is_empty());
        // Generation deliberately NOT rolled back.
        assert_eq!(led.current_gen(), mg(2));
    }

    #[test]
    fn remove_packet_absent_returns_false_no_error() {
        let mut led = ManifestLedger::new(mg(1));
        let bogus = [0u8; 32];
        match led.remove_packet(bogus) {
            Ok(false) => {}
            Ok(true) => assert!(false, "must not claim removal"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn remove_packet_is_idempotent() {
        let mut led = ManifestLedger::new(mg(1));
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(2), vec![1]);
        match led.apply(&p) {
            Ok(_) => {}
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        let hash = match p.hash() {
            Ok(h) => h,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match led.remove_packet(hash) {
            Ok(true) => {}
            Ok(false) => assert!(false, "first remove must succeed"),
            Err(e) => assert!(false, "{e}"),
        }
        // Second call: idempotent no-op.
        match led.remove_packet(hash) {
            Ok(false) => {}
            Ok(true) => assert!(false, "second remove must be no-op"),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn ledger_cbor_roundtrip() {
        let mut led = ManifestLedger::new(mg(1));
        let p = WritePacket::new("alice", AppliedAtMs::new(0), mg(1), mg(2), vec![1, 2]);
        match led.apply(&p) {
            Ok(_) => {}
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&led, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<ManifestLedger, _>(buf.as_slice()) {
            Ok(back) => assert_eq!(back, led),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
