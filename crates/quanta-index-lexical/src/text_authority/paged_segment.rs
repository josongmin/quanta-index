//! One changed Native segment, split into independently readable bodies and postings.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use crate::normalize::{self, CaseMode};
use crate::text_authority::paged_wire::{MAX_BODY_BYTES, SEGMENT_DOCS, SegmentDescriptor};
use crate::text_authority::shard::ShardBuilders;
use crate::text_authority::types::AddedTextDoc;

const POSTING_VERSION: u16 = 1;
const BODY_VERSION: u16 = 2;
pub(crate) const GRAM_RAW: u8 = 1;
pub(crate) const GRAM_FOLDED: u8 = 2;
pub(crate) const TERM_RAW: u8 = 3;
pub(crate) const TERM_FOLDED: u8 = 4;

#[derive(Clone, Debug)]
pub(crate) struct BuiltBody {
    pub(crate) logical_id: u64,
    pub(crate) local_id: u32,
    pub(crate) candidate_id: String,
    pub(crate) indexed_sha256: [u8; 32],
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct BuiltPosting {
    pub(crate) surface: u8,
    pub(crate) key: Vec<u8>,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct BuiltSegment {
    pub(crate) descriptor: SegmentDescriptor,
    pub(crate) bodies: Vec<BuiltBody>,
    pub(crate) grams: Vec<BuiltPosting>,
    pub(crate) terms: Vec<BuiltPosting>,
}

impl BuiltSegment {
    pub(crate) fn build(
        segment_key: u64,
        generation: ManifestGeneration,
        docs: &[AddedTextDoc],
    ) -> Result<Self, CoreError> {
        if segment_key == 0
            || docs.is_empty()
            || docs.len() > SEGMENT_DOCS
            || docs.iter().any(|doc| doc.doc_id == 0)
            || docs.windows(2).any(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| left.doc_id >= right.doc_id)
            })
        {
            return Err(CoreError::InvalidContract(
                "lexical: changed Native segment IDs are not increasing and bounded".into(),
            ));
        }
        let mut builder = ShardBuilders::empty(0, generation)?;
        for (local, doc) in docs.iter().enumerate() {
            let local = u64::try_from(local).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: Native local ID width: {error}"))
            })?;
            builder.upsert(local, doc.candidate_id.clone(), &doc.text)?;
        }
        let proved = builder.finish()?;
        // This verifier derives exact gram and position relations from raw
        // normalized bodies, without using the posting builder as an oracle.
        proved.verify_semantics(0, Path::new("text-authority-segment"), "changed-segment")?;
        let descriptor = SegmentDescriptor {
            id: segment_key,
            logical_ids: docs.iter().map(|doc| doc.doc_id).collect(),
            live_locals: (0..docs.len())
                .map(|local| {
                    u32::try_from(local).map_err(|error| {
                        CoreError::InvalidContract(format!(
                            "lexical: Native local ID width: {error}"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            route_inventory: None,
        };
        descriptor.validate()?;
        let mut bodies = Vec::new();
        bodies.try_reserve(docs.len()).map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native segment bodies: {error}"))
        })?;
        for (local, doc) in &proved.docs_by_id {
            let index = usize::try_from(*local).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: Native local index width: {error}"))
            })?;
            let logical_id = *descriptor.logical_ids.get(index).ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native body lacks logical locator".into())
            })?;
            let bytes = crate::channel_payloads::encode_cbor(
                &(BODY_VERSION, &doc.candidate_id, &doc.indexed_text),
                "Native body",
            )?;
            validate_body_size(bytes.len(), doc.folded_indexed_text.len())?;
            bodies.push(BuiltBody {
                logical_id,
                local_id: u32::try_from(*local).map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: Native local ID width: {error}"))
                })?,
                candidate_id: doc.candidate_id.clone(),
                indexed_sha256: Sha256::digest(doc.indexed_text.as_bytes()).into(),
                bytes,
            });
        }
        let mut grams = Vec::new();
        for (surface, index) in [
            (GRAM_RAW, &proved.trigram),
            (GRAM_FOLDED, &proved.trigram_folded),
        ] {
            for (gram, postings) in index.iter() {
                let locals = posting_locals(postings.iter().map(|id| id.0), docs.len())?;
                grams.push(BuiltPosting {
                    surface,
                    key: gram.to_vec(),
                    bytes: encode_posting(segment_key, surface, &locals, None)?,
                });
            }
        }
        let mut terms = Vec::new();
        for (surface, index, mode) in [
            (TERM_RAW, &proved.positions, CaseMode::Sensitive),
            (TERM_FOLDED, &proved.positions_folded, CaseMode::Folded),
        ] {
            let expected = independent_term_members(&proved.docs_by_id, mode)?;
            for term in index.terms() {
                let locals = expected.get(term).ok_or_else(|| {
                    CoreError::InvalidContract("lexical: Native term lacks source members".into())
                })?;
                let raw = index.raw_postings(term).ok_or_else(|| {
                    CoreError::InvalidContract("lexical: Native term lacks positions".into())
                })?;
                terms.push(BuiltPosting {
                    surface,
                    key: term.as_bytes().to_vec(),
                    bytes: encode_posting(segment_key, surface, locals, Some(raw))?,
                });
            }
        }
        Ok(Self {
            descriptor,
            bodies,
            grams,
            terms,
        })
    }
}

