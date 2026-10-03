//! Benchmark artifact data model (`BenchArtifactV1`), its one writer, and
//! the percentile math every rail shares.
//!
//! A benchmark number that cannot say which source, corpus, configuration,
//! model and host it came from is not evidence (QI-BB-010, findings §9).
//! Every benchmark and relevance artifact this harness emits is therefore
//! one [`BenchArtifactV1`] envelope: a schema version, the dimension it
//! measures, the cache mode, the client concurrency, a
//! [`BenchProvenanceV1`] (the exact 40-character `git_head`, the corpus,
//! config and model digests), the [`HostV1`] it ran on, the process's peak
//! RSS, the build / update / GC phase durations, the disk amplification
//! where a rail writes an index, and the measured rows. Dimension-specific
//! shape (tier manifests, per-route budgets, judged queries) travels under
//! `detail` so the envelope stays one contract for every rail and one
//! gate (`tools/ci/lint/check-bench-artifacts.py`) can refuse a stale or
//! unattributed artifact.
//!
//! The provenance is fail-closed at construction: a `git_head` is either
//! the 40 hex characters `git rev-parse HEAD` printed from a clean worktree
//! or it does not exist — there is no `"unknown"`, no short SHA and no
//! dirty-tree measurement. The writer is [`BenchArtifactV1::write_to`];
//! nothing else in this crate writes a benchmark artifact.
//!
//! This module carries no serde derives (banned repo-wide); every wire
//! shape is a manual `impl Serialize`.

use std::fmt;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use serde::ser::{Serialize, SerializeStruct, Serializer};
use sha2::{Digest as _, Sha256};

/// The artifact schema every benchmark/relevance writer emits and the
/// release gate requires. Schema `1` artifacts carried a short `git_rev`
/// and no digests, host or resource fields; they are refused.
pub const BENCH_ARTIFACT_SCHEMA_VERSION: u32 = 2;

/// Route family a benchmarked scenario exercises.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RouteFamily {
    Lexical,
    Semantic,
    Hybrid,
    Symbol,
    RepoMap,
    History,
    RuntimeCatalog,
    Structural,
    Adversarial,
}

impl RouteFamily {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteFamily::Lexical => "lexical",
            RouteFamily::Semantic => "semantic",
            RouteFamily::Hybrid => "hybrid",
            RouteFamily::Symbol => "symbol",
            RouteFamily::RepoMap => "repomap",
            RouteFamily::History => "history",
            RouteFamily::RuntimeCatalog => "runtime_catalog",
            RouteFamily::Structural => "structural",
            RouteFamily::Adversarial => "adversarial",
        }
    }
}

/// Query syntax surface a scenario is phrased in.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BenchSyntax {
    Native,
    Sourcegraph,
}

impl BenchSyntax {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BenchSyntax::Native => "native",
            BenchSyntax::Sourcegraph => "sourcegraph",
        }
    }
}

/// Cache-state mode under which the benchmark was taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BenchMode {
    Warm,
    Cold,
}

impl BenchMode {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BenchMode::Warm => "warm",
            BenchMode::Cold => "cold",
        }
    }
}

/// Observed result shape produced by a scenario run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ResultShape {
    Candidates,
    Commits,
    DiffPaths,
    TypedError,
    Empty,
}

impl ResultShape {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ResultShape::Candidates => "candidates",
            ResultShape::Commits => "commits",
            ResultShape::DiffPaths => "diff_paths",
            ResultShape::TypedError => "typed_error",
            ResultShape::Empty => "empty",
        }
    }
}

/// Nearest-rank latency percentiles over a sample set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatencySummary {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub samples: u32,
}

impl LatencySummary {
    /// Compute nearest-rank p50/p95/p99 over the given millisecond samples.
    ///
    /// Returns `None` for an empty slice. NaN values are ordered defensively
    /// via `f64::total_cmp` so the sort is total.
    #[must_use]
    pub fn from_samples_ms(samples: &[f64]) -> Option<LatencySummary> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        Some(LatencySummary {
            p50_ms: nearest_rank(&sorted, 50),
            p95_ms: nearest_rank(&sorted, 95),
            p99_ms: nearest_rank(&sorted, 99),
            samples: saturating_u32(n),
        })
    }
}

impl Serialize for LatencySummary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("LatencySummary", 4)?;
        state.serialize_field("p50_ms", &self.p50_ms)?;
        state.serialize_field("p95_ms", &self.p95_ms)?;
        state.serialize_field("p99_ms", &self.p99_ms)?;
        state.serialize_field("samples", &self.samples)?;
        state.end()
    }
}

