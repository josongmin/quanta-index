//! Core ID newtypes shared between the producer and the search-plane.

use core::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};
use unicode_normalization::UnicodeNormalization as _;

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
/// Admission retains its caller's exact error; allocator and callback protocol failures
/// never stand in for an original resource/lifecycle refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIdentityCopyErrorV1<E> {
    Admission(E),
    NativeAllocationFailed,
    InvalidNativeProducer,
    InvalidNativeCapacity,
}
impl<E: fmt::Display> fmt::Display for NativeIdentityCopyErrorV1<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(cause) => write!(formatter, "identity copy admission: {cause}"),
            Self::NativeAllocationFailed => {
                formatter.write_str("identity copy native allocation failed")
            }
            Self::InvalidNativeProducer => {
                formatter.write_str("identity copy native producer is invalid")
            }
            Self::InvalidNativeCapacity => {
                formatter.write_str("identity copy native capacity is invalid")
            }
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for NativeIdentityCopyErrorV1<E> {}

/// Copy only the supplied borrowed bytes.
///
/// Typed owners preserve their private validation seal by wrapping this result
/// without re-validating the input.
/// The caller admits copy work and retains its native backing grant.
pub fn try_copy_string_with_native_birth_v1<E>(
    source: &str,
    admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
) -> Result<String, NativeIdentityCopyErrorV1<E>> {
    let bytes = source.len();
    let mut value = String::new();
    let mut invoked = false;
    let mut repeated = false;
    let mut native_success = false;
    let admitted = admission(bytes, &mut || {
        if invoked {
            repeated = true;
            return false;
        }
        invoked = true;
        native_success = value.try_reserve_exact(bytes).is_ok();
        native_success
    })
    .map_err(NativeIdentityCopyErrorV1::Admission)?;
    if !invoked || repeated || admitted != native_success {
        return Err(NativeIdentityCopyErrorV1::InvalidNativeProducer);
    }
    if !admitted {
        return Err(NativeIdentityCopyErrorV1::NativeAllocationFailed);
    }
    if value.capacity() != bytes {
        return Err(NativeIdentityCopyErrorV1::InvalidNativeCapacity);
    }
    value.push_str(source);
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

#[cfg(feature = "quanta-native-identity-v1")]
struct NativeIdentityValidationV1<'a, P>(&'a mut P);
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: unicode_normalization::NativeNormalizationAdmissionV1> IdentityValidationPolicyV1
    for NativeIdentityValidationV1<'_, P>
{
    type Error = unicode_normalization::NativeNormalizationErrorV1<P::Error>;
    fn character_step_v1(&mut self) -> Result<(), Self::Error> {
        self.0.checkpoint_work_v1(1).map_err(Self::Error::Admission)
    }
    fn is_nfc_v1(&mut self, value: &str) -> Result<bool, Self::Error> {
        unicode_normalization::try_is_nfc_with_native_admission_v1(value, self.0)
    }
}

#[cfg(feature = "quanta-native-identity-v1")]
fn validate_native_identity_v1<P: unicode_normalization::NativeNormalizationAdmissionV1>(
    value: &str,
    admission: &mut P,
) -> Result<(), NativeIdentityConstructionErrorV1<P::Error>> {
    match validate_identity_with_policy_v1(value, &mut NativeIdentityValidationV1(admission)) {
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
    type Error: fmt::Display;
    fn repo_id_from_owned_v1(&self, value: String) -> Result<RepoId, Self::Error>;
    fn repo_id_from_borrowed_v1(&self, value: &str) -> Result<RepoId, Self::Error>;
    fn revision_id_from_owned_v1(&self, value: String) -> Result<RevisionId, Self::Error>;
    fn revision_id_from_borrowed_v1(&self, value: &str) -> Result<RevisionId, Self::Error>;
}

trait IdentityConstructionPolicyV1<T> {
    type Error: fmt::Display;
    fn from_owned_v1(&self, value: String) -> Result<T, Self::Error>;
    fn from_borrowed_v1(&self, value: &str) -> Result<T, Self::Error>;
}
struct OrdinaryIdentityConstructionV1;
#[cfg(feature = "quanta-native-identity-v1")]
struct NativeIdentityConstructionV1<'a, P: ?Sized>(&'a P);

trait IdentityDecodeValueV1: Sized {
    fn decode_with_policy_v1<'de, D, P>(deserializer: D, policy: P) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
        P: IdentityConstructionPolicyV1<Self>;
}

