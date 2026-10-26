//! Allocation admission shared by typed decoders without naming a runtime ledger.
//!
//! An adapter must retain every successful backing grant alongside the decoded
//! value. This module neither creates an execution authority nor funds parser
//! temporary buffers; the caller must supply a parser with its own admission.

use core::fmt;
use core::marker::PhantomData;

use serde::de::{self, DeserializeSeed, SeqAccess, Visitor};

/// Finite decode refusal signal. The allocation owner retains the authentic
/// memory, work, cancellation, or backend cause for the outer caller to read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeBackingRefusalV1;

impl fmt::Display for DecodeBackingRefusalV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("decoded backing admission refused")
    }
}

impl std::error::Error for DecodeBackingRefusalV1 {}

mod attempt;
pub use attempt::{DecodeAttemptStateV1, DecodeFieldKindV1, DecodeFieldTokenV1};

/// The owner keeps the consumed parser input even on predicate failure.
#[derive(Debug, PartialEq, Eq)]
pub enum ParserTextInputErrorV1<E> {
    Admission(DecodeBackingRefusalV1),
    Validation(E),
}

impl<E> From<DecodeBackingRefusalV1> for ParserTextInputErrorV1<E> {
    fn from(cause: DecodeBackingRefusalV1) -> Self {
        Self::Admission(cause)
    }
}

/// Admit owned decode backings before they are created or grown.
///
/// Implementations must charge the requested live capacity (including their
/// own lease metadata) before attempting growth. For a reallocation, the old
/// and new charges must overlap until growth finishes. An implementation must
/// refuse if the resulting capacity exceeds the prepaid capacity. The caller
/// must retain this admission owner for at least as long as every returned
/// decoded value; none of these methods transfers a grant to a bare DTO.
///
/// Parser-owned text and byte buffers require a separate parser allocation
/// hook using the same owner. `verify_parser_owned_bytes` checks the actual
/// parser backing before an owned byte buffer is adopted without a copy.
/// A decode error terminates this owner's attempt: callers must drop all partial
/// values before the owner and must not resume decoding with stale backing rows.
pub trait DecodeBackingAdmissionV1 {
    /// Mark the shared decode attempt terminal without refunding live grants.
    /// This hook must not allocate, fail, or replace the authentic refusal.
    /// All clones must refuse further backing operations after it returns.
    fn abort_decode(&self);