/// Saturating narrowing of a `usize` sample count into the `u32` artifact
/// field. Benchmark sample counts never approach `u32::MAX`, so saturation is
/// a defensive ceiling rather than an expected path.
fn saturating_u32(n: usize) -> u32 {
    if let Ok(value) = u32::try_from(n) {
        return value;
    }
    u32::MAX
}

/// Saturating widening of a `usize` row count into the `u64` artifact
/// field: exact on every supported target, a defensive ceiling elsewhere.
#[must_use]
pub fn saturating_u64(n: usize) -> u64 {
    if let Ok(value) = u64::try_from(n) {
        return value;
    }
    u64::MAX
}

/// Nearest-rank percentile lookup on an ascending-sorted, non-empty slice.
///
/// index = ceil(p/100 * n) - 1, clamped to `[0, n-1]`.
fn nearest_rank(sorted: &[f64], p: u32) -> f64 {
    let n = sorted.len();
    // n >= 1 guaranteed by callers.
    let scaled = f64::from(p) / 100.0 * n_as_f64(n);
    let rank = f64_ceil_to_usize(scaled);
    let idx = rank.saturating_sub(1).min(n.saturating_sub(1));
    sorted.get(idx).copied().unwrap_or(f64::NAN)
}

/// Lossy `usize -> f64` widening for percentile scaling. Sample counts in
/// benchmarks never approach the f64 mantissa boundary, so precision loss is
/// not observable here.
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "benchmark sample counts are small; f64 widening is exact in range"
)]
fn n_as_f64(n: usize) -> f64 {
    n as f64
}

/// Truncate a non-negative ceil'd percentile value to a usize rank.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "scaled rank is non-negative and bounded by the sample count"
)]
fn f64_ceil_to_usize(value: f64) -> usize {
    value.ceil() as usize
}

// ---------------------------------------------------------------------------
// Provenance: the exact head, the digests, the host.
// ---------------------------------------------------------------------------

/// Why a benchmark artifact cannot be attributed and therefore is not
/// written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BenchProvenanceError {
    /// `git` could not be run or did not answer.
    GitUnavailable { detail: String },
    /// The head is not 40 lowercase hex characters.
    NotAFullHead { text: String },
    /// The worktree has tracked or untracked changes: a measurement of it
    /// cannot be attributed to the head.
    DirtyWorktree { status: String },
    /// A host fact could not be observed.
    HostUnobservable { fact: &'static str, detail: String },
    /// The process's resource usage could not be read.
    ResourceUnobservable { detail: String },
}

impl BenchProvenanceError {
    /// The stable code a gate or a log names for this refusal.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::GitUnavailable { .. } => "BENCH_GIT_UNAVAILABLE",
            Self::NotAFullHead { .. } => "BENCH_GIT_HEAD_NOT_FULL",
            Self::DirtyWorktree { .. } => "BENCH_WORKTREE_DIRTY",
            Self::HostUnobservable { .. } => "BENCH_HOST_UNOBSERVABLE",
            Self::ResourceUnobservable { .. } => "BENCH_RESOURCE_UNOBSERVABLE",
        }
    }
}

impl fmt::Display for BenchProvenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitUnavailable { detail } => {
                write!(
                    formatter,
                    "{}: git rev-parse HEAD failed: {detail}",
                    self.code()
                )
            }
            Self::NotAFullHead { text } => write!(
                formatter,
                "{}: expected 40 lowercase hex characters, got {text:?}",
                self.code()
            ),
            Self::DirtyWorktree { status } => write!(
                formatter,
                "{}: the worktree has uncommitted changes, a measurement cannot be attributed to HEAD:\n{status}",
                self.code()
            ),
            Self::HostUnobservable { fact, detail } => {
                write!(formatter, "{}: {fact}: {detail}", self.code())
            }
            Self::ResourceUnobservable { detail } => {
                write!(formatter, "{}: {detail}", self.code())
            }
        }
    }
}

impl std::error::Error for BenchProvenanceError {}

/// The exact commit a measurement was taken at: 40 lowercase hex
/// characters, never a short SHA and never `"unknown"`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GitHeadV1(String);

