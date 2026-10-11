//! Explicit adapter-test publication custody, below the real activation catalog.
//!
//! The persistent fixture preserves original ranges through reopen/crash tests.
//! Production catalog allocation is covered by search-plane owner tests.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write as _;
use std::sync::Mutex;

use quanta_index_contract::{
    LexicalBuildStageDurationsV1, SearchCorpusIngestBatch, SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, LexicalDocumentNamespaceV1, LexicalDocumentVersionRangeV1,
    LexicalDocumentVersionReservationV1, PublicationValidationOwner, SearchCorpusBatchBuildPort,
    SourceEventAllocationRequestV1, SourceEventAllocationV1,
};
use quanta_index_lexical::LexicalAdapter;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

static FIXTURE_LOCK: Mutex<()> = Mutex::new(());
const INCARNATION: [u8; 16] = [0x4b; 16];

#[derive(Default)]
struct FixtureState {
    file_high_water: u64,
    native_high_water: u64,
    rows: BTreeMap<String, FixtureRow>,
}

struct FixtureRow {
    event: SourcePublicationEvent,
    revision: String,
    generation: u64,
    file: (u64, u64),
    native: (u64, u64),
}

impl Serialize for FixtureState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("FixtureState", 3)?;
        row.serialize_field("file_high_water", &self.file_high_water)?;
        row.serialize_field("native_high_water", &self.native_high_water)?;
        row.serialize_field("rows", &self.rows)?;
        row.end()
    }
}

impl<'de> Deserialize<'de> for FixtureState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StateVisitor;

        impl<'de> Visitor<'de> for StateVisitor {
            type Value = FixtureState;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a fixture state map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut file_high_water = None;
                let mut native_high_water = None;
                let mut rows = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "file_high_water" => {
                            if file_high_water.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("file_high_water"));
                            }
                        }
                        "native_high_water" => {
                            if native_high_water.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("native_high_water"));
                            }
                        }
                        "rows" => {
                            if rows.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("rows"));
                            }
                        }
                        _ => {
                            let _ignored: IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(FixtureState {
                    file_high_water: file_high_water
                        .ok_or_else(|| serde::de::Error::missing_field("file_high_water"))?,
                    native_high_water: native_high_water
                        .ok_or_else(|| serde::de::Error::missing_field("native_high_water"))?,
                    rows: rows.ok_or_else(|| serde::de::Error::missing_field("rows"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "FixtureState",
            &["file_high_water", "native_high_water", "rows"],
            StateVisitor,
        )
    }
}

impl Serialize for FixtureRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("FixtureRow", 5)?;
        row.serialize_field("event", &self.event)?;
        row.serialize_field("revision", &self.revision)?;
        row.serialize_field("generation", &self.generation)?;
        row.serialize_field("file", &self.file)?;
        row.serialize_field("native", &self.native)?;
        row.end()
    }
}

impl<'de> Deserialize<'de> for FixtureRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor;

        impl<'de> Visitor<'de> for RowVisitor {
            type Value = FixtureRow;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a fixture reservation row map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut event = None;
                let mut revision = None;
                let mut generation = None;
                let mut file = None;
                let mut native = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "event" => {
                            if event.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("event"));
                            }
                        }
                        "revision" => {
                            if revision.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("revision"));
                            }
                        }
                        "generation" => {
                            if generation.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                        }
                        "file" => {
                            if file.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("file"));
                            }
                        }
                        "native" => {
                            if native.replace(map.next_value()?).is_some() {
                                return Err(serde::de::Error::duplicate_field("native"));
                            }
                        }
                        _ => {
                            let _ignored: IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(FixtureRow {
                    event: event.ok_or_else(|| serde::de::Error::missing_field("event"))?,
                    revision: revision
                        .ok_or_else(|| serde::de::Error::missing_field("revision"))?,
                    generation: generation
                        .ok_or_else(|| serde::de::Error::missing_field("generation"))?,
                    file: file.ok_or_else(|| serde::de::Error::missing_field("file"))?,
                    native: native.ok_or_else(|| serde::de::Error::missing_field("native"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "FixtureRow",
            &["event", "revision", "generation", "file", "native"],
            RowVisitor,
        )
    }
}

fn failure(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("adapter publication fixture: {error}"))
}

fn reserve_range(high_water: &mut u64, count: u64) -> Result<(u64, u64), CoreError> {
    if count == 0 {
        return Ok((0, 0));
    }
    let first = high_water
        .checked_add(1)
        .ok_or_else(|| failure("ID exhaustion"))?;
    *high_water = high_water
        .checked_add(count)
        .ok_or_else(|| failure("ID exhaustion"))?;
    Ok((first, count))
}

fn bind_fixture(
    adapter: &LexicalAdapter,
    batch: &SearchCorpusIngestBatch,
    owner: &mut PublicationValidationOwner,
) -> Result<(), CoreError> {
    batch.validate_surface_mutations_v1().map_err(|error| {
        CoreError::InvalidContract(format!("lexical: file mutation admission: {error}"))
    })?;
    batch.validate_v1().map_err(|error| {
        CoreError::InvalidContract(format!("lexical: batch admission: {error}"))
    })?;
    let _guard = FIXTURE_LOCK.lock().map_err(failure)?;
    let root = adapter.state_root();
    let path = root.join(".adapter-test-publication-ranges.json");
    let mut state: FixtureState = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(failure)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FixtureState::default(),
        Err(error) => return Err(failure(error)),
    };
    let file_count = u64::try_from(batch.replace_scopes.len()).map_err(failure)?;
    let native_count = batch.replace_scopes.iter().try_fold(0_u64, |sum, scope| {
        sum.checked_add(u64::try_from(scope.chunks.len()).map_err(failure)?)
            .ok_or_else(|| failure("chunk count overflow"))
    })?;
    let key = serde_json::to_string(&(
        batch.repo_id.as_str(),
        &batch.source_event.stream_id,
        &batch.source_event.event_id,
    ))
    .map_err(failure)?;
    if !state.rows.contains_key(&key) {
        let file = reserve_range(&mut state.file_high_water, file_count)?;
        let native = reserve_range(&mut state.native_high_water, native_count)?;
        let _prior = state.rows.insert(
            key.clone(),
            FixtureRow {
                event: batch.source_event.clone(),
                revision: batch.revision_id.as_str().to_owned(),
                generation: batch.generation.get(),
                file,
                native,
            },
        );
        std::fs::create_dir_all(root).map_err(failure)?;
        let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(failure)?;
        temporary
            .write_all(&serde_json::to_vec(&state).map_err(failure)?)
            .map_err(failure)?;
        temporary.as_file().sync_all().map_err(failure)?;
        let _persisted = temporary.persist(&path).map_err(failure)?;
        std::fs::File::open(root)
            .and_then(|directory| directory.sync_all())
            .map_err(failure)?;
    }
    let row = state
        .rows
        .get(&key)
        .ok_or_else(|| failure("reservation disappeared"))?;
    if row.event != batch.source_event
        || row.revision != batch.revision_id.as_str()
        || row.generation != batch.generation.get()
        || row.file.1 != file_count
        || row.native.1 != native_count
    {
        return Err(CoreError::InvalidContract(
            "fixture event differs from its original reservation".into(),
        ));
    }
    let range = |(first, count), namespace| LexicalDocumentVersionRangeV1 {
        first,
        count,
        scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
            &INCARNATION,
            &batch.repo_id,
            namespace,
        ),
    };
    let reservation = LexicalDocumentVersionReservationV1 {
        allocation: SourceEventAllocationV1 {
            request: SourceEventAllocationRequestV1 {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                event: batch.source_event.clone(),
            },
            generation: batch.generation,
            root_incarnation: INCARNATION,
        },
        file: range(row.file, LexicalDocumentNamespaceV1::File),
        native_text: range(row.native, LexicalDocumentNamespaceV1::NativeText),
    };
    owner.bind_source_allocation(batch, &reservation.allocation)?;
    owner.bind_lexical_reservation(batch, &reservation)
}

