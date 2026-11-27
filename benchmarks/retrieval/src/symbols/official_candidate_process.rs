//! Benchmark preflight against an installed registry-grammar producer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::go_candidate_process::{read_verified_file, run_bounded_child, stage_verified_file};
use super::preflight::{
    CandidateSymbolInventory, SymbolPreflight, SymbolPreflightOptions,
    preflight_corpus_symbols_with_candidate_batch,
};
use super::{SymbolExtractError, SymbolLanguage};
use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult};

const MAX_MANIFEST_BYTES: usize = 1 << 20;
const MAX_PRODUCER_BYTES: usize = 256 << 20;
const MAX_PROJECTED_SOURCE_BYTES: usize = 1 << 20;

struct Artifact {
    path: PathBuf,
    sha256: String,
}

strict_symbol_object!(Artifact {
    path: PathBuf,
    sha256: String,
});

struct CandidateManifest {
    schema: String,
    platform: String,
    architecture: String,
    grammar_identity: String,
    official_binary: Artifact,
    syntax_batch_binary: Artifact,
    go_helper: Artifact,
    go_version: String,
    node: Artifact,
    node_version: String,
    typescript_js: Artifact,
    typescript_version: String,
    script_sha256: String,
    node_libraries: Vec<Artifact>,
}

strict_symbol_object!(CandidateManifest {
    schema: String,
    platform: String,
    architecture: String,
    grammar_identity: String,
    official_binary: Artifact,
    syntax_batch_binary: Artifact,
    go_helper: Artifact,
    go_version: String,
    node: Artifact,
    node_version: String,
    typescript_js: Artifact,
    typescript_version: String,
    script_sha256: String,
    node_libraries: Vec<Artifact>,
});

struct CandidateResponse {
    status: String,
    records: Vec<super::SymbolRecord>,
    names: BTreeMap<String, super::SymbolNameSpan>,
}

strict_symbol_object!(CandidateResponse {
    status: String,
    records: Vec<super::SymbolRecord>,
    names: BTreeMap<String, super::SymbolNameSpan>,
});

struct CandidateBatchResponse {
    path: String,
    status: String,
    records: Vec<super::SymbolRecord>,
    names: BTreeMap<String, super::SymbolNameSpan>,
}

strict_symbol_object!(CandidateBatchResponse {
    path: String,
    status: String,
    records: Vec<super::SymbolRecord>,
    names: BTreeMap<String, super::SymbolNameSpan>,
});

fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_manifest(
    path: &Path,
    expected_sha256: &str,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> BenchResult<CandidateManifest> {
    if !valid_sha(expected_sha256) {
        return Err(BenchError::Config(
            "candidate manifest SHA-256 is invalid".into(),
        ));
    }
    let bytes = read_verified_file(
        path,
        expected_sha256,
        MAX_MANIFEST_BYTES,
        "candidate-manifest",
        deadline,
        cancelled,
    )
    .map_err(|error| BenchError::Config(format!("candidate manifest verification: {error}")))?;
    let manifest: CandidateManifest = serde_json::from_slice(&bytes)
        .map_err(|error| BenchError::Config(format!("candidate manifest JSON: {error}")))?;
    if manifest.schema != "quanta-symbol-candidate-artifacts-v1"
        || manifest.platform != std::env::consts::OS
        || manifest.architecture != std::env::consts::ARCH
        || manifest.grammar_identity.is_empty()
        || manifest.grammar_identity.len() > 4096
        || manifest.node_libraries.len() > 8
        || manifest.typescript_version != "5.9.3"
        || !valid_sha(&manifest.script_sha256)
        || ![
            &manifest.official_binary,
            &manifest.syntax_batch_binary,
            &manifest.go_helper,
            &manifest.node,
            &manifest.typescript_js,
        ]
        .into_iter()
        .chain(manifest.node_libraries.iter())
        .all(|artifact| artifact.path.is_absolute() && valid_sha(&artifact.sha256))
    {
        return Err(BenchError::Config(
            "candidate artifact manifest is invalid for this platform".into(),
        ));
    }
    Ok(manifest)
}

struct CandidateProcess {
    manifest: CandidateManifest,
    staging: tempfile::TempDir,
}

impl CandidateProcess {
    fn new(
        manifest: CandidateManifest,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> BenchResult<Self> {
        let staging = tempfile::tempdir()
            .map_err(|error| BenchError::Config(format!("candidate staging: {error}")))?;
        for (name, artifact) in [
            ("official-symbol-records", &manifest.official_binary),
            (
                "official-symbol-records-batch",
                &manifest.syntax_batch_binary,
            ),
        ] {
            let verified = read_verified_file(
                &artifact.path,
                &artifact.sha256,
                MAX_PRODUCER_BYTES,
                name,
                deadline,
                cancelled,
            )
            .map_err(|error| BenchError::Config(format!("{name} artifact: {error}")))?;
            stage_verified_file(
                &staging.path().join(name),
                &verified,
                true,
                name,
                deadline,
                cancelled,
            )
            .map_err(|error| BenchError::Config(format!("{name} staging: {error}")))?;
        }
        Ok(Self { manifest, staging })
    }

    fn prepare_syntax(
        &self,
        files: &BTreeMap<String, SourceFile>,
        options: &SymbolPreflightOptions<'_>,
        deadline: Instant,
    ) -> Result<
        BTreeMap<String, Result<CandidateSymbolInventory, SymbolExtractError>>,
        SymbolExtractError,
    > {
        let mut results = BTreeMap::new();
        let mut paths = Vec::new();
        for (path, file) in files {
            if !matches!(
                SymbolLanguage::from_path(path),
                Some(SymbolLanguage::JavaScript | SymbolLanguage::TypeScript { .. })
            ) {
                continue;
            }
            if file.bytes.len() > options.max_file_bytes.min(MAX_PROJECTED_SOURCE_BYTES) {
                let _previous = results.insert(
                    path.clone(),
                    Err(SymbolExtractError::ResourceLimit { path: path.clone() }),
                );
            } else {
                paths.push((path, file));
            }
        }
        for batch in paths.chunks(8) {
            let first = batch[0].0.as_str();
            let cancelled = || {
                options
                    .cancellation
                    .is_some_and(|flag| flag.load(Ordering::Relaxed))
            };
            if cancelled() {
                return Err(SymbolExtractError::Cancelled { path: first.into() });
            }
            let payload = serde_json::to_string(
                &batch
                    .iter()
                    .map(|(path, file)| serde_json::json!({"path": path, "source": &file.text}))
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| SymbolExtractError::ProducerDefect {
                detail: format!("candidate syntax batch input failed: {first}"),
            })?;
            if payload.len() > 16 << 20 {
                return Err(SymbolExtractError::ResourceLimit { path: first.into() });
            }
            let per_file_ms = u64::try_from(options.timeout_per_file.as_millis())
                .unwrap_or(u64::MAX)
                .clamp(1, 600_000);
            let remaining_ms = u64::try_from(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis(),
            )
            .unwrap_or(u64::MAX)
            .clamp(1, 3_600_000);
            let mut command =
                Command::new(self.staging.path().join("official-symbol-records-batch"));
            let _ = command
                .arg(&self.manifest.node.path)
                .arg(&self.manifest.node.sha256)
                .arg(&self.manifest.node_version)
                .arg(&self.manifest.typescript_js.path)
                .arg(&self.manifest.typescript_js.sha256)
                .arg(&self.manifest.script_sha256)
                .arg(&self.manifest.typescript_version);
            for library in &self.manifest.node_libraries {
                let _ = command.arg(&library.path).arg(&library.sha256);
            }
            let _ = command
                .arg("--inventory")
                .arg(per_file_ms.to_string())
                .arg(remaining_ms.to_string());
            let (stdout, stderr, status) =
                run_bounded_child(first, &payload, command, deadline, &cancelled)?;
            if !status.success() || !stderr.is_empty() {
                return Err(SymbolExtractError::ProducerDefect {
                    detail: format!("candidate syntax batch process failed: {first}"),
                });
            }
            let rows: Vec<CandidateBatchResponse> =
                serde_json::from_slice(&stdout).map_err(|_| {
                    SymbolExtractError::ProducerDefect {
                        detail: format!("candidate syntax batch response malformed: {first}"),
                    }
                })?;
            if rows.len() != batch.len() {
                return Err(SymbolExtractError::ProducerDefect {
                    detail: format!("candidate syntax batch cardinality differs: {first}"),
                });
            }
            for ((path, _), row) in batch.iter().zip(rows) {
                if row.path.as_str() != path.as_str() {
                    return Err(SymbolExtractError::ProducerDefect {
                        detail: format!("candidate syntax batch path differs: {path}"),
                    });
                }
                let outcome = match row.status.as_str() {
                    "ok" => Ok(CandidateSymbolInventory {
                        records: row.records,
                        names: row.names,
                    }),
                    "parse_failure" if row.records.is_empty() && row.names.is_empty() => {
                        Err(SymbolExtractError::ParseFailure {
                            path: path.as_str().to_string(),
                        })
                    }
                    "timed_out" if row.records.is_empty() && row.names.is_empty() => {
                        Err(SymbolExtractError::TimedOut {
                            path: path.as_str().to_string(),
                        })
                    }
                    "cancelled" if row.records.is_empty() && row.names.is_empty() => {
                        Err(SymbolExtractError::Cancelled {
                            path: path.as_str().to_string(),
                        })
                    }
                    "resource_limit" if row.records.is_empty() && row.names.is_empty() => {
                        Err(SymbolExtractError::ResourceLimit {
                            path: path.as_str().to_string(),
                        })
                    }
                    _ => {
                        return Err(SymbolExtractError::ProducerDefect {
                            detail: format!("candidate syntax batch status differs: {path}"),
                        });
                    }
                };
                let _previous = results.insert(path.as_str().to_string(), outcome);
            }
        }
        Ok(results)
    }

    fn extract(
        &self,
        path: &str,
        source: &str,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<CandidateSymbolInventory, SymbolExtractError> {
        let mut command = Command::new(self.staging.path().join("official-symbol-records"));
        match SymbolLanguage::from_path(path) {
            Some(SymbolLanguage::Go) => {
                let _ = command
                    .arg("go")
                    .arg(path)
                    .arg(&self.manifest.go_helper.path)
                    .arg(&self.manifest.go_helper.sha256)
                    .arg(&self.manifest.go_version);
            }
            Some(SymbolLanguage::JavaScript | SymbolLanguage::TypeScript { .. }) => {
                let _ = command
                    .arg("syntax")
                    .arg(path)
                    .arg(&self.manifest.node.path)
                    .arg(&self.manifest.node.sha256)
                    .arg(&self.manifest.node_version)
                    .arg(&self.manifest.typescript_js.path)
                    .arg(&self.manifest.typescript_js.sha256)
                    .arg(&self.manifest.script_sha256)
                    .arg(&self.manifest.typescript_version);
                for library in &self.manifest.node_libraries {
                    let _ = command.arg(&library.path).arg(&library.sha256);
                }
            }
            _ => return Err(SymbolExtractError::Unsupported { path: path.into() }),
        }
        let _ = command.arg("--inventory");
        let (stdout, stderr, status) =
            run_bounded_child(path, source, command, deadline, &|| cancelled())?;
        if !status.success() || !stderr.is_empty() {
            return Err(SymbolExtractError::ProducerDefect {
                detail: format!("candidate producer process failed for {path}"),
            });
        }
        let response: CandidateResponse = serde_json::from_slice(&stdout).map_err(|error| {
            SymbolExtractError::ProducerDefect {
                detail: format!("candidate producer response for {path}: {error}"),
            }
        })?;
        match response.status.as_str() {
            "ok" => Ok(CandidateSymbolInventory {
                records: response.records,
                names: response.names,
            }),
            "parse_failure" if response.records.is_empty() && response.names.is_empty() => {
                Err(SymbolExtractError::ParseFailure { path: path.into() })
            }
            _ => Err(SymbolExtractError::ProducerDefect {
                detail: format!("candidate producer status is invalid for {path}"),
            }),
        }
    }
}

/// Use the pinned installation for languages that require projection.
pub fn preflight_corpus_symbols_from_candidate_manifest(
    files: &BTreeMap<String, SourceFile>,
    options: &SymbolPreflightOptions<'_>,
    manifest_path: &Path,
    manifest_sha256: &str,
) -> BenchResult<SymbolPreflight> {
    let started = Instant::now();
    let deadline = started
        .checked_add(options.timeout_total)
        .ok_or_else(|| BenchError::Config("symbol total timeout exceeds clock range".into()))?;
    let cancelled = || {
        options
            .cancellation
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    };
    let manifest = read_manifest(manifest_path, manifest_sha256, deadline, &cancelled)?;
    let process = CandidateProcess::new(manifest, deadline, &cancelled)?;
    preflight_corpus_symbols_with_candidate_batch(
        files,
        options,
        &process.manifest.grammar_identity,
        manifest_sha256,
        &|path, source, deadline, cancelled| process.extract(path, source, deadline, cancelled),
        &|files, options, deadline| process.prepare_syntax(files, options, deadline),
        started,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;
    use crate::sha256_hex;
    use crate::symbols::{SymbolNameSpan, extract_symbols};

    #[test]
    fn pinned_manifest_selects_candidate_process_and_source_inventory() {
        let path = "p.go";
        let source = "package p\nfunc One() {}\n";
        let record = extract_symbols(path, source).expect("baseline record");
        let name = SymbolNameSpan {
            start_byte: 15,
            end_byte: 18,
            name: "One".into(),
        };
        let names = BTreeMap::from([(record[0].symbol_id.as_str().to_string(), name)]);
        let response = json!({
            "status": "ok",
            "records": record,
            "names": names,
        });
        let directory = tempfile::tempdir().expect("fixture directory");
        let binary = directory.path().join("official");
        let script = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response);
        fs::write(&binary, &script).expect("fixture producer");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");
        let binary_path = binary.display().to_string();
        let manifest = json!({
            "schema": "quanta-symbol-candidate-artifacts-v1",
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "grammar_identity": "registry-go@0.25.0;registry-checksum=test",
            "official_binary": {"path": &binary_path, "sha256": sha256_hex(script.as_bytes())},
            "syntax_batch_binary": {"path": &binary_path, "sha256": sha256_hex(script.as_bytes())},
            "go_helper": {"path": &binary_path, "sha256": sha256_hex(script.as_bytes())},
            "go_version": "go1.25.4",
            "node": {"path": &binary_path, "sha256": sha256_hex(script.as_bytes())},
            "node_version": "v-test",
            "typescript_js": {"path": &binary_path, "sha256": sha256_hex(script.as_bytes())},
            "typescript_version": "5.9.3",
            "script_sha256": "a".repeat(64),
            "node_libraries": [],
        });
        let parsed: CandidateManifest =
            serde_json::from_value(manifest.clone()).expect("complete manifest fields");
        assert_eq!(parsed.official_binary.path, binary);
        let mut unknown = manifest.clone();
        unknown["unexpected"] = json!(true);
        assert!(serde_json::from_value::<CandidateManifest>(unknown).is_err());
        let mut missing = manifest.clone();
        assert!(
            missing
                .as_object_mut()
                .expect("manifest object")
                .remove("go_helper")
                .is_some()
        );
        assert!(serde_json::from_value::<CandidateManifest>(missing).is_err());
        assert!(
            serde_json::from_str::<Artifact>(
                r#"{"path":"/first","path":"/second","sha256":"digest"}"#
            )
            .is_err()
        );
        let manifest_path = directory.path().join("manifest.json");
        let manifest_bytes = serde_json::to_vec(&manifest).expect("fixture manifest");
        fs::write(&manifest_path, &manifest_bytes).expect("manifest bytes");
        let (line_starts, exotic) = crate::corpus::split_line_starts(source);
        assert!(!exotic);
        let files = BTreeMap::from([(
            path.to_string(),
            SourceFile {
                path: path.into(),
                bytes: source.as_bytes().to_vec(),
                text: source.into(),
                line_starts,
                sha256: sha256_hex(source.as_bytes()),
            },
        )]);
        let result = preflight_corpus_symbols_from_candidate_manifest(
            &files,
            &SymbolPreflightOptions::default(),
            &manifest_path,
            &sha256_hex(&manifest_bytes),
        )
        .expect("candidate preflight");
        assert_eq!(result.symbols()[path][0].local_name.as_ref(), "One");
        assert_eq!(result.names()[path].len(), 1);
        assert!(
            result
                .report()
                .grammar_identity
                .contains("artifact-manifest-sha256=")
        );
        assert!(
            preflight_corpus_symbols_from_candidate_manifest(
                &files,
                &SymbolPreflightOptions::default(),
                &manifest_path,
                &"0".repeat(64),
            )
            .is_err()
        );
    }

    #[test]
    fn syntax_preflight_batches_eligible_sources_and_refuses_oversized_file() {
        let source = "function One() {}\n";
        let directory = tempfile::tempdir().expect("fixture directory");
        let one_shot = directory.path().join("one-shot");
        let one_shot_script = "#!/bin/sh\nexit 1\n";
        fs::write(&one_shot, one_shot_script).expect("one-shot trap");
        fs::set_permissions(&one_shot, fs::Permissions::from_mode(0o700)).expect("executable");
        let mut files = BTreeMap::new();
        let mut rows = Vec::new();
        for path in ["a.js", "b.js"] {
            let records = extract_symbols(path, source).expect("baseline records");
            let names = BTreeMap::from([(
                records[0].symbol_id.as_str().to_string(),
                SymbolNameSpan {
                    start_byte: 9,
                    end_byte: 12,
                    name: "One".into(),
                },
            )]);
            rows.push(json!({"path":path,"status":"ok","records":records,"names":names}));
            let (line_starts, exotic) = crate::corpus::split_line_starts(source);
            assert!(!exotic);
            let _previous = files.insert(
                path.to_string(),
                SourceFile {
                    path: path.into(),
                    bytes: source.as_bytes().to_vec(),
                    text: source.into(),
                    line_starts,
                    sha256: sha256_hex(source.as_bytes()),
                },
            );
        }
        let oversized = "z.ts";
        let oversized_source = "function Larger() { return 1; }";
        let _previous = files.insert(
            oversized.into(),
            SourceFile {
                path: oversized.into(),
                bytes: oversized_source.as_bytes().to_vec(),
                text: oversized_source.into(),
                line_starts: vec![0],
                sha256: sha256_hex(oversized_source.as_bytes()),
            },
        );
        let batch = directory.path().join("syntax-batch");
        let batch_script = format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
            json!(rows)
        );
        fs::write(&batch, &batch_script).expect("batch fixture");
        fs::set_permissions(&batch, fs::Permissions::from_mode(0o700)).expect("executable");
        let manifest = json!({
            "schema": "quanta-symbol-candidate-artifacts-v1",
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "grammar_identity": "registry-syntax@0.25.0",
            "official_binary": {"path": one_shot.display().to_string(), "sha256": sha256_hex(one_shot_script.as_bytes())},
            "syntax_batch_binary": {"path": batch.display().to_string(), "sha256": sha256_hex(batch_script.as_bytes())},
            "go_helper": {"path": one_shot.display().to_string(), "sha256": sha256_hex(one_shot_script.as_bytes())},
            "go_version": "go1.25.4",
            "node": {"path": one_shot.display().to_string(), "sha256": sha256_hex(one_shot_script.as_bytes())},
            "node_version": "v-test",
            "typescript_js": {"path": one_shot.display().to_string(), "sha256": sha256_hex(one_shot_script.as_bytes())},
            "typescript_version": "5.9.3",
            "script_sha256": "a".repeat(64),
            "node_libraries": [],
        });
        let manifest_path = directory.path().join("manifest.json");
        let manifest_bytes = serde_json::to_vec(&manifest).expect("fixture manifest");
        fs::write(&manifest_path, &manifest_bytes).expect("manifest bytes");
        let result = preflight_corpus_symbols_from_candidate_manifest(
            &files,
            &SymbolPreflightOptions {
                max_file_bytes: source.len(),
                ..SymbolPreflightOptions::default()
            },
            &manifest_path,
            &sha256_hex(&manifest_bytes),
        )
        .expect("syntax candidate batch");
        assert_eq!(result.symbols().len(), 2);
        assert!(result.symbols().values().all(|records| records.len() == 1));
        assert!(result.names().values().all(|names| names.len() == 1));
        assert_eq!(
            result
                .report()
                .files
                .iter()
                .find(|report| report.path == oversized)
                .expect("oversized source report")
                .failure,
            Some("resource_limit"),
        );
    }
}
