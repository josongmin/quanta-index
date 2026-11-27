//! Explicit Node/TypeScript compiler projection for released JS/TS grammars.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde::ser::SerializeStruct;
use tree_sitter::Language;

use super::go_candidate_process::{
    check_budget, read_verified_file, run_bounded_leaf_child, stage_verified_file,
};
use super::{
    ExtractionControl, SymbolExtractError, SymbolLanguage, SyntaxCandidateEdit,
    SyntaxCandidateProjection, SyntaxCandidateSymbols,
    extract_syntax_candidate_symbols_with_control,
};
use crate::sha256_hex;

const SCRIPT: &[u8] = include_bytes!("../../../../tools/benchmark/retrieval/ts_projection.cjs");
const SCHEMA: &str = "quanta-ts-grammar-projection-v1";
const TYPESCRIPT_VERSION: &str = "5.9.3";
const MAX_SOURCE_BYTES: usize = 1 << 20;
const MAX_SCRIPT_BYTES: usize = 1 << 20;
const MAX_NODE_BYTES: usize = 256 << 20;
const MAX_TYPESCRIPT_BYTES: usize = 16 << 20;
const MAX_TOTAL_STAGED_BYTES: usize = 512 << 20;
const MAX_LIBRARIES: usize = 8;
const MAX_EDITS: usize = 1 << 16;
const MAX_BATCH_FILES: usize = 32;
const MAX_BATCH_BYTES: usize = 16 << 20;

/// Source and grammar for one row of an explicitly admitted syntax batch.
pub struct SyntaxBatchSource<'a> {
    pub path: &'a str,
    pub source: &'a str,
    pub grammar: Language,
}

impl serde::Serialize for SyntaxBatchSource<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("SyntaxBatchSource", 2)?;
        row.serialize_field("path", self.path)?;
        row.serialize_field("source", self.source)?;
        row.end()
    }
}

struct BatchInput(Vec<u8>);

impl Write for BatchInput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BATCH_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("syntax batch input exceeds 16 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One dynamically loaded Node library pinned by the caller's install manifest.
pub struct SyntaxHelperLibrary<'a> {
    pub path: &'a Path,
    pub sha256: &'a str,
}

/// Exact installed Node/compiler tuple for one explicit candidate evaluation.
/// The caller must bind these digests to a trusted distribution manifest.
pub struct SyntaxHelperArtifacts<'a> {
    pub node: &'a Path,
    pub node_sha256: &'a str,
    pub node_version: &'a str,
    pub typescript_js: &'a Path,
    pub typescript_sha256: &'a str,
    pub script_sha256: &'a str,
    pub node_libraries: &'a [SyntaxHelperLibrary<'a>],
}

/// Verified artifact bytes scoped to one explicit candidate evaluation batch.
/// Each invocation stages one fresh private copy. One Node process owns the
/// full batch and its compiler worker for the lifetime of that staging.
pub struct SyntaxCandidateSession {
    node: Vec<u8>,
    typescript: Vec<u8>,
    libraries: Vec<(OsString, Vec<u8>)>,
    node_sha256: String,
    node_version: String,
    typescript_sha256: String,
    script_sha256: String,
}

