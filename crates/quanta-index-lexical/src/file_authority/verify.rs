//! Independent cold-open verification of the single F15 file authority IR.
//!
//! One source-key bucket is reconstructed at a time. Root totals and blob
//! hashes alone are insufficient: this checks exact folded gram membership,
//! stable live IDs and per-source counts against committed source bodies.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_lq_trigram::trigrams_of;
use sha2::{Digest as _, Sha256};

use super::SourceFile;
use super::codec::{
    CodecLimits, PostingBlockView, PostingSurface, PostingTermDescriptor, decode_posting_block,
    decode_source_pack,
};
use super::root::{
    AuthorityPolicy, AuthorityRoot, Partition, SourceRow, TERM_DIRECTORY_BLOCK_CHARGE,
    TERM_DIRECTORY_ROW_CHARGE, resident_file_charge, source_key_digest,
};

const _: () = assert!(std::mem::size_of::<PostingTermDescriptor>() <= 64);

#[derive(Debug)]
pub(crate) struct PostingBucketDirectory {
    pub(crate) partition: Partition,
    pub(crate) terms: Vec<PostingTermDescriptor>,
}
const _: () = assert!(std::mem::size_of::<PostingBucketDirectory>() <= 128);

#[derive(Debug, Default)]
pub(crate) struct PostingDirectory {
    pub(crate) path: Vec<PostingBucketDirectory>,
    pub(crate) content: Vec<PostingBucketDirectory>,
}

#[derive(Debug)]
pub(crate) struct VerifiedAuthority {
    pub(crate) root: AuthorityRoot,
    pub(crate) files: Vec<SourceFile>,
    pub(crate) posting_directory: PostingDirectory,
}

fn corrupt(reason: &str) -> String {
    format!("file authority v15 cold open: {reason}")
}

fn blob<R>(row: &Partition, read: &mut R) -> Result<Vec<u8>, String>
where
    R: FnMut([u8; 32], u64) -> Result<Vec<u8>, String>,
{
    // The caller must stat the file and refuse size > expected before read.
    let bytes = read(row.sha256, row.bytes)?;
    if u64::try_from(bytes.len()).map_err(|_| corrupt("blob length overflow"))? != row.bytes {
        return Err(corrupt("blob length differs from root"));
    }
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if actual != row.sha256 {
        return Err(corrupt("blob digest differs from root"));
    }
    Ok(bytes)
}

struct Scratch {
    bytes: u64,
    ceiling: u64,
}

impl Scratch {
    fn charge(&mut self, bytes: u64) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| corrupt("scratch charge overflow"))?;
        if self.bytes > self.ceiling {
            return Err(corrupt("bucket scratch exceeds policy"));
        }
        Ok(())
    }
}

type Expected = BTreeMap<[u8; 3], BTreeSet<u64>>;

fn add_source(
    bytes: &[u8],
    id: u64,
    expected: &mut Expected,
    scratch: &mut Scratch,
) -> Result<u32, String> {
    let mut distinct = BTreeSet::new();
    for gram in trigrams_of(bytes) {
        if distinct.insert(gram) {
            scratch.charge(64)?;
        }
    }
    let count =
        u32::try_from(distinct.len()).map_err(|_| corrupt("one source term count overflow"))?;
    for gram in distinct {
        let new_term = !expected.contains_key(&gram);
        if new_term {
            scratch.charge(128)?;
        }
        let ids = expected.entry(gram).or_default();
        if !ids.insert(id) {
            return Err(corrupt("duplicate source ID in term"));
        }
        scratch.charge(64)?;
    }
    Ok(count)
}

fn compare_block(
    descriptor: &Partition,
    actual: &PostingBlockView<'_>,
    expected: &Expected,
) -> Result<(), String> {
    if actual.membership_count() != descriptor.entries {
        return Err(corrupt("block membership count differs from root"));
    }
    let mut actual_terms = actual.iter_terms();
    for (gram, ids) in expected {
        let (actual_gram, count, actual_ids) =
            actual_terms.next().ok_or_else(|| corrupt("missing term"))?;
        if actual_gram != *gram || count != ids.len() || !actual_ids.eq(ids.iter().copied()) {
            return Err(corrupt("posting term or live IDs differ from source bytes"));
        }
    }
    if actual_terms.next().is_some() {
        return Err(corrupt("unreferenced posting term"));
    }
    Ok(())
}

