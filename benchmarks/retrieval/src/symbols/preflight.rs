//! Complete admitted-file census. This report is diagnostic output, never an
//! ingestible substitute for source bytes or a persisted coverage authority.

use std::collections::{BTreeMap, BTreeSet};
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
    ExtractedSymbols, GoCandidateSymbols, LineIndex, SYMBOL_PRODUCER_GRAMMARS,
    SYMBOL_PRODUCER_IDENTITY, SymbolExtractError, SymbolLanguage, SymbolNameSpan, SymbolRecord,
    SyntaxCandidateSymbols, deterministic_symbol_id, extract_parsed_symbols, parse_tree,
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
    pub grammar_identity: String,
    pub lockfile_sha256: String,
    pub producer_policy_sha256: String,
    pub policy: SymbolPreflightPolicy,
    pub files: Vec<SymbolFileReport>,
    pub admitted_files: usize,
    pub incomplete_files: usize,
}

/// Source-bound candidate records and parser name captures for the product
/// preflight contract. The candidate process must already have validated its
/// helper and grammar artifacts before returning this inventory.
pub struct CandidateSymbolInventory {
    pub records: Vec<SymbolRecord>,
    pub names: BTreeMap<String, SymbolNameSpan>,
}

strict_symbol_object!(CandidateSymbolInventory {
    records: Vec<SymbolRecord>,
    names: BTreeMap<String, SymbolNameSpan>,
});

impl From<GoCandidateSymbols> for CandidateSymbolInventory {
    fn from(candidate: GoCandidateSymbols) -> Self {
        Self {
            records: candidate.records,
            names: candidate.names,
        }
    }
}

impl From<SyntaxCandidateSymbols> for CandidateSymbolInventory {
    fn from(candidate: SyntaxCandidateSymbols) -> Self {
        Self {
            records: candidate.records,
            names: candidate.names,
        }
    }
}

type CandidateExtractor<'a> = dyn Fn(
        &str,
        &str,
        Instant,
        &dyn Fn() -> bool,
    ) -> Result<CandidateSymbolInventory, SymbolExtractError>
    + 'a;

type CandidateBatch<'a> = dyn Fn(
        &BTreeMap<String, SourceFile>,
        &SymbolPreflightOptions<'_>,
        Instant,
    ) -> Result<
        BTreeMap<String, Result<CandidateSymbolInventory, SymbolExtractError>>,
        SymbolExtractError,
    > + 'a;

struct CandidatePreflight<'a> {
    grammar_identity: &'a str,
    manifest_sha256: &'a str,
    extract: &'a CandidateExtractor<'a>,
    prepare_batch: Option<&'a CandidateBatch<'a>>,
}

