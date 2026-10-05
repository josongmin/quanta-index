//! Build one canonical v15 file authority without retokenizing inherited rows.
//!
//! The caller supplies a sealed committed base root/blob reader and publishes the
//! returned immutable bytes atomically. No filesystem writes occur here.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{SourceFileKey, SourceFileRevision};
use quanta_index_lq_trigram::trigrams_of;
use sha2::{Digest as _, Sha256};

use super::codec::{
    CodecError, CodecLimits, PostingInput, PostingSurface, SourcePackInput, decode_posting_block,
    decode_source_pack, encode_posting_block, encode_source_pack,
};
use super::root::{
    AuthorityPolicy, AuthorityRoot, Partition, SourceRow, resident_file_charge, source_key_digest,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProducerErrorKind {
    Invalid,
    CorruptBase,
    Limit,
}

#[derive(Debug)]
pub(super) struct ProducerError {
    pub kind: ProducerErrorKind,
    pub reason: String,
}

impl ProducerError {
    fn invalid(reason: impl Into<String>) -> Self {
        Self {
            kind: ProducerErrorKind::Invalid,
            reason: reason.into(),
        }
    }
    fn corrupt(reason: impl Into<String>) -> Self {
        Self {
            kind: ProducerErrorKind::CorruptBase,
            reason: reason.into(),
        }
    }
    fn limit(reason: impl Into<String>) -> Self {
        Self {
            kind: ProducerErrorKind::Limit,
            reason: reason.into(),
        }
    }
}

pub(super) enum SourceDisposition<'a> {
    /// Must match source revision, admission and language in the sealed committed base.
    /// This row retains its existing ID and posting blobs.
    Inherited {
        source: SourceFileRevision,
        text_admitted: bool,
        language: LanguageCode,
    },
    /// Replaces or introduces a source. Its bytes and folded surfaces are
    /// verified and tokenized once, even when the source key already existed.
    Updated {
        source: SourceFileRevision,
        bytes: &'a [u8],
        text_admitted: bool,
        language: LanguageCode,
    },
}

impl SourceDisposition<'_> {
    fn source(&self) -> &SourceFileRevision {
        match self {
            Self::Inherited { source, .. } | Self::Updated { source, .. } => source,
        }
    }
}

pub(super) struct CommittedBase<'a> {
    pub root: &'a AuthorityRoot,
    /// Must stat against `expected_len` before a bounded read from a sealed,
    /// immutable base blob. No producer fallback to an unverified path.
    pub read_blob: &'a dyn Fn([u8; 32], u64) -> Result<Vec<u8>, String>,
}

pub(super) struct ProducedAuthority {
    pub root: AuthorityRoot,
    pub root_bytes: Vec<u8>,
}

type BlobSink<'a> = dyn FnMut([u8; 32], &[u8]) -> Result<(), String> + 'a;
#[derive(Default)]
struct PackGroup {
    digests: BTreeSet<[u8; 32]>,
    indices: Vec<usize>,
}

struct SourceRows<'a> {
    rows: Vec<SourceRow>,
    changed_bytes: BTreeMap<[u8; 32], &'a [u8]>,
    updates: BTreeMap<u64, UpdatedInput<'a>>,
    touched_ids: BTreeSet<u64>,
    next_id: u64,
}

struct UpdatedInput<'a> {
    source_id: u64,
    row_index: usize,
    bytes: &'a [u8],
    text_admitted: bool,
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn prefix(key: &[u8; 32], bits: u16) -> Result<[u8; 32], ProducerError> {
    let mut result = [0_u8; 32];
    let full = usize::from(bits).div_euclid(8);
    let target = result
        .get_mut(..full)
        .ok_or_else(|| ProducerError::invalid("partition prefix exceeds digest width"))?;
    let source = key
        .get(..full)
        .ok_or_else(|| ProducerError::invalid("partition key exceeds digest width"))?;
    target.copy_from_slice(source);
    Ok(result)
}

fn codec_limits(policy: AuthorityPolicy) -> Result<CodecLimits, ProducerError> {
    Ok(CodecLimits {
        source_pack_encoded_bytes: usize::try_from(policy.pack_bytes).map_err(|error| {
            ProducerError::limit(format!("pack byte policy exceeds usize: {error}"))
        })?,
        posting_block_encoded_bytes: usize::try_from(policy.posting_block_bytes).map_err(
            |error| ProducerError::limit(format!("posting byte policy exceeds usize: {error}")),
        )?,
        sources: usize::try_from(policy.source_files).map_err(|error| {
            ProducerError::limit(format!("source count policy exceeds usize: {error}"))
        })?,
        // A three-byte gram has only 2^24 possible values.
        terms: 1 << 24,
        memberships: policy.total_memberships,
    })
}

fn codec_input(error: CodecError) -> ProducerError {
    match error {
        CodecError::Limit(reason) => ProducerError::limit(format!("codec: {reason}")),
        error @ (CodecError::Invalid(_) | CodecError::Corrupt(_) | CodecError::DigestMismatch) => {
            ProducerError::invalid(format!("codec: {error:?}"))
        }
    }
}

fn codec_base(error: CodecError) -> ProducerError {
    ProducerError::corrupt(format!("base codec: {error:?}"))
}

fn base_blob(base: &CommittedBase<'_>, partition: &Partition) -> Result<Vec<u8>, ProducerError> {
    let bytes =
        (base.read_blob)(partition.sha256, partition.bytes).map_err(ProducerError::corrupt)?;
    let observed_len = u64::try_from(bytes.len())
        .map_err(|error| ProducerError::limit(format!("base blob length exceeds u64: {error}")))?;
    if observed_len != partition.bytes || digest(&bytes) != partition.sha256 {
        return Err(ProducerError::corrupt("base blob SHA-256 differs"));
    }
    Ok(bytes)
}

