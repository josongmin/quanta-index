//! Fixed-census corruption oracles shared by serving and publication consumers.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};

use super::tests::{codec_limits, fixture, policy};
use super::{AuthorityPolicy, AuthorityRoot, PublicationOutput, ServingOutput, verify_authority};

fn read(blobs: &BTreeMap<[u8; 32], Vec<u8>>, digest: [u8; 32]) -> Result<Vec<u8>, String> {
    blobs
        .get(&digest)
        .cloned()
        .ok_or_else(|| "missing blob".to_string())
}

fn refuse_both(
    bytes: &[u8],
    policy: AuthorityPolicy,
    blobs: &BTreeMap<[u8; 32], Vec<u8>>,
    expected: &str,
) {
    let serving =
        verify_authority::<_, ServingOutput>(bytes, policy, &codec_limits(), |digest, _| {
            read(blobs, digest)
        })
        .expect_err("serving must refuse independent corruption");
    let publication =
        verify_authority::<_, PublicationOutput>(bytes, policy, &codec_limits(), |digest, _| {
            read(blobs, digest)
        })
        .expect_err("publication must refuse independent corruption");
    assert_eq!(publication, serving);
    assert!(
        publication.contains(expected),
        "expected {expected}: {publication}"
    );
}

#[test]
fn publication_verifies_every_object_without_serving_materialization() {
    let (root, blobs) = fixture();
    let bytes = root.encode(policy()).expect("fixture root");
    let before = super::output::materialization_counts();
    let mut serving_reads = Vec::new();
    let serving =
        verify_authority::<_, ServingOutput>(&bytes, policy(), &codec_limits(), |digest, len| {
            serving_reads.push((digest, len));
            read(&blobs, digest)
        })
        .expect("serving fixed census");
    let after_serving = super::output::materialization_counts();
    assert_eq!(
        after_serving,
        (before.0.saturating_add(1), before.1.saturating_add(2))
    );
    let mut publication_reads = Vec::new();
    let publication = verify_authority::<_, PublicationOutput>(
        &bytes,
        policy(),
        &codec_limits(),
        |digest, len| {
            publication_reads.push((digest, len));
            read(&blobs, digest)
        },
    )
    .expect("publication fixed census");
    assert_eq!(serving_reads, publication_reads);
    assert_eq!(publication_reads.len(), 3);
    assert_eq!(super::output::materialization_counts(), after_serving);
    assert_eq!(
        serving.root.encode(policy()).expect("serving root"),
        publication.root.encode(policy()).expect("publication root")
    );
    assert_eq!(serving.output.files.len(), 1);
}

#[test]
fn publication_and_serving_reject_forged_census_charge_and_metadata() {
    let (root, blobs) = fixture();
    for (field, expected) in [
        (0, "resident row charge differs"),
        (1, "source posting count differs"),
        (2, "posting term count differs"),
    ] {
        let mut changed = root.clone();
        match field {
            0 => {
                changed
                    .sources
                    .first_mut()
                    .expect("source")
                    .resident_heap_bytes = 1104;
            }
            1 => {
                changed
                    .sources
                    .first_mut()
                    .expect("source")
                    .posting_memberships = 4;
                changed
                    .path_postings
                    .first_mut()
                    .expect("path partition")
                    .entries = 3;
            }
            _ => {
                changed
                    .path_postings
                    .first_mut()
                    .expect("path partition")
                    .terms = 1;
            }
        }
        let bytes = changed.encode(policy()).expect("structural mutant root");
        refuse_both(&bytes, policy(), &blobs, expected);
    }
    let mut missing = blobs.clone();
    let _removed = missing.remove(&root.content_postings.first().expect("content").sha256);
    refuse_both(
        &root.encode(policy()).expect("root"),
        policy(),
        &missing,
        "missing blob",
    );
    for digest in blobs.keys() {
        let mut changed = blobs.clone();
        *changed
            .get_mut(digest)
            .expect("object")
            .last_mut()
            .expect("nonempty object") ^= 1;
        refuse_both(
            &root.encode(policy()).expect("root"),
            policy(),
            &changed,
            "blob digest differs",
        );
    }
}

