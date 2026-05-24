//! Segment files and frame I/O.
//!
//! Each segment file layout:
//!
//! ```text
//! [SEGMENT HEADER — 16 bytes]
//!   [u32 LE magic = b"QIWL"]
//!   [u32 LE version = 1]
//!   [u64 LE seg_id]
//!
//! [FRAME — repeated]
//!   [u32 LE body_len]            # length of (seq prefix + codec body) bytes
//!   [u32 LE crc32]               # crc32 over (seq_le ++ codec_body)
//!   [u64 LE seq]
//!   [bytes codec_body]           # `OpCodec::encode_op` output, with op_tag as first byte
//! ```
//!
//! Body length is bounded by [`super::codec::MAX_FIELD_LEN`] plus a small fixed
//! overhead.

use std::fs::{File, OpenOptions, read_dir};
use std::io::{BufReader, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use quanta_index_contract::ChannelSeq;

use crate::api::error::ChannelError;

pub const SEGMENT_MAGIC: [u8; 4] = *b"QIWL";
pub const SEGMENT_VERSION: u32 = 1;
pub const SEGMENT_HEADER_SIZE_U64: u64 = 16;
pub const SEGMENT_HEADER_SIZE_USIZE: usize = 16;
pub const FRAME_HEADER_SIZE_U64: u64 = 8; // body_len + crc32
pub const FRAME_HEADER_SIZE_USIZE: usize = 8;
pub const FRAME_SEQ_PREFIX_U64: u64 = 8;
pub const FRAME_SEQ_PREFIX_USIZE: usize = 8;

/// Soft cap before rotating to the next segment.
pub const DEFAULT_SEGMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Soft entry cap before rotating.
pub const DEFAULT_SEGMENT_MAX_ENTRIES: u64 = 10_000;

/// Filesystem layout for one track. Constructed once per publisher/subscriber.
#[derive(Clone, Debug)]
pub struct SegmentLayout {
    root: PathBuf,
}

impl SegmentLayout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("publisher.lock")
    }

    pub fn cursor_path(&self) -> PathBuf {
        self.root.join("cursor")
    }

    pub fn segment_path(&self, seg_id: u64) -> PathBuf {
        self.root.join(format!("log.wal.{seg_id:020}"))
    }

    /// Enumerate segment files in sorted order (ascending `seg_id`).
    pub fn list_segments(&self) -> Result<Vec<(u64, PathBuf)>, ChannelError> {
        let mut out: Vec<(u64, PathBuf)> = Vec::new();
        let entries = match read_dir(&self.root) {
            Ok(iter) => iter,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(out),
            Err(err) => return Err(ChannelError::Io(err)),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let Some(stem) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id_str) = stem.strip_prefix("log.wal.") else {
                continue;
            };
            let Ok(seg_id) = id_str.parse::<u64>() else {
                continue;
            };
            out.push((seg_id, path));
        }
        out.sort_by_key(|(id, _)| *id);
        Ok(out)
    }
}

/// Writer over the active (highest-numbered) segment.
pub struct SegmentWriter {
    file: File,
    seg_id: u64,
    bytes_written: u64,
    entry_count: u64,
    max_bytes: u64,
    max_entries: u64,
}

impl SegmentWriter {
    pub fn open_or_create(layout: &SegmentLayout, seg_id: u64) -> Result<Self, ChannelError> {
        let path = layout.segment_path(seg_id);
        let existed = path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .truncate(false)
            .open(&path)?;
        if !existed {
            write_segment_header(&mut file, seg_id)?;
            file.sync_data()?;
        }
        let bytes_written = file.metadata()?.len();
        Ok(Self {
            file,
            seg_id,
            bytes_written,
            entry_count: 0,
            max_bytes: DEFAULT_SEGMENT_MAX_BYTES,
            max_entries: DEFAULT_SEGMENT_MAX_ENTRIES,
        })
    }

    pub fn seg_id(&self) -> u64 {
        self.seg_id
    }