/// Write or byte-compare a content-addressed object.
///
/// The sink may leave an unreferenced orphan on later failure. Only the caller
/// can publish the root after the sink's durability barrier.
fn emit_blob(
    emitted: &mut BTreeMap<[u8; 32], u64>,
    sink: &mut BlobSink<'_>,
    bytes: &[u8],
) -> Result<[u8; 32], ProducerError> {
    let sha256 = digest(bytes);
    let length = u64::try_from(bytes.len())
        .map_err(|error| ProducerError::limit(format!("emitted blob length overflow: {error}")))?;
    if emitted
        .get(&sha256)
        .is_some_and(|previous| *previous != length)
    {
        return Err(ProducerError::corrupt(
            "content-address collision length differs",
        ));
    }
    // Always call the sink, including duplicates such as empty content blocks.
    // Its read-after-write equality check handles same-length SHA collisions.
    sink(sha256, bytes).map_err(ProducerError::corrupt)?;
    if emitted
        .insert(sha256, length)
        .is_some_and(|prior| prior != length)
    {
        return Err(ProducerError::corrupt(
            "content-address collision length changed after sink",
        ));
    }
    Ok(sha256)
}

fn normalized_updated(
    source: &SourceFileRevision,
    language: &LanguageCode,
    bytes: &[u8],
    text_admitted: bool,
) -> Result<(String, Option<String>, u64), ProducerError> {
    // source_rows already verified the exact bytes and 8 MiB bound.
    let raw = if text_admitted {
        Some(std::str::from_utf8(bytes).map_err(|error| {
            ProducerError::invalid(format!("text-admitted source is not UTF-8: {error}"))
        })?)
    } else {
        None
    };
    let (indexed_path, folded_path, indexed_text, folded_content) =
        super::normalized_surfaces(source.file.repo_relative_path.as_str(), raw);
    let resident_charge = resident_file_charge(
        source,
        language,
        bytes.len(),
        indexed_path.len(),
        folded_path.len(),
        indexed_text.as_ref().map_or(0, String::len),
        folded_content.as_ref().map_or(0, String::len),
    )
    .map_err(ProducerError::limit)?;
    Ok((folded_path, folded_content, resident_charge))
}

fn checked_descriptor(
    bits: u16,
    bucket: [u8; 32],
    sha256: [u8; 32],
    bytes: usize,
    entries: u64,
    terms: u32,
) -> Result<Partition, ProducerError> {
    Ok(Partition {
        prefix_bits: bits,
        prefix: bucket,
        sha256,
        bytes: u64::try_from(bytes)
            .map_err(|error| ProducerError::limit(format!("blob bytes overflow: {error}")))?,
        entries,
        terms,
    })
}

fn source_rows<'a>(
    current: &[SourceDisposition<'a>],
    base: Option<&CommittedBase<'_>>,
    policy: AuthorityPolicy,
) -> Result<SourceRows<'a>, ProducerError> {
    if u64::try_from(current.len())
        .map_err(|error| ProducerError::limit(format!("source count overflow: {error}")))?
        > policy.source_files
    {
        return Err(ProducerError::limit("source count exceeds policy"));
    }
    let base_by_key: BTreeMap<SourceFileKey, &SourceRow> = base
        .map(|base| {
            base.root
                .sources
                .iter()
                .map(|row| (row.source.file.clone(), row))
                .collect()
        })
        .unwrap_or_default();
    let mut next_id = base.map_or(1, |base| base.root.next_source_id);
    let mut rows = Vec::new();
    rows.try_reserve_exact(current.len())
        .map_err(|error| ProducerError::limit(format!("source row allocation: {error}")))?;
    let mut changed_bytes = BTreeMap::new();
    let mut updates = BTreeMap::new();
    let mut touched_ids = BTreeSet::new();
    let mut prior: Option<&SourceFileKey> = None;
    let mut total_bytes = 0_u64;
    let mut total_memberships = 0_u64;
    for (row_index, disposition) in current.iter().enumerate() {
        let source = disposition.source();
        source.validate().map_err(ProducerError::invalid)?;
        if prior.is_some_and(|key| key >= &source.file) {
            return Err(ProducerError::invalid(
                "current source keys are not strictly sorted",
            ));
        }
        prior = Some(&source.file);
        let old = base_by_key.get(&source.file).copied();
        let source_id = if let Some(old) = old {
            old.source_id
        } else {
            let id = next_id;
            next_id = next_id
                .checked_add(1)
                .ok_or_else(|| ProducerError::limit("source ID watermark overflow"))?;
            id
        };
        if source_id == 0 || next_id > policy.source_id {
            return Err(ProducerError::limit("source ID exceeds policy"));
        }
        let row = match disposition {
            SourceDisposition::Inherited {
                text_admitted,
                language,
                ..
            } => {
                let old =
                    old.ok_or_else(|| ProducerError::invalid("inherited source absent from base"))?;
                if old.source != *source
                    || old.text_admitted != *text_admitted
                    || old.language != *language
                {
                    return Err(ProducerError::invalid(
                        "inherited source revision, admission or language differs from base",
                    ));
                }
                (*old).clone()
            }
            SourceDisposition::Updated {
                bytes,
                text_admitted,
                language,
                ..
            } => {
                if bytes.len() > 8 * 1024 * 1024 || digest(bytes) != source.source_sha256 {
                    return Err(ProducerError::invalid(
                        "updated source byte limit or digest differs",
                    ));
                }
                if let Some(previous) = changed_bytes.insert(source.source_sha256, *bytes)
                    && previous != *bytes
                {
                    return Err(ProducerError::invalid(
                        "one digest has different source bytes",
                    ));
                }
                if !touched_ids.insert(source_id) {
                    return Err(ProducerError::invalid("updated source ID is duplicated"));
                }
                if updates
                    .insert(
                        source_id,
                        UpdatedInput {
                            source_id,
                            row_index,
                            bytes,
                            text_admitted: *text_admitted,
                        },
                    )
                    .is_some()
                {
                    return Err(ProducerError::invalid("updated source ID is duplicated"));
                }
                SourceRow {
                    source: source.clone(),
                    text_admitted: *text_admitted,
                    language: language.clone(),
                    posting_memberships: 0, // Filled after bucket-local tokenization.
                    source_bytes: u64::try_from(bytes.len()).map_err(|error| {
                        ProducerError::limit(format!("source byte count overflow: {error}"))
                    })?,
                    resident_heap_bytes: 0, // Filled at bucket-local normalization.
                    source_id,
                    pack_sha256: [0; 32], // Filled after pack partitioning.
                }
            }
        };
        total_bytes = total_bytes
            .checked_add(row.source_bytes)
            .ok_or_else(|| ProducerError::limit("aggregate source bytes overflow"))?;
        total_memberships = total_memberships
            .checked_add(u64::from(row.posting_memberships))
            .ok_or_else(|| ProducerError::limit("aggregate memberships overflow"))?;
        if total_bytes > policy.source_bytes || total_memberships > policy.total_memberships {
            return Err(ProducerError::limit(
                "aggregate source or membership policy exceeded",
            ));
        }
        rows.push(row);
    }
    let current_ids: BTreeSet<u64> = rows.iter().map(|row| row.source_id).collect();
    for old in base_by_key.values() {
        if !current_ids.contains(&old.source_id) && !touched_ids.insert(old.source_id) {
            return Err(ProducerError::corrupt("retired source ID is duplicated"));
        }
    }
    Ok(SourceRows {
        rows,
        changed_bytes,
        updates,
        touched_ids,
        next_id,
    })
}

