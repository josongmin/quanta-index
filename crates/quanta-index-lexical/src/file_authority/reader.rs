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

fn checked_digest(
    bytes: &[u8],
    expected: [u8; 32],
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let mut hasher = Sha256::new();
    for chunk in bytes.chunks(64 * 1024) {
        budget.checkpoint("lexical:file-authority-v15-selected-hash")?;
        hasher.update(chunk);
    }
    if <[u8; 32]>::from(hasher.finalize()) != expected {
        return Err(corrupt("selected posting range digest differs"));
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
        let mut selected_pages: BTreeMap<usize, Vec<[u8; 3]>> = BTreeMap::new();
        for (index, gram) in grams.iter().enumerate() {
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
            let page_index = bucket.pages.partition_point(|page| page.last < *gram);
            if let Some(page) = bucket.pages.get(page_index)
                && page.first <= *gram
            {
                selected_pages.entry(page_index).or_default().push(*gram);
            }
        }
        for (page_index, selected_grams) in selected_pages {
            let page = bucket
                .pages
                .get(page_index)
                .ok_or_else(|| corrupt("directory page missing"))?;
            let length = page
                .encoded_bytes()
                .map_err(|error| corrupt(&format!("directory page length: {error:?}")))?;
            if page
                .offset
                .checked_add(length)
                .is_none_or(|end| end > bucket.partition.bytes)
            {
                return Err(corrupt("directory page range differs from object"));
            }
            charge_work(work, policy, 0, length)?;
            budget.checkpoint("lexical:file-authority-v15-page-read")?;
            let table = read_range(
                bucket.partition.sha256,
                bucket.partition.bytes,
                page.offset,
                length,
            )?;
            if u64::try_from(table.len())
                .map_err(|_length_width_error| corrupt("directory page length width"))?
                != length
            {
                return Err(corrupt("directory page short read"));
            }
            checked_digest(&table, page.sha256, budget)?;
            let view = page
                .decode(&table, bucket.partition.terms, bucket.partition.bytes)
                .map_err(|error| corrupt(&format!("directory page decode: {error:?}")))?;
            for gram in selected_grams {
                let Some(list) = view
                    .lookup(gram)
                    .map_err(|error| corrupt(&format!("directory page lookup: {error:?}")))?
                else {
                    continue;
                };
                let count = u64::from(list.count);
                let length = count
                    .checked_mul(8)
                    .ok_or_else(|| corrupt("term list length overflow"))?;
                charge_work(work, policy, count, length)?;
                budget.checkpoint("lexical:file-authority-v15-list-read")?;
                let bytes = read_range(
                    bucket.partition.sha256,
                    bucket.partition.bytes,
                    list.offset,
                    length,
                )?;
                if u64::try_from(bytes.len())
                    .map_err(|_length_width_error| corrupt("term list length width"))?
                    != length
                {
                    return Err(corrupt("term list short read"));
                }
                checked_digest(&bytes, list.sha256, budget)?;
                let ids = result
                    .get_mut(&gram)
                    .ok_or_else(|| corrupt("requested gram missing"))?;
                let additional = usize::try_from(count)
                    .map_err(|_count_width_error| plan_limit("term list count width"))?;
                ids.try_reserve(additional).map_err(|_allocation_error| {
                    plan_limit("term list result allocation refused")
                })?;
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
            query_decoded_bytes: 256,
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
                pages: view
                    .page_descriptors()
                    .map(|page| page.expect("page"))
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
        assert!(absent.get(b"zzz").expect("absent gram listed").is_empty());
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
                Ok(block
                    .get(start..end)
                    .ok_or_else(|| {
                        CoreError::Storage("reader fixture selected range missing".into())
                    })?
                    .to_vec())
            },
        )
        .expect("selected gram");
        assert_eq!(
            found.get(b"abc").expect("selected gram listed").as_slice(),
            &[1, 3]
        );
        assert_eq!(reads, 2);
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
                let mut bytes = block
                    .get(start..end)
                    .ok_or_else(|| {
                        CoreError::Storage("reader fixture tamper range missing".into())
                    })?
                    .to_vec();
                *bytes.first_mut().ok_or_else(|| {
                    CoreError::Storage("reader fixture tamper range empty".into())
                })? ^= 1;
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
            Ok(block
                .get(start..end)
                .ok_or_else(|| CoreError::Storage("reader fixture selected range missing".into()))?
                .to_vec())
        };
        let _first_stage = posting_lists(
            &root,
            &directory,
            policy(2),
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
            policy(2),
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
            assert!(empty.get(b"zzz").expect("absent gram listed").is_empty());
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

    #[test]
    fn shared_page_is_read_once_and_only_selected_lists_are_read() {
        let (root, directory, block) = fixture();
        let mut ranges = Vec::new();
        let found = posting_lists(
            &root,
            &directory,
            policy(3),
            PostingSurface::Content,
            &[*b"abc", *b"bcd"],
            &mut QueryWork::default(),
            &RequestBudgetV1::unbounded(),
            |_, _, offset, len| {
                ranges.push((offset, len));
                let start = usize::try_from(offset).expect("offset");
                let end = start + usize::try_from(len).expect("length");
                Ok(block.get(start..end).expect("selected range").to_vec())
            },
        )
        .expect("two terms");
        assert_eq!(found.get(b"abc").expect("abc list").as_slice(), &[1, 3]);
        assert_eq!(found.get(b"bcd").expect("bcd list").as_slice(), &[3]);
        assert_eq!(ranges, [(32, 96), (128, 16), (144, 8)]);
    }

    #[test]
    fn authenticated_page_does_not_hide_a_mutated_selected_list() {
        let (root, directory, block) = fixture();
        let failure = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"abc"],
            &mut QueryWork::default(),
            &RequestBudgetV1::unbounded(),
            |_, _, offset, len| {
                let start = usize::try_from(offset).expect("offset");
                let end = start + usize::try_from(len).expect("length");
                let mut bytes = block.get(start..end).expect("selected range").to_vec();
                if offset == 128 {
                    *bytes.first_mut().expect("selected list byte") ^= 1;
                }
                Ok(bytes)
            },
        )
        .expect_err("list mutation after valid page");
        assert!(matches!(
            failure,
            CoreError::Typed {
                code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
                ..
            }
        ));
    }

    #[test]
    fn page_bytes_are_admitted_before_read_and_short_pages_refuse() {
        let (root, directory, block) = fixture();
        let mut limited = policy(2);
        limited.query_decoded_bytes = 95;
        let mut reads = 0;
        let failure = posting_lists(
            &root,
            &directory,
            limited,
            PostingSurface::Content,
            &[*b"abc"],
            &mut QueryWork::default(),
            &RequestBudgetV1::unbounded(),
            |_, _, _, _| {
                reads += 1;
                Ok(Vec::new())
            },
        )
        .expect_err("96-byte page exceeds budget");
        assert!(matches!(
            failure,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            }
        ));
        assert_eq!(reads, 0);
        let short = posting_lists(
            &root,
            &directory,
            policy(2),
            PostingSurface::Content,
            &[*b"abc"],
            &mut QueryWork::default(),
            &RequestBudgetV1::unbounded(),
            |_, _, offset, len| {
                let start = usize::try_from(offset).expect("offset");
                let end = start + usize::try_from(len).expect("length") - 1;
                Ok(block.get(start..end).expect("short range").to_vec())
            },
        )
        .expect_err("short directory page");
        assert!(matches!(
            short,
            CoreError::Typed {
                code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
                ..
            }
        ));
    }

    #[test]
    fn absent_term_inside_page_fences_reads_only_the_table() {
        let (root, directory, block) = fixture();
        let mut ranges = Vec::new();
        let found = posting_lists(
            &root,
            &directory,
            policy(1),
            PostingSurface::Content,
            &[*b"abd"],
            &mut QueryWork::default(),
            &RequestBudgetV1::unbounded(),
            |_, _, offset, len| {
                ranges.push((offset, len));
                let start = usize::try_from(offset).expect("offset");
                let end = start
                    .checked_add(usize::try_from(len).expect("length"))
                    .expect("end");
                Ok(block.get(start..end).expect("table range").to_vec())
            },
        )
        .expect("authenticated page absence");
        assert!(found.get(b"abd").expect("requested gram").is_empty());
        assert_eq!(ranges, [(32, 96)]);
    }
}
