//! Pure length and digest comparison shared by existing-root verification and
//! the persisted sealed-generation reader. IO admission belongs to callers.

use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SidecarCommitmentMismatch {
    Length,
    Digest,
}

pub(crate) fn verify_measurement(
    measured: (u64, [u8; 32]),
    committed: (u64, [u8; 32]),
) -> Result<(), SidecarCommitmentMismatch> {
    if measured.0 != committed.0 {
        return Err(SidecarCommitmentMismatch::Length);
    }
    if measured.1 != committed.1 {
        return Err(SidecarCommitmentMismatch::Digest);
    }
    Ok(())
}

pub(crate) fn verify_bytes(
    bytes: &[u8],
    committed: (u64, [u8; 32]),
) -> Result<(), SidecarCommitmentMismatch> {
    let length = u64::try_from(bytes.len()).map_err(|_| SidecarCommitmentMismatch::Length)?;
    if length != committed.0 {
        return Err(SidecarCommitmentMismatch::Length);
    }
    verify_measurement((length, Sha256::digest(bytes).into()), committed)
}
