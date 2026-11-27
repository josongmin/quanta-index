//! Explicit Go candidate producer. The default symbol path remains separate.

use std::fs;
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::os::fd::AsFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::fcntl::{FcntlArg, OFlag, fcntl, open};
use nix::sys::signal::{Signal, killpg};
use nix::sys::stat::Mode;
use nix::unistd::Pid;
use sha2::{Digest, Sha256};
use tree_sitter::Language;

use super::{
    GoCandidateEdit, GoCandidateProjection, GoCandidateSymbols, SymbolExtractError, SymbolLanguage,
    extract_go_candidate_symbols,
};
use crate::sha256_hex;

const MAX_BINARY_BYTES: usize = 32 << 20;
const MAX_SOURCE_BYTES: usize = 1 << 20;
const MAX_RESPONSE_BYTES: usize = 8 << 20;
const MAX_STDERR_BYTES: usize = 1 << 16;
const MAX_EDITS: usize = 1 << 16;
const SCHEMA: &str = "quanta-go-grammar-projection-v1";

/// Verified helper bytes for a corpus census. Each extraction stages a fresh
/// executable, while the source artifact is read and hashed only once.
pub struct GoCandidateSession {
    binary: Vec<u8>,
    binary_sha256: String,
    go_version: String,
}

impl GoCandidateSession {
    pub fn new(
        path: &str,
        helper_path: &Path,
        helper_sha256: &str,
        go_version: &str,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, SymbolExtractError> {
        let binary = read_verified_file(
            helper_path,
            helper_sha256,
            MAX_BINARY_BYTES,
            path,
            deadline,
            cancelled,
        )?;
        Ok(Self {
            binary,
            binary_sha256: helper_sha256.into(),
            go_version: go_version.into(),
        })
    }

    pub fn extract(
        &self,
        path: &str,
        source: &str,
        grammar: Language,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<GoCandidateSymbols, SymbolExtractError> {
        if SymbolLanguage::from_path(path) != Some(SymbolLanguage::Go) {
            return Err(SymbolExtractError::Unsupported { path: path.into() });
        }
        if source.len() > MAX_SOURCE_BYTES {
            return Err(SymbolExtractError::ResourceLimit { path: path.into() });
        }
        check_budget(path, deadline, cancelled)?;
        let staged = tempfile::tempdir().map_err(|_| defect(path))?;
        let staged_binary = staged.path().join("go-helper");
        stage_verified_file(
            &staged_binary,
            &self.binary,
            true,
            path,
            deadline,
            cancelled,
        )?;
        let (output, errors, status) = run_bounded_child(
            path,
            source,
            Command::new(&staged_binary),
            deadline,
            cancelled,
        )?;
        if !status.success() || !errors.is_empty() {
            return Err(defect(path));
        }
        let response: HelperResponse = serde_json::from_slice(&output).map_err(|_| defect(path))?;
        if response.schema != SCHEMA
            || response.go_version != self.go_version
            || response.source_sha256 != sha256_hex(source.as_bytes())
            || response.edits.len() > MAX_EDITS
        {
            return Err(defect(path));
        }
        match response.status.as_str() {
            "parse_failure" if response.edits.is_empty() => {
                return Err(SymbolExtractError::ParseFailure { path: path.into() });
            }
            "resource_limit" if response.edits.is_empty() => {
                return Err(SymbolExtractError::ResourceLimit { path: path.into() });
            }
            "ok" => {}
            _ => return Err(defect(path)),
        }
        let mut projected = source.as_bytes().to_vec();
        let mut edits = Vec::with_capacity(response.edits.len());
        let mut previous_end = 0;
        for edit in response.edits {
            if edit.start < previous_end
                || edit.start >= edit.end
                || edit.end > projected.len()
                || edit.replacement.len() != edit.end.saturating_sub(edit.start)
            {
                return Err(defect(path));
            }
            projected[edit.start..edit.end].copy_from_slice(edit.replacement.as_bytes());
            previous_end = edit.end;
            edits.push(GoCandidateEdit {
                start: edit.start,
                end: edit.end,
                replacement: edit.replacement,
            });
        }
        let projected = String::from_utf8(projected).map_err(|_| defect(path))?;
        extract_go_candidate_symbols(
            path,
            source,
            &GoCandidateProjection {
                projected_sha256: sha256_hex(projected.as_bytes()),
                source_sha256: response.source_sha256,
                helper_binary_sha256: self.binary_sha256.clone(),
                edits,
                projected,
            },
            grammar,
        )
    }
}

struct HelperResponse {
    schema: String,
    status: String,
    go_version: String,
    source_sha256: String,
    edits: Vec<HelperEdit>,
}

strict_symbol_object!(HelperResponse {
    schema: String,
    status: String,
    go_version: String,
    source_sha256: String,
    edits: Vec<HelperEdit>,
});

struct HelperEdit {
    start: usize,
    end: usize,
    replacement: String,
}

strict_symbol_object!(HelperEdit {
    start: usize,
    end: usize,
    replacement: String,
});

fn defect(path: &str) -> SymbolExtractError {
    SymbolExtractError::ProducerDefect {
        detail: format!("Go candidate helper failed: {path}"),
    }
}

pub(super) fn check_budget(
    source_path: &str,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<(), SymbolExtractError> {
    if cancelled() {
        return Err(SymbolExtractError::Cancelled {
            path: source_path.into(),
        });
    }
    if Instant::now() >= deadline {
        return Err(SymbolExtractError::TimedOut {
            path: source_path.into(),
        });
    }
    Ok(())
}

pub(super) fn read_verified_file(
    path: &Path,
    expected_sha256: &str,
    maximum: usize,
    source_path: &str,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<u8>, SymbolExtractError> {
    check_budget(source_path, deadline, cancelled)?;
    let fd = open(
        path,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| defect(source_path))?;
    let mut file = fs::File::from(fd);
    let metadata = file.metadata().map_err(|_| defect(source_path))?;
    if !metadata.file_type().is_file()
        || metadata.len() > u64::try_from(maximum).unwrap_or(u64::MAX)
    {
        return Err(defect(source_path));
    }
    let mut bytes = Vec::new();
    let mut hasher = Sha256::new();
    let mut chunk = [0_u8; 16 << 10];
    loop {
        check_budget(source_path, deadline, cancelled)?;
        let count = file.read(&mut chunk).map_err(|_| defect(source_path))?;
        if count == 0 {
            break;
        }
        if count > maximum.saturating_sub(bytes.len()) {
            return Err(SymbolExtractError::ResourceLimit {
                path: source_path.into(),
            });
        }
        hasher.update(&chunk[..count]);
        bytes.extend_from_slice(&chunk[..count]);
    }
    check_budget(source_path, deadline, cancelled)?;
    if format!("{:x}", hasher.finalize()) != expected_sha256 {
        return Err(defect(source_path));
    }
    Ok(bytes)
}

pub(super) fn stage_verified_file(
    path: &Path,
    bytes: &[u8],
    executable: bool,
    source_path: &str,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<(), SymbolExtractError> {
    check_budget(source_path, deadline, cancelled)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| defect(source_path))?;
    for chunk in bytes.chunks(16 << 10) {
        check_budget(source_path, deadline, cancelled)?;
        file.write_all(chunk).map_err(|_| defect(source_path))?;
    }
    file.set_permissions(fs::Permissions::from_mode(if executable {
        0o500
    } else {
        0o400
    }))
    .map_err(|_| defect(source_path))?;
    check_budget(source_path, deadline, cancelled)
}

fn kill_group(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

fn stop_child(child: &mut Child, owns_group: bool) {
    if owns_group {
        kill_group(child);
    }
    drop(child.kill());
    drop(child.wait());
}

struct ChildGroup {
    child: Child,
    armed: bool,
    owns_group: bool,
}

impl Deref for ChildGroup {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl DerefMut for ChildGroup {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for ChildGroup {
    fn drop(&mut self) {
        if self.armed {
            stop_child(&mut self.child, self.owns_group);
        }
    }
}

fn set_nonblocking(fd: impl AsFd) -> nix::Result<()> {
    let flags = fcntl(fd.as_fd(), FcntlArg::F_GETFL)?;
    let _updated = fcntl(
        fd.as_fd(),
        FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
    )?;
    Ok(())
}

pub(super) fn run_bounded_child(
    path: &str,
    source: &str,
    command: Command,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), SymbolExtractError> {
    run_child(path, source, command, deadline, cancelled, true)
}

/// Run a leaf process that uses threads only. Inherit the caller's group so
/// cancelling an outer batch kills both the batch executable and this child.
pub(super) fn run_bounded_leaf_child(
    path: &str,
    source: &str,
    command: Command,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), SymbolExtractError> {
    run_child(path, source, command, deadline, cancelled, false)
}

fn run_child(
    path: &str,
    source: &str,
    mut command: Command,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
    owns_group: bool,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), SymbolExtractError> {
    check_budget(path, deadline, cancelled)?;
    if owns_group {
        let _configured = command.process_group(0);
    }
    let child = command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| defect(path))?;
    let mut child = ChildGroup {
        child,
        armed: true,
        owns_group,
    };
    let Some(stdin) = child.stdin.take() else {
        return Err(defect(path));
    };
    let Some(stdout) = child.stdout.take() else {
        return Err(defect(path));
    };
    let Some(stderr) = child.stderr.take() else {
        return Err(defect(path));
    };
    if set_nonblocking(&stdin).is_err()
        || set_nonblocking(&stdout).is_err()
        || set_nonblocking(&stderr).is_err()
    {
        return Err(defect(path));
    }
    let mut stdin = Some(stdin);
    let mut stdout = Some(stdout);
    let mut stderr = Some(stderr);
    let mut offset = 0;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut status = None;
    let mut buffer = [0_u8; 65536];
    loop {
        if cancelled() {
            return Err(SymbolExtractError::Cancelled { path: path.into() });
        }
        if Instant::now() >= deadline {
            return Err(SymbolExtractError::TimedOut { path: path.into() });
        }
        let mut progressed = false;
        if let Some(pipe) = stdin.as_mut() {
            if offset == source.len() {
                stdin = None;
                progressed = true;
            } else {
                match pipe.write(
                    source
                        .as_bytes()
                        .get(offset..)
                        .ok_or_else(|| defect(path))?,
                ) {
                    Ok(wrote) => {
                        offset = offset.saturating_add(wrote);
                        progressed = wrote > 0;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                        stdin = None;
                        progressed = true;
                    }
                    Err(_) => {
                        return Err(defect(path));
                    }
                }
            }
        }
        if let Some(pipe) = stdout.as_mut() {
            let room = MAX_RESPONSE_BYTES
                .saturating_sub(output.len())
                .saturating_add(1);
            let read_len = room.min(buffer.len());
            let chunk = buffer.get_mut(..read_len).ok_or_else(|| defect(path))?;
            match pipe.read(chunk) {
                Ok(0) => {
                    stdout = None;
                    progressed = true;
                }
                Ok(count) => {
                    output.extend_from_slice(buffer.get(..count).ok_or_else(|| defect(path))?);
                    if output.len() > MAX_RESPONSE_BYTES {
                        return Err(SymbolExtractError::ResourceLimit { path: path.into() });
                    }
                    progressed = true;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => {
                    return Err(defect(path));
                }
            }
        }
        if let Some(pipe) = stderr.as_mut() {
            let room = MAX_STDERR_BYTES
                .saturating_sub(errors.len())
                .saturating_add(1);
            let read_len = room.min(buffer.len());
            let chunk = buffer.get_mut(..read_len).ok_or_else(|| defect(path))?;
            match pipe.read(chunk) {
                Ok(0) => {
                    stderr = None;
                    progressed = true;
                }
                Ok(count) => {
                    errors.extend_from_slice(buffer.get(..count).ok_or_else(|| defect(path))?);
                    if errors.len() > MAX_STDERR_BYTES {
                        return Err(SymbolExtractError::ResourceLimit { path: path.into() });
                    }
                    progressed = true;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => {
                    return Err(defect(path));
                }
            }
        }
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(_) => {
                    return Err(defect(path));
                }
            };
        }
        if stdin.is_none() && stdout.is_none() && stderr.is_none() && status.is_some() {
            break;
        }
        if !progressed {
            thread::sleep(Duration::from_millis(5));
        }
    }
    let status = status.ok_or_else(|| defect(path))?;
    child.armed = false;
    Ok((output, errors, status))
}

/// Execute a pinned helper, then extract source-bound records with an explicitly
/// supplied grammar. The digest is checked against the bytes actually staged
/// for execution. This does not select or qualify the default producer.
pub fn extract_go_candidate_symbols_from_helper(
    path: &str,
    source: &str,
    helper_path: &Path,
    helper_sha256: &str,
    go_version: &str,
    grammar: Language,
    deadline: Instant,
    cancelled: impl Fn() -> bool,
) -> Result<GoCandidateSymbols, SymbolExtractError> {
    let session = GoCandidateSession::new(
        path,
        helper_path,
        helper_sha256,
        go_version,
        deadline,
        &cancelled,
    )?;
    session.extract(path, source, grammar, deadline, &cancelled)
}

#[cfg(test)]
mod hash_budget_tests {
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::*;

    #[test]
    fn verified_file_hash_checks_cancellation_between_chunks() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let artifact = directory.path().join("large-helper");
        fs::write(&artifact, vec![0x5a; 16 << 20]).expect("fixture bytes");
        let checks = Cell::new(0);
        let result = read_verified_file(
            &artifact,
            &"0".repeat(64),
            32 << 20,
            "p.go",
            Instant::now() + Duration::from_secs(60),
            &|| {
                checks.set(checks.get() + 1);
                checks.get() == 5
            },
        );
        assert!(matches!(result, Err(SymbolExtractError::Cancelled { .. })));
        assert_eq!(checks.get(), 5);
    }

    #[test]
    fn verified_file_hash_obeys_deadline_during_large_digest() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let artifact = directory.path().join("large-helper");
        fs::write(&artifact, vec![0x5a; 16 << 20]).expect("fixture bytes");
        let result = read_verified_file(
            &artifact,
            &"0".repeat(64),
            32 << 20,
            "p.go",
            Instant::now() + Duration::from_millis(10),
            &|| false,
        );
        assert!(matches!(result, Err(SymbolExtractError::TimedOut { .. })));
    }

    #[test]
    fn verified_go_session_survives_original_helper_replacement() {
        let source = "package p\nfunc One() {}\n";
        let response = format!(
            "{{\"schema\":\"{SCHEMA}\",\"status\":\"ok\",\"go_version\":\"go1.25.4\",\"source_sha256\":\"{}\",\"edits\":[]}}",
            sha256_hex(source.as_bytes())
        );
        let script = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{response}'\n");
        let directory = tempfile::tempdir().expect("fixture directory");
        let helper = directory.path().join("helper");
        fs::write(&helper, &script).expect("fixture helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("executable");
        let digest = sha256_hex(script.as_bytes());
        let session = GoCandidateSession::new(
            "p.go",
            &helper,
            &digest,
            "go1.25.4",
            Instant::now() + Duration::from_secs(5),
            &|| false,
        )
        .expect("verified helper bytes");
        fs::write(&helper, "#!/bin/sh\nexit 1\n").expect("replace source path");
        for _ in 0..2 {
            let result = session
                .extract(
                    "p.go",
                    source,
                    tree_sitter_go::LANGUAGE.into(),
                    Instant::now() + Duration::from_secs(5),
                    &|| false,
                )
                .expect("session uses verified bytes");
            assert_eq!(result.records.len(), 1);
            assert_eq!(result.records[0].local_name.as_ref(), "One");
            assert_eq!(result.names.len(), 1);
            assert_eq!(
                result.names[result.records[0].symbol_id.as_str()].name,
                "One"
            );
            assert_eq!(result.helper_binary_sha256, digest);
        }
    }
}

#[cfg(all(test, not(tree_sitter_registry_consumer)))]
mod tests {
    use super::*;
    use crate::symbols::extract_symbols;

    #[test]
    fn staged_helper_response_reaches_source_bound_symbol_extractor() {
        let source = "package p\nfunc One() {}\n";
        let response = format!(
            "{{\"schema\":\"{SCHEMA}\",\"status\":\"ok\",\"go_version\":\"go1.25.4\",\"source_sha256\":\"{}\",\"edits\":[]}}",
            sha256_hex(source.as_bytes())
        );
        let script = format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{response}'\n");
        let directory = tempfile::tempdir().expect("fixture directory");
        let helper = directory.path().join("helper");
        fs::write(&helper, script.as_bytes()).expect("fixture binary");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("executable");
        let digest = sha256_hex(script.as_bytes());
        let run = |expected: &str| {
            extract_go_candidate_symbols_from_helper(
                "p.go",
                source,
                &helper,
                expected,
                "go1.25.4",
                tree_sitter_go::LANGUAGE.into(),
                Instant::now() + Duration::from_secs(5),
                || false,
            )
        };
        let result = run(&digest).expect("staged helper");
        assert_eq!(
            result.records,
            extract_symbols("p.go", source).expect("vendor baseline")
        );
        assert_eq!(result.helper_binary_sha256, digest);
        assert!(matches!(
            run(&"0".repeat(64)),
            Err(SymbolExtractError::ProducerDefect { .. })
        ));
    }

    #[test]
    #[ignore = "requires the pinned Go 1.25.4 toolchain"]
    fn pinned_go_helper_runs_before_rust_symbol_extraction() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let helper = directory.path().join("go-helper");
        let source_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/benchmark/retrieval/go_projection/main.go");
        let build = Command::new("go")
            .env("GOTOOLCHAIN", "local")
            .arg("build")
            .arg("-o")
            .arg(&helper)
            .arg(&source_path)
            .status()
            .expect("Go toolchain");
        assert!(build.success(), "Go helper build");
        let digest = sha256_hex(&fs::read(&helper).expect("built helper"));
        let source = "package p\ntype Rect struct{W float64}\nfunc Run(){_=new(1)}\n";
        let result = extract_go_candidate_symbols_from_helper(
            "p.go",
            source,
            &helper,
            &digest,
            "go1.25.4",
            tree_sitter_go::LANGUAGE.into(),
            Instant::now() + Duration::from_secs(15),
            || false,
        )
        .expect("real Go helper and Rust extraction");
        assert_eq!(
            result.records,
            extract_symbols("p.go", source).expect("vendor baseline")
        );
        assert_eq!(result.helper_binary_sha256, digest);
        assert_ne!(result.source_sha256, result.projected_sha256);
    }

    #[test]
    fn oversized_child_stderr_fails_before_deadline() {
        let script = "#!/bin/sh\ncat >/dev/null\nprintf '%065537d' 0 >&2\nsleep 5\n";
        let directory = tempfile::tempdir().expect("fixture directory");
        let helper = directory.path().join("helper");
        fs::write(&helper, script).expect("fixture binary");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("executable");
        let started = Instant::now();
        let result = extract_go_candidate_symbols_from_helper(
            "p.go",
            "package p\n",
            &helper,
            &sha256_hex(script.as_bytes()),
            "go1.25.4",
            tree_sitter_go::LANGUAGE.into(),
            started + Duration::from_secs(5),
            || false,
        );
        assert!(matches!(
            result,
            Err(SymbolExtractError::ResourceLimit { .. })
        ));
    }

    #[test]
    fn exited_parent_with_inherited_pipe_obeys_deadline() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let marker = directory.path().join("started");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\n: > \"{}\"\nsleep 10 &\nexit 0\n",
            marker.display()
        );
        let helper = directory.path().join("helper");
        fs::write(&helper, &script).expect("fixture binary");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("executable");
        let started = Instant::now();
        let result = extract_go_candidate_symbols_from_helper(
            "p.go",
            "package p\n",
            &helper,
            &sha256_hex(script.as_bytes()),
            "go1.25.4",
            tree_sitter_go::LANGUAGE.into(),
            started + Duration::from_secs(3),
            || false,
        );
        assert!(matches!(result, Err(SymbolExtractError::TimedOut { .. })));
        assert!(marker.is_file());
    }

    #[test]
    fn helper_fifo_is_refused_without_blocking() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let helper = directory.path().join("helper");
        nix::unistd::mkfifo(&helper, Mode::S_IRUSR | Mode::S_IWUSR).expect("fixture fifo");
        let started = Instant::now();
        let result = extract_go_candidate_symbols_from_helper(
            "p.go",
            "package p\n",
            &helper,
            &"0".repeat(64),
            "go1.25.4",
            tree_sitter_go::LANGUAGE.into(),
            started + Duration::from_secs(2),
            || false,
        );
        assert!(matches!(
            result,
            Err(SymbolExtractError::ProducerDefect { .. })
        ));
    }

    #[test]
    fn cancellation_callback_panic_reaps_the_child_group() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let marker = directory.path().join("pid");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s' $$ > \"{}\"\nsleep 10\n",
            marker.display()
        );
        let helper = directory.path().join("helper");
        fs::write(&helper, &script).expect("fixture binary");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("executable");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            extract_go_candidate_symbols_from_helper(
                "p.go",
                "package p\n",
                &helper,
                &sha256_hex(script.as_bytes()),
                "go1.25.4",
                tree_sitter_go::LANGUAGE.into(),
                Instant::now() + Duration::from_secs(10),
                || {
                    if marker.is_file() {
                        panic!("injected cancellation callback failure");
                    }
                    false
                },
            )
        }));
        assert!(result.is_err());
        let pid: i32 = fs::read_to_string(&marker)
            .expect("child started")
            .parse()
            .expect("child PID");
        assert!(matches!(
            nix::sys::signal::kill(Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        ));
    }
}
