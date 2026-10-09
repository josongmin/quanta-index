//! Core ID newtypes shared between the producer and the search-plane.

use core::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};
use unicode_normalization::UnicodeNormalization as _;

#[cfg(test)]
mod native_copy_retained_tests_v1;

#[cfg(feature = "quanta-native-identity-v1")]
mod native_manifest_decode_v1;
#[cfg(feature = "quanta-native-identity-v1")]
pub use native_manifest_decode_v1::NativeManifestGenerationDecodeDataV1;
#[cfg(feature = "quanta-native-identity-v1")]
mod native_decode_data_v1;
#[cfg(feature = "quanta-native-identity-v1")]
pub use native_decode_data_v1::{
    NativeIdentityDecodeDataRefusalV1, NativeIdentityDecodeDataV1, NativeIdentityDecodeLoanV1,
    NativeIdentityDecodeRunnerV1,
};

const IDENTITY_MAX_UTF8_BYTES: usize = 512;
const REPOSITORY_REVISION_DOMAIN: &str = "quanta-index/repository-revision/v1";
const LOGICAL_GENERATION_DOMAIN: &str = "quanta-index/logical-generation/v1";

/// Why a repository or revision identifier is not a canonical product ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityValidationErrorV1 {
    Empty,
    TooLong,
    ControlCharacter,
    NonCanonical,
}

impl IdentityValidationErrorV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Empty => "IDENTITY_EMPTY",
            Self::TooLong => "IDENTITY_TOO_LONG",
            Self::ControlCharacter => "IDENTITY_CONTROL_CHARACTER",
            Self::NonCanonical => "IDENTITY_NON_CANONICAL",
        }
    }
}

impl fmt::Display for IdentityValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl std::error::Error for IdentityValidationErrorV1 {}

/// Native copy failure at the already-validated identity producer.
///
/// Admission retains its caller's exact error. The full physical reserve cause
/// also survives a later admission or callback-protocol refusal. Neither cause
/// is cloned, formatted, or replaced by the producer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeIdentityCopyErrorV1<E> {
    Admission(E),
    AdmissionAfterReserveFailure {
        admission: E,
        reserve: std::collections::TryReserveError,
    },
    NativeAllocationFailed(std::collections::TryReserveError),
    InvalidNativeProducer,
    InvalidNativeProducerAfterReserveFailure(std::collections::TryReserveError),
    InvalidNativeCapacity,
}
impl<E> NativeIdentityCopyErrorV1<E> {
    /// Borrow the original physical cause, including after a later refusal.
    #[must_use]
    pub const fn reserve_failure_v1(&self) -> Option<&std::collections::TryReserveError> {
        match self {
            Self::AdmissionAfterReserveFailure { reserve, .. }
            | Self::NativeAllocationFailed(reserve)
            | Self::InvalidNativeProducerAfterReserveFailure(reserve) => Some(reserve),
            Self::Admission(_) | Self::InvalidNativeProducer | Self::InvalidNativeCapacity => None,
        }
    }

    /// Borrow the complete admission cause without losing a reserve failure.
    #[must_use]
    pub const fn admission_failure_v1(&self) -> Option<&E> {
        match self {
            Self::Admission(admission) | Self::AdmissionAfterReserveFailure { admission, .. } => {
                Some(admission)
            }
            Self::NativeAllocationFailed(_)
            | Self::InvalidNativeProducer
            | Self::InvalidNativeProducerAfterReserveFailure(_)
            | Self::InvalidNativeCapacity => None,
        }
    }
}
impl<E: fmt::Display> fmt::Display for NativeIdentityCopyErrorV1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(cause)
            | Self::AdmissionAfterReserveFailure {
                admission: cause, ..
            } => {
                write!(formatter, "identity copy admission: {cause}")
            }
            Self::NativeAllocationFailed(_) => {
                formatter.write_str("identity copy native allocation failed")
            }
            Self::InvalidNativeProducer | Self::InvalidNativeProducerAfterReserveFailure(_) => {
                formatter.write_str("identity copy native producer is invalid")
            }
            Self::InvalidNativeCapacity => {
                formatter.write_str("identity copy native capacity is invalid")
            }
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for NativeIdentityCopyErrorV1<E> {}

/// Finite status only; complete copy causes stay in external DATA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIdentityCopyRefusalV1 {
    OccupiedOutput,
    UsedData,
    OperationRefused,
    MissingResult,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NativeIdentityCopyPhaseV1 {
    Fresh,
    Admitting,
    Completed,
    Refused,
    Transferred,
}

/// Caller-owned phase and complete causes for exactly one native copy.
///
/// The physical callback writes a reserve error here BEFORE returning to
/// admission. Its original cause therefore stays external during every later
/// admission poll/refusal. No input, control, callback or funding is stored.
/// Backing and actual funding remain in the enclosing caller, through finishing.
pub struct NativeIdentityCopyDataV1<E> {
    reserve_failure: Option<std::collections::TryReserveError>,
    admission_failure: Option<E>,
    failure: Option<NativeIdentityCopyErrorV1<E>>,
    phase: NativeIdentityCopyPhaseV1,
    invoked: bool,
    repeated: bool,
    native_success: bool,
}
impl<E> NativeIdentityCopyDataV1<E> {
    #[must_use]
    pub const fn new_v1() -> Self {
        Self {
            reserve_failure: None,
            admission_failure: None,
            failure: None,
            phase: NativeIdentityCopyPhaseV1::Fresh,
            invoked: false,
            repeated: false,
            native_success: false,
        }
    }
    #[must_use]
    pub const fn is_fresh_v1(&self) -> bool {
        matches!(self.phase, NativeIdentityCopyPhaseV1::Fresh)
    }
    #[must_use]
    pub const fn is_complete_v1(&self) -> bool {
        matches!(self.phase, NativeIdentityCopyPhaseV1::Completed)
    }
    #[must_use]
    pub fn reserve_failure_v1(&self) -> Option<&std::collections::TryReserveError> {
        self.reserve_failure.as_ref().or_else(|| {
            self.failure
                .as_ref()
                .and_then(NativeIdentityCopyErrorV1::reserve_failure_v1)
        })
    }
    #[must_use]
    pub fn admission_failure_v1(&self) -> Option<&E> {
        self.admission_failure.as_ref().or_else(|| {
            self.failure
                .as_ref()
                .and_then(NativeIdentityCopyErrorV1::admission_failure_v1)
        })
    }
    #[must_use]
    pub fn failure_v1(&self) -> Option<&NativeIdentityCopyErrorV1<E>> {
        self.failure.as_ref()
    }
    /// Pure move between external slots. No Source poll or owned publisher.
    pub fn failure_into_slot_v1(
        &mut self,
        output: &mut Option<NativeIdentityCopyErrorV1<E>>,
    ) -> Result<(), NativeIdentityCopyRefusalV1> {
        if output.is_some() {
            return Err(NativeIdentityCopyRefusalV1::OccupiedOutput);
        }
        if self.phase != NativeIdentityCopyPhaseV1::Refused || self.failure.is_none() {
            return Err(NativeIdentityCopyRefusalV1::MissingResult);
        }
        *output = self.failure.take();
        self.phase = NativeIdentityCopyPhaseV1::Transferred;
        Ok(())
    }
    #[cfg(feature = "quanta-native-identity-v1")]
    fn failure_into_identity_slot_v1(
        &mut self,
        output: &mut Option<NativeIdentityConstructionErrorV1<E>>,
    ) -> Result<(), NativeIdentityCopyRefusalV1> {
        if output.is_some() {
            return Err(NativeIdentityCopyRefusalV1::OccupiedOutput);
        }
        if self.phase != NativeIdentityCopyPhaseV1::Refused {
            return Err(NativeIdentityCopyRefusalV1::MissingResult);
        }
        let Some(cause) = self.failure.take() else {
            return Err(NativeIdentityCopyRefusalV1::MissingResult);
        };
        *output = Some(NativeIdentityConstructionErrorV1::Copy(cause));
        self.phase = NativeIdentityCopyPhaseV1::Transferred;
        Ok(())
    }
    // Only pure moves inside this external DATA; no returned full-error value
    // and no further Source poll while joining the already-retained causes.
    fn refuse_admission_v1(&mut self, cause: E) -> Result<(), NativeIdentityCopyRefusalV1> {
        self.admission_failure = Some(cause);
        self.phase = NativeIdentityCopyPhaseV1::Refused;
        let Some(admission) = self.admission_failure.take() else {
            return Err(NativeIdentityCopyRefusalV1::MissingResult);
        };
        self.failure = Some(match self.reserve_failure.take() {
            Some(reserve) => {
                NativeIdentityCopyErrorV1::AdmissionAfterReserveFailure { admission, reserve }
            }
            None => NativeIdentityCopyErrorV1::Admission(admission),
        });
        Err(NativeIdentityCopyRefusalV1::OperationRefused)
    }
    fn refuse_protocol_v1(&mut self) -> Result<(), NativeIdentityCopyRefusalV1> {
        self.phase = NativeIdentityCopyPhaseV1::Refused;
        self.failure = Some(self.reserve_failure.take().map_or_else(
            || NativeIdentityCopyErrorV1::InvalidNativeProducer,
            NativeIdentityCopyErrorV1::InvalidNativeProducerAfterReserveFailure,
        ));
        Err(NativeIdentityCopyRefusalV1::OperationRefused)
    }
}
impl<E> Default for NativeIdentityCopyDataV1<E> {
    fn default() -> Self {
        Self::new_v1()
    }
}

