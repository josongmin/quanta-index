//! Canonical file authority root for packed sources and disk posting shards.
//!
//! Every published root is one immutable policy and one complete source
//! universe. A decoder checks global sums before a query can read a shard.

use std::collections::BTreeSet;

use quanta_index_contract::SourceFileRevision;
use quanta_index_contract::lex::LanguageCode;
use sha2::{Digest as _, Sha256};

use crate::channel_payloads::{decode_cbor_exact, encode_cbor};

pub(super) const FORMAT: u32 = 15;
pub(crate) const DIR_NAME: &str = "file-authority";
pub(crate) const ROOT_FILE_NAME: &str = "root.cbor";
pub(crate) const OBJECTS_NAME: &str = "objects";
pub(super) const PREFIX_BITS: u16 = 8;
pub(super) const TERM_DIRECTORY_PAGE_CHARGE: u64 = 64;
pub(super) const TERM_DIRECTORY_BLOCK_CHARGE: u64 = 128;
pub(super) const RESIDENT_FILE_ROW_CHARGE: u64 = 1024;

/// A directory retains one fence and authenticated table-page digest, not
/// one list digest and offset for every duplicated bucket-local trigram.
pub(super) fn term_directory_block_charge(terms: u64) -> Result<u64, String> {
    terms
        .div_ceil(u64::from(super::codec::DIRECTORY_PAGE_TERMS))
        .checked_mul(TERM_DIRECTORY_PAGE_CHARGE)
        .and_then(|pages| pages.checked_add(TERM_DIRECTORY_BLOCK_CHARGE))
        .ok_or_else(|| invalid("term directory block charge overflow"))
}

/// Canonical object basename accepted by the sealed manifest and inventory.
pub(crate) fn is_object_file_name(name: &str) -> bool {
    let Some(hex) = name.strip_suffix(".bin") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Numeric policy is explicit and stamped in the root. The product owner
/// chooses these ceilings after a full fixture count; a shard-local ceiling
/// never stands in for the aggregate membership limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityPolicy {
    pub root_bytes: u64,
    pub source_files: u64,
    pub source_bytes: u64,
    pub pack_bytes: u64,
    pub total_pack_bytes: u64,
    pub posting_block_bytes: u64,
    pub total_posting_bytes: u64,
    pub total_memberships: u64,
    pub partitions: u64,
    pub source_id: u64,
    pub bucket_scratch_bytes: u64,
    pub term_directory_bytes: u64,
    pub resident_file_heap_bytes: u64,
    pub query_list_reads: u64,
    pub query_posting_ids: u64,
    pub query_decoded_bytes: u64,
    pub query_decoded_ids: u64,
}

