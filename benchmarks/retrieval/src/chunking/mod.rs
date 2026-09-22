//! Benchmark-owned chunking strategies (RB-03).
//!
//! Every strategy maps original file bytes to line-anchored chunks. A
//! strategy never validates itself: [`validate_chunks`] (the boundary oracle)
//! re-derives every byte slice, line span and ID from the source file.

pub mod fixed_window;
pub mod syntax;
pub mod whole_file;

use std::collections::{BTreeMap, BTreeSet};

use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult, sha256_hex};

pub const STRATEGY_WHOLE_FILE: &str = "whole_file";
pub const STRATEGY_FIXED_WINDOW: &str = "fixed_window";
pub const STRATEGY_SYNTAX: &str = "syntax";
pub const CHUNKER_VERSION: &str = "rb03-v1";

/// One emitted chunk. Byte offsets are end-exclusive; line spans are 1-based
/// inclusive. `chunk_id` binds strategy, version, config, span and content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub path: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    pub strategy: String,
    pub version: String,
    pub config: String,
    pub chunk_id: String,
    pub fallback: bool,
}

/// Deterministic content-bound chunk ID.
#[must_use]
pub fn chunk_id(
    strategy: &str,
    version: &str,
    config: &str,
    path: &str,
    start_byte: u32,
    end_byte: u32,
    text_sha256: &str,
) -> String {
    let mut raw = Vec::new();
    for part in [
        strategy,
        version,
        config,
        path,
        &start_byte.to_string(),
        &end_byte.to_string(),
        text_sha256,
    ] {
        raw.extend_from_slice(part.as_bytes());
        raw.push(0);
    }
    sha256_hex(&raw)
}

/// Benchmark token unit, byte-identical to the evaluator's `qi-regex-v1`:
/// ASCII `[A-Za-z0-9_]+` runs count once; every other code point above
/// U+0020 counts once; controls/whitespace are separators.
#[must_use]
pub fn count_tokens(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
            if ch > '\u{20}' {
                count += 1;
            }
        }
    }
    count
}

/// A deterministic chunking strategy over original file bytes.
pub trait Chunker {
    fn name(&self) -> &'static str;
    fn config(&self) -> String;
    fn chunk(&self, file: &SourceFile) -> BenchResult<Vec<Chunk>>;
}

/// Per-file chunking coverage for the ablation report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCoverage {
    pub chunks: usize,
    pub bytes: u64,
    pub tokens: u64,
    pub overlap_bytes: u64,
    pub uncovered_bytes: u64,
    pub fallback_chunks: usize,
}

/// Aggregate coverage over a corpus. Measured from emitted spans, never from
/// a relevance number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageReport {
    pub files: usize,
    pub chunks: usize,
    pub bytes: u64,
    pub tokens: u64,
    pub overlap_bytes: u64,
    pub uncovered_bytes: u64,
    pub fallback_chunks: usize,
    pub per_file: BTreeMap<String, FileCoverage>,
}

fn file_coverage(path: &str, file_len: u64, chunks: &[Chunk]) -> FileCoverage {
    let mut covered = vec![false; usize::try_from(file_len).unwrap_or(0)];
    let mut overlap: u64 = 0;
    let mut bytes: u64 = 0;
    let mut tokens: u64 = 0;
    let mut fallback = 0;
    for chunk in chunks {
        debug_assert_eq!(chunk.path, path);
        bytes += u64::from(chunk.end_byte - chunk.start_byte);
        tokens += count_tokens(&chunk.text) as u64;
        fallback += usize::from(chunk.fallback);
        for index in chunk.start_byte..chunk.end_byte {
            let slot = usize::try_from(index).unwrap_or(covered.len());
            if slot < covered.len() {
                if covered[slot] {
                    overlap += 1;
                } else {
                    covered[slot] = true;
                }
            }
        }
    }
    FileCoverage {
        chunks: chunks.len(),
        bytes,
        tokens,
        overlap_bytes: overlap,
        uncovered_bytes: covered.iter().filter(|slot| !**slot).count() as u64,
        fallback_chunks: fallback,
    }
}

/// Build the coverage report from validated per-file chunks.
#[must_use]
pub fn coverage_report(files: &BTreeMap<String, (u64, Vec<Chunk>)>) -> CoverageReport {
    let mut report = CoverageReport::default();
    report.files = files.len();
    for (path, (len, chunks)) in files {
        let file = file_coverage(path, *len, chunks);
        report.chunks += file.chunks;
        report.bytes += file.bytes;
        report.tokens += file.tokens;
        report.overlap_bytes += file.overlap_bytes;
        report.uncovered_bytes += file.uncovered_bytes;
        report.fallback_chunks += file.fallback_chunks;
        assert!(report.per_file.insert(path.clone(), file).is_none());
    }
    report
}

