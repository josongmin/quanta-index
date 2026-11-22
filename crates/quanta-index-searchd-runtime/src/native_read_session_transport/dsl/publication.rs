use std::collections::BTreeMap;

use quanta_index_contract::{
    ChunkId, RuntimeCatalogIngestBatch,
    index::{MapRoot, ProjectionBinding},
    lex::DirtyRecord,
    native_protocol::{
        NATIVE_DSL_HEADER_OWNER_V1, NATIVE_DSL_PRODUCER_V1, NativeDslCorpusV1, NativeDslKindV1,
        NativeDslRuntimeCatalogV1, NativeDslScopeV1, native_dsl_sequence_v1,
    },
};
use quanta_index_core::index::{
    AttemptContext, IndexError, MapEdit, MemoryLifetime, ObjectRef, OrdinalMapEdit, WorkBudget,
    WorkUsage, owner_map_key, ports::MapEditCursor,
};
use quanta_index_ipc::{
    CanonicalBatchDigestProgressError, canonical_batch_digest_with_progress_v1,
    matches_batch_digest_token_v1,
};
use quanta_index_kernel::{ProjectionCommitBasis, ProjectionCommitValidator};
use quanta_index_search_plane::{native_dsl, readiness::StructuralAuthorityState};
use serde::{
    Deserialize,
    de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use sha2::{Digest as _, Sha256};

use super::{
    DslStorage, decode, load_header, load_header_output, native_dsl_binding, query_error,
    require_complete_source_domain, validate_file, visit_corpus_files,
};

/// Only an admitted attempt writes this key. Producer mutations use owner_map_key.
pub(super) const CHUNK_INDEX: &[u8] = b"native-dsl-chunks\0v1";

pub(crate) struct NativeDslCommitValidator;

struct HeaderShape {
    pin: Option<IgnoredAny>,
    catalog: Option<IgnoredAny>,
}

impl<'de> Deserialize<'de> for HeaderShape {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["pin", "catalog"];

        enum Field {
            Pin,
            Catalog,
            Other,
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                struct Key;
                impl Visitor<'_> for Key {
                    type Value = Field;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("a Native DSL header field")
                    }

                    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        Ok(match value {
                            "pin" => Field::Pin,
                            "catalog" => Field::Catalog,
                            _ => Field::Other,
                        })
                    }

                    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                        Ok(match value {
                            b"pin" => Field::Pin,
                            b"catalog" => Field::Catalog,
                            _ => Field::Other,
                        })
                    }
                }
                decoder.deserialize_identifier(Key)
            }
        }

        struct Header;
        impl<'de> Visitor<'de> for Header {
            type Value = HeaderShape;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Native DSL header shape")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut pin, mut catalog) = (None, None);
                let (mut seen_pin, mut seen_catalog) = (false, false);
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::Pin => {
                            if seen_pin {
                                return Err(de::Error::duplicate_field("pin"));
                            }
                            pin = map.next_value()?;
                            seen_pin = true;
                        }
                        Field::Catalog => {
                            if seen_catalog {
                                return Err(de::Error::duplicate_field("catalog"));
                            }
                            catalog = map.next_value()?;
                            seen_catalog = true;
                        }
                        Field::Other => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(HeaderShape { pin, catalog })
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let pin = sequence
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let catalog = sequence
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(3, &self));
                }
                Ok(HeaderShape { pin, catalog })
            }
        }

        decoder.deserialize_struct("HeaderShape", FIELDS, Header)
    }
}

impl ProjectionCommitValidator for NativeDslCommitValidator {
    fn seal(
        &self,
        basis: ProjectionCommitBasis<'_>,
        next: &ProjectionBinding,
        context: &mut AttemptContext<'_>,
    ) -> Result<MapRoot, IndexError> {
        if next.producer != NATIVE_DSL_PRODUCER_V1 {
            return Ok(next.owners);
        }
        seal_dsl(basis, next, context).map_err(|error| match error {
            IndexError::CorruptObject(reason) => IndexError::InvalidInput(reason),
            IndexError::SnapshotUnavailable => invalid("Native DSL Source coverage is incomplete"),
            other => other,
        })
    }
}

fn seal_dsl(
    basis: ProjectionCommitBasis<'_>,
    next: &ProjectionBinding,
    context: &mut AttemptContext<'_>,
) -> Result<MapRoot, IndexError> {
    if !native_dsl_binding(next) {
        return Err(invalid("Native DSL publication authority differs"));
    }
    let storage = DslStorage {
        objects: basis.objects,
        maps: basis.maps,
    };
    let header = load_header_output(storage, *next, context.budget)?;
    let kind = {
        let shape = decode::<HeaderShape>(&header, context.budget)?;
        (shape.value.pin.is_some(), shape.value.catalog.is_some())
    };
    match kind {
        (true, false) => {
            let corpus = decode::<NativeDslCorpusV1>(&header, context.budget)?;
            validate_event(
                &basis,
                next,
                &NativeDslScopeV1::for_pin(&corpus.value.pin),
                NativeDslKindV1::Structural,
            )?;
            seal_corpus(storage, next, &corpus.value, context)
        }
        (false, true) => {
            let runtime = decode::<NativeDslRuntimeCatalogV1>(&header, context.budget)?;
            let scope = NativeDslScopeV1 {
                repository: runtime.value.catalog.repo_id.clone(),
                revision: runtime.value.catalog.revision_id.clone(),
            };
            validate_event(&basis, next, &scope, NativeDslKindV1::RuntimeMetadata)?;
            if basis.transition.owners.count != 1 {
                return Err(invalid(
                    "Native RuntimeMetadata publication requires one catalog owner",
                ));
            }
            validate_runtime_catalog(
                &basis,
                storage,
                next,
                &scope,
                &runtime.value,
                context.budget,
            )?;
            Ok(next.owners)
        }
        _ => Err(invalid("Native DSL publication header kind is invalid")),
    }
}

