//! Adapter to the one signed-installation verifier. No trust JSON or crypto
//! is decoded here. The verifier's token and original grants enclose publish.

use anyhow::{Result, anyhow, ensure};
use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_searchd::cli::NativeRecoveryCommandV1;
use quanta_installed_artifact_trust::{
    InstalledArtifactTrustAdmissionV1, with_verified_installed_server_peer_v1,
};

use crate::deployment_selection::DeploymentSelectionErrorV1 as Error;

struct Admission<'a> {
    budget: &'a mut WorkBudget,
}

impl InstalledArtifactTrustAdmissionV1 for Admission<'_> {
    type RefusalV1 = IndexError;
    type MemoryLeaseV1 = MemoryLease;

    fn checkpoint_v1(&mut self) -> std::result::Result<(), IndexError> {
        self.budget.checkpoint()
    }
    fn charge_io_v1(&mut self, operations: u64) -> std::result::Result<(), IndexError> {
        self.budget.charge(WorkUsage {
            reads: operations,
            ..WorkUsage::default()
        })
    }
    fn charge_read_v1(&mut self, bytes: u64) -> std::result::Result<(), IndexError> {
        self.budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })
    }
    fn charge_hash_v1(&mut self, bytes: u64) -> std::result::Result<(), IndexError> {
        self.budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })
    }
    fn reserve_memory_v1(&mut self, bytes: usize) -> std::result::Result<MemoryLease, IndexError> {
        self.budget
            .reserve_memory(
                u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)
    }
}

pub(super) fn with_installed_parent_v1<T>(
    peer: &super::peer::ObservedParentV1,
    command: &NativeRecoveryCommandV1,
    budget: &mut WorkBudget,
    execute: impl FnOnce(&mut WorkBudget) -> std::result::Result<T, Error>,
) -> std::result::Result<T, Error> {
    let mut admission = Admission { budget };
    // The canonical verifier performs fallible verification before entering
    // this callback. No post-callback failure may be flattened into Before.
    with_verified_installed_server_peer_v1(&peer.image, &mut admission, |verified, admission| {
        let binding = || -> Result<()> {
            ensure!(
                command.parent_executable == verified.server_path_v1(),
                "parent executable differs from the signed installed server"
            );
            ensure!(
                &command.parent_executable_sha256.0 == verified.server_image_sha256_bytes_v1(),
                "parent digest differs from the signed installed server"
            );
            ensure!(
                verified.generation_v1() > 0,
                "installed parent has no active generation"
            );
            Ok(())
        };
        binding().map_err(Error::RefusedBeforeRename)?;
        execute(admission.budget)
    })
    .map_err(|error| Error::RefusedBeforeRename(anyhow!(error)))?
}
