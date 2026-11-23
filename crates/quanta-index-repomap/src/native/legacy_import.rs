//! Read a provisional, page-backed Legacy RepoMap compiler payload from the
//! same immutable object authority used by Native projection builders.

mod build;
mod query;
pub use build::{
    NATIVE_LEGACY_REPOMAP_FORMAT_V1, NATIVE_LEGACY_REPOMAP_KIND_V1,
    NATIVE_LEGACY_REPOMAP_SCHEMA_V1, NativeLegacyRepoMapBuildV1,
};
pub use query::{LoadedLegacyRepoMapRowV1, NativeLegacyRepoMapReadV1};

use quanta_index_contract::index::{
    Digest, InputDomain, InstanceSpec, LegacyRepoMapImportV1,
    MAX_LEGACY_REPOMAP_IMPORT_PAYLOAD_BYTES_V1, MemberBinding, ObjectRef, SourceCut,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, StoredObject, WorkBudget, WorkUsage,
    ports::ImmutableObjectPort,
};
use sha2::{Digest as _, Sha256};

/// Minimum simultaneous Original credit for opening an imported member.
/// The root/header decoders stay live while the retained payload and its
/// canonical streaming validator are admitted. Backend object-read charges
/// and query-specific result charges remain additional runtime checks.
const IMPORT_HEADER_DECODE_CREDIT_V1: u64 = 65_536;
const PAYLOAD_DECODE_FIXED_CREDIT_V1: u64 = 65_536;

fn root_decode_credit_v1() -> Result<u64, IndexError> {
    u64::try_from(quanta_index_core::index::MAX_BINDING_METADATA_BYTES)
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(8)
        .ok_or(IndexError::ResourceLimited)
}

fn minimum_query_open_credit_v1(payload_bytes: u64) -> Result<u64, IndexError> {
    let root_decode = root_decode_credit_v1()?;
    payload_bytes
        .checked_mul(9)
        .and_then(|bytes| bytes.checked_add(PAYLOAD_DECODE_FIXED_CREDIT_V1))
        .and_then(|bytes| bytes.checked_add(IMPORT_HEADER_DECODE_CREDIT_V1))
        .and_then(|bytes| bytes.checked_add(root_decode))
        .ok_or(IndexError::ResourceLimited)
}

/// The raw payload and its allocation charge have the same lifetime. The
/// caller still needs the old-root candidate/ACK join and a certified Native
/// projection transition before these bytes can become query-visible.
pub struct VerifiedLegacyRepoMapImportV1 {
    pub header: LegacyRepoMapImportV1,
    payload: Vec<u8>,
    _payload_memory: Option<MemoryLease>,
}

impl VerifiedLegacyRepoMapImportV1 {
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Revisit the same canonical rows for each deterministic persistent-map
    /// column without materializing all graph/projection tables at once.
    pub fn visit_rows_v1(
        &self,
        budget: &mut WorkBudget,
        mut visit: impl FnMut(
            crate::materializer::CompiledPayloadEntry,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let decode_allowance = self
            .header
            .payload_bytes
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(65_536))
            .ok_or(IndexError::ResourceLimited)?;
        let _decode_memory = budget.reserve_memory(decode_allowance, MemoryLifetime::Temporary)?;
        crate::materializer::visit_compiled_payload_with(
            &self.payload,
            |error| IndexError::CorruptObject(error.to_string()),
            |row| {
                budget.checkpoint()?;
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
                visit(row, budget)
            },
        )?;
        budget.checkpoint()
    }
}

