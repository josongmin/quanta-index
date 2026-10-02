//! Complete admitted-file census. This report is diagnostic output, never an
//! ingestible substitute for source bytes or a persisted coverage authority.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ChunkRecord, ExactRepoRelativePathV1, SourceFileCoverage, SourceFileRevision, SymbolCoverage,
    source_file_unit_set_sha256,
};
use serde::Serialize;
use serde::ser::SerializeStruct;
use sha2::{Digest, Sha256};

use super::{
    SYMBOL_PRODUCER_GRAMMARS, SYMBOL_PRODUCER_IDENTITY, SymbolExtractError, SymbolLanguage,
    SymbolRecord, extract_parsed_symbols, parse_tree,
};
use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult, sha256_hex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolCoveragePolicy {
    RequireComplete,
    AllowIncomplete,
}

impl SymbolCoveragePolicy {
    pub fn parse(value: &str) -> BenchResult<Self> {
        match value {
            "require-complete" => Ok(Self::RequireComplete),
            "allow-incomplete" => Ok(Self::AllowIncomplete),
            _ => Err(BenchError::Config(format!(
                "unknown symbol coverage policy: {value}"
            ))),
        }
    }
}

pub struct SymbolPreflightOptions<'a> {
    pub max_file_bytes: usize,
    pub max_symbols_per_file: usize,
    pub max_symbols_total: usize,
    pub max_diagnostics_per_file: usize,
    pub max_diagnostics_total: usize,
    pub timeout_per_file: Duration,
    pub timeout_total: Duration,
    pub cancellation: Option<&'a AtomicBool>,
}

/// Exact extraction limits needed to interpret and reproduce the policy hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolPreflightPolicy {
    pub max_file_bytes: usize,
    pub max_symbols_per_file: usize,
    pub max_symbols_total: usize,
    pub max_diagnostics_per_file: usize,
    pub max_diagnostics_total: usize,
    pub timeout_per_file_ns: String,
    pub timeout_total_ns: String,
}

impl Serialize for SymbolPreflightPolicy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolPreflightPolicy", 7)?;
        state.serialize_field("max_file_bytes", &self.max_file_bytes)?;
        state.serialize_field("max_symbols_per_file", &self.max_symbols_per_file)?;
        state.serialize_field("max_symbols_total", &self.max_symbols_total)?;
        state.serialize_field("max_diagnostics_per_file", &self.max_diagnostics_per_file)?;
        state.serialize_field("max_diagnostics_total", &self.max_diagnostics_total)?;
        state.serialize_field("timeout_per_file_ns", &self.timeout_per_file_ns)?;
        state.serialize_field("timeout_total_ns", &self.timeout_total_ns)?;
        state.end()
    }
}

impl From<&SymbolPreflightOptions<'_>> for SymbolPreflightPolicy {
    fn from(options: &SymbolPreflightOptions<'_>) -> Self {
        Self {
            max_file_bytes: options.max_file_bytes,
            max_symbols_per_file: options.max_symbols_per_file,
            max_symbols_total: options.max_symbols_total,
            max_diagnostics_per_file: options.max_diagnostics_per_file,
            max_diagnostics_total: options.max_diagnostics_total,
            timeout_per_file_ns: options.timeout_per_file.as_nanos().to_string(),
            timeout_total_ns: options.timeout_total.as_nanos().to_string(),
        }
    }
}

impl Default for SymbolPreflightOptions<'_> {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_symbols_per_file: 100_000,
            max_symbols_total: 1_000_000,
            max_diagnostics_per_file: 32,
            max_diagnostics_total: 1024,
            timeout_per_file: Duration::from_secs(10),
            timeout_total: Duration::from_secs(120),
            cancellation: None,
        }
    }
}

pub(super) struct ExtractionControl<'a> {
    pub path: &'a str,
    pub deadline: Instant,
    pub cancellation: Option<&'a AtomicBool>,
    pub max_symbols: usize,
}

