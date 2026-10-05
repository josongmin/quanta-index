#![no_main]
#![forbid(unsafe_code)]

//! Focused fail-closed fuzzing for the SCV2 search-corpus ingest boundary.
//!
//! The broad request decoder target reaches this DTO, but its coverage signal
//! is shared by every request shape. This target keeps the semantic scope,
//! clear-surface, and source-record validation path independently reachable.

use libfuzzer_sys::fuzz_target;

use quanta_index_contract::{SearchCorpusIngestBatch, validate_semantic_source_record_v1};

fuzz_target!(|data: &[u8]| {
    let Ok(batch) = ciborium::de::from_reader::<SearchCorpusIngestBatch, _>(data) else {
        return;
    };

    let _ = batch.validate_v1();
    let _ = batch.validate_surface_mutations_v1();
    for scope in &batch.semantic_replace_scopes {
        for source in &scope.sources {
            let _ = validate_semantic_source_record_v1(source);
        }
    }
});
