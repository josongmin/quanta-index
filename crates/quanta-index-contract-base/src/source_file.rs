//! Source ownership is independent of the containing generation pin.
//!
//! A digest here is a producer attestation until an owner verifies immutable
//! source bytes. These identities alone do not establish parser completeness.

use crate::{ExactRepoRelativePathV1, RepoId, RepoRelativePath, RevisionId};
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
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
        ExactRepoRelativePathV1::new(self.repo_relative_path.as_str()).map(|_| ())
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
macro_rules! source_identity_codec {
    ($ty:ident, $visitor:ident, $fields:ident, {$($field:ident: $field_ty:ty),+ $(,)?}) => {
        const $fields: &[&str] = &[$(stringify!($field)),+];
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.validate().map_err(serde::ser::Error::custom)?;
                let mut state = serializer.serialize_struct(stringify!($ty), $fields.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }
        struct $visitor;
        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!("a ", stringify!($ty), " map"))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<$ty, A::Error> {
                $(let mut $field: Option<$field_ty> = None;)+
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        $(stringify!($field) => {
                            if $field.is_some() { return Err(de::Error::duplicate_field(stringify!($field))); }
                            $field = Some(map.next_value()?);
                        })+
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                let value = $ty { $($field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,)+ };
                value.validate().map_err(de::Error::custom)?;
                Ok(value)
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}

source_identity_codec!(SourceFileKey, SourceFileKeyVisitor, SOURCE_FILE_KEY_FIELDS, {
    source_repo_id: RepoId,
    repo_relative_path: RepoRelativePath,
});
source_identity_codec!(SourceFileRevision, SourceFileRevisionVisitor, SOURCE_FILE_REVISION_FIELDS, {
    file: SourceFileKey,
    revision_id: RevisionId,
    source_sha256: [u8; 32],
});

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
