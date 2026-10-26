use std::cell::Cell;

use quanta_index_contract_base::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};
use serde::de::DeserializeSeed;

use super::ChunkStructuralMetadata;

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Tests the DTO policy only. This fixture does not issue Original memory grants
// or fund parser scratch; product custody belongs to the Runtime owner.
struct TestAdmission {
    refuse_copy: bool,
    copied: Cell<usize>,
    aborted: Cell<bool>,
    attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
}

impl DecodeBackingAdmissionV1 for TestAdmission {
    fn abort_decode(&self) {
        self.aborted.set(true);

        self.attempt.borrow_mut().abort_v1();
    }

    fn enter_decode_field_v1(
        &self,
        kind: quanta_index_contract_base::DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<quanta_index_contract_base::DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        let mut attempt = self.attempt.borrow_mut();
        if !attempt.is_reading_v1() {
            attempt.begin_decode_v1()?;
        }
        attempt.enter_field_v1(kind, maximum)
    }

    fn leave_decode_field_v1(
        &self,
        token: quanta_index_contract_base::DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.attempt.borrow_mut().leave_field_v1(token)
    }

    fn reserve_text(
        &self,
        _destination: &mut String,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn reserve_bytes(
        &self,
        _destination: &mut Vec<u8>,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn reserve_elements<T>(
        &self,
        _destination: &mut Vec<T>,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        if self.refuse_copy {
            return Err(DecodeBackingRefusalV1);
        }
        self.copied.set(self.copied.get().saturating_add(1));
        Ok(source.into())
    }

    fn adopt_parser_text<E>(
        &self,
        _source: &mut Option<String>,
        _validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
        Err(DecodeBackingRefusalV1.into())
    }

    fn copy_boxed_parser_text<E>(
        &self,
        _source: &mut Option<String>,
        _validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, crate::ParserTextInputErrorV1<E>> {
        Err(DecodeBackingRefusalV1.into())
    }

    fn verify_parser_owned_bytes(
        &self,
        _source: &[u8],
        _capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }
}

fn decode_admitted(
    wire: &str,
    owner: &TestAdmission,
    max_text_bytes: usize,
) -> Result<ChunkStructuralMetadata, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(wire);
    let value = ChunkStructuralMetadata::deserialize_seed_with_admission_v1(owner, max_text_bytes)
        .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

const FULL: &str = r#"{"variant_tag":"node","structural_kind_tag":"function","structural_pattern_kind":"definition","structural_name":"main","structural_scope":"crate","structural_matched_node":"fn main"}"#;

#[test]
fn admitted_structural_fields_match_fixed_public_wire() -> TestResult {
    let owner = TestAdmission {
        refuse_copy: false,
        copied: Cell::new(0),
        aborted: Cell::new(false),
        attempt: std::cell::RefCell::new(
            quanta_index_contract_base::DecodeAttemptStateV1::default(),
        ),
    };
    let admitted = decode_admitted(FULL, &owner, 32)?;
    let ordinary: ChunkStructuralMetadata = serde_json::from_str(FULL)?;
    if admitted != ordinary || admitted.structural_matched_node.as_ref() != "fn main" {
        return Err("admitted structural metadata differs from fixed wire".into());
    }
    if owner.copied.get() != 6 || owner.aborted.get() {
        return Err("six admitted boxed fields were not copied exactly once".into());
    }
    Ok(())
}

#[test]
fn admitted_structural_refusal_precedes_first_box_copy_and_aborts() -> TestResult {
    let owner = TestAdmission {
        refuse_copy: true,
        copied: Cell::new(0),
        aborted: Cell::new(false),
        attempt: std::cell::RefCell::new(
            quanta_index_contract_base::DecodeAttemptStateV1::default(),
        ),
    };
    if decode_admitted(FULL, &owner, 32).is_ok() || owner.copied.get() != 0 || !owner.aborted.get()
    {
        return Err("backing denial did not stop the first boxed value and abort decode".into());
    }
    Ok(())
}

#[test]
fn admitted_structural_missing_duplicate_unknown_and_oversized_refuse() -> TestResult {
    const MISSING: &str = r#"{"variant_tag":"node","structural_kind_tag":"function","structural_pattern_kind":"definition","structural_name":"main","structural_scope":"crate"}"#;
    const DUPLICATE: &str = r#"{"variant_tag":"node","variant_tag":"node","structural_kind_tag":"function","structural_pattern_kind":"definition","structural_name":"main","structural_scope":"crate","structural_matched_node":"fn main"}"#;
    const UNKNOWN: &str = r#"{"variant_tag":"node","alien":"x","structural_kind_tag":"function","structural_pattern_kind":"definition","structural_name":"main","structural_scope":"crate","structural_matched_node":"fn main"}"#;
    for wire in [MISSING, DUPLICATE, UNKNOWN] {
        let owner = TestAdmission {
            refuse_copy: false,
            copied: Cell::new(0),
            aborted: Cell::new(false),
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        if decode_admitted(wire, &owner, 32).is_ok() || !owner.aborted.get() {
            return Err("malformed structural wire did not refuse and abort".into());
        }
    }
    let owner = TestAdmission {
        refuse_copy: false,
        copied: Cell::new(0),
        aborted: Cell::new(false),
        attempt: std::cell::RefCell::new(
            quanta_index_contract_base::DecodeAttemptStateV1::default(),
        ),
    };
    if decode_admitted(FULL, &owner, 3).is_ok() || owner.copied.get() != 0 || !owner.aborted.get() {
        return Err("byte limit did not refuse before the first boxed copy".into());
    }
    Ok(())
}
