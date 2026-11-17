//! Existing-only Semantic physical verification through the caller's actual
//! root-bound metadata/read ports and ORIGINAL WorkBudget. No engine is opened.

use super::{layout, limits, with_decoded_generation_data};
use quanta_index_contract::{GenerationSnapshot, SearchPlaneErrorCodeV2, SemanticContentRootsV1};
use quanta_index_core::{
    CoreError, ExistingTreeMetadataIoPortV1, SealedArtifactCommitmentV1,
    index::{
        IndexError, MemoryLifetime, ObjectReadIoPort, WorkBudget,
        existing_file::{
            ExistingFileBytesV1, ExistingFileReadFailureV1, read_existing_file_v1,
            verify_existing_committed_files_v1,
        },
    },
    verify_tree_layout_admitted_v1,
};
use std::path::Path;

type Failure = ExistingFileReadFailureV1;

fn corrupt(message: impl Into<String>) -> Failure {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: message.into(),
    }
    .into()
}

fn memory(
    budget: &WorkBudget,
    bytes: u64,
) -> Result<quanta_index_core::index::MemoryLease, Failure> {
    budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited.into())
}

fn width(bytes: usize) -> Result<u64, Failure> {
    u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited.into())
}

fn sidecars(
    generation: &Path,
    owner: u32,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<[ExistingFileBytesV1; 4], Failure> {
    let mut read = |name: &str, bound: usize| {
        // The enclosing verifier admits its path/control frame before this
        // join. The shared leaf retains the original bytes/decode grant.
        read_existing_file_v1(&generation.join(name), owner, width(bound)?, io, budget)
    };
    Ok([
        read(
            layout::MARKER_SEALED_FILE_NAME,
            limits::MAX_SEALED_MARKER_BYTES,
        )?,
        read(
            layout::SEALED_MANIFEST_FILE_NAME,
            limits::MAX_SEALED_MANIFEST_BYTES,
        )?,
        read(layout::MANIFEST_FILE_NAME, limits::MAX_SCOPE_MANIFEST_BYTES)?,
        read(
            layout::BUILD_CONTRACT_FILE_NAME,
            limits::MAX_GENERATION_CONTRACT_BYTES,
        )?,
    ])
}

fn refuse_quarantine(
    generation: &Path,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<(), Failure> {
    metadata.visit_directory_v1(generation, budget, &mut |name, _, _, budget| {
        budget.checkpoint()?;
        if name == std::ffi::OsStr::new(layout::QUARANTINE_RECEIPT_FILE_NAME) {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::GenerationQuarantined,
                message: "semantic generation has a retained quarantine receipt".into(),
            }
            .into());
        }
        Ok(())
    })
}

fn with_admitted_decoded_data<T>(
    generation: &Path,
    expected: &GenerationSnapshot,
    inputs: [&[u8]; 4],
    budget: &mut WorkBudget,
    use_data: impl FnOnce(
        &[SealedArtifactCommitmentV1],
        &SemanticContentRootsV1,
        &mut WorkBudget,
    ) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let [marker, seal, scope, contract] = inputs;
    let input_bytes = inputs
        .iter()
        .try_fold(0_u64, |total, input| -> Result<u64, Failure> {
            total
                .checked_add(width(input.len())?)
                .ok_or(IndexError::ResourceLimited.into())
        })?;
    // Existing typed shapes have eight Vec backings including the seal-row
    // conversion, each capped at 1MiB of header-driven Serde preallocation.
    // The separate byte term covers ordinary Vec growth, strings, temporary
    // rows and corpus BTreeSet nodes. This is logical allocation backing,
    // not RSS, and is additional to the shared read helper's own grant.
    // Its exact current-shape bound still requires the owning source review
    // and real malformed-header regression before qualification.
    let decode_bytes = input_bytes
        .checked_mul(256)
        .and_then(|n| n.checked_add(8 * 1024 * 1024))
        .ok_or(IndexError::ResourceLimited)?;
    let _decode = memory(budget, decode_bytes)?;
    let marker =
        std::str::from_utf8(marker).map_err(|_| corrupt("semantic sealed marker is not UTF-8"))?;
    budget.checkpoint()?;
    // Reuse ONE owner parser and all its existing sidecar-before-interpretation,
    // current-format/scope/model/policy checks. No alternate codec or DTO.
    with_decoded_generation_data(
        generation,
        expected,
        marker,
        seal,
        scope,
        contract,
        |committed, roots| {
            budget.checkpoint()?;
            use_data(committed, &roots, budget)
        },
    )
}

