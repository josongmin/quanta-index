//! Byte limits and readers shared by Semantic control-file producers and consumers.

mod plain;
pub(crate) mod product;
pub(crate) mod receipt;

pub(crate) use crate::legacy::limits::{
    MAX_GENERATION_CONTRACT_BYTES, MAX_RECEIPT_BYTES, MAX_SCOPE_MANIFEST_BYTES,
    MAX_SEALED_MANIFEST_BYTES, MAX_SEALED_MARKER_BYTES, ensure_bounded,
};
use plain::{measure_bounded, read_bounded};
pub(crate) use product::{open_dataset_reader, regular_file_present};

use std::{io, path::Path};

use quanta_index_core::publication_admission::PublicationObjectLifetimePort;

pub(crate) fn read_bounded_for_generation(
    path: &Path,
    max_bytes: usize,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> io::Result<Vec<u8>> {
    match lifetime {
        Some(lifetime) => product::read_bounded_with_control_lifetime(path, max_bytes, lifetime),
        None => read_bounded(path, max_bytes),
    }
}

pub(crate) fn read_string_for_generation(
    path: &Path,
    max_bytes: usize,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> io::Result<String> {
    String::from_utf8(read_bounded_for_generation(path, max_bytes, lifetime)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(crate) fn measure_bounded_for_generation(
    path: &Path,
    max_bytes: usize,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> io::Result<(u64, [u8; 32])> {
    match lifetime {
        Some(lifetime) => product::measure_bounded_with_control_lifetime(path, max_bytes, lifetime),
        None => measure_bounded(path, max_bytes),
    }
}

#[cfg(test)]
use std::fs::File;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_sparse_oversized_file_before_allocation() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let path = generation.join("oversized");
        let file = File::create(&path)?;
        file.set_len(
            u64::try_from(MAX_RECEIPT_BYTES)?
                .checked_add(1)
                .ok_or("overflow")?,
        )?;
        let error = read_bounded(&path, MAX_RECEIPT_BYTES).expect_err("oversized file");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("oversized file answered {error}").into());
        }
        let error = measure_bounded(&path, MAX_RECEIPT_BYTES)
            .expect_err("oversized file must not be hashed");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("oversized hash answered {error}").into());
        }
        Ok(())
    }

    #[test]
    fn refuses_a_control_file_symlink() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let outside = temp.path().join("outside");
        std::fs::write(&outside, b"outside")?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let link = generation.join("sealed-marker");
        std::os::unix::fs::symlink(&outside, &link)?;
        let error = read_bounded(&link, MAX_SEALED_MARKER_BYTES)
            .expect_err("control-file symlink must be refused");
        if error.kind() != io::ErrorKind::InvalidData || regular_file_present(&link).is_ok() {
            return Err(
                format!("control-file symlink was not classified as invalid: {error}").into(),
            );
        }
        let error = measure_bounded(&link, MAX_SEALED_MARKER_BYTES)
            .expect_err("symlink must not be hashed");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("symlink hash answered {error}").into());
        }
        Ok(())
    }

    #[test]
    fn dangling_symlink_is_present_but_invalid() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let marker = generation.join("SEALED");
        std::os::unix::fs::symlink(temp.path().join("missing"), &marker)?;
        let error = regular_file_present(&marker).expect_err("dangling marker is not absent");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("dangling marker answered {error}").into());
        }
        Ok(())
    }

    #[test]
    fn refuses_a_family_symlink_before_opening_a_control_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let track = temp.path().join("track");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&track)?;
        std::fs::create_dir_all(outside.join("g1"))?;
        std::fs::write(outside.join("g1/SEALED"), b"outside")?;
        std::os::unix::fs::symlink(&outside, track.join("family"))?;
        let marker = track.join("family/g1/SEALED");
        if read_bounded(&marker, MAX_SEALED_MARKER_BYTES).is_ok()
            || regular_file_present(&marker).is_ok()
        {
            return Err("semantic control read followed a family symlink".into());
        }
        Ok(())
    }
}