fn begin_native_copy_attempt_v1<E>(
    value: &String,
    data: &mut NativeIdentityCopyDataV1<E>,
) -> Result<(), NativeIdentityCopyRefusalV1> {
    if !value.is_empty() || value.capacity() != 0 {
        return Err(NativeIdentityCopyRefusalV1::OccupiedOutput);
    }
    if !data.is_fresh_v1() {
        return Err(NativeIdentityCopyRefusalV1::UsedData);
    }
    data.phase = NativeIdentityCopyPhaseV1::Admitting;
    Ok(())
}

// SAME physical callback step; the caller DATA owns every state transition.
fn reserve_native_copy_step_v1<E>(
    bytes: usize,
    value: &mut String,
    data: &mut NativeIdentityCopyDataV1<E>,
) -> bool {
    if data.invoked {
        data.repeated = true;
        // Repetition invalidates the protocol, not the first physical receipt.
        return data.native_success;
    }
    data.invoked = true;
    data.native_success = match value.try_reserve_exact(bytes) {
        Ok(()) => true,
        Err(cause) => {
            // External BEFORE returning to admission and its subsequent polls.
            data.reserve_failure = Some(cause);
            false
        }
    };
    data.native_success
}

// SAME admission protocol over the physical callback and external phase DATA.
fn admit_native_copy_backing_v1<E>(
    bytes: usize,
    value: &mut String,
    data: &mut NativeIdentityCopyDataV1<E>,
    admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
) -> Result<(), NativeIdentityCopyRefusalV1> {
    let admitted = match admission(bytes, &mut || {
        reserve_native_copy_step_v1(bytes, value, data)
    }) {
        Ok(admitted) => admitted,
        Err(cause) => return data.refuse_admission_v1(cause),
    };
    if !data.invoked || data.repeated || admitted != data.native_success {
        return data.refuse_protocol_v1();
    }
    if !admitted {
        let Some(cause) = data.reserve_failure.take() else {
            return data.refuse_protocol_v1();
        };
        data.failure = Some(NativeIdentityCopyErrorV1::NativeAllocationFailed(cause));
        data.phase = NativeIdentityCopyPhaseV1::Refused;
        return Err(NativeIdentityCopyRefusalV1::OperationRefused);
    }
    if value.capacity() != bytes {
        data.failure = Some(NativeIdentityCopyErrorV1::InvalidNativeCapacity);
        data.phase = NativeIdentityCopyPhaseV1::Refused;
        return Err(NativeIdentityCopyRefusalV1::OperationRefused);
    }
    Ok(())
}

/// Unit producer over caller-owned backing and phase/error DATA.
///
/// The physical callback retains reserve failure immediately in this external DATA. The
/// caller holds DATA/backing/actual funding through its highest Source finisher.
/// Used DATA and occupied backing reject before input/admission polling. No
/// full-error owned publisher or convenience wrapper is on this unit path.
pub fn try_copy_string_into_slots_with_native_birth_v1<E>(
    source: &str,
    value: &mut String,
    data: &mut NativeIdentityCopyDataV1<E>,
    admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
) -> Result<(), NativeIdentityCopyRefusalV1> {
    begin_native_copy_attempt_v1(value, data)?;
    admit_native_copy_backing_v1(source.len(), value, data, admission)?;
    value.push_str(source);
    data.phase = NativeIdentityCopyPhaseV1::Completed;
    Ok(())
}

// Owned convenience ONLY. Source receivers call the unit producer with their
// external DATA directly. This adapter runs after that SAME core has finished.
fn owned_native_copy_result_v1<E>(
    status: Result<(), NativeIdentityCopyRefusalV1>,
    data: &mut NativeIdentityCopyDataV1<E>,
) -> Result<(), NativeIdentityCopyErrorV1<E>> {
    if status.is_ok() {
        return Ok(());
    }
    let mut failure = None;
    if data.failure_into_slot_v1(&mut failure).is_ok()
        && let Some(cause) = failure
    {
        return Err(cause);
    }
    Err(NativeIdentityCopyErrorV1::InvalidNativeProducer)
}

/// Owned-error convenience over the SAME unit producer. Highest Source callers
/// use `try_copy_string_into_slots_with_native_birth_v1` with external DATA.
pub fn try_copy_string_into_with_native_birth_v1<E>(
    source: &str,
    value: &mut String,
    admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
) -> Result<(), NativeIdentityCopyErrorV1<E>> {
    let mut data = NativeIdentityCopyDataV1::new_v1();
    let status =
        try_copy_string_into_slots_with_native_birth_v1(source, value, &mut data, admission);
    owned_native_copy_result_v1(status, &mut data)
}

/// Owned convenience over the SAME native copy body. The Source unit API
/// retains backing and phase/error DATA with the caller's actual funding.
pub fn try_copy_string_with_native_birth_v1<E>(
    source: &str,
    admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
) -> Result<String, NativeIdentityCopyErrorV1<E>> {
    let mut value = String::new();
    try_copy_string_into_with_native_birth_v1(source, &mut value, admission)?;
    Ok(value)
}

// One predicate owner keeps the ordinary and admitted error order identical.
trait IdentityValidationPolicyV1 {
    type Error;
    fn character_step_v1(&mut self) -> Result<(), Self::Error>;
    fn is_nfc_v1(&mut self, value: &str) -> Result<bool, Self::Error>;
}

enum IdentityValidationFailureV1<E> {
    Validation(IdentityValidationErrorV1),
    Operation(E),
}

fn validate_identity_with_policy_v1<P: IdentityValidationPolicyV1>(
    value: &str,
    policy: &mut P,
) -> Result<(), IdentityValidationFailureV1<P::Error>> {
    use IdentityValidationFailureV1::{Operation, Validation};
    if value.is_empty() {
        return Err(Validation(IdentityValidationErrorV1::Empty));
    }
    if value.len() > IDENTITY_MAX_UTF8_BYTES {
        return Err(Validation(IdentityValidationErrorV1::TooLong));
    }
    let mut characters = value.chars();
    loop {
        // Includes the final iterator step. Admission precedes decoding and
        // the canonical control-character predicate, with no owned ID copy.
        policy.character_step_v1().map_err(Operation)?;
        let Some(ch) = characters.next() else {
            break;
        };
        if matches!(u32::from(ch), 0x00..=0x1f | 0x7f..=0x9f) {
            return Err(Validation(IdentityValidationErrorV1::ControlCharacter));
        }
    }
    if !policy.is_nfc_v1(value).map_err(Operation)? {
        return Err(Validation(IdentityValidationErrorV1::NonCanonical));
    }
    Ok(())
}

struct OrdinaryIdentityValidationV1;
impl IdentityValidationPolicyV1 for OrdinaryIdentityValidationV1 {
    type Error = core::convert::Infallible;
    fn character_step_v1(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn is_nfc_v1(&mut self, value: &str) -> Result<bool, Self::Error> {
        Ok(value.nfc().eq(value.chars()))
    }
}
fn validate_identity(value: &str) -> Result<(), IdentityValidationErrorV1> {
    match validate_identity_with_policy_v1(value, &mut OrdinaryIdentityValidationV1) {
        Ok(()) => Ok(()),
        Err(IdentityValidationFailureV1::Validation(cause)) => Err(cause),
        Err(IdentityValidationFailureV1::Operation(cause)) => match cause {},
    }
}

/// Failure at the mandatory canonical borrowed validation or one owned copy.
/// No caller-supplied boolean can bypass the identity's private NFC seal.
#[cfg(feature = "quanta-native-identity-v1")]
#[derive(Debug)]
pub enum NativeIdentityConstructionErrorV1<E> {
    Validation(IdentityValidationErrorV1),
    Normalization(unicode_normalization::NativeNormalizationErrorV1<E>),
    Copy(NativeIdentityCopyErrorV1<E>),
}

#[cfg(feature = "quanta-native-identity-v1")]
impl<E: fmt::Display> fmt::Display for NativeIdentityConstructionErrorV1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(cause) => fmt::Display::fmt(cause, formatter),
            Self::Normalization(cause) => fmt::Display::fmt(cause, formatter),
            Self::Copy(cause) => fmt::Display::fmt(cause, formatter),
        }
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<E: std::error::Error + 'static> std::error::Error for NativeIdentityConstructionErrorV1<E> {}

/// Transient loans of caller-owned constructor slots. No input, control,
/// admission callback or funding is stored in the physical DATA.
#[cfg(feature = "quanta-native-identity-v1")]
pub struct NativeIdentityConstructionSlotsV1<'data, T, E> {
    pub backing: &'data mut String,
    pub output: &'data mut Option<T>,
    pub attempted: &'data mut bool,
    pub normalization: &'data mut unicode_normalization::NativeNormalizationDataV1<E>,
    pub copy: &'data mut NativeIdentityCopyDataV1<E>,
    pub failure: &'data mut Option<NativeIdentityConstructionErrorV1<E>>,
}

