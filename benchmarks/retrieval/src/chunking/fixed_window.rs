//! Fixed byte-window chunker with explicit overlap.
//!
//! Windows advance by `window_bytes - overlap_bytes` from offset 0. Window
//! starts snap forward to a UTF-8 boundary; window ends snap backward to a
//! boundary and then extend to the enclosing line end so every chunk stays
//! line-anchored for the evaluator's line-span model. A file that fits in
//! one window yields one chunk.

use crate::chunking::{CHUNKER_VERSION, Chunk, Chunker, STRATEGY_FIXED_WINDOW, chunk_id};
use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult, sha256_hex};

#[derive(Debug, Clone, Copy)]
pub struct FixedWindowChunker {
    pub window_bytes: usize,
    pub overlap_bytes: usize,
}

impl FixedWindowChunker {
    #[must_use]
    pub const fn new(window_bytes: usize, overlap_bytes: usize) -> Self {
        Self {
            window_bytes,
            overlap_bytes,
        }
    }

    fn checked(&self, path: &str) -> BenchResult<usize> {
        if self.window_bytes == 0 {
            return Err(BenchError::Chunk {
                path: path.to_string(),
                message: "window_bytes must be positive".to_string(),
            });
        }
        if self.overlap_bytes >= self.window_bytes {
            return Err(BenchError::Chunk {
                path: path.to_string(),
                message: "overlap_bytes must be below window_bytes".to_string(),
            });
        }
        Ok(self.window_bytes - self.overlap_bytes)
    }
}

fn snap_forward(text: &str, mut offset: usize) -> usize {
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset.min(text.len())
}

fn snap_backward(text: &str, mut offset: usize) -> usize {
    let capped = offset.min(text.len());
    offset = capped;
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

impl Chunker for FixedWindowChunker {
    fn name(&self) -> &'static str {
        STRATEGY_FIXED_WINDOW
    }

    fn config(&self) -> String {
        format!("w{}o{}", self.window_bytes, self.overlap_bytes)
    }

    fn chunk(&self, file: &SourceFile) -> BenchResult<Vec<Chunk>> {
        let step = self.checked(&file.path)?;
        if file.bytes.is_empty() {
            return Ok(Vec::new());
        }
        let config = self.config();
        let mut chunks = Vec::new();
        let mut start = 0_usize;
        let mut guard = 0_usize;
        while start < file.text.len() {
            guard += 1;
            if guard > file.text.len() + 2 {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window advance failed to terminate".to_string(),
                });
            }
            let window_end = snap_backward(&file.text, start.saturating_add(self.window_bytes));
            // Extend to the enclosing line end so spans stay line-anchored.
            let line = file.line_of_offset(window_end.min(file.text.len().saturating_sub(1)))?;
            let (_, line_end) = file.line_span_bytes(line, line)?;
            let mut end = line_end.max(start + 1);
            end = end.min(file.text.len());
            if end <= start {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window collapsed to zero length".to_string(),
                });
            }
            let start_line = file.line_of_offset(start)?;
            let end_line = file.line_of_offset(end - 1)?;
            let text = file.text[start..end].to_string();
            let start_u32 = u32::try_from(start).map_err(|_| BenchError::Chunk {
                path: file.path.clone(),
                message: "file exceeds u32 byte range".to_string(),
            })?;
            let end_u32 = u32::try_from(end).map_err(|_| BenchError::Chunk {
                path: file.path.clone(),
                message: "file exceeds u32 byte range".to_string(),
            })?;
            let start_line_u32 = u32::try_from(start_line).map_err(|_| BenchError::Chunk {
                path: file.path.clone(),
                message: "file exceeds u32 line range".to_string(),
            })?;
            let end_line_u32 = u32::try_from(end_line).map_err(|_| BenchError::Chunk {
                path: file.path.clone(),
                message: "file exceeds u32 line range".to_string(),
            })?;
            chunks.push(Chunk {
                path: file.path.clone(),
                start_byte: start_u32,
                end_byte: end_u32,
                start_line: start_line_u32,
                end_line: end_line_u32,
                text: text.clone(),
                strategy: STRATEGY_FIXED_WINDOW.to_string(),
                version: CHUNKER_VERSION.to_string(),
                config: config.clone(),
                chunk_id: chunk_id(
                    STRATEGY_FIXED_WINDOW,
                    CHUNKER_VERSION,
                    &config,
                    &file.path,
                    start_u32,
                    end_u32,
                    &sha256_hex(text.as_bytes()),
                ),
                fallback: false,
            });
            if end >= file.text.len() {
                break;
            }
            let next = snap_forward(&file.text, start.saturating_add(step));
            if next <= start {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window advance failed to progress".to_string(),
                });
            }
            start = next;
        }
        Ok(chunks)
    }
}