fn produce_packs(
    rows: &mut [SourceRow],
    changed_bytes: &BTreeMap<[u8; 32], &[u8]>,
    base: Option<&CommittedBase<'_>>,
    bits: u16,
    policy: AuthorityPolicy,
    limits: &CodecLimits,
    emitted: &mut BTreeMap<[u8; 32], u64>,
    sink: &mut BlobSink<'_>,
) -> Result<Vec<Partition>, ProducerError> {
    let mut groups: BTreeMap<[u8; 32], PackGroup> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let key = source_key_digest(&row.source).map_err(ProducerError::invalid)?;
        let group = groups.entry(prefix(&key, bits)?).or_default();
        // Identical source bytes under distinct source keys share one pack body.
        let _new_body = group.digests.insert(row.source.source_sha256);
        group.indices.push(index);
    }
    let mut base_groups: BTreeMap<[u8; 32], BTreeSet<[u8; 32]>> = BTreeMap::new();
    for old in base.into_iter().flat_map(|base| base.root.sources.iter()) {
        let key = source_key_digest(&old.source).map_err(ProducerError::corrupt)?;
        let _new_body = base_groups
            .entry(prefix(&key, bits)?)
            .or_default()
            .insert(old.source.source_sha256);
    }
    let base_partitions: BTreeMap<[u8; 32], &Partition> = base
        .into_iter()
        .flat_map(|base| base.root.packs.iter())
        .map(|row| (row.prefix, row))
        .collect();
    let mut partitions = Vec::new();
    let mut total_bytes = 0_u64;
    for (bucket, PackGroup { digests, indices }) in groups {
        let old_partition = base_partitions.get(&bucket).copied();
        let old_digests = base_groups.get(&bucket);
        let descriptor = if old_partition.is_some() && old_digests == Some(&digests) {
            (*old_partition
                .ok_or_else(|| ProducerError::corrupt("unchanged pack descriptor is missing"))?)
            .clone()
        } else {
            let mut source_bytes: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();
            if let (Some(base), Some(prior)) = (base, old_partition) {
                let map_overhead = prior
                    .entries
                    .checked_mul(128)
                    .ok_or_else(|| ProducerError::limit("base pack map charge overflow"))?;
                if prior
                    .bytes
                    .checked_mul(2)
                    .and_then(|bytes| bytes.checked_add(map_overhead))
                    .is_none_or(|peak| peak > policy.bucket_scratch_bytes)
                {
                    return Err(ProducerError::limit(
                        "base pack decode exceeds bucket scratch ceiling",
                    ));
                }
                let bytes = base_blob(base, prior)?;
                let view = decode_source_pack(&bytes, limits).map_err(codec_base)?;
                for (digest, body) in view.entries() {
                    if source_bytes.insert(digest, body.to_vec()).is_some() {
                        return Err(ProducerError::corrupt(
                            "base pack body digest is duplicated",
                        ));
                    }
                }
                drop(view);
                drop(bytes);
            }
            let missing = digests
                .iter()
                .filter(|digest| {
                    !changed_bytes.contains_key(*digest) && !source_bytes.contains_key(*digest)
                })
                .count();
            if missing != 0 {
                return Err(ProducerError::corrupt(
                    "inherited source digest absent from base pack",
                ));
            }
            let input: Vec<SourcePackInput<'_>> = digests
                .iter()
                .map(|digest| {
                    let bytes: &[u8] = changed_bytes
                        .get(digest)
                        .copied()
                        .or_else(|| source_bytes.get(digest).map(Vec::as_slice))
                        .ok_or_else(|| {
                            ProducerError::corrupt("pack body is missing after inventory check")
                        })?;
                    Ok(SourcePackInput {
                        digest: *digest,
                        bytes,
                    })
                })
                .collect::<Result<_, ProducerError>>()?;
            let cloned_body_bytes = source_bytes.values().try_fold(0_u64, |sum, body| {
                sum.checked_add(u64::try_from(body.len()).map_err(|error| {
                    ProducerError::limit(format!("cloned body length overflow: {error}"))
                })?)
                .ok_or_else(|| ProducerError::limit("cloned body sum overflow"))
            })?;
            let map_overhead = u64::try_from(source_bytes.len())
                .map_err(|error| ProducerError::limit(format!("pack map size overflow: {error}")))?
                .checked_mul(128)
                .ok_or_else(|| ProducerError::limit("pack map charge overflow"))?;
            // Codec V1: 24-byte header and 44-byte table row. Calculate the
            // output allocation before encode while cloned base bodies live.
            let projected_len = input.iter().try_fold(24_u64, |sum, source| {
                let body_len = u64::try_from(source.bytes.len()).map_err(|error| {
                    ProducerError::limit(format!("pack projected body length: {error}"))
                })?;
                sum.checked_add(44)
                    .and_then(|sum| sum.checked_add(body_len))
                    .ok_or_else(|| ProducerError::limit("pack projected length overflow"))
            })?;
            if cloned_body_bytes
                .checked_add(map_overhead)
                .and_then(|peak| peak.checked_add(projected_len))
                .is_none_or(|peak| peak > policy.bucket_scratch_bytes)
            {
                return Err(ProducerError::limit(
                    "pack encode exceeds bucket scratch ceiling",
                ));
            }
            let encoded = encode_source_pack(&input, limits).map_err(codec_input)?;
            let len = encoded.len();
            if u64::try_from(len)
                .map_err(|error| ProducerError::limit(format!("encoded pack length: {error}")))?
                != projected_len
            {
                return Err(ProducerError::corrupt(
                    "pack encoded length differs from projection",
                ));
            }
            let sha256 = emit_blob(emitted, sink, &encoded)?;
            checked_descriptor(
                bits,
                bucket,
                sha256,
                len,
                u64::try_from(digests.len())
                    .map_err(|error| ProducerError::limit(format!("pack entry count: {error}")))?,
                0,
            )?
        };
        for index in indices {
            let row = rows
                .get_mut(index)
                .ok_or_else(|| ProducerError::invalid("pack row index exceeds source rows"))?;
            row.pack_sha256 = descriptor.sha256;
        }
        total_bytes = total_bytes
            .checked_add(descriptor.bytes)
            .ok_or_else(|| ProducerError::limit("aggregate pack bytes overflow"))?;
        if total_bytes > policy.total_pack_bytes {
            return Err(ProducerError::limit("aggregate pack bytes exceed policy"));
        }
        partitions.push(descriptor);
    }
    Ok(partitions)
}

