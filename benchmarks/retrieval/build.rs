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

fn bind_grammar(grammar: &Path, tag: &str, environment_key: &str) -> Result<(), Box<dyn Error>> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "cargo:rerun-if-changed={}", grammar.display())?;
    let mut files = Vec::new();
    source_files(grammar, &mut files)?;
    if files.is_empty() {
        return Err("vendored grammar input is empty".into());
    }
    let mut files = files
        .into_iter()
        .map(|file| {
            let relative = file
                .strip_prefix(grammar)?
                .to_str()
                .ok_or("non-UTF8 grammar path")?
                .replace('\\', "/");
            Ok((relative, file))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    hash.update(format!("quanta-index:vendored-{tag}-inputs:v1\0").as_bytes());
    for (relative, file) in files {
        let content = std::fs::read(&file)?;
        hash.update(u64::try_from(relative.len())?.to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update(u64::try_from(content.len())?.to_le_bytes());
        hash.update(&content);
    }
    writeln!(
        output,
        "cargo:rustc-env={environment_key}={:x}",
        hash.finalize()
    )?;
    Ok(())
}

fn bind_registry_grammar(
    lock_path: &Path,
    lock: &toml::Value,
    tag: &str,
    version: &str,
    environment_key: &str,
) -> Result<(), Box<dyn Error>> {
    let name = format!("tree-sitter-{tag}");
    let packages = lock["package"]
        .as_array()
        .ok_or("Cargo.lock package list missing")?;
    let matches = packages
        .iter()
        .filter(|package| package["name"].as_str() == Some(name.as_str()))
        .collect::<Vec<_>>();
    if matches.len() != 1 || matches[0]["version"].as_str() != Some(version) {
        return Err(format!("{name} lock identity differs").into());
    }
    let package = matches[0];
    if package["source"].as_str() != Some("registry+https://github.com/rust-lang/crates.io-index") {
        return Err(format!("{name} must resolve from crates.io").into());
    }
    let checksum = package["checksum"]
        .as_str()
        .ok_or("registry grammar checksum missing")?;
    if checksum.len() != 64
        || !checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{name} registry checksum malformed").into());
    }
    println!("cargo:rerun-if-changed={}", lock_path.display());
    println!("cargo:rustc-env={environment_key}={checksum}");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    for (tag, environment_key) in [
        ("typescript", "QI_TYPESCRIPT_GRAMMAR_SHA256"),
        ("go", "QI_GO_GRAMMAR_SHA256"),
        ("javascript", "QI_JAVASCRIPT_GRAMMAR_SHA256"),
    ] {
        bind_grammar(
            &manifest.join(format!("../../vendor/tree-sitter-{tag}")),
            tag,
            environment_key,
        )?;
    }
    let lock_path = manifest.join("../../Cargo.lock");
    let lock: toml::Value = std::fs::read_to_string(&lock_path)?.parse()?;
    for (tag, version, environment_key) in [
        ("rust", "0.24.2", "QI_RUST_GRAMMAR_SHA256"),
        ("python", "0.25.0", "QI_PYTHON_GRAMMAR_SHA256"),
    ] {
        bind_registry_grammar(&lock_path, &lock, tag, version, environment_key)?;
    }
    Ok(())
}
