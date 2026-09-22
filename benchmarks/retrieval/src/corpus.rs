//! Admitted-corpus loading (RB-00 manifest, RB-02 runner input).
//!
//! The manifest names the exact admitted file universe; the loader reads those
//! files and only those files from a pinned checkout, verifies every byte
//! hash, and builds the line index the chunker and the record emitter share.
//! The line split mirrors Python `splitlines` for `\r\n`, `\r` and `\n` only;
//! files containing any other Python split boundary are refused so Rust-side
//! block hashes can never silently diverge from the evaluator's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{BenchError, BenchResult, sha256_hex};

/// Default per-file byte cap. Larger admitted files fail closed; the manifest
/// (not the runner) decides the oversize policy by excluding them.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Python `splitlines` boundaries beyond CR/LF. Any file containing one is
/// refused: the Rust splitter below would otherwise disagree with the
/// evaluator's line model and every derived block hash.
const EXOTIC_SPLIT_CHARS: [char; 7] =
    ['\u{0b}', '\u{0c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}'];
const EXOTIC_SPLIT_CHAR_EXTRA: char = '\u{2029}';

#[derive(Debug, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub file_sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub repository_commit: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone)]
pub struct CorpusLimits {
    pub max_file_bytes: u64,
}

impl Default for CorpusLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// One admitted source file with its verified bytes and line-start index.
/// `line_starts[i]` is the byte offset where 1-based line `i + 1` starts;
/// the line's bytes (terminator included) run to the next start or EOF.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub text: String,
    pub line_starts: Vec<usize>,
    pub sha256: String,
}

impl SourceFile {
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte range (end-exclusive) of a 1-based inclusive line span.
    pub fn line_span_bytes(
        &self,
        start_line: usize,
        end_line: usize,
    ) -> BenchResult<(usize, usize)> {
        if start_line == 0 || end_line == 0 || start_line > end_line {
            return Err(BenchError::Corpus {
                path: self.path.clone(),
                message: format!("inverted line span {start_line}-{end_line}"),
            });
        }
        if end_line > self.line_starts.len() {
            return Err(BenchError::Corpus {
                path: self.path.clone(),
                message: format!(
                    "line span {start_line}-{end_line} beyond EOF ({} lines)",
                    self.line_starts.len()
                ),
            });
        }
        let start = self.line_starts[start_line - 1];
        let end = if end_line < self.line_starts.len() {
            self.line_starts[end_line]
        } else {
            self.bytes.len()
        };
        Ok((start, end))
    }

    /// 1-based line number containing `offset` (which must be < len for a
    /// nonempty file). Empty files contain no line.
    pub fn line_of_offset(&self, offset: usize) -> BenchResult<usize> {
        if self.line_starts.is_empty() || offset >= self.bytes.len() {
            return Err(BenchError::Corpus {
                path: self.path.clone(),
                message: format!("offset {offset} out of bounds"),
            });
        }
        let mut line = 1;
        for (index, start) in self.line_starts.iter().enumerate() {
            if *start > offset {
                break;
            }
            line = index + 1;
        }
        Ok(line)
    }
}

/// Split byte offsets of line starts, mirroring Python `splitlines(keepends)`
/// for `\r\n`, `\r`, `\n`. Returns the offsets plus whether the file holds an
/// exotic boundary the model cannot represent.
fn split_line_starts(text: &str) -> (Vec<usize>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let mut starts = vec![0_usize];
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 2;
                } else {
                    index += 1;
                }
                if index < bytes.len() {
                    starts.push(index);
                }
            }
            b'\n' => {
                index += 1;
                if index < bytes.len() {
                    starts.push(index);
                }
            }
            _ => {
                index += 1;
            }
        }
    }
    let exotic = text
        .chars()
        .any(|c| EXOTIC_SPLIT_CHARS.contains(&c) || c == EXOTIC_SPLIT_CHAR_EXTRA);
    (starts, exotic)
}

fn check_repo_path(path: &str) -> BenchResult<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(BenchError::Manifest(format!("noncanonical repository path: {path}")));
    }
    Ok(())
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

