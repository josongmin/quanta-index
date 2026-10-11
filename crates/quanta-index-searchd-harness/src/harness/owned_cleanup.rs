//! Bounded teardown of a harness-owned, driver-stopped state root.

use std::fs;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::thread;

use tempfile::TempDir;

const POOL: &str = "lexical-object-pool";
const OBJECTS: &str = "objects";
const MAX_SHARDS: usize = 256;
const MAX_WORKERS: usize = 4;

enum Shards {
    Fallback,
    Valid(Vec<PathBuf>),
}

fn real_directory(path: &Path) -> io::Result<Option<bool>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn valid_shard_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// A refusal uses `TempDir::close` alone. No parallel worker follows a link or
/// traverses a layout that is not the private object's canonical shard tree.
fn inspect(root: &Path) -> io::Result<Shards> {
    if real_directory(root)? != Some(true) {
        return Ok(Shards::Fallback);
    }
    let pool = root.join(POOL);
    if real_directory(&pool)? != Some(true) {
        return Ok(Shards::Fallback);
    }
    let objects = pool.join(OBJECTS);
    if real_directory(&objects)? != Some(true) {
        return Ok(Shards::Fallback);
    }
    let mut shards = Vec::with_capacity(MAX_SHARDS);
    for entry in fs::read_dir(objects)? {
        let entry = entry?;
        if shards.len() == MAX_SHARDS {
            return Ok(Shards::Fallback);
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Ok(Shards::Fallback);
        };
        if !valid_shard_name(name) {
            return Ok(Shards::Fallback);
        }
        let path = entry.path();
        match real_directory(&path)? {
            Some(true) => shards.push(path),
            None => return Ok(Shards::Fallback),
            Some(false) if entry.file_type()?.is_symlink() => return Ok(Shards::Fallback),
            Some(false) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("owned object shard is not a directory: {}", path.display()),
                ));
            }
        }
    }
    shards.sort_unstable();
    Ok(Shards::Valid(shards))
}

/// Each worker owns disjoint, sorted shard paths. Join every worker even when
/// one fails; report the first failure in path order, independent of timing.
fn remove_shards_with<F>(shards: &[PathBuf], remove: &F) -> io::Result<()>
where
    F: Fn(&Path) -> io::Result<()> + Sync,
{
    if shards.is_empty() {
        return Ok(());
    }
    let workers = shards.len().min(MAX_WORKERS);
    let chunk_size = shards.len().div_ceil(workers);
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        let mut spawn_error = None;
        for chunk in shards.chunks(chunk_size) {
            match thread::Builder::new().spawn_scoped(scope, move || {
                for path in chunk {
                    remove(path).map_err(|error| {
                        io::Error::new(
                            error.kind(),
                            format!("remove shard {}: {error}", path.display()),
                        )
                    })?;
                }
                Ok(())
            }) {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    spawn_error = Some(error);
                    break;
                }
            }
        }
        let mut first_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
                Err(_panic) => {
                    if first_error.is_none() {
                        first_error = Some(io::Error::other("owned shard cleanup worker panicked"));
                    }
                }
            }
        }
        first_error.or(spawn_error).map_or(Ok(()), Err)
    })
}

