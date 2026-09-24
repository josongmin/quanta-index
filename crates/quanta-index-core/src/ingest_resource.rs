//! The resource envelope one search-corpus batch may ask the plane to hold
//! (QI-BB-021).
//!
//! The ingest frame cap bounds the bytes a producer sends, not what those
//! bytes expand into: a batch of many short records expands into
//! `records × dimension × 4` bytes of vectors before anything is written,
//! and a large dimension multiplies that further. The policy here names
//! those ceilings, and [`IngestResourcePolicy::admit_search_corpus_batch`]
//! measures a batch against them before any track mutates — a batch that
//! does not fit is refused typed with zero bytes changed, never applied
//! halfway and never held in the hope that memory suffices.

use quanta_index_contract::SearchCorpusIngestBatch;

use crate::error::CoreError;

/// Wire code for a batch that exceeds the ingest resource envelope.
pub const INGEST_RESOURCE_BUDGET_EXCEEDED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded;

/// Widest embedding dimension the plane configures or serves.
///
/// Every production text-embedding model in use today is at or below
/// 4,096 components; a configured dimension past this ceiling is a
/// configuration defect rather than a larger model, and is refused at
/// boot rather than multiplied into every batch's vector residency.
pub const MAX_EMBEDDING_DIMENSION: usize = 8_192;

/// Bytes of one `f32` vector component.
const VECTOR_COMPONENT_BYTES: u64 = 4;

/// Ceilings for one search-corpus batch: records it carries, bytes of text
/// it asks to embed, and bytes of vectors those embeddings occupy.
///
/// Every bound is a strict maximum; zero is refused at construction because
/// a zero ceiling admits nothing and is a configuration defect, not a
/// disabled bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngestResourcePolicy {
    records: usize,
    text_bytes: u64,
    vector_bytes: u64,
}

impl IngestResourcePolicy {
    /// Production defaults: 100,000 records, 64 MiB of embedded text and
    /// 256 MiB of resident vectors per batch (at 1,536 components that is
    /// ~43,000 embedded records; at 64 it is over a million).
    pub const DEFAULT: Self = Self {
        records: 100_000,
        text_bytes: 64 * 1024 * 1024,
        vector_bytes: 256 * 1024 * 1024,
    };

    /// A policy with explicit ceilings; each must be at least one.
    pub fn new(
        max_records: usize,
        max_text_bytes: u64,
        max_vector_bytes: u64,
    ) -> Result<Self, CoreError> {
        if max_records == 0 || max_text_bytes == 0 || max_vector_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "ingest resource policy: every ceiling must be at least one".to_string(),
            ));
        }
        Ok(Self {
            records: max_records,
            text_bytes: max_text_bytes,
            vector_bytes: max_vector_bytes,
        })
    }

    /// Most records (chunks and typed semantic sources together) one batch
    /// may carry.
    #[must_use]
    pub const fn max_records(&self) -> usize {
        self.records
    }

    /// Most bytes of text one batch may ask to embed.
    #[must_use]
    pub const fn max_text_bytes(&self) -> u64 {
        self.text_bytes
    }

    /// Most bytes of `f32` vectors one batch's embeddings may occupy.
    #[must_use]
    pub const fn max_vector_bytes(&self) -> u64 {
        self.vector_bytes
    }

    /// Measure `batch` under `dimension` and admit it, or refuse it typed.
    ///
    /// The footprint mirrors the single typed-source derivation. An empty
    /// semantic source list is a semantic no-op, even when lexical chunks
    /// are replaced. The records ceiling counts both lexical and semantic
    /// rows the plane holds. A `dimension`
    /// outside `1..=MAX_EMBEDDING_DIMENSION` is a composition defect and is
    /// refused as an invalid contract rather than charged to the batch.
    pub fn admit_search_corpus_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        dimension: usize,
    ) -> Result<IngestBatchFootprint, CoreError> {
        if dimension == 0 || dimension > MAX_EMBEDDING_DIMENSION {
            return Err(CoreError::InvalidContract(format!(
                "ingest resource policy: embedding dimension {dimension} is outside 1..={MAX_EMBEDDING_DIMENSION}"
            )));
        }
        let footprint = IngestBatchFootprint::measure(batch, dimension)?;
        if footprint.carried_records > self.records {
            return Err(refusal(&format!(
                "batch carries {} records, the ceiling is {}",
                footprint.carried_records, self.records
            )));
        }
        if footprint.text_bytes > self.text_bytes {
            return Err(refusal(&format!(
                "batch asks to embed {} bytes of text, the ceiling is {}",
                footprint.text_bytes, self.text_bytes
            )));
        }
        if footprint.vector_bytes > self.vector_bytes {
            return Err(refusal(&format!(
                "batch expands to {} bytes of vectors ({} records × {dimension} components), the ceiling is {}",
                footprint.vector_bytes, footprint.embedded_records, self.vector_bytes
            )));
        }
        Ok(footprint)
    }
}

fn refusal(message: &str) -> CoreError {
    CoreError::Typed {
        code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
        message: format!("ingest resource envelope: {message}"),
    }
}

/// What one batch asks the plane to hold, measured before it is applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngestBatchFootprint {
    /// Chunks and typed semantic sources together: every row the batch
    /// carries.
    pub carried_records: usize,
    /// Records whose text the derivation embeds.
    pub embedded_records: usize,
    /// Bytes of text the derivation embeds.
    pub text_bytes: u64,
    /// Bytes the embedded records' vectors occupy at the given dimension.
    pub vector_bytes: u64,
}

impl IngestBatchFootprint {
    fn measure(batch: &SearchCorpusIngestBatch, dimension: usize) -> Result<Self, CoreError> {
        let chunk_records = batch
            .replace_scopes
            .iter()
            .map(|scope| scope.chunks.len())
            .try_fold(0_usize, usize::checked_add);
        let source_records = batch
            .semantic_replace_scopes
            .iter()
            .map(|scope| scope.sources.len())
            .try_fold(0_usize, usize::checked_add);
        let (Some(chunk_records), Some(source_records)) = (chunk_records, source_records) else {
            return Err(refusal("batch record count overflows"));
        };
        let carried_records = chunk_records
            .checked_add(source_records)
            .ok_or_else(|| refusal("batch record count overflows"))?;
        let embedded_records = source_records;
        let text_bytes = sum_text_bytes(
            batch
                .semantic_replace_scopes
                .iter()
                .flat_map(|scope| scope.sources.iter().map(|record| record.text.len())),
        )?;
        let (Ok(records), Ok(components)) =
            (u64::try_from(embedded_records), u64::try_from(dimension))
        else {
            return Err(refusal("batch vector bytes overflow"));
        };
        let vector_bytes = records
            .checked_mul(components)
            .and_then(|total| total.checked_mul(VECTOR_COMPONENT_BYTES))
            .ok_or_else(|| refusal("batch vector bytes overflow"))?;
        Ok(Self {
            carried_records,
            embedded_records,
            text_bytes,
            vector_bytes,
        })
    }
}

fn sum_text_bytes(lengths: impl Iterator<Item = usize>) -> Result<u64, CoreError> {
    let mut total = 0_u64;
    for length in lengths {
        let length = u64::try_from(length).map_err(|_overflow| refusal("text bytes overflow"))?;
        total = total
            .checked_add(length)
            .ok_or_else(|| refusal("text bytes overflow"))?;
    }
    Ok(total)
}