    /// Enter this attempt's LIFO field bound before invoking the parser.
    /// This is an owner-issued scope token, not an allocation/root authority.
    fn enter_decode_field_v1(
        &self,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<DecodeFieldTokenV1, DecodeBackingRefusalV1>;

    /// Leave exactly the corresponding field. Failure poisons the attempt.
    fn leave_decode_field_v1(
        &self,
        token: DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1>;

    fn reserve_text(
        &self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1>;

    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1>;

    /// Admit a `Vec<T>` backing before it grows. Zero-sized `T` has no backing,
    /// but the typed caller still enforces its element-count limit.
    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1>;

    /// Allocate an exact boxed copy while retaining the source String's grant.
    /// `String::into_boxed_str` can shrink an oversized backing by reallocating.
    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1>;

    /// Adopt the same parser-funded allocation after checking its live pointer,
    /// capacity and layout. An ordinary unadmitted String must be refused.
    fn adopt_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, ParserTextInputErrorV1<E>>;

    /// Copy a live parser-funded String into an exact box. The implementation
    /// retains both grants during the copy, removes the consumed parser backing
    /// from its provenance bank, then drops the String before refunding its
    /// grant. Pointer-only retirement after the String has died is invalid.
    fn copy_boxed_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, ParserTextInputErrorV1<E>>;

    fn verify_parser_owned_bytes(
        &self,
        source: &[u8],
        capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1>;
}

mod text;
pub use text::{AdmittedBoxedTextSeedV1, AdmittedTextSeedV1};
mod identity;
pub use identity::SourceIdentityDecodeAdmissionV1;
mod bytes;
pub use bytes::AdmittedByteStringSeedV1;
mod option;
pub use option::AdmittedOptionalSeedV1;

/// A field scope always precedes parser entry. Unwinding closes the whole
/// attempt without replacing its original refusal or releasing live grants.
pub struct DecodeFieldGuardV1<'a, A: DecodeBackingAdmissionV1 + ?Sized> {
    admission: &'a A,
    token: Option<DecodeFieldTokenV1>,
}

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized> DecodeFieldGuardV1<'a, A> {
    pub fn enter<E: de::Error>(
        admission: &'a A,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<Self, E> {
        let token = admission
            .enter_decode_field_v1(kind, maximum)
            .map_err(|cause| {
                admission.abort_decode();
                E::custom(cause)
            })?;
        Ok(Self {
            admission,
            token: Some(token),
        })
    }

    pub fn finish<T, E: de::Error>(mut self, result: Result<T, E>) -> Result<T, E> {
        let token = self.token.take();
        match result {
            Err(cause) => {
                self.admission.abort_decode();
                Err(cause)
            }
            Ok(value) => {
                let token = token.ok_or_else(|| E::custom(DecodeBackingRefusalV1))?;
                self.admission
                    .leave_decode_field_v1(token)
                    .map_err(|cause| {
                        self.admission.abort_decode();
                        E::custom(cause)
                    })?;
                Ok(value)
            }
        }
    }
}

impl<A: DecodeBackingAdmissionV1 + ?Sized> Drop for DecodeFieldGuardV1<'_, A> {
    fn drop(&mut self) {
        if self.token.is_some() {
            self.admission.abort_decode();
        }
    }
}

/// Decode one bounded sequence into the existing `Vec<T>` DTO field.
///
/// A child is decoded before the outer vector is grown. The child's own
/// allocations must be admitted by its seed; on outer refusal it is dropped
/// while the common admission owner remains alive. No allocation is made from
/// an untrusted `size_hint`, and a full sequence is checked for EOF without
/// growing the vector merely to discover that it ended.
pub struct AdmittedSequenceSeedV1<'a, A: ?Sized, F, S> {
    admission: &'a A,
    max_items: usize,
    element_seed: F,
    seed: PhantomData<fn() -> S>,
}

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized, F, S> AdmittedSequenceSeedV1<'a, A, F, S> {
    pub fn new(admission: &'a A, max_items: usize, element_seed: F) -> Self {
        Self {
            admission,
            max_items,
            element_seed,
            seed: PhantomData,
        }
    }
}

struct RefuseExtraElement;

impl<'de> DeserializeSeed<'de> for RefuseExtraElement {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, _decoder: D) -> Result<(), D::Error> {
        Err(de::Error::custom(
            "admitted sequence exceeds its element limit",
        ))
    }
}

impl<'de, A, F, S> DeserializeSeed<'de> for AdmittedSequenceSeedV1<'_, A, F, S>
where
    A: DecodeBackingAdmissionV1 + ?Sized,
    F: FnMut() -> S,
    S: DeserializeSeed<'de>,
{
    type Value = Vec<S::Value>;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let field =
            DecodeFieldGuardV1::enter(self.admission, DecodeFieldKindV1::Sequence, self.max_items)?;
        let result = decoder.deserialize_seq(self);
        field.finish(result)
    }
}

impl<'de, A, F, S> Visitor<'de> for AdmittedSequenceSeedV1<'_, A, F, S>
where
    A: DecodeBackingAdmissionV1 + ?Sized,
    F: FnMut() -> S,
    S: DeserializeSeed<'de>,
{
    type Value = Vec<S::Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded admitted sequence")
    }

    fn visit_seq<Q: SeqAccess<'de>>(mut self, mut sequence: Q) -> Result<Self::Value, Q::Error> {
        let mut output = Vec::new();
        loop {
            if output.len() == self.max_items {
                // The seed refuses before any value visitor is entered. A
                // missing next element is accepted without reserving space.
                if sequence.next_element_seed(RefuseExtraElement)?.is_none() {
                    return Ok(output);
                }
                return Err(de::Error::custom(
                    "admitted sequence exceeds its element limit",
                ));
            }
            let Some(element) = sequence.next_element_seed((self.element_seed)())? else {
                return Ok(output);
            };
            self.admission
                .reserve_elements(&mut output, 1)
                .map_err(de::Error::custom)?;
            if output.len() == output.capacity() {
                return Err(de::Error::custom(
                    "admitted sequence backing has no reserved slot",
                ));
            }
            output.push(element);
        }
    }
}

#[cfg(test)]
mod tests;
