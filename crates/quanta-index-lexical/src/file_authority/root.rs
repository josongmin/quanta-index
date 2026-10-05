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
pub(super) const PREFIX_BITS: u16 = 8;
pub(super) const TERM_DIRECTORY_ROW_CHARGE: u64 = 64;
pub(super) const TERM_DIRECTORY_BLOCK_CHARGE: u64 = 128;
pub(super) const RESIDENT_FILE_ROW_CHARGE: u64 = 1024;

/// Numeric policy is explicit and stamped in the root. The product owner
/// chooses these ceilings after a full fixture count; a shard-local ceiling
/// never stands in for the aggregate membership limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityPolicy {
    pub max_root_bytes: u64,
    pub max_source_files: u64,
    pub max_source_bytes: u64,
    pub max_pack_bytes: u64,
    pub max_total_pack_bytes: u64,
    pub max_posting_block_bytes: u64,
    pub max_total_posting_bytes: u64,
    pub max_total_memberships: u64,
    pub max_partitions: u64,
    pub max_source_id: u64,
    pub max_bucket_scratch_bytes: u64,
    pub max_term_directory_bytes: u64,
    pub max_resident_file_heap_bytes: u64,
    pub max_query_list_reads: u64,
    pub max_query_posting_ids: u64,
    pub max_query_decoded_bytes: u64,
    pub max_query_decoded_ids: u64,
}

