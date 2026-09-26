//! Source-bound capability and producer-event commitments. Missing coverage
//! cannot be interpreted as an empty, completely indexed source universe.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;

use quanta_index_contract_base::{SourceFileKey, SourceFileRevision};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};

use crate::ChunkRecord;
use crate::lex::{LanguageCode, SymbolRecord};

/// Completeness is explicit and independent of the number of indexed facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolCoverage {
    Complete { symbol_count: u64 },
    NotRequested,
    Unsupported,
    ParseFailed,
    ProducerFailed,
}

/// Producer-attested source revision and policy, bound to the exact unit set.
/// The source hash is not independent verification of bytes absent from ingress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileCoverage {
    pub source: SourceFileRevision,
    pub language: LanguageCode,
    pub producer_policy_sha256: [u8; 32],
    pub unit_set_sha256: [u8; 32],
    pub text_admitted: bool,
    pub symbols: SymbolCoverage,
}

/// The in-memory snapshot. Persisted decoders must validate sorted unique rows
/// before constructing this map; map deserialization can hide duplicate keys.
pub type FileCoverageSnapshot = BTreeMap<SourceFileKey, SourceFileCoverage>;

/// An event is identified independently of a target generation or ingest batch.
/// The durable publication owner checks replay and expected-base lineage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePublicationEvent {
    pub stream_id: String,
    pub event_id: String,
    pub expected_base_event_id: Option<String>,
    pub payload_sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceCoverageError {
    InvalidSource(&'static str),
    InvalidEventToken { field: &'static str },
    SelfReferentialEvent,
    DuplicateCandidateId(String),
    Encode(String),
}

impl fmt::Display for SourceCoverageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource(error) => write!(formatter, "source-file identity: {error}"),
            Self::InvalidEventToken { field } => write!(
                formatter,
                "source event {field} must contain 1..=512 printable ASCII bytes without whitespace"
            ),
            Self::SelfReferentialEvent => {
                formatter.write_str("source event cannot be its own base")
            }
            Self::DuplicateCandidateId(id) => {
                write!(formatter, "duplicate source-file unit ID: {id}")
            }
            Self::Encode(error) => write!(formatter, "source-file unit encoding: {error}"),
        }
    }
}

impl std::error::Error for SourceCoverageError {}

impl SourceFileCoverage {
    pub fn validate(&self) -> Result<(), SourceCoverageError> {
        self.source
            .validate()
            .map_err(SourceCoverageError::InvalidSource)
    }
}

impl SourcePublicationEvent {
    pub fn validate(&self) -> Result<(), SourceCoverageError> {
        for (field, value) in [
            ("stream_id", Some(self.stream_id.as_str())),
            ("event_id", Some(self.event_id.as_str())),
            (
                "expected_base_event_id",
                self.expected_base_event_id.as_deref(),
            ),
        ] {
            if let Some(value) = value
                && (value.is_empty()
                    || value.len() > 512
                    || !value.bytes().all(|byte| byte.is_ascii_graphic()))
            {
                return Err(SourceCoverageError::InvalidEventToken { field });
            }
        }
        if self.expected_base_event_id.as_deref() == Some(self.event_id.as_str()) {
            return Err(SourceCoverageError::SelfReferentialEvent);
        }
        Ok(())
    }
}

