use std::io::{self, Read};
use std::ops::Range;

use common::OwnedBytes;
use zstd::bulk::Decompressor;

pub struct BlockReader {
    buffer: Vec<u8>,
    reader: OwnedBytes,
    offset: usize,
}

impl BlockReader {
    /// Bound block-buffer growth and total key suffix bytes without decompressing.
    /// This uses the same zstd capacity authority as `read_block` below.
    pub(crate) fn decoding_bounds(mut encoded: &[u8]) -> io::Result<(usize, usize)> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid SSTable block bounds");
        let mut largest = 0usize;
        let mut total = 0usize;
        let mut context = 0usize;
        while !encoded.is_empty() {
            let header: [u8; 4] = encoded
                .get(..4)
                .ok_or_else(invalid)?
                .try_into()
                .map_err(|_| invalid())?;
            let length = u32::from_le_bytes(header) as usize;
            encoded = &encoded[4..];
            if length == 0 {
                if !encoded.is_empty() {
                    return Err(invalid());
                }
                break;
            }
            if length <= 1 {
                return Err(invalid());
            }
            let block = encoded.get(..length).ok_or_else(invalid)?;
            let required = match block[0] {
                0 => length - 1,
                1 => {
                    // SAFETY: This argument-free zstd query only returns the size
                    // of its single-shot DCtx; it allocates nothing. read_block
                    // uses one bulk context at a time, without a dictionary or
                    // streaming buffers, so one context covers the entire read.
                    context = unsafe { zstd::zstd_safe::zstd_sys::ZSTD_estimateDCtxSize() };
                    Decompressor::upper_bound(&block[1..]).ok_or_else(invalid)?
                }
                _ => return Err(invalid()),
            };
            largest = largest.max(required.max(8));
            total = total.checked_add(required).ok_or_else(invalid)?;
            encoded = &encoded[length..];
        }
        // Vec growth is at most twice the largest requested capacity; account
        // the minimum u8 allocation too. Allocator metadata is not logical bytes.
        let buffer = largest
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(context))
            .ok_or_else(invalid)?;
        Ok((buffer, total))
    }

    pub fn new(reader: OwnedBytes) -> BlockReader {
        BlockReader {
            buffer: Vec::new(),
            reader,
            offset: 0,
        }
    }

    pub fn deserialize_u64(&mut self) -> u64 {
        let (num_bytes, val) = super::vint::deserialize_read(self.buffer());
        self.advance(num_bytes);
        val
    }

    pub(crate) fn try_deserialize_u64(&mut self) -> io::Result<u64> {
        let (bytes, value) = super::vint::deserialize_read_checked(self.buffer())?;
        self.advance(bytes);
        Ok(value)
    }

    #[inline(always)]
    pub fn buffer_from_to(&self, range: Range<usize>) -> &[u8] {
        &self.buffer[range]
    }

    pub fn read_block(&mut self) -> io::Result<bool> {
        self.offset = 0;
        self.buffer.clear();

        let block_len = match self.reader.len() {
            0 => return Ok(false),
            1..=3 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to read block_len",
                ))
            }
            _ => self.reader.read_u32() as usize,
        };
        if block_len <= 1 {
            return Ok(false);
        }
        let compress = self.reader.read_u8();
        let block_len = block_len - 1;

        if self.reader.len() < block_len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "failed to read block content",
            ));
        }
        if compress == 1 {
            let required_capacity =
                Decompressor::upper_bound(&self.reader[..block_len]).unwrap_or(1024 * 1024);
            self.buffer.reserve(required_capacity);
            Decompressor::new()?
                .decompress_to_buffer(&self.reader[..block_len], &mut self.buffer)?;

            self.reader.advance(block_len);
        } else {
            self.buffer.resize(block_len, 0u8);
            self.reader.read_exact(&mut self.buffer[..])?;
        }

        Ok(true)
    }

    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.offset
    }

    #[inline(always)]
    pub fn advance(&mut self, num_bytes: usize) {
        self.offset += num_bytes;
    }

    #[inline(always)]
    pub fn buffer(&self) -> &[u8] {
        &self.buffer[self.offset..]
    }
}

impl io::Read for BlockReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.buffer().read(buf)?;
        self.advance(len);
        Ok(len)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let len = self.buffer.len();
        buf.extend_from_slice(self.buffer());
        self.advance(len);
        Ok(len)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.buffer().read_exact(buf)?;
        self.advance(buf.len());
        Ok(())
    }
}
