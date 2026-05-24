use quanta_index_contract::{LqQuery, PublishedGenerationSet};

use crate::CoreError;

/// Driven port: generation pin used by the query domain during concurrent prepare.
pub trait GenerationPinPort {
    fn pinned_generation(&self) -> Result<Option<PublishedGenerationSet>, CoreError>;
}

/// Driving-side query validation hook (invoked before execute).
pub trait SearchPlaneQueryValidator {
    fn validate_query(&self, query: &LqQuery) -> Result<(), CoreError>;
}
