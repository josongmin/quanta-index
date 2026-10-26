//! Logical owner admission derived from the actual persisted artifact shape.
//!
//! The same `OwnerArtifact` Serde implementation is counted without allocating
//! an artifact body. This does not bound allocator slack or transport internals.

use super::*;

// The pinned serde_json::ser::Formatter writes finite f32 via zmij::Buffer.
// The pinned zmij 1.0.21 formatter has a 24-byte output buffer; no emitted
// finite scalar can exceed its capacity. Re-audit this bound on dependency
// upgrades rather than treating a sample float's length as a universal cap.
const MAX_FINITE_F32_JSON_BYTES: u64 = 24;

#[derive(Default)]
struct CountJson(u64);

impl std::io::Write for CountJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(u64::try_from(bytes.len()).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("semantic artifact JSON length overflow"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn maximal_shape_json_bytes(
    count: usize,
    dimension: usize,
    location: ChunkLocation,
) -> Result<u64, IndexError> {
    let digest = Digest([u8::MAX; 32]);
    let object = ObjectRef(digest);
    let mut values = Vec::new();
    values
        .try_reserve_exact(dimension)
        .map_err(|_error| IndexError::ResourceLimited)?;
    values.resize(dimension, 0.0);
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    chunks.resize(
        count,
        ChunkMember {
            id: digest,
            location,
            input_digest: digest,
        },
    );
    let mut vectors = Vec::new();
    vectors
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for _member in 0..count {
        vectors.push(EmbeddingVector {
            // Both chunk bridges issue a 32-byte digest as the vector ID.
            id: vec![u8::MAX; 32],
            values: values.clone(),
        });
    }
    let artifact = OwnerArtifact {
        owner: OwnerKey {
            producer: ProducerId([u8::MAX; 16]),
            stable_id: quanta_index_core::index::StableOwnerId([u8::MAX; 16]),
        },
        input: OwnerInputBinding {
            source: Some(object),
            projection: Some(object),
            producer_context: digest,
            derivation_policy: digest,
        },
        recipe: digest,
        payload_ref: object,
        reuse_identity: Some(digest),
        chunks,
        vectors,
    };
    let mut writer = CountJson::default();
    serde_json::to_writer(&mut writer, &artifact).map_err(|error| {
        IndexError::BackendViolation(format!("semantic artifact bound count: {error}"))
    })?;
    Ok(writer.0)
}

fn validate_shape(
    dimension: u32,
    max_vectors: u32,
    max_source_bytes: u64,
) -> Result<(), IndexError> {
    let cells = u64::from(dimension)
        .checked_mul(u64::from(max_vectors))
        .ok_or(IndexError::ResourceLimited)?;
    if dimension == 0
        || dimension > MAX_DIMENSION
        || max_vectors == 0
        || max_vectors > MAX_VECTORS
        || cells > MAX_CELLS
        || max_source_bytes == 0
        || max_source_bytes > MAX_SOURCE_BYTES
    {
        return Err(IndexError::InvalidInput(
            "semantic shape exceeds exact owner limits".into(),
        ));
    }
    Ok(())
}

/// Maximum encoded owner-artifact JSON for a chunk shape under the currently
/// pinned `OwnerArtifact` Serde shape and finite-f32 bridge contract.
///
/// The two `ChunkLocation` variants are both counted; the larger one-member
/// body and larger additional-member delta cover every mixed ordering. All
/// fixed byte arrays use 255 (the longest decimal element). Coordinates use
/// `0.0` for counting and add the formatter capacity difference per cell.
pub fn max_chunk_owner_artifact_json_bytes(
    dimension: u32,
    max_vectors: u32,
) -> Result<u64, IndexError> {
    validate_shape(dimension, max_vectors, 1)?;
    let count = u64::from(max_vectors);
    let shapes = [
        ChunkLocation::PayloadBytes {
            start: u64::MAX,
            end: u64::MAX,
        },
        ChunkLocation::RenderedMember { ordinal: u32::MAX },
    ];
    let mut first = 0u64;
    let mut additional = 0u64;
    let mut coordinate = 0u64;
    for shape in shapes {
        let one = maximal_shape_json_bytes(1, 1, shape)?;
        let two = maximal_shape_json_bytes(2, 1, shape)?;
        let two_coordinates = maximal_shape_json_bytes(1, 2, shape)?;
        first = first.max(one);
        additional = additional.max(two.checked_sub(one).ok_or(IndexError::ResourceLimited)?);
        coordinate = coordinate.max(
            two_coordinates
                .checked_sub(one)
                .ok_or(IndexError::ResourceLimited)?,
        );
    }
    let zero_json = u64::try_from(
        serde_json::to_string(&0.0f32)
            .map_err(|error| IndexError::BackendViolation(format!("semantic f32 count: {error}")))?
            .len(),
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let scalar_slack = MAX_FINITE_F32_JSON_BYTES
        .checked_sub(zero_json)
        .ok_or(IndexError::ResourceLimited)?;
    first
        .checked_add(
            count
                .checked_sub(1)
                .and_then(|members| members.checked_mul(additional))
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| {
            count
                .checked_mul(u64::from(dimension).checked_sub(1)?)
                .and_then(|cells| cells.checked_mul(coordinate))
                .and_then(|extra| bytes.checked_add(extra))
        })
        .and_then(|bytes| {
            count
                .checked_mul(u64::from(dimension))
                .and_then(|cells| cells.checked_mul(scalar_slack))
                .and_then(|slack| bytes.checked_add(slack))
        })
        .ok_or(IndexError::ResourceLimited)
}

/// Add the Semantic owner and prior-reconciliation backing to the caller's
/// bounded provider batch.
///
/// `provider_peak`/`provider_escape` must already include every request,
/// response, and changed coordinate at `max_vectors`.
/// The returned pair is a logical reservation requirement, not an RSS claim.
pub fn chunk_owner_memory_envelope(
    dimension: u32,
    max_vectors: u32,
    max_source_bytes: u64,
    provider_peak: u64,
    provider_escape: u64,
) -> Result<(u64, u64), IndexError> {
    validate_shape(dimension, max_vectors, max_source_bytes)?;
    let count = u64::from(max_vectors);
    let encoded = max_chunk_owner_artifact_json_bytes(dimension, max_vectors)?;
    let member = u64::try_from(std::mem::size_of::<ChunkMember>())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let vector = u64::try_from(std::mem::size_of::<EmbeddingVector>())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let digest_index = u64::try_from(std::mem::size_of::<(Digest, usize)>())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let text_ref =
        u64::try_from(std::mem::size_of::<&str>()).map_err(|_error| IndexError::ResourceLimited)?;
    let prior_position = u64::try_from(std::mem::size_of::<Option<usize>>())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let coordinates = quanta_index_core::TextEmbeddingBatch::output_bytes(
        usize::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
        usize::try_from(dimension).map_err(|_error| IndexError::ResourceLimited)?,
    )
    .map_err(|error| IndexError::InvalidInput(format!("semantic coordinate bound: {error}")))?;
    // The immutable object carries encoded bytes. reserve_json_decode then
    // admits exactly 8 * encoded for decoded prior vectors and parser scratch.
    let prior = encoded.checked_mul(9).ok_or(IndexError::ResourceLimited)?;
    // Both the owner planner and bridge planner may retain members; each
    // planner also sorts a Digest/position index. ReuseScratch adds its old
    // index and new text/position arrays. A changed/reused batch can retain
    // both coordinate arrays while the final vector backing is assembled.
    let per_member = member
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(digest_index.checked_mul(4)?))
        .and_then(|bytes| bytes.checked_add(text_ref.checked_mul(3)?))
        .and_then(|bytes| bytes.checked_add(prior_position))
        .and_then(|bytes| bytes.checked_add(vector.checked_mul(2)?))
        .and_then(|bytes| bytes.checked_add(32 * 2))
        .ok_or(IndexError::ResourceLimited)?;
    let owner = max_source_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(prior))
        .and_then(|bytes| bytes.checked_add(per_member.checked_mul(count)?))
        .and_then(|bytes| bytes.checked_add(coordinates.checked_mul(2)?))
        .ok_or(IndexError::ResourceLimited)?;
    Ok((
        provider_peak
            .checked_add(owner)
            .ok_or(IndexError::ResourceLimited)?,
        provider_escape
            .checked_add(owner)
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actual_artifact() -> OwnerArtifact {
        let digest = Digest([41; 32]);
        let object = ObjectRef(digest);
        let texts = [(0_u32, "é\r\nβ"), (1, "a\r\nb"), (2, "東京\n")];
        let chunks = texts
            .iter()
            .map(|&(ordinal, text)| {
                let id = Digest(Sha256::digest(text.as_bytes()).into());
                ChunkMember {
                    id,
                    location: if ordinal == 1 {
                        ChunkLocation::PayloadBytes {
                            start: u64::MAX.saturating_sub(7),
                            end: u64::MAX,
                        }
                    } else {
                        ChunkLocation::RenderedMember { ordinal }
                    },
                    input_digest: id,
                }
            })
            .collect::<Vec<_>>();
        let vectors = chunks
            .iter()
            .map(|chunk| EmbeddingVector {
                id: chunk.id.0.to_vec(),
                values: vec![
                    f32::MIN_POSITIVE,
                    -f32::MAX,
                    f32::from_bits(1),
                    0.000_000_000_000_001,
                    1.0,
                    -0.0,
                    f32::MAX,
                ],
            })
            .collect();
        OwnerArtifact {
            owner: OwnerKey {
                producer: ProducerId([42; 16]),
                stable_id: quanta_index_core::index::StableOwnerId([43; 16]),
            },
            input: OwnerInputBinding {
                source: Some(object),
                projection: Some(object),
                producer_context: Digest([44; 32]),
                derivation_policy: Digest([45; 32]),
            },
            recipe: Digest([46; 32]),
            payload_ref: object,
            reuse_identity: Some(Digest([47; 32])),
            chunks,
            vectors,
        }
    }

    #[test]
    fn serialized_mixed_unicode_crlf_members_fit_independent_shape_bound() -> Result<(), IndexError>
    {
        let artifact = actual_artifact();
        let encoded = serde_json::to_vec(&artifact)
            .map_err(|error| IndexError::BackendViolation(format!("fixture JSON: {error}")))?;
        let maximum = max_chunk_owner_artifact_json_bytes(7, 3)?;
        if u64::try_from(encoded.len()).map_err(|_error| IndexError::ResourceLimited)? > maximum {
            return Err(IndexError::BackendViolation(
                "actual mixed chunk artifact exceeded structural JSON bound".into(),
            ));
        }
        for value in artifact
            .vectors
            .iter()
            .flat_map(|vector| vector.values.iter())
        {
            let length = serde_json::to_string(value)
                .map_err(|error| IndexError::BackendViolation(format!("fixture f32: {error}")))?
                .len();
            if u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_FINITE_F32_JSON_BYTES
            {
                return Err(IndexError::BackendViolation(
                    "finite f32 escaped formatter capacity".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn shape_caps_and_prior_decode_allowance_refuse_overflow() -> Result<(), IndexError> {
        for (dimension, vectors) in [(4096, 48), (48, 4096)] {
            let encoded = max_chunk_owner_artifact_json_bytes(dimension, vectors)?;
            let (peak, escape) =
                chunk_owner_memory_envelope(dimension, vectors, MAX_SOURCE_BYTES, 0, 0)?;
            let prior = encoded.checked_mul(9).ok_or(IndexError::ResourceLimited)?;
            if peak < prior || escape < prior {
                return Err(IndexError::BackendViolation(
                    "prior artifact decode allowance was omitted".into(),
                ));
            }
        }
        for (dimension, vectors) in [(0, 1), (1, 0), (4097, 1), (1, 4097), (4096, 49)] {
            if !matches!(
                max_chunk_owner_artifact_json_bytes(dimension, vectors),
                Err(IndexError::InvalidInput(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "invalid semantic shape was admitted".into(),
                ));
            }
        }
        for source in [0, MAX_SOURCE_BYTES.saturating_add(1)] {
            if !matches!(
                chunk_owner_memory_envelope(7, 3, source, 0, 0),
                Err(IndexError::InvalidInput(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "invalid semantic source bound was admitted".into(),
                ));
            }
        }
        if !matches!(
            chunk_owner_memory_envelope(7, 3, 1024, u64::MAX, 0),
            Err(IndexError::ResourceLimited)
        ) {
            return Err(IndexError::BackendViolation(
                "aggregate provider peak overflow was admitted".into(),
            ));
        }
        Ok(())
    }
}
