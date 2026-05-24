use quanta_index_contract::{ChannelSeq, ManifestGeneration};

/// Per-track ledger state. Records the highest sealed generation observed on
/// the track and the last seq emitted by its subscriber, used to validate
/// monotonicity across dispatcher invocations.
#[derive(Debug, Default)]
pub struct TrackLedger {
    sealed: Option<ManifestGeneration>,
    last_seen: ChannelSeq,
}

impl TrackLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn sealed(&self) -> Option<ManifestGeneration> {
        self.sealed
    }

    #[must_use]
    pub fn last_seen(&self) -> ChannelSeq {
        self.last_seen
    }

    /// Monotonically record a sealed generation. A lower or equal generation
    /// is ignored so that out-of-order observations cannot rewind the seal.
    pub fn record_seal(&mut self, generation: ManifestGeneration) {
        let new_value = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(new_value);
    }

    pub fn set_last_seen(&mut self, seq: ChannelSeq) {
        self.last_seen = seq;
    }
}

/// In-memory ledger consumed by the dispatcher (write) and the query path
/// (read). Internally a strongly-typed pair of [`TrackLedger`] values, one
/// per indexing track.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical: TrackLedger,
    semantic: TrackLedger,
}

impl Ledger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn lexical(&self) -> &TrackLedger {
        &self.lexical
    }

    #[must_use]
    pub fn semantic(&self) -> &TrackLedger {
        &self.semantic
    }

    pub fn lexical_mut(&mut self) -> &mut TrackLedger {
        &mut self.lexical
    }

    pub fn semantic_mut(&mut self) -> &mut TrackLedger {
        &mut self.semantic
    }

    // --- Thin convenience wrappers preserving the historical API. ---

    pub fn lexical_seal(&mut self, generation: ManifestGeneration) {
        self.lexical.record_seal(generation);
    }

    pub fn semantic_seal(&mut self, generation: ManifestGeneration) {
        self.semantic.record_seal(generation);
    }

    #[must_use]
    pub fn lexical_sealed(&self) -> Option<ManifestGeneration> {
        self.lexical.sealed()
    }

    #[must_use]
    pub fn semantic_sealed(&self) -> Option<ManifestGeneration> {
        self.semantic.sealed()
    }

    #[must_use]
    pub fn lexical_last_seen(&self) -> ChannelSeq {
        self.lexical.last_seen()
    }

    #[must_use]
    pub fn semantic_last_seen(&self) -> ChannelSeq {
        self.semantic.last_seen()
    }

    pub fn set_lexical_last_seen(&mut self, seq: ChannelSeq) {
        self.lexical.set_last_seen(seq);
    }

    pub fn set_semantic_last_seen(&mut self, seq: ChannelSeq) {
        self.semantic.set_last_seen(seq);
    }
}