impl AuthorityPolicy {
    pub(super) fn digest(self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"quanta-file-authority-policy-v15\0");
        for value in [
            self.max_root_bytes,
            self.max_source_files,
            self.max_source_bytes,
            self.max_pack_bytes,
            self.max_total_pack_bytes,
            self.max_posting_block_bytes,
            self.max_total_posting_bytes,
            self.max_total_memberships,
            self.max_partitions,
            self.max_source_id,
            self.max_bucket_scratch_bytes,
            self.max_term_directory_bytes,
            self.max_resident_file_heap_bytes,
            self.max_query_list_reads,
            self.max_query_posting_ids,
            self.max_query_decoded_bytes,
            self.max_query_decoded_ids,
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
    let full = usize::from(bits / 8);
    let partial = bits % 8;
    if partial != 0 {
        let mask = (1_u8 << (8 - partial)) - 1;
        if prefix[full] & mask != 0 {
            return false;
        }
    }
    let suffix = full + usize::from(partial != 0);
    prefix[suffix..].iter().all(|byte| *byte == 0)
}

pub(super) fn source_key_digest(source: &SourceFileRevision) -> Result<[u8; 32], String> {
    let repo = source.file.source_repo_id.as_str().as_bytes();
    let path = source.file.repo_relative_path.as_str().as_bytes();
    let repo_len = u64::try_from(repo.len()).map_err(|_| invalid("repo key length overflow"))?;
    let path_len = u64::try_from(path.len()).map_err(|_| invalid("path key length overflow"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-authority-source-key-v15\0");
    hasher.update(repo_len.to_le_bytes());
    hasher.update(repo);
    hasher.update(path_len.to_le_bytes());
    hasher.update(path);
    Ok(hasher.finalize().into())
}

/// Logical serving-heap admission, not a physical RSS bound. Six key copies
/// account for root, file, ordered and ID maps; row charge covers container
/// nodes/headers. Producer and cold opener use this exact function.
pub(super) fn resident_file_charge(
    source: &SourceFileRevision,
    language: &LanguageCode,
    raw_bytes: usize,
    indexed_path_bytes: usize,
    folded_path_bytes: usize,
    indexed_text_bytes: usize,
    folded_text_bytes: usize,
) -> Result<u64, String> {
    let width = |value: usize| u64::try_from(value).map_err(|_| invalid("resident length width"));
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
    if u64::try_from(partitions.len()).map_err(|_| invalid("partition count overflow"))?
        > policy.max_partitions
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
        .map_err(|_| invalid("source has no partition"))?;
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
        self.offset += 1;
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
                let length =
                    usize::try_from(count).map_err(|_| invalid("CBOR byte length overflow"))?;
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
                    > u64::try_from(remaining)
                        .map_err(|_| invalid("CBOR remaining length overflow"))?
                {
                    return Err(invalid("CBOR container count exceeds encoded bytes"));
                }
                for _ in 0..children {
                    self.skip(depth + 1)?;
                }
                Ok(())
            }
            6 => self.skip(depth + 1),
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
            > u64::try_from(remaining).map_err(|_| invalid("CBOR remaining length overflow"))?
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
    scan.bounded_array(policy.max_source_files)?;
    for _ in 0..3 {
        scan.bounded_array(policy.max_partitions)?;
    }
    if scan.offset != bytes.len() {
        return Err(invalid("trailing CBOR bytes"));
    }
    Ok(())
}

impl AuthorityRoot {
    pub(crate) fn term_directory_charge(&self, policy: AuthorityPolicy) -> Result<u64, String> {
        let blocks = self
            .path_postings
            .len()
            .checked_add(self.content_postings.len())
            .ok_or_else(|| invalid("term directory block count overflow"))?;
        let mut charge = u64::try_from(blocks)
            .map_err(|_| invalid("term directory block count width"))?
            .checked_mul(TERM_DIRECTORY_BLOCK_CHARGE)
            .ok_or_else(|| invalid("term directory block charge overflow"))?;
        for partition in self.path_postings.iter().chain(&self.content_postings) {
            charge = charge
                .checked_add(
                    u64::from(partition.terms)
                        .checked_mul(TERM_DIRECTORY_ROW_CHARGE)
                        .ok_or_else(|| invalid("term directory row charge overflow"))?,
                )
                .ok_or_else(|| invalid("term directory aggregate charge overflow"))?;
            if charge > policy.max_term_directory_bytes {
                return Err(invalid("term directory exceeds policy"));
            }
        }
        Ok(charge)
    }

    pub(super) fn encode(&self, policy: AuthorityPolicy) -> Result<Vec<u8>, String> {
        self.validate(policy)?;
        let bytes = encode_cbor(&wire(self), "file authority v15 root")
            .map_err(|error| invalid(&error.to_string()))?;
        if u64::try_from(bytes.len()).map_err(|_| invalid("root byte count overflow"))?
            > policy.max_root_bytes
        {
            return Err(invalid("root exceeds byte policy"));
        }
        Ok(bytes)
    }

    pub(super) fn decode(bytes: &[u8], policy: AuthorityPolicy) -> Result<Self, String> {
        if u64::try_from(bytes.len()).map_err(|_| invalid("root byte count overflow"))?
            > policy.max_root_bytes
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
        if u64::try_from(self.sources.len()).map_err(|_| invalid("source count overflow"))?
            > policy.max_source_files
        {
            return Err(invalid("source count exceeds policy"));
        }
        if self.next_source_id == 0 || self.next_source_id > policy.max_source_id {
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
            .any(|partition| partition.bytes > policy.max_pack_bytes)
            || self
                .path_postings
                .iter()
                .chain(&self.content_postings)
                .any(|partition| partition.bytes > policy.max_posting_block_bytes)
        {
            return Err(invalid("partition exceeds per-block byte policy"));
        }
        let posting_bytes = path_bytes
            .checked_add(content_bytes)
            .ok_or_else(|| invalid("posting bytes overflow"))?;
        if posting_bytes > policy.max_total_posting_bytes {
            return Err(invalid("posting bytes exceed aggregate policy"));
        }
        let memberships = path_memberships
            .checked_add(content_memberships)
            .ok_or_else(|| invalid("posting memberships overflow"))?;
        if memberships > policy.max_total_memberships {
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
            row.source
                .validate()
                .map_err(|error| invalid(&error.to_string()))?;
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
            if source_bytes > policy.max_source_bytes {
                return Err(invalid("source bytes exceed aggregate policy"));
            }
            if row.resident_heap_bytes < RESIDENT_FILE_ROW_CHARGE {
                return Err(invalid("resident row charge below fixed minimum"));
            }
            resident_heap_bytes = resident_heap_bytes
                .checked_add(row.resident_heap_bytes)
                .ok_or_else(|| invalid("resident heap charge overflow"))?;
            if resident_heap_bytes > policy.max_resident_file_heap_bytes {
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
            .map_err(|_| invalid("unique source digest count overflow"))?;
        if used_pack_prefixes.len() != self.packs.len() || digest_count != pack_entries {
            return Err(invalid("pack inventory has unreferenced source bytes"));
        }
        if used_path_prefixes.len() != self.path_postings.len()
            || used_content_prefixes.len() != self.content_postings.len()
        {
            return Err(invalid("posting inventory has unreferenced source bucket"));
        }
        if pack_bytes > policy.max_total_pack_bytes {
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
            max_root_bytes: 16 * 1024 * 1024,
            max_source_files: 10,
            max_source_bytes: 100,
            max_pack_bytes: 100,
            max_total_pack_bytes: 200,
            max_posting_block_bytes: 100,
            max_total_posting_bytes: 200,
            max_total_memberships: 10,
            max_partitions: 10,
            max_source_id: 100,
            max_bucket_scratch_bytes: 1024,
            max_term_directory_bytes: 4096,
            max_resident_file_heap_bytes: 8192,
            max_query_list_reads: 512,
            max_query_posting_ids: 1024,
            max_query_decoded_bytes: 2048,
            max_query_decoded_ids: 10,
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

    fn descriptor(sha256: [u8; 32], bytes: u64, entries: u64) -> Partition {
        let first = source_key_digest(&source()).expect("key digest")[0];
        Partition {
            prefix_bits: 8,
            prefix: std::array::from_fn(|index| if index == 0 { first } else { 0 }),
            sha256,
            bytes,
            entries,
            terms: if entries == 0 { 0 } else { 1 },
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
        changed_policy.max_total_memberships += 1;
        assert!(root().encode(changed_policy).is_err());

        let mut absent_pack = root();
        absent_pack.sources[0].pack_sha256 = [11; 32];
        assert!(absent_pack.encode(policy()).is_err());

        let mut missing_content_bucket = root();
        missing_content_bucket.content_postings.clear();
        assert!(missing_content_bucket.encode(policy()).is_err());

        let mut forged_count = root();
        forged_count.sources[0].posting_memberships = 1;
        assert!(forged_count.encode(policy()).is_err());

        let mut reused_id = root();
        reused_id.sources.push(reused_id.sources[0].clone());
        assert!(reused_id.encode(policy()).is_err());

        let mut forged_terms = root();
        forged_terms.content_postings[0].terms = 1;
        assert!(forged_terms.encode(policy()).is_err());

        let mut tight = policy();
        tight.max_term_directory_bytes = 383;
        let mut over_directory = root();
        over_directory.policy_sha256 = tight.digest();
        let reason = over_directory
            .encode(tight)
            .expect_err("producer must refuse");
        assert!(reason.contains("term directory exceeds policy"), "{reason}");

        let mut tight_heap = policy();
        tight_heap.max_resident_file_heap_bytes = 1480;
        let mut over_heap = root();
        over_heap.policy_sha256 = tight_heap.digest();
        let reason = over_heap
            .encode(tight_heap)
            .expect_err("resident producer admission");
        assert!(reason.contains("resident heap exceeds policy"), "{reason}");
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
        let first_prefix = source_key_digest(&first).expect("key digest")[0];
        let second = (0..100)
            .map(|index| SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("repo").expect("repo"),
                    repo_relative_path: RepoRelativePath::new(format!("b{index}.rs")),
                },
                revision_id: RevisionId::new("rev").expect("revision"),
                source_sha256: first.source_sha256,
            })
            .find(|candidate| source_key_digest(candidate).expect("key digest")[0] != first_prefix)
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
            source_key_digest(&root.sources[0].source).expect("key digest")[0],
            source_key_digest(&root.sources[1].source).expect("key digest")[0],
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
