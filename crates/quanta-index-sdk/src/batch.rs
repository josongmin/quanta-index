use quanta_index_contract::BatchIngestMode;

/// SDK-facing batch mode. Wire counterpart is
/// [`quanta_index_contract::BatchIngestMode`] (QI-ING-01).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchMode {
    ReplaceGeneration,
    Delta,
}

impl BatchMode {
    pub(crate) const fn to_wire(self) -> BatchIngestMode {
        match self {
            Self::ReplaceGeneration => BatchIngestMode::ReplaceGeneration,
            Self::Delta => BatchIngestMode::Delta,
        }
    }
}

/// QI-SDK-01: receipt for a batch publish.
///
/// Alias for wire [`quanta_index_contract::BatchPublishReceipt`]. The SDK no
/// longer synthesises a sequence range client-side; `searchd` is the
/// authority and returns the inclusive range in the ingest response.
pub type BatchReceipt = quanta_index_contract::BatchPublishReceipt;