#[cfg(feature = "quanta-native-identity-v1")]
struct NativeIdentityValidationV1<'a, P: unicode_normalization::NativeNormalizationAdmissionV1> {
    admission: &'a mut P,
    data: &'a mut unicode_normalization::NativeNormalizationDataV1<P::Error>,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: unicode_normalization::NativeNormalizationAdmissionV1> IdentityValidationPolicyV1
    for NativeIdentityValidationV1<'_, P>
{
    type Error = unicode_normalization::NativeNormalizationErrorV1<P::Error>;
    fn character_step_v1(&mut self) -> Result<(), Self::Error> {
        self.admission
            .checkpoint_work_v1(1)
            .map_err(Self::Error::Admission)
    }
    fn is_nfc_v1(&mut self, value: &str) -> Result<bool, Self::Error> {
        if !self.data.is_fresh_v1() {
            return Err(Self::Error::InvalidNativeProducer);
        }
        let status = unicode_normalization::try_is_nfc_into_with_native_admission_v1(
            value,
            self.data,
            self.admission,
        );
        if status.is_ok() {
            return match self.data.result_v1() {
                Some(Ok(unicode_normalization::NativeNormalizationOutcomeV1::IsNfc(value))) => {
                    Ok(value)
                }
                _ => Err(Self::Error::InvalidNativeProducer),
            };
        }
        // Move the exact full cause to the caller's parent error. All actual
        // scratch stays in external DATA; neither transfer releases a grant.
        let mut result = None;
        self.data
            .result_into_slot_v1(&mut result)
            .map_err(|_slot_refusal| Self::Error::InvalidNativeProducer)?;
        match result {
            Some(Err(cause)) => Err(cause),
            _ => Err(Self::Error::InvalidNativeProducer),
        }
    }
}

#[cfg(feature = "quanta-native-identity-v1")]
fn validate_native_identity_v1<P: unicode_normalization::NativeNormalizationAdmissionV1>(
    value: &str,
    admission: &mut P,
) -> Result<(), NativeIdentityConstructionErrorV1<P::Error>> {
    let mut data = unicode_normalization::NativeNormalizationDataV1::new_v1();
    let result = validate_native_identity_into_v1(value, &mut data, admission);
    data.release_scratch_v1(admission);
    result
}

#[cfg(feature = "quanta-native-identity-v1")]
fn validate_native_identity_into_v1<P: unicode_normalization::NativeNormalizationAdmissionV1>(
    value: &str,
    data: &mut unicode_normalization::NativeNormalizationDataV1<P::Error>,
    admission: &mut P,
) -> Result<(), NativeIdentityConstructionErrorV1<P::Error>> {
    if !data.is_fresh_v1() {
        return Err(NativeIdentityConstructionErrorV1::Normalization(
            unicode_normalization::NativeNormalizationErrorV1::InvalidNativeProducer,
        ));
    }
    match validate_identity_with_policy_v1(
        value,
        &mut NativeIdentityValidationV1 { admission, data },
    ) {
        Ok(()) => Ok(()),
        Err(IdentityValidationFailureV1::Validation(cause)) => {
            Err(NativeIdentityConstructionErrorV1::Validation(cause))
        }
        Err(IdentityValidationFailureV1::Operation(cause)) => {
            Err(NativeIdentityConstructionErrorV1::Normalization(cause))
        }
    }
}

/// Boundary adapter for the SAME identity visitors. The runtime supplies its
/// original NFC producer and retained String group; no authority is stored.
///
/// The caller must use a deserializer that admits owned `String` and escaped
/// string backing before allocation. Generic Serde deserializers may allocate
/// that backing before these identity visitor callbacks run; this admission
/// policy alone cannot account for those allocations.
#[cfg(feature = "quanta-native-identity-v1")]
pub trait NativeIdentityDecodeAdmissionV1 {
    type OriginalError;
    type Funding;
    type Error: fmt::Display + Copy;
    fn repo_id_from_owned_v1(
        &mut self,
        loan: &mut NativeIdentityDecodeLoanV1<'_, '_, RepoId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), Self::Error>;
    fn repo_id_from_borrowed_v1(
        &mut self,
        loan: &mut NativeIdentityDecodeLoanV1<'_, '_, RepoId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), Self::Error>;
    fn revision_id_from_owned_v1(
        &mut self,
        loan: &mut NativeIdentityDecodeLoanV1<
            '_,
            '_,
            RevisionId,
            Self::OriginalError,
            Self::Funding,
        >,
    ) -> Result<(), Self::Error>;
    fn revision_id_from_borrowed_v1(
        &mut self,
        loan: &mut NativeIdentityDecodeLoanV1<
            '_,
            '_,
            RevisionId,
            Self::OriginalError,
            Self::Funding,
        >,
    ) -> Result<(), Self::Error>;
}

trait IdentityConstructionPolicyV1<T> {
    type Error: fmt::Display;
    type Output;
    fn begin_v1(&mut self) -> bool;
    fn build_owned_v1(self, value: String) -> Result<Self::Output, Self::Error>;
    fn build_borrowed_v1(self, value: &str) -> Result<Self::Output, Self::Error>;
}
struct OrdinaryIdentityConstructionV1;
#[cfg(feature = "quanta-native-identity-v1")]
struct NativeIdentityConstructionV1<'a, T, P: NativeIdentityDecodeAdmissionV1 + ?Sized> {
    admission: &'a mut P,
    data: &'a mut NativeIdentityDecodeDataV1<T, P::OriginalError, P::Funding>,
}

#[cfg(feature = "quanta-native-identity-v1")]
enum NativeIdentityVisitorRefusalV1<E> {
    Policy(E),
    InvalidNativeProducer,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<E: fmt::Display> fmt::Display for NativeIdentityVisitorRefusalV1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(cause) => fmt::Display::fmt(cause, formatter),
            Self::InvalidNativeProducer => {
                formatter.write_str("native identity producer did not fill its external DATA")
            }
        }
    }
}

trait IdentityDecodeValueV1: Sized {
    fn decode_with_policy_v1<'de, D, P>(deserializer: D, policy: P) -> Result<P::Output, D::Error>
    where
        D: Deserializer<'de>,
        P: IdentityConstructionPolicyV1<Self>;
}

#[cfg(feature = "quanta-native-identity-v1")]
struct IdentityDecodeSeedV1<'a, T, P: NativeIdentityDecodeAdmissionV1 + ?Sized> {
    admission: &'a mut P,
    data: &'a mut NativeIdentityDecodeDataV1<T, P::OriginalError, P::Funding>,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<'de, T: IdentityDecodeValueV1, P: NativeIdentityDecodeAdmissionV1 + ?Sized>
    de::DeserializeSeed<'de> for IdentityDecodeSeedV1<'_, T, P>
where
    for<'a> NativeIdentityConstructionV1<'a, T, P>: IdentityConstructionPolicyV1<T, Output = ()>,
{
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        T::decode_with_policy_v1(
            deserializer,
            NativeIdentityConstructionV1 {
                admission: self.admission,
                data: &mut *self.data,
            },
        )?;
        self.data.state.phase = native_decode_data_v1::NativeIdentityDecodePhaseV1::Completed;
        Ok(())
    }
}