/// Verify a selected existing generation's recorded identity, exact dataset
/// file set and every committed payload byte. `generation` and both actual
/// ports must be bound to D's SAME held Legacy root/incarnation, and `expected`
/// must come from the genuine Catalog/Source/Activation relation.
///
/// This lends recorded roots only AFTER physical verification. It does not
/// recompute Lance row roots, validate a live index/schema, create a Source or
/// publication permission, or qualify installed recovery. The caller retains
/// the root and witnesses all physical read-close retirement before publishing.
/// `use_roots` executes while all decoded DATA still has its original memory
/// backing. It must fund any owned output/clones itself; borrowing the roots
/// grants no Source/publication authority. No owned unbacked roots escape here.
pub fn verify_existing_semantic_generation_read_only<T>(
    generation: &Path,
    expected: &GenerationSnapshot,
    owner: u32,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
    use_roots: impl FnOnce(
        &SemanticContentRootsV1,
        &mut WorkBudget,
    ) -> Result<T, ExistingFileReadFailureV1>,
) -> Result<T, ExistingFileReadFailureV1> {
    budget.checkpoint()?;
    if expected.track != quanta_index_contract::SearchPlaneTrackKind::Semantic {
        return Err(CoreError::InvalidContract(
            "semantic verifier received a foreign track".into(),
        )
        .into());
    }
    // At most eight path/control/diagnostic strings overlap. Canonical file
    // names fit in the fixed suffix allowance. No filesystem effect precedes
    // this original allocation admission.
    let frame_bytes = width(generation.as_os_str().len())?
        .checked_add(4096)
        .and_then(|n| n.checked_mul(8))
        .ok_or(IndexError::ResourceLimited)?;
    let _frame = memory(budget, frame_bytes)?;
    refuse_quarantine(generation, metadata, budget)?;
    let inputs = sidecars(generation, owner, io, budget)?;
    let [marker, seal, scope, contract] = &inputs;
    with_admitted_decoded_data(
        generation,
        expected,
        [
            marker.bytes_v1(),
            seal.bytes_v1(),
            scope.bytes_v1(),
            contract.bytes_v1(),
        ],
        budget,
        |committed, roots, budget| {
            budget.checkpoint()?;
            let dataset = layout::dataset_dir(generation);
            let before = verify_tree_layout_admitted_v1(
                &dataset,
                layout::DATASET_DIR_NAME,
                committed,
                metadata,
                budget,
            )?;
            if let Err(mismatch) = before.outcome_v1() {
                return Err(corrupt(mismatch.to_string()));
            }
            verify_existing_committed_files_v1(
                generation, "dataset/", committed, owner, io, budget,
            )?;
            let after = verify_tree_layout_admitted_v1(
                &dataset,
                layout::DATASET_DIR_NAME,
                committed,
                metadata,
                budget,
            )?;
            if let Err(mismatch) = after.outcome_v1() {
                return Err(corrupt(mismatch.to_string()));
            }
            // Preserve the original sidecar observations across the full
            // content interval. Shared readers perform their own fd/path checks.
            let repeated = sidecars(generation, owner, io, budget)?;
            for (original, current) in inputs.iter().zip(&repeated) {
                budget.checkpoint()?;
                if original.bytes_v1() != current.bytes_v1() {
                    return Err(corrupt(
                        "semantic sidecar changed during content verification",
                    ));
                }
            }
            refuse_quarantine(generation, metadata, budget)?;
            budget.checkpoint()?;
            // Both layout observations and their original allocation grants
            // survive the content/sidecar interval. These are DATA, not permits.
            let result = use_roots(roots, budget)?;
            budget.checkpoint()?;
            drop((after, before));
            Ok(result)
        },
    )
}

#[cfg(test)]
mod tests;