impl ExtractionControl<'_> {
    pub(super) fn check(&self) -> Result<(), SymbolExtractError> {
        if self
            .cancellation
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(SymbolExtractError::Cancelled {
                path: self.path.to_string(),
            });
        }
        if Instant::now() >= self.deadline {
            return Err(SymbolExtractError::TimedOut {
                path: self.path.to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn check_symbols(&self, count: usize) -> Result<(), SymbolExtractError> {
        self.check()?;
        if count >= self.max_symbols {
            return Err(SymbolExtractError::ResourceLimit {
                path: self.path.to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct SymbolFileDiagnostic {
    pub kind: &'static str,
    pub byte_start: Option<usize>,
    pub byte_end: Option<usize>,
}

impl Serialize for SymbolFileDiagnostic {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolFileDiagnostic", 3)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("byte_start", &self.byte_start)?;
        state.serialize_field("byte_end", &self.byte_end)?;
        state.end()
    }
}

#[derive(Clone, Debug)]
pub struct SymbolFileReport {
    pub path: String,
    pub source_sha256: String,
    pub language: Option<&'static str>,
    pub coverage: SymbolCoverage,
    pub failure: Option<&'static str>,
    pub failure_detail: Option<String>,
    pub failure_detail_truncated: bool,
    pub diagnostics: Vec<SymbolFileDiagnostic>,
    pub diagnostics_total: usize,
    pub diagnostics_truncated: bool,
    pub diagnostics_complete: bool,
}

impl Serialize for SymbolFileReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolFileReport", 11)?;
        state.serialize_field("path", &self.path)?;
        state.serialize_field("source_sha256", &self.source_sha256)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("coverage", &self.coverage)?;
        state.serialize_field("failure", &self.failure)?;
        state.serialize_field("failure_detail", &self.failure_detail)?;
        state.serialize_field("failure_detail_truncated", &self.failure_detail_truncated)?;
        state.serialize_field("diagnostics", &self.diagnostics)?;
        state.serialize_field("diagnostics_total", &self.diagnostics_total)?;
        state.serialize_field("diagnostics_truncated", &self.diagnostics_truncated)?;
        state.serialize_field("diagnostics_complete", &self.diagnostics_complete)?;
        state.end()
    }
}

#[derive(Debug)]
pub struct SymbolPreflightReport {
    pub schema: &'static str,
    pub producer_identity: &'static str,
    pub grammar_identity: &'static str,
    pub lockfile_sha256: String,
    pub producer_policy_sha256: String,
    pub policy: SymbolPreflightPolicy,
    pub files: Vec<SymbolFileReport>,
    pub admitted_files: usize,
    pub incomplete_files: usize,
}

impl Serialize for SymbolPreflightReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolPreflightReport", 9)?;
        state.serialize_field("schema", &self.schema)?;
        state.serialize_field("producer_identity", &self.producer_identity)?;
        state.serialize_field("grammar_identity", &self.grammar_identity)?;
        state.serialize_field("lockfile_sha256", &self.lockfile_sha256)?;
        state.serialize_field("producer_policy_sha256", &self.producer_policy_sha256)?;
        state.serialize_field("policy", &self.policy)?;
        state.serialize_field("files", &self.files)?;
        state.serialize_field("admitted_files", &self.admitted_files)?;
        state.serialize_field("incomplete_files", &self.incomplete_files)?;
        state.end()
    }
}

/// Only constructed from verified source bytes. It intentionally has no
/// deserializer or mutable accessors; imported reports cannot author coverage.
pub struct SymbolPreflight {
    report: SymbolPreflightReport,
    symbols: BTreeMap<String, Vec<SymbolRecord>>,
    producer_policy: [u8; 32],
}

impl SymbolPreflight {
    #[must_use]
    pub fn report(&self) -> &SymbolPreflightReport {
        &self.report
    }
    #[must_use]
    pub fn symbols(&self) -> &BTreeMap<String, Vec<SymbolRecord>> {
        &self.symbols
    }

    pub(super) fn into_symbols(self) -> BTreeMap<String, Vec<SymbolRecord>> {
        self.symbols
    }

    pub fn admit(&self, policy: SymbolCoveragePolicy) -> BenchResult<()> {
        // Fatal producer failures take precedence over earlier syntax gaps.
        // The optional profile admits only its explicit two recoverable states.
        let rejected = self
            .report
            .files
            .iter()
            .find(|file| matches!(file.coverage, SymbolCoverage::ProducerFailed))
            .or_else(|| {
                self.report.files.iter().find(|file| match file.coverage {
                    SymbolCoverage::Complete { .. } => false,
                    SymbolCoverage::Unsupported | SymbolCoverage::ParseFailed => {
                        policy == SymbolCoveragePolicy::RequireComplete
                    }
                    SymbolCoverage::NotRequested | SymbolCoverage::ProducerFailed => true,
                })
            });
        if let Some(file) = rejected {
            if file.coverage == SymbolCoverage::ProducerFailed {
                return Err(BenchError::Protocol(format!(
                    "symbol producer failed: {}; reason={:?}; source_sha256={}",
                    file.path, file.failure, file.source_sha256,
                )));
            }
            return Err(BenchError::Chunk {
                path: file.path.clone(),
                message: format!(
                    "symbol preflight refused: {} incomplete files of {}; state={:?}; reason={:?}; source_sha256={}",
                    self.report.incomplete_files,
                    self.report.admitted_files,
                    file.coverage,
                    file.failure,
                    file.source_sha256,
                ),
            });
        }
        Ok(())
    }

    /// Prepare the shared G0 DTO with the canonical unit-set commitment. The
    /// caller must separately pass profile admission before publishing.
    pub fn coverage_for(
        &self,
        source: SourceFileRevision,
        language: super::LanguageCode,
        file: &SourceFile,
        chunks: &[ChunkRecord],
    ) -> BenchResult<SourceFileCoverage> {
        source
            .validate()
            .map_err(|error| BenchError::Protocol(error.to_string()))?;
        let path = source.file.repo_relative_path.as_str();
        let index = self
            .report
            .files
            .binary_search_by(|file| file.path.as_str().cmp(path))
            .map_err(|insertion_index| {
                BenchError::Protocol(format!(
                    "coverage source was not admitted: {path}; insertion_index={insertion_index}"
                ))
            })?;
        let report =
            self.report.files.get(index).ok_or_else(|| {
                BenchError::Protocol("coverage report index is absent".to_string())
            })?;
        let digest = encode_hex(&source.source_sha256)?;
        if digest != report.source_sha256
            || file.path != path
            || file.sha256 != report.source_sha256
            || sha256_hex(&file.bytes) != report.source_sha256
            || file.text.as_bytes() != file.bytes
        {
            return Err(BenchError::Protocol(format!(
                "coverage source hash changed: {path}"
            )));
        }
        if let Some(expected) = SymbolLanguage::from_path(path)
            && language.as_str() != expected.language_code()
        {
            return Err(BenchError::Protocol(format!(
                "coverage language mismatch: {path}"
            )));
        }
        let symbols = self
            .symbols
            .get(path)
            .ok_or_else(|| BenchError::Protocol(format!("coverage symbols missing: {path}")))?;
        if chunks.iter().any(|chunk| {
            chunk.repo_relative_path.as_str() != path
                || chunk.language != language
                || chunk
                    .source_repo_id
                    .as_ref()
                    .is_some_and(|repo| repo != &source.file.source_repo_id)
        }) {
            return Err(BenchError::Protocol(format!(
                "coverage chunk ownership mismatch: {path}"
            )));
        }
        let line_index = super::LineIndex::new(&file.text);
        for chunk in chunks {
            let start = usize::try_from(chunk.start_byte)
                .map_err(|error| BenchError::Protocol(error.to_string()))?;
            let end = usize::try_from(chunk.end_byte)
                .map_err(|error| BenchError::Protocol(error.to_string()))?;
            let first_line = line_index
                .line(start)
                .map_err(|error| BenchError::Protocol(error.to_string()))?;
            let last_line = line_index
                .line(end.saturating_sub(1))
                .map_err(|error| BenchError::Protocol(error.to_string()))?;
            if start >= end
                || file.bytes.get(start..end) != Some(chunk.text.as_bytes())
                || chunk.start_line != first_line
                || chunk.end_line != last_line
            {
                return Err(BenchError::Protocol(format!(
                    "coverage chunk source bytes mismatch: {path}"
                )));
            }
        }
        let unit_set_sha256 = source_file_unit_set_sha256(chunks, symbols)
            .map_err(|error| BenchError::Protocol(error.to_string()))?;
        Ok(SourceFileCoverage {
            source,
            language,
            producer_policy_sha256: self.producer_policy,
            unit_set_sha256,
            // A source-only symbol scope does not publish a text surface.
            // An empty source is the explicit zero-unit text case.
            text_admitted: file.bytes.is_empty() || !chunks.is_empty(),
            symbols: report.coverage,
        })
    }
}

fn encode_hex(bytes: &[u8]) -> BenchResult<String> {
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            let digit = char::from_digit(u32::from(nibble), 16).ok_or_else(|| {
                BenchError::Protocol("byte nibble is outside the hexadecimal range".to_string())
            })?;
            encoded.push(digit);
        }
    }
    Ok(encoded)
}

