//! Native owner-local literal and token-only text indexes.

mod edit_spool;
mod literal;
mod literal_contribution;
mod literal_grams;
mod literal_reader;
mod metadata;
mod text;
mod text_filter;
mod text_memory;
mod text_occurrence;
mod text_overlay;
mod text_phrase;
mod text_preflight;
mod text_query;
mod text_regex;
mod text_universe;
pub use text_preflight::validate_native_text_input;
mod text_reader;
mod text_spool;

pub use literal::{
    LiteralCandidate, LiteralHit, LiteralPage, LiteralPageParts, LiteralPlan, LiteralQuery,
    LiteralReadContext, NativeLiteralIndex,
};
pub use literal_reader::{LiteralReadRequest, NativeLiteralReader};
pub use text::{
    CaseMode, MAX_TEXT_INSPECT_TERM_BYTES, NativeTextIndex, TextCandidate, TextPlan,
    TextReadContext, TextRecipe, TextStatistics, TextTermHit, TextTermScan,
};
pub use text_overlay::{TextOverlayCorpus, TextOverlayInput};
pub use text_query::{
    Bm25Policy, NativeTextHitSource, NativeTextRankedHit, NativeTextRanking, TextContentExecution,
    TextOverlaySource,
};
pub use text_reader::{
    NativeTextReader, PlannedTextRead, TextOverlayReadDocument, TextReadHit, TextReadResult,
};

pub mod stub;