fn validate_event(
    basis: &ProjectionCommitBasis<'_>,
    next: &ProjectionBinding,
    scope: &NativeDslScopeV1,
    kind: NativeDslKindV1,
) -> Result<(), IndexError> {
    let [event] = basis.transition.events.as_slice() else {
        return Err(invalid("Native DSL publication requires exactly one event"));
    };
    let predecessor = basis
        .transition
        .expected
        .map_or(0, |prior| native_dsl_sequence_v1(prior.dependency_context));
    if next.projection != scope.projection(kind)
        || basis.operation.stream != scope.stream(kind)
        || basis.operation.stream_epoch != 1
        || event.predecessor != predecessor
        || predecessor.checked_add(1) != Some(event.sequence)
        || event.sequence != native_dsl_sequence_v1(next.dependency_context)
    {
        return Err(invalid("Native DSL event and projection scope differ"));
    }
    Ok(())
}

fn chunk_key(id: &ChunkId) -> [u8; 33] {
    let mut key = [1; 33];
    key[1..].copy_from_slice(&Sha256::digest(id.as_str().as_bytes()));
    key
}

struct ChunkMapEdits(std::collections::btree_map::IntoIter<[u8; 33], ObjectRef>);

impl MapEditCursor for ChunkMapEdits {
    fn next_edit(&mut self, work: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        self.0
            .next()
            .map(|(key, file)| OrdinalMapEdit::copy_object_key(0, &key, None, Some(file), work))
            .transpose()
    }
}

fn seal_corpus(
    storage: DslStorage<'_>,
    binding: &ProjectionBinding,
    header: &NativeDslCorpusV1,
    context: &mut AttemptContext<'_>,
) -> Result<MapRoot, IndexError> {
    let work = &mut *context.budget;
    require_complete_source_domain(storage, *binding, header, work)?;
    let mut members = BTreeMap::new();
    let mut memory = Vec::new();
    let header_ref = storage
        .maps
        .lookup_object(
            binding.owners,
            &owner_map_key(NATIVE_DSL_HEADER_OWNER_V1),
            work,
        )?
        .ok_or_else(|| invalid("Native DSL projection header is missing"))?;
    // The header sentinel also gives an empty corpus a durable seal.
    let _previous_header = members.insert([0; 33], header_ref);
    visit_corpus_files(storage, *binding, work, |reference, record, file, work| {
        validate_file(storage, binding.source, record, header, &file.value, work)?;
        let bytes = u64::try_from(file.value.chunks.len())
            .ok()
            .and_then(|count| count.checked_mul(160))
            .and_then(|bytes| bytes.checked_add(128))
            .ok_or(IndexError::ResourceLimited)?;
        memory.push(work.reserve_memory(bytes, MemoryLifetime::Retained)?);
        for chunk in &file.value.chunks {
            work.checkpoint()?;
            if members
                .insert(chunk_key(&chunk.chunk_id), reference)
                .is_some()
            {
                return Err(invalid("Native DSL chunk identity is duplicated"));
            }
        }
        native_dsl::add_native_dsl_file_v1(
            &mut StructuralAuthorityState::default(),
            file.value,
            work.request(),
        )
        .map_err(|error| query_error(work, error))
    })?;
    let old = storage
        .maps
        .lookup_object(binding.owners, CHUNK_INDEX, work)?;
    let index = context.apply_stream(&[MapRoot(None)], &mut ChunkMapEdits(members.into_iter()))?;
    let [root] = index.as_slice() else {
        return Err(IndexError::BackendViolation(
            "Native DSL chunk index lost its root".into(),
        ));
    };
    let fresh = root
        .0
        .ok_or_else(|| invalid("Native DSL chunk index is empty"))?;
    context.apply(
        binding.owners,
        &[MapEdit::objects(CHUNK_INDEX.to_vec(), old, Some(fresh))],
    )
}