impl AuthorityPolicy {
    /// Infallible by construction: hashes a fixed domain and seventeen
    /// fixed-width limits and the directory page width without serialization,
    /// allocation, or I/O.
    /// The digest commits the policy values; it does not validate their limits.
    pub(super) fn digest(self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-file-authority-policy-v15-paged-posting-v2\0");
        hasher.update((u64::from(super::codec::DIRECTORY_PAGE_TERMS)).to_le_bytes());
        for value in [
            self.root_bytes,
            self.source_files,
            self.source_bytes,
            self.pack_bytes,
            self.total_pack_bytes,
            self.posting_block_bytes,
            self.total_posting_bytes,
            self.total_memberships,
            self.partitions,
            self.source_id,
            self.bucket_scratch_bytes,
            self.term_directory_bytes,
            self.resident_file_heap_bytes,
            self.query_list_reads,
            self.query_posting_ids,
            self.query_decoded_bytes,
            self.query_decoded_ids,
        ] {
            hasher.update(value.to_le_bytes());
        }
        hasher.finalize().into()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceRow {
    pub source: SourceFileRevision,
    pub text_admitted: bool,
    pub language: LanguageCode,
    pub posting_memberships: u32,
    pub source_bytes: u64,
    /// Checked logical heap charge for one serving source and its index rows.
    pub resident_heap_bytes: u64,
    pub source_id: u64,
    pub pack_sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Partition {
    pub prefix_bits: u16,
    pub prefix: [u8; 32],
    pub sha256: [u8; 32],
    pub bytes: u64,
    pub entries: u64,
    /// Exact distinct gram rows for posting blocks; zero for source packs.
    pub terms: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityRoot {
    pub policy_sha256: [u8; 32],
    pub next_source_id: u64,
    pub sources: Vec<SourceRow>,
    pub packs: Vec<Partition>,
    pub path_postings: Vec<Partition>,
    pub content_postings: Vec<Partition>,
}

type SourceWire = (
    SourceFileRevision,
    bool,
    LanguageCode,
    u32,
    u64,
    u64,
    u64,
    [u8; 32],
);
type PartitionWire = (u16, [u8; 32], [u8; 32], u64, u64, u32);
type RootWire = (
    u32,
    [u8; 32],
    u64,
    Vec<SourceWire>,
    Vec<PartitionWire>,
    Vec<PartitionWire>,
    Vec<PartitionWire>,
);

fn wire(root: &AuthorityRoot) -> RootWire {
    (
        FORMAT,
        root.policy_sha256,
        root.next_source_id,
        root.sources
            .iter()
            .map(|row| {
                (
                    row.source.clone(),
                    row.text_admitted,
                    row.language.clone(),
                    row.posting_memberships,
                    row.source_bytes,
                    row.resident_heap_bytes,
                    row.source_id,
                    row.pack_sha256,
                )
            })
            .collect(),
        root.packs.iter().map(partition_wire).collect(),
        root.path_postings.iter().map(partition_wire).collect(),
        root.content_postings.iter().map(partition_wire).collect(),
    )
}

fn partition_wire(row: &Partition) -> PartitionWire {
    (
        row.prefix_bits,
        row.prefix,
        row.sha256,
        row.bytes,
        row.entries,
        row.terms,
    )
}

fn from_partition_wire(
    (prefix_bits, prefix, sha256, bytes, entries, terms): PartitionWire,
) -> Partition {
    Partition {
        prefix_bits,
        prefix,
        sha256,
        bytes,
        entries,
        terms,
    }
}

fn invalid(reason: &str) -> String {
    format!("file authority v15: {reason}")
}

fn prefix_canonical(bits: u16, prefix: &[u8; 32]) -> bool {
    if bits > 256 {
        return false;
    }
    let full = usize::from(bits).div_euclid(8);
    let partial = bits % 8;
    if partial != 0 {
        let mask = u8::MAX >> partial;
        if prefix.get(full).is_none_or(|byte| byte & mask != 0) {
            return false;
        }
    }
    let Some(suffix) = full.checked_add(usize::from(partial != 0)) else {
        return false;
    };
    prefix
        .get(suffix..)
        .is_some_and(|remaining| remaining.iter().all(|byte| *byte == 0))
}

pub(super) fn source_key_digest(source: &SourceFileRevision) -> Result<[u8; 32], String> {
    let repo = source.file.source_repo_id.as_str().as_bytes();
    let path = source.file.repo_relative_path.as_str().as_bytes();
    let repo_len = u64::try_from(repo.len())
        .map_err(|error| invalid(&format!("repo key length overflow: {error}")))?;
    let path_len = u64::try_from(path.len())
        .map_err(|error| invalid(&format!("path key length overflow: {error}")))?;
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-authority-source-key-v15\0");
    hasher.update(repo_len.to_le_bytes());
    hasher.update(repo);
    hasher.update(path_len.to_le_bytes());
    hasher.update(path);
    Ok(hasher.finalize().into())
}

/// Logical serving-heap admission, not a physical RSS bound.
///
/// Six key copies account for root, file, ordered and ID maps. The row charge
/// covers container nodes and headers. Producer and cold opener use this function.
pub(super) fn resident_file_charge(
    source: &SourceFileRevision,
    language: &LanguageCode,
    raw_bytes: usize,
    indexed_path_bytes: usize,
    folded_path_bytes: usize,
    indexed_text_bytes: usize,
    folded_text_bytes: usize,
) -> Result<u64, String> {
    let width = |value: usize| {
        u64::try_from(value).map_err(|error| invalid(&format!("resident length width: {error}")))
    };
    let key = source
        .file
        .source_repo_id
        .as_str()
        .len()
        .checked_add(source.file.repo_relative_path.as_str().len())
        .ok_or_else(|| invalid("resident key length overflow"))?;
    let mutable_lengths = [
        raw_bytes,
        indexed_path_bytes,
        folded_path_bytes,
        indexed_text_bytes,
        folded_text_bytes,
    ];
    let mut charge = RESIDENT_FILE_ROW_CHARGE;
    for length in mutable_lengths {
        charge = charge
            .checked_add(width(length)?)
            .ok_or_else(|| invalid("resident surface charge overflow"))?;
    }
    let repeated_key = width(key)?
        .checked_mul(6)
        .ok_or_else(|| invalid("resident key charge overflow"))?;
    let repeated_metadata = width(source.revision_id.as_str().len())?
        .checked_add(width(language.as_str().len())?)
        .and_then(|length| length.checked_mul(2))
        .ok_or_else(|| invalid("resident metadata charge overflow"))?;
    charge
        .checked_add(repeated_key)
        .and_then(|value| value.checked_add(repeated_metadata))
        .ok_or_else(|| invalid("resident row charge overflow"))
}

fn validate_partitions(
    partitions: &[Partition],
    policy: AuthorityPolicy,
    require_entries: bool,
) -> Result<(u64, u64), String> {
    if u64::try_from(partitions.len())
        .map_err(|error| invalid(&format!("partition count overflow: {error}")))?
        > policy.partitions
    {
        return Err(invalid("partition count exceeds policy"));
    }
    let mut bytes = 0_u64;
    let mut entries = 0_u64;
    let mut previous_prefix = None;
    for row in partitions {
        if !prefix_canonical(row.prefix_bits, &row.prefix)
            || row.prefix_bits != PREFIX_BITS
            || row.bytes == 0
            || (require_entries && row.entries == 0)
        {
            return Err(invalid(
                "partition descriptor is noncanonical or duplicated",
            ));
        }
        if previous_prefix.is_some_and(|previous| previous >= row.prefix[0]) {
            return Err(invalid("partition prefixes are not strictly ascending"));
        }
        previous_prefix = Some(row.prefix[0]);
        bytes = bytes
            .checked_add(row.bytes)
            .ok_or_else(|| invalid("partition bytes overflow"))?;
        entries = entries
            .checked_add(row.entries)
            .ok_or_else(|| invalid("partition entries overflow"))?;
    }
    Ok((bytes, entries))
}

fn matching_partition<'a>(
    key: &[u8; 32],
    partitions: &'a [Partition],
) -> Result<&'a Partition, String> {
    let index = partitions
        .binary_search_by_key(&key[0], |partition| partition.prefix[0])
        .map_err(|error| invalid(&format!("source has no partition: {error}")))?;
    partitions
        .get(index)
        .ok_or_else(|| invalid("source partition index is invalid"))
}

// Reject forged container lengths before serde can reserve owned Vec/String
// storage. Canonical CBOR is checked again by re-encoding after typed decode.
struct CborPreflight<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl CborPreflight<'_> {
    fn header(&mut self) -> Result<(u8, u64), String> {
        let first = *self
            .bytes
            .get(self.offset)
            .ok_or_else(|| invalid("truncated CBOR"))?;
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or_else(|| invalid("CBOR header offset overflow"))?;
        let extra = match first & 31 {
            0..=23 => return Ok((first >> 5, u64::from(first & 31))),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err(invalid("indefinite or reserved CBOR is refused")),
        };
        let end = self
            .offset
            .checked_add(extra)
            .ok_or_else(|| invalid("CBOR length overflow"))?;
        let encoded = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| invalid("truncated CBOR length"))?;
        self.offset = end;
        let mut value = 0_u64;
        for byte in encoded {
            value = value
                .checked_mul(256)
                .and_then(|prior| prior.checked_add(u64::from(*byte)))
                .ok_or_else(|| invalid("CBOR length overflow"))?;
        }
        Ok((first >> 5, value))
    }

