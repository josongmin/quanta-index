//! Deterministic Native file fusion; callers fund scratch before entry.

use super::{NativeHybridFileHit, NativeSemanticFileHit, NativeTextHit};
use crate::{
    FiniteScore, RepoId, SemanticQueryRequest, TextQueryRequest,
    index::{Digest, IndexInstanceId, OwnerKey},
};
use sha2::{Digest as _, Sha256};
use std::cmp::Ordering;

/// Canonical order of `select:path` groups under the same published text view.
/// The representative owner is selected inside a group and cannot break ties
/// between groups. Repository and path are compared by their canonical bytes.
pub fn compare_native_text_path_rank_v1(
    left: (FiniteScore, &RepoId, &str),
    right: (FiniteScore, &RepoId, &str),
) -> Ordering {
    right
        .0
        .get()
        .total_cmp(&left.0.get())
        .then_with(|| left.1.as_str().cmp(right.1.as_str()))
        .then_with(|| left.2.cmp(right.2))
}

/// Hash the exact canonical request representation without a temporary buffer.
/// A page-size or filter change deliberately invalidates the continuation.
pub fn native_text_query_digest(
    instance: IndexInstanceId,
    query: &TextQueryRequest,
) -> Result<Digest, String> {
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    writer.0.update(b"quanta-index/native-text-page/query/v1\0");
    writer.0.update(instance.0);
    serde_json::to_writer(&mut writer, query).map_err(|error| error.to_string())?;
    Ok(Digest(writer.0.finalize().into()))
}

/// Fuse independent file pools with deterministic reciprocal ranks.
///
/// Every input is already a file candidate, with duplicate files removed by
/// its producing reader before its lane cap. No lexical intersection is used.
/// Scratch holds the full union until the final file cut.
pub fn rank_native_hybrid_files(
    lexical: &[NativeTextHit],
    dense: &[NativeSemanticFileHit],
    file_top_k: u32,
    scratch: &mut Vec<NativeHybridFileHit>,
) -> Result<(), String> {
    let required = lexical
        .len()
        .checked_add(dense.len())
        .ok_or("file union overflow")?;
    if file_top_k == 0
        || file_top_k > super::super::MAX_NATIVE_TEXT_TOP_K
        || scratch.capacity() < required
    {
        return Err("file fusion limit or scratch capacity is invalid".into());
    }
    scratch.clear();
    for (dense_lane, count) in [(false, lexical.len()), (true, dense.len())] {
        let mut previous: Option<(OwnerKey, f64)> = None;
        for index in 0..count {
            let (owner, score, dense_hit) = if dense_lane {
                let hit = dense
                    .get(index)
                    .ok_or("dense file rank is outside its pool")?;
                (hit.owner, hit.score.get(), Some(hit))
            } else {
                let hit = lexical
                    .get(index)
                    .ok_or("lexical file rank is outside its pool")?;
                (hit.owner, hit.score.get(), None)
            };
            if !score.is_finite()
                || (!dense_lane && score < 0.0)
                || (dense_lane && !(-1.0..=1.0).contains(&score))
                || previous.is_some_and(|(prior_owner, prior_score)| {
                    prior_score.total_cmp(&score).is_lt()
                        || (prior_score.total_cmp(&score).is_eq() && prior_owner >= owner)
                })
            {
                return Err("file candidate pool is not canonically ranked".into());
            }
            previous = Some((owner, score));
            let rank = u32::try_from(index)
                .map_err(|error| error.to_string())?
                .checked_add(1)
                .ok_or("file rank overflow")?;
            let position = match scratch.binary_search_by_key(&owner, |hit| hit.owner) {
                Ok(position) => position,
                Err(position) => {
                    scratch.insert(
                        position,
                        NativeHybridFileHit {
                            owner,
                            lexical_rank: None,
                            dense_rank: None,
                            score: FiniteScore::new(0.0).map_err(|error| error.to_string())?,
                        },
                    );
                    position
                }
            };
            let row = scratch
                .get_mut(position)
                .ok_or("file union position is invalid")?;
            if let Some(dense_hit) = dense_hit
                && let Some(rank) = row.lexical_rank
            {
                let lexical = lexical
                    .get(one_based_rank_index(rank)?)
                    .ok_or("file lexical rank is outside its pool")?;
                if lexical.source != dense_hit.source || lexical.path != dense_hit.path {
                    return Err(
                        "same-view file candidate source identity differs between lanes".into(),
                    );
                }
            }
            let slot = if dense_lane {
                &mut row.dense_rank
            } else {
                &mut row.lexical_rank
            };
            if slot.replace(rank).is_some() {
                return Err("file candidate pool repeated a source owner".into());
            }
        }
    }
    for row in scratch.iter_mut() {
        let score = row
            .lexical_rank
            .map_or(0.0, |rank| 1.0 / (60.0 + f64::from(rank)))
            + row
                .dense_rank
                .map_or(0.0, |rank| 1.0 / (60.0 + f64::from(rank)));
        row.score = FiniteScore::new(score).map_err(|error| error.to_string())?;
    }
    scratch.sort_unstable_by(|left, right| {
        right
            .score
            .get()
            .total_cmp(&left.score.get())
            .then_with(|| left.owner.cmp(&right.owner))
    });
    scratch.truncate(usize::try_from(file_top_k).map_err(|error| error.to_string())?);
    Ok(())
}

