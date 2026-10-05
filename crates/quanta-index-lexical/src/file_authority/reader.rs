//! Selective query reads from cold-authenticated F15 posting lists.
//! One `QueryWork` spans literal search and fallback.

use std::collections::BTreeMap;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};

use super::codec::PostingSurface;
use super::root::{AuthorityPolicy, AuthorityRoot};
use super::verify::PostingDirectory;

fn plan_limit(reason: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
        message: format!("lexical file authority v15 query: {reason}"),
    }
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("lexical file authority v15 query: {reason}"),
    }
}

#[derive(Default)]
pub(crate) struct QueryWork {
    directory_probes: u64,
    list_reads: u64,
    posting_ids: u64,
    decoded_bytes: u64,
    decoded_ids: u64,
}

fn charge_work(
    work: &mut QueryWork,
    policy: AuthorityPolicy,
    count: u64,
    bytes: u64,
) -> Result<(), CoreError> {
    work.list_reads = work
        .list_reads
        .checked_add(1)
        .ok_or_else(|| plan_limit("list read count overflow"))?;
    work.posting_ids = work
        .posting_ids
        .checked_add(count)
        .ok_or_else(|| plan_limit("posting ID count overflow"))?;
    work.decoded_ids = work
        .decoded_ids
        .checked_add(count)
        .ok_or_else(|| plan_limit("decoded ID count overflow"))?;
    work.decoded_bytes = work
        .decoded_bytes
        .checked_add(bytes)
        .ok_or_else(|| plan_limit("decoded byte count overflow"))?;
    if work.list_reads > policy.query_list_reads
        || work.posting_ids > policy.query_posting_ids
        || work.decoded_ids > policy.query_decoded_ids
        || work.decoded_bytes > policy.query_decoded_bytes
    {
        return Err(plan_limit("global query posting work exceeds policy"));
    }
    Ok(())
}

