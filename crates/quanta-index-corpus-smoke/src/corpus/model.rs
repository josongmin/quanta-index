use std::path::PathBuf;

use crate::errors::ConformanceError;

/// A loaded corpus with provenance back to its source path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Corpus {
    /// Rows in load order. IDs are unique across the corpus.
    pub rows: Vec<CorpusRow>,
    /// Filesystem path the corpus was read from.
    pub source_path: PathBuf,
}

/// One row of the corpus.
///
/// Field set is the v0 schema — a subset of the eventual full row
/// shape documented in `usecase.md` § 6, enough to exercise the
/// runner end-to-end without locking down the full 100-row schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorpusRow {
    /// Stable row identifier, e.g. `UC-LEX-01` or `AC-05`.
    pub id: String,
    /// Golden query string.
    pub query: String,
    /// Gating discipline — controls runner short-circuit behavior.
    pub gate: Gate,
    /// Expected response shape or error code.
    pub expected: ExpectedShape,
    /// Optional persona tag (`P1`..`P6`).
    pub persona: Option<String>,
    /// Engine tags this row exercises (`lexical_content`, ...).
    pub engines: Vec<String>,
    /// Optional filter tags carried for downstream attribution.
    pub filters: Vec<String>,
    /// Optional runtime syntax for rows promoted into the live
    /// searchd-runtime corpus rail.
    pub syntax: Option<RuntimeSyntax>,
    /// Optional runtime classification. Parser-only and external-producer
    /// rows stay in the same machine-readable corpus but must not be counted
    /// as runtime pass/fail rows.
    pub classification: Option<RowClassification>,
    /// Optional public query route for rows promoted into the live daemon
    /// rail. Keeps text/history/structural execution explicit instead of
    /// inferring the route from free-form engine tags.
    pub runtime_route: Option<RuntimeRoute>,
    /// Optional named fixture bundle consumed by the runtime rail.
    pub fixture: Option<String>,
    /// Optional exact corpus-id set asserted by the runtime rail.
    pub expected_ids: Vec<String>,
    /// Optional runtime `top_k` override for rows executed against the live
    /// daemon.
    pub top_k: Option<u32>,
    /// Optional typed runtime error code for rows executed against the live
    /// daemon. Kept separate from `ExpectedShape::Error`, which remains the
    /// parser/conformance placeholder enum surface.
    pub runtime_error_code: Option<String>,
}

/// Runtime syntax tag for rows promoted into the live daemon rail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeSyntax {
    Native,
    Sourcegraph,
}

/// Public query route for runtime rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRoute {
    Text,
    Structural,
    History,
}

/// Runtime-corpus classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowClassification {
    Runtime,
    ParserOnly,
    TypedUnavailable,
    DeferredExternalProducer,
}

/// Gating discipline for a row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Gate {
    /// Row is exercised in full.
    Active,
    /// Row is parked behind a later-wave ticket.
    Pending {
        /// The ticket whose landing flips this row to `Active`.
        gating_ticket: String,
    },
    /// Row is blocked on a cross-repo dependency.
    Blocked {
        /// The ticket tracking the blocker.
        gating_ticket: String,
    },
}

/// Expected shape of the executor response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedShape {
    /// Executor must return zero candidates.
    Empty,
    /// Executor must return exactly one candidate.
    Single,
    /// Executor must return between `min` and `max` candidates,
    /// inclusive. `max == None` means unbounded above.
    Multi {
        /// Lower bound on candidate count.
        min: u32,
        /// Upper bound, inclusive.
        max: Option<u32>,
    },
    /// Executor must return a paginated response with the given
    /// page size.
    Paginated {
        /// Page size requested by the caller / asserted by the row.
        page_size: u32,
    },
    /// Normalize OR execute must return this typed error code.
    Error {
        /// Closed-set error code expected from the pipeline.
        code: ConformanceError,
    },
}
