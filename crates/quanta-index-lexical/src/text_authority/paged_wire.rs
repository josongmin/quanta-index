//! Bounded keys and rows for the paged Native authority.

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    SourceFileCoverage, SourceFileKey, SourceFileRevision, SymbolCoverage, SymbolNameSourcePolicyV1,
};
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use crate::lexical_store::BlobRef;

const ROW_VERSION: u16 = 1;
pub(crate) const SEGMENT_DOCS: usize = 2048;
pub(crate) const MAX_CANDIDATE_BYTES: usize = 16 * 1024;
// One admitted 8 MiB source can expand under folding and CBOR framing.
pub(crate) const MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_POSTING_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) enum NativeSourceEdit {
    Upsert(SourceFileCoverage),
    Delete(SourceFileKey),
}

impl NativeSourceEdit {
    pub(crate) fn key(&self) -> &SourceFileKey {
        match self {
            Self::Upsert(coverage) => &coverage.source.file,
            Self::Delete(key) => key,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CurrentRecord {
    pub(crate) id: u64,
    pub(crate) source: SourceFileKey,
    pub(crate) candidate_id: String,
    pub(crate) indexed_sha256: [u8; 32],
    pub(crate) body: BlobRef,
    pub(crate) segment_key: u64,
    pub(crate) local_id: u32,
}

impl CurrentRecord {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        crate::channel_payloads::encode_cbor(
            &(
                ROW_VERSION,
                self.id,
                &self.source,
                &self.candidate_id,
                self.indexed_sha256,
                &self.body,
                self.segment_key,
                self.local_id,
            ),
            "Native current record",
        )
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let (version, id, source, candidate_id, indexed_sha256, body, segment_key, local_id): (
            u16,
            u64,
            SourceFileKey,
            String,
            [u8; 32],
            BlobRef,
            u64,
            u32,
        ) = crate::channel_payloads::decode_cbor_exact(bytes).map_err(|error| {
            CoreError::Storage(format!("lexical: decode Native current record: {error}"))
        })?;
        if version != ROW_VERSION
            || id == 0
            || segment_key == 0
            || usize::try_from(local_id).map_or(true, |id| id >= SEGMENT_DOCS)
            || candidate_id.is_empty()
            || candidate_id.len() > MAX_CANDIDATE_BYTES
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native current record shape is invalid".into(),
            ));
        }
        Ok(Self {
            id,
            source,
            candidate_id,
            indexed_sha256,
            body,
            segment_key,
            local_id,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SegmentDescriptor {
    pub(crate) id: u64,
    pub(crate) logical_ids: Vec<u64>,
    /// Mutable by COW map value on retirement; immutable postings may still
    /// name retired local IDs and are filtered through this sorted live set.
    pub(crate) live_locals: Vec<u32>,
    /// Route-key inventory for bounded retirement and independent changed-
    /// segment verification. The referenced object may span physical blocks.
    pub(crate) route_inventory: Option<BlobRef>,
}

impl SegmentDescriptor {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        self.validate()?;
        crate::channel_payloads::encode_cbor(
            &(
                ROW_VERSION,
                self.id,
                &self.logical_ids,
                &self.live_locals,
                &self.route_inventory,
            ),
            "Native segment",
        )
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let (version, id, logical_ids, live_locals, route_inventory): (
            u16,
            u64,
            Vec<u64>,
            Vec<u32>,
            Option<BlobRef>,
        ) = crate::channel_payloads::decode_cbor_exact(bytes).map_err(|error| {
            CoreError::Storage(format!("lexical: decode Native segment: {error}"))
        })?;
        if version != ROW_VERSION {
            return Err(CoreError::InvalidContract(
                "lexical: Native segment format differs".into(),
            ));
        }
        let descriptor = Self {
            id,
            logical_ids,
            live_locals,
            route_inventory,
        };
        if descriptor.route_inventory.is_none() {
            return Err(CoreError::InvalidContract(
                "lexical: Native segment route inventory is absent".into(),
            ));
        }
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.id == 0
            || self.logical_ids.is_empty()
            || self.logical_ids.len() > SEGMENT_DOCS
            || self.logical_ids.contains(&0)
            || self.logical_ids.windows(2).any(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| left >= right)
            })
            || self.live_locals.windows(2).any(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| left >= right)
            })
            || self.live_locals.iter().any(|local| {
                usize::try_from(*local).map_or(true, |local| local >= self.logical_ids.len())
            })
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native segment IDs are invalid".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn source_digest(key: &SourceFileKey) -> Result<[u8; 32], CoreError> {
    let encoded = crate::channel_payloads::encode_cbor(key, "Native source identity")?;
    let mut hash = Sha256::new();
    hash.update(b"native-source-key-v1");
    hash.update(encoded);
    Ok(hash.finalize().into())
}

