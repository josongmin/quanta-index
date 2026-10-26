//! Exact borrowed ordering scratch for Native corpus mutation validation.
//!
//! The caller funds and sorts the backing. This module verifies every index
//! permutation and key ordering against the actual batch before the ordinary
//! record predicates can use the certified no-set policy.

use super::{
    SearchCorpusIngestBatch, SearchCorpusSurfaceMutationConflictV1,
    SearchCorpusValidationProgressError, SearchScopeSurface,
};

pub struct CorpusSurfaceOrderScratchV3<'a> {
    pub replace_files: &'a [usize],
    pub tombstone_files: &'a [usize],
    pub candidate_ids: &'a [&'a str],
    pub candidate_order: &'a [usize],
    pub replace_semantic: &'a [usize],
    pub tombstone_semantic: &'a [usize],
    pub seen: &'a mut [u8],
}

type Failure<E> = SearchCorpusValidationProgressError<E, SearchCorpusSurfaceMutationConflictV1>;

fn invalid<E>() -> Failure<E> {
    // A bad scratch layout cannot be accepted as batch validation.
    Failure::Invalid(SearchCorpusSurfaceMutationConflictV1::InvalidCoverage)
}

fn certify_order<E, F, W, K, G, D>(
    order: &[usize],
    expected: usize,
    seen: &mut [u8],
    mut key: G,
    duplicate: D,
    progress: &mut F,
    work: &mut W,
) -> Result<(), Failure<E>>
where
    F: FnMut() -> Result<(), E>,
    W: FnMut(usize) -> Result<(), E>,
    K: Ord,
    G: FnMut(usize) -> Option<K>,
    D: Fn(usize) -> Option<SearchCorpusSurfaceMutationConflictV1>,
{
    if order.len() != expected || seen.len() < expected {
        return Err(invalid());
    }
    for slot in seen.get_mut(..expected).ok_or_else(invalid)? {
        work(1).map_err(Failure::Progress)?;
        *slot = 0;
    }
    for (ordinal, &index) in order.iter().enumerate() {
        if ordinal % 32 == 0 {
            progress().map_err(Failure::Progress)?;
        }
        let slot = seen.get_mut(index).ok_or_else(invalid)?;
        if index >= expected || *slot != 0 || key(index).is_none() {
            return Err(invalid());
        }
        work(1).map_err(Failure::Progress)?;
        *slot = 1;
    }
    for pair in order.windows(2) {
        let (Some(&left_index), Some(&right_index)) = (pair.first(), pair.get(1)) else {
            return Err(invalid());
        };
        let (Some(left), Some(right)) = (key(left_index), key(right_index)) else {
            return Err(invalid());
        };
        work(1).map_err(Failure::Progress)?;
        match left.cmp(&right) {
            core::cmp::Ordering::Less => {}
            core::cmp::Ordering::Equal => {
                return Err(Failure::Invalid(
                    duplicate(right_index).ok_or_else(invalid)?,
                ));
            }
            core::cmp::Ordering::Greater => return Err(invalid()),
        }
    }
    Ok(())
}

fn semantic_replace_key(
    batch: &SearchCorpusIngestBatch,
    index: usize,
) -> Option<(&str, &str, &str)> {
    let scope = &batch.semantic_replace_scopes.get(index)?.scope;
    Some((
        scope.corpus_kind.as_code_str(),
        scope.owner_kind.as_code_str(),
        scope.owner_id.as_str(),
    ))
}

fn semantic_tombstone_key(
    batch: &SearchCorpusIngestBatch,
    index: usize,
) -> Option<(&str, &str, &str)> {
    let scope = batch.semantic_tombstone_scopes.get(index)?;
    Some((
        scope.corpus_kind.as_code_str(),
        scope.owner_kind.as_code_str(),
        scope.owner_id.as_str(),
    ))
}

