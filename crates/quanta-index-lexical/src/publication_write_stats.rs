//! Completed-build pool write costs. These counters do not describe retained
//! physical occupancy, failed writes, control files, or reserved capacity.

use quanta_index_core::CoreError;

use crate::lexical_store::AttemptWriteCost;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexicalPublicationWriteStats {
    pub completed_builds: u64,
    pub new_objects: u64,
    pub new_encoded_bytes: u64,
    pub reused_objects: u64,
    pub reused_encoded_bytes: u64,
}

impl LexicalPublicationWriteStats {
    pub(crate) fn record(&mut self, cost: AttemptWriteCost) -> Result<(), CoreError> {
        // Calculate the whole snapshot first; an overflow never exposes a
        // partially updated set of counters to a concurrent scrape.
        let add = |current: u64, increment: u64| {
            current.checked_add(increment).ok_or_else(|| {
                CoreError::Storage("lexical completed-build write cost overflow".into())
            })
        };
        let next = Self {
            completed_builds: add(self.completed_builds, 1)?,
            new_objects: add(self.new_objects, cost.new_objects)?,
            new_encoded_bytes: add(self.new_encoded_bytes, cost.new_encoded_bytes)?,
            reused_objects: add(self.reused_objects, cost.reused_objects)?,
            reused_encoded_bytes: add(self.reused_encoded_bytes, cost.reused_encoded_bytes)?,
        };
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::LexicalPublicationWriteStats;
    use crate::lexical_store::AttemptWriteCost;

    #[test]
    fn overflow_does_not_publish_partial_counters() {
        let mut stats = LexicalPublicationWriteStats {
            completed_builds: 7,
            new_encoded_bytes: u64::MAX,
            ..LexicalPublicationWriteStats::default()
        };
        let before = stats;
        assert!(
            stats
                .record(AttemptWriteCost {
                    new_objects: 1,
                    new_encoded_bytes: 1,
                    reused_objects: 0,
                    reused_encoded_bytes: 0,
                })
                .is_err()
        );
        assert_eq!(stats, before);
    }
}
