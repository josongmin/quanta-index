//! Native owner-local literal and token-only text indexes.

#[cfg(test)]
mod edit_spool;
mod literal;
mod literal_blocks;
mod literal_contribution;
mod literal_delta_runs;
mod literal_grams;
mod literal_memberships;
mod literal_packed;
mod literal_reader;
mod literal_work;
mod metadata;
mod text;
mod text_filter;
mod text_inventory_descriptor;
mod text_memory;
mod text_occurrence;
mod text_overlay;
mod text_path;
mod text_path_group;
mod text_phrase;
mod text_preflight;
mod text_query;
mod text_regex;
mod text_repo_predicate;
mod text_universe;
pub use text_preflight::validate_native_text_input;
mod text_reader;
mod text_spool;
mod text_statistics;
mod text_token_batch;

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