const CANDIDATE_FALLBACK_GRAMMARS: &str = concat!(
    "rust@0.24.2;registry-checksum=",
    env!("QI_RUST_GRAMMAR_SHA256"),
    ";python@0.25.0;registry-checksum=",
    env!("QI_PYTHON_GRAMMAR_SHA256"),
);

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
    names: BTreeMap<String, BTreeMap<String, SymbolNameSpan>>,
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

    #[must_use]
    pub fn names(&self) -> &BTreeMap<String, BTreeMap<String, SymbolNameSpan>> {
        &self.names
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
            symbol_name_source_policy:
                quanta_index_contract::SymbolNameSourcePolicyV1::RawAsciiLocalName,
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

fn producer_policy(
    options: &SymbolPreflightOptions<'_>,
    candidate: Option<&CandidatePreflight<'_>>,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    let grammar_identity =
        candidate.map_or(SYMBOL_PRODUCER_GRAMMARS, |value| value.grammar_identity);
    for part in [
        b"quanta-index:symbol-preflight:v1".as_slice(),
        SYMBOL_PRODUCER_IDENTITY.as_bytes(),
        grammar_identity.as_bytes(),
        include_bytes!("../../../../Cargo.lock").as_slice(),
        include_bytes!("../symbols.rs").as_slice(),
        include_bytes!("preflight.rs").as_slice(),
        include_bytes!("definition_query.rs").as_slice(),
        include_bytes!("../../build.rs").as_slice(),
    ] {
        hash.update(part.len().to_string().as_bytes());
        hash.update([0]);
        hash.update(part);
    }
    if let Some(candidate) = candidate {
        hash.update(b"quanta-index:candidate-artifact-manifest-sha256:v1\0");
        hash.update(candidate.manifest_sha256.as_bytes());
        for source in [
            include_bytes!("official_candidate_process.rs").as_slice(),
            include_bytes!("go_candidate_process.rs").as_slice(),
            include_bytes!("syntax_candidate_process.rs").as_slice(),
        ] {
            hash.update(source.len().to_string().as_bytes());
            hash.update([0]);
            hash.update(source);
        }
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

fn validate_candidate_inventory(
    path: &str,
    source: &str,
    language: SymbolLanguage,
    inventory: CandidateSymbolInventory,
    control: &ExtractionControl<'_>,
) -> Result<ExtractedSymbols, SymbolExtractError> {
    let defect = || SymbolExtractError::ProducerDefect {
        detail: format!("candidate symbol inventory differs from admitted source: {path}"),
    };
    if inventory.records.len() > control.max_symbols {
        return Err(SymbolExtractError::ResourceLimit { path: path.into() });
    }
    if inventory.names.len() != inventory.records.len() {
        return Err(defect());
    }
    let line_index = LineIndex::new(source);
    let mut seen = BTreeSet::new();
    let mut last_start = None;
    for record in &inventory.records {
        control.check()?;
        let start = usize::try_from(record.definition_span.byte_start).map_err(|_| defect())?;
        let end = usize::try_from(record.definition_span.byte_end).map_err(|_| defect())?;
        let name = inventory
            .names
            .get(record.symbol_id.as_str())
            .ok_or_else(|| defect())?;
        if record.repo_relative_path.as_str() != path
            || record.definition_span.path.as_ref() != path
            || record.language.as_str() != language.language_code()
            || record.relationship != super::SymbolRelationship::Def
            || record.symbol_kind_family.is_some()
            || record.signature.is_some()
            || record.visibility.is_some()
            || start >= end
            || end > source.len()
            || last_start.is_some_and(|previous| start < previous)
            || !(start <= name.start_byte
                && name.start_byte < name.end_byte
                && name.end_byte <= end)
            || source.get(name.start_byte..name.end_byte) != Some(name.name.as_str())
            || record.local_name.as_ref() != name.name.as_str()
            || line_index.line(start).map_err(|_| defect())? != record.definition_span.line_start
            || line_index.line(end - 1).map_err(|_| defect())? != record.definition_span.line_end
            || deterministic_symbol_id(
                path,
                &record.language,
                &record.symbol_kind,
                record.qualified_name.as_ref(),
                start,
                end,
            ) != record.symbol_id.as_str()
            || !seen.insert(record.symbol_id.as_str().to_string())
        {
            return Err(defect());
        }
        last_start = Some(start);
    }
    Ok(ExtractedSymbols {
        records: inventory.records,
        names: inventory.names,
    })
}

fn inspect_file(
    path: &str,
    file: &SourceFile,
    options: &SymbolPreflightOptions<'_>,
    deadline: Instant,
    remaining_symbols: usize,
    candidate: Option<&CandidatePreflight<'_>>,
    prepared: Option<Result<CandidateSymbolInventory, SymbolExtractError>>,
) -> Result<(SymbolFileReport, ExtractedSymbols), SymbolExtractError> {
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
    if let Some(candidate) = candidate
        && language.requires_projection()
    {
        let cancelled = || {
            options
                .cancellation
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
        };
        let inventory = match prepared {
            Some(result) => result?,
            None if candidate.prepare_batch.is_some()
                && !matches!(language, SymbolLanguage::Go) =>
            {
                return Err(SymbolExtractError::ProducerDefect {
                    detail: format!("candidate batch omitted admitted syntax file: {path}"),
                });
            }
            None => (candidate.extract)(path, &file.text, control.deadline, &cancelled)?,
        };
        control.check()?;
        let extracted =
            validate_candidate_inventory(path, &file.text, language, inventory, &control)?;
        let symbol_count = u64::try_from(extracted.records.len())
            .map_err(|_| SymbolExtractError::ResourceLimit { path: path.into() })?;
        return Ok((
            SymbolFileReport {
                path: path.into(),
                source_sha256: file.sha256.clone(),
                language: Some(language.coverage_identity()),
                coverage: SymbolCoverage::Complete { symbol_count },
                failure: None,
                failure_detail: None,
                failure_detail_truncated: false,
                diagnostics: Vec::new(),
                diagnostics_total: 0,
                diagnostics_truncated: false,
                diagnostics_complete: true,
            },
            extracted,
        ));
    }
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
                    return Ok((report, ExtractedSymbols::default()));
                }
                let symbols = extract_parsed_symbols(
                    language,
                    path,
                    &file.text,
                    &tree,
                    Some(&control),
                    None,
                )?;
                let symbol_count = u64::try_from(symbols.records.len()).map_err(|_overflow| {
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

/// Run census directly against the linked grammars. The benchmark CLI uses
/// the verified projection pipeline for source languages requiring a helper.
pub fn preflight_corpus_symbols(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
) -> BenchResult<SymbolPreflight> {
    preflight_corpus_symbols_inner(files, options, None, Instant::now())
}

/// Explicit candidate admission through the same preflight and publication
/// contract. The caller owns verification of the registry grammar and helper
/// artifact manifest; this API never selects a candidate by default.
pub fn preflight_corpus_symbols_with_candidate<F>(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
    candidate_grammar_identity: &str,
    artifact_manifest_sha256: &str,
    extract: &F,
) -> BenchResult<SymbolPreflight>
where
    F: Fn(
        &str,
        &str,
        Instant,
        &dyn Fn() -> bool,
    ) -> Result<CandidateSymbolInventory, SymbolExtractError>,
{
    preflight_corpus_symbols_with_candidate_inner(
        files,
        options,
        candidate_grammar_identity,
        artifact_manifest_sha256,
        extract,
        None,
        Instant::now(),
    )
}

pub(super) fn preflight_corpus_symbols_with_candidate_batch<F, B>(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
    candidate_grammar_identity: &str,
    artifact_manifest_sha256: &str,
    extract: &F,
    prepare_batch: &B,
    started: Instant,
) -> BenchResult<SymbolPreflight>
where
    F: Fn(
        &str,
        &str,
        Instant,
        &dyn Fn() -> bool,
    ) -> Result<CandidateSymbolInventory, SymbolExtractError>,
    B: Fn(
        &BTreeMap<String, SourceFile>,
        &SymbolPreflightOptions<'_>,
        Instant,
    ) -> Result<
        BTreeMap<String, Result<CandidateSymbolInventory, SymbolExtractError>>,
        SymbolExtractError,
    >,
{
    preflight_corpus_symbols_with_candidate_inner(
        files,
        options,
        candidate_grammar_identity,
        artifact_manifest_sha256,
        extract,
        Some(prepare_batch),
        started,
    )
}

fn preflight_corpus_symbols_with_candidate_inner<F>(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
    candidate_grammar_identity: &str,
    artifact_manifest_sha256: &str,
    extract: &F,
    prepare_batch: Option<&CandidateBatch<'_>>,
    started: Instant,
) -> BenchResult<SymbolPreflight>
where
    F: Fn(
        &str,
        &str,
        Instant,
        &dyn Fn() -> bool,
    ) -> Result<CandidateSymbolInventory, SymbolExtractError>,
{
    if !candidate_grammar_identity.starts_with("registry-")
        || candidate_grammar_identity == SYMBOL_PRODUCER_GRAMMARS
        || candidate_grammar_identity.len() > 4096
        || artifact_manifest_sha256.len() != 64
        || !artifact_manifest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(BenchError::Config(
            "candidate grammar or artifact manifest identity is invalid".into(),
        ));
    }
    let grammar_identity = format!(
        "{candidate_grammar_identity};fallback={CANDIDATE_FALLBACK_GRAMMARS};artifact-manifest-sha256={artifact_manifest_sha256}"
    );
    let candidate = CandidatePreflight {
        grammar_identity: &grammar_identity,
        manifest_sha256: artifact_manifest_sha256,
        extract,
        prepare_batch,
    };
    preflight_corpus_symbols_inner(files, options, Some(&candidate), started)
}

fn preflight_corpus_symbols_inner(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
    candidate: Option<&CandidatePreflight<'_>>,
    started: Instant,
) -> BenchResult<SymbolPreflight> {
    let deadline = started.checked_add(options.timeout_total).ok_or_else(|| {
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
    let mut prepared = candidate
        .and_then(|candidate| candidate.prepare_batch)
        .map(|prepare| prepare(files, options, deadline))
        .transpose()
        .map_err(|error| BenchError::Protocol(format!("candidate batch failed: {error}")))?;
    let policy = producer_policy(options, candidate);
    let mut report = SymbolPreflightReport {
        schema: "symbol-preflight-v1",
        producer_identity: SYMBOL_PRODUCER_IDENTITY,
        grammar_identity: candidate.map_or_else(
            || SYMBOL_PRODUCER_GRAMMARS.to_string(),
            |value| value.grammar_identity.to_string(),
        ),
        lockfile_sha256: sha256_hex(include_bytes!("../../../../Cargo.lock")),
        producer_policy_sha256: encode_hex(&policy)?,
        policy: options.into(),
        files: Vec::with_capacity(files.len()),
        admitted_files: files.len(),
        incomplete_files: 0,
    };
    let mut symbols = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut retained = 0usize;
    let mut retained_symbols = 0usize;
    for (path, file) in files {
        let (mut row, records) = match inspect_file(
            path,
            file,
            options,
            deadline,
            options.max_symbols_total.saturating_sub(retained_symbols),
            candidate,
            prepared.as_mut().and_then(|rows| rows.remove(path)),
        ) {
            Ok(value) => value,
            Err(error) => (
                failure_report(path, file, &error),
                ExtractedSymbols::default(),
            ),
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
            .checked_add(records.records.len())
            .ok_or_else(|| BenchError::Protocol("symbol record count overflow".to_string()))?;
        let _previous = names.insert(path.clone(), records.names);
        let _previous = symbols.insert(path.clone(), records.records);
    }
    Ok(SymbolPreflight {
        report,
        symbols,
        names,
        producer_policy: policy,
    })
}