/// The prepass is only an optimization.
///
/// The `TempDir` owner always consumes the
/// root once, including on prepass failure or panic, within the caller's
/// existing `RuntimeStop` span and whole-call deadline.
pub(super) fn close(tempdir: TempDir) -> io::Result<()> {
    let prepass = match catch_unwind(AssertUnwindSafe(|| match inspect(tempdir.path())? {
        Shards::Fallback => Ok(()),
        Shards::Valid(shards) => {
            remove_shards_with(&shards, &|path: &Path| fs::remove_dir_all(path))
        }
    })) {
        Ok(result) => result,
        Err(_panic) => Err(io::Error::other("owned shard cleanup panicked")),
    };
    let close = tempdir.close();
    match (prepass, close) {
        (Ok(()), close) => close,
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(close)) => Err(io::Error::new(
            primary.kind(),
            format!("{primary}; remaining owned root cleanup also failed: {close}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    type TestResult = anyhow::Result<()>;

    fn objects(tempdir: &TempDir) -> io::Result<PathBuf> {
        let path = tempdir.path().join(POOL).join(OBJECTS);
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    #[test]
    fn more_than_four_populated_shards_close_with_owned_root() -> TestResult {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path().to_path_buf();
        let objects = objects(&tempdir)?;
        for index in 0..8 {
            let shard = objects.join(format!("{index:02x}"));
            fs::create_dir(&shard)?;
            fs::write(shard.join("object"), b"owned")?;
        }
        anyhow::ensure!(matches!(inspect(&root)?, Shards::Valid(shards) if shards.len() == 8));
        close(tempdir)?;
        anyhow::ensure!(!root.exists());
        Ok(())
    }

    #[test]
    fn malformed_and_linked_shards_fall_back_without_touching_foreign_target() -> TestResult {
        let foreign = tempfile::tempdir()?;
        let witness = foreign.path().join("witness");
        fs::write(&witness, b"foreign")?;
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path().to_path_buf();
        let objects = objects(&tempdir)?;
        fs::create_dir(objects.join("not-hex"))?;
        anyhow::ensure!(matches!(inspect(&root)?, Shards::Fallback));
        fs::remove_dir(objects.join("not-hex"))?;
        std::os::unix::fs::symlink(foreign.path(), objects.join("00"))?;
        anyhow::ensure!(matches!(inspect(&root)?, Shards::Fallback));
        close(tempdir)?;
        anyhow::ensure!(fs::read(witness)? == b"foreign");
        Ok(())
    }

    #[test]
    fn linked_ancestor_refuses_parallel_prepass_and_falls_back() -> TestResult {
        let foreign = tempfile::tempdir()?;
        let witness = foreign.path().join("witness");
        fs::write(&witness, b"foreign")?;
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path().to_path_buf();
        std::os::unix::fs::symlink(foreign.path(), root.join(POOL))?;
        anyhow::ensure!(matches!(inspect(&root)?, Shards::Fallback));
        close(tempdir)?;
        anyhow::ensure!(!root.exists());
        anyhow::ensure!(fs::read(witness)? == b"foreign");
        Ok(())
    }

    #[test]
    fn non_directory_shard_reports_failure_after_consuming_root() -> TestResult {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path().to_path_buf();
        fs::write(objects(&tempdir)?.join("00"), b"not a shard")?;
        let failure = close(tempdir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("non-directory shard was accepted"))?;
        anyhow::ensure!(failure.kind() == io::ErrorKind::InvalidData);
        anyhow::ensure!(!root.exists());
        Ok(())
    }

    #[test]
    fn worker_failure_joins_every_started_worker() -> TestResult {
        let tempdir = tempfile::tempdir()?;
        let objects = objects(&tempdir)?;
        let mut shards = Vec::new();
        for index in 0..8 {
            let shard = objects.join(format!("{index:02x}"));
            fs::create_dir(&shard)?;
            shards.push(shard);
        }
        let completed = AtomicUsize::new(0);
        let result = remove_shards_with(&shards, &|path| {
            if path.ends_with("00") {
                return Err(io::Error::other("injected first shard failure"));
            }
            fs::remove_dir_all(path)?;
            let _previous = completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        anyhow::ensure!(
            result
                .err()
                .ok_or_else(|| anyhow::anyhow!("worker failure lost"))?
                .kind()
                == io::ErrorKind::Other
        );
        anyhow::ensure!(completed.load(Ordering::SeqCst) == 6);
        anyhow::ensure!(shards.iter().skip(2).all(|shard| !shard.exists()));
        tempdir.close()?;
        Ok(())
    }
}
