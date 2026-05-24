use quanta_index_contract::{ChannelSeq, ManifestGeneration};

/// In-memory ledger consumed by the dispatcher (write) and the query path
/// (read, eventually). Records the highest sealed generation observed per
/// track and the last seq emitted by each subscriber, used to validate
/// monotonicity across dispatcher invocations.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical_sealed: Option<ManifestGeneration>,
    semantic_sealed: Option<ManifestGeneration>,
    lexical_last_seen: ChannelSeq,
    semantic_last_seen: ChannelSeq,
}

impl Ledger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lexical_seal(&mut self, generation: ManifestGeneration) {
        let new_value = match self.lexical_sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.lexical_sealed = Some(new_value);
    }

    pub fn semantic_seal(&mut self, generation: ManifestGeneration) {
        let new_value = match self.semantic_sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.semantic_sealed = Some(new_value);
    }

    #[must_use]
    pub fn lexical_sealed(&self) -> Option<ManifestGeneration> {
        self.lexical_sealed
    }

    #[must_use]
    pub fn semantic_sealed(&self) -> Option<ManifestGeneration> {
        self.semantic_sealed
    }

    #[must_use]
    pub fn lexical_last_seen(&self) -> ChannelSeq {
        self.lexical_last_seen
    }

    #[must_use]
    pub fn semantic_last_seen(&self) -> ChannelSeq {
        self.semantic_last_seen
    }

    pub fn set_lexical_last_seen(&mut self, seq: ChannelSeq) {
        self.lexical_last_seen = seq;
    }

    pub fn set_semantic_last_seen(&mut self, seq: ChannelSeq) {
        self.semantic_last_seen = seq;
    }
}
