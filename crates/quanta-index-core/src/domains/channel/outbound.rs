use quanta_index_contract::channel::ChannelSeq;
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

use crate::error::CoreError;

/// Optional observability hook for the channel dispatcher. Implementors can
/// surface metrics / tracing without coupling the dispatcher to specific
/// infrastructure.
pub trait ChannelObserver: Send + Sync {
    fn on_event(&self, repo: &RepoId, revision: &RevisionId, seq: ChannelSeq);
    fn on_seal(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        seq: ChannelSeq,
    );
    fn on_dispatch_error(&self, err: &CoreError);
}