impl SyntaxCandidateSession {
    pub fn new(
        path: &str,
        artifacts: &SyntaxHelperArtifacts<'_>,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, SymbolExtractError> {
        check_budget(path, deadline, cancelled)?;
        if SCRIPT.len() > MAX_SCRIPT_BYTES
            || sha256_hex(SCRIPT) != artifacts.script_sha256
            || artifacts.node_libraries.len() > MAX_LIBRARIES
        {
            return Err(defect(path));
        }
        let node = read_verified_file(
            artifacts.node,
            artifacts.node_sha256,
            MAX_NODE_BYTES,
            path,
            deadline,
            cancelled,
        )
        .map_err(|error| retag(error, path))?;
        let typescript = read_verified_file(
            artifacts.typescript_js,
            artifacts.typescript_sha256,
            MAX_TYPESCRIPT_BYTES,
            path,
            deadline,
            cancelled,
        )
        .map_err(|error| retag(error, path))?;
        let mut total = node
            .len()
            .saturating_add(typescript.len())
            .saturating_add(SCRIPT.len());
        if total > MAX_TOTAL_STAGED_BYTES {
            return Err(SymbolExtractError::ResourceLimit { path: path.into() });
        }
        let mut names = BTreeSet::new();
        let mut libraries = Vec::with_capacity(artifacts.node_libraries.len());
        for library in artifacts.node_libraries {
            let name = library.path.file_name().ok_or_else(|| defect(path))?;
            if !names.insert(name.to_os_string()) {
                return Err(defect(path));
            }
            let bytes = read_verified_file(
                library.path,
                library.sha256,
                MAX_NODE_BYTES,
                path,
                deadline,
                cancelled,
            )
            .map_err(|error| retag(error, path))?;
            total = total.saturating_add(bytes.len());
            if total > MAX_TOTAL_STAGED_BYTES {
                return Err(SymbolExtractError::ResourceLimit { path: path.into() });
            }
            libraries.push((name.to_os_string(), bytes));
        }
        check_budget(path, deadline, cancelled)?;
        Ok(Self {
            node,
            typescript,
            libraries,
            node_sha256: artifacts.node_sha256.into(),
            node_version: artifacts.node_version.into(),
            typescript_sha256: artifacts.typescript_sha256.into(),
            script_sha256: artifacts.script_sha256.into(),
        })
    }
}

struct HelperResponse {
    schema: String,
    status: String,
    node_version: String,
    typescript_version: String,
    source_sha256: String,
    edits: Vec<HelperEdit>,
}

strict_symbol_object!(HelperResponse {
    schema: String,
    status: String,
    node_version: String,
    typescript_version: String,
    source_sha256: String,
    edits: Vec<HelperEdit>,
});

struct BatchRow {
    path: String,
    elapsed_ms: u64,
    response: HelperResponse,
}

strict_symbol_object!(BatchRow {
    path: String,
    elapsed_ms: u64,
    response: HelperResponse,
});

struct HelperEdit {
    start: usize,
    end: usize,
    replacement: String,
}

strict_symbol_object!(HelperEdit {
    start: usize,
    end: usize,
    replacement: String,
});

fn defect(path: &str) -> SymbolExtractError {
    SymbolExtractError::ProducerDefect {
        detail: format!("syntax candidate helper failed: {path}"),
    }
}

fn retag(error: SymbolExtractError, path: &str) -> SymbolExtractError {
    match error {
        SymbolExtractError::ProducerDefect { .. } => defect(path),
        other => other,
    }
}

fn validate_source(path: &str, source: &str) -> Result<(), SymbolExtractError> {
    let language = SymbolLanguage::from_path(path)
        .ok_or_else(|| SymbolExtractError::Unsupported { path: path.into() })?;
    if !matches!(
        language,
        SymbolLanguage::JavaScript | SymbolLanguage::TypeScript { .. }
    ) {
        return Err(SymbolExtractError::Unsupported { path: path.into() });
    }
    if path.is_empty() || path.starts_with('-') || path.contains('\0') {
        return Err(defect(path));
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(SymbolExtractError::ResourceLimit { path: path.into() });
    }
    Ok(())
}

