//! Typed text seeds share one byte bound and the caller's canonical predicate.

use core::fmt;

use serde::de::{self, DeserializeSeed, Visitor};

use super::{
    DecodeBackingAdmissionV1, DecodeFieldGuardV1, DecodeFieldKindV1, ParserTextInputErrorV1,
};

fn input_error<E: de::Error>(error: ParserTextInputErrorV1<E>) -> E {
    match error {
        ParserTextInputErrorV1::Admission(cause) => E::custom(cause),
        ParserTextInputErrorV1::Validation(cause) => cause,
    }
}

type TextValidator = fn(&str) -> Result<(), &'static str>;

struct TextInput<'a, A: ?Sized> {
    admission: &'a A,
    max_bytes: usize,
    validator: Option<TextValidator>,
}

impl<A: ?Sized> TextInput<'_, A> {
    fn validate<E: de::Error>(&self, value: &str) -> Result<(), E> {
        if value.len() > self.max_bytes {
            return Err(E::custom("admitted text exceeds its byte limit"));
        }
        if let Some(validate) = self.validator {
            validate(value).map_err(E::custom)?;
        }
        Ok(())
    }
}

/// Decode an existing String field with its backing admitted before birth.
///
/// Owned visitor inputs must come from a parser funded by this same admission
/// owner. Borrowed input is copied once after admission; owned parser input is
/// adopted without copying. The owner must outlive the returned String.
pub struct AdmittedTextSeedV1<'a, A: ?Sized>(TextInput<'a, A>);

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized> AdmittedTextSeedV1<'a, A> {
    pub fn new(admission: &'a A, max_bytes: usize) -> Self {
        Self(TextInput {
            admission,
            max_bytes,
            validator: None,
        })
    }

    /// Use the existing DTO predicate before any destination allocation.
    pub fn new_validated(admission: &'a A, max_bytes: usize, validator: TextValidator) -> Self {
        Self(TextInput {
            admission,
            max_bytes,
            validator: Some(validator),
        })
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for AdmittedTextSeedV1<'_, A> {
    type Value = String;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let field =
            DecodeFieldGuardV1::enter(self.0.admission, DecodeFieldKindV1::Text, self.0.max_bytes)?;
        let result = decoder.deserialize_string(self);
        field.finish(result)
    }
}

impl<A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'_> for AdmittedTextSeedV1<'_, A> {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded admitted text string")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.0.validate(value)?;
        let mut output = String::new();
        self.0
            .admission
            .reserve_text(&mut output, value.len())
            .map_err(E::custom)?;
        if output.capacity() < value.len() {
            return Err(E::custom("admitted text backing is smaller than its input"));
        }
        output.push_str(value);
        Ok(output)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        let mut input = Some(value);
        self.0
            .admission
            .adopt_parser_text(&mut input, |value| self.0.validate(value))
            .map_err(input_error)
    }
}

/// Decode an existing `Box<str>` field without `String::into_boxed_str` shrinking
/// an oversized parser backing outside admission.
pub struct AdmittedBoxedTextSeedV1<'a, A: ?Sized>(TextInput<'a, A>);

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized> AdmittedBoxedTextSeedV1<'a, A> {
    pub fn new(admission: &'a A, max_bytes: usize) -> Self {
        Self(TextInput {
            admission,
            max_bytes,
            validator: None,
        })
    }

    /// Use the existing DTO predicate before any destination allocation.
    pub fn new_validated(admission: &'a A, max_bytes: usize, validator: TextValidator) -> Self {
        Self(TextInput {
            admission,
            max_bytes,
            validator: Some(validator),
        })
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedBoxedTextSeedV1<'_, A>
{
    type Value = Box<str>;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let field =
            DecodeFieldGuardV1::enter(self.0.admission, DecodeFieldKindV1::Text, self.0.max_bytes)?;
        let result = decoder.deserialize_string(self);
        field.finish(result)
    }
}

impl<A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'_> for AdmittedBoxedTextSeedV1<'_, A> {
    type Value = Box<str>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded admitted boxed text string")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.0.validate(value)?;
        self.0.admission.copy_boxed_text(value).map_err(E::custom)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        let mut input = Some(value);
        self.0
            .admission
            .copy_boxed_parser_text(&mut input, |value| self.0.validate(value))
            .map_err(input_error)
    }
}

#[cfg(test)]
mod tests;
