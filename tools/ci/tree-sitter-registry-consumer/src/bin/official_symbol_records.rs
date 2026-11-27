//! Emit records from the shared Rust producer linked only to registry grammars.

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use tree_sitter_registry_consumer::symbols::{
        SymbolExtractError, SymbolLanguage, SyntaxHelperArtifacts, SyntaxHelperLibrary,
        extract_go_candidate_symbols_from_helper, extract_syntax_candidate_symbols_from_helper,
    };

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let inventory_requested = args.last().is_some_and(|arg| arg == "--inventory");
    if inventory_requested {
        let _ = args.pop();
    }
    let producer = args.first().ok_or("expected producer")?;
    let path = args.get(1).ok_or("expected repo-relative path")?;
    let mut source = Vec::new();
    let _read = std::io::stdin()
        .take((1 << 20) + 1)
        .read_to_end(&mut source)?;
    if source.len() > 1 << 20 {
        return Err("source exceeds 1 MiB".into());
    }
    let source = String::from_utf8(source)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let result = match producer.as_str() {
        "go" if args.len() == 5 => {
            if SymbolLanguage::from_path(path) != Some(SymbolLanguage::Go) {
                return Err("Go producer requires .go path".into());
            }
            extract_go_candidate_symbols_from_helper(
                path,
                &source,
                &PathBuf::from(&args[2]),
                &args[3],
                &args[4],
                tree_sitter_go::LANGUAGE.into(),
                deadline,
                || false,
            )
            .map(|candidate| (candidate.records, candidate.names))
        }
        "syntax" if args.len() >= 9 && (args.len() - 9).is_multiple_of(2) => {
            let grammar = match SymbolLanguage::from_path(path) {
                Some(SymbolLanguage::JavaScript) => tree_sitter_javascript::LANGUAGE.into(),
                Some(SymbolLanguage::TypeScript { is_tsx: false }) => {
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
                }
                Some(SymbolLanguage::TypeScript { is_tsx: true }) => {
                    tree_sitter_typescript::LANGUAGE_TSX.into()
                }
                _ => return Err("syntax producer requires JS/TS/TSX path".into()),
            };
            let libraries: Vec<(PathBuf, String)> = args[9..]
                .chunks_exact(2)
                .map(|pair| (PathBuf::from(&pair[0]), pair[1].clone()))
                .collect();
            let library_refs: Vec<SyntaxHelperLibrary<'_>> = libraries
                .iter()
                .map(|(path, sha256)| SyntaxHelperLibrary { path, sha256 })
                .collect();
            let node = PathBuf::from(&args[2]);
            let typescript = PathBuf::from(&args[5]);
            let artifacts = SyntaxHelperArtifacts {
                node: &node,
                node_sha256: &args[3],
                node_version: &args[4],
                typescript_js: &typescript,
                typescript_sha256: &args[6],
                script_sha256: &args[7],
                node_libraries: &library_refs,
            };
            // Position 8 is a caller-selected TypeScript release assertion.
            if args[8] != "5.9.3" {
                return Err("TypeScript version must be 5.9.3".into());
            }
            extract_syntax_candidate_symbols_from_helper(
                path,
                &source,
                &artifacts,
                grammar,
                deadline,
                || false,
            )
            .map(|candidate| (candidate.records, candidate.names))
        }
        _ => return Err("invalid candidate producer arguments".into()),
    };
    match result {
        Ok((records, names)) if inventory_requested => {
            serde_json::to_writer(
                std::io::stdout(),
                &serde_json::json!({"status":"ok","records":records,"names":names}),
            )?;
        }
        Ok((records, _)) => serde_json::to_writer(std::io::stdout(), &records)?,
        Err(SymbolExtractError::ParseFailure { .. }) if inventory_requested => {
            serde_json::to_writer(
                std::io::stdout(),
                &serde_json::json!({"status":"parse_failure","records":[],"names":{}}),
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(not(unix))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("official helper process is supported only on Unix".into())
}