    fn skip(&mut self, depth: u8) -> Result<(), String> {
        if depth > 16 {
            return Err(invalid("CBOR nesting exceeds bound"));
        }
        let (major, count) = self.header()?;
        match major {
            0 | 1 | 7 => Ok(()),
            2 | 3 => {
                let length = usize::try_from(count)
                    .map_err(|error| invalid(&format!("CBOR byte length overflow: {error}")))?;
                self.offset = self
                    .offset
                    .checked_add(length)
                    .ok_or_else(|| invalid("CBOR offset overflow"))?;
                if self.offset > self.bytes.len() {
                    return Err(invalid("truncated CBOR body"));
                }
                Ok(())
            }
            4 | 5 => {
                let children = if major == 5 {
                    count
                        .checked_mul(2)
                        .ok_or_else(|| invalid("CBOR map count overflow"))?
                } else {
                    count
                };
                let remaining = self.bytes.len().saturating_sub(self.offset);
                if children
                    > u64::try_from(remaining).map_err(|error| {
                        invalid(&format!("CBOR remaining length overflow: {error}"))
                    })?
                {
                    return Err(invalid("CBOR container count exceeds encoded bytes"));
                }
                for _ in 0..children {
                    let child_depth = depth
                        .checked_add(1)
                        .ok_or_else(|| invalid("CBOR nesting depth overflow"))?;
                    self.skip(child_depth)?;
                }
                Ok(())
            }
            6 => self.skip(
                depth
                    .checked_add(1)
                    .ok_or_else(|| invalid("CBOR nesting depth overflow"))?,
            ),
            _ => Err(invalid("unknown CBOR major type")),
        }
    }