fn replace_object(
    root: &mut AuthorityRoot,
    blobs: &mut BTreeMap<[u8; 32], Vec<u8>>,
    content: bool,
    bytes: Vec<u8>,
) {
    let descriptor = if content {
        root.content_postings.first_mut().expect("content")
    } else {
        root.packs.first_mut().expect("pack")
    };
    let _old = blobs.remove(&descriptor.sha256);
    descriptor.sha256 = Sha256::digest(&bytes).into();
    descriptor.bytes = u64::try_from(bytes.len()).expect("object width");
    if !content {
        root.sources.first_mut().expect("source").pack_sha256 = descriptor.sha256;
    }
    let _new = blobs.insert(descriptor.sha256, bytes);
}

#[test]
fn publication_and_serving_decode_rehashed_objects_and_prove_source_membership() {
    use super::super::codec::{
        PostingInput, PostingSurface, SourcePackInput, encode_posting_block, encode_source_pack,
    };
    let (mut root, mut blobs) = fixture();
    let retired = encode_posting_block(
        PostingSurface::Content,
        &[PostingInput {
            gram: *b"abc",
            source_ids: &[2],
        }],
        &codec_limits(),
    )
    .expect("retired ID block");
    replace_object(&mut root, &mut blobs, true, retired);
    refuse_both(
        &root.encode(policy()).expect("forged root"),
        policy(),
        &blobs,
        "live IDs differ",
    );

    let (mut root, mut blobs) = fixture();
    let invalid_utf8 = [0xff, b'b', b'c'];
    let digest = Sha256::digest(invalid_utf8).into();
    let pack = encode_source_pack(
        &[SourcePackInput {
            digest,
            bytes: &invalid_utf8,
        }],
        &codec_limits(),
    )
    .expect("invalid UTF-8 source pack");
    root.sources
        .first_mut()
        .expect("source")
        .source
        .source_sha256 = digest;
    replace_object(&mut root, &mut blobs, false, pack);
    refuse_both(
        &root.encode(policy()).expect("forged root"),
        policy(),
        &blobs,
        "not UTF-8",
    );

    for content in [false, true] {
        let (mut root, mut blobs) = fixture();
        replace_object(&mut root, &mut blobs, content, vec![0; 8]);
        refuse_both(
            &root.encode(policy()).expect("malformed object root"),
            policy(),
            &blobs,
            if content {
                "content posting decode"
            } else {
                "source pack decode"
            },
        );
    }
}

#[test]
fn publication_preserves_policy_refusals_even_without_serving_allocations() {
    let (root, blobs) = fixture();
    let bytes = root.encode(policy()).expect("fixture root");
    for dimension in 0..6 {
        let mut limited = policy();
        match dimension {
            0 => limited.bucket_scratch_bytes = 1,
            1 => limited.resident_file_heap_bytes = 1,
            2 => limited.term_directory_bytes = 1,
            3 => limited.source_bytes = 1,
            4 => limited.total_memberships = 1,
            _ => limited.source_files = 0,
        }
        // Mutate only the canonical policy commitment, without going through
        // the producer's admission, so both readers must enforce the new cap.
        let mut wire: ciborium::Value = ciborium::from_reader(bytes.as_slice()).expect("root wire");
        let ciborium::Value::Array(fields) = &mut wire else {
            panic!("root tuple");
        };
        *fields.get_mut(1).expect("policy field") = ciborium::Value::Array(
            limited
                .digest()
                .into_iter()
                .map(|byte| ciborium::Value::Integer(byte.into()))
                .collect(),
        );
        let mut changed = Vec::new();
        ciborium::into_writer(&wire, &mut changed).expect("mutant root bytes");
        refuse_both(
            &changed,
            limited,
            &blobs,
            if dimension == 0 { "scratch" } else { "policy" },
        );
    }
}