    pub fn append_frame(
        &mut self,
        seq: ChannelSeq,
        codec_body: &[u8],
        fsync: bool,
    ) -> Result<u64, ChannelError> {
        let body_len_usize = codec_body.len().saturating_add(FRAME_SEQ_PREFIX_USIZE);
        let body_len = u32::try_from(body_len_usize).map_err(|_err| {
            ChannelError::Encoding(format!("frame body length {body_len_usize} overflow"))
        })?;

        let mut hasher = crc32fast::Hasher::new();
        let seq_le = seq.get().to_le_bytes();
        hasher.update(&seq_le);
        hasher.update(codec_body);
        let crc = hasher.finalize();

        let total_capacity = body_len_usize.saturating_add(FRAME_HEADER_SIZE_USIZE);
        let mut frame: Vec<u8> = Vec::with_capacity(total_capacity);
        frame.extend_from_slice(&body_len.to_le_bytes());
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(&seq_le);
        frame.extend_from_slice(codec_body);

        self.file.write_all(&frame)?;
        if fsync {
            self.file.sync_data()?;
        }
        let written = u64::try_from(frame.len())
            .map_err(|_err| ChannelError::State(format!("frame size {} overflow", frame.len())))?;
        self.bytes_written = self.bytes_written.saturating_add(written);
        self.entry_count = self.entry_count.saturating_add(1);
        Ok(written)
    }

    pub fn fsync(&self) -> Result<(), ChannelError> {
        self.file.sync_data()?;
        Ok(())
    }

    pub fn should_rotate(&self) -> bool {
        self.bytes_written >= self.max_bytes || self.entry_count >= self.max_entries
    }

    pub fn reset_after_rotate(&mut self, new_file: File, new_seg_id: u64) {
        self.file = new_file;
        self.seg_id = new_seg_id;
        self.bytes_written = SEGMENT_HEADER_SIZE_U64;
        self.entry_count = 0;
    }
}

fn write_segment_header(file: &mut File, seg_id: u64) -> Result<(), ChannelError> {
    let mut header = [0u8; SEGMENT_HEADER_SIZE_USIZE];
    write_into(&mut header, 0..4, &SEGMENT_MAGIC)?;
    write_into(&mut header, 4..8, &SEGMENT_VERSION.to_le_bytes())?;
    write_into(&mut header, 8..16, &seg_id.to_le_bytes())?;
    file.write_all(&header)?;
    Ok(())
}

fn write_into(
    buf: &mut [u8],
    range: std::ops::Range<usize>,
    src: &[u8],
) -> Result<(), ChannelError> {
    let slot = buf
        .get_mut(range)
        .ok_or_else(|| ChannelError::State("buffer slice unavailable".to_string()))?;
    slot.copy_from_slice(src);
    Ok(())
}

/// Decoded segment header.
#[derive(Clone, Copy, Debug)]
pub struct SegmentHeader {
    pub seg_id: u64,
}

pub fn read_segment_header<R: Read>(reader: &mut R) -> Result<SegmentHeader, ChannelError> {
    let mut buf = [0u8; SEGMENT_HEADER_SIZE_USIZE];
    reader.read_exact(&mut buf)?;
    let magic = take_arr::<4>(&buf, 0)?;
    if magic != SEGMENT_MAGIC {
        return Err(ChannelError::State("segment magic mismatch".to_string()));
    }
    let version_arr = take_arr::<4>(&buf, 4)?;
    if u32::from_le_bytes(version_arr) != SEGMENT_VERSION {
        return Err(ChannelError::State(
            "segment version unsupported".to_string(),
        ));
    }
    let seg_arr = take_arr::<8>(&buf, 8)?;
    Ok(SegmentHeader {
        seg_id: u64::from_le_bytes(seg_arr),
    })
}

fn take_arr<const N: usize>(buf: &[u8], offset: usize) -> Result<[u8; N], ChannelError> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| ChannelError::State("offset overflow".to_string()))?;
    let slice = buf
        .get(offset..end)
        .ok_or_else(|| ChannelError::State("buffer slice unavailable".to_string()))?;
    let arr: [u8; N] = slice
        .try_into()
        .map_err(|_err| ChannelError::State("slice length mismatch".to_string()))?;
    Ok(arr)
}

/// Reader over a single segment, advancing one frame at a time.
pub struct SegmentReader {
    inner: BufReader<File>,
    file_len: u64,
    pos: u64,
    seg_id: u64,
}