impl GitHeadV1 {
    /// Accept a full head as `git rev-parse HEAD` prints it.
    pub fn parse(text: &str) -> Result<Self, BenchProvenanceError> {
        let trimmed = text.trim();
        if trimmed.len() != 40
            || !trimmed
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(BenchProvenanceError::NotAFullHead {
                text: trimmed.to_string(),
            });
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The head of the worktree at `worktree`, refusing a dirty tree.
    ///
    /// Runs `git rev-parse HEAD` and `git status --porcelain`; any tracked
    /// modification or untracked (non-ignored) file makes the tree dirty,
    /// since either could have changed what was measured.
    pub fn resolve(worktree: &Path) -> Result<Self, BenchProvenanceError> {
        let head = git_stdout(worktree, &["rev-parse", "HEAD"])?;
        let head = Self::parse(&head)?;
        let status = git_stdout(worktree, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            return Err(BenchProvenanceError::DirtyWorktree { status });
        }
        Ok(head)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn git_stdout(worktree: &Path, args: &[&str]) -> Result<String, BenchProvenanceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|err| BenchProvenanceError::GitUnavailable {
            detail: format!("git {}: {err}", args.join(" ")),
        })?;
    if !output.status.success() {
        return Err(BenchProvenanceError::GitUnavailable {
            detail: format!(
                "git {} exited {}: {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    String::from_utf8(output.stdout).map_err(|err| BenchProvenanceError::GitUnavailable {
        detail: format!("git {} printed non-UTF-8: {err}", args.join(" ")),
    })
}

/// A `sha256:`-prefixed digest over length-framed parts under a domain
/// tag, so two rails hashing the same bytes under different domains never
/// collide and a part boundary is never ambiguous.
#[must_use]
pub fn framed_digest(domain: &str, parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    for part in parts {
        hasher.update(part.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(7_usize.saturating_add(digest.len().saturating_mul(2)));
    encoded.push_str("sha256:");
    for byte in digest {
        // Infallible write into a String.
        let _written: Result<(), std::fmt::Error> = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// A corpus digest over `(path, content)` pairs in the order they were
/// ingested.
#[must_use]
pub fn corpus_digest(dimension: &str, files: &[(String, String)]) -> String {
    corpus_digest_refs(
        dimension,
        files
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_str())),
    )
}

/// The same framed corpus digest over borrowed file bytes. Large scale rails
/// use this to avoid cloning every source file solely for provenance.
#[must_use]
pub fn corpus_digest_refs<'a>(
    dimension: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let domain = format!("quanta-index:bench:{dimension}:corpus:v1");
    let parts: Vec<&[u8]> = files
        .into_iter()
        .flat_map(|(path, content)| [path.as_bytes(), content.as_bytes()])
        .collect();
    framed_digest(&domain, &parts)
}

/// A config digest over `key=value` pairs in a canonical order.
#[must_use]
pub fn config_digest(dimension: &str, entries: &[(&str, String)]) -> String {
    let domain = format!("quanta-index:bench:{dimension}:config:v1");
    let mut sorted: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    sorted.sort();
    let parts: Vec<&[u8]> = sorted.iter().map(String::as_bytes).collect();
    framed_digest(&domain, &parts)
}

/// What a measurement was taken of: the exact head, the corpus, the
/// configuration and the embedding model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchProvenanceV1 {
    pub git_head: GitHeadV1,
    /// [`corpus_digest`] over the bytes the rail ingested.
    pub corpus_digest: String,
    /// [`config_digest`] over the rail's parameters.
    pub config_digest: String,
    /// `<model_id>@<revision>` of the embedder the fixture's dense lane ran
    /// with; `None` when the rail exercised no embedding model.
    pub model_revision: Option<String>,
}

impl Serialize for BenchProvenanceV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("BenchProvenanceV1", 4)?;
        state.serialize_field("git_head", self.git_head.as_str())?;
        state.serialize_field("corpus_digest", &self.corpus_digest)?;
        state.serialize_field("config_digest", &self.config_digest)?;
        state.serialize_field("model_revision", &self.model_revision)?;
        state.end()
    }
}

/// The embedder identity the harness runtime ran with, as the provenance
/// names it.
#[must_use]
pub fn model_revision_of(
    profile: &quanta_index_searchd::app::SemanticEmbedderProfile,
) -> Option<String> {
    use quanta_index_searchd::app::SemanticEmbedderProfile;
    match profile {
        SemanticEmbedderProfile::Hash { dimension } => Some(format!(
            "{}@{}:d{dimension}",
            quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID,
            quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_REVISION
        )),
        SemanticEmbedderProfile::OpenAi {
            model,
            model_revision,
            dimension,
            ..
        } => Some(format!("{model}@{model_revision}:d{dimension}")),
        SemanticEmbedderProfile::PotionCode { encoding, .. } => Some(format!(
            "{}@{}:d{}",
            quanta_index_embed::POTION_CODE_MODEL_ID,
            encoding.model_revision(),
            quanta_index_embed::POTION_CODE_DIMENSION
        )),
        SemanticEmbedderProfile::Unavailable => None,
    }
}

/// The host a measurement was taken on. The hostname is stored hashed,
/// which is enough to tell two hosts apart without recording either.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostV1 {
    pub os: String,
    pub arch: String,
    pub cpu_count: u32,
    pub mem_bytes: u64,
    pub hostname_hash: String,
}