/// Boundary oracle: re-derive every span, slice and ID from source bytes.
/// Any disagreement fails; the strategy's own claims prove nothing.
pub fn validate_chunks(chunks: &[Chunk], file: &SourceFile) -> BenchResult<()> {
    let mut seen_ids = BTreeSet::new();
    let mut previous_start: Option<u32> = None;
    for chunk in chunks {
        let where_ = chunk.path.clone();
        if chunk.path != file.path {
            return Err(BenchError::Chunk {
                path: where_,
                message: "chunk path differs from validated file".to_string(),
            });
        }
        if chunk.start_byte > chunk.end_byte {
            return Err(BenchError::Chunk {
                path: where_,
                message: "inverted byte span".to_string(),
            });
        }
        if usize::try_from(chunk.end_byte).unwrap_or(usize::MAX) > file.bytes.len() {
            return Err(BenchError::Chunk {
                path: where_,
                message: "byte span beyond EOF".to_string(),
            });
        }
        if chunk.start_byte == chunk.end_byte {
            return Err(BenchError::Chunk {
                path: where_,
                message: "zero-length chunk".to_string(),
            });
        }
        let start = usize::try_from(chunk.start_byte).unwrap_or(usize::MAX);
        let end = usize::try_from(chunk.end_byte).unwrap_or(0);
        if !file.text.is_char_boundary(start) || !file.text.is_char_boundary(end) {
            return Err(BenchError::Chunk {
                path: where_,
                message: "byte span splits a UTF-8 boundary".to_string(),
            });
        }
        if file.text[start..end] != chunk.text {
            return Err(BenchError::Chunk {
                path: where_,
                message: "emitted text differs from original byte slice".to_string(),
            });
        }
        let expected_start_line = file.line_of_offset(start).map_err(|err| BenchError::Chunk {
            path: where_.clone(),
            message: err.to_string(),
        })?;
        let expected_end_line =
            file.line_of_offset(end - 1)
                .map_err(|err| BenchError::Chunk {
                    path: where_.clone(),
                    message: err.to_string(),
                })?;
        if usize::try_from(chunk.start_line).unwrap_or(0) != expected_start_line
            || usize::try_from(chunk.end_line).unwrap_or(0) != expected_end_line
        {
            return Err(BenchError::Chunk {
                path: where_,
                message: format!(
                    "line span {}-{} does not cover byte span (expected {expected_start_line}-{expected_end_line})",
                    chunk.start_line, chunk.end_line
                ),
            });
        }
        let expected_id = chunk_id(
            &chunk.strategy,
            &chunk.version,
            &chunk.config,
            &chunk.path,
            chunk.start_byte,
            chunk.end_byte,
            &sha256_hex(chunk.text.as_bytes()),
        );
        if chunk.chunk_id != expected_id {
            return Err(BenchError::Chunk {
                path: where_,
                message: "chunk_id is not the content-bound ID".to_string(),
            });
        }
        if !seen_ids.insert(chunk.chunk_id.clone()) {
            return Err(BenchError::Chunk {
                path: where_,
                message: "duplicate chunk_id".to_string(),
            });
        }
        if let Some(previous) = previous_start {
            if chunk.start_byte < previous {
                return Err(BenchError::Chunk {
                    path: where_,
                    message: "chunks not in start-byte order".to_string(),
                });
            }
        }
        previous_start = Some(chunk.start_byte);
    }
    Ok(())
}

/// Chunk every file with one strategy, validating each output. Returns chunks
/// keyed by path plus the measured coverage. Empty files yield no chunks.
pub fn chunk_corpus<C: Chunker>(
    chunker: &C,
    files: &[SourceFile],
) -> BenchResult<(BTreeMap<String, Vec<Chunk>>, CoverageReport)> {
    let mut per_file: BTreeMap<String, Vec<Chunk>> = BTreeMap::new();
    let mut coverage_input: BTreeMap<String, (u64, Vec<Chunk>)> = BTreeMap::new();
    for file in files {
        let chunks = chunker.chunk(file)?;
        validate_chunks(&chunks, file)?;
        assert!(coverage_input.insert(
            file.path.clone(),
            (u64::try_from(file.bytes.len()).unwrap_or(u64::MAX), chunks.clone()),
        ).is_none());
        assert!(per_file.insert(file.path.clone(), chunks).is_none());
    }
    let report = coverage_report(&coverage_input);
    Ok((per_file, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_unit_matches_evaluator_cases() {
        // Hand-derived from TOKEN_RE = [A-Za-z0-9_]+|[^\x00-\x20].
        assert_eq!(count_tokens("fn main() {}"), 6); // fn, main, (, ), {, }
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("   \n\t"), 0);
        assert_eq!(count_tokens("hello_world 42"), 2);
        assert_eq!(count_tokens("a+b"), 3);
        assert_eq!(count_tokens("héllo"), 5); // h, é, l, l, o
    }
}