impl SegmentReader {
    pub fn open(layout: &SegmentLayout, seg_id: u64) -> Result<Self, ChannelError> {
        let path = layout.segment_path(seg_id);
        let mut file = OpenOptions::new().read(true).open(&path)?;
        let file_len = file.metadata()?.len();
        let new_pos = file.seek(SeekFrom::Start(0))?;
        if new_pos != 0 {
            return Err(ChannelError::State(format!(
                "segment seek to start landed at {new_pos}"
            )));
        }
        let mut buffered = BufReader::new(file);
        let header = read_segment_header(&mut buffered)?;
        if header.seg_id != seg_id {
            return Err(ChannelError::State(format!(
                "segment id mismatch (file={seg_id}, header={})",
                header.seg_id
            )));
        }
        Ok(Self {
            inner: buffered,
            file_len,
            pos: SEGMENT_HEADER_SIZE_U64,
            seg_id,
        })
    }

    pub fn seg_id(&self) -> u64 {
        self.seg_id
    }

    /// Current byte position within the segment file. After a successful
    /// `read_next_frame` this points just past the last good frame; after a
    /// failed read this still points at the start of the failed frame, since
    /// `pos` is only advanced on success.
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn refresh_len(&mut self) -> Result<(), ChannelError> {
        let new_len = self.inner.get_ref().metadata()?.len();
        self.file_len = new_len;
        Ok(())
    }

    /// Read the next frame. Returns:
    /// - `Ok(Some((seq, body)))` on success
    /// - `Ok(None)` if EOF was reached cleanly between frames
    /// - `Err(...)` on corruption / I/O / partial frame
    pub fn read_next_frame(&mut self) -> Result<Option<(ChannelSeq, Vec<u8>)>, ChannelError> {
        if self.pos >= self.file_len {
            return Ok(None);
        }
        let remaining = self.file_len.saturating_sub(self.pos);
        if remaining < FRAME_HEADER_SIZE_U64 {
            return Err(ChannelError::Corrupted {
                at_seq: ChannelSeq::ZERO,
                reason: "partial frame header at segment tail".to_string(),
            });
        }

        let mut header_buf = [0u8; FRAME_HEADER_SIZE_USIZE];
        self.inner.read_exact(&mut header_buf)?;
        let body_len = u32::from_le_bytes(take_arr::<4>(&header_buf, 0)?);
        let expected_crc = u32::from_le_bytes(take_arr::<4>(&header_buf, 4)?);

        let body_len_usize = usize::try_from(body_len).map_err(|_err| {
            ChannelError::Encoding(format!("body_len {body_len} usize overflow"))
        })?;
        if body_len_usize < FRAME_SEQ_PREFIX_USIZE {
            return Err(ChannelError::Corrupted {
                at_seq: ChannelSeq::ZERO,
                reason: "frame body shorter than seq prefix".to_string(),
            });
        }
        let new_pos = self
            .pos
            .checked_add(FRAME_HEADER_SIZE_U64)
            .and_then(|v| v.checked_add(u64::from(body_len)))
            .ok_or_else(|| ChannelError::State("frame pos overflow".to_string()))?;
        if new_pos > self.file_len {
            return Err(ChannelError::Corrupted {
                at_seq: ChannelSeq::ZERO,
                reason: "frame extends past segment end".to_string(),
            });
        }

        let mut body_buf = vec![0u8; body_len_usize];
        self.inner.read_exact(&mut body_buf)?;

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&body_buf);
        let actual_crc = hasher.finalize();
        if actual_crc != expected_crc {
            let seq_arr = take_arr::<8>(&body_buf, 0).unwrap_or([0u8; 8]);
            return Err(ChannelError::Corrupted {
                at_seq: ChannelSeq::new(u64::from_le_bytes(seq_arr)),
                reason: format!("crc mismatch (expected {expected_crc:#x}, got {actual_crc:#x})"),
            });
        }

        let seq_arr = take_arr::<8>(&body_buf, 0)?;
        let seq = ChannelSeq::new(u64::from_le_bytes(seq_arr));
        let codec_body = body_buf
            .get(FRAME_SEQ_PREFIX_USIZE..)
            .ok_or_else(|| ChannelError::Encoding("body slice unavailable".to_string()))?
            .to_vec();
        self.pos = new_pos;
        Ok(Some((seq, codec_body)))
    }
}

/// Open a segment file for tail-write (creating segment header if new).
pub fn open_segment_for_write(layout: &SegmentLayout, seg_id: u64) -> Result<File, ChannelError> {
    let path = layout.segment_path(seg_id);
    let existed = path.exists();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .truncate(false)
        .open(&path)?;
    if !existed {
        write_segment_header(&mut file, seg_id)?;
        file.sync_data()?;
    }
    Ok(file)
}
