//! `DirectSemanticMaterializer`: semantic-only ingest into a staged generation.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{BatchPublishReceipt, GenerationPin};
use quanta_index_core::{
    CoreError, MetricPointV1, MetricSourcePort, SemanticIngestHeaderV1, SemanticIngestPort,
    SemanticScopeSource, SemanticScopeStreamBuildPort, SemanticStreamTallyV1,
};

/// Direct semantic batch materializer.
///
/// Drives the durable, generation-scoped semantic adapter through its
/// streamed build port (rows window by window on every batch; graph +
/// manifest + seal on `seal`). Durability lives entirely in the adapter's
/// generation directories; there is no journal write here, and a failed
/// durable write leaves no SEALED marker. The readiness ledger is not this
/// materializer's to write: the search-corpus finalize records both tracks
/// of the pair once the pair is durable, whether this batch built the
/// semantic track or found it sealed on disk (QI-BB-029).
///
/// The tally the build returns is compared with the source's own: the two
/// are counted on opposite sides of the stream, so a window the build did
/// not append, or appended twice, is refused typed and never acknowledged.
pub struct DirectSemanticMaterializer {
    builder: Arc<dyn SemanticScopeStreamBuildPort + Send + Sync>,
    stream_stats: Mutex<SemanticIngestStreamStats>,
}

/// What the streamed builds added up to (QI-BB-021).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SemanticIngestStreamStats {
    /// Windows every build appended, over the process lifetime.
    pub windows_total: u64,
    /// The generation most recently built into and the most bytes of
    /// vectors one of its windows held resident; a batch for another
    /// generation starts the peak over.
    pub resident_vector_bytes_peak: Option<(GenerationPin, u64)>,
}

impl SemanticIngestStreamStats {
    fn record(&mut self, pin: &GenerationPin, tally: SemanticStreamTallyV1) {
        self.windows_total = self.windows_total.saturating_add(tally.windows);
        let same_generation = self
            .resident_vector_bytes_peak
            .as_ref()
            .is_some_and(|(current, _peak)| current == pin);
        self.resident_vector_bytes_peak = match self.resident_vector_bytes_peak.take() {
            Some((current, peak)) if same_generation => {
                Some((current, peak.max(tally.peak_vector_bytes)))
            }
            _other_generation_or_first => Some((pin.clone(), tally.peak_vector_bytes)),
        };
    }

    /// The peak the gauge reports: zero before the first build.
    #[must_use]
    pub fn resident_vector_bytes_peak_gauge(&self) -> u64 {
        self.resident_vector_bytes_peak
            .as_ref()
            .map_or(0, |(_pin, peak)| *peak)
    }
}

impl DirectSemanticMaterializer {
    #[must_use]
    pub fn new(builder: Arc<dyn SemanticScopeStreamBuildPort + Send + Sync>) -> Self {
        Self {
            builder,
            stream_stats: Mutex::new(SemanticIngestStreamStats::default()),
        }
    }

    /// What the streamed builds have added up to so far.
    pub fn stream_stats(&self) -> Result<SemanticIngestStreamStats, CoreError> {
        self.stream_stats
            .lock()
            .map(|stats| stats.clone())
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct semantic materialize: stream stats poisoned: {err}"
                ))
            })
    }

    fn record_stream(
        &self,
        pin: &GenerationPin,
        tally: SemanticStreamTallyV1,
    ) -> Result<(), CoreError> {
        self.stream_stats
            .lock()
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct semantic materialize: stream stats poisoned: {err}"
                ))
            })?
            .record(pin, tally);
        Ok(())
    }
}

/// The streamed builds' tallies as scrape points, `semantic_ingest_…`
/// (QI-BB-015): the windows appended and the observed peak of resident
/// vector bytes for the generation most recently built into.
impl MetricSourcePort for DirectSemanticMaterializer {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let stats = self.stream_stats()?;
        Ok(vec![
            MetricPointV1::counter("semantic_ingest_windows_total", stats.windows_total),
            MetricPointV1::gauge_count(
                "semantic_ingest_resident_vector_bytes_peak",
                stats.resident_vector_bytes_peak_gauge(),
            ),
        ])
    }
}

impl SemanticIngestPort for DirectSemanticMaterializer {
    fn publish_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
    ) -> Result<
        (
            BatchPublishReceipt,
            quanta_index_contract::IngestStageReport,
        ),
        CoreError,
    > {
        let (appended, mut report) = self.builder.build_stream(header, scopes)?;
        report.durations.embedding = scopes.embedding_elapsed_ns();
        let issued = scopes.tally();
        if appended != issued {
            return Err(CoreError::InvalidContract(format!(
                "direct semantic materialize: the build appended {appended:?} but the source issued {issued:?}"
            )));
        }
        let windows_match = report.windows == appended.windows;
        let scopes_match = report.owner_scopes == appended.replace_scopes;
        if !windows_match || !scopes_match {
            return Err(CoreError::InvalidContract(format!(
                "direct semantic materialize: stage report coverage {}/{} differs from appended windows/scopes {}/{}",
                report.windows, report.owner_scopes, appended.windows, appended.replace_scopes,
            )));
        }
        self.record_stream(&header.pin, appended)?;
        let mut receipt = BatchPublishReceipt::empty_for(
            header.pin.manifest_generation,
            Some(header.batch.manifest_digest.clone()),
            header.batch.batch_digest.clone(),
        );
        for _scope in 0..appended.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &header.mutations.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        for _surface in &header.mutations.clear_surfaces {
            receipt.accept_clear_surface();
        }
        if header.batch.seal {
            receipt.mark_sealed();
        }
        Ok((receipt, report))
    }
}