fn producer_policy(options: &SymbolPreflightOptions<'_>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in [
        b"quanta-index:symbol-preflight:v1".as_slice(),
        SYMBOL_PRODUCER_IDENTITY.as_bytes(),
        SYMBOL_PRODUCER_GRAMMARS.as_bytes(),
        include_bytes!("../../../../Cargo.lock").as_slice(),
        include_bytes!("../symbols.rs").as_slice(),
        include_bytes!("preflight.rs").as_slice(),
        include_bytes!("../../build.rs").as_slice(),
    ] {
        hash.update(part.len().to_string().as_bytes());
        hash.update([0]);
        hash.update(part);
    }
    for limit in [
        options.max_file_bytes,
        options.max_symbols_per_file,
        options.max_symbols_total,
        options.max_diagnostics_per_file,
        options.max_diagnostics_total,
    ] {
        hash.update(limit.to_string().as_bytes());
        hash.update([0]);
    }
    hash.update(options.timeout_per_file.as_nanos().to_le_bytes());
    hash.update(options.timeout_total.as_nanos().to_le_bytes());
    hash.finalize().into()
}

fn failure_report(path: &str, file: &SourceFile, error: &SymbolExtractError) -> SymbolFileReport {
    let detail = error.to_string();
    let failure_detail_truncated = detail.chars().count() > 512;
    let failure_detail = Some(detail.chars().take(512).collect());
    let (coverage, kind, complete) = match error {
        SymbolExtractError::Unsupported { .. } => {
            (SymbolCoverage::Unsupported, "unsupported_language", true)
        }
        SymbolExtractError::ParseFailure { .. } => {
            (SymbolCoverage::ParseFailed, "syntax_error", true)
        }
        SymbolExtractError::Cancelled { .. } => {
            (SymbolCoverage::ProducerFailed, "cancelled", false)
        }
        SymbolExtractError::TimedOut { .. } => (SymbolCoverage::ProducerFailed, "timeout", false),
        SymbolExtractError::ResourceLimit { .. } => {
            (SymbolCoverage::ProducerFailed, "resource_limit", false)
        }
        SymbolExtractError::IdCollision { .. } | SymbolExtractError::ProducerDefect { .. } => {
            (SymbolCoverage::ProducerFailed, "producer_invariant", false)
        }
    };
    SymbolFileReport {
        path: path.to_string(),
        source_sha256: file.sha256.clone(),
        language: SymbolLanguage::from_path(path).map(SymbolLanguage::coverage_identity),
        coverage,
        failure: Some(kind),
        failure_detail,
        failure_detail_truncated,
        diagnostics: vec![SymbolFileDiagnostic {
            kind,
            byte_start: None,
            byte_end: None,
        }],
        diagnostics_total: 1,
        diagnostics_truncated: false,
        diagnostics_complete: complete,
    }
}