impl SyntaxCandidateSession {
    fn stage(
        &self,
        path: &str,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<tempfile::TempDir, SymbolExtractError> {
        check_budget(path, deadline, cancelled)?;
        let staging = tempfile::tempdir().map_err(|_| defect(path))?;
        let bin_dir = staging.path().join("bin");
        let lib_dir = staging.path().join("lib");
        fs::create_dir(&bin_dir).map_err(|_| defect(path))?;
        fs::create_dir(&lib_dir).map_err(|_| defect(path))?;
        stage_verified_file(
            &bin_dir.join("node"),
            &self.node,
            true,
            path,
            deadline,
            cancelled,
        )?;
        stage_verified_file(
            &staging.path().join("projection.cjs"),
            SCRIPT,
            false,
            path,
            deadline,
            cancelled,
        )?;
        stage_verified_file(
            &staging.path().join("typescript.js"),
            &self.typescript,
            false,
            path,
            deadline,
            cancelled,
        )?;
        for (name, bytes) in &self.libraries {
            stage_verified_file(&lib_dir.join(name), bytes, false, path, deadline, cancelled)?;
        }
        Ok(staging)
    }

    /// Execute one source with fresh staging of the verified in-memory bytes.
    pub fn extract(
        &self,
        path: &str,
        source: &str,
        grammar: Language,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<SyntaxCandidateSymbols, SymbolExtractError> {
        validate_source(path, source)?;
        let staging = self.stage(path, deadline, cancelled)?;
        let mut command = Command::new(staging.path().join("bin/node"));
        let _ = command
            .arg(staging.path().join("projection.cjs"))
            .arg(path)
            .arg(staging.path().join("typescript.js"));
        let (output, errors, status) =
            run_bounded_leaf_child(path, source, command, deadline, cancelled)
                .map_err(|error| retag(error, path))?;
        if !status.success() || !errors.is_empty() {
            return Err(defect(path));
        }
        let response: HelperResponse = serde_json::from_slice(&output).map_err(|_| defect(path))?;
        self.extract_response(path, source, grammar, response, deadline, cancelled)
    }

    /// Project an ordered batch in one Node process and extract each admitted
    /// row within the unused portion of its file budget and the total deadline.
    pub fn extract_batch(
        &self,
        sources: &[SyntaxBatchSource<'_>],
        per_file_ms: u64,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<Result<SyntaxCandidateSymbols, SymbolExtractError>>, SymbolExtractError> {
        let first = sources.first().ok_or_else(|| defect("empty batch"))?.path;
        if sources.len() > MAX_BATCH_FILES {
            return Err(SymbolExtractError::ResourceLimit { path: first.into() });
        }
        if !(1..=600_000).contains(&per_file_ms) {
            return Err(defect(first));
        }
        check_budget(first, deadline, cancelled)?;
        let admitted = sources
            .iter()
            .map(|entry| validate_source(entry.path, entry.source).map(|()| entry))
            .collect::<Vec<_>>();
        let requests = admitted
            .iter()
            .filter_map(|row| row.as_ref().ok().copied())
            .collect::<Vec<_>>();
        let rows = match requests.first() {
            Some(entry) => {
                self.project_batch(&requests, entry.path, per_file_ms, deadline, cancelled)?
            }
            None => Vec::new(),
        };
        // Validate the full ordered response before accepting any file result.
        if rows.len() != requests.len()
            || requests.iter().zip(&rows).any(|(entry, row)| {
                row.path != entry.path
                    || row.elapsed_ms > per_file_ms
                    || (row.response.status == "timed_out" && row.elapsed_ms != per_file_ms)
            })
        {
            return Err(defect(first));
        }
        let mut rows = rows.into_iter();
        Ok(admitted
            .into_iter()
            .map(|entry| {
                let entry = entry?;
                let row = rows.next().ok_or_else(|| defect(entry.path))?;
                check_budget(entry.path, deadline, cancelled)?;
                let remaining = Duration::from_millis(per_file_ms - row.elapsed_ms);
                self.extract_response(
                    entry.path,
                    entry.source,
                    entry.grammar.clone(),
                    row.response,
                    deadline.min(Instant::now() + remaining),
                    cancelled,
                )
            })
            .collect())
    }

    fn project_batch(
        &self,
        requests: &[&SyntaxBatchSource<'_>],
        path: &str,
        per_file_ms: u64,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<BatchRow>, SymbolExtractError> {
        let mut input = BatchInput(Vec::new());
        serde_json::to_writer(&mut input, requests).map_err(|error| {
            if error.is_io() {
                SymbolExtractError::ResourceLimit { path: path.into() }
            } else {
                defect(path)
            }
        })?;
        let input = std::str::from_utf8(&input.0).map_err(|_| defect(path))?;
        let staging = self.stage(path, deadline, cancelled)?;
        let mut command = Command::new(staging.path().join("bin/node"));
        let _ = command
            .arg(staging.path().join("projection.cjs"))
            .arg("--batch")
            .arg(staging.path().join("typescript.js"))
            .arg(per_file_ms.to_string());
        let (output, errors, status) =
            run_bounded_leaf_child(path, input, command, deadline, cancelled)
                .map_err(|error| retag(error, path))?;
        if !status.success() || !errors.is_empty() {
            return Err(defect(path));
        }
        serde_json::from_slice(&output).map_err(|_| defect(path))
    }

    fn extract_response(
        &self,
        path: &str,
        source: &str,
        grammar: Language,
        response: HelperResponse,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<SyntaxCandidateSymbols, SymbolExtractError> {
        if response.schema != SCHEMA
            || response.node_version != self.node_version
            || response.typescript_version != TYPESCRIPT_VERSION
            || response.source_sha256 != sha256_hex(source.as_bytes())
            || response.edits.len() > MAX_EDITS
        {
            return Err(defect(path));
        }
        match response.status.as_str() {
            "parse_failure" if response.edits.is_empty() => {
                return Err(SymbolExtractError::ParseFailure { path: path.into() });
            }
            "resource_limit" if response.edits.is_empty() => {
                return Err(SymbolExtractError::ResourceLimit { path: path.into() });
            }
            "timed_out" if response.edits.is_empty() => {
                return Err(SymbolExtractError::TimedOut { path: path.into() });
            }
            "ok" => {}
            _ => return Err(defect(path)),
        }
        check_budget(path, deadline, cancelled)?;
        let mut projected = source.as_bytes().to_vec();
        let mut edits = Vec::with_capacity(response.edits.len());
        let mut previous_end = 0;
        for edit in response.edits {
            if edit.start < previous_end
                || edit.start >= edit.end
                || edit.end > projected.len()
                || !edit.replacement.is_ascii()
                || edit.replacement.len() != edit.end.saturating_sub(edit.start)
            {
                return Err(defect(path));
            }
            let Some(target) = projected.get_mut(edit.start..edit.end) else {
                return Err(defect(path));
            };
            target.copy_from_slice(edit.replacement.as_bytes());
            previous_end = edit.end;
            edits.push(SyntaxCandidateEdit {
                start: edit.start,
                end: edit.end,
                replacement: edit.replacement,
            });
        }
        let projected = String::from_utf8(projected).map_err(|_| defect(path))?;
        let control = ExtractionControl {
            path,
            deadline,
            cancellation: None,
            max_symbols: usize::MAX,
        };
        let result = extract_syntax_candidate_symbols_with_control(
            path,
            source,
            &SyntaxCandidateProjection {
                projected_sha256: sha256_hex(projected.as_bytes()),
                source_sha256: response.source_sha256,
                node_sha256: self.node_sha256.clone(),
                typescript_sha256: self.typescript_sha256.clone(),
                helper_script_sha256: self.script_sha256.clone(),
                edits,
                projected,
            },
            grammar,
            Some(&control),
        )?;
        check_budget(path, deadline, cancelled)?;
        Ok(result)
    }
}

/// One-shot compatibility wrapper around a verified candidate session.
/// Default corpus preflight continues to use its separately pinned grammar.
pub fn extract_syntax_candidate_symbols_from_helper(
    path: &str,
    source: &str,
    artifacts: &SyntaxHelperArtifacts<'_>,
    grammar: Language,
    deadline: Instant,
    cancelled: impl Fn() -> bool,
) -> Result<SyntaxCandidateSymbols, SymbolExtractError> {
    validate_source(path, source)?;
    let session = SyntaxCandidateSession::new(path, artifacts, deadline, &cancelled)?;
    session.extract(path, source, grammar, deadline, &cancelled)
}

#[cfg(test)]
mod session_tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::*;
    use crate::symbols::extract_symbols;

    #[test]
    fn batch_uses_worker_over_one_pipe_and_shares_each_file_budget() {
        const FILE_BUDGET_MS: u64 = 5_000;
        let source = "function local() {}\n";
        let source_sha256 = sha256_hex(source.as_bytes());
        let row = |path, elapsed_ms, status| {
            serde_json::json!({
                "path": path, "elapsed_ms": elapsed_ms,
                "response": {
                    "schema": SCHEMA, "status": status, "node_version": "v-test",
                    "typescript_version": TYPESCRIPT_VERSION, "source_sha256": source_sha256,
                    "edits": [],
                },
            })
        };
        let response = serde_json::json!([
            row("spent.js", FILE_BUDGET_MS, "ok"),
            row("expired.ts", FILE_BUDGET_MS, "timed_out"),
            row("accepted.ts", 20, "ok"),
        ]);
        let shell = format!(
            "#!/bin/sh\ntest \"$2\" = --batch || exit 98\ncat >/dev/null\nprintf '%s' '{response}'\n"
        );
        let session = SyntaxCandidateSession {
            node: shell.into_bytes(),
            typescript: Vec::new(),
            libraries: Vec::new(),
            node_sha256: "a".repeat(64),
            node_version: "v-test".into(),
            typescript_sha256: "b".repeat(64),
            script_sha256: sha256_hex(SCRIPT),
        };
        let too_large = " ".repeat(MAX_SOURCE_BYTES + 1);
        let inputs = [
            ("spent.js", source, tree_sitter_javascript::LANGUAGE.into()),
            (
                "too-large.js",
                too_large.as_str(),
                tree_sitter_javascript::LANGUAGE.into(),
            ),
            (
                "expired.ts",
                source,
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            ),
            (
                "accepted.ts",
                source,
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            ),
        ]
        .map(|(path, source, grammar)| SyntaxBatchSource {
            path,
            source,
            grammar,
        });
        let outcomes = session
            .extract_batch(
                &inputs,
                FILE_BUDGET_MS,
                Instant::now() + Duration::from_secs(10),
                &|| false,
            )
            .expect("admitted batch response");
        let [spent, oversized, expired, accepted] = outcomes.as_slice() else {
            panic!("expected one result per input");
        };
        assert!(matches!(spent, Err(SymbolExtractError::TimedOut { path }) if path == "spent.js"));
        assert!(
            matches!(oversized, Err(SymbolExtractError::ResourceLimit { path }) if path == "too-large.js")
        );
        assert!(
            matches!(expired, Err(SymbolExtractError::TimedOut { path }) if path == "expired.ts")
        );
        assert_eq!(
            accepted.as_ref().expect("valid row").records,
            extract_symbols("accepted.ts", source).expect("source fixture")
        );
    }

    #[test]
    fn verified_session_reuses_bytes_after_original_path_is_replaced() {
        let source = "function local() {}\n";
        let response = format!(
            "{{\"schema\":\"{SCHEMA}\",\"status\":\"ok\",\"node_version\":\"v-test\",\"typescript_version\":\"{TYPESCRIPT_VERSION}\",\"source_sha256\":\"{}\",\"edits\":[]}}",
            sha256_hex(source.as_bytes())
        );
        let script = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{response}'\n");
        let directory = tempfile::tempdir().expect("fixture directory");
        let node = directory.path().join("node");
        let typescript = directory.path().join("typescript.js");
        fs::write(&node, &script).expect("fixture node");
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).expect("executable");
        fs::write(&typescript, "module.exports = {};").expect("fixture compiler");
        let node_digest = sha256_hex(script.as_bytes());
        let compiler_digest = sha256_hex(b"module.exports = {};");
        let script_digest = sha256_hex(SCRIPT);
        let artifacts = SyntaxHelperArtifacts {
            node: &node,
            node_sha256: &node_digest,
            node_version: "v-test",
            typescript_js: &typescript,
            typescript_sha256: &compiler_digest,
            script_sha256: &script_digest,
            node_libraries: &[],
        };
        let session = SyntaxCandidateSession::new(
            "a.js",
            &artifacts,
            Instant::now() + Duration::from_secs(5),
            &|| false,
        )
        .expect("verified session");
        fs::write(&node, "#!/bin/sh\nexit 99\n").expect("replace original path");
        fs::write(&typescript, "forged compiler").expect("replace original compiler");
        for (path, grammar) in [
            ("a.js", tree_sitter_javascript::LANGUAGE.into()),
            ("b.ts", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        ] {
            let result = session
                .extract(
                    path,
                    source,
                    grammar,
                    Instant::now() + Duration::from_secs(5),
                    &|| false,
                )
                .expect("reused verified bytes");
            assert_eq!(
                result.records,
                extract_symbols(path, source).expect("baseline")
            );
            assert_eq!(result.node_sha256, node_digest);
        }
    }
}

#[cfg(all(test, not(tree_sitter_registry_consumer)))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::symbols::extract_symbols;

    #[test]
    fn staged_node_response_reaches_js_ts_and_tsx_symbol_extraction() {
        let source = "function local() {}\n";
        let response = format!(
            "{{\"schema\":\"{SCHEMA}\",\"status\":\"ok\",\"node_version\":\"v-test\",\"typescript_version\":\"{TYPESCRIPT_VERSION}\",\"source_sha256\":\"{}\",\"edits\":[]}}",
            sha256_hex(source.as_bytes())
        );
        let script = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{response}'\n");
        let directory = tempfile::tempdir().expect("fixture directory");
        let node = directory.path().join("node");
        let typescript = directory.path().join("typescript.js");
        fs::write(&node, &script).expect("fixture node");
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).expect("executable");
        fs::write(&typescript, "module.exports = {};").expect("fixture compiler");
        let node_digest = sha256_hex(script.as_bytes());
        let compiler_digest = sha256_hex(b"module.exports = {};");
        let script_digest = sha256_hex(SCRIPT);
        let artifacts = SyntaxHelperArtifacts {
            node: &node,
            node_sha256: &node_digest,
            node_version: "v-test",
            typescript_js: &typescript,
            typescript_sha256: &compiler_digest,
            script_sha256: &script_digest,
            node_libraries: &[],
        };
        for (path, grammar) in [
            ("a.js", tree_sitter_javascript::LANGUAGE.into()),
            ("a.ts", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            ("a.tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        ] {
            let result = extract_syntax_candidate_symbols_from_helper(
                path,
                source,
                &artifacts,
                grammar,
                Instant::now() + Duration::from_secs(5),
                || false,
            )
            .expect("staged helper");
            assert_eq!(
                result.records,
                extract_symbols(path, source).expect("vendor baseline")
            );
            assert_eq!(result.node_sha256, node_digest);
        }
        let wrong_digest = "0".repeat(64);
        let invalid = SyntaxHelperArtifacts {
            node_sha256: &wrong_digest,
            ..artifacts
        };
        assert!(matches!(
            extract_syntax_candidate_symbols_from_helper(
                "a.ts",
                source,
                &invalid,
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                Instant::now() + Duration::from_secs(5),
                || false,
            ),
            Err(SymbolExtractError::ProducerDefect { .. })
        ));
    }

    #[test]
    fn helper_statuses_are_typed_and_duplicate_fields_are_rejected() {
        let source = "function local() {}\n";
        let directory = tempfile::tempdir().expect("fixture directory");
        let node = directory.path().join("node");
        let typescript = directory.path().join("typescript.js");
        fs::write(&typescript, "module.exports = {};").expect("fixture compiler");
        let compiler_digest = sha256_hex(b"module.exports = {};");
        let script_digest = sha256_hex(SCRIPT);
        for (status, duplicate, expected) in [
            ("\"parse_failure\"", "", "parse_failure"),
            ("\"resource_limit\"", "", "resource_limit"),
            ("false", "", "producer_defect"),
            ("\"ok\"", ",\"status\":\"ok\"", "producer_defect"),
        ] {
            let response = format!(
                "{{\"schema\":\"{SCHEMA}\",\"status\":{status}{duplicate},\"node_version\":\"v-test\",\"typescript_version\":\"{TYPESCRIPT_VERSION}\",\"source_sha256\":\"{}\",\"edits\":[]}}",
                sha256_hex(source.as_bytes())
            );
            let shell = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{response}'\n");
            fs::write(&node, &shell).expect("fixture node");
            fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).expect("executable");
            let node_digest = sha256_hex(shell.as_bytes());
            let artifacts = SyntaxHelperArtifacts {
                node: &node,
                node_sha256: &node_digest,
                node_version: "v-test",
                typescript_js: &typescript,
                typescript_sha256: &compiler_digest,
                script_sha256: &script_digest,
                node_libraries: &[],
            };
            let result = extract_syntax_candidate_symbols_from_helper(
                "a.js",
                source,
                &artifacts,
                tree_sitter_javascript::LANGUAGE.into(),
                Instant::now() + Duration::from_secs(5),
                || false,
            );
            let observed = match result {
                Err(SymbolExtractError::ParseFailure { .. }) => "parse_failure",
                Err(SymbolExtractError::ResourceLimit { .. }) => "resource_limit",
                Err(SymbolExtractError::ProducerDefect { .. }) => "producer_defect",
                _ => "unexpected",
            };
            assert_eq!(observed, expected);
        }
    }

    #[test]
    #[ignore = "requires pinned Node and TypeScript compiler paths"]
    fn installed_node_compiler_projects_original_js_ts_records() {
        let node = PathBuf::from(std::env::var_os("QUANTA_TS_NODE").expect("Node path"));
        let typescript = PathBuf::from(
            std::env::var_os("QUANTA_TS_TYPESCRIPT_JS").expect("TypeScript compiler path"),
        );
        let node_version = Command::new(&node)
            .arg("--version")
            .output()
            .expect("Node version");
        assert!(node_version.status.success());
        let node_version = String::from_utf8(node_version.stdout).expect("version UTF-8");
        let node_version = node_version.trim();
        let node_digest = sha256_hex(&fs::read(&node).expect("Node bytes"));
        let compiler_digest = sha256_hex(&fs::read(&typescript).expect("compiler bytes"));
        let optional_library = std::env::var_os("QUANTA_TS_LIBNODE").map(PathBuf::from);
        let library_digest = optional_library
            .as_ref()
            .map(|path| sha256_hex(&fs::read(path).expect("libnode bytes")));
        let libraries = optional_library
            .as_ref()
            .zip(library_digest.as_deref())
            .map(|(path, digest)| {
                vec![SyntaxHelperLibrary {
                    path,
                    sha256: digest,
                }]
            })
            .unwrap_or_default();
        let script_digest = sha256_hex(SCRIPT);
        let artifacts = SyntaxHelperArtifacts {
            node: &node,
            node_sha256: &node_digest,
            node_version,
            typescript_js: &typescript,
            typescript_sha256: &compiler_digest,
            script_sha256: &script_digest,
            node_libraries: &libraries,
        };
        for (path, source, grammar) in [
            (
                "alias.js",
                "function local() {} export { local as const };",
                tree_sitter_javascript::LANGUAGE.into(),
            ),
            (
                "star.ts",
                "export type * as HttpProxy from './basic'\nfunction sentinel() {}",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            ),
            (
                "star.tsx",
                "export type * as HttpProxy from './basic'\nfunction sentinel() { return <div/> }",
                tree_sitter_typescript::LANGUAGE_TSX.into(),
            ),
        ] {
            let result = extract_syntax_candidate_symbols_from_helper(
                path,
                source,
                &artifacts,
                grammar,
                Instant::now() + Duration::from_secs(60),
                || false,
            )
            .expect("installed compiler projection");
            assert_eq!(
                result.records,
                extract_symbols(path, source).expect("vendor baseline")
            );
            assert_ne!(result.source_sha256, result.projected_sha256);
        }
    }
}