fn append_directory(
    target: &mut Vec<PostingBucketDirectory>,
    descriptor: &Partition,
    view: &PostingBlockView<'_>,
    charged: &mut u64,
    policy: AuthorityPolicy,
) -> Result<(), String> {
    let terms =
        usize::try_from(descriptor.terms).map_err(|_| corrupt("term directory count width"))?;
    let block_charge = u64::from(descriptor.terms)
        .checked_mul(TERM_DIRECTORY_ROW_CHARGE)
        .and_then(|rows| rows.checked_add(TERM_DIRECTORY_BLOCK_CHARGE))
        .ok_or_else(|| corrupt("term directory charge overflow"))?;
    *charged = charged
        .checked_add(block_charge)
        .ok_or_else(|| corrupt("term directory aggregate charge overflow"))?;
    if *charged > policy.max_term_directory_bytes {
        return Err(corrupt("term directory exceeds policy"));
    }
    if view.term_descriptors().len() != terms {
        return Err(corrupt("posting term count differs from root"));
    }
    let mut rows = Vec::new();
    rows.try_reserve_exact(terms)
        .map_err(|_| corrupt("term directory allocation refused"))?;
    for term in view.term_descriptors() {
        rows.push(term.map_err(|error| corrupt(&format!("term descriptor: {error:?}")))?);
    }
    target
        .try_reserve(1)
        .map_err(|_| corrupt("term directory bucket allocation refused"))?;
    target.push(PostingBucketDirectory {
        partition: descriptor.clone(),
        terms: rows,
    });
    Ok(())
}

