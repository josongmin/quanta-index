//! Shared bounded-preview types and admission used by matching and rendering.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use quanta_index_contract::{
    HighlightSpan, LqExpr, LqOptions, PreviewKind, PreviewMetadata, PreviewUnavailableReason,
    SearchPlaneErrorCodeV2, SourceFileRevision,
};
use quanta_index_core::{
    CoreError, LexicalCollectionBudget, LexicalMemoryReservation, RequestBudgetV1,
};
use quanta_index_lq_text_normalizer::{self as normalize, MappingError};

/// Local dimensions backed by the caller's one request-scoped preview ledger.
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
            witnesses: 32,
        }
    }
}

/// Prepared semantics plus a preview ledger separate from mandatory collection.
pub(crate) struct SnippetContext<'a> {
    pub(crate) expr: &'a LqExpr,
    pub(crate) filters: &'a [quanta_index_contract::LqFilter],
    pub(crate) options: &'a LqOptions,
    pub(crate) limits: SnippetLimits,
    pub(crate) ledger: &'a LexicalCollectionBudget,
    pub(crate) request: &'a RequestBudgetV1,
}

/// All bytes must be obtained from the selected row's immutable read view.
///
/// The expected digest is publication authority, never recomputed by the reader
/// from these same bytes and passed back as an independent oracle.
pub(crate) struct SelectedSnippetSource<'a> {
    pub(crate) raw: Option<&'a str>,
    pub(crate) indexed_nfc: Option<&'a str>,
    pub(crate) path: &'a str,
    pub(crate) kind: PreviewKind,
    pub(crate) source: Option<&'a SourceFileRevision>,
    pub(crate) chunk_start_byte: Option<u64>,
    pub(crate) expected_raw_sha256: Option<[u8; 32]>,
}

/// Owner output and its retained-memory lease. The caller must keep the lease
/// while the returned strings/highlights/metadata remain in flight.
pub(crate) struct RenderedPreview {
    pub(crate) snippet: String,
    pub(crate) snippet_hit_offset: Option<u32>,
    pub(crate) highlights: Vec<HighlightSpan>,
    pub(crate) preview: PreviewMetadata,
    pub(crate) reservation: Option<LexicalMemoryReservation>,
}

pub(crate) enum PreviewStop {
    Unavailable(PreviewUnavailableReason),
    Mandatory(CoreError),
}

pub(crate) type PreviewResult<T> = Result<T, PreviewStop>;

pub(crate) fn integrity(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
        message: format!("lexical preview: {message}"),
    }
}

impl SnippetContext<'_> {
    pub(crate) fn checkpoint(&self) -> PreviewResult<()> {
        self.request
            .checkpoint("lexical:preview")
            .map_err(PreviewStop::Mandatory)
    }

    pub(crate) fn charge(&self, work: usize) -> PreviewResult<()> {
        self.checkpoint()?;
        let work = u64::try_from(work)
            .map_err(|_overflow| PreviewStop::Unavailable(PreviewUnavailableReason::WorkBudget))?;
        self.ledger.charge_work(work).map_err(ledger_error)
    }

    pub(crate) fn reserve(&self, bytes: usize) -> PreviewResult<LexicalMemoryReservation> {
        self.checkpoint()?;
        let bytes = u64::try_from(bytes)
            .map_err(|_overflow| PreviewStop::Unavailable(PreviewUnavailableReason::WorkBudget))?;
        self.ledger.reserve_bytes(bytes).map_err(ledger_error)
    }

    pub(crate) fn mapping_error(&self, error: MappingError) -> PreviewStop {
        match error {
            MappingError::ByteLimit
            | MappingError::EntryLimit
            | MappingError::AllocationRefused => {
                PreviewStop::Unavailable(PreviewUnavailableReason::WorkBudget)
            }
            MappingError::Interrupted => PreviewStop::Mandatory(
                self.request
                    .interrupted_at("lexical:preview-normalize")
                    .unwrap_or_else(|| integrity("normalizer reported unobserved interruption")),
            ),
            MappingError::SourceMismatch | MappingError::InvalidSpan => {
                PreviewStop::Mandatory(integrity("source/NFC provenance mismatch"))
            }
        }
    }
}

fn ledger_error(error: CoreError) -> PreviewStop {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        }
    ) {
        PreviewStop::Unavailable(PreviewUnavailableReason::WorkBudget)
    } else {
        PreviewStop::Mandatory(error)
    }
}

pub(crate) fn token_allocation_bound(bytes: usize) -> PreviewResult<usize> {
    bytes
        .checked_mul(
            core::mem::size_of::<normalize::Token>()
                .saturating_mul(2)
                .saturating_add(16),
        )
        .and_then(|bytes| bytes.checked_add(2048))
        .ok_or(PreviewStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ))
}
