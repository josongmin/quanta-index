use quanta_index_contract::ChannelSeq;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchMode {
    ReplaceGeneration,
    Delta,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchReceipt {
    pub first_seq: Option<ChannelSeq>,
    pub last_seq: Option<ChannelSeq>,
    pub sealed: bool,
}

impl BatchReceipt {
    pub(crate) fn record(&mut self, seq: ChannelSeq) {
        if self.first_seq.is_none() {
            self.first_seq = Some(seq);
        }
        self.last_seq = Some(seq);
    }

    pub(crate) fn mark_sealed(&mut self) {
        self.sealed = true;
    }
}