fn one_based_rank_index(rank: u32) -> Result<usize, String> {
    let index = rank.checked_sub(1).ok_or("file rank is not one-based")?;
    usize::try_from(index).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{native_text_query_digest, rank_native_hybrid_files};
    use crate::{
        FiniteScore, QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
        index::{Digest, IndexInstanceId, ObjectRef, OwnerKey, ProducerId, StableOwnerId},
        native_protocol::{NativeSemanticFileHit, NativeTextHit},
    };

    fn owner(id: u8) -> OwnerKey {
        OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([id; 16]),
        }
    }
    fn lexical(id: u8, score: f64) -> Result<NativeTextHit, String> {
        Ok(NativeTextHit {
            owner: owner(id),
            source: ObjectRef(Digest([id; 32])),
            path: format!("{id}.rs"),
            score: FiniteScore::new(score).map_err(|error| error.to_string())?,
        })
    }
    fn dense(id: u8, score: f64) -> Result<NativeSemanticFileHit, String> {
        Ok(NativeSemanticFileHit {
            owner: owner(id),
            source: ObjectRef(Digest([id; 32])),
            path: format!("{id}.rs"),
            projection_owner: owner(id + 10),
            vector_id: vec![id],
            chunk: None,
            score: FiniteScore::new(score).map_err(|error| error.to_string())?,
        })
    }

    #[test]
    fn independent_file_rrf_gold_keeps_dense_only_and_both_full_pools() -> Result<(), String> {
        let lexical = [lexical(1, 3.0)?, lexical(2, 2.0)?, lexical(3, 1.0)?];
        let dense = [dense(2, 0.9)?, dense(4, 0.8)?, dense(1, 0.7)?];
        let mut fused = Vec::with_capacity(6);
        rank_native_hybrid_files(&lexical, &dense, 4, &mut fused)?;
        assert_eq!(
            fused.iter().map(|hit| hit.owner).collect::<Vec<_>>(),
            [owner(2), owner(1), owner(4), owner(3)]
        );
        let gold = [
            (123.0 / 3782.0, Some(2), Some(1)),
            (124.0 / 3843.0, Some(1), Some(3)),
            (1.0 / 62.0, None, Some(2)),
            (1.0 / 63.0, Some(3), None),
        ];
        for (actual, (score, lexical, dense)) in fused.iter().zip(gold) {
            assert!((actual.score.get() - score).abs() < 1e-15);
            assert_eq!((actual.lexical_rank, actual.dense_rank), (lexical, dense));
        }
        Ok(())
    }

    #[test]
    fn rrf_ties_use_full_source_owner_and_cut_after_union() -> Result<(), String> {
        let mut lexical = lexical(1, 1.0)?;
        lexical.path = "same.rs".into();
        lexical.owner.producer = ProducerId([2; 16]);
        let mut dense = dense(1, 1.0)?;
        dense.path = "same.rs".into();
        let mut fused = Vec::with_capacity(2);
        rank_native_hybrid_files(&[lexical], &[dense], 2, &mut fused)?;
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].owner.producer, ProducerId([1; 16]));
        assert_eq!(fused[1].owner.producer, ProducerId([2; 16]));
        Ok(())
    }

    #[test]
    fn fusion_refuses_duplicate_files_bad_order_and_unfunded_scratch() -> Result<(), String> {
        let rows = [lexical(1, 2.0)?, lexical(1, 1.0)?];
        let mut scratch = Vec::with_capacity(2);
        assert!(rank_native_hybrid_files(&rows, &[], 2, &mut scratch).is_err());
        let rows = [lexical(1, 1.0)?, lexical(2, 2.0)?];
        assert!(rank_native_hybrid_files(&rows, &[], 2, &mut scratch).is_err());
        let mut scratch = Vec::new();
        assert!(rank_native_hybrid_files(&rows, &[], 2, &mut scratch).is_err());
        assert_eq!(scratch.capacity(), 0);
        rank_native_hybrid_files(&[], &[], 1, &mut scratch)?;
        assert!(scratch.is_empty());
        Ok(())
    }

    #[test]
    fn text_page_digest_binds_exact_query_filter_instance_and_size() -> Result<(), String> {
        let mut query = TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 2,
            cursor: None,
        };
        let instance = IndexInstanceId([1; 16]);
        let original = native_text_query_digest(instance, &query)?;
        assert_eq!(original, native_text_query_digest(instance, &query)?);
        query.top_k = 3;
        assert_ne!(original, native_text_query_digest(instance, &query)?);
        query.top_k = 2;
        query.query_text = "other".into();
        assert_ne!(original, native_text_query_digest(instance, &query)?);
        query.query_text = "needle".into();
        assert_ne!(
            original,
            native_text_query_digest(IndexInstanceId([2; 16]), &query)?
        );
        Ok(())
    }
}

