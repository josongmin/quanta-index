//! Source ownership is independent of the containing generation pin.
//!
//! A digest here is a producer attestation until an owner verifies immutable
//! source bytes. These identities alone do not establish parser completeness.

use crate::admitted_decode::{AdmittedTextSeedV1, SourceIdentityDecodeAdmissionV1};
use crate::admitted_decode::{DecodeFieldGuardV1, DecodeFieldKindV1};
use crate::query::validate_exact_repo_relative_path_v1;
use crate::{RepoId, RepoRelativePath, RevisionId};
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// One source file within a containing index. Never group by path alone.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SourceFileKey {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
}

impl SourceFileKey {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_exact_repo_relative_path_v1(self.repo_relative_path.as_str())
    }
}

/// Source revision/hash, not the revision of a containing federated snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SourceFileRevision {
    pub file: SourceFileKey,
    pub revision_id: RevisionId,
    pub source_sha256: [u8; 32],
}

impl SourceFileRevision {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.file.validate()
    }
}

// Strict manual serde: every field is required, and duplicate/unknown fields
// are rejected. The same semantic validation protects encoding and decoding.
macro_rules! source_identity_encode {
    ($ty:ident, $fields:ident, {$($field:ident),+ $(,)?}) => {
        const $fields: &[&str] = &[$(stringify!($field)),+];
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.validate().map_err(serde::ser::Error::custom)?;
                let mut state = serializer.serialize_struct(stringify!($ty), $fields.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }
    };
}

source_identity_encode!(SourceFileKey, SOURCE_FILE_KEY_FIELDS, {
    source_repo_id,
    repo_relative_path,
});
source_identity_encode!(SourceFileRevision, SOURCE_FILE_REVISION_FIELDS, {
    file,
    revision_id,
    source_sha256,
});

struct SourceFieldKeySeedV1 {
    fields: &'static [&'static str],
}

impl SourceFieldKeySeedV1 {
    fn classify<E: de::Error>(&self, key: &str) -> Result<&'static str, E> {
        self.fields
            .iter()
            .copied()
            .find(|field| *field == key)
            .ok_or_else(|| E::unknown_field(key, self.fields))
    }
}

impl<'de> DeserializeSeed<'de> for SourceFieldKeySeedV1 {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for SourceFieldKeySeedV1 {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed source-file field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.classify(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted source-file field key must be borrowed"))
    }
}

fn ordinary_source_field_v1<E: de::Error>(
    key: &str,
    fields: &'static [&'static str],
) -> Result<&'static str, E> {
    SourceFieldKeySeedV1 { fields }.classify(key)
}

trait SourceFileKeyDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error>;
    fn next_repo_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error>;
    fn validate_path<E: de::Error>(&self, path: &RepoRelativePath) -> Result<(), E>;
}

struct OrdinarySourceFileKeyPolicyV1;

impl<'de> SourceFileKeyDecodePolicyV1<'de> for OrdinarySourceFileKeyPolicyV1 {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| ordinary_source_field_v1(key.as_str(), SOURCE_FILE_KEY_FIELDS))
            .transpose()
    }

    fn next_repo_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error> {
        map.next_value()
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value()
    }

    fn validate_path<E: de::Error>(&self, path: &RepoRelativePath) -> Result<(), E> {
        validate_exact_repo_relative_path_v1(path.as_str()).map_err(E::custom)
    }
}

struct SourceRepoIdSeedV1<'a, A: ?Sized>(&'a A);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for SourceRepoIdSeedV1<'_, A>
{
    type Value = RepoId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let field =
            DecodeFieldGuardV1::enter(self.0, DecodeFieldKindV1::Text, RepoId::MAX_UTF8_BYTES_V1)?;
        field.finish(self.0.decode_repo_id_v1(decoder))
    }
}

struct AdmittedSourceFileKeyPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_path_bytes: usize,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> SourceFileKeyDecodePolicyV1<'de>
    for AdmittedSourceFileKeyPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(SourceFieldKeySeedV1 {
            fields: SOURCE_FILE_KEY_FIELDS,
        })
    }

    fn next_repo_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error> {
        map.next_value_seed(SourceRepoIdSeedV1(self.admission))
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        let path = map.next_value_seed(AdmittedTextSeedV1::new_validated(
            self.admission,
            self.max_path_bytes,
            validate_exact_repo_relative_path_v1,
        ))?;
        Ok(RepoRelativePath::new(path))
    }

    fn validate_path<E: de::Error>(&self, path: &RepoRelativePath) -> Result<(), E> {
        validate_exact_repo_relative_path_v1(path.as_str()).map_err(E::custom)
    }
}

struct SourceFileKeyVisitor<P>(P);

