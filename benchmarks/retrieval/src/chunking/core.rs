//! Benchmark-owned chunking strategies (RB-03).
//!
//! Every strategy maps original file bytes to line-anchored chunks. A
//! strategy never validates itself: [`validate_chunks`] (the boundary oracle)
//! re-derives every byte slice, line span and ID from the source file.

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
    #[expect(
        clippy::struct_field_names,
        reason = "chunk_id is the stable external batch identity field"
    )]
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
    let mut count: usize = 0;
    let mut in_word = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if !in_word {
                count = count.saturating_add(1);
                in_word = true;
            }
        } else {
            in_word = false;
            if ch > '\u{20}' {
                count = count.saturating_add(1);
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

fn file_coverage(path: &str, file_len: u64, chunks: &[Chunk]) -> BenchResult<FileCoverage> {
    let capacity = usize::try_from(file_len).map_err(|err| BenchError::Chunk {
        path: path.to_string(),
        message: format!("file length cannot fit usize: {err}"),
    })?;
    let mut covered = vec![false; capacity];
    let mut overlap: u64 = 0;
    let mut bytes: u64 = 0;
    let mut tokens: u64 = 0;
    let mut fallback: usize = 0;
    for chunk in chunks {
        if chunk.path != path {
            return Err(BenchError::Chunk {
                path: path.to_string(),
                message: "coverage contains a chunk from another file".to_string(),
            });
        }
        let span = chunk
            .end_byte
            .checked_sub(chunk.start_byte)
            .ok_or_else(|| BenchError::Chunk {
                path: path.to_string(),
                message: "coverage contains an inverted byte span".to_string(),
            })?;
        bytes = bytes
            .checked_add(u64::from(span))
            .ok_or_else(|| BenchError::Chunk {
                path: path.to_string(),
                message: "coverage byte count overflow".to_string(),
            })?;
        let chunk_tokens =
            u64::try_from(count_tokens(&chunk.text)).map_err(|err| BenchError::Chunk {
                path: path.to_string(),
                message: format!("token count cannot fit u64: {err}"),
            })?;
        tokens = tokens
            .checked_add(chunk_tokens)
            .ok_or_else(|| BenchError::Chunk {
                path: path.to_string(),
                message: "coverage token count overflow".to_string(),
            })?;
        fallback = fallback
            .checked_add(usize::from(chunk.fallback))
            .ok_or_else(|| BenchError::Chunk {
                path: path.to_string(),
                message: "fallback chunk count overflow".to_string(),
            })?;
        for index in chunk.start_byte..chunk.end_byte {
            let slot_index = usize::try_from(index).map_err(|err| BenchError::Chunk {
                path: path.to_string(),
                message: format!("chunk offset cannot fit usize: {err}"),
            })?;
            let slot = covered
                .get_mut(slot_index)
                .ok_or_else(|| BenchError::Chunk {
                    path: path.to_string(),
                    message: "coverage chunk extends beyond EOF".to_string(),
                })?;
            if *slot {
                overlap = overlap.checked_add(1).ok_or_else(|| BenchError::Chunk {
                    path: path.to_string(),
                    message: "coverage overlap count overflow".to_string(),
                })?;
            } else {
                *slot = true;
            }
        }
    }
    let uncovered_count = covered.iter().filter(|slot| !**slot).count();
    let uncovered_bytes = u64::try_from(uncovered_count).map_err(|err| BenchError::Chunk {
        path: path.to_string(),
        message: format!("uncovered byte count cannot fit u64: {err}"),
    })?;
    Ok(FileCoverage {
        chunks: chunks.len(),
        bytes,
        tokens,
        overlap_bytes: overlap,
        uncovered_bytes,
        fallback_chunks: fallback,
    })
}

/// Build the coverage report from validated per-file chunks.
pub fn coverage_report(files: &BTreeMap<String, (u64, Vec<Chunk>)>) -> BenchResult<CoverageReport> {
    let mut report = CoverageReport {
        files: files.len(),
        ..CoverageReport::default()
    };
    for (path, (len, chunks)) in files {
        let file = file_coverage(path, *len, chunks)?;
        report.chunks =
            report
                .chunks
                .checked_add(file.chunks)
                .ok_or_else(|| BenchError::Chunk {
                    path: path.clone(),
                    message: "total chunk count overflow".to_string(),
                })?;
        report.bytes = report
            .bytes
            .checked_add(file.bytes)
            .ok_or_else(|| BenchError::Chunk {
                path: path.clone(),
                message: "total byte count overflow".to_string(),
            })?;
        report.tokens =
            report
                .tokens
                .checked_add(file.tokens)
                .ok_or_else(|| BenchError::Chunk {
                    path: path.clone(),
                    message: "total token count overflow".to_string(),
                })?;
        report.overlap_bytes = report
            .overlap_bytes
            .checked_add(file.overlap_bytes)
            .ok_or_else(|| BenchError::Chunk {
                path: path.clone(),
                message: "total overlap count overflow".to_string(),
            })?;
        report.uncovered_bytes = report
            .uncovered_bytes
            .checked_add(file.uncovered_bytes)
            .ok_or_else(|| BenchError::Chunk {
                path: path.clone(),
                message: "total uncovered count overflow".to_string(),
            })?;
        report.fallback_chunks = report
            .fallback_chunks
            .checked_add(file.fallback_chunks)
            .ok_or_else(|| BenchError::Chunk {
                path: path.clone(),
                message: "total fallback count overflow".to_string(),
            })?;
        let _previous = report.per_file.insert(path.clone(), file);
    }
    Ok(report)
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
        let end = usize::try_from(chunk.end_byte).map_err(|err| BenchError::Chunk {
            path: where_.clone(),
            message: format!("chunk end cannot fit usize: {err}"),
        })?;
        if end > file.bytes.len() {
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
        let start = usize::try_from(chunk.start_byte).map_err(|err| BenchError::Chunk {
            path: where_.clone(),
            message: format!("chunk start cannot fit usize: {err}"),
        })?;
        if !file.text.is_char_boundary(start) || !file.text.is_char_boundary(end) {
            return Err(BenchError::Chunk {
                path: where_,
                message: "byte span splits a UTF-8 boundary".to_string(),
            });
        }
        if file.text.get(start..end) != Some(chunk.text.as_str()) {
            return Err(BenchError::Chunk {
                path: where_,
                message: "emitted text differs from original byte slice".to_string(),
            });
        }
        let expected_start_line = file
            .line_of_offset(start)
            .map_err(|err| BenchError::Chunk {
                path: where_.clone(),
                message: err.to_string(),
            })?;
        let expected_end_line =
            file.line_of_offset(end.saturating_sub(1))
                .map_err(|err| BenchError::Chunk {
                    path: where_.clone(),
                    message: err.to_string(),
                })?;
        let actual_start_line =
            usize::try_from(chunk.start_line).map_err(|err| BenchError::Chunk {
                path: where_.clone(),
                message: format!("chunk start line cannot fit usize: {err}"),
            })?;
        let actual_end_line = usize::try_from(chunk.end_line).map_err(|err| BenchError::Chunk {
            path: where_.clone(),
            message: format!("chunk end line cannot fit usize: {err}"),
        })?;
        if actual_start_line != expected_start_line || actual_end_line != expected_end_line {
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
        if let Some(previous) = previous_start
            && chunk.start_byte < previous
        {
            return Err(BenchError::Chunk {
                path: where_,
                message: "chunks not in start-byte order".to_string(),
            });
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
        let file_len = u64::try_from(file.bytes.len()).map_err(|err| BenchError::Chunk {
            path: file.path.clone(),
            message: format!("file length cannot fit u64: {err}"),
        })?;
        if coverage_input
            .insert(file.path.clone(), (file_len, chunks.clone()))
            .is_some()
        {
            return Err(BenchError::Corpus {
                path: file.path.clone(),
                message: "duplicate corpus path".to_string(),
            });
        }
        let _previous = per_file.insert(file.path.clone(), chunks);
    }
    let report = coverage_report(&coverage_input)?;
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
        assert_eq!(count_tokens("héllo"), 3); // h, é, llo
        assert_eq!(count_tokens("a\tb"), 2);
        assert_eq!(count_tokens("x\ny"), 2);
    }
}