macro_rules! validated_identity {
    ($name:ident, $owned_v1:ident, $borrowed_v1:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Construct only from an already-canonical identity. This never normalizes input.
            pub fn new(value: impl Into<String>) -> Result<Self, IdentityValidationErrorV1> {
                let value = value.into();
                validate_identity(value.as_str())?;
                Ok(Self(value))
            }

            /// Owned-error convenience over the SAME unit constructor.
            /// Highest Source receivers use
            /// `try_from_str_into_slots_with_native_admission_v1` with external
            /// copy DATA; this convenience owns local copy/error state.
            ///
            /// `attempted` must be false, `backing` empty with zero capacity,
            /// `output` None, and `normalization_data` fresh. Invalid or reused
            /// slots are preserved without invoking either admission. Once validation begins, this attempt
            /// cannot be reused, including after refusal before native birth.
            ///
            /// The same predicate/NFC producers validate before the single
            /// admitted copy. Failure preserves partial or complete unsealed
            /// backing in the caller's String and returns the exact cause.
            /// Only a successful final checkpoint moves backing into `output`.
            /// The caller retains these slots, the returned failure, and its
            /// original copy grant through failure settlement. Normalization
            /// scratch stays in `normalization_data` on every outcome. Retain
            /// the actual normalization funding bank through the highest source
            /// finisher, then drop DATA before that bank or explicitly retire
            /// DATA at the caller's safe terminal point.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn try_from_str_into_with_native_admission_v1<P>(
                value: &str,
                backing: &mut String,
                output: &mut Option<Self>,
                attempted: &mut bool,
                normalization_data: &mut unicode_normalization::NativeNormalizationDataV1<P::Error>,
                normalization_admission: &mut P,
                copy_admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, P::Error>,
            ) -> Result<(), NativeIdentityConstructionErrorV1<P::Error>>
            where
                P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                let mut copy = NativeIdentityCopyDataV1::new_v1();
                let mut failure = None;
                let status = Self::try_from_str_into_slots_with_native_admission_v1(
                    value,
                    NativeIdentityConstructionSlotsV1 {
                        backing, output, attempted, normalization: normalization_data,
                        copy: &mut copy, failure: &mut failure,
                    },
                    normalization_admission, copy_admission,
                );
                match status {
                    Ok(()) => Ok(()),
                    Err(_) => Err(failure.unwrap_or(NativeIdentityConstructionErrorV1::Copy(
                        NativeIdentityCopyErrorV1::InvalidNativeProducer,
                    ))),
                }
            }

            /// SAME constructor over external slots, including the actual
            /// copy phase/reserve DATA. Source receivers use this unit entry;
            /// owned-error conveniences never run on this path. Keep all slots
            /// and actual normalization/copy funding through highest finishing.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn try_from_str_into_slots_with_native_admission_v1<P>(
                value: &str,
                slots: NativeIdentityConstructionSlotsV1<'_, Self, P::Error>,
                normalization_admission: &mut P,
                copy_admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, P::Error>,
            ) -> Result<(), NativeIdentityDecodeDataRefusalV1>
            where P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                if slots.failure.is_some() {
                    return Err(NativeIdentityDecodeDataRefusalV1::OccupiedOutput);
                }
                if *slots.attempted || slots.output.is_some()
                    || !slots.backing.is_empty() || slots.backing.capacity() != 0
                    || !slots.normalization.is_fresh_v1() || !slots.copy.is_fresh_v1() {
                    return Err(NativeIdentityDecodeDataRefusalV1::InvalidNativeProducer);
                }
                *slots.attempted = true;
                if let Err(cause) = validate_native_identity_into_v1(value, slots.normalization, normalization_admission) {
                    *slots.failure = Some(cause);
                    return Err(NativeIdentityDecodeDataRefusalV1::OperationRefused);
                }
                let Ok(copy_work) = u64::try_from(value.len()) else {
                    *slots.failure = Some(NativeIdentityConstructionErrorV1::Normalization(
                        unicode_normalization::NativeNormalizationErrorV1::ArithmeticOverflow,
                    ));
                    return Err(NativeIdentityDecodeDataRefusalV1::OperationRefused);
                };
                if let Err(cause) = normalization_admission.checkpoint_work_v1(copy_work) {
                    *slots.failure = Some(NativeIdentityConstructionErrorV1::Copy(
                        NativeIdentityCopyErrorV1::Admission(cause),
                    ));
                    return Err(NativeIdentityDecodeDataRefusalV1::OperationRefused);
                }
                if try_copy_string_into_slots_with_native_birth_v1(value, slots.backing, slots.copy, copy_admission).is_err() {
                    return match slots.copy.failure_into_identity_slot_v1(slots.failure) {
                        Ok(()) => Err(NativeIdentityDecodeDataRefusalV1::OperationRefused),
                        Err(_) => Err(NativeIdentityDecodeDataRefusalV1::InvalidNativeProducer),
                    };
                }
                if let Err(cause) = normalization_admission.checkpoint_work_v1(0) {
                    *slots.failure = Some(NativeIdentityConstructionErrorV1::Copy(
                        NativeIdentityCopyErrorV1::Admission(cause),
                    ));
                    return Err(NativeIdentityDecodeDataRefusalV1::OperationRefused);
                }
                *slots.output = Some(Self(core::mem::take(slots.backing)));
                Ok(())
            }

            /// Owned convenience over the same borrowed attempt body.
            /// Use the into-slot API when backing must outlive a late refusal.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn try_from_str_with_native_admission_v1<P>(
                value: &str,
                normalization_admission: &mut P,
                copy_admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, P::Error>,
            ) -> Result<Self, NativeIdentityConstructionErrorV1<P::Error>>
            where
                P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                let mut backing = String::new();
                let mut output = None;
                let mut attempted = false;
                let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
                let result = Self::try_from_str_into_with_native_admission_v1(
                    value, &mut backing, &mut output, &mut attempted,
                    &mut normalization_data, normalization_admission, copy_admission,
                );
                normalization_data.release_scratch_v1(normalization_admission);
                result?;
                output.ok_or(NativeIdentityConstructionErrorV1::Copy(
                    NativeIdentityCopyErrorV1::InvalidNativeProducer,
                ))
            }

            /// Validate an already-owned, caller-admitted String without a
            /// second copy. Input NFC is rejected, never normalized.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn try_from_owned_with_native_admission_v1<P>(
                value: String,
                normalization_admission: &mut P,
            ) -> Result<Self, NativeIdentityConstructionErrorV1<P::Error>>
            where
                P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                validate_native_identity_v1(&value, normalization_admission)?;
                normalization_admission.checkpoint_work_v1(0)
                    .map_err(|cause| NativeIdentityConstructionErrorV1::Normalization(
                        unicode_normalization::NativeNormalizationErrorV1::Admission(cause)
                    ))?;
                Ok(Self(value))
            }

            /// SAME identity visitor with an explicitly borrowed native owner.
            /// Owned wire Strings move to the canonical constructor unchanged.
            /// The caller's deserializer must admit owned and escaped string
            /// backing before allocation, as required by the admission trait.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn native_decode_seed_v1<'data, 'de, P: NativeIdentityDecodeAdmissionV1 + ?Sized>(
                admission: &'data mut P,
                data: &'data mut NativeIdentityDecodeDataV1<Self, P::OriginalError, P::Funding>,
            ) -> impl de::DeserializeSeed<'de, Value = ()> + 'data {
                IdentityDecodeSeedV1::<Self, P> { admission, data }
            }

            /// Unit driver that parks the complete deserializer error outside
            /// Source. The caller retains both external slots through finishing.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn try_decode_into_v1<'de, D: Deserializer<'de>, P: NativeIdentityDecodeAdmissionV1 + ?Sized>(
                deserializer: D, admission: &mut P,
                data: &mut NativeIdentityDecodeDataV1<Self, P::OriginalError, P::Funding>,
                failure: &mut Option<D::Error>,
            ) -> Result<(), NativeIdentityDecodeDataRefusalV1> {
                if failure.is_some() { return Err(NativeIdentityDecodeDataRefusalV1::OccupiedOutput); }
                if !data.is_fresh_v1() { return Err(NativeIdentityDecodeDataRefusalV1::UsedData); }
                match de::DeserializeSeed::deserialize(Self::native_decode_seed_v1(admission, data), deserializer) {
                    Ok(()) => Ok(()),
                    Err(cause) => { *failure = Some(cause); Err(NativeIdentityDecodeDataRefusalV1::OperationRefused) }
                }
            }

            /// Validate borrowed wire bytes through the SAME raw predicate/NFC
            /// producer. No identity copy or new authority is materialized.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn validate_str_with_native_admission_v1<P>(
                value: &str,
                normalization_admission: &mut P,
            ) -> Result<(), NativeIdentityConstructionErrorV1<P::Error>>
            where
                P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                validate_native_identity_v1(value, normalization_admission)
            }

            /// Validate borrowed bytes with the SAME predicate/NFC producer,
            /// retaining all actual NFC scratch in the caller's external DATA.
            /// No identity String, typed identity, or authority is constructed.
            ///
            /// Retain DATA and the returned full non-Copy failure through the
            /// highest Source finisher, then drop DATA before its actual funding
            /// bank. This function never releases normalization grants. Used or
            /// retired normalization DATA rejects before any input/admission
            /// polling and preserves the original result and backing.
            ///
            /// Predicate or work refusal before NFC leaves normalization DATA
            /// fresh because no normalization attempt began. The enclosing
            /// validation owner must retain that full error and its own attempt
            /// state; normalization DATA is not a whole-validation retry guard.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn validate_str_into_with_native_admission_v1<P>(
                value: &str,
                normalization_data: &mut unicode_normalization::NativeNormalizationDataV1<P::Error>,
                normalization_admission: &mut P,
            ) -> Result<(), NativeIdentityConstructionErrorV1<P::Error>>
            where
                P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                validate_native_identity_into_v1(value, normalization_data, normalization_admission)
            }

            /// Owned-error convenience for exact private canonical bytes
            /// without re-running identity/NFC validation. `backing` must be
            /// empty with zero capacity and `output` must be None.
            ///
            /// Admission failure, including refusal after actual native birth,
            /// leaves backing in the caller's String and output unpopulated.
            /// Only complete success moves that same backing into the typed
            /// output. The caller keeps both slots with its original grant.
            /// Highest Source receivers use the unit clone with external copy
            /// DATA, rather than this local-DATA owned-error convenience.
            pub fn try_clone_into_with_native_birth_v1<E>(
                &self,
                backing: &mut String,
                output: &mut Option<Self>,
                admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
            ) -> Result<(), NativeIdentityCopyErrorV1<E>> {
                let mut data = NativeIdentityCopyDataV1::new_v1();
                let status = self.try_clone_into_slots_with_native_birth_v1(backing, output, &mut data, admission);
                owned_native_copy_result_v1(status, &mut data)
            }

            /// SAME sealed clone, with caller-owned phase/error DATA injected
            /// into the physical reserve callback. No owned convenience call,
            /// repeated NFC work or additional native birth is on this path.
            pub fn try_clone_into_slots_with_native_birth_v1<E>(
                &self,
                backing: &mut String,
                output: &mut Option<Self>,
                data: &mut NativeIdentityCopyDataV1<E>,
                admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
            ) -> Result<(), NativeIdentityCopyRefusalV1> {
                if output.is_some() {
                    return Err(NativeIdentityCopyRefusalV1::OccupiedOutput);
                }
                try_copy_string_into_slots_with_native_birth_v1(self.0.as_str(), backing, data, admission)?;
                *output = Some(Self(core::mem::take(backing)));
                Ok(())
            }

            /// Owned convenience over the same sealed into-slot copy body.
            /// Use that API when partial backing must outlive a late refusal.
            pub fn try_clone_with_native_birth_v1<E>(
                &self,
                admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
            ) -> Result<Self, NativeIdentityCopyErrorV1<E>> {
                let mut backing = String::new();
                let mut output = None;
                self.try_clone_into_with_native_birth_v1(&mut backing, &mut output, admission)?;
                output.ok_or(NativeIdentityCopyErrorV1::InvalidNativeProducer)
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentityValidationErrorV1;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdentityValidationErrorV1;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl FromStr for $name {
            type Err = IdentityValidationErrorV1;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.0.as_str())
            }
        }

        impl IdentityConstructionPolicyV1<$name> for OrdinaryIdentityConstructionV1 {
            type Error = IdentityValidationErrorV1;
            type Output = $name;
            fn begin_v1(&mut self) -> bool { true }
            fn build_owned_v1(self, value: String) -> Result<$name, Self::Error> { $name::new(value) }
            fn build_borrowed_v1(self, value: &str) -> Result<$name, Self::Error> { $name::new(value) }
        }
        #[cfg(feature = "quanta-native-identity-v1")]
        impl<P: NativeIdentityDecodeAdmissionV1 + ?Sized> IdentityConstructionPolicyV1<$name>
            for NativeIdentityConstructionV1<'_, $name, P>
        {
            type Error = NativeIdentityVisitorRefusalV1<P::Error>;
            type Output = ();
            fn begin_v1(&mut self) -> bool { self.data.begin_v1() }
            fn build_owned_v1(self, value: String) -> Result<(), Self::Error> {
                self.data.state.backing = value;
                self.admission.$owned_v1(&mut self.data.loan_v1(native_decode_data_v1::NativeIdentityDecodeInputV1::Owned)).map_err(Self::Error::Policy)?;
                if !self.data.is_complete_v1() { return Err(Self::Error::InvalidNativeProducer); }
                Ok(())
            }
            fn build_borrowed_v1(self, value: &str) -> Result<(), Self::Error> {
                self.admission.$borrowed_v1(&mut self.data.loan_v1(native_decode_data_v1::NativeIdentityDecodeInputV1::Borrowed(value))).map_err(Self::Error::Policy)?;
                if !self.data.is_complete_v1() { return Err(Self::Error::InvalidNativeProducer); }
                Ok(())
            }
        }

        #[cfg(feature = "quanta-native-identity-v1")]
        impl<E> NativeIdentityDecodeRunnerV1<'_, '_, $name, E> {
            /// Fill only the exact wire input bound by the canonical visitor.
            /// All backing and complete failures stay in its external DATA.
            pub fn try_fill_v1<P>(
                &mut self,
                normalization: &mut P,
                copy: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
            ) -> Result<(), NativeIdentityDecodeDataRefusalV1>
            where P: unicode_normalization::NativeNormalizationAdmissionV1<Error = E>,
            {
                if self.state.construction_attempted || self.state.output.is_some()
                    || self.state.failure.is_some() {
                    return Err(NativeIdentityDecodeDataRefusalV1::UsedData);
                }
                let result = match self.input {
                    native_decode_data_v1::NativeIdentityDecodeInputV1::Borrowed(value) => {
                        return $name::try_from_str_into_slots_with_native_admission_v1(
                            value,
                            NativeIdentityConstructionSlotsV1 {
                                backing: &mut self.state.backing,
                                output: &mut self.state.output,
                                attempted: &mut self.state.construction_attempted,
                                normalization: &mut self.state.normalization,
                                copy: &mut self.state.copy,
                                failure: &mut self.state.failure,
                            },
                            normalization, copy,
                        );
                    }
                    native_decode_data_v1::NativeIdentityDecodeInputV1::Owned => {
                        self.state.construction_attempted = true;
                        (|| {
                            validate_native_identity_into_v1(
                                &self.state.backing, &mut self.state.normalization, normalization,
                            )?;
                            normalization.checkpoint_work_v1(0).map_err(|cause|
                                NativeIdentityConstructionErrorV1::Normalization(
                                    unicode_normalization::NativeNormalizationErrorV1::Admission(cause)
                                ))?;
                            self.state.output = Some($name(core::mem::take(&mut self.state.backing)));
                            Ok(())
                        })()
                    }
                };
                self.record_v1(result)
            }
        }

        impl IdentityDecodeValueV1 for $name {
            fn decode_with_policy_v1<'de, D, P>(deserializer: D, mut policy: P) -> Result<P::Output, D::Error>
            where D: Deserializer<'de>, P: IdentityConstructionPolicyV1<Self>,
            {
                struct IdentityVisitor<P>(P);

                impl<'de, P: IdentityConstructionPolicyV1<$name>> de::Visitor<'de> for IdentityVisitor<P> {
                    type Value = P::Output;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("a canonical ", stringify!($name), " string"))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.0.build_borrowed_v1(value).map_err(E::custom)
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.0.build_owned_v1(value).map_err(E::custom)
                    }
                }

                if !policy.begin_v1() {
                    return Err(de::Error::custom("native identity DATA is already used"));
                }
                deserializer.deserialize_string(IdentityVisitor(policy))
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::decode_with_policy_v1(deserializer, OrdinaryIdentityConstructionV1)
            }
        }
    };
}

