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

fn validate_identity(value: &str) -> Result<(), IdentityValidationErrorV1> {
    if value.is_empty() {
        return Err(IdentityValidationErrorV1::Empty);
    }
    if value.len() > IDENTITY_MAX_UTF8_BYTES {
        return Err(IdentityValidationErrorV1::TooLong);
    }
    if value
        .chars()
        .any(|ch| matches!(ch as u32, 0x00..=0x1f | 0x7f..=0x9f))
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
        let mut bytes = Vec::with_capacity(8 + self.repo_id.0.len() + self.revision_id.0.len());
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
    target.extend_from_slice(&length[length.len() - 4..]);
    target.extend_from_slice(value);
}

fn domain_digest(domain: &str, payload: &[u8]) -> [u8; 32] {
    let domain_length = domain.len().to_be_bytes();
    let mut hasher = Sha256::new();
    hasher.update(&domain_length[domain_length.len() - 4..]);
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
    use super::{
        IdentityValidationErrorV1, LogicalGenerationIdentityV1, RepoId,
        RepositoryRevisionIdentityV1, RevisionId,
    };

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
                RepoId::new(accepted).map(|id| id.into_inner()),
                Ok(accepted.to_owned())
            );
        }
    }

    #[test]
    fn identity_serde_and_constructor_share_validation() -> Result<(), Box<dyn std::error::Error>> {
        let id = RepoId::new("repo/../%")?;
        let json = serde_json::to_string(&id)?;
        assert_eq!(serde_json::from_str::<RepoId>(json.as_str())?, id);
        assert!(serde_json::from_str::<RepoId>("\"e\\u0301\"").is_err());
        Ok(())
    }

    #[test]
    fn tuple_framing_is_injective_for_separator_collision_fixture()
    -> Result<(), Box<dyn std::error::Error>> {
        let left = RepositoryRevisionIdentityV1::new(RepoId::new("a--b")?, RevisionId::new("c")?);
        let right = RepositoryRevisionIdentityV1::new(RepoId::new("a")?, RevisionId::new("b--c")?);
        assert_ne!(left.canonical_payload(), right.canonical_payload());
        assert_ne!(left.digest(), right.digest());
        assert_ne!(
            LogicalGenerationIdentityV1::new(left, 7).digest(),
            LogicalGenerationIdentityV1::new(right, 7).digest()
        );
        Ok(())
    }
}
