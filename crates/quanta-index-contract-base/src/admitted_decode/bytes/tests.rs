use std::cell::Cell;

use serde::de::{
    DeserializeSeed, Visitor,
    value::{BorrowedBytesDeserializer, Error, SeqDeserializer},
};

use super::AdmittedByteStringSeedV1;
use crate::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct ByteAdmission {
    reserves: Cell<usize>,
    verifies: Cell<usize>,
    parser_backing: Cell<Option<(usize, usize)>>,
    deny: bool,
    undersized: bool,
    aborted: Cell<bool>,
    attempt: std::cell::RefCell<crate::DecodeAttemptStateV1>,
}

impl DecodeBackingAdmissionV1 for ByteAdmission {
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
        _destination: &mut String,
        _additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }

    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.reserves.set(self.reserves.get().saturating_add(1));
        if self.deny || self.aborted.get() {
            return Err(DecodeBackingRefusalV1);
        }
        if !self.undersized {
            destination
                .try_reserve_exact(additional)
                .map_err(|_error| DecodeBackingRefusalV1)?;
        }
        Ok(())
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
        source: &[u8],
        capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.verifies.set(self.verifies.get().saturating_add(1));
        if self.aborted.get()
            || self.parser_backing.get() != Some((source.as_ptr().addr(), capacity))
        {
            return Err(DecodeBackingRefusalV1);
        }
        Ok(())
    }
}

#[test]
fn borrowed_byte_string_admits_before_copy_and_preserves_fixed_bytes() -> TestResult {
    let admission = ByteAdmission::default();
    let input = &[0, 127, 255][..];
    let bytes = AdmittedByteStringSeedV1::new(&admission, 3)
        .deserialize(BorrowedBytesDeserializer::<Error>::new(input))?;
    if bytes != input || admission.reserves.get() != 1 || admission.verifies.get() != 0 {
        return Err("borrowed bytes were not admitted and copied exactly once".into());
    }
    Ok(())
}

#[test]
fn integer_array_wire_preserves_existing_source_bytes_form_and_limit() -> TestResult {
    let parsed: Vec<u8> = ciborium::from_reader(&b"\x83\x00\x18\x7f\x18\xff"[..])?;
    let admission = ByteAdmission::default();
    let bytes =
        AdmittedByteStringSeedV1::new(&admission, 3)
            .deserialize(SeqDeserializer::<_, Error>::new(parsed.into_iter()))?;
    if bytes != [0, 127, 255] || admission.reserves.get() != 3 {
        return Err("admitted integer-array bytes changed the existing wire form".into());
    }

    let over_limit = ByteAdmission::default();
    let rejected =
        AdmittedByteStringSeedV1::new(&over_limit, 2).deserialize(
            SeqDeserializer::<_, Error>::new([0_u8, 127, 255].into_iter()),
        );
    if rejected.is_ok() || over_limit.reserves.get() != 2 || !over_limit.aborted.get() {
        return Err("over-limit array decoded or allocated its third byte".into());
    }
    Ok(())
}

#[test]
fn parser_owned_buffer_requires_provenance_and_moves_without_copy() -> TestResult {
    let admission = ByteAdmission::default();
    let mut input = Vec::new();
    input.try_reserve_exact(256)?;
    input.extend_from_slice(&[1, 2, 3]);
    let backing = (input.as_ptr().addr(), input.capacity());
    admission.parser_backing.set(Some(backing));
    let bytes = AdmittedByteStringSeedV1::new(&admission, 3).visit_byte_buf::<Error>(input)?;
    if bytes != [1, 2, 3]
        || (bytes.as_ptr().addr(), bytes.capacity()) != backing
        || admission.verifies.get() != 1
        || admission.reserves.get() != 0
    {
        return Err("parser-owned bytes were copied or not verified".into());
    }

    let unowned = ByteAdmission::default();
    let rejected =
        AdmittedByteStringSeedV1::new(&unowned, 3).visit_byte_buf::<Error>(vec![1, 2, 3]);
    if rejected.is_ok() || unowned.verifies.get() != 1 {
        return Err("unowned byte buffer was adopted".into());
    }
    let empty = AdmittedByteStringSeedV1::new(&unowned, 0).visit_byte_buf::<Error>(Vec::new())?;
    if !empty.is_empty() || unowned.verifies.get() != 1 {
        return Err("zero-capacity empty bytes required nonexistent parser backing".into());
    }
    Ok(())
}

#[test]
fn limit_refusal_and_insufficient_reservation_prevent_copy() -> TestResult {
    let input = &[1, 2, 3][..];
    let limited = ByteAdmission::default();
    let rejected = AdmittedByteStringSeedV1::new(&limited, 2)
        .deserialize(BorrowedBytesDeserializer::<Error>::new(input));
    if rejected.is_ok() || limited.reserves.get() != 0 || !limited.aborted.get() {
        return Err("byte limit was checked after allocation".into());
    }

    for admission in [
        ByteAdmission {
            deny: true,
            ..ByteAdmission::default()
        },
        ByteAdmission {
            undersized: true,
            ..ByteAdmission::default()
        },
    ] {
        let rejected = AdmittedByteStringSeedV1::new(&admission, 3)
            .deserialize(BorrowedBytesDeserializer::<Error>::new(input));
        if rejected.is_ok() || admission.reserves.get() != 1 || !admission.aborted.get() {
            return Err("refused or under-reserved byte backing was copied".into());
        }
    }
    Ok(())
}

#[test]
fn wrong_wire_kind_terminates_decode_attempt() {
    let admission = ByteAdmission::default();
    let mut decoder = serde_json::Deserializer::from_str("true");
    let result = AdmittedByteStringSeedV1::new(&admission, 32).deserialize(&mut decoder);
    assert!(result.is_err());
    assert!(admission.aborted.get());
}