/// Test calls explicitly opt into fixture custody; the production entry point
/// continues to refuse an unreserved publication.
pub(crate) trait FixtureBatchBuildPort {
    fn build_fixture_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<Option<LexicalBuildStageDurationsV1>, CoreError>;
    fn build_fixture_batch_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut PublicationValidationOwner,
    ) -> Result<Option<LexicalBuildStageDurationsV1>, CoreError>;
}

impl FixtureBatchBuildPort for LexicalAdapter {
    fn build_fixture_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<Option<LexicalBuildStageDurationsV1>, CoreError> {
        FixtureBatchBuildPort::build_fixture_batch_with_owner(
            self,
            batch,
            &mut PublicationValidationOwner::default(),
        )
    }
    fn build_fixture_batch_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut PublicationValidationOwner,
    ) -> Result<Option<LexicalBuildStageDurationsV1>, CoreError> {
        bind_fixture(self, batch, owner)?;
        SearchCorpusBatchBuildPort::build_batch_with_owner(self, batch, owner, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_reservation_json_keeps_named_fields_and_rejects_missing_or_duplicate_fields() {
        let state = FixtureState {
            file_high_water: 7,
            native_high_water: 9,
            rows: BTreeMap::new(),
        };
        let literal = r#"{"file_high_water":7,"native_high_water":9,"rows":{}}"#;
        assert!(matches!(serde_json::to_string(&state), Ok(encoded) if encoded == literal));
        assert!(matches!(
            serde_json::from_str::<FixtureState>(literal),
            Ok(decoded)
                if (decoded.file_high_water, decoded.native_high_water) == (7, 9)
                    && decoded.rows.is_empty()
        ));
        assert!(
            serde_json::from_str::<FixtureState>(r#"{"file_high_water":7,"native_high_water":9}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<FixtureState>(
                r#"{"file_high_water":7,"file_high_water":8,"native_high_water":9,"rows":{}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<FixtureState>(
                r#"{"file_high_water":7,"native_high_water":9,"rows":{},"future":true}"#
            )
            .is_ok()
        );

        let event = SourcePublicationEvent {
            stream_id: "fixture-stream".into(),
            event_id: "fixture-event".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        };
        let row = FixtureRow {
            event: event.clone(),
            revision: "fixture-revision".into(),
            generation: 3,
            file: (4, 2),
            native: (8, 1),
        };
        let encoded = serde_json::to_value(&row);
        let expected = serde_json::json!({
            "event": event,
            "revision": "fixture-revision",
            "generation": 3,
            "file": [4, 2],
            "native": [8, 1],
        });
        assert!(matches!(&encoded, Ok(value) if value == &expected));
        let restored = encoded.and_then(serde_json::from_value::<FixtureRow>);
        assert!(matches!(
            restored,
            Ok(row)
                if row.event == event && (row.file, row.native) == ((4, 2), (8, 1))
        ));
    }
}
