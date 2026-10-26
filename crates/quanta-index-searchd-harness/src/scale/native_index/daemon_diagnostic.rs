//! Preserve bounded child failure output before its temporary root is removed.

use std::fs::{self, Metadata, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, ensure};
use nix::fcntl::OFlag;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const MAX_STDERR_BYTES: u64 = 16 * 1024;
const OBJECT_IO_PREFIX: &str = "native-object-io ";
const OBJECT_IO_NUMERIC_FIELDS: [&str; 24] = [
    "write_calls",
    "written_bytes",
    "failed_writes",
    "enospc_writes",
    "file_sync_calls",
    "failed_file_syncs",
    "directory_sync_calls",
    "failed_directory_syncs",
    "barrier_calls",
    "failed_barriers",
    "reserve_calls",
    "reserve_objects",
    "single_object_reserves",
    "failed_reserves",
    "confirm_calls",
    "confirm_objects",
    "single_object_confirms",
    "failed_confirms",
    "write_elapsed_ns",
    "file_sync_elapsed_ns",
    "directory_sync_elapsed_ns",
    "barrier_elapsed_ns",
    "reserve_elapsed_ns",
    "confirm_elapsed_ns",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileEpoch {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    owner: u32,
    group: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
}

impl FileEpoch {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StderrTail {
    file: FileEpoch,
    offset: u64,
    bytes: Vec<u8>,
    sha256: String,
}

fn stderr_tail(path: &Path) -> Result<StderrTail> {
    let path_before = fs::symlink_metadata(path).context("stat native daemon stderr path")?;
    ensure!(
        path_before.is_file(),
        "native daemon stderr is not a regular file"
    );
    let epoch = FileEpoch::of(&path_before);
    // A FIFO swapped into the path between stat and open must not block capture.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits())
        .open(path)
        .context("open native daemon stderr")?;
    let descriptor_before = file
        .metadata()
        .context("stat opened native daemon stderr")?;
    ensure!(
        descriptor_before.is_file() && FileEpoch::of(&descriptor_before) == epoch,
        "native daemon stderr path changed before capture"
    );
    ensure!(
        descriptor_before.permissions().mode() & 0o777 == 0o600 && epoch.links == 1,
        "native daemon stderr must be private and singly linked"
    );
    let offset = epoch.size.saturating_sub(MAX_STDERR_BYTES);
    let tail_len = epoch
        .size
        .checked_sub(offset)
        .context("stderr tail length underflow")?;
    let count = usize::try_from(tail_len).context("stderr tail length overflow")?;
    let _position = file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; count];
    file.read_exact(&mut bytes)
        .context("read native daemon stderr tail")?;
    let descriptor_after = file
        .metadata()
        .context("restat opened native daemon stderr")?;
    let path_after = fs::symlink_metadata(path).context("restat native daemon stderr path")?;
    ensure!(
        FileEpoch::of(&descriptor_after) == epoch
            && path_after.is_file()
            && FileEpoch::of(&path_after) == epoch,
        "native daemon stderr changed during capture"
    );
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(StderrTail {
        file: epoch,
        offset,
        bytes,
        sha256,
    })
}

#[derive(Clone, Debug)]
pub(super) struct VerifiedDiagnostic {
    path: PathBuf,
    summary: Value,
    tail: StderrTail,
}

impl VerifiedDiagnostic {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn summary(&self) -> &Value {
        &self.summary
    }

    pub(super) fn verify(&self, path: &Path) -> Result<Value> {
        ensure!(
            path == self.path.as_path(),
            "native daemon stderr role path changed"
        );
        let current = stderr_tail(path)?;
        ensure!(
            current == self.tail,
            "native daemon stderr tail changed after drain verification"
        );
        Ok(json!({
            "path": path,
            "scope": "bounded last 16KiB of stderr only; full file content not hashed",
            "whole_file_bytes_at_drain": self.tail.file.size,
            "tail_offset": self.tail.offset,
            "tail_bytes": u64::try_from(self.tail.bytes.len())?,
            "tail_sha256": self.tail.sha256,
            "file_device": self.tail.file.device,
            "file_inode": self.tail.file.inode,
        }))
    }
}

fn parse_object_io_line(line: &str) -> Result<Map<String, Value>> {
    let fields = line
        .strip_prefix(OBJECT_IO_PREFIX)
        .ok_or_else(|| anyhow!("native object IO line has no prefix"))?;
    let mut parsed = Map::new();
    for field in fields.split(' ') {
        let (key, raw) = field
            .split_once('=')
            .ok_or_else(|| anyhow!("native object IO field is not key=value"))?;
        ensure!(
            !key.is_empty() && !raw.is_empty(),
            "empty native object IO field"
        );
        let value = match key {
            "capture_id" | "status" => Value::String(raw.to_owned()),
            _ if OBJECT_IO_NUMERIC_FIELDS.contains(&key) => {
                ensure!(
                    raw.bytes().all(|byte| byte.is_ascii_digit()),
                    "native object IO {key} is not decimal u64"
                );
                Value::from(
                    raw.parse::<u64>()
                        .with_context(|| format!("native object IO {key} is not unsigned u64"))?,
                )
            }
            _ => return Err(anyhow!("unknown native object IO field: {key}")),
        };
        ensure!(
            parsed.insert(key.to_owned(), value).is_none(),
            "duplicate native object IO field: {key}"
        );
    }
    Ok(parsed)
}

fn select_object_io_summary(tail: &StderrTail, capture_id: &str) -> Result<Map<String, Value>> {
    let text = String::from_utf8_lossy(&tail.bytes);
    let mut selected = None;
    let capture_field = format!("capture_id={capture_id}");
    for (index, chunk) in text.split_inclusive('\n').enumerate() {
        // A nonzero offset may start halfway through a line. A line without a
        // newline is incomplete, including a short-written final summary.
        if (tail.offset != 0 && index == 0) || !chunk.ends_with('\n') {
            continue;
        }
        let line = chunk.trim_end_matches('\n');
        if !line.starts_with("native-object-io") {
            continue;
        }
        if !line.split(' ').any(|field| field == capture_field.as_str()) {
            continue;
        }
        let parsed = parse_object_io_line(line)?;
        ensure!(selected.is_none(), "duplicate native object IO summary");
        selected = Some(parsed);
    }
    selected.ok_or_else(|| anyhow!("native object IO summary missing"))
}

/// Bind the bounded daemon summary to independently parsed capture counters.
///
/// Missing metrics remain missing; a complete trace footer does not substitute
/// for this line.
pub(super) fn verified_object_io(path: &Path, capture: &Value) -> Result<VerifiedDiagnostic> {
    let capture_id = capture
        .get("capture_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("capture has no string capture_id"))?;
    let capture_counter = |key: &str| {
        capture
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("capture has no unsigned {key}"))
    };
    let expected_calls = capture_counter("sync_calls")?;
    let expected_failures = capture_counter("sync_failures")?;
    let expected_elapsed_ns = capture_counter("sync_elapsed_ns")?;
    let tail = stderr_tail(path)?;
    let parsed = select_object_io_summary(&tail, capture_id)?;
    ensure!(
        parsed.get("status").and_then(Value::as_str) == Some("quiescent"),
        "native object IO summary unavailable"
    );
    for key in OBJECT_IO_NUMERIC_FIELDS {
        ensure!(
            parsed.get(key).and_then(Value::as_u64).is_some(),
            "native object IO {key} missing"
        );
    }
    ensure!(
        Some(parsed.len()) == OBJECT_IO_NUMERIC_FIELDS.len().checked_add(2),
        "native object IO summary has extra fields"
    );
    let observed = |key: &str| {
        parsed
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("native object IO {key} missing"))
    };
    ensure!(
        observed("enospc_writes")? <= observed("failed_writes")?
            && observed("failed_writes")? <= observed("write_calls")?,
        "native object IO write failure counts contradict reached writes"
    );
    let calls = [
        observed("file_sync_calls")?,
        observed("directory_sync_calls")?,
        observed("barrier_calls")?,
    ]
    .into_iter()
    .try_fold(0_u64, u64::checked_add)
    .ok_or_else(|| anyhow!("native object IO sync calls overflow"))?;
    let failures = [
        observed("failed_file_syncs")?,
        observed("failed_directory_syncs")?,
        observed("failed_barriers")?,
    ]
    .into_iter()
    .try_fold(0_u64, u64::checked_add)
    .ok_or_else(|| anyhow!("native object IO sync failures overflow"))?;
    let elapsed_ns = [
        observed("file_sync_elapsed_ns")?,
        observed("directory_sync_elapsed_ns")?,
        observed("barrier_elapsed_ns")?,
    ]
    .into_iter()
    .try_fold(0_u64, u64::checked_add)
    .ok_or_else(|| anyhow!("native object IO sync elapsed overflow"))?;
    ensure!(
        calls == expected_calls,
        "native object IO sync calls mismatch"
    );
    ensure!(
        failures == expected_failures,
        "native object IO sync failures mismatch"
    );
    ensure!(
        elapsed_ns == expected_elapsed_ns,
        "native object IO sync elapsed mismatch"
    );
    Ok(VerifiedDiagnostic {
        path: path.to_path_buf(),
        summary: Value::Object(parsed),
        tail,
    })
}