impl HostV1 {
    /// Observe the running host.
    pub fn observe() -> Result<Self, BenchProvenanceError> {
        let cpu_count = std::thread::available_parallelism()
            .map_err(|err| BenchProvenanceError::HostUnobservable {
                fact: "cpu_count",
                detail: err.to_string(),
            })?
            .get();
        let hostname =
            nix::unistd::gethostname().map_err(|err| BenchProvenanceError::HostUnobservable {
                fact: "hostname",
                detail: err.to_string(),
            })?;
        Ok(Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_count: saturating_u32(cpu_count),
            mem_bytes: total_memory_bytes()?,
            hostname_hash: framed_digest(
                "quanta-index:bench:hostname:v1",
                &[hostname.as_encoded_bytes()],
            ),
        })
    }
}

impl Serialize for HostV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HostV1", 5)?;
        state.serialize_field("os", &self.os)?;
        state.serialize_field("arch", &self.arch)?;
        state.serialize_field("cpu_count", &self.cpu_count)?;
        state.serialize_field("mem_bytes", &self.mem_bytes)?;
        state.serialize_field("hostname_hash", &self.hostname_hash)?;
        state.end()
    }
}

#[cfg(target_os = "linux")]
fn total_memory_bytes() -> Result<u64, BenchProvenanceError> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").map_err(|err| {
        BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: format!("/proc/meminfo: {err}"),
        }
    })?;
    let kib_text = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or_else(|| BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: "/proc/meminfo has no MemTotal line".to_string(),
        })?;
    let kib = kib_text
        .parse::<u64>()
        .map_err(|err| BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: format!("/proc/meminfo MemTotal {kib_text:?} is not an integer: {err}"),
        })?;
    kib.checked_mul(1024)
        .ok_or_else(|| BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: format!("MemTotal {kib} KiB overflows bytes"),
        })
}

#[cfg(target_os = "macos")]
fn total_memory_bytes() -> Result<u64, BenchProvenanceError> {
    let output = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .map_err(|err| BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: format!("sysctl -n hw.memsize: {err}"),
        })?;
    if !output.status.success() {
        return Err(BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: format!("sysctl -n hw.memsize exited {}", output.status),
        });
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|err| BenchProvenanceError::HostUnobservable {
            fact: "mem_bytes",
            detail: format!("sysctl -n hw.memsize printed a non-integer: {err}"),
        })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn total_memory_bytes() -> Result<u64, BenchProvenanceError> {
    Err(BenchProvenanceError::HostUnobservable {
        fact: "mem_bytes",
        detail: format!("no total-memory source on {}", std::env::consts::OS),
    })
}

/// The process's peak resident set, from `getrusage(RUSAGE_SELF)`.
///
/// The harness drives the daemon in-process, so this is the daemon's peak
/// plus the harness's own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceUsageV1 {
    pub peak_rss_bytes: u64,
}

impl ResourceUsageV1 {
    /// Observe the calling process.
    pub fn observe_self() -> Result<Self, BenchProvenanceError> {
        let usage = nix::sys::resource::getrusage(nix::sys::resource::UsageWho::RUSAGE_SELF)
            .map_err(|err| BenchProvenanceError::ResourceUnobservable {
                detail: format!("getrusage(RUSAGE_SELF): {err}"),
            })?;
        let max_rss = u64::try_from(usage.max_rss()).map_err(|err| {
            BenchProvenanceError::ResourceUnobservable {
                detail: format!("ru_maxrss is negative: {err}"),
            }
        })?;
        let peak_rss_bytes = if cfg!(target_os = "macos") {
            max_rss
        } else {
            max_rss
                .checked_mul(1024)
                .ok_or_else(|| BenchProvenanceError::ResourceUnobservable {
                    detail: format!("ru_maxrss {max_rss} KiB overflows bytes"),
                })?
        };
        Ok(Self { peak_rss_bytes })
    }
}

impl Serialize for ResourceUsageV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("ResourceUsageV1", 1)?;
        state.serialize_field("peak_rss_bytes", &self.peak_rss_bytes)?;
        state.end()
    }
}

/// The durations of the phases a rail ran; `None` is a phase the rail has
/// no such step for (a query-only rail builds nothing), never a phase it
/// failed to time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "the unit is part of each artifact field's name: every phase duration is in milliseconds"
)]
pub struct PhaseDurationsV1 {
    /// Full build: ingest through seal.
    pub build_ms: Option<f64>,
    /// One-file delta: ingest of the change through seal.
    pub update_ms: Option<f64>,
    /// Sealed-generation reclaim.
    pub gc_ms: Option<f64>,
}