    fn bounded_array(&mut self, max: u64) -> Result<(), String> {
        let (major, count) = self.header()?;
        if major != 4 || count > max {
            return Err(invalid("root array count exceeds policy"));
        }
        let remaining = self.bytes.len().saturating_sub(self.offset);
        if count
            > u64::try_from(remaining)
                .map_err(|error| invalid(&format!("CBOR remaining length overflow: {error}")))?
        {
            return Err(invalid("root array count exceeds encoded bytes"));
        }
        for _ in 0..count {
            self.skip(1)?;
        }
        Ok(())
    }
}

fn preflight_root(bytes: &[u8], policy: AuthorityPolicy) -> Result<(), String> {
    let mut scan = CborPreflight { bytes, offset: 0 };
    let (major, count) = scan.header()?;
    if major != 4 || count != 7 {
        return Err(invalid("root CBOR tuple shape"));
    }
    for _ in 0..3 {
        scan.skip(1)?;
    }
    scan.bounded_array(policy.source_files)?;
    for _ in 0..3 {
        scan.bounded_array(policy.partitions)?;
    }
    if scan.offset != bytes.len() {
        return Err(invalid("trailing CBOR bytes"));
    }
    Ok(())
}

impl AuthorityRoot {
    pub(crate) fn term_directory_charge(&self, policy: AuthorityPolicy) -> Result<u64, String> {
        let mut charge = 0_u64;
        for partition in self.path_postings.iter().chain(&self.content_postings) {
            charge = charge
                .checked_add(term_directory_block_charge(u64::from(partition.terms))?)
                .ok_or_else(|| invalid("term directory aggregate charge overflow"))?;
            if charge > policy.term_directory_bytes {
                return Err(invalid("term directory exceeds policy"));
            }
        }
        Ok(charge)
    }

    pub(super) fn encode(&self, policy: AuthorityPolicy) -> Result<Vec<u8>, String> {
        self.validate(policy)?;
        let bytes = encode_cbor(&wire(self), "file authority v15 root")
            .map_err(|error| format!("file authority v15: {error}"))?;
        if u64::try_from(bytes.len())
            .map_err(|error| invalid(&format!("root byte count overflow: {error}")))?
            > policy.root_bytes
        {
            return Err(invalid("root exceeds byte policy"));
        }
        Ok(bytes)
    }

