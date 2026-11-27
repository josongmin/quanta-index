//! One bounded, FD-only Darwin directory batch for admitted inventories.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

const RETURNED_SET_BYTES: usize = std::mem::size_of::<libc::attribute_set_t>();
const REFERENCE_BYTES: usize = std::mem::size_of::<libc::attrreference_t>();
const RECORD_PREFIX_BYTES: usize = 4 + RETURNED_SET_BYTES;
// Darwin sys/attr.h defines this, but libc 0.2.186 does not export it.
const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;

#[derive(Debug)]
pub enum DarwinDirectoryVisitErrorV1<E> {
    Io(io::Error),
    Visit(E),
}

fn malformed(detail: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail)
}

fn u32_at(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| malformed("directory record offset overflow"))?;
    let raw: [u8; 4] = bytes
        .get(offset..end)
        .ok_or_else(|| malformed("directory record truncated"))?
        .try_into()
        .map_err(|_| malformed("directory record width invalid"))?;
    Ok(u32::from_ne_bytes(raw))
}

fn u64_at(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| malformed("directory record offset overflow"))?;
    let raw: [u8; 8] = bytes
        .get(offset..end)
        .ok_or_else(|| malformed("directory record truncated"))?
        .try_into()
        .map_err(|_| malformed("directory record width invalid"))?;
    Ok(u64::from_ne_bytes(raw))
}

fn entry_in_record(record: &[u8]) -> io::Result<(&[u8], u64)> {
    if record.len() < RECORD_PREFIX_BYTES {
        return Err(malformed("directory record too short"));
    }
    let returned_common = u32_at(record, 4)?;
    // The error field, when returned, precedes the name reference.
    let mut reference = RECORD_PREFIX_BYTES;
    if returned_common & ATTR_CMN_ERROR != 0 {
        let entry_error = u32_at(record, reference)?;
        if entry_error != 0 {
            return Err(io::Error::from_raw_os_error(
                i32::try_from(entry_error)
                    .map_err(|_| malformed("directory entry error code invalid"))?,
            ));
        }
        reference += 4;
    }
    if returned_common & (libc::ATTR_CMN_NAME | libc::ATTR_CMN_FILEID)
        != (libc::ATTR_CMN_NAME | libc::ATTR_CMN_FILEID)
    {
        return Err(malformed("directory record omitted name or file ID"));
    }
    if record.len() < reference + REFERENCE_BYTES + 8 {
        return Err(malformed("directory record too short"));
    }
    let relative = i32::from_ne_bytes(
        record
            .get(reference..reference + 4)
            .ok_or_else(|| malformed("directory name reference truncated"))?
            .try_into()
            .map_err(|_| malformed("directory name reference invalid"))?,
    );
    let name_len = usize::try_from(u32_at(record, reference + 4)?)
        .map_err(|_| malformed("directory name length invalid"))?;
    let start = reference
        .checked_add_signed(
            isize::try_from(relative).map_err(|_| malformed("directory name offset invalid"))?,
        )
        .ok_or_else(|| malformed("directory name offset overflow"))?;
    let end = start
        .checked_add(name_len)
        .ok_or_else(|| malformed("directory name length overflow"))?;
    if start < reference + REFERENCE_BYTES + 8 || name_len < 2 {
        return Err(malformed("directory name reference invalid"));
    }
    let bytes = record
        .get(start..end)
        .ok_or_else(|| malformed("directory name outside record"))?;
    let (last, name) = bytes
        .split_last()
        .ok_or_else(|| malformed("directory name empty"))?;
    if *last != 0
        || name.is_empty()
        || name == b"."
        || name == b".."
        || name.contains(&0)
        || name.contains(&b'/')
    {
        return Err(malformed("directory name malformed"));
    }
    let file_id = u64_at(record, reference + REFERENCE_BYTES)?;
    if file_id == 0 {
        return Err(malformed("directory entry file ID is zero"));
    }
    Ok((name, file_id))
}