impl Serialize for PhaseDurationsV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("PhaseDurationsV1", 3)?;
        state.serialize_field("build_ms", &self.build_ms)?;
        state.serialize_field("update_ms", &self.update_ms)?;
        state.serialize_field("gc_ms", &self.gc_ms)?;
        state.end()
    }
}

/// Bytes the rail wrote to the state root against the bytes that changed:
/// `1.0` is an index that costs exactly its input; a delta that rewrites
/// the corpus reads far above it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskAmplificationV1 {
    pub bytes_written: u64,
    pub changed_bytes: u64,
}

impl DiskAmplificationV1 {
    /// `bytes_written / changed_bytes`; `None` when nothing changed, so a
    /// division by zero is never reported as a number.
    #[must_use]
    pub fn ratio(&self) -> Option<f64> {
        if self.changed_bytes == 0 {
            return None;
        }
        Some(u64_as_f64(self.bytes_written) / u64_as_f64(self.changed_bytes))
    }
}

/// Byte counts far below `2^53` widen exactly.
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "byte counts in these rails are far below 2^53, so the widening is exact"
)]
fn u64_as_f64(value: u64) -> f64 {
    value as f64
}

impl Serialize for DiskAmplificationV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("DiskAmplificationV1", 3)?;
        state.serialize_field("bytes_written", &self.bytes_written)?;
        state.serialize_field("changed_bytes", &self.changed_bytes)?;
        state.serialize_field("ratio", &self.ratio())?;
        state.end()
    }
}

/// Sum of the sizes of every regular file under `root`, walked
/// recursively: the byte oracle behind disk amplification.
pub fn directory_bytes(root: &Path) -> std::io::Result<u64> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Rows and the envelope.
// ---------------------------------------------------------------------------

/// One measured (or early-stopped) benchmark scenario row.
#[derive(Clone, Debug)]
pub struct BenchRowV1 {
    pub scenario_id: String,
    pub route_family: RouteFamily,
    pub syntax: BenchSyntax,
    pub result_shape: ResultShape,
    /// `None` when `early_stop_reason` is set (latency was not measured).
    pub latency: Option<LatencySummary>,
    /// Served queries per second over the measured window; `None` for a
    /// rail that measures single queries rather than throughput.
    pub qps: Option<f64>,
    /// Queries answered with a typed error during the measured window.
    pub error_count: u64,
    /// Queries that did not answer within the client's deadline.
    pub timeout_count: u64,
    pub result_count: Option<u64>,
    pub typed_error_code: Option<String>,
    pub engine_touched: Vec<String>,
    pub early_stop_reason: Option<String>,
}

impl Serialize for BenchRowV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("BenchRowV1", 12)?;
        state.serialize_field("scenario_id", &self.scenario_id)?;
        state.serialize_field("route_family", self.route_family.as_str())?;
        state.serialize_field("syntax", self.syntax.as_str())?;
        state.serialize_field("result_shape", self.result_shape.as_str())?;
        state.serialize_field("latency", &self.latency)?;
        state.serialize_field("qps", &self.qps)?;
        state.serialize_field("error_count", &self.error_count)?;
        state.serialize_field("timeout_count", &self.timeout_count)?;
        state.serialize_field("result_count", &self.result_count)?;
        state.serialize_field("typed_error_code", &self.typed_error_code)?;
        state.serialize_field("engine_touched", &self.engine_touched)?;
        state.serialize_field("early_stop_reason", &self.early_stop_reason)?;
        state.end()
    }
}

/// The one benchmark/relevance artifact envelope.
#[derive(Clone, Debug)]
pub struct BenchArtifactV1 {
    /// Which rail: `dsl-warm`, `dsl-cold`, `scale`, `tail`, `relevance`,
    /// `relevance-openai-ab`, `scan-vs-index`, `concurrency`.
    pub dimension: String,
    pub mode: BenchMode,
    /// Concurrent clients issuing queries during the measured window.
    pub concurrency: u32,
    pub provenance: BenchProvenanceV1,
    pub host: HostV1,
    pub resources: ResourceUsageV1,
    pub phases: PhaseDurationsV1,
    /// Present for a rail that wrote an index.
    pub disk_amplification: Option<DiskAmplificationV1>,
    pub rows: Vec<BenchRowV1>,
    /// Dimension-specific shape; a JSON object.
    pub detail: serde_json::Value,
}

