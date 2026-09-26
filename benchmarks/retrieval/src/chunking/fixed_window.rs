//! Fixed byte-window chunker with explicit overlap.
//!
//! Windows advance by `window_bytes - overlap_bytes` from offset 0. Window
//! starts snap forward to a UTF-8 boundary; window ends snap backward to a
//! boundary and then extend to the enclosing line end so every chunk stays
//! line-anchored for the evaluator's line-span model. A file that fits in
//! one window yields one chunk.

use crate::chunking::{
    CHUNKER_VERSION, Chunk, Chunker, STRATEGY_FIXED_WINDOW_LINE_ALIGNED,
    STRATEGY_FIXED_WINDOW_STRICT, chunk_id,
};
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
        self.window_bytes
            .checked_sub(self.overlap_bytes)
            .ok_or_else(|| BenchError::Chunk {
                path: path.to_string(),
                message: "window step underflow".to_string(),
            })
    }
}

fn snap_forward(text: &str, mut offset: usize) -> usize {
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset = offset.saturating_add(1);
    }
    offset.min(text.len())
}

fn snap_backward(text: &str, mut offset: usize) -> usize {
    let capped = offset.min(text.len());
    offset = capped;
    while offset > 0 && !text.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}

impl Chunker for FixedWindowChunker {
    fn name(&self) -> &'static str {
        STRATEGY_FIXED_WINDOW_LINE_ALIGNED
    }

    fn config(&self) -> String {
        format!("w{}o{}", self.window_bytes, self.overlap_bytes)
    }

    fn config_value(&self) -> serde_json::Value {
        serde_json::json!({
            "window_bytes": self.window_bytes,
            "overlap_bytes": self.overlap_bytes,
            "alignment": "line",
        })
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
            guard = guard.saturating_add(1);
            if guard > file.text.len().saturating_add(2) {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window advance failed to terminate".to_string(),
                });
            }
            let window_end = snap_backward(&file.text, start.saturating_add(self.window_bytes));
            // Extend to the enclosing line end so spans stay line-anchored.
            let line = file.line_of_offset(window_end.min(file.text.len().saturating_sub(1)))?;
            let (_, line_end) = file.line_span_bytes(line, line)?;
            let mut end = line_end.max(start.saturating_add(1));
            end = end.min(file.text.len());
            if end <= start {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window collapsed to zero length".to_string(),
                });
            }
            let start_line = file.line_of_offset(start)?;
            let end_line = file.line_of_offset(end.saturating_sub(1))?;
            let text = file
                .text
                .get(start..end)
                .ok_or_else(|| BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window span is not a UTF-8 boundary".to_string(),
                })?
                .to_string();
            let start_u32 = u32::try_from(start).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 byte range: {err}"),
            })?;
            let end_u32 = u32::try_from(end).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 byte range: {err}"),
            })?;
            let start_line_u32 = u32::try_from(start_line).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 line range: {err}"),
            })?;
            let end_line_u32 = u32::try_from(end_line).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 line range: {err}"),
            })?;
            chunks.push(Chunk {
                path: file.path.clone(),
                start_byte: start_u32,
                end_byte: end_u32,
                start_line: start_line_u32,
                end_line: end_line_u32,
                text: text.clone(),
                strategy: STRATEGY_FIXED_WINDOW_LINE_ALIGNED.to_string(),
                version: CHUNKER_VERSION.to_string(),
                config: config.clone(),
                chunk_id: chunk_id(
                    STRATEGY_FIXED_WINDOW_LINE_ALIGNED,
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

/// Strict byte-window chunker: windows end exactly at the byte cap
/// (UTF-8 snap only, never extended to a line end), so ends can land
/// mid-line. The byte-span oracle still re-derives every boundary.
#[derive(Debug, Clone, Copy)]
pub struct StrictWindowChunker {
    pub window_bytes: usize,
    pub overlap_bytes: usize,
}

impl StrictWindowChunker {
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
        self.window_bytes
            .checked_sub(self.overlap_bytes)
            .ok_or_else(|| BenchError::Chunk {
                path: path.to_string(),
                message: "window step underflow".to_string(),
            })
    }
}

impl Chunker for StrictWindowChunker {
    fn name(&self) -> &'static str {
        STRATEGY_FIXED_WINDOW_STRICT
    }

    fn config(&self) -> String {
        format!("w{}o{}strict", self.window_bytes, self.overlap_bytes)
    }

    fn config_value(&self) -> serde_json::Value {
        serde_json::json!({
            "window_bytes": self.window_bytes,
            "overlap_bytes": self.overlap_bytes,
            "alignment": "byte",
            "byte_cap_strict": true,
        })
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
            guard = guard.saturating_add(1);
            if guard > file.text.len().saturating_add(2) {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window advance failed to terminate".to_string(),
                });
            }
            let end = snap_backward(&file.text, start.saturating_add(self.window_bytes));
            if end <= start {
                return Err(BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window byte budget cannot contain the next UTF-8 code point"
                        .to_string(),
                });
            }
            let start_line = file.line_of_offset(start)?;
            let end_line = file.line_of_offset(end.saturating_sub(1))?;
            let text = file
                .text
                .get(start..end)
                .ok_or_else(|| BenchError::Chunk {
                    path: file.path.clone(),
                    message: "window span is not a UTF-8 boundary".to_string(),
                })?
                .to_string();
            let start_u32 = u32::try_from(start).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 byte range: {err}"),
            })?;
            let end_u32 = u32::try_from(end).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 byte range: {err}"),
            })?;
            let start_line_u32 = u32::try_from(start_line).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 line range: {err}"),
            })?;
            let end_line_u32 = u32::try_from(end_line).map_err(|err| BenchError::Chunk {
                path: file.path.clone(),
                message: format!("file exceeds u32 line range: {err}"),
            })?;
            chunks.push(Chunk {
                path: file.path.clone(),
                start_byte: start_u32,
                end_byte: end_u32,
                start_line: start_line_u32,
                end_line: end_line_u32,
                text: text.clone(),
                strategy: STRATEGY_FIXED_WINDOW_STRICT.to_string(),
                version: CHUNKER_VERSION.to_string(),
                config: config.clone(),
                chunk_id: chunk_id(
                    STRATEGY_FIXED_WINDOW_STRICT,
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
            // Snapping the cap backward and the step forward independently
            // can skip a whole code point. Clamp to the emitted end, which
            // is a valid boundary and strictly beyond start.
            let next = snap_forward(&file.text, start.saturating_add(step)).min(end);
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
