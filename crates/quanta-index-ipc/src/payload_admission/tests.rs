//! Encoder reservation tests use a local ledger, not a product Original issuer.

use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Serialize, Serializer};

use super::{
    encode_cbor_payload_admitted_v1, encode_cbor_payload_into_slots_v1, CborPayloadAdmissionErrorV1,
};
use crate::{
    IndexError, IpcError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
    MemoryScopePort, MAX_FRAME_BODY_BYTES,
};

#[derive(Default)]
struct TestLedger {
    live: AtomicU64,
    reserved: AtomicU64,
    released: AtomicU64,
    calls: AtomicUsize,
}

impl MemoryLeaseOwner for TestLedger {
    fn release(&self, bytes: u64) {
        let prior_live = self
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_sub(bytes)
            });
        assert!(
            prior_live.is_ok(),
            "test ledger released more than reserved"
        );
        let prior_released =
            self.released
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |released| {
                    released.checked_add(bytes)
                });
        assert!(
            prior_released.is_ok(),
            "test ledger release counter overflowed"
        );
    }
}

struct TestScope {
    ledger: Arc<TestLedger>,
    deny: bool,
}

impl MemoryScopePort for TestScope {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let prior_calls =
            self.ledger
                .calls
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |calls| {
                    calls.checked_add(1)
                });
        let _prior_calls = prior_calls.map_err(|_overflow| {
            IndexError::BackendViolation("test ledger reservation counter overflowed".into())
        })?;
        if self.deny || lifetime != MemoryLifetime::Retained {
            return Err(IndexError::ResourceLimited);
        }
        let prior_live =
            self.ledger
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                });
        let _prior_live = prior_live.map_err(|_overflow| {
            IndexError::BackendViolation("test ledger live charge overflowed".into())
        })?;
        let prior_reserved =
            self.ledger
                .reserved
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |reserved| {
                    reserved.checked_add(bytes)
                });
        let _prior_reserved = prior_reserved.map_err(|_overflow| {
            IndexError::BackendViolation("test ledger reservation total overflowed".into())
        })?;
        let owner: Arc<dyn MemoryLeaseOwner> = self.ledger.clone();
        Ok(MemoryLease::new(bytes, owner))
    }
}

fn test_scope(deny: bool) -> (MemoryScope, Arc<TestLedger>) {
    let ledger = Arc::new(TestLedger::default());
    let scope = MemoryScope::new(Arc::new(TestScope {
        ledger: Arc::clone(&ledger),
        deny,
    }));
    (scope, ledger)
}

fn required<T, E: Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(Box::new(format!("fixture failed: {error:?}"))),
    }
}

struct RawBytes<'a>(&'a [u8]);

impl Serialize for RawBytes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

struct ChangingScalar {
    calls: AtomicUsize,
    second: u64,
}

impl Serialize for ChangingScalar {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let pass = self.calls.fetch_add(1, Ordering::SeqCst);
        serializer.serialize_u64(if pass == 0 { 7 } else { self.second })
    }
}

#[test]
fn scalar_wire_is_raw_cbor_and_exact_retained_charge_refunds_on_drop() {
    let (scope, ledger) = test_scope(false);
    let (wire, lease) = required(encode_cbor_payload_admitted_v1(
        &7_u8,
        &scope,
        1,
        &mut || Ok::<(), ()>(()),
    ));
    assert_eq!(wire, [0x07]);
    assert_eq!(wire.capacity(), 1);
    assert_eq!(lease.bytes(), 1);
    assert_eq!(ledger.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 1);
    drop(wire);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 1);
    drop(lease);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.reserved.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.released.load(Ordering::SeqCst), 1);
}

#[test]
fn native_slots_keep_exact_wire_and_charge_until_last_drop() {
    let (scope, ledger) = test_scope(false);
    let mut wire = Vec::new();
    let mut lease = None;
    required(encode_cbor_payload_into_slots_v1(
        &7_u8,
        &scope,
        1,
        &mut || Ok::<(), ()>(()),
        &mut wire,
        &mut lease,
    ));
    assert_eq!(wire, [0x07]);
    assert_eq!(lease.as_ref().map(MemoryLease::bytes), Some(1));
    assert_eq!(ledger.live.load(Ordering::SeqCst), 1);
    drop(wire);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 1);
    drop(lease);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.released.load(Ordering::SeqCst), 1);
}

#[test]
fn native_slots_retain_partial_encode_and_lease_after_progress_refusal() {
    let (scope, ledger) = test_scope(false);
    let body = vec![0xa5; 2 * 64 * 1024];
    let expected_wire_len = required(u64::try_from(body.len() + 5));
    let mut wire = Vec::new();
    let mut lease = None;
    let mut after_reservation = 0_usize;
    let result = encode_cbor_payload_into_slots_v1(
        &RawBytes(&body),
        &scope,
        expected_wire_len,
        &mut || {
            if ledger.calls.load(Ordering::SeqCst) > 0 {
                after_reservation += 1;
                if after_reservation == 2 {
                    return Err("write-refused");
                }
            }
            Ok(())
        },
        &mut wire,
        &mut lease,
    );
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Progress("write-refused"))
    ));
    assert!(!wire.is_empty());
    assert!(wire.len() < body.len() + 5);
    assert_eq!(
        lease.as_ref().map(MemoryLease::bytes),
        Some(expected_wire_len)
    );
    assert_eq!(ledger.live.load(Ordering::SeqCst), expected_wire_len);
    drop(wire);
    assert_eq!(ledger.live.load(Ordering::SeqCst), expected_wire_len);
    drop(lease);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.released.load(Ordering::SeqCst), expected_wire_len);
}