fn inspect_file(
    path: &str,
    file: &SourceFile,
    options: &SymbolPreflightOptions<'_>,
    deadline: Instant,
    remaining_symbols: usize,
) -> Result<(SymbolFileReport, Vec<SymbolRecord>), SymbolExtractError> {
    let control = ExtractionControl {
        path,
        deadline: Instant::now()
            .checked_add(options.timeout_per_file)
            .ok_or_else(|| SymbolExtractError::ResourceLimit {
                path: path.to_string(),
            })?
            .min(deadline),
        cancellation: options.cancellation,
        max_symbols: options.max_symbols_per_file.min(remaining_symbols),
    };
    control.check()?;
    if file.bytes.len() > options.max_file_bytes || u32::try_from(file.bytes.len()).is_err() {
        return Err(SymbolExtractError::ResourceLimit {
            path: path.to_string(),
        });
    }
    let language =
        SymbolLanguage::from_path(path).ok_or_else(|| SymbolExtractError::Unsupported {
            path: path.to_string(),
        })?;
    let tree = parse_tree(language, path, &file.text, Some(&control))?;
    let mut report = SymbolFileReport {
        path: path.to_string(),
        source_sha256: file.sha256.clone(),
        language: Some(language.coverage_identity()),
        coverage: SymbolCoverage::ParseFailed,
        failure: None,
        failure_detail: None,
        failure_detail_truncated: false,
        diagnostics: Vec::new(),
        diagnostics_total: 0,
        diagnostics_truncated: false,
        diagnostics_complete: true,
    };
    // Cursor traversal uses O(depth) native traversal state without materializing
    // every tree node. Count all ERROR/MISSING nodes even when output is capped.
    let mut cursor = tree.walk();
    loop {
        control.check()?;
        let node = cursor.node();
        if node.is_error() || node.is_missing() {
            report.diagnostics_total =
                report.diagnostics_total.checked_add(1).ok_or_else(|| {
                    SymbolExtractError::ResourceLimit {
                        path: path.to_string(),
                    }
                })?;
            let diagnostic = SymbolFileDiagnostic {
                kind: if node.is_missing() {
                    "missing_syntax"
                } else {
                    "syntax_error"
                },
                byte_start: Some(node.start_byte()),
                byte_end: Some(node.end_byte()),
            };
            let key = |value: &SymbolFileDiagnostic| (value.byte_start, value.byte_end, value.kind);
            let position = report
                .diagnostics
                .partition_point(|existing| key(existing) <= key(&diagnostic));
            if position < options.max_diagnostics_per_file {
                report.diagnostics.insert(position, diagnostic);
                report
                    .diagnostics
                    .truncate(options.max_diagnostics_per_file);
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                if tree.root_node().has_error() || report.diagnostics_total != 0 {
                    report.failure = Some("syntax_error");
                    // Tree-sitter can report a missing token in the root S-expression
                    // without exposing it as a TreeCursor child. Keep one explicitly
                    // unlocated, source-bounded diagnostic instead of reporting an
                    // impossible parse_failed state with a zero diagnostic census.
                    if report.diagnostics_total == 0 {
                        report.diagnostics_total = 1;
                        report.failure_detail =
                            Some("parser root reports an unlocated syntax error".to_string());
                        if options.max_diagnostics_per_file > 0 {
                            report.diagnostics.push(SymbolFileDiagnostic {
                                kind: "syntax_error",
                                byte_start: Some(tree.root_node().start_byte()),
                                byte_end: Some(tree.root_node().end_byte()),
                            });
                        }
                    }
                    report.diagnostics_truncated =
                        report.diagnostics_total > report.diagnostics.len();
                    return Ok((report, Vec::new()));
                }
                let symbols =
                    extract_parsed_symbols(language, path, &file.text, &tree, Some(&control))?;
                let symbol_count = u64::try_from(symbols.len()).map_err(|_overflow| {
                    SymbolExtractError::ResourceLimit {
                        path: path.to_string(),
                    }
                })?;
                report.coverage = SymbolCoverage::Complete { symbol_count };
                return Ok((report, symbols));
            }
        }
    }
}

