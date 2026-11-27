//! Produce a bounded Go corpus with one verified helper-byte session.

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use serde::Deserialize;
    use serde_json::json;
    use tree_sitter_registry_consumer::symbols::{
        GoCandidateSession, SymbolExtractError, SymbolLanguage,
    };

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Source {
        path: String,
        source: String,
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("expected Go helper path, SHA-256, and version".into());
    }
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
    if sources
        .iter()
        .any(|source| SymbolLanguage::from_path(&source.path) != Some(SymbolLanguage::Go))
    {
        return Err("Go batch requires .go paths".into());
    }
    let total_deadline = Instant::now() + Duration::from_secs(300);
    let session = GoCandidateSession::new(
        &sources[0].path,
        &PathBuf::from(&args[0]),
        &args[1],
        &args[2],
        (Instant::now() + Duration::from_secs(60)).min(total_deadline),
        &|| false,
    )?;
    let mut output = Vec::with_capacity(sources.len());
    for source in sources {
        let row = match session.extract(
            &source.path,
            &source.source,
            tree_sitter_go::LANGUAGE.into(),
            (Instant::now() + Duration::from_secs(60)).min(total_deadline),
            &|| false,
        ) {
            Ok(result) => json!({"path":source.path,"status":"ok","records":result.records}),
            Err(SymbolExtractError::ParseFailure { .. }) => {
                json!({"path":source.path,"status":"parse_failure","records":[]})
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
    Err("official Go batch is supported only on Unix".into())
}