#[test]
fn native_slots_refuse_replacement_without_touching_existing_custody() {
    let (scope, ledger) = test_scope(false);
    let mut wire = vec![0xaa];
    let mut lease = None;
    let result = encode_cbor_payload_into_slots_v1(
        &7_u8,
        &scope,
        1,
        &mut || Ok::<(), ()>(()),
        &mut wire,
        &mut lease,
    );
    assert!(matches!(result, Err(CborPayloadAdmissionErrorV1::Codec(_))));
    assert_eq!(wire, [0xaa]);
    assert!(lease.is_none());
    assert_eq!(ledger.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn raw_artifact_payload_exceeds_single_frame_without_transport_prefix() {
    const BODY_BYTES: usize = 17 * 1024 * 1024;
    const WIRE_BYTES: usize = BODY_BYTES + 5;
    assert!(BODY_BYTES > MAX_FRAME_BODY_BYTES);
    let body = vec![0xa5; BODY_BYTES];
    let (scope, ledger) = test_scope(false);
    let wire_len = required(u64::try_from(WIRE_BYTES));
    let (wire, lease) = required(encode_cbor_payload_admitted_v1(
        &RawBytes(&body),
        &scope,
        wire_len,
        &mut || Ok::<(), ()>(()),
    ));
    let expected_header = [0x5a, 0x01, 0x10, 0x00, 0x00];
    assert_eq!(wire.get(..5), Some(expected_header.as_slice()));
    assert_eq!(wire.get(5..), Some(body.as_slice()));
    assert_eq!(required(u64::try_from(wire.len())), wire_len);
    assert_eq!(lease.bytes(), wire_len);
    assert_eq!(ledger.live.load(Ordering::SeqCst), wire_len);
    drop(wire);
    drop(lease);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.released.load(Ordering::SeqCst), wire_len);
}

#[test]
fn admission_denial_stops_before_output_serialization() {
    let (scope, ledger) = test_scope(true);
    let value = ChangingScalar {
        calls: AtomicUsize::new(0),
        second: 8,
    };
    let result = encode_cbor_payload_admitted_v1(&value, &scope, 16, &mut || Ok::<(), ()>(()));
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Admission(
            IndexError::ResourceLimited
        ))
    ));
    assert_eq!(value.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
}

#[test]
fn count_pass_progress_refusal_preserves_cause_and_never_reserves() {
    let (scope, ledger) = test_scope(false);
    let value = ChangingScalar {
        calls: AtomicUsize::new(0),
        second: 8,
    };
    let mut checkpoints = 0_u32;
    let result = encode_cbor_payload_admitted_v1(&value, &scope, 16, &mut || {
        checkpoints = checkpoints.saturating_add(1);
        if checkpoints == 2 {
            Err("count-refused")
        } else {
            Ok(())
        }
    });
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Progress("count-refused"))
    ));
    assert_eq!(value.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.calls.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
}

#[test]
fn write_pass_progress_refusal_preserves_cause_and_refunds() {
    let (scope, ledger) = test_scope(false);
    let value = ChangingScalar {
        calls: AtomicUsize::new(0),
        second: 7,
    };
    let result = encode_cbor_payload_admitted_v1(&value, &scope, 16, &mut || {
        if ledger.calls.load(Ordering::SeqCst) == 0 {
            Ok(())
        } else {
            Err("write-refused")
        }
    });
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Progress("write-refused"))
    ));
    assert_eq!(value.calls.load(Ordering::SeqCst), 2);
    assert_eq!(ledger.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.reserved.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.released.load(Ordering::SeqCst), 1);
}

#[test]
fn changed_serialized_length_refuses_after_exact_charge_refund() {
    let (scope, ledger) = test_scope(false);
    let value = ChangingScalar {
        calls: AtomicUsize::new(0),
        second: 1000,
    };
    let result = encode_cbor_payload_admitted_v1(&value, &scope, 16, &mut || Ok::<(), ()>(()));
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(_)))
    ));
    assert_eq!(value.calls.load(Ordering::SeqCst), 2);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.reserved.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.released.load(Ordering::SeqCst), 1);
}

#[test]
fn same_length_but_changed_cbor_value_refuses_after_refund() {
    let (scope, ledger) = test_scope(false);
    let value = ChangingScalar {
        calls: AtomicUsize::new(0),
        second: 8,
    };
    let result = encode_cbor_payload_admitted_v1(&value, &scope, 16, &mut || Ok::<(), ()>(()));
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(_)))
    ));
    assert_eq!(value.calls.load(Ordering::SeqCst), 2);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.reserved.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.released.load(Ordering::SeqCst), 1);
}

#[test]
fn owner_artifact_cap_refuses_in_count_pass_before_output_birth() {
    let (scope, ledger) = test_scope(false);
    let value = ChangingScalar {
        calls: AtomicUsize::new(0),
        second: 8,
    };
    let result = encode_cbor_payload_admitted_v1(&value, &scope, 0, &mut || Ok::<(), ()>(()));
    assert!(matches!(
        result,
        Err(CborPayloadAdmissionErrorV1::Codec(IpcError::Encode(_)))
    ));
    assert_eq!(value.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.calls.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
}
