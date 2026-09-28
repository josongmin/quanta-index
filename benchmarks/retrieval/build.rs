use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn source_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(format!(
                "vendored grammar source must not be a symlink: {}",
                entry.path().display()
            )
            .into());
        }
        if kind.is_dir() {
            source_files(&entry.path(), files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        } else {
            return Err(format!(
                "unexpected vendored grammar input: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let grammar = manifest.join("../../vendor/tree-sitter-typescript");
    let mut output = std::io::stdout().lock();
    writeln!(output, "cargo:rerun-if-changed={}", grammar.display())?;
    let mut files = Vec::new();
    source_files(&grammar, &mut files)?;
    if files.is_empty() {
        return Err("vendored TypeScript grammar input is empty".into());
    }
    let mut files = files
        .into_iter()
        .map(|file| {
            let relative = file
                .strip_prefix(&grammar)?
                .to_str()
                .ok_or("non-UTF8 grammar path")?
                .replace('\\', "/");
            Ok((relative, file))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:vendored-typescript-inputs:v1\0");
    for (relative, file) in files {
        let content = std::fs::read(&file)?;
        hash.update(u64::try_from(relative.len())?.to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update(u64::try_from(content.len())?.to_le_bytes());
        hash.update(&content);
    }
    writeln!(
        output,
        "cargo:rustc-env=QI_TYPESCRIPT_GRAMMAR_SHA256={:x}",
        hash.finalize()
    )?;
    Ok(())
}
