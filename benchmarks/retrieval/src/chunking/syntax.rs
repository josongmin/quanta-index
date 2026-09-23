//! Syntax-aware chunker for the pilot language (Rust).
//!
//! A tree-sitter parse cuts top-level items into one chunk each with
//! exact parser byte spans: leading doc/attribute comments attach to
//! their item and the preamble joins the first chunk. Parse errors,
//! missing nodes, oversized items, empty item sets and non-Rust files
//! fall back to an explicit whole-file chunk flagged `fallback: true`,
//! counted in coverage, never silent.

use crate::chunking::{CHUNKER_VERSION, Chunk, Chunker, STRATEGY_BRACE_HEURISTIC, chunk_id};
use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult, sha256_hex};

/// Default per-item byte cap before declared whole-file fallback.
pub const DEFAULT_MAX_ITEM_BYTES: usize = 32 * 1024;

const RUST_EXTENSION: &str = "rs";

#[derive(Debug, Clone, Copy)]
pub struct SyntaxChunker {
    pub max_item_bytes: usize,
}

impl SyntaxChunker {
    #[must_use]
    pub const fn new(max_item_bytes: usize) -> Self {
        Self { max_item_bytes }
    }
}

impl Default for SyntaxChunker {
    fn default() -> Self {
        Self {
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
        }
    }
}

/// Parse top-level item byte spans with tree-sitter.
///
/// Every named root child except comments is one item; leading comment
/// siblings attach to their item. `None` means declared fallback: a
/// parse error, a missing node, a degenerate span, or no items at all.
fn parse_rust_items(text: &str) -> Option<Vec<(usize, usize)>> {
    let mut parser = tree_sitter::Parser::new();
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    if parser.set_language(&language).is_err() {
        return None;
    }
    let tree = parser.parse(text, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let mut cursor = root.walk();
    let mut items = Vec::new();
    let mut pending_comment_start: Option<usize> = None;
    for child in root.named_children(&mut cursor) {
        if child.is_missing() {
            return None;
        }
        match child.kind() {
            "line_comment" | "block_comment" => {
                if pending_comment_start.is_none() {
                    pending_comment_start = Some(child.start_byte());
                }
            }
            _ => {
                let start = pending_comment_start
                    .take()
                    .unwrap_or_else(|| child.start_byte());
                if start >= child.end_byte() {
                    return None;
                }
                items.push((start, child.end_byte()));
            }
        }
    }
    if items.is_empty() {
        return None;
    }
    Some(items)
}

impl Chunker for SyntaxChunker {
    fn name(&self) -> &'static str {
        STRATEGY_BRACE_HEURISTIC
    }

    fn config(&self) -> String {
        format!("rust-ts-m{}", self.max_item_bytes)
    }

    fn config_value(&self) -> serde_json::Value {
        serde_json::json!({"max_item_bytes": self.max_item_bytes})
    }

    fn chunk(&self, file: &SourceFile) -> BenchResult<Vec<Chunk>> {
        if file.bytes.is_empty() {
            return Ok(Vec::new());
        }
        if std::path::Path::new(&file.path).extension()
            != Some(std::ffi::OsStr::new(RUST_EXTENSION))
        {
            return fallback_chunk(file, &self.config());
        }
        let Some(items) = parse_rust_items(&file.text) else {
            return fallback_chunk(file, &self.config());
        };
        let config = self.config();
        let mut chunks = Vec::with_capacity(items.len());
        for (position, (mut start, end)) in items.iter().copied().enumerate() {
            if position == 0 && start > 0 {
                // Preamble before the first item joins the first chunk.
                start = 0;
            }
            let item_bytes = end.checked_sub(start).ok_or_else(|| BenchError::Chunk {
                path: file.path.clone(),
                message: "syntax item has an inverted byte span".to_string(),
            })?;
            if item_bytes > self.max_item_bytes {
                // Declared fallback: an oversized item keeps one whole-file
                // chunk flagged fallback instead of a silent re-chunk.
                return fallback_chunk(file, &config);
            }
            let start_line = file.line_of_offset(start)?;
            let end_line = file.line_of_offset(end.saturating_sub(1))?;
            chunks.push(make_chunk(
                file, &config, start, end, start_line, end_line, false,
            )?);
        }
        Ok(chunks)
    }
}

fn make_chunk(
    file: &SourceFile,
    config: &str,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    fallback: bool,
) -> BenchResult<Chunk> {
    let text = file
        .text
        .get(start_byte..end_byte)
        .ok_or_else(|| BenchError::Chunk {
            path: file.path.clone(),
            message: "syntax item span is not a UTF-8 boundary".to_string(),
        })?
        .to_string();
    let start_u32 = u32::try_from(start_byte).map_err(|err| BenchError::Chunk {
        path: file.path.clone(),
        message: format!("file exceeds u32 byte range: {err}"),
    })?;
    let end_u32 = u32::try_from(end_byte).map_err(|err| BenchError::Chunk {
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
    Ok(Chunk {
        path: file.path.clone(),
        start_byte: start_u32,
        end_byte: end_u32,
        start_line: start_line_u32,
        end_line: end_line_u32,
        text: text.clone(),
        strategy: STRATEGY_BRACE_HEURISTIC.to_string(),
        version: CHUNKER_VERSION.to_string(),
        config: config.to_string(),
        chunk_id: chunk_id(
            STRATEGY_BRACE_HEURISTIC,
            CHUNKER_VERSION,
            config,
            &file.path,
            start_u32,
            end_u32,
            &sha256_hex(text.as_bytes()),
        ),
        fallback,
    })
}

fn fallback_chunk(file: &SourceFile, config: &str) -> BenchResult<Vec<Chunk>> {
    let end_line = file.line_count();
    if end_line == 0 {
        return Ok(Vec::new());
    }
    Ok(vec![make_chunk(
        file,
        config,
        0,
        file.bytes.len(),
        1,
        end_line,
        true,
    )?])
}