validated_identity!(RepoId, repo_id_from_owned_v1, repo_id_from_borrowed_v1);
validated_identity!(
    RevisionId,
    revision_id_from_owned_v1,
    revision_id_from_borrowed_v1
);

/// The validated logical repository/revision tuple.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepositoryRevisionIdentityV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
}

impl RepositoryRevisionIdentityV1 {
    #[must_use]
    pub const fn new(repo_id: RepoId, revision_id: RevisionId) -> Self {
        Self {
            repo_id,
            revision_id,
        }
    }

    #[must_use]
    pub const fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }

    #[must_use]
    pub const fn revision_id(&self) -> &RevisionId {
        &self.revision_id
    }

    /// Exact length-delimited payload from SEP-21-001.
    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            8usize
                .saturating_add(self.repo_id.0.len())
                .saturating_add(self.revision_id.0.len()),
        );
        push_len_prefixed(&mut bytes, self.repo_id.0.as_bytes());
        push_len_prefixed(&mut bytes, self.revision_id.0.as_bytes());
        bytes
    }

    #[must_use]
    /// Infallible by construction: SHA-256 over validated canonical payload cannot fail.
    pub fn digest(&self) -> [u8; 32] {
        domain_digest(
            REPOSITORY_REVISION_DOMAIN,
            self.canonical_payload().as_slice(),
        )
    }
}

/// A repository/revision tuple plus its logical generation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LogicalGenerationIdentityV1 {
    repository_revision: RepositoryRevisionIdentityV1,
    generation: u64,
}

impl LogicalGenerationIdentityV1 {
    #[must_use]
    pub const fn new(repository_revision: RepositoryRevisionIdentityV1, generation: u64) -> Self {
        Self {
            repository_revision,
            generation,
        }
    }

    #[must_use]
    pub const fn repository_revision(&self) -> &RepositoryRevisionIdentityV1 {
        &self.repository_revision
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Exact tuple payload; it deliberately does not nest a previously framed digest.
    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        let mut bytes = self.repository_revision.canonical_payload();
        bytes.extend_from_slice(&self.generation.to_be_bytes());
        bytes
    }

    #[must_use]
    /// Infallible by construction: SHA-256 over validated canonical payload cannot fail.
    pub fn digest(&self) -> [u8; 32] {
        domain_digest(
            LOGICAL_GENERATION_DOMAIN,
            self.canonical_payload().as_slice(),
        )
    }
}

fn push_len_prefixed(target: &mut Vec<u8>, value: &[u8]) {
    let length = value.len().to_be_bytes();
    target.extend_from_slice(&length[4..]);
    target.extend_from_slice(value);
}

fn domain_digest(domain: &str, payload: &[u8]) -> [u8; 32] {
    let domain_length = domain.len().to_be_bytes();
    let mut hasher = Sha256::new();
    hasher.update(&domain_length[4..]);
    hasher.update(domain.as_bytes());
    hasher.update(payload);
    hasher.finalize().into()
}

u64_newtype!(ManifestGeneration);
u64_newtype!(GenerationId);
string_newtype!(ManifestDigest);
string_newtype!(FileId);
string_newtype!(RepoRelativePath);

#[cfg(test)]
mod tests {
    use super::NativeIdentityCopyErrorV1 as NativeCopy;
    use super::{
        IdentityValidationErrorV1, LogicalGenerationIdentityV1, RepoId,
        RepositoryRevisionIdentityV1, RevisionId,
    };

    #[test]
    fn native_identity_copy_preserves_sealed_bytes_and_uses_one_actual_birth() {
        let original = RepoId::new("répo/../%").expect("canonical fixture");
        let mut calls = 0;
        let copy = original
            .try_clone_with_native_birth_v1(|bytes, birth| {
                assert_eq!(bytes, "répo/../%".len());
                calls += 1;
                Ok::<_, u8>(birth())
            })
            .expect("native copy");
        assert_eq!(calls, 1);
        assert_eq!(copy, original);
        assert_ne!(copy.0.as_ptr(), original.0.as_ptr());
        assert_eq!(copy.0.capacity(), original.as_str().len());
        let revision = RevisionId::new("révision/%").expect("canonical revision");
        assert_eq!(
            revision.try_clone_with_native_birth_v1(|_, birth| Ok::<_, u8>(birth())),
            Ok(revision)
        );
    }