impl<'de, P: SourceFileKeyDecodePolicyV1<'de>> Visitor<'de> for SourceFileKeyVisitor<P> {
    type Value = SourceFileKey;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SourceFileKey map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut source_repo_id = None;
        let mut repo_relative_path = None;
        while let Some(field) = self.0.next_field(&mut map)? {
            match field {
                "source_repo_id" => {
                    if source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("source_repo_id"));
                    }
                    source_repo_id = Some(self.0.next_repo_id(&mut map)?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(self.0.next_path(&mut map)?);
                }
                _ => return Err(de::Error::custom("invalid source-file key producer")),
            }
        }
        let value = SourceFileKey {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
        };
        self.0.validate_path(&value.repo_relative_path)?;
        Ok(value)
    }
}

impl<'de> Deserialize<'de> for SourceFileKey {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_struct(
            "SourceFileKey",
            SOURCE_FILE_KEY_FIELDS,
            SourceFileKeyVisitor(OrdinarySourceFileKeyPolicyV1),
        )
    }
}

impl SourceFileKey {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_path_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        AdmittedSourceFileKeyPolicyV1 {
            admission,
            max_path_bytes,
        }
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSourceFileKeyPolicyV1<'_, A>
{
    type Value = SourceFileKey;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "SourceFileKey",
            SOURCE_FILE_KEY_FIELDS,
            SourceFileKeyVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

trait SourceFileRevisionDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error>;
    fn next_file<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileKey, M::Error>;
    fn next_revision_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error>;
    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<[u8; 32], M::Error>;
    fn validate_file<E: de::Error>(&self, file: &SourceFileKey) -> Result<(), E>;
}

struct OrdinarySourceFileRevisionPolicyV1;

impl<'de> SourceFileRevisionDecodePolicyV1<'de> for OrdinarySourceFileRevisionPolicyV1 {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| ordinary_source_field_v1(key.as_str(), SOURCE_FILE_REVISION_FIELDS))
            .transpose()
    }

    fn next_file<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileKey, M::Error> {
        map.next_value()
    }

    fn next_revision_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error> {
        map.next_value()
    }

    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<[u8; 32], M::Error> {
        map.next_value()
    }

    fn validate_file<E: de::Error>(&self, file: &SourceFileKey) -> Result<(), E> {
        file.validate().map_err(E::custom)
    }
}

struct SourceRevisionIdSeedV1<'a, A: ?Sized>(&'a A);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for SourceRevisionIdSeedV1<'_, A>
{
    type Value = RevisionId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let field = DecodeFieldGuardV1::enter(
            self.0,
            DecodeFieldKindV1::Text,
            RevisionId::MAX_UTF8_BYTES_V1,
        )?;
        field.finish(self.0.decode_revision_id_v1(decoder))
    }
}

struct AdmittedSourceFileRevisionPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_path_bytes: usize,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> SourceFileRevisionDecodePolicyV1<'de>
    for AdmittedSourceFileRevisionPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(SourceFieldKeySeedV1 {
            fields: SOURCE_FILE_REVISION_FIELDS,
        })
    }

    fn next_file<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileKey, M::Error> {
        map.next_value_seed(SourceFileKey::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_path_bytes,
        ))
    }

    fn next_revision_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error> {
        map.next_value_seed(SourceRevisionIdSeedV1(self.admission))
    }

    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<[u8; 32], M::Error> {
        map.next_value()
    }

    fn validate_file<E: de::Error>(&self, file: &SourceFileKey) -> Result<(), E> {
        validate_exact_repo_relative_path_v1(file.repo_relative_path.as_str()).map_err(E::custom)
    }
}

struct SourceFileRevisionVisitor<P>(P);

impl<'de, P: SourceFileRevisionDecodePolicyV1<'de>> Visitor<'de> for SourceFileRevisionVisitor<P> {
    type Value = SourceFileRevision;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SourceFileRevision map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut file = None;
        let mut revision_id = None;
        let mut source_sha256 = None;
        while let Some(field) = self.0.next_field(&mut map)? {
            match field {
                "file" => {
                    if file.is_some() {
                        return Err(de::Error::duplicate_field("file"));
                    }
                    file = Some(self.0.next_file(&mut map)?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(self.0.next_revision_id(&mut map)?);
                }
                "source_sha256" => {
                    if source_sha256.is_some() {
                        return Err(de::Error::duplicate_field("source_sha256"));
                    }
                    source_sha256 = Some(self.0.next_digest(&mut map)?);
                }
                _ => return Err(de::Error::custom("invalid source-file revision producer")),
            }
        }
        let value = SourceFileRevision {
            file: file.ok_or_else(|| de::Error::missing_field("file"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            source_sha256: source_sha256
                .ok_or_else(|| de::Error::missing_field("source_sha256"))?,
        };
        self.0.validate_file(&value.file)?;
        Ok(value)
    }
}

impl<'de> Deserialize<'de> for SourceFileRevision {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_struct(
            "SourceFileRevision",
            SOURCE_FILE_REVISION_FIELDS,
            SourceFileRevisionVisitor(OrdinarySourceFileRevisionPolicyV1),
        )
    }
}