fn posting_locals(
    ids: impl Iterator<Item = u64>,
    document_count: usize,
) -> Result<Vec<u32>, CoreError> {
    let mut locals = Vec::new();
    for id in ids {
        let local = u32::try_from(id).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: Native posting local ID width: {error}"))
        })?;
        if usize::try_from(local).map_or(true, |id| id >= document_count)
            || locals.last().is_some_and(|prior| *prior >= local)
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native posting locals are not sorted within segment".into(),
            ));
        }
        locals.push(local);
    }
    Ok(locals)
}

fn independent_term_members(
    docs: &BTreeMap<u64, crate::text_authority::shard::TextAuthorityDoc>,
    mode: CaseMode,
) -> Result<BTreeMap<String, Vec<u32>>, CoreError> {
    let mut terms = BTreeMap::<String, Vec<u32>>::new();
    for (id, doc) in docs {
        let local = u32::try_from(*id).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: Native term local ID width: {error}"))
        })?;
        let mut unique = BTreeSet::new();
        for token in normalize::tokenize(&doc.indexed_text, mode).indexable() {
            let _inserted = unique.insert(token.text.clone());
        }
        for term in unique {
            terms.entry(term).or_default().push(local);
        }
    }
    Ok(terms)
}

fn encode_posting(
    segment_key: u64,
    surface: u8,
    locals: &[u32],
    positions: Option<&[u8]>,
) -> Result<Vec<u8>, CoreError> {
    crate::channel_payloads::encode_cbor(
        &(POSTING_VERSION, segment_key, surface, locals, positions),
        "Native posting",
    )
}

pub(crate) fn decode_posting(
    bytes: &[u8],
    segment_key: u64,
    surface: u8,
    document_count: usize,
) -> Result<Vec<u32>, CoreError> {
    let (version, actual_segment, actual_surface, locals, positions): (
        u16,
        u64,
        u8,
        Vec<u32>,
        Option<Vec<u8>>,
    ) = crate::channel_payloads::decode_cbor_exact(bytes)
        .map_err(|error| CoreError::Storage(format!("lexical: decode Native posting: {error}")))?;
    if version != POSTING_VERSION
        || actual_segment != segment_key
        || actual_surface != surface
        || locals.is_empty()
        || locals.len() > document_count
        || matches!(surface, GRAM_RAW | GRAM_FOLDED) != positions.is_none()
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native posting row differs".into(),
        ));
    }
    posting_locals(locals.into_iter().map(u64::from), document_count)
}

pub(crate) fn decode_body(bytes: &[u8]) -> Result<(String, String, String), CoreError> {
    let (version, candidate, indexed): (u16, String, String) =
        crate::channel_payloads::decode_cbor_exact(bytes)
            .map_err(|error| CoreError::Storage(format!("lexical: decode Native body: {error}")))?;
    if version != BODY_VERSION || normalize::nfc(&indexed).as_ref() != indexed {
        return Err(CoreError::InvalidContract(
            "lexical: Native body policy differs".into(),
        ));
    }
    // Folding was already performed to verify the persisted second surface.
    // Keep that deterministic result instead of storing and decoding it twice.
    let folded = normalize::fold(&indexed);
    validate_body_size(bytes.len(), folded.len())?;
    Ok((candidate, indexed, folded))
}

/// Admit normalization scratch and both outputs before allocating them. The
/// caller has already charged the raw blob and its decoded wire strings.
pub(super) fn decode_body_for_preparation(
    bytes: &[u8],
    base: usize,
    ceiling: usize,
) -> Result<(String, String, String), CoreError> {
    let (version, candidate, indexed): (u16, String, String) =
        crate::channel_payloads::decode_cbor_exact(bytes)
            .map_err(|error| CoreError::Storage(format!("lexical: decode Native body: {error}")))?;
    if version != BODY_VERSION {
        return Err(CoreError::InvalidContract(
            "lexical: Native body policy differs".into(),
        ));
    }
    let plan = normalize::NfcFoldPlan::new_with_budget(&indexed, base, ceiling)
        .map_err(|error| preparation_normalization_error(&error))?;
    validate_body_size(bytes.len(), plan.folded_bytes())?;
    let (nfc, folded) = plan
        .build_with_budget(base, ceiling)
        .map_err(|error| preparation_normalization_error(&error))?;
    if nfc != indexed {
        return Err(CoreError::InvalidContract(
            "lexical: Native body policy differs".into(),
        ));
    }
    Ok((candidate, indexed, folded))
}