/// Load and validate the admitted-file manifest. Unknown JSON fields fail.
pub fn load_manifest(path: &Path) -> BenchResult<Manifest> {
    let raw = std::fs::read_to_string(path).map_err(|err| BenchError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|err| BenchError::Json {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;
    let object = value.as_object().ok_or_else(|| {
        BenchError::Manifest(format!("manifest must be an object: {}", path.display()))
    })?;
    let allowed: std::collections::BTreeSet<&str> =
        ["repository_commit", "files"].into_iter().collect();
    let unknown: Vec<&String> = object
        .keys()
        .filter(|key| !allowed.contains(key.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(BenchError::Manifest(format!(
            "manifest has unknown fields: {unknown:?}"
        )));
    }
    let manifest: Manifest = serde_json::from_value(value).map_err(|err| BenchError::Json {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    if !is_lower_hex(&manifest.repository_commit, 40) {
        return Err(BenchError::Manifest(
            "repository_commit must be a full lowercase Git SHA".to_string(),
        ));
    }
    if manifest.files.is_empty() {
        return Err(BenchError::Manifest(
            "manifest admits no files".to_string(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for file in &manifest.files {
        check_repo_path(&file.path)?;
        if !is_lower_hex(&file.file_sha256, 64) {
            return Err(BenchError::Manifest(format!(
                "file_sha256 must be a lowercase sha256: {}",
                file.path
            )));
        }
        if !seen.insert(file.path.clone()) {
            return Err(BenchError::Manifest(format!(
                "duplicate manifest path: {}",
                file.path
            )));
        }
    }
    Ok(manifest)
}

/// Canonical digest of the admitted universe: SHA-256 over the sorted
/// `path + NUL + file_sha256 + NUL` rows. Order-independent by construction.
#[must_use]
pub fn universe_digest(files: &[ManifestFile]) -> String {
    let mut rows: Vec<(&str, &str)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.file_sha256.as_str()))
        .collect();
    rows.sort();
    let mut raw = Vec::new();
    for (path, digest) in rows {
        raw.extend_from_slice(path.as_bytes());
        raw.push(0);
        raw.extend_from_slice(digest.as_bytes());
        raw.push(0);
    }
    sha256_hex(&raw)
}

/// Load every admitted file, verifying hashes. Files load in manifest order;
/// the caller sorts if canonical order is needed.
pub fn load_corpus(
    repo_root: &Path,
    manifest: &Manifest,
    limits: &CorpusLimits,
) -> BenchResult<Vec<SourceFile>> {
    let mut files = Vec::with_capacity(manifest.files.len());
    for entry in &manifest.files {
        files.push(load_source_file(repo_root, entry, limits)?);
    }
    Ok(files)
}

fn load_source_file(
    repo_root: &Path,
    entry: &ManifestFile,
    limits: &CorpusLimits,
) -> BenchResult<SourceFile> {
    let absolute: PathBuf = repo_root.join(&entry.path);
    let metadata = std::fs::symlink_metadata(&absolute).map_err(|err| BenchError::Io {
        path: entry.path.clone(),
        message: err.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(BenchError::Corpus {
            path: entry.path.clone(),
            message: "symlink is not source-file evidence".to_string(),
        });
    }
    if !metadata.is_file() {
        return Err(BenchError::Corpus {
            path: entry.path.clone(),
            message: "repository file missing".to_string(),
        });
    }
    if metadata.len() > limits.max_file_bytes {
        return Err(BenchError::Corpus {
            path: entry.path.clone(),
            message: format!(
                "file size {} exceeds {} byte cap",
                metadata.len(),
                limits.max_file_bytes
            ),
        });
    }
    let bytes = std::fs::read(&absolute).map_err(|err| BenchError::Io {
        path: entry.path.clone(),
        message: err.to_string(),
    })?;
    if bytes.contains(&0) {
        return Err(BenchError::Corpus {
            path: entry.path.clone(),
            message: "NUL byte: binary files are not chunkable source".to_string(),
        });
    }
    let digest = sha256_hex(&bytes);
    if digest != entry.file_sha256 {
        return Err(BenchError::Corpus {
            path: entry.path.clone(),
            message: "file hash mismatch against admitted manifest".to_string(),
        });
    }
    let text = String::from_utf8(bytes.clone()).map_err(|_| BenchError::Corpus {
        path: entry.path.clone(),
        message: "file is not UTF-8 source text".to_string(),
    })?;
    let (line_starts, exotic) = split_line_starts(&text);
    if exotic {
        return Err(BenchError::Corpus {
            path: entry.path.clone(),
            message: "exotic line boundary present; Rust/Python line models would diverge"
                .to_string(),
        });
    }
    Ok(SourceFile {
        path: entry.path.clone(),
        bytes,
        text,
        line_starts,
        sha256: digest,
    })
}

/// Canonical per-file line inventory for mapping proofs: path -> (line count,
/// file sha). Sorted by path.
#[must_use]
pub fn file_inventory(files: &[SourceFile]) -> BTreeMap<&str, (usize, &str)> {
    files
        .iter()
        .map(|file| {
            (
                file.path.as_str(),
                (file.line_count(), file.sha256.as_str()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_split_matches_python_model_for_lf_crlf_and_cr() {
        // "a\nb\r\nc\rd" -> starts at 0, 2, 5, 7.
        let (starts, exotic) = split_line_starts("a\nb\r\nc\rd");
        assert_eq!(starts, vec![0, 2, 5, 7]);
        assert!(!exotic);
        let (trailing, _) = split_line_starts("a\n");
        assert_eq!(trailing, vec![0]);
        let (empty, _) = split_line_starts("");
        assert!(empty.is_empty());
    }

    #[test]
    fn exotic_boundaries_are_detected() {
        for text in ["a\u{0b}b", "a\u{2029}b", "x\x1cy"] {
            let (_, exotic) = split_line_starts(text);
            assert!(exotic, "must flag {text:?}");
        }
    }

    #[test]
    fn universe_digest_is_order_independent() {
        let left = vec![
            ManifestFile {
                path: "b.rs".to_string(),
                file_sha256: "b".repeat(64),
            },
            ManifestFile {
                path: "a.rs".to_string(),
                file_sha256: "a".repeat(64),
            },
        ];
        let right: Vec<ManifestFile> = vec![
            ManifestFile {
                path: "a.rs".to_string(),
                file_sha256: "a".repeat(64),
            },
            ManifestFile {
                path: "b.rs".to_string(),
                file_sha256: "b".repeat(64),
            },
        ];
        assert_eq!(universe_digest(&left), universe_digest(&right));
    }

    #[test]
    fn unsafe_manifest_paths_fail() {
        for bad in ["", "/abs.rs", "a\\b.rs", "../x.rs", "a//b.rs", "a/./b.rs"] {
            assert!(check_repo_path(bad).is_err(), "must reject {bad:?}");
        }
        assert!(check_repo_path("src/lib.rs").is_ok());
    }
}
