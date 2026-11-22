//! Original request admission and bounds for selected-row witnesses.

#[cfg(test)]
use {
    quanta_index_contract::PreviewUnavailableReason, quanta_index_lq_text_normalizer::MappingError,
};

#[derive(Clone, Copy)]
pub(crate) struct SnippetLimits {
    pub(crate) source_bytes: usize,
    pub(crate) transformed_bytes: usize,
    pub(crate) map_entries: usize,
    pub(crate) witnesses: usize,
}

impl Default for SnippetLimits {
    fn default() -> Self {
        Self {
            source_bytes: 65_536,
            transformed_bytes: 131_072,
            map_entries: 262_144,
            witnesses: quanta_index_contract::native_protocol::MAX_NATIVE_TEXT_PREVIEW_HIGHLIGHTS,
        }
    }
}

#[cfg(test)]
pub(crate) enum WitnessStop<E> {
    Unavailable(PreviewUnavailableReason),
    Mandatory(E),
}

#[cfg(test)]
pub(crate) type WitnessResult<T, E> = Result<T, WitnessStop<E>>;

/// One request's original work, cancellation and allocation authority.
/// Native implementations keep their native error type; they do not translate
/// a resource or cancellation refusal into a Legacy CoreError.
#[cfg(test)]
pub(crate) trait WitnessBudget {
    type Error;
    type Reservation;

    fn charge(&self, work: usize) -> WitnessResult<(), Self::Error>;
    fn reserve(&self, bytes: usize) -> WitnessResult<Self::Reservation, Self::Error>;
    fn mapping_error(&self, error: MappingError) -> WitnessStop<Self::Error>;
    fn interrupted(&self) -> bool;
    fn interrupted_error(&self) -> Self::Error;
    fn integrity(&self, message: &str) -> Self::Error;
}

pub(crate) fn token_allocation_bound_bytes(bytes: usize) -> Option<usize> {
    bytes
        .checked_mul(
            core::mem::size_of::<quanta_index_lq_text_normalizer::Token>()
                .saturating_mul(2)
                .saturating_add(16),
        )
        .and_then(|bytes| bytes.checked_add(2048))
}
