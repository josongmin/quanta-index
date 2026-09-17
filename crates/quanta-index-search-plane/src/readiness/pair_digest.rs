//! Stable repo/revision pair digest and the lock striping derived from it.

use quanta_index_contract::{RepoId, RevisionId};
use sha2::{Digest, Sha256};

pub(crate) const SEARCH_CORPUS_LOCK_STRIPES_V1: usize = 64;

pub(super) fn search_corpus_pair_digest(repo_id: &RepoId, revision_id: &RevisionId) -> String {
    let mut hasher = Sha256::new();
    for value in [repo_id.as_str(), revision_id.as_str()] {
        hasher.update(value.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    format!("{digest:X}")
}

pub(crate) fn search_corpus_lock_stripe_v1(repo_id: &RepoId, revision_id: &RevisionId) -> usize {
    // Stable FNV-1a is used only to distribute lock contention. Collisions are
    // conservative serialization, never an authority or identity decision.
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in repo_id
        .as_str()
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(revision_id.as_str().bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    usize::from(hash.to_le_bytes()[0] & 0x3f)
}