pub fn load_verified_legacy_repomap_import_v1(
    header_ref: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<VerifiedLegacyRepoMapImportV1, IndexError> {
    load_verified_legacy_repomap_import_with_get_v1(
        header_ref,
        &mut |reference, budget| objects.get(reference, budget),
        budget,
    )
}

fn load_verified_legacy_repomap_import_with_get_v1(
    header_ref: ObjectRef,
    get: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
) -> Result<VerifiedLegacyRepoMapImportV1, IndexError> {
    let stored = get(header_ref, budget)?;
    if stored.registration.reference != header_ref
        || stored.payload.is_empty()
        || stored.payload.len() > 8192
    {
        return Err(IndexError::CorruptObject(
            "Legacy RepoMap import header identity or size differs".into(),
        ));
    }
    let _header_memory = budget.reserve_memory(65_536, MemoryLifetime::Temporary)?;
    let header: LegacyRepoMapImportV1 = serde_json::from_slice(&stored.payload).map_err(|_| {
        IndexError::CorruptObject("Legacy RepoMap import header decode refused".into())
    })?;
    header
        .verify_children_v1(&stored.registration.children)
        .map_err(|_| {
            IndexError::CorruptObject("Legacy RepoMap import header children differ".into())
        })?;
    let canonical = serde_json::to_vec(&header).map_err(|_| {
        IndexError::CorruptObject("Legacy RepoMap import header encode refused".into())
    })?;
    if canonical != stored.payload {
        return Err(IndexError::CorruptObject(
            "Legacy RepoMap import header encoding differs".into(),
        ));
    }
    let payload_len =
        usize::try_from(header.payload_bytes).map_err(|_| IndexError::ResourceLimited)?;
    if header.payload_bytes > MAX_LEGACY_REPOMAP_IMPORT_PAYLOAD_BYTES_V1 {
        return Err(IndexError::ResourceLimited);
    }
    // Refuse before reconstructing pages if this read admission cannot open
    // the complete imported member under its own Original memory scope.
    let root_decode = root_decode_credit_v1()?;
    let preflight_credit = minimum_query_open_credit_v1(header.payload_bytes)?
        .checked_sub(IMPORT_HEADER_DECODE_CREDIT_V1)
        .and_then(|bytes| bytes.checked_sub(root_decode))
        .ok_or(IndexError::ResourceLimited)?;
    let preflight = budget.reserve_memory(preflight_credit, MemoryLifetime::Temporary)?;
    drop(preflight);
    let payload_memory = budget.reserve_memory(header.payload_bytes, MemoryLifetime::Retained)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(payload_len)
        .map_err(|_| IndexError::ResourceLimited)?;
    if payload.capacity() > payload_len {
        return Err(IndexError::ResourceLimited);
    }
    let mut hash = Sha256::new();
    for page in &header.pages {
        budget.checkpoint()?;
        let object = get(page.object, budget)?;
        if object.registration.reference != page.object
            || !object.registration.children.is_empty()
            || object.payload.len()
                != usize::try_from(page.bytes).map_err(|_| IndexError::ResourceLimited)?
        {
            return Err(IndexError::CorruptObject(
                "Legacy RepoMap import page identity or length differs".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::from(page.bytes),
            ..WorkUsage::default()
        })?;
        hash.update(&object.payload);
        payload.extend_from_slice(&object.payload);
    }
    if payload.len() != payload_len || Digest(hash.finalize().into()) != header.payload_sha256 {
        return Err(IndexError::CorruptObject(
            "Legacy RepoMap import payload commitment differs".into(),
        ));
    }
    // The canonical decoder emits one row at a time. Its temporary nested
    // identities/strings must be admitted before decoding any untrusted row.
    let decode_allowance = header
        .payload_bytes
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(65_536))
        .ok_or(IndexError::ResourceLimited)?;
    let _decode_memory = budget.reserve_memory(decode_allowance, MemoryLifetime::Temporary)?;
    crate::materializer::validate_compiled_payload(&payload).map_err(|_| {
        IndexError::CorruptObject("Legacy RepoMap import compiler payload refused".into())
    })?;
    budget.checkpoint()?;
    Ok(VerifiedLegacyRepoMapImportV1 {
        header,
        payload,
        _payload_memory: payload_memory,
    })
}

fn load_verified_legacy_repomap_member_with_get_v1(
    member: &MemberBinding,
    expected_source: Option<SourceCut>,
    get: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
) -> Result<(build::LegacyImportRootV1, VerifiedLegacyRepoMapImportV1), IndexError> {
    let stored = get(member.artifact, budget)?;
    if stored.registration.reference != member.artifact
        || stored.payload.len() > quanta_index_core::index::MAX_BINDING_METADATA_BYTES
        || stored.payload.is_empty()
    {
        return Err(IndexError::CorruptObject(
            "Legacy RepoMap member root differs".into(),
        ));
    }
    let _root_memory = budget.reserve_memory(
        u64::try_from(
            stored
                .payload
                .len()
                .checked_mul(8)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let root: build::LegacyImportRootV1 = serde_json::from_slice(&stored.payload)
        .map_err(|_| IndexError::CorruptObject("Legacy RepoMap root decode refused".into()))?;
    if serde_json::to_vec(&root).ok().as_deref() != Some(stored.payload.as_slice())
        || stored.registration.children != root.children()
        || root.input != member.input
        || member.input.root != root.projection.owners
        || member.input.schema != root.projection.schema
        || member.input.context != root.projection.materialization_context()
        || expected_source.is_some_and(|source| root.projection.source != source)
        || Digest(Sha256::digest(&stored.payload).into()) != member.logical_commitment
    {
        return Err(IndexError::CorruptObject(
            "Legacy RepoMap root binding differs".into(),
        ));
    }
    let import = load_verified_legacy_repomap_import_with_get_v1(root.header, get, budget)?;
    Ok((root, import))
}

/// Lend a checked imported RepoMap header while the complete old compiler
/// payload and its Original memory grant remain live. The caller must still
/// join the header scope and digest to the independently held Legacy candidate.
pub fn with_verified_native_legacy_repomap_member_v1<T>(
    member: &MemberBinding,
    instance: &InstanceSpec,
    expected_source: SourceCut,
    mut get: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&LegacyRepoMapImportV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    build::NativeLegacyRepoMapBuildV1::validate_profile(instance)?;
    if member.instance != instance.instance
        || member.format != instance.format
        || member.input.schema != instance.schema
        || member.input.policy != instance.policy
    {
        return Err(IndexError::DigestConflict);
    }
    let (root, import) = load_verified_legacy_repomap_member_with_get_v1(
        member,
        Some(expected_source),
        &mut get,
        budget,
    )?;
    if instance.input_domain
        != (InputDomain::Projection {
            projection: root.projection.projection,
            producer: root.projection.producer,
        })
    {
        return Err(IndexError::DigestConflict);
    }
    visit(&import.header, budget)
}