/// `read_blob` is an owner-provided bounded file reader. It receives the
/// committed digest and expected length; it must reject symlinks, devices,
/// oversized files and short reads before allocating bytes.
///
/// This verifies one fixed source-key bucket at a time. A caller keeps the
/// generation lease for the lifetime of the returned root and lazy queries.
pub(super) fn verify_authority<R>(
    root_bytes: &[u8],
    policy: AuthorityPolicy,
    codec_limits: &CodecLimits,
    mut read_blob: R,
) -> Result<VerifiedAuthority, String>
where
    R: FnMut([u8; 32], u64) -> Result<Vec<u8>, String>,
{
    let root = AuthorityRoot::decode(root_bytes, policy)?;
    let mut files = Vec::new();
    let mut posting_directory = PostingDirectory::default();
    let mut directory_charge = 0_u64;
    let mut resident_charge = root.term_directory_charge(policy)?;
    files
        .try_reserve(root.sources.len())
        .map_err(|_| corrupt("source vector allocation refused"))?;
    let mut by_bucket: BTreeMap<u8, Vec<&SourceRow>> = BTreeMap::new();
    for row in &root.sources {
        let key = source_key_digest(&row.source)?;
        by_bucket.entry(key[0]).or_default().push(row);
    }
    if root.packs.len() != root.path_postings.len()
        || root.packs.len() != root.content_postings.len()
        || root.packs.len() != by_bucket.len()
    {
        return Err(corrupt("source pack and posting bucket inventories differ"));
    }
    for ((pack, path), content) in root
        .packs
        .iter()
        .zip(&root.path_postings)
        .zip(&root.content_postings)
    {
        if pack.prefix != path.prefix || pack.prefix != content.prefix {
            return Err(corrupt("source and posting bucket prefixes differ"));
        }
        let bucket_rows = by_bucket
            .get(&pack.prefix[0])
            .ok_or_else(|| corrupt("missing source bucket"))?;
        let mut scratch = Scratch {
            bytes: 0,
            ceiling: policy.max_bucket_scratch_bytes,
        };
        scratch.charge(pack.bytes)?;
        scratch.charge(path.bytes)?;
        scratch.charge(content.bytes)?;
        let source_bytes = blob(pack, &mut read_blob)?;
        let source_view = decode_source_pack(&source_bytes, codec_limits)
            .map_err(|error| corrupt(&format!("source pack decode: {error:?}")))?;
        let mut expected_digests = BTreeMap::new();
        for row in bucket_rows {
            if let Some(previous_length) =
                expected_digests.insert(row.source.source_sha256, row.source_bytes)
            {
                if previous_length != row.source_bytes {
                    return Err(corrupt("same digest has different source length"));
                }
            }
        }
        if source_view.entries().len() != expected_digests.len() {
            return Err(corrupt("source pack digest inventory differs"));
        }
        for (digest, bytes) in source_view.entries() {
            let expected_length = expected_digests
                .get(&digest)
                .ok_or_else(|| corrupt("unreferenced packed source"))?;
            if u64::try_from(bytes.len()).map_err(|_| corrupt("packed source length overflow"))?
                != *expected_length
            {
                return Err(corrupt("packed source length differs from root"));
            }
        }
        let mut expected_path = Expected::new();
        let mut expected_content = Expected::new();
        for row in bucket_rows {
            let source = source_view
                .get(&row.source.source_sha256)
                .ok_or_else(|| corrupt("missing packed source"))?;
            let raw = if row.text_admitted {
                Some(
                    std::str::from_utf8(source)
                        .map_err(|_| corrupt("text-admitted source is not UTF-8"))?,
                )
            } else {
                None
            };
            let (indexed_path, folded_path, indexed_text, folded_text) =
                super::normalized_surfaces(row.source.file.repo_relative_path.as_str(), raw);
            let file_charge = resident_file_charge(
                &row.source,
                &row.language,
                source.len(),
                indexed_path.len(),
                folded_path.len(),
                indexed_text.as_ref().map_or(0, String::len),
                folded_text.as_ref().map_or(0, String::len),
            )?;
            if file_charge != row.resident_heap_bytes {
                return Err(corrupt("resident row charge differs from source bytes"));
            }
            resident_charge = resident_charge
                .checked_add(file_charge)
                .ok_or_else(|| corrupt("resident heap charge overflow"))?;
            if resident_charge > policy.max_resident_file_heap_bytes {
                return Err(corrupt("resident heap exceeds policy"));
            }
            let path_count = add_source(
                folded_path.as_bytes(),
                row.source_id,
                &mut expected_path,
                &mut scratch,
            )?;
            let content_count = if let Some(folded) = folded_text.as_ref() {
                add_source(
                    folded.as_bytes(),
                    row.source_id,
                    &mut expected_content,
                    &mut scratch,
                )?
            } else {
                0
            };
            let total = path_count
                .checked_add(content_count)
                .ok_or_else(|| corrupt("source membership overflow"))?;
            if total != row.posting_memberships {
                return Err(corrupt("source posting count differs from bytes"));
            }
            files.push(SourceFile {
                source: row.source.clone(),
                bytes: source.to_vec(),
                text_admitted: row.text_admitted,
                language: row.language.clone(),
                indexed_text,
                folded_text,
                indexed_path,
                folded_path,
                expected_postings: row.posting_memberships,
            });
        }
        let path_bytes = blob(path, &mut read_blob)?;
        let path_view = decode_posting_block(&path_bytes, PostingSurface::Path, codec_limits)
            .map_err(|error| corrupt(&format!("path posting decode: {error:?}")))?;
        compare_block(path, &path_view, &expected_path)?;
        append_directory(
            &mut posting_directory.path,
            path,
            &path_view,
            &mut directory_charge,
            policy,
        )?;
        let content_bytes = blob(content, &mut read_blob)?;
        let content_view =
            decode_posting_block(&content_bytes, PostingSurface::Content, codec_limits)
                .map_err(|error| corrupt(&format!("content posting decode: {error:?}")))?;
        compare_block(content, &content_view, &expected_content)?;
        append_directory(
            &mut posting_directory.content,
            content,
            &content_view,
            &mut directory_charge,
            policy,
        )?;
    }
    if directory_charge != root.term_directory_charge(policy)? {
        return Err(corrupt("term directory charge differs from root"));
    }
    Ok(VerifiedAuthority {
        root,
        files,
        posting_directory,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision,
    };
    use sha2::{Digest as _, Sha256};

    use super::super::codec::{
        CodecLimits, PostingInput, PostingSurface, SourcePackInput, encode_posting_block,
        encode_source_pack,
    };
    use super::super::root::{
        AuthorityPolicy, AuthorityRoot, Partition, SourceRow, source_key_digest,
    };
    use super::verify_authority;

    fn policy() -> AuthorityPolicy {
        AuthorityPolicy {
            max_root_bytes: 16 * 1024 * 1024,
            max_source_files: 4,
            max_source_bytes: 1024,
            max_pack_bytes: 1024,
            max_total_pack_bytes: 1024,
            max_posting_block_bytes: 1024,
            max_total_posting_bytes: 2048,
            max_total_memberships: 8,
            max_partitions: 4,
            max_source_id: 8,
            max_bucket_scratch_bytes: 8192,
            max_term_directory_bytes: 4096,
            max_resident_file_heap_bytes: 8192,
            max_query_list_reads: 512,
            max_query_posting_ids: 1024,
            max_query_decoded_bytes: 2048,
            max_query_decoded_ids: 10,
        }
    }

    fn codec_limits() -> CodecLimits {
        CodecLimits {
            max_source_pack_encoded_bytes: 1024,
            max_posting_block_encoded_bytes: 1024,
            max_sources: 4,
            max_terms: 4,
            max_memberships: 8,
        }
    }

    fn descriptor(prefix: u8, bytes: &[u8], entries: u64) -> Partition {
        Partition {
            prefix_bits: 8,
            prefix: std::array::from_fn(|index| if index == 0 { prefix } else { 0 }),
            sha256: Sha256::digest(bytes).into(),
            bytes: u64::try_from(bytes.len()).expect("length"),
            entries,
            terms: if entries == 0 { 0 } else { 1 },
        }
    }

    fn fixture() -> (AuthorityRoot, BTreeMap<[u8; 32], Vec<u8>>) {
        let body = b"abc";
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("a.rs"),
            },
            revision_id: RevisionId::new("rev").expect("revision"),
            source_sha256: Sha256::digest(body).into(),
        };
        let prefix = source_key_digest(&source).expect("key digest")[0];
        let ids = [1_u64];
        let source_pack = encode_source_pack(
            &[SourcePackInput {
                digest: source.source_sha256,
                bytes: body,
            }],
            &codec_limits(),
        )
        .expect("pack");
        // Independent fixed census: folded path `a.rs` has `.rs`,`a.r`;
        // folded content `abc` has `abc`. Each appears for source ID 1.
        let path = encode_posting_block(
            PostingSurface::Path,
            &[
                PostingInput {
                    gram: *b".rs",
                    source_ids: &ids,
                },
                PostingInput {
                    gram: *b"a.r",
                    source_ids: &ids,
                },
            ],
            &codec_limits(),
        )
        .expect("path block");
        let content = encode_posting_block(
            PostingSurface::Content,
            &[PostingInput {
                gram: *b"abc",
                source_ids: &ids,
            }],
            &codec_limits(),
        )
        .expect("content block");
        let pack = Partition {
            terms: 0,
            ..descriptor(prefix, &source_pack, 1)
        };
        let path_descriptor = Partition {
            terms: 2,
            ..descriptor(prefix, &path, 2)
        };
        let content_descriptor = descriptor(prefix, &content, 1);
        let root = AuthorityRoot {
            policy_sha256: policy().digest(),
            next_source_id: 2,
            sources: vec![SourceRow {
                source,
                text_admitted: true,
                language: LanguageCode::new("rust").expect("language"),
                posting_memberships: 3,
                source_bytes: 3,
                resident_heap_bytes: 1103,
                source_id: 1,
                pack_sha256: pack.sha256,
            }],
            packs: vec![pack],
            path_postings: vec![path_descriptor],
            content_postings: vec![content_descriptor],
        };
        let blobs = [source_pack, path, content]
            .into_iter()
            .map(|bytes| (Sha256::digest(&bytes).into(), bytes))
            .collect();
        (root, blobs)
    }

    #[test]
    fn fixed_three_term_census_and_retired_id_mutant() {
        let (root, blobs) = fixture();
        let root_bytes = root.encode(policy()).expect("root");
        let opened = verify_authority(&root_bytes, policy(), &codec_limits(), |sha, _expected| {
            blobs
                .get(&sha)
                .cloned()
                .ok_or_else(|| "missing blob".to_string())
        })
        .expect("fixed corpus opens");
        assert_eq!(opened.root.sources[0].posting_memberships, 3);
        assert_eq!(opened.files.len(), 1);

        let mut forged_charge = root.clone();
        forged_charge.sources[0].resident_heap_bytes += 1;
        let forged_charge_root = forged_charge.encode(policy()).expect("structural root");
        let failure = verify_authority(
            &forged_charge_root,
            policy(),
            &codec_limits(),
            |sha, _expected| {
                blobs
                    .get(&sha)
                    .cloned()
                    .ok_or_else(|| "missing blob".to_string())
            },
        )
        .expect_err("forged resident charge must fail independent source census");
        assert!(failure.contains("resident row charge differs"), "{failure}");

        let mut forged_terms = root.clone();
        forged_terms.path_postings[0].terms = 1;
        let forged_term_root = forged_terms.encode(policy()).expect("structural root");
        let failure = verify_authority(
            &forged_term_root,
            policy(),
            &codec_limits(),
            |sha, _expected| {
                blobs
                    .get(&sha)
                    .cloned()
                    .ok_or_else(|| "missing blob".to_string())
            },
        )
        .expect_err("forged term count must fail cold descriptor census");
        assert!(failure.contains("posting term count differs"), "{failure}");

        let mut forged = root;
        let ids = [2_u64];
        let retired = encode_posting_block(
            PostingSurface::Content,
            &[PostingInput {
                gram: *b"abc",
                source_ids: &ids,
            }],
            &codec_limits(),
        )
        .expect("retired block");
        forged.content_postings[0] = descriptor(forged.content_postings[0].prefix[0], &retired, 1);
        let mut forged_blobs = blobs;
        let _prior = forged_blobs.insert(Sha256::digest(&retired).into(), retired);
        let forged_root = forged
            .encode(policy())
            .expect("self-consistent root metadata");
        let failure =
            verify_authority(&forged_root, policy(), &codec_limits(), |sha, _expected| {
                forged_blobs
                    .get(&sha)
                    .cloned()
                    .ok_or_else(|| "missing blob".to_string())
            })
            .expect_err("retired ID must fail independent source census");
        assert!(failure.contains("live IDs differ"), "{failure}");
    }
}