#[cfg(feature = "quanta-native-identity-v1")]
struct IdentityDecodeSeedV1<T, P> {
    policy: P,
    value: core::marker::PhantomData<T>,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<'de, T: IdentityDecodeValueV1, P: IdentityConstructionPolicyV1<T>> de::DeserializeSeed<'de>
    for IdentityDecodeSeedV1<T, P>
{
    type Value = T;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        T::decode_with_policy_v1(deserializer, self.policy)
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

            /// Validate borrowed input at the same predicate/NFC producers,
            /// then perform exactly one admitted owned copy. The normalization
            /// policy owns temporary scratch grants; the copy admission retains
            /// the resulting String backing with the caller's original group.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn try_from_str_with_native_admission_v1<P>(
                value: &str,
                normalization_admission: &mut P,
                copy_admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, P::Error>,
            ) -> Result<Self, NativeIdentityConstructionErrorV1<P::Error>>
            where
                P: unicode_normalization::NativeNormalizationAdmissionV1,
            {
                validate_native_identity_v1(value, normalization_admission)?;
                let copy_work = u64::try_from(value.len()).map_err(|_| {
                    NativeIdentityConstructionErrorV1::Normalization(
                        unicode_normalization::NativeNormalizationErrorV1::ArithmeticOverflow,
                    )
                })?;
                normalization_admission.checkpoint_work_v1(copy_work)
                    .map_err(|cause| NativeIdentityConstructionErrorV1::Copy(NativeIdentityCopyErrorV1::Admission(cause)))?;
                let value = try_copy_string_with_native_birth_v1(value, copy_admission)
                    .map_err(NativeIdentityConstructionErrorV1::Copy)?;
                normalization_admission.checkpoint_work_v1(0)
                    .map_err(|cause| NativeIdentityConstructionErrorV1::Copy(NativeIdentityCopyErrorV1::Admission(cause)))?;
                Ok(Self(value))
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
            /// Owned JSON Strings move to the canonical constructor unchanged.
            /// The caller's deserializer must admit owned and escaped string
            /// backing before allocation, as required by the admission trait.
            #[cfg(feature = "quanta-native-identity-v1")]
            pub fn native_decode_seed_v1<'a, 'de, P: NativeIdentityDecodeAdmissionV1 + ?Sized>(
                admission: &'a P,
            ) -> impl de::DeserializeSeed<'de, Value = Self> + 'a {
                IdentityDecodeSeedV1::<Self, _> {
                    policy: NativeIdentityConstructionV1(admission),
                    value: core::marker::PhantomData,
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

            /// Copy these exact private canonical bytes without re-running NFC
            /// or constructing a second identity authority. The caller admits
            /// copy work before this call, admits actual backing before the
            /// supplied native callback, and retains its grant with the result.
            pub fn try_clone_with_native_birth_v1<E>(
                &self,
                admission: impl FnOnce(usize, &mut dyn FnMut() -> bool) -> Result<bool, E>,
            ) -> Result<Self, NativeIdentityCopyErrorV1<E>> {
                try_copy_string_with_native_birth_v1(self.0.as_str(), admission).map(Self)
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
            fn from_owned_v1(&self, value: String) -> Result<$name, Self::Error> { $name::new(value) }
            fn from_borrowed_v1(&self, value: &str) -> Result<$name, Self::Error> { $name::new(value) }
        }
        #[cfg(feature = "quanta-native-identity-v1")]
        impl<P: NativeIdentityDecodeAdmissionV1 + ?Sized> IdentityConstructionPolicyV1<$name>
            for NativeIdentityConstructionV1<'_, P>
        {
            type Error = P::Error;
            fn from_owned_v1(&self, value: String) -> Result<$name, Self::Error> { self.0.$owned_v1(value) }
            fn from_borrowed_v1(&self, value: &str) -> Result<$name, Self::Error> { self.0.$borrowed_v1(value) }
        }

        impl IdentityDecodeValueV1 for $name {
            fn decode_with_policy_v1<'de, D, P>(deserializer: D, policy: P) -> Result<Self, D::Error>
            where D: Deserializer<'de>, P: IdentityConstructionPolicyV1<Self>,
            {
                struct IdentityVisitor<P>(P);

                impl<'de, P: IdentityConstructionPolicyV1<$name>> de::Visitor<'de> for IdentityVisitor<P> {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("a canonical ", stringify!($name), " string"))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.0.from_borrowed_v1(value).map_err(E::custom)
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.0.from_owned_v1(value).map_err(E::custom)
                    }
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
                assert!(!birth());
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
            self.births += 1;
            let success = birth();
            if success && let Some(cause) = self.fail_after_birth {
                return Err(cause);
            }
            Ok(success)
        }
        fn release_scratch_v1(&mut self, _owner: NativeNormalizationScratchOwnerV1) {
            self.releases += 1;
        }
    }

    #[test]
    fn raw_ids_reuse_unicode_validation_and_copy_once_v1() {
        // Equal combining classes are NFC-canonical here, and the run exceeds
        // both canonical iterators' inline scratch. No ASCII-only shortcut.
        let source = format!("q{}", "\u{301}".repeat(20));
        let expected = RepoId::new(source.as_str()).unwrap();
        let mut admission = Admission::default();
        let mut copies = 0;
        let copied = RepoId::try_from_str_with_native_admission_v1(
            &source,
            &mut admission,
            |bytes, birth| {
                assert_eq!(bytes, source.len());
                copies += 1;
                Ok(birth())
            },
        )
        .unwrap();
        assert_eq!(copied, expected);
        assert_ne!(copied.as_str().as_ptr(), source.as_ptr());
        assert_eq!(copies, 1);
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
}
