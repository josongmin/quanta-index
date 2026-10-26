//! Certified producer text members, without JSON restoration or inferred ASTs.

use quanta_index_contract::native_protocol::{
    NativeSemanticChunkManifest, NativeSemanticManifestHeader, NativeSemanticManifestLimits,
    NativeSemanticManifestMember, encode_native_semantic_manifest_into,
    native_semantic_manifest_encoded_len,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        Digest, IndexError, MemoryLease, MemoryLifetime, OwnerInputBinding, OwnerKey, WorkBudget,
        WorkUsage,
    },
};
use sha2::{Digest as _, Sha256};

use super::{
    chunks::{ChunkLocation, ChunkMember, ChunkPlan},
    embedding::{EmbeddingInputPolicy, EmbeddingRecipe},
};

const OUTPUT_POLICY: &[u8] = b"quanta-index-native-semantic-certified-rendered-members-v1\0";

pub(crate) fn output_policy() -> Digest {
    Digest(Sha256::digest(OUTPUT_POLICY).into())
}

/// The encoder keeps allocated wire backing charged until its consumer drops it.
pub struct CertifiedChunkPayload {
    bytes: Vec<u8>,
    _memory: Option<MemoryLease>,
}

impl AsRef<[u8]> for CertifiedChunkPayload {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Encode producer-rendered members for the existing projection-output ingress.
/// This is a payload encoder, not a certificate issuer or publication shortcut.
pub fn encode_certified_manifest(
    header: NativeSemanticManifestHeader,
    members: &[NativeSemanticManifestMember<'_>],
    budget: &mut WorkBudget,
) -> Result<CertifiedChunkPayload, IndexError> {
    budget.checkpoint()?;
    let length =
        native_semantic_manifest_encoded_len(header, members, wire_limits()).map_err(|error| {
            IndexError::InvalidInput(format!("semantic manifest encoding: {error}"))
        })?;
    let bytes = u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    let count = members.len();
    budget.charge(WorkUsage {
        bytes: sorting_work(count)?,
        mutations: u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let scratch_bytes = count
        .checked_mul(std::mem::size_of::<Digest>())
        .ok_or(IndexError::ResourceLimited)?;
    let scratch_memory = budget.reserve_memory(
        u64::try_from(scratch_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut ids = Vec::new();
    ids.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    ids.extend(members.iter().map(|member| member.id));
    check_unique_ids(&mut ids, budget.request())?;
    drop(ids);
    drop(scratch_memory);
    let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
    let mut output = CertifiedChunkPayload {
        bytes: Vec::new(),
        _memory: memory,
    };
    output
        .bytes
        .try_reserve_exact(length)
        .map_err(|_error| IndexError::ResourceLimited)?;
    output.bytes.resize(length, 0);
    encode_native_semantic_manifest_into(header, members, wire_limits(), &mut output.bytes)
        .map_err(|error| {
            IndexError::InvalidInput(format!("semantic manifest encoding: {error}"))
        })?;
    budget.checkpoint()?;
    Ok(output)
}

fn wire_limits() -> NativeSemanticManifestLimits {
    NativeSemanticManifestLimits::MAXIMUM
}

fn checkpoint(request: &RequestBudgetV1) -> Result<(), IndexError> {
    if request.is_cancelled() {
        Err(IndexError::Cancelled)
    } else if std::time::Instant::now() >= request.deadline() {
        Err(IndexError::ResourceLimited)
    } else {
        Ok(())
    }
}

fn sorting_work(count: usize) -> Result<u64, IndexError> {
    let levels = usize::BITS
        .checked_sub(count.saturating_sub(1).leading_zeros())
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(count)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(
            u64::from(levels)
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|value| value.checked_mul(32))
        .ok_or(IndexError::ResourceLimited)
}

fn check_unique_ids(ids: &mut [Digest], request: &RequestBudgetV1) -> Result<(), IndexError> {
    checkpoint(request)?;
    ids.sort_unstable();
    if ids
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left == right))
    {
        return Err(IndexError::InvalidInput(
            "certified manifest repeats a logical member ID".into(),
        ));
    }
    checkpoint(request)
}

/// Read only the canonical manifest selected by the certified owner binding.
pub(crate) fn select_manifest<'a>(
    payload: &'a [u8],
    owner: OwnerKey,
    input: OwnerInputBinding,
    recipe: &EmbeddingRecipe,
) -> Result<NativeSemanticChunkManifest<'a>, IndexError> {
    let EmbeddingInputPolicy::ProjectionChunks {
        projection,
        producer,
        schema,
    } = recipe.input_policy
    else {
        return Err(IndexError::InvalidInput(
            "semantic manifest requires certified chunk policy".into(),
        ));
    };
    if input.projection.is_none()
        || input.derivation_policy != recipe.fingerprint()
        || u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            > recipe.max_source_bytes
    {
        return Err(IndexError::InvalidInput(
            "semantic manifest input binding mismatch".into(),
        ));
    }
    let manifest = NativeSemanticChunkManifest::parse(
        payload,
        NativeSemanticManifestLimits {
            max_members: recipe.max_vectors,
            max_member_utf8_bytes: u32::try_from(recipe.max_source_bytes)
                .map_err(|_error| IndexError::ResourceLimited)?,
        },
    )
    .map_err(|error| IndexError::InvalidInput(format!("semantic manifest: {error}")))?;
    let header = manifest.header();
    if header.owner != owner
        || header.projection != projection
        || header.producer != producer
        || header.schema != schema
        || header.source != input.source
        || header.materialization_context() != input.producer_context
    {
        return Err(IndexError::InvalidInput(
            "semantic manifest certified header mismatch".into(),
        ));
    }
    Ok(manifest)
}

pub(crate) fn plan_within_work(
    payload: &[u8],
    owner: OwnerKey,
    input: OwnerInputBinding,
    recipe: &EmbeddingRecipe,
    budget: &mut WorkBudget,
) -> Result<ChunkPlan, IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let manifest = select_manifest(payload, owner, input, recipe)?;
    let count = manifest.len();
    let comparisons = sorting_work(count)?;
    budget.charge(WorkUsage {
        bytes: comparisons,
        mutations: u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let request = budget.request().clone();
    plan_manifest(manifest, &request, |bytes, lifetime| {
        budget.reserve_memory(bytes, lifetime)
    })
}

pub(crate) fn plan_within_request(
    payload: &[u8],
    owner: OwnerKey,
    input: OwnerInputBinding,
    recipe: &EmbeddingRecipe,
    request: &RequestBudgetV1,
) -> Result<ChunkPlan, IndexError> {
    checkpoint(request)?;
    let manifest = select_manifest(payload, owner, input, recipe)?;
    plan_manifest(manifest, request, |bytes, lifetime| {
        request
            .reserve_provider_memory_v1(bytes, lifetime)
            .map_err(|error| {
                checkpoint(request).err().unwrap_or_else(|| {
                    IndexError::Storage(format!("certified member admission: {error}"))
                })
            })
    })
}

fn plan_manifest(
    manifest: NativeSemanticChunkManifest<'_>,
    request: &RequestBudgetV1,
    mut reserve: impl FnMut(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ChunkPlan, IndexError> {
    checkpoint(request)?;
    let count = manifest.len();
    let bytes = count
        .checked_mul(std::mem::size_of::<ChunkMember>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = reserve(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut plan = ChunkPlan {
        members: Vec::new(),
        memory,
    };
    plan.members
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let scratch_bytes = count
        .checked_mul(std::mem::size_of::<Digest>())
        .ok_or(IndexError::ResourceLimited)?;
    let scratch_memory = reserve(
        u64::try_from(scratch_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    // Data is declared after the lease, so it drops before the charge returns.
    let mut ids = Vec::new();
    ids.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for (ordinal, member) in manifest.iter().enumerate() {
        checkpoint(request)?;
        let member = member.map_err(|error| {
            IndexError::InvalidInput(format!("semantic manifest member: {error}"))
        })?;
        plan.members.push(ChunkMember {
            id: member.id,
            location: ChunkLocation::RenderedMember {
                ordinal: u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
            },
            input_digest: Digest(Sha256::digest(member.text.as_bytes()).into()),
        });
        ids.push(member.id);
    }
    check_unique_ids(&mut ids, request)?;
    drop(ids);
    drop(scratch_memory);
    Ok(plan)
}