/// Read and visit one kernel batch using the caller's already-owned directory FD.
///
/// The caller must charge the full buffer and one read before calling. No FD is
/// opened or closed here. Zero means end of directory; the kernel advances the
/// held FD's cursor. Do not interleave another directory API on that FD.
pub fn visit_darwin_directory_batch_v1<E>(
    directory: BorrowedFd<'_>,
    buffer: &mut [u8],
    mut visit: impl FnMut(&[u8], u64) -> Result<(), E>,
) -> Result<usize, DarwinDirectoryVisitErrorV1<E>> {
    if buffer.len() < 4096 {
        return Err(DarwinDirectoryVisitErrorV1::Io(malformed(
            "directory batch buffer too small",
        )));
    }
    buffer.fill(0);
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_NAME
            | libc::ATTR_CMN_RETURNED_ATTRS
            | libc::ATTR_CMN_FILEID
            | ATTR_CMN_ERROR,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    // SAFETY: the descriptor is borrowed for this call; the kernel writes at
    // most buffer.len() initialized bytes. `attributes` has the libc layout.
    let count = unsafe {
        libc::getattrlistbulk(
            directory.as_raw_fd(),
            (&mut attributes as *mut libc::attrlist).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
        )
    };
    if count < 0 {
        return Err(DarwinDirectoryVisitErrorV1::Io(io::Error::last_os_error()));
    }
    let count = usize::try_from(count)
        .map_err(|_| DarwinDirectoryVisitErrorV1::Io(malformed("directory entry count invalid")))?;
    let mut offset = 0usize;
    for _ in 0..count {
        let length =
            usize::try_from(u32_at(buffer, offset).map_err(DarwinDirectoryVisitErrorV1::Io)?)
                .map_err(|_| {
                    DarwinDirectoryVisitErrorV1::Io(malformed("directory record length invalid"))
                })?;
        let end = offset.checked_add(length).ok_or_else(|| {
            DarwinDirectoryVisitErrorV1::Io(malformed("directory record length overflow"))
        })?;
        if length < RECORD_PREFIX_BYTES || length % 8 != 0 {
            return Err(DarwinDirectoryVisitErrorV1::Io(malformed(
                "directory record length malformed",
            )));
        }
        let record = buffer.get(offset..end).ok_or_else(|| {
            DarwinDirectoryVisitErrorV1::Io(malformed("directory record outside batch"))
        })?;
        let (name, file_id) = entry_in_record(record).map_err(DarwinDirectoryVisitErrorV1::Io)?;
        visit(name, file_id).map_err(DarwinDirectoryVisitErrorV1::Visit)?;
        offset = end;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt as _;

    #[test]
    fn held_directory_batches_visit_all_names() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("alpha"), b"a")?;
        std::fs::write(root.path().join("beta"), b"b")?;
        let held = File::open(root.path())?;
        let mut buffer = vec![0; 65_536];
        let mut names = Vec::new();
        loop {
            let count =
                visit_darwin_directory_batch_v1(held.as_fd(), &mut buffer, |name, inode| {
                    names.push((name.to_vec(), inode));
                    Ok::<_, io::Error>(())
                })
                .map_err(|error| match error {
                    DarwinDirectoryVisitErrorV1::Io(error)
                    | DarwinDirectoryVisitErrorV1::Visit(error) => error,
                })?;
            if count == 0 {
                break;
            }
        }
        names.sort();
        assert_eq!(
            names,
            [
                (
                    b"alpha".to_vec(),
                    std::fs::metadata(root.path().join("alpha"))?.ino()
                ),
                (
                    b"beta".to_vec(),
                    std::fs::metadata(root.path().join("beta"))?.ino()
                ),
            ]
        );
        assert!(held.metadata()?.is_dir());
        Ok(())
    }

    #[test]
    fn multiple_batches_preserve_every_name_once() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let mut expected = Vec::new();
        for index in 0..1800 {
            let name = format!("file-{index:04}");
            File::create(root.path().join(&name))?;
            expected.push(name.into_bytes());
        }
        let held = File::open(root.path())?;
        let mut buffer = vec![0; 65_536];
        let mut observed = Vec::new();
        let mut batches = 0;
        loop {
            let count =
                visit_darwin_directory_batch_v1(held.as_fd(), &mut buffer, |name, _inode| {
                    observed.push(name.to_vec());
                    Ok::<_, io::Error>(())
                })
                .map_err(|error| match error {
                    DarwinDirectoryVisitErrorV1::Io(error)
                    | DarwinDirectoryVisitErrorV1::Visit(error) => error,
                })?;
            if count == 0 {
                break;
            }
            batches += 1;
        }
        observed.sort();
        assert!(batches > 1);
        assert_eq!(observed, expected);
        Ok(())
    }

    #[test]
    fn malformed_record_is_rejected() {
        let record = [0u8; 64];
        assert!(entry_in_record(&record).is_err());
    }

    #[test]
    fn name_reference_must_stay_inside_its_record() {
        let mut record = [0u8; 48];
        let length = record.len() as u32;
        record[..4].copy_from_slice(&length.to_ne_bytes());
        record[4..8].copy_from_slice(&(libc::ATTR_CMN_NAME | libc::ATTR_CMN_FILEID).to_ne_bytes());
        record[24..28].copy_from_slice(&16i32.to_ne_bytes());
        record[28..32].copy_from_slice(&6u32.to_ne_bytes());
        record[32..40].copy_from_slice(&123u64.to_ne_bytes());
        record[40..46].copy_from_slice(b"alpha\0");
        assert_eq!(
            entry_in_record(&record).unwrap(),
            (b"alpha".as_slice(), 123)
        );

        record[28..32].copy_from_slice(&64u32.to_ne_bytes());
        assert!(entry_in_record(&record).is_err());
    }

    #[test]
    fn per_entry_kernel_error_keeps_its_errno_without_name_attrs() {
        let mut record = [0u8; 32];
        record[4..8].copy_from_slice(&ATTR_CMN_ERROR.to_ne_bytes());
        record[24..28].copy_from_slice(&(libc::EIO as u32).to_ne_bytes());
        assert_eq!(
            entry_in_record(&record).unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
    }

    #[test]
    fn directory_control_names_are_rejected_before_a_visitor_can_skip_them() {
        let mut record = [0u8; 48];
        record[4..8].copy_from_slice(&(libc::ATTR_CMN_NAME | libc::ATTR_CMN_FILEID).to_ne_bytes());
        record[24..28].copy_from_slice(&16i32.to_ne_bytes());
        record[32..40].copy_from_slice(&123u64.to_ne_bytes());
        for name in [b".\0".as_slice(), b"..\0".as_slice()] {
            record[28..32].copy_from_slice(&u32::try_from(name.len()).unwrap().to_ne_bytes());
            record[40..40 + name.len()].copy_from_slice(name);
            assert!(entry_in_record(&record).is_err());
        }
    }

    #[test]
    fn visitor_refusal_is_not_relabelled_and_borrowed_directory_stays_open() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("entry"), b"x")?;
        let held = File::open(root.path())?;
        let mut buffer = vec![0; 65_536];
        let result = visit_darwin_directory_batch_v1(held.as_fd(), &mut buffer, |_name, _inode| {
            Err::<(), _>("visitor refused")
        });
        assert!(matches!(
            result,
            Err(DarwinDirectoryVisitErrorV1::Visit("visitor refused"))
        ));
        assert!(held.metadata()?.is_dir());
        Ok(())
    }
}