impl SourceFileRevision {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_path_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        AdmittedSourceFileRevisionPolicyV1 {
            admission,
            max_path_bytes,
        }
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSourceFileRevisionPolicyV1<'_, A>
{
    type Value = SourceFileRevision;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "SourceFileRevision",
            SOURCE_FILE_REVISION_FIELDS,
            SourceFileRevisionVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(repo: &str) -> SourceFileKey {
        SourceFileKey {
            source_repo_id: RepoId::new(repo).expect("fixture identity"),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
        }
    }

    #[test]
    fn same_path_different_sources_remain_distinct() {
        let keys = std::collections::BTreeSet::from([key("a"), key("b")]);
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn source_revision_roundtrip_preserves_hash_and_owner() {
        let source = SourceFileRevision {
            file: key("source"),
            revision_id: RevisionId::new("source-rev").expect("fixture revision"),
            source_sha256: [7; 32],
        };
        let raw = serde_json::to_string(&source).expect("encode");
        let decoded: SourceFileRevision = serde_json::from_str(&raw).expect("decode");
        assert_eq!(decoded, source);
    }

    #[test]
    fn missing_duplicate_unknown_and_invalid_paths_are_refused() {
        for raw in [
            r#"{"repo_relative_path":"a.rs"}"#,
            r#"{"source_repo_id":"a","source_repo_id":"b","repo_relative_path":"a.rs"}"#,
            r#"{"source_repo_id":"a","repo_relative_path":"a.rs","extra":0}"#,
            r#"{"source_repo_id":"a","repo_relative_path":"../a.rs"}"#,
            r#"{"source_repo_id":"a","repo_relative_path":"/a.rs"}"#,
            r#"{"source_repo_id":"a","repo_relative_path":""}"#,
        ] {
            assert!(serde_json::from_str::<SourceFileKey>(raw).is_err(), "{raw}");
        }
        let mut invalid = key("a");
        invalid.repo_relative_path = RepoRelativePath::new("../a.rs");
        assert!(serde_json::to_string(&invalid).is_err());
    }

    #[test]
    fn missing_or_wrong_length_digest_is_not_synthesized() {
        for raw in [
            r#"{"file":{"source_repo_id":"a","repo_relative_path":"a.rs"},"revision_id":"r"}"#,
            r#"{"file":{"source_repo_id":"a","repo_relative_path":"a.rs"},"revision_id":"r","source_sha256":[]}"#,
        ] {
            assert!(serde_json::from_str::<SourceFileRevision>(raw).is_err());
        }
    }
}

#[cfg(test)]
mod admitted_tests {
    use std::cell::Cell;

    use serde::de::DeserializeSeed;

    use super::*;
    use crate::admitted_decode::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    // Policy-only fixture. Product identity decoding must instead use native
    // identity DATA, original NFC and retained funding from the real runtime.
    struct TestAdmission {
        refuse_identity: bool,
        repo_calls: Cell<usize>,
        revision_calls: Cell<usize>,
        path_copies: Cell<usize>,
        aborted: Cell<bool>,
        attempt: std::cell::RefCell<crate::DecodeAttemptStateV1>,
    }

    impl TestAdmission {
        fn new(refuse_identity: bool) -> Self {
            Self {
                refuse_identity,
                repo_calls: Cell::new(0),
                revision_calls: Cell::new(0),
                path_copies: Cell::new(0),
                aborted: Cell::new(false),
                attempt: std::cell::RefCell::new(crate::DecodeAttemptStateV1::default()),
            }
        }
    }

    impl DecodeBackingAdmissionV1 for TestAdmission {
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
            destination
                .try_reserve_exact(additional)
                .map_err(|_physical_refusal| DecodeBackingRefusalV1)?;
            self.path_copies
                .set(self.path_copies.get().saturating_add(1));
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

        fn copy_boxed_text(&self, _source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn adopt_parser_text<E>(
            &self,
            source: &mut Option<String>,
            validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
            let value = source
                .as_deref()
                .ok_or(crate::ParserTextInputErrorV1::Admission(
                    DecodeBackingRefusalV1,
                ))?;
            validate(value).map_err(crate::ParserTextInputErrorV1::Validation)?;
            let value = source
                .take()
                .ok_or(crate::ParserTextInputErrorV1::Admission(
                    DecodeBackingRefusalV1,
                ))?;
            self.path_copies
                .set(self.path_copies.get().saturating_add(1));
            Ok(value)
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

    impl SourceIdentityDecodeAdmissionV1 for TestAdmission {
        fn decode_repo_id_v1<'de, D: Deserializer<'de>>(
            &self,
            decoder: D,
        ) -> Result<RepoId, D::Error> {
            self.repo_calls.set(self.repo_calls.get().saturating_add(1));
            if self.refuse_identity {
                return Err(de::Error::custom("fixture identity refusal"));
            }
            let value = String::deserialize(decoder)?;
            RepoId::new(value).map_err(de::Error::custom)
        }

        fn decode_revision_id_v1<'de, D: Deserializer<'de>>(
            &self,
            decoder: D,
        ) -> Result<RevisionId, D::Error> {
            self.revision_calls
                .set(self.revision_calls.get().saturating_add(1));
            if self.refuse_identity {
                return Err(de::Error::custom("fixture identity refusal"));
            }
            let value = String::deserialize(decoder)?;
            RevisionId::new(value).map_err(de::Error::custom)
        }
    }

    fn decode_key(
        wire: &str,
        owner: &TestAdmission,
        max_path_bytes: usize,
    ) -> Result<SourceFileKey, serde_json::Error> {
        let mut decoder = serde_json::Deserializer::from_str(wire);
        let value = SourceFileKey::deserialize_seed_with_admission_v1(owner, max_path_bytes)
            .deserialize(&mut decoder)?;
        decoder.end()?;
        Ok(value)
    }

    fn decode_revision(
        wire: &str,
        owner: &TestAdmission,
    ) -> Result<SourceFileRevision, serde_json::Error> {
        let mut decoder = serde_json::Deserializer::from_str(wire);
        let value = SourceFileRevision::deserialize_seed_with_admission_v1(owner, 4096)
            .deserialize(&mut decoder)?;
        decoder.end()?;
        Ok(value)
    }

    #[test]
    fn admitted_source_file_key_matches_fixed_wire_and_path_policy() -> TestResult {
        const WIRE: &str = r#"{"source_repo_id":"repo-main","repo_relative_path":"src/main.rs"}"#;
        let owner = TestAdmission::new(false);
        let admitted = decode_key(WIRE, &owner, 4096)?;
        let ordinary: SourceFileKey = serde_json::from_str(WIRE)?;
        if admitted != ordinary
            || admitted.repo_relative_path.as_str() != "src/main.rs"
            || owner.repo_calls.get() != 1
            || owner.path_copies.get() != 1
            || owner.aborted.get()
        {
            return Err("admitted key differs from fixed source-file contract".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_source_file_revision_preserves_fixed_digest_and_identities() -> TestResult {
        const WIRE: &str = r#"{"file":{"source_repo_id":"repo-main","repo_relative_path":"src/main.rs"},"revision_id":"rev-main","source_sha256":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7]}"#;
        let owner = TestAdmission::new(false);
        let admitted = decode_revision(WIRE, &owner)?;
        let ordinary: SourceFileRevision = serde_json::from_str(WIRE)?;
        if admitted != ordinary
            || admitted.source_sha256 != [7; 32]
            || owner.repo_calls.get() != 1
            || owner.revision_calls.get() != 1
            || owner.path_copies.get() != 1
            || owner.aborted.get()
        {
            return Err("admitted revision differs from fixed source-file contract".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_source_file_refuses_before_path_copy_and_aborts() -> TestResult {
        for (wire, max_path_bytes) in [
            (
                r#"{"source_repo_id":"repo-main","repo_relative_path":"../escape.rs"}"#,
                4096,
            ),
            (
                r#"{"source_repo_id":"repo-main","repo_relative_path":"src/main.rs"}"#,
                3,
            ),
        ] {
            let owner = TestAdmission::new(false);
            if decode_key(wire, &owner, max_path_bytes).is_ok()
                || owner.path_copies.get() != 0
                || !owner.aborted.get()
            {
                return Err("invalid or oversized path copied before refusal".into());
            }
        }
        let owner = TestAdmission::new(true);
        if decode_key(
            r#"{"source_repo_id":"repo-main","repo_relative_path":"src/main.rs"}"#,
            &owner,
            4096,
        )
        .is_ok()
            || owner.path_copies.get() != 0
            || !owner.aborted.get()
        {
            return Err("identity refusal permitted a later path copy".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_source_file_rejects_missing_duplicate_and_unknown_fields() -> TestResult {
        for wire in [
            r#"{"source_repo_id":"repo-main"}"#,
            r#"{"source_repo_id":"repo-main","source_repo_id":"repo-next","repo_relative_path":"src/main.rs"}"#,
            r#"{"source_repo_id":"repo-main","repo_relative_path":"src/main.rs","other":true}"#,
        ] {
            let owner = TestAdmission::new(false);
            if decode_key(wire, &owner, 4096).is_ok() || !owner.aborted.get() {
                return Err("malformed source-file key did not refuse and abort".into());
            }
        }
        Ok(())
    }
}