    pub(super) fn decode(bytes: &[u8], policy: AuthorityPolicy) -> Result<Self, String> {
        if u64::try_from(bytes.len())
            .map_err(|error| invalid(&format!("root byte count overflow: {error}")))?
            > policy.root_bytes
        {
            return Err(invalid("root exceeds byte policy"));
        }
        preflight_root(bytes, policy)?;
        let (
            format,
            policy_sha256,
            next_source_id,
            sources,
            packs,
            path_postings,
            content_postings,
        ): RootWire =
            decode_cbor_exact(bytes).map_err(|error| invalid(&format!("root decode: {error}")))?;
        if format != FORMAT {
            return Err(invalid("root format requires rebuild"));
        }
        let root = Self {
            policy_sha256,
            next_source_id,
            sources: sources
                .into_iter()
                .map(
                    |(
                        source,
                        text_admitted,
                        language,
                        posting_memberships,
                        source_bytes,
                        resident_heap_bytes,
                        source_id,
                        pack_sha256,
                    )| {
                        SourceRow {
                            source,
                            text_admitted,
                            language,
                            posting_memberships,
                            source_bytes,
                            resident_heap_bytes,
                            source_id,
                            pack_sha256,
                        }
                    },
                )
                .collect(),
            packs: packs.into_iter().map(from_partition_wire).collect(),
            path_postings: path_postings.into_iter().map(from_partition_wire).collect(),
            content_postings: content_postings
                .into_iter()
                .map(from_partition_wire)
                .collect(),
        };
        root.validate(policy)?;
        if root.encode(policy)? != bytes {
            return Err(invalid("root CBOR is noncanonical"));
        }
        Ok(root)
    }

