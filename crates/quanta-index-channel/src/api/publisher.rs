use quanta_index_contract::channel::ChannelSeq;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

use super::error::ChannelError;

/// Producer-facing channel surface.
///
/// Implementors are responsible for:
/// 1. assigning a strictly-increasing [`ChannelSeq`] per published op;
/// 2. persisting the op durably enough to satisfy the documented fsync policy;
/// 3. enforcing one-publisher-per-track exclusivity.
///
/// Implementors must NOT expose transport-specific types in this surface. If a
/// backend needs to surface more detail, route it through [`ChannelError::State`]
/// or telemetry, not through new trait methods.
pub trait BundleChannelPublisher: Send + Sync {
    /// Op type published over this channel (e.g. `LexicalChannelOp`).
    type Op;

    /// Append a typed op to the channel. Returns the assigned sequence number.
    fn publish(&self, op: Self::Op) -> Result<ChannelSeq, ChannelError>;

    /// Emit a [`Seal`] marker for `(repo, revision, generation)`. The publisher
    /// guarantees that all earlier published ops with the same generation are
    /// durably visible before this call returns.
    ///
    /// [`Seal`]: quanta_index_contract::channel::LexicalChannelOp::Seal
    fn seal(
        &self,
        repo: RepoId,
        revision: RevisionId,
        generation: ManifestGeneration,
    ) -> Result<ChannelSeq, ChannelError>;

    /// Force a durability barrier. Implementors that already fsync on every
    /// publish may make this a no-op.
    fn flush(&self) -> Result<(), ChannelError>;
}