fn validate_runtime_catalog(
    basis: &ProjectionCommitBasis<'_>,
    storage: DslStorage<'_>,
    next: &ProjectionBinding,
    scope: &NativeDslScopeV1,
    runtime: &NativeDslRuntimeCatalogV1,
    work: &mut WorkBudget,
) -> Result<(), IndexError> {
    let digest = catalog_digest(&runtime.catalog, work)?;
    if !runtime.catalog.batch_digest.is_empty()
        && !matches_batch_digest_token_v1(&runtime.catalog.batch_digest, &digest)
    {
        return Err(invalid("Native RuntimeMetadata catalog digest differs"));
    }
    let corpus = basis
        .binding(scope.projection(NativeDslKindV1::Structural), work)?
        .filter(|binding| {
            native_dsl_binding(binding)
                && binding.source == basis.view.source
                && binding.certificate == runtime.corpus_certificate
        })
        .ok_or_else(|| invalid("Native RuntimeMetadata requires the current chunk projection"))?;
    let header = load_header::<NativeDslCorpusV1>(storage, corpus, work)?;
    native_dsl::validate_native_runtime_catalog_pin_v1(runtime, &header.value.pin)
        .map_err(|error| query_error(work, error))?;
    native_dsl::validate_native_runtime_catalog_keys_v1(&runtime.catalog, work.request())
        .map_err(|error| query_error(work, error))?;
    native_dsl::validate_native_runtime_dirty_v1(runtime, work.request())
        .map_err(|error| query_error(work, error))?;
    if let Some(prior) = basis.transition.expected {
        let prior = load_header::<NativeDslRuntimeCatalogV1>(storage, prior, work)?;
        if prior.value.catalog.generation == runtime.catalog.generation {
            let same = prior.value.catalog.overlay_epoch_ms == runtime.catalog.overlay_epoch_ms
                && catalog_digest(&prior.value.catalog, work)? == digest
                && same_dirty_records(&prior.value.dirty, &runtime.dirty, work)?;
            native_dsl::enforce_runtime_catalog_epoch_v1(
                prior.value.catalog.overlay_epoch_ms,
                &runtime.catalog,
                same,
            )
            .map_err(|error| query_error(work, error))?;
        }
    }
    let index = chunk_index(storage, corpus, work)?;
    for id in runtime
        .dirty
        .iter()
        .map(|record| &record.doc_id)
        .chain(native_dsl::runtime_catalog_chunk_ids_v1(&runtime.catalog))
    {
        if storage
            .maps
            .lookup_object(index, &chunk_key(id), work)?
            .is_none()
        {
            return Err(invalid(
                "Native RuntimeMetadata doc_id is not present in the pinned lexical chunk universe",
            ));
        }
    }
    // A header-only catalog must never inherit file contributions.
    let owners = storage.maps.scan(next.owners, None, 2, work)?;
    if owners.has_more || owners.entries.len() != 1 {
        return Err(invalid(
            "Native RuntimeMetadata publication requires one catalog owner",
        ));
    }
    Ok(())
}

fn same_dirty_records(
    previous: &[DirtyRecord],
    incoming: &[DirtyRecord],
    work: &WorkBudget,
) -> Result<bool, IndexError> {
    if previous.len() != incoming.len() {
        return Ok(false);
    }
    for (previous, incoming) in previous.iter().zip(incoming) {
        work.request()
            .checkpoint("native-runtime-metadata:dirty-replay")
            .map_err(|error| query_error(work, error))?;
        if previous != incoming {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn chunk_index(
    storage: DslStorage<'_>,
    corpus: ProjectionBinding,
    work: &mut WorkBudget,
) -> Result<MapRoot, IndexError> {
    storage
        .maps
        .lookup_object(corpus.owners, CHUNK_INDEX, work)?
        .map(|root| MapRoot(Some(root)))
        .ok_or(IndexError::RebuildRequired)
}

fn catalog_digest(
    batch: &RuntimeCatalogIngestBatch,
    work: &mut WorkBudget,
) -> Result<[u8; 32], IndexError> {
    canonical_batch_digest_with_progress_v1(batch, &mut |bytes| {
        work.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })
    })
    .map_err(|error| match error {
        CanonicalBatchDigestProgressError::Progress(error) => error,
        CanonicalBatchDigestProgressError::Encode(error) => {
            IndexError::InvalidInput(error.to_string())
        }
    })
}

fn invalid(message: &str) -> IndexError {
    IndexError::InvalidInput(message.into())
}

#[cfg(test)]
mod header_shape_tests {
    use super::HeaderShape;

    #[test]
    fn header_shape_preserves_map_defaults_null_and_positional_fields() {
        for (wire, pin, catalog) in [
            (r#"{}"#, false, false),
            (r#"{"pin":null,"catalog":null}"#, false, false),
            (r#"{"pin":{"value":1},"future":[1,2]}"#, true, false),
            (r#"{"catalog":{"value":2}}"#, false, true),
            (r#"[null,null]"#, false, false),
            (r#"[{"value":1},null]"#, true, false),
        ] {
            let shape: HeaderShape = serde_json::from_str(wire).expect("header shape");
            assert_eq!(shape.pin.is_some(), pin, "{wire}");
            assert_eq!(shape.catalog.is_some(), catalog, "{wire}");
        }
        for wire in [
            r#"{"pin":null,"pin":null}"#,
            r#"{"catalog":null,"catalog":null}"#,
            r#"[]"#,
            r#"[null]"#,
            r#"[null,null,null]"#,
        ] {
            assert!(serde_json::from_str::<HeaderShape>(wire).is_err(), "{wire}");
        }
    }
}