    #[test]
    fn native_identity_copy_refuses_missing_repeated_or_misreported_birth() {
        let original = RepoId::new("repo/test").expect("canonical fixture");
        for report in [false, true] {
            assert_eq!(
                original.try_clone_with_native_birth_v1(|_, _| Ok::<_, u8>(report)),
                Err(NativeCopy::InvalidNativeProducer)
            );
        }
        assert_eq!(
            original.try_clone_with_native_birth_v1(|_, birth| {
                assert!(birth());
                assert!(birth(), "repeat keeps first physical receipt");
                Ok::<_, u8>(true)
            }),
            Err(NativeCopy::InvalidNativeProducer)
        );
        assert_eq!(
            original.try_clone_with_native_birth_v1(|_, birth| {
                assert!(birth());
                Ok::<_, u8>(false)
            }),
            Err(NativeCopy::InvalidNativeProducer)
        );
        assert_eq!(original.as_str(), "repo/test");
    }

    #[test]
    fn native_identity_copy_preserves_original_admission_before_and_after_birth() {
        let original = RepoId::new("repo/test").expect("canonical fixture");
        assert_eq!(
            original.try_clone_with_native_birth_v1(|_, _| Err::<bool, _>(7_u8)),
            Err(NativeCopy::Admission(7))
        );
        assert_eq!(
            original.try_clone_with_native_birth_v1(|_, birth| {
                assert!(birth());
                Err::<bool, _>(8_u8)
            }),
            Err(NativeCopy::Admission(8))
        );
    }

    #[test]
    fn identity_policy_is_exact_and_does_not_normalize() {
        assert_eq!(RepoId::new(""), Err(IdentityValidationErrorV1::Empty));
        assert_eq!(
            RepoId::new("a\u{0000}b"),
            Err(IdentityValidationErrorV1::ControlCharacter)
        );
        assert_eq!(
            RepoId::new("e\u{301}"),
            Err(IdentityValidationErrorV1::NonCanonical)
        );
        assert_eq!(
            RepoId::new("a".repeat(513)),
            Err(IdentityValidationErrorV1::TooLong)
        );
        for accepted in ["%", "/", ".", "..", "A", "a", "é"] {
            assert_eq!(
                RepoId::new(accepted).map(super::RepoId::into_inner),
                Ok(accepted.to_owned())
            );
        }
    }

    #[test]
    fn identity_serde_and_constructor_share_validation() {
        let id = RepoId::new("repo/../%").expect("valid fixture ID");
        let json = serde_json::to_string(&id).expect("serialize id");
        assert_eq!(
            serde_json::from_str::<RepoId>(json.as_str()).expect("roundtrip decode"),
            id
        );
        assert!(serde_json::from_str::<RepoId>("\"e\\u0301\"").is_err());
    }

    #[test]
    fn tuple_framing_is_injective_for_separator_collision_fixture() {
        let left = RepositoryRevisionIdentityV1::new(
            RepoId::new("a--b").expect("valid"),
            RevisionId::new("c").expect("valid"),
        );
        let right = RepositoryRevisionIdentityV1::new(
            RepoId::new("a").expect("valid"),
            RevisionId::new("b--c").expect("valid"),
        );
        assert_ne!(left.canonical_payload(), right.canonical_payload());
        assert_ne!(left.digest(), right.digest());
        assert_ne!(
            LogicalGenerationIdentityV1::new(left, 7).digest(),
            LogicalGenerationIdentityV1::new(right, 7).digest()
        );
    }
}

#[cfg(all(test, feature = "quanta-native-identity-v1"))]
mod native_raw_identity_tests_v1 {
    use super::*;
    use unicode_normalization::{
        NativeNormalizationAdmissionV1, NativeNormalizationErrorV1,
        NativeNormalizationScratchDemandV1, NativeNormalizationScratchOwnerV1,
    };

    #[derive(Default)]
    struct Admission {
        work: u64,
        births: usize,
        releases: usize,
        fail_work: Option<u8>,
        fail_birth: Option<u8>,
        fail_after_birth: Option<u8>,
    }
    impl NativeNormalizationAdmissionV1 for Admission {
        type Error = u8;
        fn checkpoint_work_v1(&mut self, units: u64) -> Result<(), u8> {
            if let Some(cause) = self.fail_work {
                return Err(cause);
            }
            let Some(work) = self.work.checked_add(units) else {
                return Err(u8::MAX);
            };
            self.work = work;
            Ok(())
        }
        fn native_birth_v1(
            &mut self,
            demand: NativeNormalizationScratchDemandV1,
            birth: &mut dyn FnMut() -> bool,
        ) -> Result<bool, u8> {
            if let Some(cause) = self.fail_birth {
                return Err(cause);
            }
            if demand.new_bytes_v1 <= demand.current_bytes_v1 {
                return Err(u8::MAX);
            }
            self.births = self.births.checked_add(1).ok_or(u8::MAX)?;
            let success = birth();
            if success && let Some(cause) = self.fail_after_birth {
                return Err(cause);
            }
            Ok(success)
        }
        fn release_scratch_v1(&mut self, _owner: NativeNormalizationScratchOwnerV1) {
            let (releases, overflow) = self.releases.overflowing_add(1);
            assert!(!overflow, "fixture release count exceeds usize");
            self.releases = releases;
        }
    }

    #[test]
    fn raw_ids_reuse_unicode_validation_and_copy_once_v1() {
        // Equal combining classes are NFC-canonical here, and the run exceeds
        // both canonical iterators' inline scratch. No ASCII-only shortcut.
        let source = format!("q{}", "\u{301}".repeat(20));
        let expected = RepoId::new(source.as_str()).unwrap();
        let mut admission = Admission::default();
        let mut copy_calls = 0_usize;
        let constructed = RepoId::try_from_str_with_native_admission_v1(
            &source,
            &mut admission,
            |bytes, birth| {
                assert_eq!(bytes, source.len());
                copy_calls = copy_calls
                    .checked_add(1)
                    .expect("copy call count fits usize");
                Ok(birth())
            },
        )
        .unwrap();
        assert_eq!(constructed, expected);
        assert_ne!(constructed.as_str().as_ptr(), source.as_ptr());
        assert_eq!(copy_calls, 1);
        assert!(admission.births >= 2);
        assert_eq!(admission.releases, 2);
    }

    #[test]
    fn owned_raw_id_moves_original_string_after_same_validation_v1() {
        let source = String::from("révision");
        let pointer = source.as_ptr();
        let mut admission = Admission::default();
        let value =
            RevisionId::try_from_owned_with_native_admission_v1(source, &mut admission).unwrap();
        assert_eq!(value.as_str(), "révision");
        assert_eq!(value.as_str().as_ptr(), pointer);
    }

    #[test]
    fn same_raw_predicate_error_order_precedes_any_owned_copy_v1() {
        let too_long_control = format!("\n{}", "x".repeat(512));
        for (source, expected) in [
            ("", IdentityValidationErrorV1::Empty),
            (
                too_long_control.as_str(),
                IdentityValidationErrorV1::TooLong,
            ),
            ("\n", IdentityValidationErrorV1::ControlCharacter),
            ("e\u{301}", IdentityValidationErrorV1::NonCanonical),
        ] {
            assert_eq!(RepoId::new(source), Err(expected));
            let mut admission = Admission::default();
            let error = RepoId::try_from_str_with_native_admission_v1(
                source,
                &mut admission,
                |_, _| -> Result<bool, u8> { panic!("invalid ID was copied") },
            )
            .unwrap_err();
            assert!(
                matches!(error, NativeIdentityConstructionErrorV1::Validation(cause) if cause == expected)
            );
        }
    }

