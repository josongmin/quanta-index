//! Subscriber cursor file. Persists the last durably-ack'd `ChannelSeq` so a
//! restarted subscriber resumes from the right offset.
//!
//! File format:
//!
//! ```text
//! [u32 LE magic = b"QICR"]
//! [u32 LE version = 1]
//! [u64 LE last_acked_seq]
//! ```
//!
//! Writes are performed via temp-file + rename so a torn write never corrupts
//! the cursor record.

use std::fs::{File, OpenOptions, rename};
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;

use quanta_index_contract::channel::ChannelSeq;

use crate::api::error::ChannelError;

const CURSOR_MAGIC: [u8; 4] = *b"QICR";
const CURSOR_VERSION: u32 = 1;
const CURSOR_SIZE: usize = 4 + 4 + 8;

pub struct CursorFile {
    path: PathBuf,
    value: ChannelSeq,
}

impl CursorFile {
    pub fn open(path: PathBuf) -> Result<Self, ChannelError> {
        let value = match File::open(&path) {
            Ok(mut file) => {
                let mut buf = [0u8; CURSOR_SIZE];
                file.read_exact(&mut buf)?;
                let magic = buf
                    .get(0..4)
                    .ok_or_else(|| ChannelError::State("cursor magic slice".to_string()))?;
                if magic != CURSOR_MAGIC {
                    return Err(ChannelError::State("cursor magic mismatch".to_string()));
                }
                let version_slice = buf
                    .get(4..8)
                    .ok_or_else(|| ChannelError::State("cursor version slice".to_string()))?;
                let version_arr: [u8; 4] = version_slice
                    .try_into()
                    .map_err(|_err| ChannelError::State("cursor version slice".to_string()))?;
                if u32::from_le_bytes(version_arr) != CURSOR_VERSION {
                    return Err(ChannelError::State(
                        "cursor version unsupported".to_string(),
                    ));
                }
                let seq_slice = buf
                    .get(8..16)
                    .ok_or_else(|| ChannelError::State("cursor seq slice".to_string()))?;
                let seq_arr: [u8; 8] = seq_slice
                    .try_into()
                    .map_err(|_err| ChannelError::State("cursor seq slice".to_string()))?;
                ChannelSeq::new(u64::from_le_bytes(seq_arr))
            }
            Err(err) if err.kind() == ErrorKind::NotFound => ChannelSeq::ZERO,
            Err(err) => return Err(ChannelError::Io(err)),
        };
        Ok(Self { path, value })
    }

    #[must_use]
    pub fn value(&self) -> ChannelSeq {
        self.value
    }

    pub fn store(&mut self, seq: ChannelSeq) -> Result<(), ChannelError> {
        let mut tmp_path = self.path.clone();
        if !tmp_path.set_extension("cursor.tmp") {
            return Err(ChannelError::State(
                "cursor path has no file_name; cannot stage temp file".to_string(),
            ));
        }
        let mut buf = [0u8; CURSOR_SIZE];
        if let Some(slot) = buf.get_mut(0..4) {
            slot.copy_from_slice(&CURSOR_MAGIC);
        }
        if let Some(slot) = buf.get_mut(4..8) {
            slot.copy_from_slice(&CURSOR_VERSION.to_le_bytes());
        }
        if let Some(slot) = buf.get_mut(8..16) {
            slot.copy_from_slice(&seq.get().to_le_bytes());
        }
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(&buf)?;
            file.sync_data()?;
        }
        rename(&tmp_path, &self.path)?;
        self.value = seq;
        Ok(())
    }
}