impl Serialize for SymbolCoverage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (tag, count) = match *self {
            Self::Complete { symbol_count } => ("complete", Some(symbol_count)),
            Self::NotRequested => ("not_requested", None),
            Self::Unsupported => ("unsupported", None),
            Self::ParseFailed => ("parse_failed", None),
            Self::ProducerFailed => ("producer_failed", None),
        };
        let mut state =
            serializer.serialize_struct("SymbolCoverage", if count.is_some() { 2 } else { 1 })?;
        state.serialize_field("state", tag)?;
        if let Some(count) = count {
            state.serialize_field("symbol_count", &count)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for SymbolCoverage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CoverageVisitor;
        impl<'de> Visitor<'de> for CoverageVisitor {
            type Value = SymbolCoverage;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an explicit symbol coverage state")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut state: Option<String> = None;
                let mut count: Option<u64> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "state" => {
                            if state.is_some() {
                                return Err(de::Error::duplicate_field("state"));
                            }
                            state = Some(map.next_value()?);
                        }
                        "symbol_count" => {
                            if count.is_some() {
                                return Err(de::Error::duplicate_field("symbol_count"));
                            }
                            count = Some(map.next_value::<u64>()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                &["state", "symbol_count"],
                            ));
                        }
                    }
                }
                let state = state.ok_or_else(|| de::Error::missing_field("state"))?;
                if state == "complete" {
                    return Ok(SymbolCoverage::Complete {
                        symbol_count: count
                            .ok_or_else(|| de::Error::missing_field("symbol_count"))?,
                    });
                }
                if count.is_some() {
                    return Err(de::Error::custom(
                        "only complete coverage has a symbol_count",
                    ));
                }
                match state.as_str() {
                    "not_requested" => Ok(SymbolCoverage::NotRequested),
                    "unsupported" => Ok(SymbolCoverage::Unsupported),
                    "parse_failed" => Ok(SymbolCoverage::ParseFailed),
                    "producer_failed" => Ok(SymbolCoverage::ProducerFailed),
                    _ => Err(de::Error::unknown_variant(
                        &state,
                        &[
                            "complete",
                            "not_requested",
                            "unsupported",
                            "parse_failed",
                            "producer_failed",
                        ],
                    )),
                }
            }
        }
        deserializer.deserialize_map(CoverageVisitor)
    }
}

// Closed maps: unknown, missing and duplicate fields all fail. The outer Option
// tracks presence separately from a nullable field's actual value.
macro_rules! coverage_record_serde {
    ($name:ident { $($field:ident: $ty:ty),+ $(,)? } validate $validate:expr) => {
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                ($validate)(self).map_err(serde::ser::Error::custom)?;
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut record = serializer.serialize_struct(stringify!($name), FIELDS.len())?;
                $(record.serialize_field(stringify!($field), &self.$field)?;)+
                record.end()
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                struct RecordVisitor;
                impl<'de> Visitor<'de> for RecordVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("a complete ", stringify!($name), " map"))
                    }
                    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        $(let mut $field: Option<$ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(de::Error::duplicate_field(stringify!($field)));
                                    }
                                    $field = Some(map.next_value::<$ty>()?);
                                },)+
                                _ => return Err(de::Error::unknown_field(&key, FIELDS)),
                            }
                        }
                        let record = $name {
                            $($field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,)+
                        };
                        ($validate)(&record).map_err(de::Error::custom)?;
                        Ok(record)
                    }
                }
                deserializer.deserialize_map(RecordVisitor)
            }
        }
    };
}

coverage_record_serde!(SourceFileCoverage {
    source: SourceFileRevision,
    language: LanguageCode,
    producer_policy_sha256: [u8; 32],
    unit_set_sha256: [u8; 32],
    text_admitted: bool,
    symbols: SymbolCoverage,
} validate SourceFileCoverage::validate);

coverage_record_serde!(SourcePublicationEvent {
    stream_id: String,
    event_id: String,
    expected_base_event_id: Option<String>,
    payload_sha256: [u8; 32],
} validate SourcePublicationEvent::validate);

struct DigestWriter(Sha256);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// SHA-256 of the domain followed by a fixed-order CBOR pair `(chunks, symbols)`.
/// Each vector is sorted by its typed ID and uses the complete record's manual
/// wire serializer. Input order has no authority; duplicate IDs never collapse.
pub fn source_file_unit_set_sha256(
    chunks: &[ChunkRecord],
    symbols: &[SymbolRecord],
) -> Result<[u8; 32], SourceCoverageError> {
    let mut ids = BTreeSet::new();
    for id in chunks
        .iter()
        .map(|chunk| chunk.chunk_id.as_str())
        .chain(symbols.iter().map(|symbol| symbol.symbol_id.as_str()))
    {
        if !ids.insert(id) {
            return Err(SourceCoverageError::DuplicateCandidateId(id.to_owned()));
        }
    }
    let mut chunks: Vec<_> = chunks.iter().collect();
    let mut symbols: Vec<_> = symbols.iter().collect();
    chunks.sort_by(|left, right| left.chunk_id.as_str().cmp(right.chunk_id.as_str()));
    symbols.sort_by(|left, right| left.symbol_id.as_str().cmp(right.symbol_id.as_str()));
    let mut writer = DigestWriter(Sha256::new());
    writer.0.update(b"quanta-index:source-file-unit-set:v1\0");
    ciborium::into_writer(&(chunks, symbols), &mut writer)
        .map_err(|error| SourceCoverageError::Encode(error.to_string()))?;
    Ok(writer.0.finalize().into())
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]
mod tests {
    use super::*;
    use crate::{ChunkId, RepoId, RepoRelativePath, RevisionId};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn event() -> SourcePublicationEvent {
        SourcePublicationEvent {
            stream_id: "producer-main".into(),
            event_id: "event-2".into(),
            expected_base_event_id: Some("event-1".into()),
            payload_sha256: [3; 32],
        }
    }

