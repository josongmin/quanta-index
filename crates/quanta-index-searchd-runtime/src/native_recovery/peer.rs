//! Kernel observation only. Installed-product trust must authenticate this
//! exact opened image before any Source evidence can authorize publication.

use anyhow::Result;
use quanta_index_core::index::WorkBudget;
use std::{fs::File, os::unix::net::UnixDatagram};

pub(super) struct ObservedParentV1 {
    pub(super) image: File,
}

/// A descriptor-launched helper may execute a sealed memfd with a different
/// inode from the configured image. Authenticate its actual kernel image once
/// against the already locked selection's digest and retain the opened File.
#[cfg(target_os = "linux")]
pub(super) fn retain_running_image_v1(
    expected: quanta_index_contract::index::Digest,
    budget: &mut WorkBudget,
) -> Result<File> {
    use anyhow::ensure;
    use sha2::{Digest as _, Sha256};
    use std::os::unix::fs::FileExt;
    super::custody::charge(budget, 2, 0)?;
    let image = File::open("/proc/self/exe")?;
    let metadata = image.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.len() > 0
            && metadata.len() <= quanta_explicit_fd_process::MAX_PINNED_EXECUTABLE_BYTES_V1,
        "running recovery image exceeds the explicit process image bound"
    );
    let mut buffer = [0; 64 * 1024];
    let mut offset = 0_u64;
    let mut hash = Sha256::new();
    while offset < metadata.len() {
        let remaining = usize::try_from((metadata.len() - offset).min(buffer.len() as u64))?;
        super::custody::charge(budget, 1, remaining as u64)?;
        let count = image.read_at(&mut buffer[..remaining], offset)?;
        ensure!(
            count > 0,
            "running recovery image ended before its physical length"
        );
        hash.update(&buffer[..count]);
        offset += u64::try_from(count)?;
    }
    super::custody::charge(budget, 1, 1)?;
    let mut trailing = [0];
    ensure!(
        image.read_at(&mut trailing, offset)? == 0
            && quanta_index_contract::index::Digest(hash.finalize().into()) == expected,
        "running recovery image differs from the actual selected Q image"
    );
    budget.checkpoint()?;
    Ok(image)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn retain_running_image_v1(
    _expected: quanta_index_contract::index::Digest,
    _budget: &mut WorkBudget,
) -> Result<File> {
    anyhow::bail!("recovery requires the actual kernel executable descriptor")
}

#[cfg(target_os = "linux")]
pub(super) fn observe_parent_v1(
    control: &UnixDatagram,
    budget: &mut WorkBudget,
) -> Result<ObservedParentV1> {
    use anyhow::{anyhow, ensure};
    super::custody::charge(budget, 3, 0)?;
    let peer = rustix::net::sockopt::socket_peercred(control)?;
    let parent =
        rustix::process::getppid().ok_or_else(|| anyhow!("helper has no resident parent"))?;
    ensure!(
        peer.pid == parent && peer.uid == rustix::process::geteuid(),
        "recovery control peer is not the actual same-UID spawning resident"
    );
    let pid = u32::try_from(peer.pid.as_raw_nonzero().get())?;
    // This is intentionally a kernel executable link, never an argv-selected
    // filesystem image. Its opened identity is passed to the canonical signed
    // installation verifier; a matching self-reported digest is insufficient.
    let image = File::open(format!("/proc/{pid}/exe"))?;
    anyhow::ensure!(
        image.metadata()?.is_file(),
        "parent executable is not a regular image"
    );
    budget.checkpoint()?;
    Ok(ObservedParentV1 { image })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn observe_parent_v1(
    _control: &UnixDatagram,
    _budget: &mut WorkBudget,
) -> Result<ObservedParentV1> {
    anyhow::bail!("installed recovery requires a kernel-reported parent executable identity")
}