fn preparation_normalization_error(error: &normalize::NfcFoldBuildError) -> CoreError {
    use std::error::Error as _;
    let cap = matches!(
        error,
        normalize::NfcFoldBuildError::ScratchExceeded
            | normalize::NfcFoldBuildError::LengthOverflow
    ) || matches!(error, normalize::NfcFoldBuildError::Native(native)
            if matches!(native.source().and_then(|cause| cause.downcast_ref::<normalize::NfcAdmissionError>()),
                Some(normalize::NfcAdmissionError::ScratchExceeded | normalize::NfcAdmissionError::ArithmeticOverflow)));
    if cap {
        compaction_scratch_limit()
    } else {
        CoreError::Storage(format!("lexical: prepared Native normalization: {error}"))
    }
}

pub(super) fn compaction_scratch_limit() -> CoreError {
    CoreError::InvalidContract(
        "lexical: Native compaction preparation scratch budget exceeded".into(),
    )
}

#[cfg(test)]
mod preparation_tests {
    use super::{BODY_VERSION, decode_body, decode_body_for_preparation};

    #[test]
    fn admitted_decode_matches_the_canonical_unicode_body() {
        let bytes = crate::channel_payloads::encode_cbor(
            &(BODY_VERSION, "candidate", "İ CAFÉ 한글"),
            "test body",
        )
        .expect("encode");
        let admitted = decode_body_for_preparation(&bytes, 2 * bytes.len(), 1 << 20)
            .expect("admitted normalization");
        assert_eq!(
            admitted,
            decode_body(&bytes).expect("canonical normalization")
        );
        assert_eq!(admitted.2, "i\u{307} café 한글");
        assert!(decode_body_for_preparation(&bytes, 2 * bytes.len(), 2 * bytes.len()).is_err());
    }

    #[test]
    fn admitted_decode_rejects_noncanonical_nfc() {
        let bytes = crate::channel_payloads::encode_cbor(
            &(BODY_VERSION, "candidate", "e\u{301}"),
            "test body",
        )
        .expect("encode");
        assert!(decode_body_for_preparation(&bytes, 2 * bytes.len(), 1 << 20).is_err());
        assert!(decode_body(&bytes).is_err());
    }
}

fn validate_body_size(encoded_bytes: usize, folded_bytes: usize) -> Result<(), CoreError> {
    let folded_bytes = u64::try_from(folded_bytes).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: folded body width: {error}"))
    })?;
    let string_header = match folded_bytes {
        0..=23 => 1,
        24..=255 => 2,
        256..=65_535 => 3,
        65_536..=4_294_967_295 => 5,
        _ => 9,
    };
    // Preserve the prior two-surface admission bound despite the smaller wire
    // representation. Both tuple headers and body versions occupy one byte.
    let encoded_bytes = u64::try_from(encoded_bytes).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: encoded body width: {error}"))
    })?;
    let total = encoded_bytes
        .checked_add(folded_bytes)
        .and_then(|bytes| bytes.checked_add(string_header));
    if total.is_none_or(|bytes| bytes > MAX_BODY_BYTES) {
        return Err(CoreError::InvalidContract(
            "lexical: Native body exceeds admitted document bound".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod body_tests {
    use super::{decode_body, validate_body_size};

    #[test]
    fn body_recreates_unicode_fold_from_the_single_nfc_surface() {
        let bytes = crate::channel_payloads::encode_cbor(
            &(2_u16, "candidate", "CAFÉ Straße İ"),
            "fixed body",
        )
        .expect("fixed body encodes");
        assert_eq!(
            decode_body(&bytes).expect("fixed body decodes"),
            (
                "candidate".into(),
                "CAFÉ Straße İ".into(),
                "café straße i\u{307}".into()
            )
        );
        let legacy = crate::channel_payloads::encode_cbor(
            &(1_u16, "candidate", "CAFÉ Straße İ", "café straße i\u{307}"),
            "legacy body",
        )
        .expect("legacy body encodes");
        assert!(decode_body(&legacy).is_err());
    }

    #[test]
    fn body_refuses_wrong_version_and_non_nfc_text() {
        for (version, indexed) in [(1_u16, "CAFÉ"), (2, "CAFE\u{301}")] {
            let bytes = crate::channel_payloads::encode_cbor(
                &(version, "candidate", indexed),
                "invalid body",
            )
            .expect("invalid body encodes");
            assert!(decode_body(&bytes).is_err());
        }
    }

    #[test]
    fn compact_body_keeps_the_original_two_surface_size_admission() {
        let limit = usize::try_from(super::MAX_BODY_BYTES).expect("body bound fits usize");
        assert!(validate_body_size(limit - 25, 23).is_ok());
        assert!(validate_body_size(limit - 25, 24).is_err());
        assert!(validate_body_size(usize::MAX, usize::MAX).is_err());
    }
}