impl Serialize for BenchArtifactV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("BenchArtifactV1", 11)?;
        state.serialize_field("schema_version", &BENCH_ARTIFACT_SCHEMA_VERSION)?;
        state.serialize_field("dimension", &self.dimension)?;
        state.serialize_field("mode", self.mode.as_str())?;
        state.serialize_field("concurrency", &self.concurrency)?;
        state.serialize_field("provenance", &self.provenance)?;
        state.serialize_field("host", &self.host)?;
        state.serialize_field("resources", &self.resources)?;
        state.serialize_field("phases", &self.phases)?;
        state.serialize_field("disk_amplification", &self.disk_amplification)?;
        state.serialize_field("rows", &self.rows)?;
        state.serialize_field("detail", &self.detail)?;
        state.end()
    }
}

impl BenchArtifactV1 {
    /// The artifact as a JSON value.
    pub fn to_json(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::to_value(self)
    }

    /// The one writer: pretty-printed JSON with a trailing newline at
    /// `path`, creating parent directories if needed.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        let mut file = std::fs::File::create(path)?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }
}

/// Serialize `value` as pretty-printed JSON with a trailing newline to `path`.
///
/// Creates the parent directory if present. Shared writer for the quality
/// rails whose artifacts are verdict records rather than measurements
/// (ambiguity/ops/ui/snippet); benchmark and relevance artifacts go through
/// [`BenchArtifactV1::write_to`].
pub(crate) fn write_json_pretty(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests read the serialized envelope by JSON path; a missing path fails the test"
)]
mod tests {
    use super::*;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn head() -> GitHeadV1 {
        GitHeadV1::parse(HEAD).expect("a full head")
    }

