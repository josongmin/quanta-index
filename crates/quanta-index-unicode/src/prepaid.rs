//! Borrowed comparison under a caller's existing coarse scratch reservation.

use super::{
    NfcError, Scratch, ScratchAdmission, ScratchBuffers, ScratchDemand, ScratchOwner,
    retained::compare_nfc_with_completion,
};
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum PrepaidNfcError<E> {
    ScratchLimitExceeded,
    Normalization(NfcError<E>),
}

impl<E: fmt::Display> fmt::Display for PrepaidNfcError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScratchLimitExceeded => {
                formatter.write_str("prepaid normalization scratch limit exceeded")
            }
            Self::Normalization(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for PrepaidNfcError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ScratchLimitExceeded => None,
            Self::Normalization(error) => Some(error),
        }
    }
}

struct PrepaidScratch<F> {
    checkpoint: F,
    limit: usize,
    live: [usize; 3],
    peak_refused: bool,
}

impl<F, E> ScratchAdmission for PrepaidScratch<F>
where
    F: FnMut() -> Result<(), E>,
{
    type Error = E;

    fn checkpoint(&mut self, _: u64) -> Result<(), E> {
        (self.checkpoint)()
    }

    fn birth(&mut self, demand: ScratchDemand, birth: &mut dyn FnMut() -> bool) -> Result<bool, E> {
        let slot = match demand.owner {
            ScratchOwner::Decomposition => 0,
            ScratchOwner::Recomposition => 1,
            ScratchOwner::Sort => 2,
        };
        if self.live.get(slot).copied() != Some(demand.current_bytes) {
            return Ok(false);
        }
        let peak = self
            .live
            .iter()
            .try_fold(demand.new_bytes, |sum, bytes| sum.checked_add(*bytes));
        if peak.is_none_or(|peak| peak > self.limit) {
            self.peak_refused = true;
            return Ok(false);
        }
        let born = birth();
        if born {
            let Some(live) = self.live.get_mut(slot) else {
                return Ok(false);
            };
            *live = demand.new_bytes;
        }
        Ok(born)
    }

    fn release(&mut self, _: ScratchOwner) {}
}

/// The caller prepays NFC work and keeps its reservation of
/// `scratch_bytes` live throughout this call. Polls consume no new work.
/// The comparison publishes no verdict on interruption or scratch refusal.
pub fn try_is_nfc_with_prepaid_scratch_v1<E>(
    source: &str,
    scratch_bytes: usize,
    checkpoint: impl FnMut() -> Result<(), E>,
) -> Result<bool, PrepaidNfcError<E>> {
    let mut admission = PrepaidScratch {
        checkpoint,
        limit: scratch_bytes,
        live: [0; 3],
        peak_refused: false,
    };
    let mut buffers = ScratchBuffers::default();
    let result = {
        let mut scratch = Scratch::new(&mut admission, &mut buffers);
        scratch.allow_portable_short_sort = true;
        compare_nfc_with_completion(source, &mut scratch)
    };
    drop(buffers);
    if admission.peak_refused {
        return Err(PrepaidNfcError::ScratchLimitExceeded);
    }
    result.map_err(PrepaidNfcError::Normalization)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{grow_vec, push_tiny};
    use std::mem::size_of;
    use tinyvec::TinyVec;

    #[test]
    fn coarse_limit_counts_the_other_owner_and_both_growth_backings() {
        let peak = (8 + 16) * size_of::<char>() + 4 * size_of::<(u8, char)>();
        for limit in [peak - 1, peak] {
            let mut admission = PrepaidScratch {
                checkpoint: || Ok::<(), ()>(()),
                limit,
                live: [0; 3],
                peak_refused: false,
            };
            let mut sort = Vec::<(u8, char)>::new();
            let mut sort_attempted = false;
            grow_vec(
                &mut admission,
                ScratchOwner::Sort,
                &mut sort,
                4,
                &mut sort_attempted,
            )
            .unwrap();
            let mut recomposition = TinyVec::<[char; 4]>::new();
            for _ in 0..8 {
                push_tiny(
                    &mut admission,
                    ScratchOwner::Recomposition,
                    &mut recomposition,
                    'a',
                )
                .unwrap();
            }
            let result = push_tiny(
                &mut admission,
                ScratchOwner::Recomposition,
                &mut recomposition,
                'b',
            );
            if limit < peak {
                assert!(result.is_err());
                assert!(admission.peak_refused);
                assert_eq!((recomposition.len(), recomposition.capacity()), (8, 8));
                assert!(recomposition.iter().all(|&ch| ch == 'a'));
            } else {
                assert_eq!(result, Ok(()));
                assert!(!admission.peak_refused);
                assert_eq!((recomposition.len(), recomposition.capacity()), (9, 16));
                assert_eq!(recomposition.last(), Some(&'b'));
            }
        }
    }
}
