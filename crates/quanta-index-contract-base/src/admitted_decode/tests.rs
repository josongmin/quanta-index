use super::{AdmittedSequenceSeedV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1};
use std::cell::Cell;

use serde::{
    Deserialize,
    de::{DeserializeSeed, value::SeqDeserializer},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct SequenceAdmission {
    reserve_calls: Cell<usize>,
    refuse: bool,
    attempt: std::cell::RefCell<crate::DecodeAttemptStateV1>,
}

impl DecodeBackingAdmissionV1 for SequenceAdmission {
    fn abort_decode(&self) {
        self.attempt.borrow_mut().abort_v1();
    }

    fn enter_decode_field_v1(
        &self,
        kind: crate::DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<crate::DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        let mut attempt = self.attempt.borrow_mut();
        if !attempt.is_reading_v1() {
            attempt.begin_decode_v1()?;
        }
        attempt.enter_field_v1(kind, maximum)
    }

    fn leave_decode_field_v1(
        &self,
        token: crate::DecodeFieldTokenV1,
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
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.reserve_calls
            .set(self.reserve_calls.get().saturating_add(1));
        if self.refuse {
            return Err(DecodeBackingRefusalV1);
        }
        if destination.try_reserve_exact(additional).is_err() {
            return Err(DecodeBackingRefusalV1);
        }
        Ok(())
    }

    fn copy_boxed_text(&self, _source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
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

struct ByteSeed<'a>(&'a Cell<usize>);

impl<'de> DeserializeSeed<'de> for ByteSeed<'_> {
    type Value = u8;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<u8, D::Error> {
        self.0.set(self.0.get().saturating_add(1));
        u8::deserialize(decoder)
    }
}

/// Unit tests exercise the seed against an existing CBOR-decoded value stream.
/// Parser allocation admission is a separate IPC integration contract.
fn decode_cbor_sequence(
    cbor: &[u8],
    admission: &SequenceAdmission,
    max_items: usize,
    child_calls: &Cell<usize>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let elements: Vec<u8> = ciborium::from_reader(cbor)?;
    let decoder = SeqDeserializer::<_, serde::de::value::Error>::new(elements.into_iter());
    Ok(
        AdmittedSequenceSeedV1::new(admission, max_items, || ByteSeed(child_calls))
            .deserialize(decoder)?,
    )
}

#[test]
fn fixed_cbor_sequence_and_empty_sequence_preserve_values_without_hint_reserve() -> TestResult {
    let admission = SequenceAdmission::default();
    let child_calls = Cell::new(0);
    let decoded = decode_cbor_sequence(&[0x82, 1, 2], &admission, 2, &child_calls)?;
    if decoded != [1, 2] || admission.reserve_calls.get() != 2 || child_calls.get() != 2 {
        return Err("fixed CBOR sequence was not decoded and admitted once per child".into());
    }

    let empty_admission = SequenceAdmission::default();
    let empty_calls = Cell::new(0);
    let empty = decode_cbor_sequence(&[0x80], &empty_admission, 0, &empty_calls)?;
    if !empty.is_empty() || empty_admission.reserve_calls.get() != 0 || empty_calls.get() != 0 {
        return Err("empty CBOR sequence allocated or decoded an element".into());
    }
    Ok(())
}

#[test]
fn over_limit_refuses_before_decoding_or_reserving_extra_child() -> TestResult {
    let admission = SequenceAdmission::default();
    let child_calls = Cell::new(0);
    let result = decode_cbor_sequence(&[0x82, 1, 2], &admission, 1, &child_calls);
    if result.is_ok() || child_calls.get() != 1 || admission.reserve_calls.get() != 1 {
        return Err("extra CBOR child was decoded or admitted before limit refusal".into());
    }
    Ok(())
}

#[test]
fn outer_admission_refusal_occurs_after_typed_child_but_before_push() -> TestResult {
    let admission = SequenceAdmission {
        reserve_calls: Cell::new(0),
        refuse: true,
        attempt: std::cell::RefCell::new(crate::DecodeAttemptStateV1::default()),
    };
    let child_calls = Cell::new(0);
    let result = decode_cbor_sequence(&[0x81, 7], &admission, 1, &child_calls);
    if result.is_ok() || child_calls.get() != 1 || admission.reserve_calls.get() != 1 {
        return Err("outer refusal did not preserve typed child-before-push order".into());
    }
    Ok(())
}

#[test]
fn typed_child_failure_does_not_reserve_outer_vector() -> TestResult {
    let admission = SequenceAdmission::default();
    let child_calls = Cell::new(0);
    let values = std::iter::once("invalid");
    let decoder = SeqDeserializer::<_, serde::de::value::Error>::new(values);
    let result =
        AdmittedSequenceSeedV1::new(&admission, 1, || ByteSeed(&child_calls)).deserialize(decoder);
    if result.is_ok() || child_calls.get() != 1 || admission.reserve_calls.get() != 0 {
        return Err("typed child failure incorrectly reserved an outer vector".into());
    }
    Ok(())
}