    fn host() -> HostV1 {
        HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 8,
            mem_bytes: 1 << 34,
            hostname_hash: framed_digest("quanta-index:bench:hostname:v1", &[b"host"]),
        }
    }

    fn artifact() -> BenchArtifactV1 {
        BenchArtifactV1 {
            dimension: "dsl-warm".to_string(),
            mode: BenchMode::Warm,
            concurrency: 1,
            provenance: BenchProvenanceV1 {
                git_head: head(),
                corpus_digest: corpus_digest("dsl-warm", &[("a".to_string(), "b".to_string())]),
                config_digest: config_digest("dsl-warm", &[("samples", "5".to_string())]),
                model_revision: None,
            },
            host: host(),
            resources: ResourceUsageV1 { peak_rss_bytes: 42 },
            phases: PhaseDurationsV1::default(),
            disk_amplification: None,
            rows: vec![BenchRowV1 {
                scenario_id: "lexical.keyword.native".to_string(),
                route_family: RouteFamily::Lexical,
                syntax: BenchSyntax::Native,
                result_shape: ResultShape::Candidates,
                latency: LatencySummary::from_samples_ms(&[1.0, 2.0, 3.0, 4.0]),
                qps: None,
                error_count: 0,
                timeout_count: 0,
                result_count: Some(12),
                typed_error_code: None,
                engine_touched: vec!["lexical".to_string()],
                early_stop_reason: None,
            }],
            detail: serde_json::json!({}),
        }
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "percentile boundaries land on exact integer-valued samples"
    )]
    fn nearest_rank_on_one_to_hundred() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let summary = LatencySummary::from_samples_ms(&samples).expect("non-empty");
        // nearest-rank: idx = ceil(p/100 * 100) - 1 = p - 1 -> value p.
        assert_eq!(summary.p50_ms, 50.0);
        assert_eq!(summary.p95_ms, 95.0);
        assert_eq!(summary.p99_ms, 99.0);
        assert_eq!(summary.samples, 100);
    }

    #[test]
    fn empty_samples_yield_none() {
        assert!(LatencySummary::from_samples_ms(&[]).is_none());
    }

    #[test]
    fn route_family_labels_preserve_retrieval_ownership() {
        assert_eq!(RouteFamily::Semantic.as_str(), "semantic");
        assert_eq!(RouteFamily::Hybrid.as_str(), "hybrid");
        assert_eq!(RouteFamily::Symbol.as_str(), "symbol");
        assert_eq!(RouteFamily::RepoMap.as_str(), "repomap");
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "single-sample percentiles collapse to the exact input value"
    )]
    fn single_element_collapses_all_percentiles() {
        let summary = LatencySummary::from_samples_ms(&[7.5]).expect("non-empty");
        assert_eq!(summary.p50_ms, 7.5);
        assert_eq!(summary.p95_ms, 7.5);
        assert_eq!(summary.p99_ms, 7.5);
        assert_eq!(summary.samples, 1);
    }

    #[test]
    fn nan_is_ordered_defensively() {
        // Should not panic; NaN sorts to the high end via total_cmp.
        let summary = LatencySummary::from_samples_ms(&[1.0, f64::NAN, 2.0]).expect("non-empty");
        assert_eq!(summary.samples, 3);
    }

    // A head is the 40 hex characters or it is not a head: short SHAs,
    // "unknown", uppercase and padding are all refused with the typed code.
    #[test]
    fn a_git_head_is_forty_lowercase_hex_or_refused() {
        assert_eq!(head().as_str(), HEAD);
        assert_eq!(
            GitHeadV1::parse(&format!("  {HEAD}\n")).map(|head| head.as_str().to_string()),
            Ok(HEAD.to_string())
        );
        for refused in [
            "unknown",
            "",
            "3148c02",
            "0123456789ABCDEF0123456789ABCDEF01234567",
            "0123456789abcdef0123456789abcdef0123456",
            "0123456789abcdef0123456789abcdef012345678",
            "0123456789abcdef0123456789abcdef0123456g",
        ] {
            let outcome = GitHeadV1::parse(refused);
            assert!(
                matches!(outcome, Err(BenchProvenanceError::NotAFullHead { .. })),
                "{refused:?}: {outcome:?}"
            );
            assert_eq!(
                outcome.map_err(|err| err.code()),
                Err("BENCH_GIT_HEAD_NOT_FULL")
            );
        }
    }

    // The resolver refuses a dirty tree and a non-repository, and never
    // answers a placeholder.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the test asserts on the artifact with assert macros and propagates I/O with `?`"
    )]
    fn resolving_a_head_refuses_a_dirty_tree_and_a_non_repository()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let not_a_repo = GitHeadV1::resolve(temp.path());
        assert!(
            matches!(not_a_repo, Err(BenchProvenanceError::GitUnavailable { .. })),
            "{not_a_repo:?}"
        );
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo)?;
        let git = |args: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "bench")
                .env("GIT_AUTHOR_EMAIL", "bench@example.invalid")
                .env("GIT_COMMITTER_NAME", "bench")
                .env("GIT_COMMITTER_EMAIL", "bench@example.invalid")
                .status()?;
            if !status.success() {
                return Err(format!("git {} failed", args.join(" ")).into());
            }
            Ok(())
        };
        git(&["init", "-q"])?;
        std::fs::write(repo.join("tracked.txt"), "one\n")?;
        git(&["add", "tracked.txt"])?;
        git(&["commit", "-q", "-m", "one"])?;
        let clean = GitHeadV1::resolve(&repo)?;
        assert_eq!(clean.as_str().len(), 40);
        // A tracked modification is dirty.
        std::fs::write(repo.join("tracked.txt"), "two\n")?;
        let dirty = GitHeadV1::resolve(&repo);
        assert!(
            matches!(dirty, Err(BenchProvenanceError::DirtyWorktree { .. })),
            "{dirty:?}"
        );
        git(&["checkout", "--", "tracked.txt"])?;
        // An untracked file is dirty too: it could be the source measured.
        std::fs::write(repo.join("untracked.rs"), "fn main() {}\n")?;
        let untracked = GitHeadV1::resolve(&repo);
        assert!(
            matches!(untracked, Err(BenchProvenanceError::DirtyWorktree { .. })),
            "{untracked:?}"
        );
        std::fs::remove_file(repo.join("untracked.rs"))?;
        assert_eq!(GitHeadV1::resolve(&repo)?, clean);
        Ok(())
    }

    #[test]
    fn digests_are_framed_and_domain_separated() {
        let split_late = corpus_digest("scale", &[("a".to_string(), "bc".to_string())]);
        let split_early = corpus_digest("scale", &[("ab".to_string(), "c".to_string())]);
        let other_dimension = corpus_digest("tail", &[("a".to_string(), "bc".to_string())]);
        assert_eq!(
            split_late,
            corpus_digest_refs("scale", [("a", "bc")]),
            "borrowed source bytes retain the exact existing digest"
        );
        assert_eq!(
            split_late, "sha256:165f3d3e494e609f64259037eabd576ce3c6bae091c1d751311491bc2f4a7a04",
            "independent SHA-256 of domain-NUL, framed path, and framed content"
        );
        assert_ne!(split_late, split_early, "part boundaries are framed");
        assert_ne!(
            split_late, other_dimension,
            "dimensions are domain-separated"
        );
        assert!(split_late.starts_with("sha256:") && split_late.len() == 7 + 64);
        let x = config_digest(
            "scale",
            &[("seed", "1".to_string()), ("tier", "small".to_string())],
        );
        let y = config_digest(
            "scale",
            &[("tier", "small".to_string()), ("seed", "1".to_string())],
        );
        assert_eq!(x, y, "config entries are canonically ordered");
        assert_ne!(
            x,
            config_digest(
                "scale",
                &[("seed", "2".to_string()), ("tier", "small".to_string())]
            )
        );
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the test asserts on the artifact with assert macros and propagates I/O with `?`"
    )]
    fn the_host_and_the_process_are_observable_here() -> Result<(), BenchProvenanceError> {
        let host = HostV1::observe()?;
        assert!(host.cpu_count >= 1);
        assert!(host.mem_bytes > 0);
        assert!(host.hostname_hash.starts_with("sha256:"));
        assert!(!host.os.is_empty() && !host.arch.is_empty());
        let usage = ResourceUsageV1::observe_self()?;
        assert!(usage.peak_rss_bytes > 0);
        Ok(())
    }

    #[test]
    fn disk_amplification_never_divides_by_zero() {
        let none = DiskAmplificationV1 {
            bytes_written: 10,
            changed_bytes: 0,
        };
        assert_eq!(none.ratio(), None);
        let two = DiskAmplificationV1 {
            bytes_written: 20,
            changed_bytes: 10,
        };
        assert_eq!(two.ratio(), Some(2.0));
    }

    #[test]
    fn model_revision_names_selected_embedder_policy() {
        use quanta_index_embed::PotionCodeEncodingPolicy;
        use quanta_index_searchd::app::SemanticEmbedderProfile;
        let hash = model_revision_of(&SemanticEmbedderProfile::Hash { dimension: 16 });
        assert_eq!(
            hash.as_deref(),
            Some("search-owned-hash-text-v1@fnv1a64-slots-l2unit-v1:d16")
        );
        assert_eq!(
            model_revision_of(&SemanticEmbedderProfile::Unavailable),
            None
        );
        for policy in [
            PotionCodeEncodingPolicy::Pinned512V1,
            PotionCodeEncodingPolicy::FullLengthV2,
        ] {
            let profile = SemanticEmbedderProfile::PotionCode {
                model_dir: "/unused/model".into(),
                encoding: policy,
            };
            assert_eq!(
                model_revision_of(&profile).as_deref(),
                Some(
                    format!(
                        "{}@{}:d{}",
                        quanta_index_embed::POTION_CODE_MODEL_ID,
                        policy.model_revision(),
                        quanta_index_embed::POTION_CODE_DIMENSION,
                    )
                    .as_str()
                )
            );
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the test asserts on the artifact with assert macros and propagates I/O with `?`"
    )]
    fn the_envelope_carries_every_gate_required_field() -> Result<(), Box<dyn std::error::Error>> {
        let value = artifact().to_json()?;
        let object = value.as_object().ok_or("an object")?;
        for key in [
            "schema_version",
            "dimension",
            "mode",
            "concurrency",
            "provenance",
            "host",
            "resources",
            "phases",
            "disk_amplification",
            "rows",
            "detail",
        ] {
            assert!(object.contains_key(key), "missing {key}");
        }
        assert_eq!(
            value["schema_version"],
            serde_json::json!(BENCH_ARTIFACT_SCHEMA_VERSION)
        );
        assert_eq!(value["provenance"]["git_head"], serde_json::json!(HEAD));
        assert!(
            value["provenance"]["corpus_digest"]
                .as_str()
                .is_some_and(|d| d.starts_with("sha256:"))
        );
        assert!(
            value["provenance"]["config_digest"]
                .as_str()
                .is_some_and(|d| d.starts_with("sha256:"))
        );
        assert!(value["provenance"]["model_revision"].is_null());
        for key in ["os", "arch", "cpu_count", "mem_bytes", "hostname_hash"] {
            assert!(value["host"].get(key).is_some(), "host.{key}");
        }
        assert_eq!(value["resources"]["peak_rss_bytes"], serde_json::json!(42));
        for key in ["build_ms", "update_ms", "gc_ms"] {
            assert!(value["phases"].get(key).is_some(), "phases.{key}");
        }
        let row = &value["rows"][0];
        for key in [
            "scenario_id",
            "route_family",
            "syntax",
            "result_shape",
            "latency",
            "qps",
            "error_count",
            "timeout_count",
            "result_count",
            "typed_error_code",
            "engine_touched",
            "early_stop_reason",
        ] {
            assert!(row.get(key).is_some(), "rows[0].{key}");
        }
        for key in ["p50_ms", "p95_ms", "p99_ms", "samples"] {
            assert!(row["latency"].get(key).is_some(), "rows[0].latency.{key}");
        }
        assert!(
            !value.to_string().contains("git_rev"),
            "the short-SHA field is gone"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the test asserts on the artifact with assert macros and propagates I/O with `?`"
    )]
    fn the_writer_writes_the_envelope_once_with_a_trailing_newline()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("nested").join("artifact.json");
        artifact().write_to(&path)?;
        let text = std::fs::read_to_string(&path)?;
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(&text)?;
        assert_eq!(value, artifact().to_json()?);
        Ok(())
    }
}