const SCRATCH_BITMAP_BYTES: usize = 2 * 1024 * 1024;
const SCRATCH_TERM_BYTES: usize = 256;
const SCRATCH_MEMBERSHIP_BYTES: usize = 128;

fn charge(scratch: &mut usize, additional: usize, maximum: usize) -> Result<(), ProducerError> {
    *scratch = scratch
        .checked_add(additional)
        .ok_or_else(|| ProducerError::limit("posting bucket scratch overflow"))?;
    if *scratch > maximum {
        return Err(ProducerError::limit(
            "posting bucket scratch ceiling exceeded",
        ));
    }
    Ok(())
}

fn insert_membership(
    postings: &mut BTreeMap<[u8; 3], BTreeSet<u64>>,
    gram: [u8; 3],
    id: u64,
    scratch: &mut usize,
    max_scratch: usize,
) -> Result<bool, ProducerError> {
    let new_term = !postings.contains_key(&gram);
    let inserted = postings.entry(gram).or_default().insert(id);
    if inserted {
        charge(scratch, SCRATCH_MEMBERSHIP_BYTES, max_scratch)?;
        if new_term {
            charge(scratch, SCRATCH_TERM_BYTES, max_scratch)?;
        }
    }
    Ok(inserted)
}

fn add_surface(
    bytes: &[u8],
    id: u64,
    bitmap: &mut [u8],
    postings: &mut BTreeMap<[u8; 3], BTreeSet<u64>>,
    scratch: &mut usize,
    max_scratch: usize,
) -> Result<usize, ProducerError> {
    bitmap.fill(0);
    let mut count = 0_usize;
    for gram in trigrams_of(bytes) {
        let key = (usize::from(gram[0]) << 16) | (usize::from(gram[1]) << 8) | usize::from(gram[2]);
        let slot = bitmap
            .get_mut(key >> 3)
            .ok_or_else(|| ProducerError::invalid("trigram bitmap index overflow"))?;
        let mask = 1_u8 << (key & 7);
        if *slot & mask != 0 {
            continue;
        }
        *slot |= mask;
        if insert_membership(postings, gram, id, scratch, max_scratch)? {
            count = count
                .checked_add(1)
                .ok_or_else(|| ProducerError::limit("source membership count overflow"))?;
        }
    }
    Ok(count)
}

fn load_old_postings(
    base: Option<&CommittedBase<'_>>,
    partition: Option<&Partition>,
    surface: PostingSurface,
    touched_ids: &BTreeSet<u64>,
    limits: &CodecLimits,
    scratch: &mut usize,
    max_scratch: usize,
) -> Result<BTreeMap<[u8; 3], BTreeSet<u64>>, ProducerError> {
    let mut postings = BTreeMap::new();
    if let (Some(base), Some(partition)) = (base, partition) {
        let expected = usize::try_from(partition.bytes).map_err(|error| {
            ProducerError::limit(format!("base posting bytes exceed usize: {error}"))
        })?;
        if scratch
            .checked_add(expected)
            .is_none_or(|sum| sum > max_scratch)
        {
            return Err(ProducerError::limit(
                "base posting read exceeds bucket scratch ceiling",
            ));
        }
        let bytes = base_blob(base, partition)?;
        charge(scratch, bytes.len(), max_scratch)?;
        let view = decode_posting_block(&bytes, surface, limits).map_err(codec_base)?;
        for (gram, _, ids) in view.iter_terms() {
            for id in ids {
                if !touched_ids.contains(&id)
                    && !insert_membership(&mut postings, gram, id, scratch, max_scratch)?
                {
                    return Err(ProducerError::corrupt(
                        "base posting repeats a source membership",
                    ));
                }
            }
        }
    }
    Ok(postings)
}

