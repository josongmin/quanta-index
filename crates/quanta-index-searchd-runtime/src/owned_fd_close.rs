//! Explicit close result for non-SQLite publication files.
//!
//! A failed close is ambiguous: the descriptor may already be closed. Callers
//! must retain the publication charge and never retry its numeric descriptor.

use std::{fs::File, io};

/// Consume a file and report only a confirmed successful close.
pub(crate) fn close_owned_file(file: File) -> io::Result<()> {
    // nix consumes ownership and issues exactly one close(2), including on
    // EINTR. Retrying an ambiguous descriptor could close a reused FD.
    nix::unistd::close(file).map_err(|error| io::Error::from_raw_os_error(error as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_owned_regular_file_once() -> io::Result<()> {
        let file = tempfile::tempfile()?;
        close_owned_file(file)
    }
}
