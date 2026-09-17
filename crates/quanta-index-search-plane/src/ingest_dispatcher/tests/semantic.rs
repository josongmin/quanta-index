use std::sync::{Arc, RwLock};

use quanta_index_contract::SearchPlaneTrackKind;
use quanta_index_core::SemanticIngestPort;

use crate::Ledger;
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::ingest_dispatcher::tests::support::{
    FakeSemanticBuilder, TestRes, fixture_semantic_batch,
};

#[test]
fn direct_semantic_materializer_builds_durably_and_marks_ledger() -> TestRes {
    let builder = Arc::new(FakeSemanticBuilder::default());
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let materializer = DirectSemanticMaterializer::new(builder.clone(), Arc::clone(&ledger));
    let batch = fixture_semantic_batch()?;
    let receipt = materializer.publish_batch(&batch)?;
    if !receipt.sealed || receipt.manifest_digest.as_deref() != Some(batch.manifest_digest.as_str())
    {
        return Err("unexpected semantic materialize receipt".into());
    }

    // The durable builder received the batch (durability lives in the adapter).
    let built = builder.take()?;
    if built.as_slice() != [batch.clone()] {
        return Err(format!("durable builder did not receive batch: {built:?}").into());
    }

    // Readiness reflects the durable seal, not a journal write.
    let guard = ledger
        .read()
        .map_err(|err| format!("ledger poisoned: {err}"))?;
    if guard.track_sealed(
        &batch.repo_id,
        &batch.revision_id,
        SearchPlaneTrackKind::Semantic,
    ) != Some(batch.generation)
    {
        return Err("publish did not record sealed generation".into());
    }
    if guard.track_manifest_digest(
        &batch.repo_id,
        &batch.revision_id,
        SearchPlaneTrackKind::Semantic,
    ) != Some(batch.manifest_digest.as_str())
    {
        return Err("publish did not preserve manifest digest".into());
    }
    drop(guard);
    Ok(())
}