    pub(super) fn validate(&self, policy: AuthorityPolicy) -> Result<(), String> {
        if self.policy_sha256 != policy.digest() {
            return Err(invalid("policy identity differs; rebuild required"));
        }
        if u64::try_from(self.sources.len())
            .map_err(|error| invalid(&format!("source count overflow: {error}")))?
            > policy.source_files
        {
            return Err(invalid("source count exceeds policy"));
        }
        if self.next_source_id == 0 || self.next_source_id > policy.source_id {
            return Err(invalid("next source ID exceeds policy"));
        }
        let (pack_bytes, pack_entries) = validate_partitions(&self.packs, policy, true)?;
        let (path_bytes, path_memberships) =
            validate_partitions(&self.path_postings, policy, false)?;
        let (content_bytes, content_memberships) =
            validate_partitions(&self.content_postings, policy, false)?;
        if self.packs.iter().any(|partition| partition.terms != 0)
            || self
                .path_postings
                .iter()
                .chain(&self.content_postings)
                .any(|partition| {
                    (partition.entries == 0) != (partition.terms == 0)
                        || u64::from(partition.terms) > partition.entries
                })
        {
            return Err(invalid("partition term count differs from surface"));
        }
        let _directory_charge = self.term_directory_charge(policy)?;
        if self
            .packs
            .iter()
            .any(|partition| partition.bytes > policy.pack_bytes)
            || self
                .path_postings
                .iter()
                .chain(&self.content_postings)
                .any(|partition| partition.bytes > policy.posting_block_bytes)
        {
            return Err(invalid("partition exceeds per-block byte policy"));
        }
        let posting_bytes = path_bytes
            .checked_add(content_bytes)
            .ok_or_else(|| invalid("posting bytes overflow"))?;
        if posting_bytes > policy.total_posting_bytes {
            return Err(invalid("posting bytes exceed aggregate policy"));
        }
        let memberships = path_memberships
            .checked_add(content_memberships)
            .ok_or_else(|| invalid("posting memberships overflow"))?;
        if memberships > policy.total_memberships {
            return Err(invalid("postings exceed aggregate policy"));
        }
        let mut previous = None;
        let mut ids = BTreeSet::new();
        let mut digest_per_pack = BTreeSet::new();
        let mut source_memberships = 0_u64;
        let mut source_bytes = 0_u64;
        let mut resident_heap_bytes = self.term_directory_charge(policy)?;
        let mut used_pack_prefixes = BTreeSet::new();
        let mut used_path_prefixes = BTreeSet::new();
        let mut used_content_prefixes = BTreeSet::new();
        for row in &self.sources {
            row.source.validate().map_err(invalid)?;
            if previous.as_ref().is_some_and(|key| key >= &row.source.file) {
                return Err(invalid("source keys are not strictly ascending"));
            }
            previous = Some(row.source.file.clone());
            if row.source_id == 0
                || row.source_id >= self.next_source_id
                || !ids.insert(row.source_id)
            {
                return Err(invalid("source ID is absent, reused or outside watermark"));
            }
            source_memberships = source_memberships
                .checked_add(u64::from(row.posting_memberships))
                .ok_or_else(|| invalid("source posting count overflow"))?;
            if row.source_bytes > 8 * 1024 * 1024 {
                return Err(invalid("source file exceeds 8 MiB"));
            }
            source_bytes = source_bytes
                .checked_add(row.source_bytes)
                .ok_or_else(|| invalid("source byte count overflow"))?;
            if source_bytes > policy.source_bytes {
                return Err(invalid("source bytes exceed aggregate policy"));
            }
            if row.resident_heap_bytes < RESIDENT_FILE_ROW_CHARGE {
                return Err(invalid("resident row charge below fixed minimum"));
            }
            resident_heap_bytes = resident_heap_bytes
                .checked_add(row.resident_heap_bytes)
                .ok_or_else(|| invalid("resident heap charge overflow"))?;
            if resident_heap_bytes > policy.resident_file_heap_bytes {
                return Err(invalid("resident heap exceeds policy"));
            }
            let key_digest = source_key_digest(&row.source)?;
            let pack = matching_partition(&key_digest, &self.packs)?;
            if row.pack_sha256 != pack.sha256 {
                return Err(invalid("source pack binding differs"));
            }
            // Many source rows can share one pack and one packed body.
            let _new_pack = used_pack_prefixes.insert((pack.prefix_bits, pack.prefix));
            let _new_body =
                digest_per_pack.insert((pack.prefix_bits, pack.prefix, row.source.source_sha256));
            let path = matching_partition(&key_digest, &self.path_postings)?;
            let content = matching_partition(&key_digest, &self.content_postings)?;
            // Many source rows can share one posting bucket.
            let _new_path = used_path_prefixes.insert((path.prefix_bits, path.prefix));
            let _new_content = used_content_prefixes.insert((content.prefix_bits, content.prefix));
        }
        if source_memberships != memberships {
            return Err(invalid("source and posting membership totals differ"));
        }
        let digest_count = u64::try_from(digest_per_pack.len())
            .map_err(|error| invalid(&format!("unique source digest count overflow: {error}")))?;
        if used_pack_prefixes.len() != self.packs.len() || digest_count != pack_entries {
            return Err(invalid("pack inventory has unreferenced source bytes"));
        }
        if used_path_prefixes.len() != self.path_postings.len()
            || used_content_prefixes.len() != self.content_postings.len()
        {
            return Err(invalid("posting inventory has unreferenced source bucket"));
        }
        if pack_bytes > policy.total_pack_bytes {
            return Err(invalid("pack encoded bytes exceed aggregate policy"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision,
    };

    use super::{
        AuthorityPolicy, AuthorityRoot, Partition, SourceRow, preflight_root, resident_file_charge,
        source_key_digest,
    };

    fn policy() -> AuthorityPolicy {
        AuthorityPolicy {
            root_bytes: 16 * 1024 * 1024,
            source_files: 10,
            source_bytes: 100,
            pack_bytes: 100,
            total_pack_bytes: 200,
            posting_block_bytes: 100,
            total_posting_bytes: 200,
            total_memberships: 10,
            partitions: 10,
            source_id: 100,
            bucket_scratch_bytes: 1024,
            term_directory_bytes: 4096,
            resident_file_heap_bytes: 8192,
            query_list_reads: 512,
            query_posting_ids: 1024,
            query_decoded_bytes: 2048,
            query_decoded_ids: 10,
        }
    }

    fn source() -> SourceFileRevision {
        SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("a.rs"),
            },
            revision_id: RevisionId::new("rev").expect("revision"),
            source_sha256: [7; 32],
        }
    }

    fn source_prefix(value: &SourceFileRevision) -> u8 {
        source_key_digest(value)
            .expect("key digest")
            .first()
            .copied()
            .expect("digest prefix")
    }

    fn descriptor(sha256: [u8; 32], bytes: u64, entries: u64) -> Partition {
        let first = source_prefix(&source());
        Partition {
            prefix_bits: 8,
            prefix: std::array::from_fn(|index| if index == 0 { first } else { 0 }),
            sha256,
            bytes,
            entries,
            terms: u32::from(entries != 0),
        }
    }

    fn root() -> AuthorityRoot {
        AuthorityRoot {
            policy_sha256: policy().digest(),
            next_source_id: 2,
            sources: vec![SourceRow {
                source: source(),
                text_admitted: false,
                language: LanguageCode::new("rust").expect("language"),
                posting_memberships: 2,
                source_bytes: 3,
                resident_heap_bytes: 1097,
                source_id: 1,
                pack_sha256: [8; 32],
            }],
            packs: vec![Partition {
                terms: 0,
                ..descriptor([8; 32], 47, 1)
            }],
            path_postings: vec![Partition {
                terms: 2,
                ..descriptor([9; 32], 48, 2)
            }],
            content_postings: vec![descriptor([10; 32], 32, 0)],
        }
    }

    #[test]
    fn exact_root_roundtrip_includes_empty_content_block() {
        let bytes = root().encode(policy()).expect("encode");
        assert_eq!(
            AuthorityRoot::decode(&bytes, policy()).expect("decode"),
            root()
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert!(AuthorityRoot::decode(&trailing, policy()).is_err());
    }

    #[test]
    fn root_rejects_policy_inventory_and_global_count_mutants() {
        let mut changed_policy = policy();
        changed_policy.total_memberships += 1;
        assert!(root().encode(changed_policy).is_err());

        let mut absent_pack = root();
        absent_pack.sources.first_mut().expect("source").pack_sha256 = [11; 32];
        assert!(absent_pack.encode(policy()).is_err());

        let mut missing_content_bucket = root();
        missing_content_bucket.content_postings.clear();
        assert!(missing_content_bucket.encode(policy()).is_err());

        let mut forged_count = root();
        forged_count
            .sources
            .first_mut()
            .expect("source")
            .posting_memberships = 1;
        assert!(forged_count.encode(policy()).is_err());

        let mut reused_id = root();
        let duplicate = reused_id.sources.first().expect("source").clone();
        reused_id.sources.push(duplicate);
        assert!(reused_id.encode(policy()).is_err());

        let mut forged_terms = root();
        forged_terms
            .content_postings
            .first_mut()
            .expect("content bucket")
            .terms = 1;
        assert!(forged_terms.encode(policy()).is_err());

        let mut tight = policy();
        // One nonempty page (64 bytes) and two retained buckets (128 each).
        assert_eq!(root().term_directory_charge(policy()).expect("charge"), 320);
        tight.term_directory_bytes = 319;
        let mut over_directory = root();
        over_directory.policy_sha256 = tight.digest();
        let reason = over_directory
            .encode(tight)
            .expect_err("producer must refuse");
        assert!(reason.contains("term directory exceeds policy"), "{reason}");

        let mut tight_heap = policy();
        // The source row costs 1,097 bytes, plus the 320-byte directory.
        tight_heap.resident_file_heap_bytes = 1416;
        let mut over_heap = root();
        over_heap.policy_sha256 = tight_heap.digest();
        let reason = over_heap
            .encode(tight_heap)
            .expect_err("resident producer admission");
        assert!(reason.contains("resident heap exceeds policy"), "{reason}");
    }

    #[test]
    fn paged_directory_admits_many_terms_and_enforces_the_actual_retained_charge() {
        let mut many = root();
        many.path_postings.first_mut().expect("path bucket").terms = 262_144;
        many.content_postings
            .first_mut()
            .expect("content bucket")
            .terms = 262_144;
        let mut admitted = policy();
        admitted.term_directory_bytes = 32 * 1024 * 1024;
        // Two 2,048-page tables, 64 bytes/page plus 128 bytes/bucket.
        assert_eq!(
            many.term_directory_charge(admitted)
                .expect("bounded sparse directory"),
            262_400
        );
        admitted.term_directory_bytes = 262_399;
        assert!(many.term_directory_charge(admitted).is_err());
    }

    #[test]
    fn resident_charge_fixed_ascii_census_and_overflow() {
        let source = source();
        let language = LanguageCode::new("rust").expect("language");
        // 1024 row, 3 raw, 4+4 path, no content, six 8-byte keys,
        // two copies of the 3-byte revision and 4-byte language.
        assert_eq!(
            resident_file_charge(&source, &language, 3, 4, 4, 0, 0).expect("charge"),
            1097
        );
        assert!(resident_file_charge(&source, &language, usize::MAX, 4, 4, 0, 0).is_err());
    }

    #[test]
    fn encoded_small_root_with_forged_huge_array_count_fails_before_decode() {
        let mut bytes = vec![0x87, 0x0f, 0x58, 0x20];
        bytes.extend_from_slice(&[0; 32]);
        bytes.push(0x02);
        bytes.push(0x9b);
        bytes.extend_from_slice(&u64::MAX.to_be_bytes());
        let failure = preflight_root(&bytes, policy()).expect_err("array count");
        assert!(failure.contains("count exceeds policy"), "{failure}");
    }

    #[test]
    fn duplicate_body_in_two_source_key_buckets_is_canonical() {
        let first = source();
        let first_prefix = source_prefix(&first);
        let second = (0..100)
            .map(|index| SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("repo").expect("repo"),
                    repo_relative_path: RepoRelativePath::new(format!("b{index}.rs")),
                },
                revision_id: RevisionId::new("rev").expect("revision"),
                source_sha256: first.source_sha256,
            })
            .find(|candidate| source_prefix(candidate) != first_prefix)
            .expect("opposite key bucket");
        let language = LanguageCode::new("rust").expect("language");
        let charge = |source: &SourceFileRevision| {
            let path = source.file.repo_relative_path.as_str().len();
            resident_file_charge(source, &language, 3, path, path, 0, 0).expect("resident charge")
        };
        let first_charge = charge(&first);
        let second_charge = charge(&second);
        let mut root = root();
        root.sources = vec![
            SourceRow {
                source: first,
                text_admitted: false,
                language: language.clone(),
                posting_memberships: 1,
                source_bytes: 3,
                resident_heap_bytes: first_charge,
                source_id: 1,
                pack_sha256: [8; 32],
            },
            SourceRow {
                source: second,
                text_admitted: false,
                language,
                posting_memberships: 1,
                source_bytes: 3,
                resident_heap_bytes: second_charge,
                source_id: 2,
                pack_sha256: [8; 32],
            },
        ];
        root.sources
            .sort_by(|a, b| a.source.file.cmp(&b.source.file));
        root.next_source_id = 3;
        let mut prefixes = [
            source_prefix(&root.sources.first().expect("first source").source),
            source_prefix(&root.sources.get(1).expect("second source").source),
        ];
        prefixes.sort_unstable();
        let buckets = prefixes.map(|prefix| Partition {
            prefix_bits: 8,
            prefix: std::array::from_fn(|index| if index == 0 { prefix } else { 0 }),
            sha256: [8; 32],
            bytes: 47,
            entries: 1,
            terms: 0,
        });
        root.packs = buckets.to_vec();
        root.path_postings = buckets
            .iter()
            .map(|row| Partition {
                sha256: [9; 32],
                bytes: 48,
                terms: 1,
                ..row.clone()
            })
            .collect();
        root.content_postings = buckets
            .iter()
            .map(|row| Partition {
                sha256: [10; 32],
                bytes: 32,
                entries: 0,
                terms: 0,
                ..row.clone()
            })
            .collect();
        assert!(root.encode(policy()).is_ok());
        root.path_postings.swap(0, 1);
        assert!(root.encode(policy()).is_err());
    }
}