struct PostingEncodeContext<'a> {
    bits: u16,
    limits: &'a CodecLimits,
    scratch: &'a mut usize,
    max_scratch: usize,
}

fn encode_postings(
    postings: BTreeMap<[u8; 3], BTreeSet<u64>>,
    surface: PostingSurface,
    bucket: [u8; 32],
    context: &mut PostingEncodeContext<'_>,
    emitted: &mut BTreeMap<[u8; 32], u64>,
    sink: &mut BlobSink<'_>,
) -> Result<Partition, ProducerError> {
    let owned: Vec<([u8; 3], Vec<u64>)> = postings
        .into_iter()
        .map(|(gram, ids)| (gram, ids.into_iter().collect()))
        .collect();
    let input: Vec<PostingInput<'_>> = owned
        .iter()
        .map(|(gram, ids)| PostingInput {
            gram: *gram,
            source_ids: ids,
        })
        .collect();
    let entries = owned.iter().try_fold(0_u64, |total, (_, ids)| {
        total
            .checked_add(u64::try_from(ids.len()).map_err(|error| {
                ProducerError::limit(format!("posting membership conversion: {error}"))
            })?)
            .ok_or_else(|| ProducerError::limit("posting membership sum"))
    })?;
    let encoded = encode_posting_block(surface, &input, context.limits).map_err(codec_input)?;
    charge(context.scratch, encoded.len(), context.max_scratch)?;
    let len = encoded.len();
    let sha256 = emit_blob(emitted, sink, &encoded)?;
    let terms = u32::try_from(owned.len()).map_err(|error| {
        ProducerError::limit(format!("posting term count exceeds u32: {error}"))
    })?;
    checked_descriptor(context.bits, bucket, sha256, len, entries, terms)
}

