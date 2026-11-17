//! Existing sealed-history DATA using the canonical authority record decoder.
//! A matching historical record does not prove current physical content.

use super::SearchCorpusAuthorityRecordV1;
use crate::readiness::{
    pair_digest::search_corpus_pair_digest,
    recovery_read_io::{RecoveryReadFailureV1, directory, read_existing},
};
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::index::{IndexError, MemoryLifetime, ObjectReadIoPort, WorkBudget};
use quanta_index_ipc::decode_cbor_payload;
use std::path::Path;

/// Require an exact, already-existing historical seal without opening the
/// mutating AuxiliaryAuthorityStore or restoring its in-memory ledger.
/// The caller retains its real Legacy root lease and I/O owner through the
/// deployment transaction and separately validates Catalog and sealed files.
pub fn inspect_existing_sealed_search_corpus_v1(
    state_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    manifest_digest: &str,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<(), RecoveryReadFailureV1> {
    budget.checkpoint()?;
    // The canonical persisted record has five fields. This bound only limits
    // this recovery reader; unsupported oversized records refuse before read.
    const MAX_RECORD_BYTES: u64 = 65_536;
    let path_bytes = u64::try_from(state_root.as_os_str().len())
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(8)
        .and_then(|n| n.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    let _paths = budget
        .reserve_memory(path_bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let root_before = directory(state_root, budget)?;
    let authorities = state_root.join("authorities");
    let corpus = authorities.join("search-corpus");
    let pair = corpus.join(search_corpus_pair_digest(repo, revision));
    let parents = [&authorities, &corpus, &pair];
    let mut identities = Vec::with_capacity(parents.len());
    for parent in parents {
        let observed = directory(parent, budget)?;
        if observed.owner != root_before.owner {
            return Err(IndexError::DigestConflict.into());
        }
        identities.push(observed);
    }
    let path = pair.join(format!("g{}.cbor", generation.get()));
    let input = read_existing(&path, root_before.owner, MAX_RECORD_BYTES, io, budget)?;
    let record: SearchCorpusAuthorityRecordV1 =
        decode_cbor_payload(input.bytes_v1()).map_err(|error| {
            quanta_index_core::CoreError::Storage(format!(
                "existing search-corpus authority decode: {error}"
            ))
        })?;
    record.validate_identity(repo, revision, generation, &path)?;
    if record.manifest_digest != manifest_digest {
        return Err(IndexError::DigestConflict.into());
    }
    for (parent, before) in parents.into_iter().zip(identities) {
        if directory(parent, budget)? != before {
            return Err(IndexError::DigestConflict.into());
        }
    }
    if directory(state_root, budget)? != root_before {
        return Err(IndexError::DigestConflict.into());
    }
    budget.checkpoint()?;
    Ok(())
}