pub(super) fn attach(error: anyhow::Error, path: &Path) -> anyhow::Error {
    match stderr_tail(path) {
        Ok(tail) if tail.bytes.is_empty() => error,
        Ok(tail) => error.context(format!(
            "native daemon stderr tail (byte_offset={}, truncated={}): {}",
            tail.offset,
            tail.offset != 0,
            String::from_utf8_lossy(&tail.bytes)
        )),
        Err(read) => error.context(format!("native daemon stderr unavailable: {read:#}")),
    }
}

#[cfg(test)]
pub(super) fn write_balanced_stderr_fixture(path: &Path, capture: &Value) -> Result<()> {
    let id = capture
        .get("capture_id")
        .and_then(Value::as_str)
        .context("fixture capture ID absent")?;
    let calls = capture
        .get("sync_calls")
        .and_then(Value::as_u64)
        .context("fixture sync calls absent")?;
    let failures = capture
        .get("sync_failures")
        .and_then(Value::as_u64)
        .context("fixture sync failures absent")?;
    let elapsed_ns = capture
        .get("sync_elapsed_ns")
        .and_then(Value::as_u64)
        .context("fixture sync elapsed absent")?;
    let line = format!(
        "native-object-io capture_id={id} status=quiescent \
         write_calls=5 written_bytes=4096 failed_writes=0 enospc_writes=0 \
         file_sync_calls={calls} failed_file_syncs={failures} \
         directory_sync_calls=0 failed_directory_syncs=0 \
         barrier_calls=0 failed_barriers=0 \
         reserve_calls=1 reserve_objects=1 single_object_reserves=1 failed_reserves=0 \
         confirm_calls=1 confirm_objects=1 single_object_confirms=1 failed_confirms=0 \
         write_elapsed_ns=1 file_sync_elapsed_ns={elapsed_ns} directory_sync_elapsed_ns=0 \
         barrier_elapsed_ns=0 reserve_elapsed_ns=1 confirm_elapsed_ns=1\n"
    );
    fs::write(path, line)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_io_summary_binds_actual_fields_to_capture_and_refuses_partial_data() -> Result<()> {
        const ID: &str = "00112233445566778899aabbccddeeff";
        const LINE: &str = concat!(
            "native-object-io capture_id=00112233445566778899aabbccddeeff status=quiescent",
            " write_calls=5 written_bytes=4096 failed_writes=1 enospc_writes=1",
            " file_sync_calls=2 failed_file_syncs=1",
            " directory_sync_calls=3 failed_directory_syncs=0",
            " barrier_calls=4 failed_barriers=1",
            " reserve_calls=6 reserve_objects=7 single_object_reserves=2 failed_reserves=0",
            " confirm_calls=9 confirm_objects=10 single_object_confirms=3 failed_confirms=0",
            " write_elapsed_ns=12 file_sync_elapsed_ns=10 directory_sync_elapsed_ns=20",
            " barrier_elapsed_ns=30 reserve_elapsed_ns=40 confirm_elapsed_ns=50\n",
        );
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.stderr");
        let capture = serde_json::json!({
            "capture_id": ID,
            "sync_calls": 9,
            "sync_failures": 2,
            "sync_elapsed_ns": 60,
        });
        std::fs::write(&path, LINE)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let verified = verified_object_io(&path, &capture)?;
        let tail = verified.verify(&path)?;
        ensure!(tail.get("tail_offset") == Some(&Value::from(0_u64)));
        ensure!(tail.get("tail_bytes") == Some(&Value::from(557_u64)));
        ensure!(tail.get("whole_file_bytes_at_drain") == Some(&Value::from(557_u64)));
        ensure!(
            tail.get("tail_sha256")
                == Some(&Value::from(
                    "43d81e1c765114cf3e3ac76d6c46da1b40348b5bb24bdd78665380b741daf819"
                ))
        );
        let summary = verified.summary();
        ensure!(summary.get("written_bytes").and_then(Value::as_u64) == Some(4096));
        ensure!(summary.get("enospc_writes").and_then(Value::as_u64) == Some(1));
        ensure!(summary.get("reserve_objects").and_then(Value::as_u64) == Some(7));
        ensure!(summary.get("confirm_objects").and_then(Value::as_u64) == Some(10));
        ensure!(summary.get("file_sync_calls").and_then(Value::as_u64) == Some(2));
        ensure!(summary.get("capture_id").and_then(Value::as_str) == Some(ID));

        let mut wrong_counter = capture.clone();
        *wrong_counter
            .get_mut("sync_calls")
            .context("fixture sync calls absent")? = Value::from(8);
        let rejected = [
            ("counter mismatch", LINE.to_owned(), wrong_counter),
            ("duplicate", format!("{LINE}{LINE}"), capture.clone()),
            (
                "unavailable",
                format!(
                    "native-object-io capture_id={ID} status=unavailable reason=not_quiescent\n"
                ),
                capture.clone(),
            ),
            (
                "missing field",
                LINE.replace(" confirm_elapsed_ns=50", ""),
                capture.clone(),
            ),
            (
                "missing ENOSPC field",
                LINE.replace(" enospc_writes=1", ""),
                capture.clone(),
            ),
            (
                "ENOSPC exceeds failed writes",
                LINE.replace("enospc_writes=1", "enospc_writes=2"),
                capture.clone(),
            ),
            (
                "failed writes exceed reached calls",
                LINE.replace("failed_writes=1", "failed_writes=6"),
                capture.clone(),
            ),
            (
                "unknown key",
                LINE.replace(
                    " confirm_elapsed_ns=50\n",
                    " confirm_elapsed_ns=50 unknown=1\n",
                ),
                capture.clone(),
            ),
            (
                "not unsigned",
                LINE.replace("written_bytes=4096", "written_bytes=-1"),
                capture.clone(),
            ),
            (
                "signed positive",
                LINE.replace("written_bytes=4096", "written_bytes=+4096"),
                capture.clone(),
            ),
            (
                "sum overflow",
                LINE.replace("file_sync_calls=2", "file_sync_calls=18446744073709551615"),
                capture.clone(),
            ),
            ("missing", String::new(), capture.clone()),
            (
                "incomplete",
                LINE.trim_end_matches('\n').to_owned(),
                capture,
            ),
        ];
        for (case, text, capture) in rejected {
            std::fs::write(&path, text)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            ensure!(
                verified_object_io(&path, &capture).is_err(),
                "native object IO accepted {case}"
            );
        }
        Ok(())
    }

    #[test]
    fn child_tail_is_bounded_and_keeps_original_failure() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("child.stderr");
        let mut contents = vec![b'x'; 32 * 1024];
        contents.extend_from_slice(b"\nnative_work_refusal: bytes\n");
        std::fs::write(&path, contents)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let tail = stderr_tail(&path)?;
        anyhow::ensure!(tail.offset > 0, "oversize output was not truncated");
        anyhow::ensure!(tail.bytes.len() == 16 * 1024, "tail bound changed");
        anyhow::ensure!(tail.bytes.ends_with(b"native_work_refusal: bytes\n"));
        let error = attach(anyhow::anyhow!("original-refusal"), &path);
        anyhow::ensure!(error.root_cause().to_string() == "original-refusal");
        anyhow::ensure!(error.to_string().contains("truncated=true"));
        Ok(())
    }

    #[test]
    fn missing_child_output_preserves_primary_error() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let error = attach(
            anyhow::anyhow!("original-refusal"),
            &directory.path().join("missing"),
        );
        anyhow::ensure!(error.root_cause().to_string() == "original-refusal");
        anyhow::ensure!(error.to_string().contains("stderr unavailable"));
        Ok(())
    }

    #[test]
    fn bounded_stderr_window_rejects_tail_mutation_and_same_content_path_replacement() -> Result<()>
    {
        const FIXED_COUNT: &[u8] = b"written_bytes=4096";
        const REPLACED_COUNT: &[u8] = b"written_bytes=4097";
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("first.stderr");
        let capture = json!({
            "capture_id": "00112233445566778899aabbccddeeff",
            "sync_calls": 3,
            "sync_failures": 0,
            "sync_elapsed_ns": 27,
        });
        write_balanced_stderr_fixture(&path, &capture)?;
        let line = fs::read(&path)?;
        let mut contents = vec![b'x'; 20 * 1024];
        contents.extend_from_slice(b"\n");
        contents.extend_from_slice(&line);
        fs::write(&path, &contents)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let diagnostic = verified_object_io(&path, &capture)?;
        let detail = diagnostic.verify(&path)?;
        ensure!(
            detail
                .get("tail_offset")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0)
        );
        ensure!(detail.get("tail_bytes") == Some(&Value::from(MAX_STDERR_BYTES)));
        ensure!(
            detail
                .get("whole_file_bytes_at_drain")
                .and_then(Value::as_u64)
                == Some(u64::try_from(contents.len())?)
        );

        let mut changed = contents.clone();
        let at = changed
            .windows(FIXED_COUNT.len())
            .position(|part| part == FIXED_COUNT)
            .context("fixed fixture count absent")?;
        let end = at
            .checked_add(FIXED_COUNT.len())
            .context("fixed count end overflow")?;
        let target = changed
            .get_mut(at..end)
            .context("fixed count slice absent")?;
        target.copy_from_slice(REPLACED_COUNT);
        fs::write(&path, changed)?;
        ensure!(
            diagnostic.verify(&path).is_err(),
            "same-width tail mutation accepted"
        );

        fs::write(&path, &contents)?;
        let current = verified_object_io(&path, &capture)?;
        let replacement = directory.path().join("replacement.stderr");
        fs::write(&replacement, &contents)?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        fs::rename(&replacement, &path)?;
        ensure!(
            current.verify(&path).is_err(),
            "same-content path replacement accepted"
        );
        Ok(())
    }
}
