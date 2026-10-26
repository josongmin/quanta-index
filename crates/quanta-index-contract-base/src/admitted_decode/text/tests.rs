use std::cell::Cell;

use serde::de::{
    DeserializeSeed,
    value::{Error, StrDeserializer, StringDeserializer},
};

use super::{AdmittedBoxedTextSeedV1, AdmittedTextSeedV1};
use crate::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A dispatch/custody test double, not a production memory-budget proof.
#[derive(Default)]
struct TextAdmission {
    calls: Cell<usize>,
    parser_backing: Cell<Option<(usize, usize)>>,
    deny: bool,
    undersized: bool,
    aborted: Cell<bool>,
    attempt: std::cell::RefCell<crate::DecodeAttemptStateV1>,
}

impl TextAdmission {
    fn checkpoint(&self) -> Result<(), DecodeBackingRefusalV1> {
        self.calls.set(self.calls.get().saturating_add(1));
        if self.deny || self.aborted.get() {
            Err(DecodeBackingRefusalV1)
        } else {
            Ok(())
        }
    }

    fn check_parser(&self, text: &str, capacity: usize) -> Result<(), DecodeBackingRefusalV1> {
        self.checkpoint()?;
        if self.parser_backing.get() == Some((text.as_ptr().addr(), capacity)) {
            Ok(())
        } else {
            Err(DecodeBackingRefusalV1)
        }
    }
}

impl DecodeBackingAdmissionV1 for TextAdmission {
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
        self.checkpoint()?;
        if !self.undersized {
            destination
                .try_reserve_exact(additional)
                .map_err(|_allocation_refusal| DecodeBackingRefusalV1)?;
        }
        Ok(())
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
        self.checkpoint()?;
        Ok(source.into())
    }

    fn adopt_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
        let value = source
            .as_ref()
            .ok_or(crate::ParserTextInputErrorV1::Admission(
                DecodeBackingRefusalV1,
            ))?;
        self.check_parser(value, value.capacity())?;
        validate(value).map_err(crate::ParserTextInputErrorV1::Validation)?;
        source
            .take()
            .ok_or(crate::ParserTextInputErrorV1::Admission(
                DecodeBackingRefusalV1,
            ))
    }

    fn copy_boxed_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, crate::ParserTextInputErrorV1<E>> {
        let value = source
            .as_ref()
            .ok_or(crate::ParserTextInputErrorV1::Admission(
                DecodeBackingRefusalV1,
            ))?;
        self.check_parser(value, value.capacity())?;
        validate(value).map_err(crate::ParserTextInputErrorV1::Validation)?;
        let boxed = value.as_str().into();
        let value = source
            .take()
            .ok_or(crate::ParserTextInputErrorV1::Admission(
                DecodeBackingRefusalV1,
            ))?;
        self.parser_backing.set(None);
        drop(value);
        Ok(boxed)
    }

    fn verify_parser_owned_bytes(
        &self,
        _source: &[u8],
        _capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }
}

fn require_rust(value: &str) -> Result<(), &'static str> {
    if value == "rust" {
        Ok(())
    } else {
        Err("fixed language oracle refused")
    }
}

#[test]
fn borrowed_utf8_preserves_value_with_exact_byte_limit() -> TestResult {
    let admission = TextAdmission::default();
    let input = "é界";
    let text =
        AdmittedTextSeedV1::new(&admission, 5).deserialize(StrDeserializer::<Error>::new(input))?;
    if text != input || admission.calls.get() != 1 {
        return Err("borrowed UTF-8 value or admission dispatch changed".into());
    }
    let boxed = AdmittedBoxedTextSeedV1::new(&admission, 5)
        .deserialize(StrDeserializer::<Error>::new(input))?;
    if boxed.as_ref() != input || admission.calls.get() != 2 {
        return Err("boxed UTF-8 value or admission dispatch changed".into());
    }
    Ok(())
}