pub fn preflight_corpus_symbols(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
) -> BenchResult<SymbolPreflight> {
    let deadline = Instant::now()
        .checked_add(options.timeout_total)
        .ok_or_else(|| {
            BenchError::Config("symbol total timeout exceeds clock range".to_string())
        })?;
    // Validate every admitted identity before starting the parser census.
    for (path, file) in files {
        let _validated_path =
            ExactRepoRelativePathV1::new(path).map_err(|message| BenchError::Corpus {
                path: path.clone(),
                message: message.to_string(),
            })?;
        let (line_starts, exotic) = crate::corpus::split_line_starts(&file.text);
        if &file.path != path
            || file.sha256 != sha256_hex(&file.bytes)
            || file.text.as_bytes() != file.bytes.as_slice()
            || file.line_starts != line_starts
            || exotic
        {
            return Err(BenchError::Corpus {
                path: path.clone(),
                message: "symbol source path/hash/text/line model differs from admitted bytes"
                    .to_string(),
            });
        }
    }
    let policy = producer_policy(options);
    let mut report = SymbolPreflightReport {
        schema: "symbol-preflight-v1",
        producer_identity: SYMBOL_PRODUCER_IDENTITY,
        grammar_identity: SYMBOL_PRODUCER_GRAMMARS,
        lockfile_sha256: sha256_hex(include_bytes!("../../../../Cargo.lock")),
        producer_policy_sha256: encode_hex(&policy)?,
        policy: options.into(),
        files: Vec::with_capacity(files.len()),
        admitted_files: files.len(),
        incomplete_files: 0,
    };
    let mut symbols = BTreeMap::new();
    let mut retained = 0usize;
    let mut retained_symbols = 0usize;
    for (path, file) in files {
        let (mut row, records) = match inspect_file(
            path,
            file,
            options,
            deadline,
            options.max_symbols_total.saturating_sub(retained_symbols),
        ) {
            Ok(value) => value,
            Err(error) => (failure_report(path, file, &error), Vec::new()),
        };
        row.diagnostics.truncate(
            options
                .max_diagnostics_per_file
                .min(options.max_diagnostics_total.saturating_sub(retained)),
        );
        row.diagnostics_truncated |= row.diagnostics_total > row.diagnostics.len();
        retained = retained
            .checked_add(row.diagnostics.len())
            .ok_or_else(|| BenchError::Protocol("symbol diagnostic count overflow".to_string()))?;
        if !matches!(row.coverage, SymbolCoverage::Complete { .. }) {
            report.incomplete_files = report.incomplete_files.checked_add(1).ok_or_else(|| {
                BenchError::Protocol("symbol incomplete-file count overflow".to_string())
            })?;
        }
        report.files.push(row);
        retained_symbols = retained_symbols
            .checked_add(records.len())
            .ok_or_else(|| BenchError::Protocol("symbol record count overflow".to_string()))?;
        let _previous = symbols.insert(path.clone(), records);
    }
    Ok(SymbolPreflight {
        report,
        symbols,
        producer_policy: policy,
    })
}