    #[test]
    fn raw_validation_retains_exact_source_cause_before_and_after_scratch_birth_v1() {
        let mut admission = Admission {
            fail_work: Some(7),
            ..Admission::default()
        };
        let error = RepoId::try_from_str_with_native_admission_v1(
            "repository",
            &mut admission,
            |_, _| -> Result<bool, u8> { panic!("refused ID copied") },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            NativeIdentityConstructionErrorV1::Normalization(
                NativeNormalizationErrorV1::Admission(7)
            )
        ));
        assert_eq!(admission.births, 0);
        let source = format!("q{}", "\u{301}".repeat(20));
        for (before, after, expected_births, cause) in
            [(Some(8), None, 0, 8), (None, Some(9), 1, 9)]
        {
            let mut admission = Admission {
                fail_birth: before,
                fail_after_birth: after,
                ..Admission::default()
            };
            let error = RevisionId::try_from_str_with_native_admission_v1(
                &source,
                &mut admission,
                |_, _| -> Result<bool, u8> { panic!("refused ID copied") },
            )
            .unwrap_err();
            assert!(
                matches!(error, NativeIdentityConstructionErrorV1::Normalization(NativeNormalizationErrorV1::Admission(actual)) if actual == cause)
            );
            assert_eq!(admission.births, expected_births);
            assert_eq!(admission.releases, 2);
        }
    }

    #[test]
    fn borrowed_validation_into_data_preserves_canonical_predicate_order_v1() {
        let too_long_control = format!("\n{}", "x".repeat(512));
        for (source, expected) in [
            ("", IdentityValidationErrorV1::Empty),
            (
                too_long_control.as_str(),
                IdentityValidationErrorV1::TooLong,
            ),
            ("\n", IdentityValidationErrorV1::ControlCharacter),
            ("e\u{301}", IdentityValidationErrorV1::NonCanonical),
        ] {
            for revision in [false, true] {
                let mut data = unicode_normalization::NativeNormalizationDataV1::new_v1();
                let mut admission = Admission::default();
                let result = if revision {
                    RevisionId::validate_str_into_with_native_admission_v1(
                        source,
                        &mut data,
                        &mut admission,
                    )
                } else {
                    RepoId::validate_str_into_with_native_admission_v1(
                        source,
                        &mut data,
                        &mut admission,
                    )
                };
                assert!(
                    matches!(result, Err(NativeIdentityConstructionErrorV1::Validation(cause)) if cause == expected)
                );
                assert_eq!(RepoId::new(source), Err(expected));
                assert_eq!(admission.releases, 0);
                if expected != IdentityValidationErrorV1::NonCanonical {
                    assert_eq!(admission.births, 0);
                    assert!(
                        data.is_fresh_v1(),
                        "raw predicate refusal did not start NFC"
                    );
                }
            }
        }
    }

    #[test]
    fn borrowed_validation_into_data_rejects_used_and_retired_scratch_without_poll_v1() {
        let source = format!("q{}", "\u{301}".repeat(20));
        for revision in [false, true] {
            let mut data = unicode_normalization::NativeNormalizationDataV1::new_v1();
            let mut admission = Admission::default();
            let first = if revision {
                RevisionId::validate_str_into_with_native_admission_v1(
                    &source,
                    &mut data,
                    &mut admission,
                )
            } else {
                RepoId::validate_str_into_with_native_admission_v1(
                    &source,
                    &mut data,
                    &mut admission,
                )
            };
            assert!(first.is_ok());
            assert!(admission.births >= 2);
            assert_eq!(admission.releases, 0);
            let work = admission.work;
            let births = admission.births;
            for retired in [false, true] {
                if retired {
                    data.release_scratch_v1(&mut admission);
                }
                // Even an empty input cannot bypass the freshness check or
                // replace the first successful NFC result with a raw error.
                let retry = RepoId::validate_str_into_with_native_admission_v1(
                    "",
                    &mut data,
                    &mut admission,
                );
                assert!(matches!(
                    retry,
                    Err(NativeIdentityConstructionErrorV1::Normalization(
                        NativeNormalizationErrorV1::InvalidNativeProducer
                    ))
                ));
                assert_eq!(
                    data.result_v1(),
                    Some(Ok(
                        unicode_normalization::NativeNormalizationOutcomeV1::IsNfc(true)
                    ))
                );
                assert_eq!(admission.work, work);
                assert_eq!(admission.births, births);
            }
        }
    }

    #[test]
    fn borrowed_into_slots_seals_canonical_repo_and_revision_only_after_success_v1() {
        let source = format!("q{}", "\u{301}".repeat(20));
        let mut backing = String::new();
        let mut output = None;
        let mut attempted = false;
        let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
        let mut admission = Admission::default();
        let mut copies = 0_usize;
        RepoId::try_from_str_into_with_native_admission_v1(
            &source,
            &mut backing,
            &mut output,
            &mut attempted,
            &mut normalization_data,
            &mut admission,
            |bytes, birth| {
                assert_eq!(bytes, 41);
                copies = copies
                    .checked_add(1_usize)
                    .expect("fixture copy count fits usize");
                Ok(birth())
            },
        )
        .unwrap();
        let repo = output.unwrap();
        assert_eq!(repo.as_str(), source);
        assert_ne!(repo.as_str().as_ptr(), source.as_ptr());
        assert!(attempted);
        assert_eq!(copies, 1);
        assert_eq!(backing.capacity(), 0);
        assert!(admission.births >= 2);
        assert_eq!(admission.releases, 0);
        normalization_data.release_scratch_v1(&mut admission);
        assert_eq!(admission.releases, 2);

        let source = "é".repeat(256);
        let mut revision = None;
        let mut attempted = false;
        let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
        RevisionId::try_from_str_into_with_native_admission_v1(
            &source,
            &mut backing,
            &mut revision,
            &mut attempted,
            &mut normalization_data,
            &mut admission,
            |bytes, birth| {
                assert_eq!(bytes, 512);
                Ok(birth())
            },
        )
        .unwrap();
        assert_eq!(revision.unwrap().as_str(), source);
        assert!(attempted);
        assert_eq!(backing.capacity(), 0);
    }

    #[test]
    fn borrowed_into_slots_preserves_invalid_input_error_order_and_blocks_retry_v1() {
        let too_long_control = format!("\n{}", "x".repeat(512));
        for (source, expected) in [
            ("", IdentityValidationErrorV1::Empty),
            (
                too_long_control.as_str(),
                IdentityValidationErrorV1::TooLong,
            ),
            ("\n", IdentityValidationErrorV1::ControlCharacter),
            ("e\u{301}", IdentityValidationErrorV1::NonCanonical),
        ] {
            let mut backing = String::new();
            let mut output = None;
            let mut attempted = false;
            let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
            let mut admission = Admission::default();
            let error = RepoId::try_from_str_into_with_native_admission_v1(
                source,
                &mut backing,
                &mut output,
                &mut attempted,
                &mut normalization_data,
                &mut admission,
                |_, _| -> Result<bool, u8> { panic!("invalid source must not be copied") },
            )
            .unwrap_err();
            assert!(
                matches!(error, NativeIdentityConstructionErrorV1::Validation(cause) if cause == expected)
            );
            assert!(attempted);
            assert!(output.is_none());
            assert_eq!(backing.capacity(), 0);
            let work = admission.work;
            admission.fail_work = Some(99);
            let error = RepoId::try_from_str_into_with_native_admission_v1(
                "valid",
                &mut backing,
                &mut output,
                &mut attempted,
                &mut normalization_data,
                &mut admission,
                |_, _| -> Result<bool, u8> { panic!("used attempt must not be copied") },
            )
            .unwrap_err();
            assert!(matches!(
                error,
                NativeIdentityConstructionErrorV1::Copy(
                    NativeIdentityCopyErrorV1::InvalidNativeProducer
                )
            ));
            assert_eq!(admission.work, work);
        }
    }

    #[test]
    fn borrowed_into_slots_preserves_occupied_storage_before_any_poll_v1() {
        for mut backing in [String::from("prior"), String::with_capacity(9)] {
            let pointer = backing.as_ptr();
            let capacity = backing.capacity();
            let expected = backing.clone();
            let mut output = None;
            let mut attempted = false;
            let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
            let mut admission = Admission {
                fail_work: Some(7),
                ..Admission::default()
            };
            let error = RevisionId::try_from_str_into_with_native_admission_v1(
                "revision",
                &mut backing,
                &mut output,
                &mut attempted,
                &mut normalization_data,
                &mut admission,
                |_, _| -> Result<bool, u8> { panic!("occupied backing must not admit") },
            )
            .unwrap_err();
            assert!(matches!(
                error,
                NativeIdentityConstructionErrorV1::Copy(
                    NativeIdentityCopyErrorV1::InvalidNativeProducer
                )
            ));
            assert_eq!(backing, expected);
            assert_eq!(backing.as_ptr(), pointer);
            assert_eq!(backing.capacity(), capacity);
            assert!(output.is_none());
            assert!(!attempted);
            assert_eq!(admission.work, 0);
        }
        let prior = RepoId::new("prior").unwrap();
        let pointer = prior.as_str().as_ptr();
        let mut output = Some(prior);
        let mut backing = String::new();
        let mut attempted = false;
        let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
        let mut admission = Admission {
            fail_work: Some(7),
            ..Admission::default()
        };
        let error = RepoId::try_from_str_into_with_native_admission_v1(
            "repository",
            &mut backing,
            &mut output,
            &mut attempted,
            &mut normalization_data,
            &mut admission,
            |_, _| -> Result<bool, u8> { panic!("occupied output must not admit") },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            NativeIdentityConstructionErrorV1::Copy(
                NativeIdentityCopyErrorV1::InvalidNativeProducer
            )
        ));
        let prior = output.unwrap();
        assert_eq!(prior.as_str(), "prior");
        assert_eq!(prior.as_str().as_ptr(), pointer);
        assert_eq!(backing.capacity(), 0);
        assert!(!attempted);
        assert_eq!(admission.work, 0);
    }

    struct SlotAdmission<'a> {
        copied: &'a std::cell::Cell<bool>,
        refuse_after_copy: Option<Box<u8>>,
        refuse_after_normalization_birth: Option<Box<u8>>,
        births: usize,
        releases: usize,
        polls: usize,
    }
    fn count_slot_call_v1(count: &mut usize) {
        let (next, overflow) = count.overflowing_add(1);
        assert!(!overflow, "fixture call count exceeds usize");
        *count = next;
    }
    impl NativeNormalizationAdmissionV1 for SlotAdmission<'_> {
        type Error = Box<u8>;
        fn checkpoint_work_v1(&mut self, _units: u64) -> Result<(), Self::Error> {
            count_slot_call_v1(&mut self.polls);
            if self.copied.get()
                && let Some(cause) = self.refuse_after_copy.take()
            {
                return Err(cause);
            }
            Ok(())
        }
        fn native_birth_v1(
            &mut self,
            _demand: NativeNormalizationScratchDemandV1,
            birth: &mut dyn FnMut() -> bool,
        ) -> Result<bool, Self::Error> {
            count_slot_call_v1(&mut self.births);
            let success = birth();
            if success && let Some(cause) = self.refuse_after_normalization_birth.take() {
                return Err(cause);
            }
            Ok(success)
        }
        fn release_scratch_v1(&mut self, _owner: NativeNormalizationScratchOwnerV1) {
            count_slot_call_v1(&mut self.releases);
        }
    }

    #[test]
    fn borrowed_into_slots_retains_noncopy_refusal_and_any_born_backing_v1() {
        for after_birth in [false, true] {
            let copied = std::cell::Cell::new(false);
            let mut admission = SlotAdmission {
                copied: &copied,
                refuse_after_copy: None,
                refuse_after_normalization_birth: None,
                births: 0,
                releases: 0,
                polls: 0,
            };
            let mut backing = String::new();
            let mut output = None;
            let mut attempted = false;
            let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
            let cause = Box::new(23);
            let pointer = std::ptr::from_ref(cause.as_ref());
            let error = RepoId::try_from_str_into_with_native_admission_v1(
                "repo/test",
                &mut backing,
                &mut output,
                &mut attempted,
                &mut normalization_data,
                &mut admission,
                |bytes, birth| {
                    assert_eq!(bytes, 9);
                    if after_birth {
                        assert!(birth());
                    }
                    Err(cause)
                },
            )
            .unwrap_err();
            assert!(matches!(&error,
                NativeIdentityConstructionErrorV1::Copy(NativeIdentityCopyErrorV1::Admission(cause))
                if std::ptr::from_ref(cause.as_ref()) == pointer && **cause == 23
            ));
            assert!(attempted);
            assert!(output.is_none());
            assert!(backing.is_empty());
            assert_eq!(backing.capacity(), if after_birth { 9 } else { 0 });
            let polls = admission.polls;
            let error = RepoId::try_from_str_into_with_native_admission_v1(
                "repo/test",
                &mut backing,
                &mut output,
                &mut attempted,
                &mut normalization_data,
                &mut admission,
                |_, _| -> Result<bool, Box<u8>> { panic!("refused attempt must not be retried") },
            )
            .unwrap_err();
            assert!(matches!(
                error,
                NativeIdentityConstructionErrorV1::Copy(
                    NativeIdentityCopyErrorV1::InvalidNativeProducer
                )
            ));
            assert_eq!(admission.polls, polls);
            assert_eq!(backing.capacity(), if after_birth { 9 } else { 0 });
        }
    }

    #[test]
    fn borrowed_into_slots_retains_normalization_cause_and_blocks_used_scratch_v1() {
        let copied = std::cell::Cell::new(false);
        let cause = Box::new(37);
        let pointer = std::ptr::from_ref(cause.as_ref());
        let mut admission = SlotAdmission {
            copied: &copied,
            refuse_after_copy: None,
            refuse_after_normalization_birth: Some(cause),
            births: 0,
            releases: 0,
            polls: 0,
        };
        let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
        let mut backing = String::new();
        let mut output = None;
        let mut attempted = false;
        let source = format!("q{}", "\u{301}".repeat(20));
        let error = RepoId::try_from_str_into_with_native_admission_v1(
            &source,
            &mut backing,
            &mut output,
            &mut attempted,
            &mut normalization_data,
            &mut admission,
            |_, _| -> Result<bool, Box<u8>> { panic!("normalization refusal precedes copy") },
        )
        .unwrap_err();
        assert!(matches!(error,
            NativeIdentityConstructionErrorV1::Normalization(NativeNormalizationErrorV1::Admission(ref cause))
            if std::ptr::from_ref(cause.as_ref()) == pointer && **cause == 37
        ));
        assert_eq!(admission.births, 1);
        assert_eq!(admission.releases, 0);
        assert!(attempted);
        assert!(output.is_none());
        assert_eq!(backing.capacity(), 0);
        // Error moved to the parent's external result; scratch did not move.
        assert!(normalization_data.result_v1().is_none());
        assert!(!normalization_data.is_fresh_v1());
        let polls = admission.polls;
        let mut fresh_attempt = false;
        let retry = RepoId::try_from_str_into_with_native_admission_v1(
            "other",
            &mut backing,
            &mut output,
            &mut fresh_attempt,
            &mut normalization_data,
            &mut admission,
            |_, _| -> Result<bool, Box<u8>> { panic!("used scratch must not copy") },
        )
        .unwrap_err();
        assert!(matches!(
            retry,
            NativeIdentityConstructionErrorV1::Copy(
                NativeIdentityCopyErrorV1::InvalidNativeProducer
            )
        ));
        assert_eq!(admission.polls, polls);
        assert!(!fresh_attempt);
        normalization_data.release_scratch_v1(&mut admission);
        assert_eq!(admission.releases, 2);
        assert!(matches!(error,
            NativeIdentityConstructionErrorV1::Normalization(NativeNormalizationErrorV1::Admission(ref cause))
            if std::ptr::from_ref(cause.as_ref()) == pointer
        ));
    }

    #[test]
    fn borrowed_into_slots_keeps_complete_unsealed_bytes_after_final_refusal_v1() {
        let copied = std::cell::Cell::new(false);
        let cause = Box::new(31);
        let cause_pointer = std::ptr::from_ref(cause.as_ref());
        let mut admission = SlotAdmission {
            copied: &copied,
            refuse_after_copy: Some(cause),
            refuse_after_normalization_birth: None,
            births: 0,
            releases: 0,
            polls: 0,
        };
        let mut backing = String::new();
        let mut output = None;
        let mut attempted = false;
        let mut normalization_data = unicode_normalization::NativeNormalizationDataV1::new_v1();
        let error = RevisionId::try_from_str_into_with_native_admission_v1(
            "rev/test",
            &mut backing,
            &mut output,
            &mut attempted,
            &mut normalization_data,
            &mut admission,
            |bytes, birth| {
                assert_eq!(bytes, 8);
                let success = birth();
                assert!(success);
                copied.set(true);
                Ok(success)
            },
        )
        .unwrap_err();
        assert!(matches!(&error,
            NativeIdentityConstructionErrorV1::Copy(NativeIdentityCopyErrorV1::Admission(cause))
            if std::ptr::from_ref(cause.as_ref()) == cause_pointer && **cause == 31
        ));
        assert!(attempted);
        assert!(output.is_none());
        assert_eq!(backing, "rev/test");
        assert_eq!(backing.capacity(), 8);
        let pointer = backing.as_ptr();
        let polls = admission.polls;
        let error = RevisionId::try_from_str_into_with_native_admission_v1(
            "other",
            &mut backing,
            &mut output,
            &mut attempted,
            &mut normalization_data,
            &mut admission,
            |_, _| -> Result<bool, Box<u8>> {
                panic!("complete refused attempt must not be retried")
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            NativeIdentityConstructionErrorV1::Copy(
                NativeIdentityCopyErrorV1::InvalidNativeProducer
            )
        ));
        assert_eq!(backing, "rev/test");
        assert_eq!(backing.as_ptr(), pointer);
        assert_eq!(admission.polls, polls);
    }
    macro_rules! native_decode_method_v1 {
        ($method:ident, $identity:ty) => {
            fn $method(
                &mut self,
                loan: &mut NativeIdentityDecodeLoanV1<'_, '_, $identity, u8, ()>,
            ) -> Result<(), &'static str> {
                loan.with_funding_v1(|runner, funding| {
                    *funding = Some(());
                    runner
                        .try_fill_v1(self, |_, birth| Ok(birth()))
                        .map_err(|_status| "native identity refused")
                })
                .map_err(|_status| "native identity loan refused")?
            }
        };
    }
    impl NativeIdentityDecodeAdmissionV1 for Admission {
        type OriginalError = u8;
        type Funding = ();
        type Error = &'static str;
        native_decode_method_v1!(repo_id_from_owned_v1, RepoId);
        native_decode_method_v1!(repo_id_from_borrowed_v1, RepoId);
        native_decode_method_v1!(revision_id_from_owned_v1, RevisionId);
        native_decode_method_v1!(revision_id_from_borrowed_v1, RevisionId);
    }
    #[test]
    fn owned_serde_refusal_retains_original_wire_backing_before_source_finishing_v1() {
        let input = String::from("e\u{301}");
        let pointer = input.as_ptr();
        let capacity = input.capacity();
        let mut data = NativeIdentityDecodeDataV1::new_v1();
        let mut admission = Admission::default();
        let mut error = None;
        assert_eq!(
            RepoId::try_decode_into_v1(
                serde::de::value::StringDeserializer::<serde_json::Error>::new(input),
                &mut admission,
                &mut data,
                &mut error
            ),
            Err(NativeIdentityDecodeDataRefusalV1::OperationRefused)
        );
        assert!(error.is_some());
        assert_eq!(data.state.backing, "e\u{301}");
        assert_eq!(data.state.backing.as_ptr(), pointer);
        assert_eq!(data.state.backing.capacity(), capacity);
        assert!(matches!(
            data.failure_v1(),
            Some(NativeIdentityConstructionErrorV1::Validation(
                IdentityValidationErrorV1::NonCanonical
            ))
        ));
        assert!(data.state.output.is_none());
    }
}
