//! Durable rows for the auxiliary authorities — history, runtime metadata
//! and structural — one row per record (QI-BB-020, W2).
//!
//! Before this, the three authorities lived in one in-memory ledger and
//! were persisted as three whole-map snapshot files: a one-row mutation
//! cloned, encoded and rewrote every generation of every repo, a query
//! held the global lock while it scanned, and a generation was never
//! forgotten. The catalog behind this port stores each record as its own
//! row under `(domain, repo, revision, generation, family, key)`, applies a
//! batch of row mutations as one transaction, and forgets a generation as
//! one delete — so what is written is proportional to what changed, what
//! is durable is exactly what was acknowledged, and what is retained is
//! bounded by retention.
//!
//! Rows are opaque to the catalog: the search plane owns the encoding of
//! each family's value and the meaning of each key. The catalog owns the
//! engine, the row digest it verifies on every read (G0-C), and the
//! transaction.

use std::fmt;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};

use crate::error::CoreError;

/// Which authority a row belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AuxiliaryDomainV1 {
    History,
    Runtime,
    Structural,
}

impl AuxiliaryDomainV1 {
    /// Every domain, in catalog order.
    pub const ALL: [Self; 3] = [Self::History, Self::Runtime, Self::Structural];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Runtime => "runtime",
            Self::Structural => "structural",
        }
    }

    /// The domain a stored code names, if any.
    #[must_use]
    pub fn from_code_str(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|domain| domain.as_code_str() == code)
    }
}

impl fmt::Display for AuxiliaryDomainV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// Which kind of record a row holds within its domain.
///
/// `StateMeta` is the one row per generation that carries what is not a
/// record: which families a history generation has materialized, a
/// runtime generation's catalog epoch and digests, whether a structural
/// generation requested its seal.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AuxiliaryRowFamilyV1 {
    Commit,
    Ref,
    Tag,
    DiffHunk,
    DirtyDoc,
    ChangedDoc,
    DocFacet,
    Snapshot,
    AffectedDocs,
    InvalidatedByDocs,
    Chunk,
    ParseTree,
    StateMeta,
}

impl AuxiliaryRowFamilyV1 {
    /// Every family, in catalog order.
    pub const ALL: [Self; 13] = [
        Self::Commit,
        Self::Ref,
        Self::Tag,
        Self::DiffHunk,
        Self::DirtyDoc,
        Self::ChangedDoc,
        Self::DocFacet,
        Self::Snapshot,
        Self::AffectedDocs,
        Self::InvalidatedByDocs,
        Self::Chunk,
        Self::ParseTree,
        Self::StateMeta,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Ref => "ref",
            Self::Tag => "tag",
            Self::DiffHunk => "diff-hunk",
            Self::DirtyDoc => "dirty-doc",
            Self::ChangedDoc => "changed-doc",
            Self::DocFacet => "doc-facet",
            Self::Snapshot => "snapshot",
            Self::AffectedDocs => "affected-docs",
            Self::InvalidatedByDocs => "invalidated-by-docs",
            Self::Chunk => "chunk",
            Self::ParseTree => "parse-tree",
            Self::StateMeta => "state-meta",
        }
    }

    /// The family a stored code names, if any.
    #[must_use]
    pub fn from_code_str(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.as_code_str() == code)
    }
}

impl fmt::Display for AuxiliaryRowFamilyV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// The generation one row belongs to.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuxiliaryGenerationKeyV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}

/// The full address of one row.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuxiliaryRowKeyV1 {
    pub domain: AuxiliaryDomainV1,
    pub generation: AuxiliaryGenerationKeyV1,
    pub family: AuxiliaryRowFamilyV1,
    /// The record's own key bytes, as the owning domain encodes them.
    pub row_key: Vec<u8>,
}

/// One row: its address and its encoded value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuxiliaryRowV1 {
    pub key: AuxiliaryRowKeyV1,
    pub value: Vec<u8>,
}

/// The per-track authority state row (which generation a track has
/// materialized and sealed, under which digest). Keyed by pair and track;
/// there is one per structural track.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuxiliaryTrackRowV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub track: SearchPlaneTrackKind,
    pub value: Vec<u8>,
}

/// One change to the row store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuxiliaryRowMutationV1 {
    /// Write the row, replacing any row under its key.
    Upsert(AuxiliaryRowV1),
    /// Remove the row under the key, if any.
    Delete(AuxiliaryRowKeyV1),
    /// Remove every row of one family under one generation (a whole-family
    /// replace: the batch that follows carries the family's new contents).
    ClearFamily {
        domain: AuxiliaryDomainV1,
        generation: AuxiliaryGenerationKeyV1,
        family: AuxiliaryRowFamilyV1,
    },
}

/// The row changes one accepted batch amounts to, applied as one
/// transaction: every row mutation in order, then every track row.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuxiliaryMutationBatchV1 {
    pub rows: Vec<AuxiliaryRowMutationV1>,
    pub tracks: Vec<AuxiliaryTrackRowV1>,
}

impl AuxiliaryMutationBatchV1 {
    /// Whether the batch changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.tracks.is_empty()
    }
}

/// What one transaction did, as the engine counted it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AuxiliaryMutationReceiptV1 {
    /// Rows written (inserted or replaced), track rows included.
    pub rows_written: u64,
    /// Rows removed by deletes and family clears.
    pub rows_deleted: u64,
}

/// Durable auxiliary authority rows (QI-BB-020).
///
/// The adapter owns the storage engine. A batch is applied as one
/// transaction under full synchronization: after `apply` returns, every
/// mutation in the batch is durable, and if it fails none is. Every row
/// carries its own digest, verified on read.
pub trait AuxiliaryAuthorityCatalogPort: Send + Sync {
    /// Apply `batch` as one transaction.
    fn apply(
        &self,
        batch: &AuxiliaryMutationBatchV1,
    ) -> Result<AuxiliaryMutationReceiptV1, CoreError>;

    /// Visit every stored row, verified, in key order.
    fn for_each_row(
        &self,
        visit: &mut dyn FnMut(AuxiliaryRowV1) -> Result<(), CoreError>,
    ) -> Result<(), CoreError>;

    /// Every stored track row, verified.
    fn track_rows(&self) -> Result<Vec<AuxiliaryTrackRowV1>, CoreError>;

    /// Remove every row of every domain under one generation; returns how
    /// many rows went.
    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError>;
}
