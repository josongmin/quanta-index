//! Bounded, offline source preparation. The caller owns source bytes, revision
//! attestations, prior manifests, and publication lifecycle.

mod implementation;

pub use implementation::{
    CompleteSourceSet, MarkdownAdapter, PlainTextAdapter, PreparationBudgets,
    PreparationCapabilities, PreparationError, PreparationProfile, PreparedChanges, PreparedSource,
    PriorSourceEntry, PriorSourceManifest, ReconcileIntent, SourceAdapter, SourceContext,
    TextSource, reconcile_complete_universe,
};
