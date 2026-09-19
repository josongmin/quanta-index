use std::sync::Arc;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::{
    CoreError, MetricSourcePort, MetricValueV1, ResidentScopeSource, SemanticIngestHeaderV1,
    SemanticIngestPort, SemanticScopeSource, SemanticScopeStreamBuildPort, SemanticStreamTallyV1,
    SemanticStreamWindowPolicy,
};

use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::ingest_dispatcher::tests::support::{
    FakeSemanticBuilder, TestRes, fixture_semantic_batch,
};

/// The materializer builds through the durable adapter and receipts the
/// seal; the readiness ledger is the search-corpus finalize's to write (the
/// retry tests in `search_corpus` hold it to both tracks).
#[test]
fn direct_semantic_materializer_builds_durably_and_receipts_the_seal() -> TestRes {
    let builder = Arc::new(FakeSemanticBuilder::default());
    let materializer = DirectSemanticMaterializer::new(builder.clone());
    let batch = fixture_semantic_batch()?;
    let header = SemanticIngestHeaderV1::of_batch(&batch);
    let mut source =
        ResidentScopeSource::new(&batch.replace_scopes, SemanticStreamWindowPolicy::DEFAULT)?;
    let receipt = materializer.publish_stream(&header, &mut source)?;
    if !receipt.sealed || receipt.manifest_digest.as_deref() != Some(batch.manifest_digest.as_str())
    {
        return Err("unexpected semantic materialize receipt".into());
    }

    // The durable builder received the batch (durability lives in the adapter).
    let built = builder.take()?;
    if built.as_slice() != [batch] {
        return Err(format!("durable builder did not receive batch: {built:?}").into());
    }
    Ok(())
}

/// A build port that drains every window but reports one window fewer
/// than it consumed.
struct UndercountingBuilder;

impl SemanticScopeStreamBuildPort for UndercountingBuilder {
    fn build_stream(
        &self,
        _header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
    ) -> Result<SemanticStreamTallyV1, CoreError> {
        let mut tally = SemanticStreamTallyV1::default();
        while let Some(window) = scopes.next_window()? {
            tally.count_window(window.scopes().len(), window.rows()?, window.vector_bytes())?;
            drop(window);
        }
        tally.windows = tally.windows.saturating_sub(1);
        Ok(tally)
    }
}

// CASE-COVERS (QI-BB-021 follow-up #2): the build's tally and the source's
// are counted on opposite sides of the stream; a build that does not add up
// to what the source issued is refused typed and counted nowhere.
#[test]
fn a_build_whose_tally_disagrees_with_the_source_is_refused_and_counts_nothing() -> TestRes {
    let materializer = DirectSemanticMaterializer::new(Arc::new(UndercountingBuilder));
    let batch = fixture_semantic_batch()?;
    let header = SemanticIngestHeaderV1::of_batch(&batch);
    let mut source =
        ResidentScopeSource::new(&batch.replace_scopes, SemanticStreamWindowPolicy::DEFAULT)?;
    match materializer.publish_stream(&header, &mut source) {
        Err(CoreError::InvalidContract(message)) if message.contains("source issued") => {}
        other => return Err(format!("a tally mismatch is refused typed, got {other:?}").into()),
    }
    if materializer.stream_stats()?.windows_total != 0 {
        return Err("a refused build is not counted".into());
    }
    Ok(())
}

/// Whether the gauge is exactly `expected`: both sides are integers below
/// 2^53, each with one `f64` representation.
fn gauge_is(gauge: f64, expected: u64) -> bool {
    gauge.to_bits() == quanta_index_core::count_as_f64(expected).to_bits()
}

fn scrape_of(
    materializer: &DirectSemanticMaterializer,
) -> Result<(u64, f64), Box<dyn std::error::Error>> {
    let points = materializer.scrape()?;
    let mut windows = None;
    let mut peak = None;
    for point in points {
        match (point.name.as_str(), point.value) {
            ("semantic_ingest_windows_total", MetricValueV1::Counter(value)) => {
                windows = Some(value);
            }
            ("semantic_ingest_resident_vector_bytes_peak", MetricValueV1::Gauge(value)) => {
                peak = Some(value);
            }
            (name, value) => return Err(format!("unexpected point {name}={value:?}").into()),
        }
    }
    Ok((
        windows.ok_or("windows counter is in the scrape")?,
        peak.ok_or("peak gauge is in the scrape")?,
    ))
}

// CASE-COVERS (QI-BB-021 follow-up #2): the windows counter accumulates
// across builds, the peak gauge is the widest window of the generation
// most recently built into, and a batch for another generation starts the
// peak over. Every expected value is computed from the fixture's own row
// count and dimension, not read back from the accounting under test.
#[test]
fn stream_metrics_count_windows_and_reset_the_peak_per_generation() -> TestRes {
    let builder = Arc::new(FakeSemanticBuilder::default());
    let materializer = DirectSemanticMaterializer::new(builder);
    let one_row = SemanticStreamWindowPolicy::vector_bytes(1, 3)?;
    let one_row_window = SemanticStreamWindowPolicy::new(1, one_row)?;

    let mut two_rows = fixture_semantic_batch()?;
    two_rows.seal = false;
    let mut second = two_rows
        .replace_scopes
        .first()
        .and_then(|scope| scope.embeddings.first())
        .cloned()
        .ok_or("fixture carries one embedding")?;
    second.embedding_id = quanta_index_contract::EmbeddingId::new("emb-2");
    second.record_id = "emb-2".to_string().into_boxed_str();
    second.owner_id = "main-2".to_string().into_boxed_str();
    two_rows
        .replace_scopes
        .first_mut()
        .ok_or("fixture carries one scope")?
        .embeddings
        .push(second);

    // Generation 1, one-row windows: two windows, peak one row.
    let header = SemanticIngestHeaderV1::of_batch(&two_rows);
    let mut source = ResidentScopeSource::new(&two_rows.replace_scopes, one_row_window)?;
    let _receipt = materializer.publish_stream(&header, &mut source)?;
    let (windows, peak) = scrape_of(&materializer)?;
    if windows != 2 || !gauge_is(peak, one_row) {
        return Err(format!("after two one-row windows: windows={windows} peak={peak}").into());
    }
    // Generation 1 again, both rows in one window: the peak widens.
    let two_row_window = SemanticStreamWindowPolicy::new(2, one_row.saturating_mul(2))?;
    let mut source = ResidentScopeSource::new(&two_rows.replace_scopes, two_row_window)?;
    let _receipt = materializer.publish_stream(&header, &mut source)?;
    let (windows, peak) = scrape_of(&materializer)?;
    if windows != 3 || !gauge_is(peak, one_row.saturating_mul(2)) {
        return Err(format!("after a two-row window: windows={windows} peak={peak}").into());
    }
    // Generation 2 with one-row windows: the peak starts over.
    two_rows.generation = ManifestGeneration::new(2);
    let header = SemanticIngestHeaderV1::of_batch(&two_rows);
    let mut source = ResidentScopeSource::new(&two_rows.replace_scopes, one_row_window)?;
    let _receipt = materializer.publish_stream(&header, &mut source)?;
    let (windows, peak) = scrape_of(&materializer)?;
    if windows != 5 || !gauge_is(peak, one_row) {
        return Err(format!("after a new generation: windows={windows} peak={peak}").into());
    }
    Ok(())
}