#[test]
fn byte_limit_and_canonical_predicate_refuse_before_destination_birth() -> TestResult {
    let admission = TextAdmission::default();
    let byte_error =
        AdmittedTextSeedV1::new(&admission, 1).deserialize(StrDeserializer::<Error>::new("é"));
    let predicate_error = AdmittedBoxedTextSeedV1::new_validated(&admission, 16, require_rust)
        .deserialize(StrDeserializer::<Error>::new("Rust"));
    if byte_error.is_ok() || predicate_error.is_ok() || admission.calls.get() != 0 {
        return Err("invalid byte bound or predicate entered the allocation owner".into());
    }
    if !admission.aborted.get() {
        return Err("text refusal did not terminate the shared attempt".into());
    }
    let fresh_admission = TextAdmission::default();
    let accepted = AdmittedTextSeedV1::new_validated(&fresh_admission, 4, require_rust)
        .deserialize(StrDeserializer::<Error>::new("rust"))?;
    if accepted != "rust" || fresh_admission.calls.get() != 1 {
        return Err("same canonical predicate did not accept the fixed value".into());
    }
    Ok(())
}

#[test]
fn parser_owned_string_adoption_preserves_original_pointer_and_capacity() -> TestResult {
    let admission = TextAdmission::default();
    let mut input = String::new();
    input.try_reserve_exact(256)?;
    input.push_str("rust");
    let backing = (input.as_ptr().addr(), input.capacity());
    admission.parser_backing.set(Some(backing));
    let text = AdmittedTextSeedV1::new(&admission, 4)
        .deserialize(StringDeserializer::<Error>::new(input))?;
    if text != "rust" || (text.as_ptr().addr(), text.capacity()) != backing {
        return Err("parser-owned String was cloned or shrunk on adoption".into());
    }
    Ok(())
}

#[test]
fn parser_owned_box_conversion_consumes_the_live_input() -> TestResult {
    let admission = TextAdmission::default();
    let mut input = String::new();
    input.try_reserve_exact(256)?;
    input.push_str("rust");
    admission
        .parser_backing
        .set(Some((input.as_ptr().addr(), input.capacity())));
    let boxed = AdmittedBoxedTextSeedV1::new_validated(&admission, 4, require_rust)
        .deserialize(StringDeserializer::<Error>::new(input))?;
    if boxed.as_ref() != "rust"
        || admission.parser_backing.get().is_some()
        || admission.calls.get() != 1
    {
        return Err("boxed text did not use the owner-consuming parser route".into());
    }
    Ok(())
}

#[test]
fn unadmitted_parser_owned_input_is_not_accepted_by_either_seed() -> TestResult {
    let text_admission = TextAdmission::default();
    let text = AdmittedTextSeedV1::new(&text_admission, 4)
        .deserialize(StringDeserializer::<Error>::new("rust".into()));
    if text.is_ok() || text_admission.calls.get() != 1 || !text_admission.aborted.get() {
        return Err("unfunded owned String was accepted by the text seed".into());
    }

    let reused = AdmittedBoxedTextSeedV1::new(&text_admission, 4)
        .deserialize(StringDeserializer::<Error>::new("rust".into()));
    if reused.is_ok() || text_admission.calls.get() != 1 {
        return Err("aborted text attempt reached parser backing on reuse".into());
    }

    let boxed_admission = TextAdmission::default();
    let boxed = AdmittedBoxedTextSeedV1::new(&boxed_admission, 4)
        .deserialize(StringDeserializer::<Error>::new("rust".into()));
    if boxed.is_ok() || boxed_admission.calls.get() != 1 || !boxed_admission.aborted.get() {
        return Err("unfunded owned String was accepted by the boxed seed".into());
    }
    Ok(())
}

#[test]
fn refusal_and_under_reserved_backing_cannot_trigger_hidden_string_growth() -> TestResult {
    for admission in [
        TextAdmission {
            deny: true,
            ..TextAdmission::default()
        },
        TextAdmission {
            undersized: true,
            ..TextAdmission::default()
        },
    ] {
        let text = AdmittedTextSeedV1::new(&admission, 4)
            .deserialize(StrDeserializer::<Error>::new("rust"));
        if text.is_ok() || admission.calls.get() != 1 {
            return Err("text copied after refusal or insufficient reserved capacity".into());
        }
    }
    Ok(())
}