pub(crate) fn current_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}
pub(crate) fn segment_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

pub(crate) fn membership_key(source: &[u8; 32], id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(40);
    key.extend_from_slice(source);
    key.extend_from_slice(&id.to_be_bytes());
    key
}

pub(crate) fn gram_prefix(surface: u8, gram: [u8; 3]) -> [u8; 4] {
    [surface, gram[0], gram[1], gram[2]]
}

pub(crate) fn term_prefix(surface: u8, term: &str) -> Result<Vec<u8>, CoreError> {
    let length = u16::try_from(term.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: Native term key width: {error}"))
    })?;
    let mut key = Vec::new();
    let capacity = 3_usize.checked_add(term.len()).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native term key length overflows".into())
    })?;
    key.try_reserve(capacity).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve Native term key: {error}"))
    })?;
    key.push(surface);
    key.extend_from_slice(&length.to_be_bytes());
    key.extend_from_slice(term.as_bytes());
    Ok(key)
}

pub(crate) fn route_key(prefix: &[u8], segment_key: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len().saturating_add(8));
    key.extend_from_slice(prefix);
    key.extend_from_slice(&segment_key.to_be_bytes());
    key
}

pub(crate) fn prefix_upper(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut upper = prefix.to_vec();
    for index in (0..upper.len()).rev() {
        let byte = upper.get_mut(index)?;
        if *byte != u8::MAX {
            *byte = byte.saturating_add(1);
            upper.truncate(index.saturating_add(1));
            return Some(upper);
        }
    }
    None
}

pub(crate) fn encode_source(coverage: &SourceFileCoverage) -> Result<Vec<u8>, CoreError> {
    coverage.validate().map_err(|error| {
        CoreError::InvalidContract(format!("lexical: Native source coverage: {error}"))
    })?;
    crate::channel_payloads::encode_cbor(
        &(
            ROW_VERSION,
            &coverage.source,
            &coverage.language,
            coverage.producer_policy_sha256,
            coverage.symbol_name_source_policy,
            coverage.unit_set_sha256,
            coverage.text_admitted,
            coverage.symbols,
        ),
        "Native source record",
    )
}

pub(crate) fn decode_source(bytes: &[u8]) -> Result<SourceFileCoverage, CoreError> {
    let (
        version,
        source,
        language,
        producer_policy_sha256,
        symbol_name_source_policy,
        unit_set_sha256,
        text_admitted,
        symbols,
    ): (
        u16,
        SourceFileRevision,
        LanguageCode,
        [u8; 32],
        SymbolNameSourcePolicyV1,
        [u8; 32],
        bool,
        SymbolCoverage,
    ) = crate::channel_payloads::decode_cbor_exact(bytes).map_err(|error| {
        CoreError::Storage(format!("lexical: decode Native source record: {error}"))
    })?;
    if version != ROW_VERSION {
        return Err(CoreError::InvalidContract(
            "lexical: Native source format differs".into(),
        ));
    }
    let coverage = SourceFileCoverage {
        source,
        language,
        producer_policy_sha256,
        symbol_name_source_policy,
        unit_set_sha256,
        text_admitted,
        symbols,
    };
    coverage.validate().map_err(|error| {
        CoreError::InvalidContract(format!("lexical: Native source coverage: {error}"))
    })?;
    Ok(coverage)
}