pub(super) fn certify_v3<E, F, W>(
    batch: &SearchCorpusIngestBatch,
    scratch: &mut CorpusSurfaceOrderScratchV3<'_>,
    progress: &mut F,
    work: &mut W,
) -> Result<(), Failure<E>>
where
    F: FnMut() -> Result<(), E>,
    W: FnMut(usize) -> Result<(), E>,
{
    certify_order(
        scratch.replace_files,
        batch.replace_scopes.len(),
        &mut *scratch.seen,
        |index| {
            batch
                .replace_scopes
                .get(index)
                .map(|scope| &scope.coverage.source.file)
        },
        |_| {
            Some(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(
                    SearchScopeSurface::Chunk,
                ),
            )
        },
        progress,
        work,
    )?;
    certify_order(
        scratch.tombstone_files,
        batch.tombstone_scopes.len(),
        &mut *scratch.seen,
        |index| batch.tombstone_scopes.get(index).map(|scope| &scope.file),
        |_| {
            Some(
                SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(
                    SearchScopeSurface::Chunk,
                ),
            )
        },
        progress,
        work,
    )?;
    let candidate_count = batch
        .replace_scopes
        .iter()
        .try_fold(0usize, |count, scope| {
            count
                .checked_add(scope.chunks.len())?
                .checked_add(scope.symbols.len())
        })
        .ok_or_else(invalid)?;
    if scratch.candidate_ids.len() != candidate_count {
        return Err(invalid());
    }
    let mut ordinal = 0usize;
    for scope in &batch.replace_scopes {
        for id in scope
            .chunks
            .iter()
            .map(|row| row.chunk_id.as_str())
            .chain(scope.symbols.iter().map(|row| row.symbol_id.as_str()))
        {
            if scratch.candidate_ids.get(ordinal).copied() != Some(id) {
                return Err(invalid());
            }
            ordinal = ordinal.checked_add(1).ok_or_else(invalid)?;
        }
    }
    certify_order(
        scratch.candidate_order,
        candidate_count,
        &mut *scratch.seen,
        |index| scratch.candidate_ids.get(index).copied(),
        |_| Some(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId),
        progress,
        work,
    )?;
    certify_order(
        scratch.replace_semantic,
        batch.semantic_replace_scopes.len(),
        &mut *scratch.seen,
        |index| semantic_replace_key(batch, index),
        |index| {
            let scope = &batch.semantic_replace_scopes.get(index)?.scope;
            Some(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(
                    SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind),
                ),
            )
        },
        progress,
        work,
    )?;
    certify_order(
        scratch.tombstone_semantic,
        batch.semantic_tombstone_scopes.len(),
        &mut *scratch.seen,
        |index| semantic_tombstone_key(batch, index),
        |index| {
            let scope = batch.semantic_tombstone_scopes.get(index)?;
            Some(
                SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(
                    SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind),
                ),
            )
        },
        progress,
        work,
    )?;
    let mut left = 0;
    let mut right = 0;
    while let (Some(&replace), Some(&tombstone)) = (
        scratch.replace_files.get(left),
        scratch.tombstone_files.get(right),
    ) {
        let (Some(a), Some(b)) = (
            batch
                .replace_scopes
                .get(replace)
                .map(|scope| &scope.coverage.source.file),
            batch
                .tombstone_scopes
                .get(tombstone)
                .map(|scope| &scope.file),
        ) else {
            return Err(invalid());
        };
        work(1).map_err(Failure::Progress)?;
        match a.cmp(b) {
            core::cmp::Ordering::Less => left = left.checked_add(1).ok_or_else(invalid)?,
            core::cmp::Ordering::Greater => right = right.checked_add(1).ok_or_else(invalid)?,
            core::cmp::Ordering::Equal => {
                return Err(Failure::Invalid(
                    SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                        SearchScopeSurface::Chunk,
                    ),
                ));
            }
        }
    }
    left = 0;
    right = 0;
    while let (Some(&replace), Some(&tombstone)) = (
        scratch.replace_semantic.get(left),
        scratch.tombstone_semantic.get(right),
    ) {
        let (Some(a), Some(b)) = (
            semantic_replace_key(batch, replace),
            semantic_tombstone_key(batch, tombstone),
        ) else {
            return Err(invalid());
        };
        work(1).map_err(Failure::Progress)?;
        match a.cmp(&b) {
            core::cmp::Ordering::Less => left = left.checked_add(1).ok_or_else(invalid)?,
            core::cmp::Ordering::Greater => right = right.checked_add(1).ok_or_else(invalid)?,
            core::cmp::Ordering::Equal => {
                let scope = &batch
                    .semantic_replace_scopes
                    .get(replace)
                    .ok_or_else(invalid)?
                    .scope;
                return Err(Failure::Invalid(
                    SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                        SearchScopeSurface::for_semantic_owner_v1(
                            scope.owner_kind,
                            scope.corpus_kind,
                        ),
                    ),
                ));
            }
        }
    }
    progress().map_err(Failure::Progress)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sorted_permutation_is_required_before_set_elision() {
        let keys = ["z", "a", "m"];
        let mut seen = [0_u8; 3];
        let mut work = 0usize;
        let mut charge = |count| {
            work += count;
            Ok::<(), ()>(())
        };
        let good = certify_order(
            &[1, 2, 0],
            keys.len(),
            &mut seen,
            |index| keys.get(index).copied(),
            |_| Some(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId),
            &mut || Ok::<(), ()>(()),
            &mut charge,
        );
        assert!(good.is_ok());
        assert_eq!(seen, [1, 1, 1]);
        assert!(work > 0);
        for bad in [&[0, 1, 2][..], &[1, 1, 0][..], &[1, 2][..]] {
            assert!(
                certify_order(
                    bad,
                    keys.len(),
                    &mut seen,
                    |index| keys.get(index).copied(),
                    |_| Some(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId),
                    &mut || Ok::<(), ()>(()),
                    &mut |_| Ok::<(), ()>(()),
                )
                .is_err()
            );
        }
        let duplicate = ["z", "a", "a"];
        assert!(matches!(
            certify_order(
                &[1, 2, 0],
                duplicate.len(),
                &mut seen,
                |index| duplicate.get(index).copied(),
                |_| Some(SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId),
                &mut || Ok::<(), ()>(()),
                &mut |_| Ok::<(), ()>(()),
            ),
            Err(Failure::Invalid(
                SearchCorpusSurfaceMutationConflictV1::DuplicateCandidateId
            ))
        ));
    }
}