    fn coverage() -> Result<SourceFileCoverage, Box<dyn std::error::Error>> {
        Ok(SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source")?,
                    repo_relative_path: RepoRelativePath::new("empty.rs"),
                },
                revision_id: RevisionId::new("revision")?,
                source_sha256: [2; 32],
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [1; 32],
            unit_set_sha256: source_file_unit_set_sha256(&[], &[])?,
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 0 },
        })
    }

    #[test]
    fn all_symbol_states_and_complete_zero_round_trip_distinctly() -> TestResult {
        let states = [
            SymbolCoverage::Complete { symbol_count: 0 },
            SymbolCoverage::Complete { symbol_count: 2 },
            SymbolCoverage::NotRequested,
            SymbolCoverage::Unsupported,
            SymbolCoverage::ParseFailed,
            SymbolCoverage::ProducerFailed,
        ];
        let mut encoded = BTreeSet::new();
        for state in states {
            let mut bytes = Vec::new();
            ciborium::into_writer(&state, &mut bytes)?;
            assert_eq!(
                ciborium::from_reader::<SymbolCoverage, _>(bytes.as_slice())?,
                state
            );
            assert!(encoded.insert(bytes));
        }
        Ok(())
    }

    #[test]
    fn malformed_symbol_states_never_become_complete() {
        for invalid in [
            r#"{}"#,
            r#"{"state":"complete"}"#,
            r#"{"state":"complete","symbol_count":null}"#,
            r#"{"state":"complete","symbol_count":-1}"#,
            r#"{"state":"complete","symbol_count":0,"symbol_count":1}"#,
            r#"{"state":"complete","symbol_count":0,"extra":false}"#,
            r#"{"state":"unknown"}"#,
            r#"{"state":"parse_failed","symbol_count":0}"#,
            r#"{"state":"parse_failed","state":"complete","symbol_count":0}"#,
        ] {
            assert!(
                serde_json::from_str::<SymbolCoverage>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn coverage_and_event_closed_maps_reject_missing_unknown_duplicate_fields() -> TestResult {
        fn exercise<T>(value: &T) -> TestResult
        where
            T: Serialize + for<'de> Deserialize<'de> + Eq + fmt::Debug,
        {
            let mut bytes = Vec::new();
            ciborium::into_writer(value, &mut bytes)?;
            assert_eq!(&ciborium::from_reader::<T, _>(bytes.as_slice())?, value);
            let ciborium::Value::Map(fields) = ciborium::from_reader(bytes.as_slice())? else {
                return Err("fixture must encode a map".into());
            };
            for field in &fields {
                let mut duplicate = fields.clone();
                duplicate.push(field.clone());
                let missing = fields
                    .iter()
                    .filter(|entry| entry.0 != field.0)
                    .cloned()
                    .collect();
                let mut unknown = fields.clone();
                unknown.push((
                    ciborium::Value::Text("unknown".into()),
                    ciborium::Value::Null,
                ));
                for invalid in [duplicate, missing, unknown] {
                    let mut bytes = Vec::new();
                    ciborium::into_writer(&ciborium::Value::Map(invalid), &mut bytes)?;
                    assert!(ciborium::from_reader::<T, _>(bytes.as_slice()).is_err());
                }
            }
            Ok(())
        }
        exercise(&coverage()?)?;
        exercise(&event())?;
        let mut initial = event();
        initial.expected_base_event_id = None;
        exercise(&initial)
    }

    #[test]
    fn event_tokens_are_bounded_and_validate_before_serialization() -> TestResult {
        for invalid in [
            "".to_string(),
            "a b".into(),
            "a\n".into(),
            "é".into(),
            "x".repeat(513),
        ] {
            for field in ["stream", "event", "base"] {
                let mut request = event();
                match field {
                    "stream" => request.stream_id.clone_from(&invalid),
                    "event" => request.event_id.clone_from(&invalid),
                    _ => request.expected_base_event_id = Some(invalid.clone()),
                }
                assert!(request.validate().is_err());
                assert!(serde_json::to_string(&request).is_err());
            }
        }
        let mut boundary = event();
        boundary.event_id = "x".repeat(512);
        boundary.validate()?;
        boundary.expected_base_event_id = Some(boundary.event_id.clone());
        assert_eq!(
            boundary.validate(),
            Err(SourceCoverageError::SelfReferentialEvent)
        );
        Ok(())
    }

    fn chunk(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new("a.rs"),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: text.into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    }

    #[test]
    fn unit_digest_is_order_independent_and_binds_complete_records() -> TestResult {
        let first = chunk("a", "alpha")?;
        let second = chunk("b", "beta")?;
        let canonical = source_file_unit_set_sha256(&[first.clone(), second.clone()], &[])?;
        assert_eq!(
            canonical,
            source_file_unit_set_sha256(&[second.clone(), first.clone()], &[])?
        );
        for field in ["text", "path", "source", "span", "parent"] {
            let mut changed = first.clone();
            match field {
                "text" => changed.text = "other".into(),
                "path" => changed.repo_relative_path = RepoRelativePath::new("b.rs"),
                "source" => changed.source_repo_id = Some(RepoId::new("different-source")?),
                "span" => changed.start_line = 2,
                _ => changed.parent_chunk_id = Some(ChunkId::new("parent")),
            }
            assert_ne!(
                canonical,
                source_file_unit_set_sha256(&[changed, second.clone()], &[])?
            );
        }
        assert!(matches!(
            source_file_unit_set_sha256(&[first.clone(), first], &[]),
            Err(SourceCoverageError::DuplicateCandidateId(_))
        ));
        Ok(())
    }

    #[test]
    fn empty_unit_digest_matches_fixed_cbor_golden() -> TestResult {
        // Independent oracle: SHA256(domain || 0x82 0x80 0x80), the CBOR
        // pair of empty arrays. No source-file-byte completeness claim.
        let digest = source_file_unit_set_sha256(&[], &[])?;
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "03b78693ecc7e7294c11af4366b387162c2ebe9547683a8ac614a717f0027d32"
        );
        Ok(())
    }

    #[test]
    fn symbol_records_are_committed_and_cross_kind_ids_are_rejected() -> TestResult {
        use crate::SymbolId;
        use crate::lex::{SymbolKindCode, SymbolKindFamily, SymbolRelationship, SymbolSpan};

        let symbol = SymbolRecord {
            symbol_id: SymbolId::new("symbol-a"),
            repo_relative_path: RepoRelativePath::new("a.rs"),
            language: LanguageCode::new("rust")?,
            symbol_kind: SymbolKindCode::new("function")?,
            symbol_kind_family: Some(SymbolKindFamily::Callable),
            local_name: "name".into(),
            qualified_name: "crate::name".into(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: "a.rs".into(),
                byte_start: 0,
                byte_end: 4,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: None,
            relationship: SymbolRelationship::Def,
        };
        let canonical = source_file_unit_set_sha256(&[], std::slice::from_ref(&symbol))?;
        let mut renamed = symbol.clone();
        renamed.qualified_name = "other::name".into();
        assert_ne!(canonical, source_file_unit_set_sha256(&[], &[renamed])?);
        let mut changed_signature = symbol.clone();
        changed_signature.signature = Some("fn name(x: u32)".into());
        assert_ne!(
            canonical,
            source_file_unit_set_sha256(&[], &[changed_signature])?
        );
        assert!(matches!(
            source_file_unit_set_sha256(
                &[chunk("symbol-a", "name")?],
                std::slice::from_ref(&symbol)
            ),
            Err(SourceCoverageError::DuplicateCandidateId(_))
        ));
        assert!(matches!(
            source_file_unit_set_sha256(&[], &[symbol.clone(), symbol]),
            Err(SourceCoverageError::DuplicateCandidateId(_))
        ));
        Ok(())
    }
}
