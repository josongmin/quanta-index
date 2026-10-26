use std::cell::Cell;

use serde::de::DeserializeSeed;

use super::AdmittedOptionalSeedV1;
use crate::{AdmittedTextSeedV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct OptionalAdmission {
    text_reserves: Cell<usize>,
    aborted: Cell<bool>,
    deny: bool,
    attempt: std::cell::RefCell<crate::DecodeAttemptStateV1>,
}

impl DecodeBackingAdmissionV1 for OptionalAdmission {
    fn abort_decode(&self) {
        self.aborted.set(true);

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
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.text_reserves
            .set(self.text_reserves.get().saturating_add(1));
        if self.deny || self.aborted.get() {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_error| DecodeBackingRefusalV1)
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

fn decode_json(
    source: &str,
    admission: &OptionalAdmission,
) -> Result<Option<String>, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(source);
    AdmittedOptionalSeedV1::new(admission, AdmittedTextSeedV1::new(admission, 8))
        .deserialize(&mut decoder)
}

#[test]
fn none_and_some_use_one_typed_child_without_none_allocation() -> TestResult {
    let admission = OptionalAdmission::default();
    if decode_json("null", &admission)?.is_some() || admission.text_reserves.get() != 0 {
        return Err("None allocated or changed the optional value".into());
    }
    let some = decode_json("\"rust\"", &admission)?;
    if some.as_deref() != Some("rust") || admission.text_reserves.get() != 1 {
        return Err("Some did not use its admitted text child".into());
    }
    Ok(())
}

#[test]
fn child_refusal_and_wrong_kind_abort_shared_attempt() {
    let denied = OptionalAdmission {
        deny: true,
        ..OptionalAdmission::default()
    };
    assert!(decode_json("\"rust\"", &denied).is_err());
    assert!(denied.aborted.get());
    assert_eq!(denied.text_reserves.get(), 1);

    let wrong_kind = OptionalAdmission::default();
    assert!(decode_json("[1]", &wrong_kind).is_err());
    assert!(wrong_kind.aborted.get());
    assert_eq!(wrong_kind.text_reserves.get(), 0);
}
