use std::error::Error;
use std::path::PathBuf;

fn bind_registry_grammar(
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
    let [package] = matches.as_slice() else {
        return Err(format!("{name} lock identity differs").into());
    };
    if package["version"].as_str() != Some(version) {
        return Err(format!("{name} lock identity differs").into());
    }
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
    println!("cargo:rustc-env={environment_key}={checksum}");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rustc-check-cfg=cfg(tree_sitter_registry_consumer)");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let lock_path = manifest.join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let lock: toml::Value = std::fs::read_to_string(&lock_path)?.parse()?;
    for (tag, version, environment_key) in [
        ("rust", "0.24.2", "QI_RUST_GRAMMAR_SHA256"),
        ("go", "0.25.0", "QI_GO_GRAMMAR_SHA256"),
        ("javascript", "0.25.0", "QI_JAVASCRIPT_GRAMMAR_SHA256"),
        ("python", "0.25.0", "QI_PYTHON_GRAMMAR_SHA256"),
        ("typescript", "0.23.2", "QI_TYPESCRIPT_GRAMMAR_SHA256"),
    ] {
        bind_registry_grammar(&lock, tag, version, environment_key)?;
    }
    Ok(())
}
