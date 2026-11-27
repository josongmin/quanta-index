//! Project an ordered JS/TS batch using one staged compiler process.

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use serde::Deserialize;
    use serde_json::json;
    use tree_sitter_registry_consumer::symbols::{
        SymbolExtractError, SymbolLanguage, SyntaxBatchSource, SyntaxCandidateSession,
        SyntaxHelperArtifacts, SyntaxHelperLibrary,
    };

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Source {
        path: String,
        source: String,
    }

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let inventory_requested = args.len() >= 10 && args[args.len() - 3] == "--inventory";
    let (per_file_ms, total_ms) = if inventory_requested {
        let per_file_ms: u64 = args[args.len() - 2].parse()?;
        let total_ms: u64 = args[args.len() - 1].parse()?;
        if per_file_ms == 0 || per_file_ms > 600_000 || total_ms == 0 || total_ms > 3_600_000 {
            return Err("invalid syntax batch timeout".into());
        }
        args.truncate(args.len() - 3);
        (per_file_ms, total_ms)
    } else {
        (60_000, 300_000)
    };
    if args.len() < 7 || (args.len() - 7) % 2 != 0 || args[6] != "5.9.3" {
        return Err("invalid syntax batch artifact arguments".into());
    }
    let libraries: Vec<(PathBuf, String)> = args[7..]
        .chunks_exact(2)
        .map(|pair| (PathBuf::from(&pair[0]), pair[1].clone()))
        .collect();
    let library_refs: Vec<SyntaxHelperLibrary<'_>> = libraries
        .iter()
        .map(|(path, sha256)| SyntaxHelperLibrary { path, sha256 })
        .collect();
    let node = PathBuf::from(&args[0]);
    let typescript = PathBuf::from(&args[3]);
    let artifacts = SyntaxHelperArtifacts {
        node: &node,
        node_sha256: &args[1],
        node_version: &args[2],
        typescript_js: &typescript,
        typescript_sha256: &args[4],
        script_sha256: &args[5],
        node_libraries: &library_refs,
    };
    let mut input = Vec::new();
    let _read = std::io::stdin()
        .take((16 << 20) + 1)
        .read_to_end(&mut input)?;
    if input.len() > 16 << 20 {
        return Err("batch input exceeds 16 MiB".into());
    }
    let sources: Vec<Source> = serde_json::from_slice(&input)?;
    if sources.is_empty() || sources.len() > 32 {
        return Err("batch must contain 1..32 sources".into());
    }
    let total_deadline = Instant::now() + Duration::from_millis(total_ms);
    let mut batch = Vec::with_capacity(sources.len());
    for source in &sources {
        let grammar = match SymbolLanguage::from_path(&source.path) {
            Some(SymbolLanguage::JavaScript) => tree_sitter_javascript::LANGUAGE.into(),
            Some(SymbolLanguage::TypeScript { is_tsx: false }) => {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
            Some(SymbolLanguage::TypeScript { is_tsx: true }) => {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            _ => return Err(format!("unsupported syntax path: {}", source.path).into()),
        };
        batch.push(SyntaxBatchSource {
            path: &source.path,
            source: &source.source,
            grammar,
        });
    }
    let results = SyntaxCandidateSession::new(batch[0].path, &artifacts, total_deadline, &|| false)
        .and_then(|session| session.extract_batch(&batch, per_file_ms, total_deadline, &|| false));
    let results = match results {
        Ok(results) => results,
        Err(error) if inventory_requested => {
            let status = match error {
                SymbolExtractError::TimedOut { .. } => "timed_out",
                SymbolExtractError::Cancelled { .. } => "cancelled",
                SymbolExtractError::ResourceLimit { .. } => "resource_limit",
                _ => "producer_defect",
            };
            let rows = sources
                .iter()
                .map(|source| json!({"path":&source.path,"status":status,"records":[],"names":{}}))
                .collect::<Vec<_>>();
            serde_json::to_writer(std::io::stdout(), &rows)?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let mut output = Vec::with_capacity(sources.len());
    for (source, result) in sources.iter().zip(results) {
        let row = match result {
            Ok(result) if inventory_requested => {
                json!({"path":source.path,"status":"ok","records":result.records,"names":result.names})
            }
            Ok(result) => json!({"path":source.path,"status":"ok","records":result.records}),
            Err(SymbolExtractError::ParseFailure { .. }) => {
                if inventory_requested {
                    json!({"path":source.path,"status":"parse_failure","records":[],"names":{}})
                } else {
                    json!({"path":source.path,"status":"parse_failure","records":[]})
                }
            }
            Err(SymbolExtractError::TimedOut { .. }) if inventory_requested => {
                json!({"path":source.path,"status":"timed_out","records":[],"names":{}})
            }
            Err(SymbolExtractError::Cancelled { .. }) if inventory_requested => {
                json!({"path":source.path,"status":"cancelled","records":[],"names":{}})
            }
            Err(SymbolExtractError::ResourceLimit { .. }) if inventory_requested => {
                json!({"path":source.path,"status":"resource_limit","records":[],"names":{}})
            }
            Err(error) => return Err(format!("{}: {error}", source.path).into()),
        };
        output.push(row);
    }
    serde_json::to_writer(std::io::stdout(), &output)?;
    Ok(())
}

#[cfg(not(unix))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("official syntax batch is supported only on Unix".into())
}
