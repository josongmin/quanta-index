//! Argument parsing for artifact producers with only an optional output path.

use std::ffi::OsString;
use std::path::PathBuf;

pub(super) fn parse(
    args: impl IntoIterator<Item = OsString>,
    default: &str,
) -> anyhow::Result<PathBuf> {
    let mut args = args.into_iter();
    let Some(flag) = args.next() else {
        return Ok(PathBuf::from(default));
    };
    anyhow::ensure!(flag == "--out-dir", "expected --out-dir PATH, got {flag:?}");
    let path = args
        .next()
        .filter(|path| !path.is_empty() && !path.as_encoded_bytes().starts_with(b"--"))
        .ok_or_else(|| anyhow::anyhow!("--out-dir requires a directory path"))?;
    anyhow::ensure!(
        args.next().is_none(),
        "unexpected argument after --out-dir PATH"
    );
    Ok(PathBuf::from(path))
}
