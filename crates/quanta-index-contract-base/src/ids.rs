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

/// Native copy failure at the already-validated identity producer. Admission
/// retains its caller's exact error; allocator and callback protocol failures
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

/// Copy only the supplied borrowed bytes. Typed owners preserve their private
/// validation seal by wrapping this result without re-validating the input.
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

fn validate_identity(value: &str) -> Result<(), IdentityValidationErrorV1> {
    if value.is_empty() {
        return Err(IdentityValidationErrorV1::Empty);
    }
    if value.len() > IDENTITY_MAX_UTF8_BYTES {
        return Err(IdentityValidationErrorV1::TooLong);
    }
    if value
        .chars()
        .any(|ch| matches!(u32::from(ch), 0x00..=0x1f | 0x7f..=0x9f))
    {
        return Err(IdentityValidationErrorV1::ControlCharacter);
    }
    if !value.nfc().eq(value.chars()) {
        return Err(IdentityValidationErrorV1::NonCanonical);
    }
    Ok(())
}

macro_rules! validated_identity {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Construct only from an already-canonical identity. This never normalizes input.
            pub fn new(value: impl Into<String>) -> Result<Self, IdentityValidationErrorV1> {
                let value = value.into();
                validate_identity(value.as_str())?;
                Ok(Self(value))
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

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct IdentityVisitor;

                impl<'de> de::Visitor<'de> for IdentityVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("a canonical ", stringify!($name), " string"))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::new(value).map_err(E::custom)
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::new(value).map_err(E::custom)
                    }
                }

                deserializer.deserialize_string(IdentityVisitor)
            }
        }
    };
}

validated_identity!(RepoId);
validated_identity!(RevisionId);

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
            Ok(revision.clone())
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