fn charge_term_directory(
    current: &mut u64,
    path_terms: u64,
    content_terms: u64,
    policy: AuthorityPolicy,
) -> Result<(), ProducerError> {
    let terms = path_terms
        .checked_add(content_terms)
        .ok_or_else(|| ProducerError::limit("posting term count overflow"))?;
    let rows = terms
        .checked_mul(super::root::TERM_DIRECTORY_ROW_CHARGE)
        .ok_or_else(|| ProducerError::limit("term directory row charge overflow"))?;
    let blocks = 2_u64
        .checked_mul(super::root::TERM_DIRECTORY_BLOCK_CHARGE)
        .ok_or_else(|| ProducerError::limit("term directory block charge overflow"))?;
    *current = current
        .checked_add(rows)
        .and_then(|sum| sum.checked_add(blocks))
        .ok_or_else(|| ProducerError::limit("term directory charge overflow"))?;
    if *current > policy.term_directory_bytes {
        return Err(ProducerError::limit("term directory exceeds policy"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PostingBuildContext<'a, 'b> {
    base: Option<&'a CommittedBase<'b>>,
    bits: u16,
    policy: AuthorityPolicy,
    limits: &'a CodecLimits,
    bucket_scratch_bytes: usize,
}

fn produce_posting_buckets(
    rows: &mut [SourceRow],
    updates: &BTreeMap<u64, UpdatedInput<'_>>,
    touched_ids: &BTreeSet<u64>,
    build_limits: PostingBuildContext<'_, '_>,
    emitted: &mut BTreeMap<[u8; 32], u64>,
    sink: &mut BlobSink<'_>,
) -> Result<(Vec<Partition>, Vec<Partition>), ProducerError> {
    let PostingBuildContext {
        base,
        bits,
        policy,
        limits,
        bucket_scratch_bytes,
    } = build_limits;
    if bucket_scratch_bytes <= SCRATCH_BITMAP_BYTES {
        return Err(ProducerError::limit(
            "posting bucket scratch ceiling is too small",
        ));
    }
    let mut groups: BTreeMap<[u8; 32], Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let key = source_key_digest(&row.source).map_err(ProducerError::invalid)?;
        groups.entry(prefix(&key, bits)?).or_default().push(index);
    }
    let mut old_groups: BTreeMap<[u8; 32], BTreeSet<u64>> = BTreeMap::new();
    let mut base_path = BTreeMap::new();
    let mut base_content = BTreeMap::new();
    if let Some(base) = base {
        for row in &base.root.sources {
            let key = source_key_digest(&row.source).map_err(ProducerError::corrupt)?;
            if !old_groups
                .entry(prefix(&key, bits)?)
                .or_default()
                .insert(row.source_id)
            {
                return Err(ProducerError::corrupt("base source ID is duplicated"));
            }
        }
        base_path.extend(base.root.path_postings.iter().map(|row| (row.prefix, row)));
        base_content.extend(
            base.root
                .content_postings
                .iter()
                .map(|row| (row.prefix, row)),
        );
    }
    let mut path_partitions = Vec::new();
    let mut content_partitions = Vec::new();
    let mut total_bytes = 0_u64;
    let mut total_memberships = 0_u64;
    let mut term_directory_charge = 0_u64;
    let mut resident_charge = rows.iter().try_fold(0_u64, |total, row| {
        total
            .checked_add(row.resident_heap_bytes)
            .ok_or_else(|| ProducerError::limit("resident heap charge overflow"))
    })?;
    for (bucket, indices) in groups {
        let current_ids: BTreeSet<u64> = indices
            .iter()
            .map(|index| {
                rows.get(*index)
                    .map(|row| row.source_id)
                    .ok_or_else(|| ProducerError::invalid("posting row index exceeds source rows"))
            })
            .collect::<Result<_, _>>()?;
        let old_ids = old_groups.get(&bucket);
        let old_path = base_path.get(&bucket).copied();
        let old_content = base_content.get(&bucket).copied();
        let unchanged = old_path.is_some()
            && old_content.is_some()
            && old_ids.is_some_and(|ids| ids == &current_ids && ids.is_disjoint(touched_ids));
        let (path, content) = if unchanged {
            let path = (*old_path.ok_or_else(|| {
                ProducerError::corrupt("unchanged path posting descriptor is missing")
            })?)
            .clone();
            let content = (*old_content.ok_or_else(|| {
                ProducerError::corrupt("unchanged content posting descriptor is missing")
            })?)
            .clone();
            charge_term_directory(
                &mut term_directory_charge,
                u64::from(path.terms),
                u64::from(content.terms),
                policy,
            )?;
            if resident_charge
                .checked_add(term_directory_charge)
                .is_none_or(|total| total > policy.resident_file_heap_bytes)
            {
                return Err(ProducerError::limit("resident heap exceeds policy"));
            }
            (path, content)
        } else {
            let mut scratch = SCRATCH_BITMAP_BYTES;
            let mut bitmap = vec![0_u8; SCRATCH_BITMAP_BYTES];
            let mut path = load_old_postings(
                base,
                old_path,
                PostingSurface::Path,
                touched_ids,
                limits,
                &mut scratch,
                bucket_scratch_bytes,
            )?;
            let mut content = load_old_postings(
                base,
                old_content,
                PostingSurface::Content,
                touched_ids,
                limits,
                &mut scratch,
                bucket_scratch_bytes,
            )?;
            for index in indices {
                let row = rows.get_mut(index).ok_or_else(|| {
                    ProducerError::invalid("posting row index exceeds source rows")
                })?;
                if let Some(update) = updates.get(&row.source_id) {
                    if update.row_index != index || update.source_id != row.source_id {
                        return Err(ProducerError::invalid(
                            "updated source row identity differs",
                        ));
                    }
                    let (folded_path, folded_content, file_charge) = normalized_updated(
                        &row.source,
                        &row.language,
                        update.bytes,
                        update.text_admitted,
                    )?;
                    row.resident_heap_bytes = file_charge;
                    resident_charge = resident_charge
                        .checked_add(file_charge)
                        .ok_or_else(|| ProducerError::limit("resident heap charge overflow"))?;
                    if resident_charge
                        .checked_add(term_directory_charge)
                        .is_none_or(|total| total > policy.resident_file_heap_bytes)
                    {
                        return Err(ProducerError::limit("resident heap exceeds policy"));
                    }
                    let temporary = folded_path
                        .len()
                        .checked_add(folded_content.as_ref().map_or(0, String::len))
                        .ok_or_else(|| ProducerError::limit("normalized source bytes overflow"))?;
                    if scratch
                        .checked_add(temporary)
                        .is_none_or(|peak| peak > bucket_scratch_bytes)
                    {
                        return Err(ProducerError::limit(
                            "normalized source scratch ceiling exceeded",
                        ));
                    }
                    let path_count = add_surface(
                        folded_path.as_bytes(),
                        row.source_id,
                        &mut bitmap,
                        &mut path,
                        &mut scratch,
                        bucket_scratch_bytes,
                    )?;
                    let content_count = if let Some(text) = &folded_content {
                        add_surface(
                            text.as_bytes(),
                            row.source_id,
                            &mut bitmap,
                            &mut content,
                            &mut scratch,
                            bucket_scratch_bytes,
                        )?
                    } else {
                        0
                    };
                    row.posting_memberships =
                        u32::try_from(path_count.checked_add(content_count).ok_or_else(|| {
                            ProducerError::limit("source membership sum overflow")
                        })?)
                        .map_err(|error| {
                            ProducerError::limit(format!(
                                "source membership count exceeds u32: {error}"
                            ))
                        })?;
                }
            }
            charge_term_directory(
                &mut term_directory_charge,
                u64::try_from(path.len()).map_err(|error| {
                    ProducerError::limit(format!("path term count exceeds u64: {error}"))
                })?,
                u64::try_from(content.len()).map_err(|error| {
                    ProducerError::limit(format!("content term count exceeds u64: {error}"))
                })?,
                policy,
            )?;
            if resident_charge
                .checked_add(term_directory_charge)
                .is_none_or(|total| total > policy.resident_file_heap_bytes)
            {
                return Err(ProducerError::limit("resident heap exceeds policy"));
            }
            let mut encode_context = PostingEncodeContext {
                bits,
                limits,
                scratch: &mut scratch,
                max_scratch: bucket_scratch_bytes,
            };
            let path = encode_postings(
                path,
                PostingSurface::Path,
                bucket,
                &mut encode_context,
                emitted,
                sink,
            )?;
            let content = encode_postings(
                content,
                PostingSurface::Content,
                bucket,
                &mut encode_context,
                emitted,
                sink,
            )?;
            (path, content)
        };
        for descriptor in [&path, &content] {
            total_bytes = total_bytes
                .checked_add(descriptor.bytes)
                .ok_or_else(|| ProducerError::limit("aggregate posting bytes overflow"))?;
            total_memberships = total_memberships
                .checked_add(descriptor.entries)
                .ok_or_else(|| ProducerError::limit("aggregate posting memberships overflow"))?;
            if total_bytes > policy.total_posting_bytes
                || total_memberships > policy.total_memberships
            {
                return Err(ProducerError::limit("aggregate posting policy exceeded"));
            }
        }
        path_partitions.push(path);
        content_partitions.push(content);
    }
    let source_total = rows.iter().try_fold(0_u64, |sum, row| {
        sum.checked_add(u64::from(row.posting_memberships))
            .ok_or_else(|| ProducerError::limit("source membership sum overflow"))
    })?;
    if source_total != total_memberships || source_total > policy.total_memberships {
        return Err(ProducerError::invalid(
            "source and posting membership totals differ",
        ));
    }
    Ok((path_partitions, content_partitions))
}

/// Build a complete root and only the blobs whose partition content changed.
///
/// `prefix_bits` is fixed for this generation and must match a reused base.
/// Scratch admission is stamped in the root policy, independently of
/// aggregate retained bytes.
pub(super) fn produce_authority(
    current: &[SourceDisposition<'_>],
    base: Option<&CommittedBase<'_>>,
    policy: AuthorityPolicy,
    prefix_bits: u16,
    sink: &mut BlobSink<'_>,
) -> Result<ProducedAuthority, ProducerError> {
    if prefix_bits != 8 {
        return Err(ProducerError::invalid("F15 requires exactly 8 prefix bits"));
    }
    if let Some(base) = base {
        base.root.validate(policy).map_err(ProducerError::corrupt)?;
        for partition in base
            .root
            .packs
            .iter()
            .chain(&base.root.path_postings)
            .chain(&base.root.content_postings)
        {
            if partition.prefix_bits != prefix_bits {
                return Err(ProducerError::invalid(
                    "base partition width differs; rebuild required",
                ));
            }
        }
    }
    let limits = codec_limits(policy)?;
    let bucket_scratch_bytes = usize::try_from(policy.bucket_scratch_bytes).map_err(|error| {
        ProducerError::limit(format!("bucket scratch policy exceeds usize: {error}"))
    })?;
    let SourceRows {
        mut rows,
        changed_bytes,
        updates,
        touched_ids,
        next_id: next_source_id,
    } = source_rows(current, base, policy)?;
    let mut emitted_blobs = BTreeMap::new();
    let packs = produce_packs(
        &mut rows,
        &changed_bytes,
        base,
        prefix_bits,
        policy,
        &limits,
        &mut emitted_blobs,
        sink,
    )?;
    let (path_postings, content_postings) = produce_posting_buckets(
        &mut rows,
        &updates,
        &touched_ids,
        PostingBuildContext {
            base,
            bits: prefix_bits,
            policy,
            limits: &limits,
            bucket_scratch_bytes,
        },
        &mut emitted_blobs,
        sink,
    )?;
    let root = AuthorityRoot {
        policy_sha256: policy.digest(),
        next_source_id,
        sources: rows,
        packs,
        path_postings,
        content_postings,
    };
    let root_bytes = root.encode(policy).map_err(ProducerError::invalid)?;
    Ok(ProducedAuthority { root, root_bytes })
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision,
    };
    use sha2::{Digest as _, Sha256};

    use super::super::root::AuthorityPolicy;
    use super::{
        CommittedBase, ProducerErrorKind, SourceDisposition, emit_blob, produce_authority,
    };

    fn policy() -> AuthorityPolicy {
        AuthorityPolicy {
            root_bytes: 1 << 20,
            source_files: 10,
            source_bytes: 1 << 20,
            pack_bytes: 1 << 20,
            total_pack_bytes: 1 << 20,
            posting_block_bytes: 1 << 20,
            total_posting_bytes: 1 << 20,
            total_memberships: 1000,
            partitions: 256,
            source_id: 100,
            bucket_scratch_bytes: 16 << 20,
            term_directory_bytes: 1 << 20,
            resident_file_heap_bytes: 1 << 20,
            query_list_reads: 512,
            query_posting_ids: 1000,
            query_decoded_bytes: 1 << 20,
            query_decoded_ids: 1000,
        }
    }

    fn source() -> SourceFileRevision {
        SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("a.rs"),
            },
            revision_id: RevisionId::new("rev").expect("revision"),
            source_sha256: Sha256::digest(b"abc").into(),
        }
    }

    fn language(value: &str) -> LanguageCode {
        LanguageCode::new(value).expect("language")
    }

    fn fresh(text_admitted: bool) -> super::ProducedAuthority {
        let mut written = std::collections::BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut sink = |sha, bytes: &[u8]| -> Result<(), String> {
            if let Some(previous) = written.get(&sha) {
                if previous != bytes {
                    return Err("content collision".into());
                }
            } else if written.insert(sha, bytes.to_vec()).is_some() {
                return Err("content digest unexpectedly replaced".into());
            }
            Ok(())
        };
        produce_authority(
            &[SourceDisposition::Updated {
                source: source(),
                bytes: b"abc",
                text_admitted,
                language: language("rust"),
            }],
            None,
            policy(),
            8,
            &mut sink,
        )
        .expect("fresh authority")
    }

    fn inherited(
        base: &super::ProducedAuthority,
        text_admitted: bool,
        language_code: &str,
    ) -> Result<super::ProducedAuthority, super::ProducerError> {
        let no_read = |_, _| -> Result<Vec<u8>, String> { panic!("unchanged base blob was read") };
        let mut no_write =
            |_, _: &[u8]| -> Result<(), String> { panic!("unchanged blob was emitted") };
        produce_authority(
            &[SourceDisposition::Inherited {
                source: source(),
                text_admitted,
                language: language(language_code),
            }],
            Some(&CommittedBase {
                root: &base.root,
                read_blob: &no_read,
            }),
            policy(),
            8,
            &mut no_write,
        )
    }

    #[test]
    fn unchanged_source_keeps_id_and_reuses_all_blobs_without_read() {
        let base = fresh(true);
        assert_eq!(base.root.sources[0].resident_heap_bytes, 1103);
        let next = inherited(&base, true, "rust").expect("unchanged");
        assert_eq!(
            next.root.sources[0].source_id,
            base.root.sources[0].source_id
        );
        assert_eq!(next.root_bytes, base.root_bytes);
        assert_eq!(next.root.sources[0].resident_heap_bytes, 1103);
    }

    #[test]
    fn resident_cap_refuses_updated_source() {
        let mut strict = policy();
        strict.resident_file_heap_bytes = 1102;
        let mut sink = |_, _: &[u8]| Ok(());
        let error = produce_authority(
            &[SourceDisposition::Updated {
                source: source(),
                bytes: b"abc",
                text_admitted: true,
                language: language("rust"),
            }],
            None,
            strict,
            8,
            &mut sink,
        )
        .err()
        .expect("resident cap must refuse");
        assert_eq!(error.kind, ProducerErrorKind::Limit);
    }

    #[test]
    fn inherited_admission_and_language_mutants_refuse_before_reuse() {
        let binary = fresh(false);
        assert_eq!(
            inherited(&binary, true, "rust").err().unwrap().kind,
            ProducerErrorKind::Invalid
        );
        let text = fresh(true);
        assert_eq!(
            inherited(&text, false, "rust").err().unwrap().kind,
            ProducerErrorKind::Invalid
        );
        assert_eq!(
            inherited(&text, true, "python").err().unwrap().kind,
            ProducerErrorKind::Invalid
        );
    }

    #[test]
    fn duplicate_digest_is_offered_twice_to_verifying_sink() {
        let mut inventory = std::collections::BTreeMap::new();
        let mut calls = 0;
        let mut sink = |_sha, bytes: &[u8]| -> Result<(), String> {
            assert_eq!(bytes, b"same empty block");
            calls += 1;
            Ok(())
        };
        let first = emit_blob(&mut inventory, &mut sink, b"same empty block").unwrap();
        let second = emit_blob(&mut inventory, &mut sink, b"same empty block").unwrap();
        assert_eq!(first, second);
        assert_eq!(inventory.len(), 1);
        assert_eq!(calls, 2);
    }

    #[test]
    fn updated_source_reuses_id_and_rebuilds_only_its_bucket() {
        let mut base_blobs = std::collections::BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut base_sink = |sha, bytes: &[u8]| -> Result<(), String> {
            if let Some(previous) = base_blobs.get(&sha) {
                if previous != bytes {
                    return Err("content collision".into());
                }
            } else if base_blobs.insert(sha, bytes.to_vec()).is_some() {
                return Err("base digest unexpectedly replaced".into());
            }
            Ok(())
        };
        let base = produce_authority(
            &[SourceDisposition::Updated {
                source: source(),
                bytes: b"abc",
                text_admitted: true,
                language: language("rust"),
            }],
            None,
            policy(),
            8,
            &mut base_sink,
        )
        .unwrap();
        let mut replacement = source();
        replacement.revision_id = RevisionId::new("rev2").expect("revision");
        replacement.source_sha256 = Sha256::digest(b"abd").into();
        let read = |sha, len| -> Result<Vec<u8>, String> {
            let bytes = base_blobs.get(&sha).ok_or("missing base blob")?;
            if u64::try_from(bytes.len()).map_err(|error| error.to_string())? != len {
                return Err("base length".into());
            }
            Ok(bytes.clone())
        };
        let mut output = std::collections::BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut sink = |sha, bytes: &[u8]| -> Result<(), String> {
            if let Some(previous) = output.get(&sha) {
                if previous != bytes {
                    return Err("content collision".into());
                }
            } else if output.insert(sha, bytes.to_vec()).is_some() {
                return Err("output digest unexpectedly replaced".into());
            }
            Ok(())
        };
        let delta = produce_authority(
            &[SourceDisposition::Updated {
                source: replacement,
                bytes: b"abd",
                text_admitted: true,
                language: language("rust"),
            }],
            Some(&CommittedBase {
                root: &base.root,
                read_blob: &read,
            }),
            policy(),
            8,
            &mut sink,
        )
        .unwrap();
        assert_eq!(
            delta.root.sources[0].source_id,
            base.root.sources[0].source_id
        );
        assert_eq!(delta.root.next_source_id, base.root.next_source_id);
        assert!(!output.is_empty(), "replacement must emit a changed object");
    }

    #[test]
    fn tombstoned_source_id_is_not_reused() {
        let base = fresh(false);
        let no_read = |_, _| -> Result<Vec<u8>, String> { panic!("tombstone read base") };
        let mut no_write = |_, _: &[u8]| -> Result<(), String> { panic!("tombstone wrote blob") };
        let empty = produce_authority(
            &[],
            Some(&CommittedBase {
                root: &base.root,
                read_blob: &no_read,
            }),
            policy(),
            8,
            &mut no_write,
        )
        .unwrap();
        assert!(empty.root.sources.is_empty());
        assert_eq!(empty.root.next_source_id, 2);
        let mut fresh_source = source();
        fresh_source.file.repo_relative_path = RepoRelativePath::new("b.rs");
        let mut sink = |_sha, _bytes: &[u8]| -> Result<(), String> { Ok(()) };
        let next = produce_authority(
            &[SourceDisposition::Updated {
                source: fresh_source,
                bytes: b"abc",
                text_admitted: false,
                language: language("rust"),
            }],
            Some(&CommittedBase {
                root: &empty.root,
                read_blob: &no_read,
            }),
            policy(),
            8,
            &mut sink,
        )
        .unwrap();
        assert_eq!(next.root.sources[0].source_id, 2);
    }
}