pub fn validate_native_hybrid_file_query(
    text: &TextQueryRequest,
    semantic: &SemanticQueryRequest,
    file_top_k: u32,
) -> Result<(), String> {
    use super::super::{
        MAX_NATIVE_SEMANTIC_QUERY_BYTES, MAX_NATIVE_SEMANTIC_TOP_K, MAX_NATIVE_TEXT_TOP_K,
    };
    if text.syntax != crate::TextQuerySyntax::Native
        || text.cursor.is_some()
        || text.generation.is_some()
        || text.generation_selector.is_some()
        || semantic.generation.is_some()
        || semantic.generation_selector.is_some()
        || semantic.lexical_scope.is_some()
        || text.constraints != semantic.constraints
        || !text.constraints.language_any_of.is_empty()
        || !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&text.top_k)
        || !(1..=MAX_NATIVE_SEMANTIC_TOP_K).contains(&semantic.top_k)
        || file_top_k == 0
        || file_top_k > text.top_k
        || file_top_k > semantic.top_k
        || text.query_text.trim().is_empty()
        || semantic.query_text.trim().is_empty()
        || semantic.query_text.len() > MAX_NATIVE_SEMANTIC_QUERY_BYTES
        || text.query_text.len() > crate::MAX_NATIVE_CONTENT_INPUT_BYTES
        || text.query_text.contains('\0')
        || semantic.query_text.contains('\0')
    {
        return Err("Native Hybrid requires bounded independent file lanes and identical supported constraints".into());
    }
    Ok(())
}