/// Read selected posting lists under the generation lease.
///
/// The owner callback refuses symlinks/devices and checks committed object
/// length and exact range before allocation. The selected list digest detects
/// same-length in-place object mutation.
pub(super) fn posting_lists<R>(
    root: &AuthorityRoot,
    directory: &PostingDirectory,
    policy: AuthorityPolicy,
    surface: PostingSurface,
    grams: &[[u8; 3]],
    work: &mut QueryWork,
    budget: &RequestBudgetV1,
    mut read_range: R,
) -> Result<BTreeMap<[u8; 3], Vec<u64>>, CoreError>
where
    R: FnMut([u8; 32], u64, u64, u64) -> Result<Vec<u8>, CoreError>,
{
    if grams.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left >= right)
    }) {
        return Err(CoreError::Storage(
            "lexical file authority v15: query grams not sorted".into(),
        ));
    }
    let (partitions, buckets) = match surface {
        PostingSurface::Path => (&root.path_postings, &directory.path),
        PostingSurface::Content => (&root.content_postings, &directory.content),
    };
    if partitions.len() != buckets.len()
        || partitions
            .iter()
            .zip(buckets)
            .any(|(partition, bucket)| partition != &bucket.partition)
    {
        return Err(corrupt("term directory differs from committed root"));
    }
    let mut result: BTreeMap<[u8; 3], Vec<u64>> = grams
        .iter()
        .copied()
        .map(|gram| (gram, Vec::new()))
        .collect();
    let max_directory_probes = policy
        .query_list_reads
        .checked_mul(policy.partitions)
        .ok_or_else(|| plan_limit("directory lookup policy overflow"))?;
    for bucket in buckets {
        budget.checkpoint("lexical:file-authority-v15-list-lookup")?;
        for (index, (gram, ids)) in result.iter_mut().enumerate() {
            if index.is_multiple_of(64) {
                budget.checkpoint("lexical:file-authority-v15-directory-lookup")?;
            }
            work.directory_probes = work
                .directory_probes
                .checked_add(1)
                .ok_or_else(|| plan_limit("directory lookup count overflow"))?;
            if work.directory_probes > max_directory_probes {
                return Err(plan_limit("global query directory lookup limit exceeded"));
            }
            let Ok(index) = bucket.terms.binary_search_by_key(gram, |row| row.gram) else {
                continue;
            };
            let row = bucket
                .terms
                .get(index)
                .ok_or_else(|| corrupt("term directory index missing"))?;
            let count = u64::from(row.count);
            let length = count
                .checked_mul(8)
                .ok_or_else(|| corrupt("term list length overflow"))?;
            if row
                .offset
                .checked_add(length)
                .is_none_or(|end| end > bucket.partition.bytes)
            {
                return Err(corrupt("term list range differs from object"));
            }
            charge_work(work, policy, count, length)?;
            budget.checkpoint("lexical:file-authority-v15-list-read")?;
            let bytes = read_range(
                bucket.partition.sha256,
                bucket.partition.bytes,
                row.offset,
                length,
            )?;
            if u64::try_from(bytes.len())
                .map_err(|_length_width_error| corrupt("term list length width"))?
                != length
            {
                return Err(corrupt("term list short read"));
            }
            let mut hasher = Sha256::new();
            for chunk in bytes.chunks(64 * 1024) {
                budget.checkpoint("lexical:file-authority-v15-list-hash")?;
                hasher.update(chunk);
            }
            let actual: [u8; 32] = hasher.finalize().into();
            if actual != row.sha256 {
                return Err(corrupt("selected posting list digest differs"));
            }
            let additional = usize::try_from(count)
                .map_err(|_count_width_error| plan_limit("term list count width"))?;
            ids.try_reserve(additional)
                .map_err(|_allocation_error| plan_limit("term list result allocation refused"))?;
            let mut previous = None;
            for (index, word) in bytes.chunks_exact(8).enumerate() {
                if index.is_multiple_of(1024) {
                    budget.checkpoint("lexical:file-authority-v15-list-decode")?;
                }
                let id = u64::from_le_bytes(
                    word.try_into()
                        .map_err(|_id_width_error| corrupt("term list ID width"))?,
                );
                if id == 0 || previous.is_some_and(|prior| prior >= id) {
                    return Err(corrupt("term list IDs not strictly ascending"));
                }
                previous = Some(id);
                ids.push(id);
            }
        }
    }
    for ids in result.values_mut() {
        budget.checkpoint("lexical:file-authority-v15-sort-list")?;
        ids.sort_unstable();
        budget.checkpoint("lexical:file-authority-v15-sorted-list")?;
        for (index, pair) in ids.windows(2).enumerate() {
            if index.is_multiple_of(1024) {
                budget.checkpoint("lexical:file-authority-v15-unique-list")?;
            }
            if pair.first() == pair.get(1) {
                return Err(corrupt("source ID occurs in multiple source-key buckets"));
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_core::{CoreError, RequestBudgetV1};
    use sha2::{Digest as _, Sha256};

    use super::super::codec::{
        CodecLimits, PostingInput, PostingSurface, decode_posting_block, encode_posting_block,
    };
    use super::super::root::{AuthorityPolicy, AuthorityRoot, Partition};
    use super::super::verify::{PostingBucketDirectory, PostingDirectory};
    use super::{QueryWork, posting_lists};

    fn policy(list_reads: u64) -> AuthorityPolicy {
        AuthorityPolicy {
            root_bytes: 4096,
            source_files: 4,
            source_bytes: 1024,
            pack_bytes: 1024,
            total_pack_bytes: 1024,
            posting_block_bytes: 1024,
            total_posting_bytes: 2048,
            total_memberships: 8,
            partitions: 4,
            source_id: 8,
            bucket_scratch_bytes: 8192,
            term_directory_bytes: 4096,
            resident_file_heap_bytes: 8192,
            query_list_reads: list_reads,
            query_posting_ids: 8,
            query_decoded_bytes: 64,
            query_decoded_ids: 8,
        }
    }

    fn fixture() -> (AuthorityRoot, PostingDirectory, Vec<u8>) {
        let limits = CodecLimits {
            source_pack_encoded_bytes: 1024,
            posting_block_encoded_bytes: 1024,
            sources: 4,
            terms: 4,
            memberships: 8,
        };
        let block = encode_posting_block(
            PostingSurface::Content,
            &[
                PostingInput {
                    gram: *b"abc",
                    source_ids: &[1, 3],
                },
                PostingInput {
                    gram: *b"bcd",
                    source_ids: &[3],
                },
            ],
            &limits,
        )
        .expect("fixed block");
        let view = decode_posting_block(&block, PostingSurface::Content, &limits).expect("decode");
        let partition = Partition {
            prefix_bits: 8,
            prefix: [0; 32],
            sha256: Sha256::digest(&block).into(),
            bytes: u64::try_from(block.len()).expect("length"),
            entries: 3,
            terms: 2,
        };
        let directory = PostingDirectory {
            path: Vec::new(),
            content: vec![PostingBucketDirectory {
                partition: partition.clone(),
                terms: view
                    .term_descriptors()
                    .map(|term| term.expect("term"))
                    .collect(),
            }],
        };
        let root = AuthorityRoot {
            policy_sha256: policy(2).digest(),
            next_source_id: 1,
            sources: Vec::new(),
            packs: Vec::new(),
            path_postings: Vec::new(),
            content_postings: vec![partition],
        };
        (root, directory, block)
    }

    #[test]
    fn unmatched_term_reads_no_payload_and_selected_list_is_exact() {
        let (root, directory, block) = fixture();
        let mut work = QueryWork::default();
        let mut reads = 0;
        let absent = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"zzz"],
            &mut work,
            &RequestBudgetV1::unbounded(),
            |_, _, _, _| {
                reads += 1;
                Ok(Vec::new())
            },
        )
        .expect("absent gram");
        assert_eq!(absent[&*b"zzz"], Vec::<u64>::new());
        assert_eq!(reads, 0);
        let found = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"abc"],
            &mut work,
            &RequestBudgetV1::unbounded(),
            |sha, total, offset, len| {
                reads += 1;
                let expected: [u8; 32] = Sha256::digest(&block).into();
                assert_eq!(sha, expected);
                assert_eq!(total, u64::try_from(block.len()).expect("length"));
                let start = usize::try_from(offset).expect("offset");
                let end = start + usize::try_from(len).expect("length");
                Ok(block[start..end].to_vec())
            },
        )
        .expect("selected gram");
        assert_eq!(found[&*b"abc"], vec![1, 3]);
        assert_eq!(reads, 1);
    }

    #[test]
    fn same_length_mutation_and_cross_stage_global_cap_refuse() {
        let (root, directory, block) = fixture();
        let mut work = QueryWork::default();
        let mutated = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"abc"],
            &mut work,
            &RequestBudgetV1::unbounded(),
            |_, _, offset, len| {
                let start = usize::try_from(offset).expect("offset");
                let end = start + usize::try_from(len).expect("length");
                let mut bytes = block[start..end].to_vec();
                bytes[0] ^= 1;
                Ok(bytes)
            },
        )
        .expect_err("same-size tamper");
        assert!(matches!(
            mutated,
            CoreError::Typed {
                code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
                ..
            }
        ));

        let mut shared = QueryWork::default();
        let read = |_: [u8; 32], _: u64, offset: u64, len: u64| {
            let start = usize::try_from(offset).expect("offset");
            let end = start + usize::try_from(len).expect("length");
            Ok(block[start..end].to_vec())
        };
        let _first_stage = posting_lists(
            &root,
            &directory,
            policy(1),
            PostingSurface::Content,
            &[*b"abc"],
            &mut shared,
            &RequestBudgetV1::unbounded(),
            read,
        )
        .expect("first stage");
        let failure = posting_lists(
            &root,
            &directory,
            policy(1),
            PostingSurface::Content,
            &[*b"bcd"],
            &mut shared,
            &RequestBudgetV1::unbounded(),
            read,
        )
        .expect_err("shared cap");
        assert!(matches!(
            failure,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            }
        ));
    }

    #[test]
    fn cancelled_query_preserves_request_error_before_read() {
        let (root, directory, _) = fixture();
        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        let mut reads = 0;
        let failure = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"abc"],
            &mut QueryWork::default(),
            &cancelled,
            |_, _, _, _| {
                reads += 1;
                Ok(Vec::new())
            },
        )
        .expect_err("cancelled");
        assert!(matches!(
            failure,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            }
        ));
        assert_eq!(reads, 0);

        let past = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("past deadline");
        let failure = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"abc"],
            &mut QueryWork::default(),
            &RequestBudgetV1::until(past),
            |_, _, _, _| {
                reads += 1;
                Ok(Vec::new())
            },
        )
        .expect_err("expired deadline");
        assert!(matches!(
            failure,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
                ..
            }
        ));
        assert_eq!(reads, 0);
    }

    #[test]
    fn zero_match_directory_probes_share_a_global_cpu_cap() {
        let (root, directory, _) = fixture();
        let mut work = QueryWork::default();
        let mut reads = 0;
        for _ in 0..4 {
            let empty = posting_lists(
                &root,
                &directory,
                policy(1),
                PostingSurface::Content,
                &[*b"zzz"],
                &mut work,
                &RequestBudgetV1::unbounded(),
                |_, _, _, _| {
                    reads += 1;
                    Ok(Vec::new())
                },
            )
            .expect("bounded directory probe");
            assert!(empty[&*b"zzz"].is_empty());
        }
        let failure = posting_lists(
            &root,
            &directory,
            policy(1),
            PostingSurface::Content,
            &[*b"zzz"],
            &mut work,
            &RequestBudgetV1::unbounded(),
            |_, _, _, _| {
                reads += 1;
                Ok(Vec::new())
            },
        )
        .expect_err("global directory probe cap");
        assert!(matches!(
            failure,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            }
        ));
        assert_eq!(reads, 0);
    }
}
