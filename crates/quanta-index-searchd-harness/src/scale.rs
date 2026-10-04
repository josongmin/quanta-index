//! Scale-tier rail (J7Q-03): seeded synthetic corpus generator + tier manifest.
//!
//! This module owns three things, all checked-in and deterministic:
//!
//! - a **tier manifest** — `small`/`medium`/`large`/`xlarge`, each with declared
//!   shape params (`repo_count`, `files_per_repo`, `avg_file_lines`,
//!   `hit_density`, `symbol_density`). The params are the SSOT for how big each
//!   tier is, reviewable by eye against the emitted `tier_manifest.json`;
//! - a **deterministic seeded generator** — `generate_corpus(tier, seed)` returns
//!   a reproducible `Vec<(path, content)>`. Content is derived purely from the
//!   `(tier, file_index, seed)` triple through an FNV-1a hash and a small LCG
//!   word stream. There is no `rand` crate, no clock, no OS entropy: same
//!   `(tier, seed)` always yields byte-identical output, and a different seed
//!   yields different output. That reproducibility is what lets a perf number be
//!   attributed to a known corpus instead of a one-off random draw;
//! - a **measured run of a selected tier** — `measure_tier` boots one
//!   [`E2eRuntime`], ingests the seeded corpus, seals, activates, queries
//!   cold and warm across the socket, opens the same sealed generation through
//!   the lexical adapter in-process, then applies a one-file delta — capturing
//!   measured phases into a [`TierMeasurement`] (QI-BB-010 #2): build,
//!   activation, route timing from daemon metrics, optional query cold-open,
//!   adapter open / plan / execute, delta update and activation. A one-file
//!   delta under minimum two-generation retention need not reclaim bytes.
//!   Medium/large/xlarge use distinct source-repository IDs and per-repo
//!   probes inside one serving owner. Each selected run measures one tier;
//!   canonical performance qualification still requires a quiet host.
//!
//! Fail-closed posture: a query that the runtime rejects on any selected tier is a
//! rail error (typed error -> `Err`), never a zero-latency "pass". Required
//! route samples must be present; a legitimately absent query cold-open is null.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
#[cfg(target_os = "macos")]
use std::io::Read as _;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use nix::sys::resource::{UsageWho, getrusage};
use nix::sys::time::TimeValLike as _;
use quanta_index_contract::{
    LexicalCandidate, ManifestGeneration, MetricsSnapshotV1, QueryConstraintSetV1,
    TextQueryRequest, TextQuerySyntax,
};
#[cfg(target_os = "linux")]
use quanta_index_core::ProcessMemoryProbePort as _;
use quanta_index_core::{LexicalIndexOpenPort as _, LexicalPageSpec, RequestBudgetV1};
use quanta_index_ipc::DEFAULT_CLIENT_IO_TIMEOUT;
use quanta_index_lexical::LexicalAdapter;
use quanta_index_search_plane::lower_lexical_text_query;
#[cfg(target_os = "linux")]
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, DiskAmplificationV1,
    GitHeadV1, HostV1, LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily,
    config_digest, corpus_digest, corpus_digest_refs, directory_bytes, model_revision_of,
    saturating_u64,
};
use crate::harness::{
    E2eRuntime, E2eTextChunkSpec, HARNESS_HISTORY_MAX_BYTES, HARNESS_HISTORY_MAX_REVISION_PAIRS,
    HARNESS_HISTORY_MAX_TOTAL_BYTES,
};

/// The artifact dimension this rail writes.
pub const DIMENSION: &str = "scale";

/// Explicit runtime policy for a scale measurement. Requested values remain
/// separate from the effective defaults in both success and refusal records.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScaleRuntimeConfig {
    pub client_timeout: Option<Duration>,
    pub history_max_bytes: Option<u64>,
}

impl ScaleRuntimeConfig {
    pub fn effective_timeout_ms(self) -> AnyResult<u64> {
        timeout_ms(self.client_timeout)
    }

    pub fn effective_history_max_bytes(self) -> AnyResult<u64> {
        let bytes = self.history_max_bytes.unwrap_or(HARNESS_HISTORY_MAX_BYTES);
        if !(1..=HARNESS_HISTORY_MAX_TOTAL_BYTES).contains(&bytes) {
            anyhow::bail!(
                "scale: history max bytes must be in 1..={HARNESS_HISTORY_MAX_TOTAL_BYTES} (harness total retention cap)"
            );
        }
        Ok(bytes)
    }

    pub fn execution_json(self) -> AnyResult<Value> {
        Ok(json!({
            "client_request_timeout_ms": self.effective_timeout_ms()?,
            "requested_client_request_timeout_ms": self.client_timeout.map(|_| self.effective_timeout_ms()).transpose()?,
            "history_max_generations": 2,
            "history_max_bytes": self.effective_history_max_bytes()?,
            "requested_history_max_bytes": self.history_max_bytes,
            "history_max_revision_pairs": HARNESS_HISTORY_MAX_REVISION_PAIRS,
            "history_max_total_bytes": HARNESS_HISTORY_MAX_TOTAL_BYTES,
        }))
    }
}

/// The deterministic query the small-tier measurement issues.
///
/// Every generated file embeds this token at the configured hit density, so a
/// correct lexical engine returns a non-empty ordering. An empty result is a
/// rail failure, not a benign zero.
const SCALE_QUERY_TOKEN: &str = "scale_needle_token";

/// Result cap for the measured small-tier query.
const SCALE_TOP_K: u32 = 10;

/// A scale tier's declared shape.
///
/// These are the knobs a reviewer reads to understand "how big is `large`": they
/// are inputs to the generator, not measured outputs. `hit_density` and
/// `symbol_density` are per-thousand rates (parts per 1000) so they stay integer
/// and exactly reproducible — no float seeding into the deterministic stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TierParams {
    pub tier: ScaleTier,
    pub repo_count: u32,
    pub files_per_repo: u32,
    pub avg_file_lines: u32,
    /// Hit occurrences per 1000 lines (the query token's planted frequency).
    pub hit_density_per_mille: u32,
    /// Symbol-bearing lines per 1000 lines.
    pub symbol_density_per_mille: u32,
}

impl TierParams {
    /// Total file count = `repo_count * files_per_repo`, saturating.
    #[must_use]
    pub fn total_files(&self) -> u32 {
        self.repo_count.saturating_mul(self.files_per_repo)
    }
}

/// The four declared scale tiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ScaleTier {
    Small,
    Medium,
    Large,
    Xlarge,
}

impl ScaleTier {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::Xlarge => "xlarge",
        }
    }

    /// The default invocation selects small; every tier is separately
    /// selectable and must pass its own source and wire admission checks.
    #[must_use]
    pub fn is_default_tier(self) -> bool {
        matches!(self, Self::Small)
    }
}

// The declared shape for each tier. Calibrated so `small` is genuinely cheap to
// run on a developer machine while the larger tiers grow file count and per-file
// size roughly an order of magnitude at a step, keeping the progression
// auditable. Each tier is a named const so [`params_for`] can return it through a
// total match (no fallible lookup), and [`TIER_MANIFEST`] stays the single
// ordered source the manifest artifact serializes.
const TIER_SMALL: TierParams = TierParams {
    tier: ScaleTier::Small,
    repo_count: 1,
    files_per_repo: 16,
    avg_file_lines: 40,
    hit_density_per_mille: 60,
    symbol_density_per_mille: 120,
};
const TIER_MEDIUM: TierParams = TierParams {
    tier: ScaleTier::Medium,
    repo_count: 4,
    files_per_repo: 64,
    avg_file_lines: 80,
    hit_density_per_mille: 40,
    symbol_density_per_mille: 100,
};
const TIER_LARGE: TierParams = TierParams {
    tier: ScaleTier::Large,
    repo_count: 16,
    files_per_repo: 256,
    avg_file_lines: 120,
    hit_density_per_mille: 25,
    symbol_density_per_mille: 80,
};
const TIER_XLARGE: TierParams = TierParams {
    tier: ScaleTier::Xlarge,
    repo_count: 64,
    files_per_repo: 512,
    avg_file_lines: 200,
    hit_density_per_mille: 15,
    symbol_density_per_mille: 60,
};

/// The checked-in tier manifest: the four declared tiers in growth order.
pub const TIER_MANIFEST: &[TierParams] = &[TIER_SMALL, TIER_MEDIUM, TIER_LARGE, TIER_XLARGE];

/// Look up the declared params for a tier.
///
/// Total by construction: the match is exhaustive over [`ScaleTier`], so every
/// tier resolves to its declared row without a fallible lookup or a default.
#[must_use]
pub fn params_for(tier: ScaleTier) -> TierParams {
    match tier {
        ScaleTier::Small => TIER_SMALL,
        ScaleTier::Medium => TIER_MEDIUM,
        ScaleTier::Large => TIER_LARGE,
        ScaleTier::Xlarge => TIER_XLARGE,
    }
}

// ---------------------------------------------------------------------------
// Deterministic content derivation (no rand, no clock).
// ---------------------------------------------------------------------------

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a hash of a `u64` mixed into a running state.
///
/// Deterministic and dependency-free; the dominant primitive behind both the
/// per-file seed and the word stream.
fn fnv1a_mix(mut state: u64, value: u64) -> u64 {
    for shift in [0u32, 8, 16, 24, 32, 40, 48, 56] {
        let byte = (value >> shift) & 0xff;
        state ^= byte;
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

/// Derive a stable per-file seed from `(tier, repo_index, file_index, seed)`.
fn file_seed(tier: ScaleTier, repo_index: u32, file_index: u32, seed: u64) -> u64 {
    let mut state = FNV_OFFSET;
    state = fnv1a_mix(state, tier_tag(tier));
    state = fnv1a_mix(state, u64::from(repo_index));
    state = fnv1a_mix(state, u64::from(file_index));
    fnv1a_mix(state, seed)
}

/// Distinct numeric tag per tier so tiers never collide in the seed space.
fn tier_tag(tier: ScaleTier) -> u64 {
    match tier {
        ScaleTier::Small => 1,
        ScaleTier::Medium => 2,
        ScaleTier::Large => 3,
        ScaleTier::Xlarge => 4,
    }
}

/// A tiny LCG word picker. Deterministic, period-irrelevant here (we only need
/// reproducible, well-spread token selection, not cryptographic quality).
fn lcg_next(state: u64) -> u64 {
    // Numerical Recipes constants.
    state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

/// A small fixed vocabulary the generator draws from for filler tokens.
///
/// Its length is exactly 16 (asserted below) so [`vocab_index`] can select an
/// entry with the low four bits of the stream state — equivalent to `% 16` but
/// with no overflow path and no fallible length conversion.
const VOCAB: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "omicron", "pi",
];

const _: () = assert!(
    VOCAB.len() == 16,
    "vocab_index selects with the low 4 bits and assumes a 16-entry VOCAB"
);

/// Pick a filler word for a given stream state.
///
/// [`vocab_index`] always yields an in-range index, so the `get` is total; the
/// `unwrap_or` default is unreachable and present only to keep the read
/// panic-free.
fn pick_word(state: u64) -> &'static str {
    VOCAB.get(vocab_index(state)).copied().unwrap_or("alpha")
}

/// Fold the 64-bit stream state into a `VOCAB` index in `0..16`.
///
/// The low four bits are always a valid index into the 16-entry table, so the
/// `u64 -> usize` narrowing is exact on every supported target.
#[expect(
    clippy::as_conversions,
    reason = "state & 0xF is in 0..=15, so the u64->usize narrowing is exact on every supported target"
)]
fn vocab_index(state: u64) -> usize {
    (state & 0xF) as usize
}

// ---------------------------------------------------------------------------
// Corpus generation.
// ---------------------------------------------------------------------------

/// Generate the seeded synthetic corpus for `(tier, seed)`.
///
/// Output is a deterministic `Vec<(path, content)>`: identical for the same
/// `(tier, seed)`, different for a different seed. Each file is laid out as
/// `repo{r}/src/file_{f}.rs` and carries:
///
/// - a deterministic set of filler lines drawn from `VOCAB`;
/// - the query token `SCALE_QUERY_TOKEN` planted at the tier's `hit_density`;
/// - synthetic `fn` symbol lines at the tier's `symbol_density`.
#[must_use]
pub fn generate_corpus(tier: ScaleTier, seed: u64) -> Vec<(String, String)> {
    let params = params_for(tier);
    let mut out: Vec<(String, String)> = Vec::new();
    for repo_index in 0..params.repo_count {
        for file_index in 0..params.files_per_repo {
            let path = format!("repo{repo_index}/src/file_{file_index}.rs");
            let content = generate_file(tier, &params, repo_index, file_index, seed);
            out.push((path, content));
        }
    }
    out
}

/// A source file's identity inside one serving owner. The path is relative to
/// `source_repo_id`; two source repositories may own the same relative path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedFile {
    pub source_repo_id: String,
    pub repo_relative_path: String,
    pub content: String,
}

fn parse_source_repo_index(source_repo_id: &str) -> AnyResult<u32> {
    let suffix = source_repo_id
        .strip_prefix("repo")
        .ok_or_else(|| anyhow::anyhow!("scale: invalid source repo ID"))?;
    suffix
        .parse::<u32>()
        .map_err(|error| anyhow::anyhow!("scale: invalid source repo ID {source_repo_id}: {error}"))
}

/// Planted query token unique to one generated source repository.
pub fn repo_query_token(repo_index: u32) -> String {
    format!("scalereponeedle{repo_index:03}")
}

fn file_query_token(repo_index: u32, file_index: u32) -> String {
    format!("scalefileneedle{repo_index:03}file{file_index:05}")
}

/// Reuse the seeded file generator while assigning real source-repository
/// identities. The extra repo-specific anchor makes each source repo
/// independently observable through lexical queries.
pub fn generate_scoped_corpus(tier: ScaleTier, seed: u64) -> AnyResult<Vec<ScopedFile>> {
    let params = params_for(tier);
    let mut files = Vec::with_capacity(usize::try_from(params.total_files())?);
    for repo_index in 0..params.repo_count {
        for file_index in 0..params.files_per_repo {
            let mut content = generate_file(tier, &params, repo_index, file_index, seed);
            writeln!(&mut content, "// {} anchor", repo_query_token(repo_index))?;
            writeln!(
                &mut content,
                "// {} anchor",
                file_query_token(repo_index, file_index)
            )?;
            files.push(ScopedFile {
                source_repo_id: format!("repo{repo_index}"),
                repo_relative_path: format!("src/file_{file_index}.rs"),
                content,
            });
        }
    }
    Ok(files)
}

/// Bind the existing corpus digest to the source identity.
///
/// The generator reserves `repoN` as a single path component, so this canonical
/// projection is injective over this fixture and matches the old path shape.
#[must_use]
pub fn scoped_corpus_digest(dimension: &str, files: &[ScopedFile]) -> String {
    let paths = files
        .iter()
        .map(|file| format!("{}/{}", file.source_repo_id, file.repo_relative_path))
        .collect::<Vec<_>>();
    corpus_digest_refs(
        dimension,
        paths
            .iter()
            .zip(files)
            .map(|(path, file)| (path.as_str(), file.content.as_str())),
    )
}

/// Source-derived identities and expected result counts, independent of the
/// engine's candidate list or ranking.
#[derive(Clone, Debug)]
pub struct ScopedOracle {
    paths_by_repo: BTreeMap<String, BTreeSet<String>>,
}

impl ScopedOracle {
    pub fn from_source(files: &[ScopedFile], tier: ScaleTier) -> AnyResult<Self> {
        let params = params_for(tier);
        let mut paths_by_repo = BTreeMap::<String, BTreeSet<String>>::new();
        for file in files {
            let repo_index = parse_source_repo_index(&file.source_repo_id)?;
            if repo_index >= params.repo_count {
                return Err(anyhow::anyhow!("scale: invalid source repo ID"));
            }
            let repo_anchor = format!("// {} anchor", repo_query_token(repo_index));
            let file_index = file
                .repo_relative_path
                .strip_prefix("src/file_")
                .and_then(|suffix| suffix.strip_suffix(".rs"))
                .and_then(|digits| digits.parse::<u32>().ok())
                .ok_or_else(|| anyhow::anyhow!("scale: invalid scoped file path"))?;
            let file_anchor = format!("// {} anchor", file_query_token(repo_index, file_index));
            if !file.repo_relative_path.starts_with("src/")
                || file.repo_relative_path.contains("..")
                || !file.content.contains(SCALE_QUERY_TOKEN)
                || !file.content.lines().any(|line| line == repo_anchor)
                || !file.content.lines().any(|line| line == file_anchor)
            {
                return Err(anyhow::anyhow!(
                    "scale: invalid planted source for {}/{}",
                    file.source_repo_id,
                    file.repo_relative_path
                ));
            }
            let paths = paths_by_repo
                .entry(file.source_repo_id.clone())
                .or_default();
            if !paths.insert(file.repo_relative_path.clone()) {
                return Err(anyhow::anyhow!(
                    "scale: duplicate source identity {}/{}",
                    file.source_repo_id,
                    file.repo_relative_path
                ));
            }
        }
        let expected_files_per_repo = usize::try_from(params.files_per_repo)?;
        let expected_paths = (0..params.files_per_repo)
            .map(|index| format!("src/file_{index}.rs"))
            .collect::<BTreeSet<_>>();
        if paths_by_repo.len() != usize::try_from(params.repo_count)?
            || paths_by_repo
                .values()
                .any(|paths| paths.len() != expected_files_per_repo || paths != &expected_paths)
        {
            return Err(anyhow::anyhow!(
                "scale: source repository/file counts differ from the tier manifest"
            ));
        }
        Ok(Self { paths_by_repo })
    }

    fn without_file(&self, source_repo_id: &str, path: &str) -> AnyResult<Self> {
        let mut successor = self.clone();
        let paths = successor
            .paths_by_repo
            .get_mut(source_repo_id)
            .ok_or_else(|| anyhow::anyhow!("scale: source repo absent from deletion oracle"))?;
        if !paths.remove(path) {
            return Err(anyhow::anyhow!(
                "scale: source file absent from deletion oracle"
            ));
        }
        Ok(successor)
    }

    fn expected_count(&self, source_repo_id: Option<&str>) -> AnyResult<usize> {
        let files = source_repo_id.map_or_else(
            || self.paths_by_repo.values().map(BTreeSet::len).sum(),
            |repo| self.paths_by_repo.get(repo).map_or(0, BTreeSet::len),
        );
        Ok(files.min(usize::try_from(SCALE_TOP_K)?))
    }

    /// Verify one observed page without deriving the expected set from a
    /// previous query. A source-repo probe must contain only that repo's rows.
    fn verify_identities<'a>(
        &self,
        source_repo_id: Option<&str>,
        candidates: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> AnyResult<usize> {
        let mut seen = BTreeSet::new();
        let mut count: usize = 0;
        for (repo, path) in candidates {
            if source_repo_id.is_some_and(|expected| expected != repo)
                || !self
                    .paths_by_repo
                    .get(repo)
                    .is_some_and(|paths| paths.contains(path))
                || !seen.insert((repo, path))
            {
                return Err(anyhow::anyhow!(
                    "scale: response has a foreign or duplicate source identity {repo}/{path}"
                ));
            }
            count = count.saturating_add(1);
        }
        require_result_count(count, self.expected_count(source_repo_id)?, "scoped query")?;
        Ok(count)
    }

    /// Require a full, distinct page of source-backed candidates. Set
    /// `source_repo_id` to prove one repository independently of global rank.
    pub fn verify_page(
        &self,
        source_repo_id: Option<&str>,
        candidates: &[LexicalCandidate],
    ) -> AnyResult<usize> {
        self.verify_identities(
            source_repo_id,
            candidates.iter().map(|candidate| {
                (
                    candidate.source_repo_id.as_str(),
                    candidate.repo_relative_path.as_str(),
                )
            }),
        )
    }
}

/// The scale fixture and source paths are ASCII.
///
/// Count distinct three-byte windows per file/surface, matching the file
/// authority's membership admission. These limits mirror
/// `lexical/file_authority.rs`; keep focused threshold tests in sync when that
/// authority changes.
const MAX_SCALE_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SCALE_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SCALE_POSTING_MEMBERSHIPS: u64 = 4_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScaleAdmissionLimit {
    SourceFileBytes,
    TotalSourceBytes,
    TrigramPostingMemberships,
}

impl ScaleAdmissionLimit {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceFileBytes => "lexical_source_file_bytes_8m",
            Self::TotalSourceBytes => "lexical_total_source_bytes_128m",
            Self::TrigramPostingMemberships => "lexical_trigram_posting_memberships_4m",
        }
    }
}

#[derive(Debug)]
pub struct ScaleAdmissionRefusal {
    pub limit: ScaleAdmissionLimit,
    pub observed: u64,
    pub maximum: u64,
}

impl std::fmt::Display for ScaleAdmissionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "scale: {} admission refused: observed {} > maximum {}",
            self.limit.as_str(),
            self.observed,
            self.maximum
        )
    }
}

impl std::error::Error for ScaleAdmissionRefusal {}

#[derive(Debug)]
pub struct ScaleStageError {
    pub stage: &'static str,
    pub limit: Option<ScaleAdmissionLimit>,
    pub observed: Option<u64>,
    pub maximum: Option<u64>,
    pub message: String,
}

impl std::fmt::Display for ScaleStageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "scale {}: {}", self.stage, self.message)
    }
}

impl std::error::Error for ScaleStageError {}

impl ScaleStageError {
    pub fn operation(stage: &'static str, error: anyhow::Error) -> Self {
        Self {
            stage,
            limit: None,
            observed: None,
            maximum: None,
            message: format!("{error:#}"),
        }
    }

    pub fn source_admission(error: anyhow::Error) -> Self {
        let refusal = error.downcast_ref::<ScaleAdmissionRefusal>();
        Self {
            stage: "source_preflight",
            limit: refusal.map(|refusal| refusal.limit),
            observed: refusal.map(|refusal| refusal.observed),
            maximum: refusal.map(|refusal| refusal.maximum),
            message: format!("{error:#}"),
        }
    }

    pub fn wire_admission(error: anyhow::Error) -> Self {
        Self {
            stage: "wire_preflight",
            limit: None,
            observed: None,
            maximum: None,
            message: format!("{error:#}"),
        }
    }
}

fn stage_or_preserve<E: Into<anyhow::Error>>(stage: &'static str, error: E) -> anyhow::Error {
    let error = error.into();
    if error.downcast_ref::<ScaleStageError>().is_some()
        || error.downcast_ref::<ScaleRuntimeFailure>().is_some()
    {
        error
    } else {
        ScaleStageError::operation(stage, error).into()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopedCorpusAdmission {
    pub source_bytes: u64,
    pub posting_memberships: u64,
}

fn check_admission_counts(
    file_bytes: u64,
    source_bytes: u64,
    postings: u64,
) -> Result<(), ScaleAdmissionRefusal> {
    if file_bytes > MAX_SCALE_FILE_BYTES {
        return Err(ScaleAdmissionRefusal {
            limit: ScaleAdmissionLimit::SourceFileBytes,
            observed: file_bytes,
            maximum: MAX_SCALE_FILE_BYTES,
        });
    }
    if source_bytes > MAX_SCALE_SOURCE_BYTES {
        return Err(ScaleAdmissionRefusal {
            limit: ScaleAdmissionLimit::TotalSourceBytes,
            observed: source_bytes,
            maximum: MAX_SCALE_SOURCE_BYTES,
        });
    }
    if postings > MAX_SCALE_POSTING_MEMBERSHIPS {
        return Err(ScaleAdmissionRefusal {
            limit: ScaleAdmissionLimit::TrigramPostingMemberships,
            observed: postings,
            maximum: MAX_SCALE_POSTING_MEMBERSHIPS,
        });
    }
    Ok(())
}

fn distinct_ascii_trigrams(bytes: &[u8]) -> AnyResult<usize> {
    let trigrams = bytes
        .windows(3)
        .map(<[u8; 3]>::try_from)
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(trigrams.len())
}

/// Refuse oversized source/posting fixtures before starting the daemon.
///
/// `source_scope` adds one newline after each chunk, which is included here.
/// IPC wire size is checked separately against the actual pending batch because
/// its encoded metadata cannot be inferred from bytes.
pub fn preflight_scoped_corpus(files: &[ScopedFile]) -> AnyResult<ScopedCorpusAdmission> {
    let mut source_bytes = 0_u64;
    let mut posting_memberships = 0_u64;
    for file in files {
        if !file.content.is_ascii() || !file.repo_relative_path.is_ascii() {
            return Err(anyhow::anyhow!(
                "scale: scoped fixture must remain ASCII for exact admission preflight"
            ));
        }
        let file_bytes = u64::try_from(file.content.len())?
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("scale: source byte count overflow"))?;
        source_bytes = source_bytes
            .checked_add(file_bytes)
            .ok_or_else(|| anyhow::anyhow!("scale: source byte count overflow"))?;
        check_admission_counts(file_bytes, source_bytes, posting_memberships)?;

        let path = file.repo_relative_path.to_ascii_lowercase();
        let mut source = file.content.to_ascii_lowercase().into_bytes();
        source.push(b'\n');
        let memberships = distinct_ascii_trigrams(path.as_bytes())?
            .checked_add(distinct_ascii_trigrams(&source)?)
            .ok_or_else(|| anyhow::anyhow!("scale: posting membership count overflow"))?;
        posting_memberships = posting_memberships
            .checked_add(u64::try_from(memberships)?)
            .ok_or_else(|| anyhow::anyhow!("scale: posting membership count overflow"))?;
        check_admission_counts(file_bytes, source_bytes, posting_memberships)?;
    }
    Ok(ScopedCorpusAdmission {
        source_bytes,
        posting_memberships,
    })
}

/// Build one file's content deterministically.
fn generate_file(
    tier: ScaleTier,
    params: &TierParams,
    repo_index: u32,
    file_index: u32,
    seed: u64,
) -> String {
    let mut state = file_seed(tier, repo_index, file_index, seed);
    let mut lines: Vec<String> = Vec::new();
    for line_index in 0..params.avg_file_lines {
        state = lcg_next(state);
        // `per_mille` thresholds: planted features fire when the rolled value's
        // low 1000-modulus lands under the configured density.
        let roll = state % 1000;
        if roll < u64::from(params.hit_density_per_mille) {
            lines.push(format!("// {SCALE_QUERY_TOKEN} {}", pick_word(state)));
        } else if roll < u64::from(params.symbol_density_per_mille) {
            lines.push(format!(
                "fn sym_{repo_index}_{file_index}_{line_index}() {{ /* {} */ }}",
                pick_word(state)
            ));
        } else {
            lines.push(format!("let {} = {};", pick_word(state), line_index));
        }
    }
    // Guarantee at least one hit per file so the measured query is never empty
    // by an unlucky-but-deterministic roll; this is a construction guarantee,
    // documented, not a silent fallback over a failure.
    lines.push(format!("// guaranteed {SCALE_QUERY_TOKEN} anchor"));
    let mut content = lines.join("\n");
    content.push('\n');
    content
}

// ---------------------------------------------------------------------------
// Small-tier measurement; scoped tiers use measure_tier below.
// ---------------------------------------------------------------------------

/// Result cap and repetition of the warm and adapter-only query loops.
const WARM_QUERY_SAMPLES: usize = 32;

/// The daemon's own timing of the phases a query passes through, read from
/// its metrics scrape as before/after deltas.
///
/// Every value is what the daemon measured of itself at whole-millisecond
/// resolution (its histograms carry integer milliseconds), never a wall
/// clock read across the socket.
#[derive(Clone, Copy, Debug, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "each field names the millisecond metric it was read from"
)]
pub struct DaemonPhaseTimingV1 {
    /// `lq_snapshot_lexical_cold_open_ms` if the first query opens a snapshot.
    /// Activation normally promotes an already opened snapshot, so no query
    /// cold-open sample exists in that case.
    pub cold_open_ms: Option<f64>,
    /// `lq_route_lexical_latency_ms` of the first query inside the dispatcher.
    pub first_route_ms: f64,
    /// Mean `lq_route_lexical_latency_ms` over the warm queries: plan +
    /// execute inside the dispatcher, no open.
    pub warm_route_mean_ms: f64,
}

/// The same generation opened and queried in-process through the lexical
/// adapter, beside the daemon: the adapter-only cost with no socket, no
/// dispatcher and no read view.
#[derive(Clone, Copy, Debug, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "every phase is timed in milliseconds and the artifact names the unit"
)]
pub struct AdapterPhaseTimingV1 {
    /// `LexicalAdapter::open` of the sealed generation.
    pub open_ms: f64,
    /// Median of lowering the text query to its plan (the same lowering the
    /// dispatcher runs).
    pub plan_ms: f64,
    /// Median of `search_constrained` on the opened searcher: execution
    /// alone.
    pub execute_ms: f64,
}

/// One delta step: one file changed, ingested and sealed as a delta
/// generation, then activated. Retention may reclaim an older generation;
/// the recorded byte difference establishes whether physical bytes shrank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeltaMeasurementV1 {
    /// Ingest of the one changed file through seal.
    pub update_ms: f64,
    /// Bytes of the changed file.
    pub changed_bytes: u64,
    /// Bytes the state root grew by during the delta build.
    pub bytes_written: u64,
    /// Activation of the delta generation. The historical field name also
    /// covers any reclaim performed within the activation, but a one-file
    /// delta under two-generation retention may reclaim nothing.
    pub activation_with_reclaim_ms: f64,
    /// Bytes the state root shrank by across that activation; zero means no
    /// physical reclaim was observed in this window.
    pub reclaimed_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeleteReopenMeasurementV1 {
    pub delete_seal_ms: f64,
    pub delete_activation_ms: f64,
    /// Stops and restarts the daemon thread in the same OS process, then
    /// waits for readiness. This is not a cold process or cold page cache.
    pub same_process_reopen_ms: f64,
    /// First served positive file query after the daemon thread is ready.
    pub reopened_first_query_ms: f64,
}

/// Captured measurements for one measured tier run.
#[derive(Clone, Debug)]
pub struct TierMeasurement {
    pub tier: ScaleTier,
    pub seed: u64,
    pub file_count: usize,
    pub corpus_digest: String,
    /// Distinct source repositories inside one serving owner/generation.
    pub source_repo_count: usize,
    /// Bytes of every generated file.
    pub corpus_bytes: u64,
    /// Actual encoded pending IPC envelope, before the timed seal.
    pub ingest_decoded_bytes: Option<u64>,
    pub ingest_wire_bytes: Option<u64>,
    /// Ingest of every file through seal.
    pub build_ms: f64,
    /// Bytes the state root grew by during the full build.
    pub build_bytes_written: u64,
    /// Activation of the freshly sealed generation (no reclaim).
    pub activation_ms: f64,
    /// Wall time of the first query after activation, across the socket.
    pub first_query_ms: f64,
    /// Wall time of the warm queries after it, across the socket.
    pub warm_query: LatencySummary,
    pub daemon: DaemonPhaseTimingV1,
    pub adapter: AdapterPhaseTimingV1,
    pub delta: DeltaMeasurementV1,
    pub result_count: usize,
    pub model_revision: Option<String>,
    /// Effective client request deadline; raising it does not raise daemon
    /// admission limits and must be recorded with each measured tier.
    pub client_request_timeout_ms: u64,
    pub requested_client_request_timeout_ms: Option<u64>,
    pub history_max_bytes: u64,
    pub requested_history_max_bytes: Option<u64>,
    /// RUSAGE_SELF around runtime boot through driver cleanup. The daemon is
    /// an in-process thread; this includes harness and daemon CPU time.
    pub cpu: Option<CpuUsageV1>,
    /// Current-RSS samples and process CPU for the named timed operations.
    /// Separate from the process high-water RSS in `BenchArtifactV1`.
    pub phase_resources: BTreeMap<&'static str, PhaseResourceV1>,
    pub delete_reopen: Option<DeleteReopenMeasurementV1>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuUsageV1 {
    pub user_ms: f64,
    pub system_ms: f64,
}

const PHASE_RSS_INTERVAL: Duration = Duration::from_millis(100);
const PHASE_RSS_MAX_GAP: Duration = Duration::from_millis(500);
const PHASE_RSS_INTERIOR_REQUIRED_AFTER: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, PartialEq)]
pub struct PhaseResourceV1 {
    pub cpu: CpuUsageV1,
    pub rss_start_bytes: u64,
    pub rss_end_bytes: u64,
    /// Greatest observed current RSS; sampling cannot establish a true peak.
    pub sampled_max_rss_bytes: u64,
    pub interior_samples: usize,
    pub observed_max_gap_ms: f64,
    pub observation_span_ms: f64,
    pub observer_setup_ms: f64,
    pub observer_teardown_ms: f64,
    pub observer_periodic_probe_wall_ms: f64,
    pub discarded_outside_phase_samples: usize,
}

#[derive(Clone, Copy)]
struct RssPoint {
    at: Instant,
    bytes: u64,
}

struct PhaseSampler {
    started: Instant,
    cpu_started: CpuSnapshot,
    rss_start: RssPoint,
    setup_ms: f64,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<AnyResult<(Vec<RssPoint>, f64)>>>,
}

impl PhaseSampler {
    fn start() -> AnyResult<Self> {
        let setup_started = Instant::now();
        let rss_start_bytes = current_rss_bytes()?;
        let rss_start = RssPoint {
            at: Instant::now(),
            bytes: rss_start_bytes,
        };
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new()
            .name("scale-rss-sampler".to_string())
            .spawn(move || {
                let mut samples = Vec::new();
                let mut probe_wall_ms = 0.0;
                while !worker_stopped.load(Ordering::Acquire) {
                    thread::park_timeout(PHASE_RSS_INTERVAL);
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let probe_started = Instant::now();
                    let bytes = current_rss_bytes()?;
                    probe_wall_ms += probe_started.elapsed().as_secs_f64() * 1_000.0;
                    samples.push(RssPoint {
                        at: Instant::now(),
                        bytes,
                    });
                }
                Ok((samples, probe_wall_ms))
            })?;
        let cpu_started = match CpuSnapshot::observe() {
            Ok(value) => value,
            Err(error) => {
                stopped.store(true, Ordering::Release);
                worker.thread().unpark();
                let _ = worker.join();
                return Err(error);
            }
        };
        let started = Instant::now();
        let setup_ms = setup_started.elapsed().as_secs_f64() * 1_000.0;
        Ok(Self {
            started,
            cpu_started,
            rss_start,
            setup_ms,
            stopped,
            worker: Some(worker),
        })
    }

    fn stop(mut self) -> AnyResult<PhaseResourceV1> {
        let ended = Instant::now();
        let cpu_ended = CpuSnapshot::observe();
        let teardown_started = Instant::now();
        self.stopped.store(true, Ordering::Release);
        let Some(worker) = self.worker.take() else {
            anyhow::bail!("scale: RSS sampler worker missing at stop");
        };
        worker.thread().unpark();
        let (samples, observer_periodic_probe_wall_ms) = worker
            .join()
            .map_err(|_| anyhow::anyhow!("scale: RSS sampler thread panicked"))??;
        let rss_end_bytes = current_rss_bytes()?;
        let rss_end = RssPoint {
            at: Instant::now(),
            bytes: rss_end_bytes,
        };
        let cpu = cpu_ended?.elapsed_since(self.cpu_started)?;
        summarize_phase_resources(
            self.started,
            ended,
            self.rss_start,
            rss_end,
            &samples,
            cpu,
            self.setup_ms,
            teardown_started.elapsed().as_secs_f64() * 1_000.0,
            observer_periodic_probe_wall_ms,
        )
    }
}

impl Drop for PhaseSampler {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stopped.store(true, Ordering::Release);
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn summarize_phase_resources(
    started: Instant,
    ended: Instant,
    rss_start: RssPoint,
    rss_end: RssPoint,
    samples: &[RssPoint],
    cpu: CpuUsageV1,
    setup_ms: f64,
    teardown_ms: f64,
    periodic_probe_wall_ms: f64,
) -> AnyResult<PhaseResourceV1> {
    if ended < started || rss_start.at > started || rss_end.at < ended {
        anyhow::bail!("scale: RSS phase boundary timestamps are invalid");
    }
    if rss_start.bytes == 0 || rss_end.bytes == 0 {
        anyhow::bail!("scale: RSS phase boundary sample is zero");
    }
    let mut previous = rss_start.at;
    let mut sampled_max = rss_start.bytes.max(rss_end.bytes);
    let mut interior_samples = 0;
    let mut discarded_outside_phase_samples = 0;
    for sample in samples {
        if sample.bytes == 0 || sample.at <= previous || sample.at >= rss_end.at {
            anyhow::bail!("scale: RSS sample is zero or timestamps are not increasing");
        }
        previous = sample.at;
        if sample.at > started && sample.at < ended {
            interior_samples += 1;
            sampled_max = sampled_max.max(sample.bytes);
        } else {
            discarded_outside_phase_samples += 1;
        }
    }
    // The endpoint gap, rather than average cadence, detects a stalled sampler.
    let mut max_gap = Duration::ZERO;
    let mut previous = rss_start.at;
    for sample in samples {
        if sample.at > started && sample.at < ended {
            max_gap = max_gap.max(sample.at.duration_since(previous));
            previous = sample.at;
        }
    }
    max_gap = max_gap.max(rss_end.at.duration_since(previous));
    if ended.duration_since(started) >= PHASE_RSS_INTERIOR_REQUIRED_AFTER && interior_samples == 0 {
        anyhow::bail!("scale: long phase has no interior RSS sample");
    }
    if max_gap > PHASE_RSS_MAX_GAP {
        anyhow::bail!("scale: RSS sampling gap exceeds 500 ms");
    }
    Ok(PhaseResourceV1 {
        cpu,
        rss_start_bytes: rss_start.bytes,
        rss_end_bytes: rss_end.bytes,
        sampled_max_rss_bytes: sampled_max,
        interior_samples,
        observed_max_gap_ms: max_gap.as_secs_f64() * 1_000.0,
        observation_span_ms: ended.duration_since(started).as_secs_f64() * 1_000.0,
        observer_setup_ms: setup_ms,
        observer_teardown_ms: teardown_ms,
        observer_periodic_probe_wall_ms: periodic_probe_wall_ms,
        discarded_outside_phase_samples,
    })
}

fn observe_phase<T>(work: impl FnOnce() -> AnyResult<T>) -> AnyResult<(T, PhaseResourceV1)> {
    let sampler =
        PhaseSampler::start().map_err(|error| stage_or_preserve("resource_observation", error))?;
    let measurement = work();
    let observation = sampler
        .stop()
        .map_err(|error| stage_or_preserve("resource_observation", error));
    combine_phase_result(measurement, observation)
}

fn combine_phase_result<T>(
    measurement: AnyResult<T>,
    observation: AnyResult<PhaseResourceV1>,
) -> AnyResult<(T, PhaseResourceV1)> {
    match (measurement, observation) {
        (Ok(value), Ok(resources)) => Ok((value, resources)),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(cleanup)) => Err(ScaleRuntimeFailure {
            primary: Some(primary),
            cleanup,
            cleanup_context: "phase resource observation",
        }
        .into()),
    }
}

fn record_phase(
    phases: &mut BTreeMap<&'static str, PhaseResourceV1>,
    name: &'static str,
    observation: PhaseResourceV1,
) -> AnyResult<()> {
    if phases.insert(name, observation).is_some() {
        anyhow::bail!("scale: duplicate phase resource observation {name}");
    }
    Ok(())
}

fn current_rss_bytes() -> AnyResult<u64> {
    #[cfg(target_os = "linux")]
    {
        let bytes = KernelResidentMemoryProbe.resident_bytes()?;
        anyhow::ensure!(bytes > 0, "scale: Linux VmRSS is zero");
        Ok(bytes)
    }
    #[cfg(target_os = "macos")]
    {
        current_rss_bytes_via_ps()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        anyhow::bail!("scale: current RSS observation unsupported on this OS")
    }
}

#[cfg(target_os = "macos")]
fn current_rss_bytes_via_ps() -> AnyResult<u64> {
    let mut child = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        if let Some(status) = status {
            anyhow::ensure!(status.success(), "scale: ps RSS probe exited {status}");
            let mut output = String::new();
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("scale: ps RSS stdout unavailable"))?
                .read_to_string(&mut output)?;
            let kib = output.trim().parse::<u64>()?;
            anyhow::ensure!(kib > 0, "scale: ps RSS is zero");
            return kib
                .checked_mul(1024)
                .ok_or_else(|| anyhow::anyhow!("scale: ps RSS overflows bytes"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("scale: ps RSS probe exceeded 2 s deadline");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Clone, Copy)]
struct CpuSnapshot {
    user_us: i64,
    system_us: i64,
}

impl CpuSnapshot {
    fn observe() -> AnyResult<Self> {
        let usage = getrusage(UsageWho::RUSAGE_SELF)?;
        Ok(Self {
            user_us: usage.user_time().num_microseconds(),
            system_us: usage.system_time().num_microseconds(),
        })
    }

    fn elapsed_since(self, earlier: Self) -> AnyResult<CpuUsageV1> {
        let user = self
            .user_us
            .checked_sub(earlier.user_us)
            .filter(|elapsed| *elapsed >= 0)
            .ok_or_else(|| anyhow::anyhow!("scale: CPU user time decreased"))?;
        let system = self
            .system_us
            .checked_sub(earlier.system_us)
            .filter(|elapsed| *elapsed >= 0)
            .ok_or_else(|| anyhow::anyhow!("scale: CPU system time decreased"))?;
        Ok(CpuUsageV1 {
            user_ms: Duration::from_micros(u64::try_from(user)?).as_secs_f64() * 1_000.0,
            system_ms: Duration::from_micros(u64::try_from(system)?).as_secs_f64() * 1_000.0,
        })
    }
}

fn scale_runtime(config: ScaleRuntimeConfig) -> AnyResult<E2eRuntime> {
    let runtime =
        match config.client_timeout {
            Some(timeout) => Ok(E2eRuntime::boot_with_client_request_timeout(timeout)?
                .with_history_max_generations(2)),
            None => E2eRuntime::boot_with_history_max_generations(2),
        }?;
    Ok(runtime.with_history_max_bytes(config.effective_history_max_bytes()?))
}

fn timeout_ms(client_timeout: Option<Duration>) -> AnyResult<u64> {
    let duration = client_timeout.unwrap_or(DEFAULT_CLIENT_IO_TIMEOUT);
    let millis = u64::try_from(duration.as_millis())?;
    if !(1..=600_000).contains(&millis) || Duration::from_millis(millis) != duration {
        anyhow::bail!("scale: client request timeout must be whole milliseconds in 1..=600000");
    }
    Ok(millis)
}

/// Keep the measurement error and daemon teardown error separately. A failed
/// driver must never turn a failed measurement into a successful tier, and a
/// panic in `E2eRuntime::Drop` must not erase the primary error.
#[derive(Debug)]
struct ScaleRuntimeFailure {
    primary: Option<anyhow::Error>,
    cleanup: anyhow::Error,
    cleanup_context: &'static str,
}

impl std::fmt::Display for ScaleRuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(primary) = &self.primary {
            write!(
                formatter,
                "scale measurement failed: {primary:#}; {} failed: {:#}",
                self.cleanup_context, self.cleanup
            )
        } else {
            write!(
                formatter,
                "scale {} failed: {:#}",
                self.cleanup_context, self.cleanup
            )
        }
    }
}

impl std::error::Error for ScaleRuntimeFailure {}

fn finish_runtime_measurement<T>(
    measurement: AnyResult<T>,
    cleanup: AnyResult<()>,
) -> AnyResult<T> {
    match (measurement, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (primary, Err(cleanup)) => Err(ScaleRuntimeFailure {
            primary: primary.err(),
            cleanup,
            cleanup_context: "daemon cleanup",
        }
        .into()),
    }
}

/// Convert an elapsed `Instant` span to milliseconds.
fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// `count` and `sum` of one histogram in the daemon's scrape; a histogram
/// the daemon has not emitted yet reads as zero of each.
fn histogram_totals(snapshot: &MetricsSnapshotV1, name: &str) -> (u64, f64) {
    snapshot
        .histograms
        .iter()
        .find(|histogram| histogram.name == name)
        .map_or((0, 0.0), |histogram| (histogram.count, histogram.sum))
}

/// The daemon's timing of a query window as the delta of its histograms
/// between two scrapes, requiring exactly `expected_count` samples in it.
fn histogram_window(
    before: &MetricsSnapshotV1,
    after: &MetricsSnapshotV1,
    name: &str,
    expected_count: u64,
) -> AnyResult<f64> {
    let (count_before, sum_before) = histogram_totals(before, name);
    let (count_after, sum_after) = histogram_totals(after, name);
    let observed = count_after.saturating_sub(count_before);
    if observed != expected_count {
        return Err(anyhow::anyhow!(
            "scale: `{name}` recorded {observed} samples in the window, expected {expected_count}"
        ));
    }
    Ok(sum_after - sum_before)
}

fn optional_cold_open_window(
    before: &MetricsSnapshotV1,
    after: &MetricsSnapshotV1,
) -> AnyResult<Option<f64>> {
    let name = "lq_snapshot_lexical_cold_open_ms";
    let (count_before, sum_before) = histogram_totals(before, name);
    let (count_after, sum_after) = histogram_totals(after, name);
    let count = count_after
        .checked_sub(count_before)
        .ok_or_else(|| anyhow::anyhow!("scale: cold-open histogram count decreased"))?;
    match count {
        0 if sum_after == sum_before => Ok(None),
        1 => Ok(Some(sum_after - sum_before)),
        _ => Err(anyhow::anyhow!(
            "scale: cold-open histogram recorded {count} samples for one query"
        )),
    }
}

fn median_ms(samples: &mut [f64]) -> AnyResult<f64> {
    samples.sort_by(f64::total_cmp);
    LatencySummary::from_samples_ms(samples)
        .map(|summary| summary.p50_ms)
        .ok_or_else(|| anyhow::anyhow!("scale: no samples to take a median of"))
}

/// The one query every timed loop issues, as the daemon receives it.
fn scale_query() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: SCALE_QUERY_TOKEN.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: SCALE_TOP_K,
        cursor: None,
    }
}

/// Issue the scale query once across the socket and require a served,
/// non-empty page.
fn served_query(rt: &mut E2eRuntime) -> AnyResult<usize> {
    let result = rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K);
    if let Some(error) = result.typed_error {
        return Err(anyhow::anyhow!(
            "scale small-tier query returned typed error {}: {}",
            error.code,
            error.message
        ));
    }
    if result.candidates.is_empty() {
        return Err(anyhow::anyhow!(
            "scale small-tier query returned an empty ordering; planted `{SCALE_QUERY_TOKEN}` was not retrievable"
        ));
    }
    Ok(result.candidates.len())
}

/// Every generated small-tier file carries the planted term. The expected
/// bounded result count comes from those source bytes, not a prior search.
fn expected_small_result_count(corpus: &[(String, String)]) -> AnyResult<usize> {
    if corpus.is_empty()
        || corpus
            .iter()
            .any(|(_, content)| !content.contains(SCALE_QUERY_TOKEN))
    {
        return Err(anyhow::anyhow!(
            "scale: the small-tier source fixture lacks its planted query in a file"
        ));
    }
    Ok(corpus.len().min(usize::try_from(SCALE_TOP_K)?))
}

fn require_result_count(observed: usize, expected: usize, phase: &str) -> AnyResult<()> {
    if observed != expected {
        return Err(anyhow::anyhow!(
            "scale: {phase} returned {observed} candidates, source oracle requires {expected}"
        ));
    }
    Ok(())
}

/// Keep the socket-query clock free of validation work while refusing any
/// measured response whose count differs from the planted source oracle.
fn collect_warm_samples(
    expected: usize,
    sample_count: usize,
    mut query: impl FnMut() -> AnyResult<usize>,
) -> AnyResult<Vec<f64>> {
    let mut samples_ms = Vec::with_capacity(sample_count);
    for sample_index in 0..sample_count {
        let started = Instant::now();
        let observed = query().map_err(|err| {
            anyhow::anyhow!(
                "scale: warm query sample {}/{} failed: {err}",
                sample_index.saturating_add(1),
                sample_count
            )
        })?;
        let elapsed = elapsed_ms(started);
        require_result_count(
            observed,
            expected,
            &format!(
                "warm query sample {}/{}",
                sample_index.saturating_add(1),
                sample_count
            ),
        )?;
        samples_ms.push(elapsed);
    }
    Ok(samples_ms)
}

fn collect_validated_samples<T>(
    sample_count: usize,
    mut query: impl FnMut() -> T,
    mut validate: impl FnMut(&T) -> AnyResult<()>,
) -> AnyResult<Vec<f64>> {
    let mut samples_ms = Vec::with_capacity(sample_count);
    for sample_index in 0..sample_count {
        let started = Instant::now();
        let response = query();
        let elapsed = elapsed_ms(started);
        validate(&response).map_err(|error| {
            anyhow::anyhow!(
                "scale: warm query sample {}/{} failed: {error}",
                sample_index.saturating_add(1),
                sample_count
            )
        })?;
        samples_ms.push(elapsed);
    }
    Ok(samples_ms)
}

fn validate_scoped_response(
    oracle: &ScopedOracle,
    source_repo_id: Option<&str>,
    result: &crate::harness::E2eQueryResult,
) -> AnyResult<usize> {
    if let Some(error) = &result.typed_error {
        return Err(anyhow::anyhow!(
            "scale: scoped query returned typed error {}: {}",
            error.code,
            error.message
        ));
    }
    oracle.verify_page(source_repo_id, &result.candidates)
}

fn verify_scoped_repositories(rt: &mut E2eRuntime, oracle: &ScopedOracle) -> AnyResult<()> {
    for source_repo_id in oracle.paths_by_repo.keys() {
        let repo_index = parse_source_repo_index(source_repo_id)?;
        let response = rt.query_text(
            TextQuerySyntax::Native,
            &repo_query_token(repo_index),
            SCALE_TOP_K,
        );
        let _verified_count = validate_scoped_response(oracle, Some(source_repo_id), &response)?;
    }
    Ok(())
}

fn require_single_source_file(
    result: &crate::harness::E2eQueryResult,
    source_repo_id: &str,
    path: &str,
) -> AnyResult<()> {
    if let Some(error) = &result.typed_error {
        anyhow::bail!(
            "scale: file query returned typed error {}: {}",
            error.code,
            error.message
        );
    }
    if result.candidates.len() != 1
        || !result.candidates.first().is_some_and(|candidate| {
            candidate.source_repo_id.as_str() == source_repo_id
                && candidate.repo_relative_path.as_str() == path
        })
    {
        anyhow::bail!("scale: file query did not return exactly {source_repo_id}/{path}");
    }
    Ok(())
}

fn require_no_source_file(result: &crate::harness::E2eQueryResult) -> AnyResult<()> {
    if let Some(error) = &result.typed_error {
        anyhow::bail!(
            "scale: deleted-file query returned typed error {}: {}",
            error.code,
            error.message
        );
    }
    if !result.candidates.is_empty() {
        anyhow::bail!("scale: deleted-file query still returned candidates");
    }
    Ok(())
}

fn measure_scoped_delete_reopen(
    rt: &mut E2eRuntime,
    oracle: &ScopedOracle,
    file: &ScopedFile,
) -> AnyResult<(
    DeleteReopenMeasurementV1,
    BTreeMap<&'static str, PhaseResourceV1>,
)> {
    if file.source_repo_id != "repo0" || file.repo_relative_path != "src/file_0.rs" {
        anyhow::bail!("scale: deletion fixture must be repo0/src/file_0.rs");
    }
    let deleted_token = file_query_token(0, 0);
    let retained_token = file_query_token(1, 0);
    let deleted_before = rt.query_text(TextQuerySyntax::Native, &deleted_token, SCALE_TOP_K);
    require_single_source_file(&deleted_before, "repo0", &file.repo_relative_path)?;
    let retained_before = rt.query_text(TextQuerySyntax::Native, &retained_token, SCALE_TOP_K);
    require_single_source_file(&retained_before, "repo1", &file.repo_relative_path)?;

    let (delete_seal_ms, delete_resource) = observe_phase(|| {
        let delete_started = Instant::now();
        rt.delete_chunk_for_source_file("repo0", &file.repo_relative_path)
            .map_err(|error| ScaleStageError::operation("delete", error))?;
        let _generation = rt
            .seal()
            .map_err(|error| ScaleStageError::operation("delete_seal", error))?;
        Ok(elapsed_ms(delete_started))
    })?;
    let (delete_activation_ms, activation_resource) = observe_phase(|| {
        let activation_started = Instant::now();
        rt.activate_last_sealed_generation()
            .map_err(|error| ScaleStageError::operation("delete_activate", error))?;
        Ok(elapsed_ms(activation_started))
    })?;
    let successor = oracle.without_file("repo0", &file.repo_relative_path)?;

    let deleted_after = rt.query_text(TextQuerySyntax::Native, &deleted_token, SCALE_TOP_K);
    require_no_source_file(&deleted_after)?;
    let retained_after = rt.query_text(TextQuerySyntax::Native, &retained_token, SCALE_TOP_K);
    require_single_source_file(&retained_after, "repo1", &file.repo_relative_path)?;
    let global_after = rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K);
    let _count = validate_scoped_response(&successor, None, &global_after)?;
    verify_scoped_repositories(rt, &successor)?;

    let (same_process_reopen_ms, reopen_resource) = observe_phase(|| {
        let reopen_started = Instant::now();
        rt.try_reopen_in_place()
            .map_err(|error| ScaleStageError::operation("reopen_stop", error))?;
        rt.start()
            .map_err(|error| ScaleStageError::operation("reopen_start", error))?;
        Ok(elapsed_ms(reopen_started))
    })?;
    let first_query_started = Instant::now();
    let retained_reopened = rt.query_text(TextQuerySyntax::Native, &retained_token, SCALE_TOP_K);
    let reopened_first_query_ms = elapsed_ms(first_query_started);
    require_single_source_file(&retained_reopened, "repo1", &file.repo_relative_path)?;
    let deleted_reopened = rt.query_text(TextQuerySyntax::Native, &deleted_token, SCALE_TOP_K);
    require_no_source_file(&deleted_reopened)?;
    let global_reopened = rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K);
    let _count = validate_scoped_response(&successor, None, &global_reopened)?;
    verify_scoped_repositories(rt, &successor)?;
    let mut phase_resources = BTreeMap::new();
    record_phase(&mut phase_resources, "delete_seal", delete_resource)?;
    record_phase(&mut phase_resources, "delete_activate", activation_resource)?;
    record_phase(&mut phase_resources, "same_process_reopen", reopen_resource)?;
    Ok((
        DeleteReopenMeasurementV1 {
            delete_seal_ms,
            delete_activation_ms,
            same_process_reopen_ms,
            reopened_first_query_ms,
        },
        phase_resources,
    ))
}

/// Open the sealed generation the daemon serves through the lexical
/// adapter in-process and time open, plan and execute on their own.
fn measure_adapter_phases(
    rt: &E2eRuntime,
    scoped_oracle: Option<&ScopedOracle>,
) -> AnyResult<AdapterPhaseTimingV1> {
    let adapter = LexicalAdapter::with_state_root(rt.state_root().join("indexes/lexical"));
    let sealed = ManifestGeneration::new(rt.current_generation().get().saturating_sub(1));
    let open_started = Instant::now();
    let searcher = adapter.open(
        &rt.repo(),
        &rt.revision(),
        sealed,
        &RequestBudgetV1::unbounded(),
    )?;
    let open_ms = elapsed_ms(open_started);
    let request = scale_query();
    let mut plan_samples = Vec::with_capacity(WARM_QUERY_SAMPLES);
    let mut query = None;
    for _ in 0..WARM_QUERY_SAMPLES {
        let started = Instant::now();
        query = Some(lower_lexical_text_query(&request)?);
        plan_samples.push(elapsed_ms(started));
    }
    let Some(query) = query else {
        return Err(anyhow::anyhow!("scale: the plan loop produced no plan"));
    };
    let constraints = QueryConstraintSetV1::unconstrained();
    let mut execute_samples = Vec::with_capacity(WARM_QUERY_SAMPLES);
    for _ in 0..WARM_QUERY_SAMPLES {
        let started = Instant::now();
        let page = searcher.search_constrained(
            &query,
            &constraints,
            &LexicalPageSpec::first(SCALE_TOP_K),
            &RequestBudgetV1::unbounded(),
        )?;
        execute_samples.push(elapsed_ms(started));
        if let Some(oracle) = scoped_oracle {
            let _verified_count = oracle.verify_page(None, &page.candidates)?;
        } else if page.candidates.is_empty() {
            return Err(anyhow::anyhow!(
                "scale: the adapter-only query returned an empty page"
            ));
        }
    }
    Ok(AdapterPhaseTimingV1 {
        open_ms,
        plan_ms: median_ms(&mut plan_samples)?,
        execute_ms: median_ms(&mut execute_samples)?,
    })
}

/// Change one file, ingest and seal it as a delta, activate (reclaiming the
/// predecessor), and measure each step against the byte oracle.
fn measure_delta(
    rt: &mut E2eRuntime,
    seed: u64,
) -> AnyResult<(DeltaMeasurementV1, BTreeMap<&'static str, PhaseResourceV1>)> {
    let corpus = generate_corpus(ScaleTier::Small, seed);
    let Some((path, original)) = corpus.first() else {
        return Err(anyhow::anyhow!("scale: the corpus has no file to change"));
    };
    let changed = format!("{original}// delta {SCALE_QUERY_TOKEN} touched\n");
    let changed_bytes = u64::try_from(changed.len())?;
    let before_build = directory_bytes(rt.state_root())?;
    let serving_owner = rt.repo();
    let (update_ms, update_resource) = observe_phase(|| {
        let update_started = Instant::now();
        rt.ingest_text(serving_owner.as_str(), path, &changed)?;
        let _generation = rt
            .seal()
            .map_err(|error| ScaleStageError::operation("delta_seal", error))?;
        Ok(elapsed_ms(update_started))
    })?;
    let after_build = directory_bytes(rt.state_root())?;
    let (activation_with_reclaim_ms, activation_resource) = observe_phase(|| {
        let activation_started = Instant::now();
        rt.activate_last_sealed_generation()
            .map_err(|error| ScaleStageError::operation("delta_activate", error))?;
        Ok(elapsed_ms(activation_started))
    })?;
    let after_activation = directory_bytes(rt.state_root())?;
    let mut phase_resources = BTreeMap::new();
    record_phase(&mut phase_resources, "delta_ingest_seal", update_resource)?;
    record_phase(&mut phase_resources, "delta_activate", activation_resource)?;
    Ok((
        DeltaMeasurementV1 {
            update_ms,
            changed_bytes,
            bytes_written: after_build.saturating_sub(before_build),
            activation_with_reclaim_ms,
            reclaimed_bytes: after_build.saturating_sub(after_activation),
        },
        phase_resources,
    ))
}

/// Measure the SMALL tier phase by phase.
///
/// Boot, seed, seal, activate, query cold and warm, open the generation
/// through the adapter beside the daemon, then apply a one-file delta and
/// reclaim the predecessor — capturing each phase on its own.
///
/// Fail-closed: a typed error on any measured query is a rail error, never
/// a zero-latency pass over an empty result; a daemon histogram that does
/// not show the expected samples in a window is a rail error, never a
/// fabricated phase time.
pub fn measure_small_tier(seed: u64) -> AnyResult<TierMeasurement> {
    measure_small_tier_with_config(seed, ScaleRuntimeConfig::default())
}

fn measure_small_tier_with_config(
    seed: u64,
    config: ScaleRuntimeConfig,
) -> AnyResult<TierMeasurement> {
    let effective_timeout_ms = config.effective_timeout_ms()?;
    let effective_history_max_bytes = config.effective_history_max_bytes()?;
    let corpus = generate_corpus(ScaleTier::Small, seed);
    let corpus_digest = corpus_digest(DIMENSION, &corpus);
    let expected_results = expected_small_result_count(&corpus)
        .map_err(|error| stage_or_preserve("source_fixture", error))?;
    let file_count = corpus.len();
    let corpus_bytes = corpus
        .iter()
        .try_fold(0_u64, |total, (_, content)| -> AnyResult<u64> {
            Ok(total.saturating_add(u64::try_from(content.len())?))
        })
        .map_err(|error| stage_or_preserve("source_fixture", error))?;

    // The search-corpus history contract requires at least two generations.
    let cpu_started =
        CpuSnapshot::observe().map_err(|error| stage_or_preserve("resource_observation", error))?;
    let mut rt = scale_runtime(config).map_err(|error| stage_or_preserve("runtime_boot", error))?;
    let measurement = (|| -> AnyResult<TierMeasurement> {
        let model_revision = model_revision_of(rt.embedder_profile());

        let before_build = directory_bytes(rt.state_root())
            .map_err(|error| stage_or_preserve("build_io", error))?;
        let serving_owner = rt.repo();
        let (build_ms, build_resource) = observe_phase(|| {
            let build_started = Instant::now();
            for (path, content) in &corpus {
                rt.ingest_text(serving_owner.as_str(), path, content)
                    .map_err(|error| stage_or_preserve("build_ingest", error))?;
            }
            let _generation = rt
                .seal()
                .map_err(|error| ScaleStageError::operation("build_seal", error))?;
            Ok(elapsed_ms(build_started))
        })?;
        let build_bytes_written = directory_bytes(rt.state_root())
            .map_err(|error| stage_or_preserve("build_io", error))?
            .saturating_sub(before_build);

        let (activation_ms, activation_resource) = observe_phase(|| {
            let activation_started = Instant::now();
            rt.activate_last_sealed_generation()
                .map_err(|error| ScaleStageError::operation("build_activate", error))?;
            Ok(elapsed_ms(activation_started))
        })?;

        let scrape_before_first = rt
            .metrics_snapshot()
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let first_started = Instant::now();
        let result_count =
            served_query(&mut rt).map_err(|error| stage_or_preserve("query_first", error))?;
        let first_query_ms = elapsed_ms(first_started);
        require_result_count(result_count, expected_results, "first query")
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let scrape_after_first = rt
            .metrics_snapshot()
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let cold_open_ms = optional_cold_open_window(&scrape_before_first, &scrape_after_first)
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let first_route_ms = histogram_window(
            &scrape_before_first,
            &scrape_after_first,
            "lq_route_lexical_latency_ms",
            1,
        )
        .map_err(|error| stage_or_preserve("query_first", error))?;

        let warm_samples = collect_warm_samples(expected_results, WARM_QUERY_SAMPLES, || {
            served_query(&mut rt)
        })
        .map_err(|error| stage_or_preserve("query_warm", error))?;
        let scrape_after_warm = rt
            .metrics_snapshot()
            .map_err(|error| stage_or_preserve("query_warm", error))?;
        let warm_route_total_ms = histogram_window(
            &scrape_after_first,
            &scrape_after_warm,
            "lq_route_lexical_latency_ms",
            u64::try_from(WARM_QUERY_SAMPLES)?,
        )
        .map_err(|error| stage_or_preserve("query_warm", error))?;
        let warm_query = LatencySummary::from_samples_ms(&warm_samples)
            .ok_or_else(|| anyhow::anyhow!("scale: no warm samples"))?;

        let adapter = measure_adapter_phases(&rt, None)
            .map_err(|error| stage_or_preserve("adapter", error))?;
        let (delta, delta_resources) =
            measure_delta(&mut rt, seed).map_err(|error| stage_or_preserve("delta", error))?;
        let mut phase_resources = delta_resources;
        record_phase(&mut phase_resources, "full_ingest_seal", build_resource)?;
        record_phase(&mut phase_resources, "full_activate", activation_resource)?;

        Ok(TierMeasurement {
            tier: ScaleTier::Small,
            seed,
            file_count,
            corpus_digest,
            source_repo_count: 1,
            corpus_bytes,
            ingest_decoded_bytes: None,
            ingest_wire_bytes: None,
            build_ms,
            build_bytes_written,
            activation_ms,
            first_query_ms,
            warm_query,
            daemon: DaemonPhaseTimingV1 {
                cold_open_ms,
                first_route_ms,
                warm_route_mean_ms: warm_route_total_ms
                    / f64::from(u32::try_from(WARM_QUERY_SAMPLES)?),
            },
            adapter,
            delta,
            result_count,
            model_revision,
            client_request_timeout_ms: effective_timeout_ms,
            requested_client_request_timeout_ms: config
                .client_timeout
                .map(|_| effective_timeout_ms),
            history_max_bytes: effective_history_max_bytes,
            requested_history_max_bytes: config.history_max_bytes,
            cpu: None,
            phase_resources,
            delete_reopen: None,
        })
    })()
    .map_err(|error| stage_or_preserve("measurement", error));
    let cleanup = rt
        .stop()
        .map_err(|error| stage_or_preserve("cleanup", error));
    let mut measurement = finish_runtime_measurement(measurement, cleanup)?;
    measurement.cpu = Some(
        CpuSnapshot::observe()
            .and_then(|snapshot| snapshot.elapsed_since(cpu_started))
            .map_err(|error| stage_or_preserve("resource_observation", error))?,
    );
    Ok(measurement)
}

fn measure_scoped_delta(
    rt: &mut E2eRuntime,
    file: &ScopedFile,
) -> AnyResult<(DeltaMeasurementV1, BTreeMap<&'static str, PhaseResourceV1>)> {
    let changed = format!("{}// delta {SCALE_QUERY_TOKEN} touched\n", file.content);
    let changed_bytes = u64::try_from(changed.len())?;
    let before_build = directory_bytes(rt.state_root())?;
    let serving_owner = rt.repo();
    let (update_ms, update_resource) = observe_phase(|| {
        let update_started = Instant::now();
        let _ids = rt.ingest_text_chunks(
            serving_owner.as_str(),
            &file.repo_relative_path,
            &[E2eTextChunkSpec {
                content: &changed,
                start_line: 1,
                end_line: 2,
                source_repo_id: Some(&file.source_repo_id),
            }],
        )?;
        let _generation = rt
            .seal()
            .map_err(|error| ScaleStageError::operation("delta_seal", error))?;
        Ok(elapsed_ms(update_started))
    })?;
    let after_build = directory_bytes(rt.state_root())?;
    let (activation_with_reclaim_ms, activation_resource) = observe_phase(|| {
        let activation_started = Instant::now();
        rt.activate_last_sealed_generation()
            .map_err(|error| ScaleStageError::operation("delta_activate", error))?;
        Ok(elapsed_ms(activation_started))
    })?;
    let after_activation = directory_bytes(rt.state_root())?;
    let mut phase_resources = BTreeMap::new();
    record_phase(&mut phase_resources, "delta_ingest_seal", update_resource)?;
    record_phase(&mut phase_resources, "delta_activate", activation_resource)?;
    Ok((
        DeltaMeasurementV1 {
            update_ms,
            changed_bytes,
            bytes_written: after_build.saturating_sub(before_build),
            activation_with_reclaim_ms,
            reclaimed_bytes: after_build.saturating_sub(after_activation),
        },
        phase_resources,
    ))
}

/// Measure one declared scale tier.
///
/// Small retains its existing one-repository fixture; larger tiers publish
/// distinct source-repository identities into one serving owner and
/// independently probe every source repository.
pub fn measure_tier(tier: ScaleTier, seed: u64) -> AnyResult<TierMeasurement> {
    measure_tier_with_runtime_config(tier, seed, ScaleRuntimeConfig::default())
}

pub fn measure_tier_with_client_timeout(
    tier: ScaleTier,
    seed: u64,
    client_timeout: Option<Duration>,
) -> AnyResult<TierMeasurement> {
    measure_tier_with_runtime_config(
        tier,
        seed,
        ScaleRuntimeConfig {
            client_timeout,
            history_max_bytes: None,
        },
    )
}

pub fn measure_tier_with_runtime_config(
    tier: ScaleTier,
    seed: u64,
    config: ScaleRuntimeConfig,
) -> AnyResult<TierMeasurement> {
    let effective_timeout_ms = config.effective_timeout_ms()?;
    let effective_history_max_bytes = config.effective_history_max_bytes()?;
    if tier == ScaleTier::Small {
        return measure_small_tier_with_config(seed, config);
    }
    let files = generate_scoped_corpus(tier, seed)
        .map_err(|error| stage_or_preserve("source_fixture", error))?;
    let corpus_digest = scoped_corpus_digest(DIMENSION, &files);
    let oracle = ScopedOracle::from_source(&files, tier)
        .map_err(|error| stage_or_preserve("source_fixture", error))?;
    let _admission = preflight_scoped_corpus(&files).map_err(ScaleStageError::source_admission)?;
    let file_count = files.len();
    let corpus_bytes = files
        .iter()
        .try_fold(0_u64, |total, file| -> AnyResult<u64> {
            total
                .checked_add(u64::try_from(file.content.len())?)
                .ok_or_else(|| anyhow::anyhow!("scale: corpus byte count overflow"))
        })
        .map_err(|error| stage_or_preserve("source_fixture", error))?;

    let cpu_started =
        CpuSnapshot::observe().map_err(|error| stage_or_preserve("resource_observation", error))?;
    let mut rt = scale_runtime(config).map_err(|error| stage_or_preserve("runtime_boot", error))?;
    let measurement = (|| -> AnyResult<TierMeasurement> {
        let model_revision = model_revision_of(rt.embedder_profile());
        let before_build = directory_bytes(rt.state_root())
            .map_err(|error| stage_or_preserve("build_io", error))?;
        let chunks = files
            .iter()
            .map(|file| {
                [E2eTextChunkSpec {
                    content: &file.content,
                    start_line: 1,
                    end_line: 2,
                    source_repo_id: Some(&file.source_repo_id),
                }]
            })
            .collect::<Vec<_>>();
        let batch_files = files
            .iter()
            .zip(&chunks)
            .map(|(file, chunk)| (file.repo_relative_path.as_str(), chunk.as_slice()))
            .collect::<Vec<_>>();
        let (ingest_ms, ingest_resource) = observe_phase(|| {
            let ingest_started = Instant::now();
            let _ids = rt
                .ingest_text_files_one_batch(&batch_files)
                .map_err(|error| stage_or_preserve("build_ingest", error))?;
            Ok(elapsed_ms(ingest_started))
        })?;
        let (ingest_decoded_bytes, ingest_wire_bytes) = rt
            .preview_pending_search_corpus_wire_bytes()
            .map_err(ScaleStageError::wire_admission)?;
        let (seal_ms, seal_resource) = observe_phase(|| {
            let seal_started = Instant::now();
            let _generation = rt
                .seal()
                .map_err(|error| ScaleStageError::operation("build_seal", error))?;
            Ok(elapsed_ms(seal_started))
        })?;
        let build_ms = ingest_ms + seal_ms;
        let build_bytes_written = directory_bytes(rt.state_root())
            .map_err(|error| stage_or_preserve("build_io", error))?
            .saturating_sub(before_build);
        let (activation_ms, activation_resource) = observe_phase(|| {
            let activation_started = Instant::now();
            rt.activate_last_sealed_generation()
                .map_err(|error| ScaleStageError::operation("build_activate", error))?;
            Ok(elapsed_ms(activation_started))
        })?;

        let scrape_before_first = rt
            .metrics_snapshot()
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let first_started = Instant::now();
        let first = rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K);
        let first_query_ms = elapsed_ms(first_started);
        let result_count = validate_scoped_response(&oracle, None, &first)
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let scrape_after_first = rt
            .metrics_snapshot()
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let cold_open_ms = optional_cold_open_window(&scrape_before_first, &scrape_after_first)
            .map_err(|error| stage_or_preserve("query_first", error))?;
        let first_route_ms = histogram_window(
            &scrape_before_first,
            &scrape_after_first,
            "lq_route_lexical_latency_ms",
            1,
        )
        .map_err(|error| stage_or_preserve("query_first", error))?;
        let warm_samples = collect_validated_samples(
            WARM_QUERY_SAMPLES,
            || rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K),
            |response| validate_scoped_response(&oracle, None, response).map(|_| ()),
        )
        .map_err(|error| stage_or_preserve("query_warm", error))?;
        let scrape_after_warm = rt
            .metrics_snapshot()
            .map_err(|error| stage_or_preserve("query_warm", error))?;
        let warm_route_total_ms = histogram_window(
            &scrape_after_first,
            &scrape_after_warm,
            "lq_route_lexical_latency_ms",
            u64::try_from(WARM_QUERY_SAMPLES)?,
        )
        .map_err(|error| stage_or_preserve("query_warm", error))?;
        let warm_query = LatencySummary::from_samples_ms(&warm_samples)
            .ok_or_else(|| anyhow::anyhow!("scale: no warm samples"))?;

        // The global top-10 can be dominated by one source repo. These independent
        // probes prove that every declared repo's source files reached the index.
        verify_scoped_repositories(&mut rt, &oracle)
            .map_err(|error| stage_or_preserve("repo_probe", error))?;

        let adapter = measure_adapter_phases(&rt, Some(&oracle))
            .map_err(|error| stage_or_preserve("adapter", error))?;
        let delta_file = files
            .first()
            .ok_or_else(|| anyhow::anyhow!("scale: scoped corpus has no file to change"))?;
        let (delta, delta_resources) = measure_scoped_delta(&mut rt, delta_file)
            .map_err(|error| stage_or_preserve("delta", error))?;
        let after_delta = rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K);
        let _delta_result_count = validate_scoped_response(&oracle, None, &after_delta)
            .map_err(|error| stage_or_preserve("delta_verify", error))?;
        verify_scoped_repositories(&mut rt, &oracle)
            .map_err(|error| stage_or_preserve("delta_verify", error))?;
        let (delete_reopen, delete_resources) =
            measure_scoped_delete_reopen(&mut rt, &oracle, delta_file)
                .map_err(|error| stage_or_preserve("delete_reopen", error))?;
        let mut phase_resources = delta_resources;
        for (name, observation) in delete_resources {
            record_phase(&mut phase_resources, name, observation)?;
        }
        record_phase(&mut phase_resources, "full_ingest", ingest_resource)?;
        record_phase(&mut phase_resources, "full_seal", seal_resource)?;
        record_phase(&mut phase_resources, "full_activate", activation_resource)?;
        Ok(TierMeasurement {
            tier,
            seed,
            file_count,
            corpus_digest,
            source_repo_count: oracle.paths_by_repo.len(),
            corpus_bytes,
            ingest_decoded_bytes: Some(ingest_decoded_bytes),
            ingest_wire_bytes: Some(ingest_wire_bytes),
            build_ms,
            build_bytes_written,
            activation_ms,
            first_query_ms,
            warm_query,
            daemon: DaemonPhaseTimingV1 {
                cold_open_ms,
                first_route_ms,
                warm_route_mean_ms: warm_route_total_ms
                    / f64::from(u32::try_from(WARM_QUERY_SAMPLES)?),
            },
            adapter,
            delta,
            result_count,
            model_revision,
            client_request_timeout_ms: effective_timeout_ms,
            requested_client_request_timeout_ms: config
                .client_timeout
                .map(|_| effective_timeout_ms),
            history_max_bytes: effective_history_max_bytes,
            requested_history_max_bytes: config.history_max_bytes,
            cpu: None,
            phase_resources,
            delete_reopen: Some(delete_reopen),
        })
    })()
    .map_err(|error| stage_or_preserve("measurement", error));
    let cleanup = rt
        .stop()
        .map_err(|error| stage_or_preserve("cleanup", error));
    let mut measurement = finish_runtime_measurement(measurement, cleanup)?;
    measurement.cpu = Some(
        CpuSnapshot::observe()
            .and_then(|snapshot| snapshot.elapsed_since(cpu_started))
            .map_err(|error| stage_or_preserve("resource_observation", error))?,
    );
    Ok(measurement)
}

/// Source identity for a refusal, recomputed from the deterministic generator
/// only after a failed run. It is never substituted for a successful timing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScaleSourceBinding {
    pub tier: ScaleTier,
    pub seed: u64,
    pub serving_owner_count: u32,
    pub source_repo_count: u32,
    pub file_count: usize,
    pub corpus_bytes: u64,
    pub source_bytes: u64,
    pub corpus_digest: String,
}

pub fn source_binding_for_failure(tier: ScaleTier, seed: u64) -> AnyResult<ScaleSourceBinding> {
    source_binding_for_failure_in_dimension(DIMENSION, tier, seed)
}

/// Reuse the scale fixture identity for another rail without reusing its
/// domain-separated digest as if it were the same measurement.
pub fn source_binding_for_failure_in_dimension(
    dimension: &str,
    tier: ScaleTier,
    seed: u64,
) -> AnyResult<ScaleSourceBinding> {
    let params = params_for(tier);
    let (corpus_digest, file_count, corpus_bytes) = if tier == ScaleTier::Small {
        let corpus = generate_corpus(tier, seed);
        let bytes = corpus.iter().try_fold(0_u64, |total, (_, content)| {
            total
                .checked_add(u64::try_from(content.len())?)
                .ok_or_else(|| anyhow::anyhow!("scale: corpus byte count overflow"))
        })?;
        (corpus_digest(dimension, &corpus), corpus.len(), bytes)
    } else {
        let files = generate_scoped_corpus(tier, seed)?;
        let bytes = files.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(u64::try_from(file.content.len())?)
                .ok_or_else(|| anyhow::anyhow!("scale: corpus byte count overflow"))
        })?;
        (scoped_corpus_digest(dimension, &files), files.len(), bytes)
    };
    let source_bytes = corpus_bytes
        .checked_add(u64::try_from(file_count)?)
        .ok_or_else(|| anyhow::anyhow!("scale: source byte count overflow"))?;
    Ok(ScaleSourceBinding {
        tier,
        seed,
        serving_owner_count: 1,
        source_repo_count: params.repo_count,
        file_count,
        corpus_bytes,
        source_bytes,
        corpus_digest,
    })
}

pub fn refusal_json(
    binding: &ScaleSourceBinding,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
) -> Value {
    refusal_json_with_context(binding, git_head, host, error, DIMENSION, None)
}

/// Common failure record for rails sharing the scale source fixture. The
/// execution context names the request schedule without changing scale's
/// existing refusal schema when it is absent.
pub fn refusal_json_with_context(
    binding: &ScaleSourceBinding,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
    dimension: &str,
    execution: Option<&Value>,
) -> Value {
    let runtime_failure = error.downcast_ref::<ScaleRuntimeFailure>();
    let mut primary = error;
    let mut secondary_failures = Vec::new();
    while let Some(failure) = primary.downcast_ref::<ScaleRuntimeFailure>() {
        secondary_failures.push(json!({
            "context": failure.cleanup_context,
            "message": format!("{:#}", failure.cleanup),
        }));
        primary = failure.primary.as_ref().unwrap_or(&failure.cleanup);
    }
    let stage = primary.downcast_ref::<ScaleStageError>();
    let mut value = json!({
        "kind": format!("quanta-index-{dimension}-failure"),
        "schema_version": 1,
        "status": if runtime_failure.is_none() && stage.and_then(|failure| failure.limit).is_some() { "refused" } else { "failed" },
        "source": {
            "tier": binding.tier.as_str(),
            "seed": binding.seed,
            "serving_owner_count": binding.serving_owner_count,
            "source_repo_count": binding.source_repo_count,
            "file_count": binding.file_count,
            "corpus_bytes": binding.corpus_bytes,
            "source_bytes": binding.source_bytes,
            "corpus_digest": binding.corpus_digest,
            "corpus_digest_kind": if binding.tier == ScaleTier::Small {
                "generated_chunk_content_prefixed_path_v1"
            } else {
                "generated_chunk_content_source_repo_path_v1"
            },
        },
        "provenance": {
            "git_head": git_head.as_str(),
            "host": host,
        },
        "failure": {
            "stage": stage.map(|failure| failure.stage).unwrap_or("execution_unclassified"),
            "limit": stage.and_then(|failure| failure.limit).map(ScaleAdmissionLimit::as_str),
            "observed": stage.and_then(|failure| failure.observed),
            "maximum": stage.and_then(|failure| failure.maximum),
            "message": format!("{primary:#}"),
        },
    });
    if let Some(runtime_failure) = runtime_failure {
        value["failure"]["primary"] = runtime_failure
            .primary
            .as_ref()
            .map(|error| json!({ "message": format!("{error:#}") }))
            .unwrap_or(Value::Null);
        value["failure"]["cleanup"] = json!({
            "context": runtime_failure.cleanup_context,
            "message": format!("{:#}", runtime_failure.cleanup),
        });
        value["failure"]["secondary_failures"] = json!(secondary_failures);
    }
    if let Some(execution) = execution {
        value["execution"] = execution.clone();
    }
    value
}

pub fn write_refusal_artifact(
    binding: &ScaleSourceBinding,
    dir: &Path,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
) -> AnyResult<()> {
    write_refusal_artifact_with_context(binding, dir, git_head, host, error, DIMENSION, None)
}

pub fn write_refusal_artifact_with_context(
    binding: &ScaleSourceBinding,
    dir: &Path,
    git_head: &GitHeadV1,
    host: &HostV1,
    error: &anyhow::Error,
    dimension: &str,
    execution: Option<&Value>,
) -> AnyResult<()> {
    crate::artifact::write_json_pretty_noclobber(
        &dir.join("refusal.json"),
        &refusal_json_with_context(binding, git_head, host, error, dimension, execution),
    )
}

// ---------------------------------------------------------------------------
// Artifact emission (BenchArtifactV1; the tier manifest is a plain record).
// ---------------------------------------------------------------------------

fn tier_params_json(params: &TierParams) -> Value {
    json!({
        "tier": params.tier.as_str(),
        "repo_count": params.repo_count,
        "files_per_repo": params.files_per_repo,
        "avg_file_lines": params.avg_file_lines,
        "hit_density_per_mille": params.hit_density_per_mille,
        "symbol_density_per_mille": params.symbol_density_per_mille,
        "total_files": params.total_files(),
        "default_run": params.tier.is_default_tier(),
        "selectable": true,
        "serving_owner_count": 1,
        "source_repo_count": params.repo_count,
        "qualification_owner": "canonical-quiet-host-run",
    })
}

/// The checked-in tier manifest, serialized.
#[must_use]
pub fn tier_manifest_json() -> Value {
    json!({
        "kind": "quanta-index-scale-tier-manifest",
        "manifest_schema_version": 2,
        "dimension": "scale",
        "query_token": SCALE_QUERY_TOKEN,
        "tiers": TIER_MANIFEST.iter().map(tier_params_json).collect::<Vec<_>>(),
    })
}

/// The measured tier as the artifact's detail: every phase on its own,
/// named for what measured it.
fn measurement_json(measurement: &TierMeasurement) -> Value {
    let phase_resources = measurement
        .phase_resources
        .iter()
        .map(|(phase, observation)| {
            (*phase, json!({
                "cpu_process_user_ms": observation.cpu.user_ms,
                "cpu_process_system_ms": observation.cpu.system_ms,
                "rss_start_bytes": observation.rss_start_bytes,
                "rss_end_bytes": observation.rss_end_bytes,
                "sampled_max_rss_bytes": observation.sampled_max_rss_bytes,
                "sampled_max_is_true_peak": false,
                "rss_method": if cfg!(target_os = "linux") { "proc_self_status_vmrss" } else { "ps_rss_kib_self" },
                "sample_interval_ms": PHASE_RSS_INTERVAL.as_millis(),
                "maximum_allowed_gap_ms": PHASE_RSS_MAX_GAP.as_millis(),
                "interior_required_after_ms": PHASE_RSS_INTERIOR_REQUIRED_AFTER.as_millis(),
                "observed_max_gap_ms": observation.observed_max_gap_ms,
                "interior_samples": observation.interior_samples,
                "discarded_outside_phase_samples": observation.discarded_outside_phase_samples,
                "coverage": if observation.interior_samples == 0 { "boundaries_only" } else { "periodic_with_boundaries" },
                "observation_span_ms": observation.observation_span_ms,
                "observer_setup_ms": observation.observer_setup_ms,
                "observer_teardown_ms": observation.observer_teardown_ms,
                "observer_periodic_probe_wall_ms": observation.observer_periodic_probe_wall_ms,
                "scope": "same process harness plus in-process daemon; observation envelope surrounds the matching wall-timed operation",
            }))
        })
        .collect::<BTreeMap<_, _>>();
    json!({
        "tier": measurement.tier.as_str(),
        "seed": measurement.seed,
        "client_request_timeout_ms": measurement.client_request_timeout_ms,
        "requested_client_request_timeout_ms": measurement.requested_client_request_timeout_ms,
        "history_max_generations": 2,
        "history_max_bytes": measurement.history_max_bytes,
        "requested_history_max_bytes": measurement.requested_history_max_bytes,
        "history_max_revision_pairs": HARNESS_HISTORY_MAX_REVISION_PAIRS,
        "history_max_total_bytes": HARNESS_HISTORY_MAX_TOTAL_BYTES,
        "cpu_process": {
            "scope": "RUSAGE_SELF, harness and in-process daemon, runtime boot through cleanup",
            "user_ms": measurement.cpu.map(|cpu| cpu.user_ms),
            "system_ms": measurement.cpu.map(|cpu| cpu.system_ms),
        },
        "phase_resources": phase_resources,
        "phase_resources_method": "RUSAGE_SELF CPU deltas for harness plus in-process daemon; sampled current RSS; macOS ps probe child CPU is excluded; observer setup and teardown are outside operation wall timers; physical write I/O is not measured",
        "file_count": measurement.file_count,
        "serving_owner_count": 1,
        "source_repo_count": measurement.source_repo_count,
        "corpus_bytes": measurement.corpus_bytes,
        "ingest_envelope_bytes": {
            "decoded": measurement.ingest_decoded_bytes,
            "wire": measurement.ingest_wire_bytes,
        },
        "status": "measured",
        "disk_measurement": "logical_directory_size_delta; hard links may be counted more than once; not physical write I/O",
        "build": {
            "build_ms": measurement.build_ms,
            "bytes_written": measurement.build_bytes_written,
            "timer_excludes_wire_preflight": measurement.ingest_wire_bytes.is_some(),
        },
        "activation_ms": measurement.activation_ms,
        "wall_across_socket": {
            "first_query_ms": measurement.first_query_ms,
            "warm_query": measurement.warm_query,
        },
        "daemon_phases_ms": {
            "source": "daemon metrics scrape deltas (whole-millisecond histograms)",
            "cold_open_ms": measurement.daemon.cold_open_ms,
            "first_route_ms": measurement.daemon.first_route_ms,
            "warm_route_mean_ms": measurement.daemon.warm_route_mean_ms,
        },
        "adapter_only_phases_ms": {
            "source": "the sealed generation opened in-process through the lexical adapter",
            "open_ms": measurement.adapter.open_ms,
            "plan_ms": measurement.adapter.plan_ms,
            "execute_ms": measurement.adapter.execute_ms,
        },
        "delta": {
            "update_ms": measurement.delta.update_ms,
            "changed_bytes": measurement.delta.changed_bytes,
            "bytes_written": measurement.delta.bytes_written,
            "activation_with_reclaim_ms": measurement.delta.activation_with_reclaim_ms,
            "reclaimed_bytes": measurement.delta.reclaimed_bytes,
        },
        "delete_reopen": measurement.delete_reopen.map(|timing| json!({
            "delete_seal_ms": timing.delete_seal_ms,
            "delete_activation_ms": timing.delete_activation_ms,
            "same_process_reopen_ms": timing.same_process_reopen_ms,
            "reopened_first_query_ms": timing.reopened_first_query_ms,
            "scope": "same OS process and state root; daemon thread restarted; page cache not cleared",
        })),
        "result_count": measurement.result_count,
    })
}

/// Advisory row for a tier that is declared but not measured here.
fn declared_advisory_json(params: &TierParams) -> Value {
    json!({
        "tier": params.tier.as_str(),
        "total_files": params.total_files(),
        "status": "declared-advisory",
        "note": "selectable tier; not measured in this artifact",
    })
}

/// The scale artifact's detail: the one measured tier plus the advisory
/// declared tiers.
#[must_use]
pub fn detail_json(measurement: &TierMeasurement) -> Value {
    let advisory: Vec<Value> = TIER_MANIFEST
        .iter()
        .filter(|p| p.tier != measurement.tier)
        .map(declared_advisory_json)
        .collect();
    json!({
        // Rail pass condition: the measured small-tier query retrieved the planted
        // token (an empty ordering is a rail error upstream, never written here).
        // Surfaced as a top-level flag so the J7Q-08 integration summary can treat
        // scale as a live dimension without re-deriving the verdict.
        "passed": measurement.result_count > 0,
        "measured_tiers": [measurement_json(measurement)],
        "declared_advisory_tiers": advisory,
        "blocking_note": "this artifact covers one selected tier; performance qualification requires a canonical quiet-host run",
    })
}

/// The one artifact row: the warm scale query across the socket.
fn warm_query_row(measurement: &TierMeasurement) -> BenchRowV1 {
    BenchRowV1 {
        scenario_id: format!("scale.{}.warm_query", measurement.tier.as_str()),
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        result_shape: ResultShape::Candidates,
        latency: Some(measurement.warm_query),
        qps: None,
        error_count: 0,
        timeout_count: 0,
        result_count: Some(saturating_u64(measurement.result_count)),
        typed_error_code: None,
        engine_touched: vec!["Lexical".to_string()],
        early_stop_reason: None,
    }
}

/// The scale artifact: one `BenchArtifactV1` for the measured tier.
///
/// Its provenance names the head, the generated corpus (by its exact
/// bytes) and the tier parameters; its phases are the build, the one-file
/// delta and the reclaim; its disk amplification is the full build's.
pub fn artifact(
    measurement: &TierMeasurement,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    if measurement.cpu.is_none() {
        anyhow::bail!("scale: measured tier has no process CPU observation");
    }
    let expected_phases: &[&str] = if measurement.tier == ScaleTier::Small {
        &[
            "full_ingest_seal",
            "full_activate",
            "delta_ingest_seal",
            "delta_activate",
        ]
    } else {
        &[
            "full_ingest",
            "full_seal",
            "full_activate",
            "delta_ingest_seal",
            "delta_activate",
            "delete_seal",
            "delete_activate",
            "same_process_reopen",
        ]
    };
    let observed_phases = measurement
        .phase_resources
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let expected_phases = expected_phases.iter().copied().collect::<BTreeSet<_>>();
    if observed_phases != expected_phases {
        anyhow::bail!("scale: measured tier is missing a required phase resource observation");
    }
    for (phase, observation) in &measurement.phase_resources {
        if observation.rss_start_bytes == 0
            || observation.rss_end_bytes == 0
            || observation.sampled_max_rss_bytes
                < observation.rss_start_bytes.max(observation.rss_end_bytes)
            || (observation.observation_span_ms
                >= PHASE_RSS_INTERIOR_REQUIRED_AFTER.as_secs_f64() * 1_000.0
                && observation.interior_samples == 0)
            || ![
                observation.cpu.user_ms,
                observation.cpu.system_ms,
                observation.observed_max_gap_ms,
                observation.observation_span_ms,
                observation.observer_setup_ms,
                observation.observer_teardown_ms,
                observation.observer_periodic_probe_wall_ms,
            ]
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
            || observation.observation_span_ms == 0.0
            || observation.observed_max_gap_ms > PHASE_RSS_MAX_GAP.as_secs_f64() * 1_000.0
        {
            anyhow::bail!("scale: phase {phase} has an invalid resource observation");
        }
    }
    let params = params_for(measurement.tier);
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Cold,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: measurement.corpus_digest.clone(),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("tier", params.tier.as_str().to_string()),
                    ("seed", measurement.seed.to_string()),
                    ("repo_count", params.repo_count.to_string()),
                    ("serving_owner_count", "1".to_string()),
                    (
                        "source_identity",
                        if measurement.tier == ScaleTier::Small {
                            "legacy-prefixed-path"
                        } else {
                            "scoped-source-repo-v1"
                        }
                        .to_string(),
                    ),
                    ("files_per_repo", params.files_per_repo.to_string()),
                    ("avg_file_lines", params.avg_file_lines.to_string()),
                    (
                        "hit_density_per_mille",
                        params.hit_density_per_mille.to_string(),
                    ),
                    (
                        "symbol_density_per_mille",
                        params.symbol_density_per_mille.to_string(),
                    ),
                    ("top_k", SCALE_TOP_K.to_string()),
                    ("warm_query_samples", WARM_QUERY_SAMPLES.to_string()),
                    (
                        "phase_rss_interval_ms",
                        PHASE_RSS_INTERVAL.as_millis().to_string(),
                    ),
                    (
                        "phase_rss_max_gap_ms",
                        PHASE_RSS_MAX_GAP.as_millis().to_string(),
                    ),
                    (
                        "phase_rss_method",
                        if cfg!(target_os = "linux") {
                            "proc_self_status_vmrss"
                        } else {
                            "ps_rss_kib_self"
                        }
                        .to_string(),
                    ),
                    ("history_max_generations", "2".to_string()),
                    (
                        "history_max_revision_pairs",
                        HARNESS_HISTORY_MAX_REVISION_PAIRS.to_string(),
                    ),
                    (
                        "history_max_total_bytes",
                        HARNESS_HISTORY_MAX_TOTAL_BYTES.to_string(),
                    ),
                    (
                        "history_max_bytes",
                        measurement.history_max_bytes.to_string(),
                    ),
                    (
                        "requested_history_max_bytes",
                        format!("{:?}", measurement.requested_history_max_bytes),
                    ),
                    (
                        "client_request_timeout_ms",
                        measurement.client_request_timeout_ms.to_string(),
                    ),
                    (
                        "requested_client_request_timeout_ms",
                        format!("{:?}", measurement.requested_client_request_timeout_ms),
                    ),
                ],
            ),
            model_revision: measurement.model_revision.clone(),
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1 {
            build_ms: Some(measurement.build_ms),
            update_ms: Some(measurement.delta.update_ms),
            // Activation is only a GC upper bound when physical bytes shrink.
            gc_ms: (measurement.delta.reclaimed_bytes > 0)
                .then_some(measurement.delta.activation_with_reclaim_ms),
        },
        disk_amplification: Some(DiskAmplificationV1 {
            bytes_written: measurement.build_bytes_written,
            changed_bytes: measurement.corpus_bytes,
        }),
        rows: vec![warm_query_row(measurement)],
        detail: detail_json(measurement),
    })
}

/// Write the two canonical scale artifacts under `dir`:
/// `tier_manifest.json` and `summary.json` (the `BenchArtifactV1`).
pub fn write_artifacts(
    measurement: &TierMeasurement,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("tier_manifest.json"), &tier_manifest_json())?;
    artifact(measurement, git_head, host)?.write_to(&dir.join("summary.json"))?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index into JSON values and slices whose shape this module constructs and asserts directly; an out-of-range index is a legitimate test failure"
)]
mod tests {
    //! Generator, artifact and narrow runtime behavior.
    use super::*;

    #[test]
    fn activated_snapshot_without_query_cold_open_is_unavailable() -> AnyResult<()> {
        use quanta_index_contract::MetricHistogramV1;

        let before = MetricsSnapshotV1::default();
        assert_eq!(optional_cold_open_window(&before, &before)?, None);
        let mut after = before.clone();
        after.histograms.push(MetricHistogramV1 {
            name: "lq_snapshot_lexical_cold_open_ms".to_string(),
            count: 1,
            sum: 3.0,
            min: 3.0,
            max: 3.0,
            buckets: Vec::new(),
        });
        assert_eq!(optional_cold_open_window(&before, &after)?, Some(3.0));
        after.histograms[0].count = 2;
        assert!(optional_cold_open_window(&before, &after).is_err());
        Ok(())
    }

    #[test]
    fn client_timeout_contract_requires_bounded_whole_milliseconds() -> AnyResult<()> {
        assert_eq!(timeout_ms(None)?, 30_000);
        assert_eq!(timeout_ms(Some(Duration::from_secs(300)))?, 300_000);
        for invalid in [
            Duration::ZERO,
            Duration::from_micros(1),
            Duration::from_micros(1_001),
            Duration::from_millis(600_001),
        ] {
            assert!(timeout_ms(Some(invalid)).is_err());
        }
        Ok(())
    }

    #[test]
    fn runtime_config_records_requested_and_effective_policy() -> AnyResult<()> {
        let default = ScaleRuntimeConfig::default().execution_json()?;
        assert_eq!(default["client_request_timeout_ms"], 30_000);
        assert!(default["requested_client_request_timeout_ms"].is_null());
        assert_eq!(default["history_max_bytes"], 16_777_216);
        assert!(default["requested_history_max_bytes"].is_null());
        assert_eq!(default["history_max_total_bytes"], 268_435_456);
        assert_eq!(default["history_max_revision_pairs"], 128);

        let explicit = ScaleRuntimeConfig {
            client_timeout: Some(Duration::from_secs(300)),
            history_max_bytes: Some(268_435_456),
        };
        let execution = explicit.execution_json()?;
        assert_eq!(execution["client_request_timeout_ms"], 300_000);
        assert_eq!(execution["requested_client_request_timeout_ms"], 300_000);
        assert_eq!(execution["history_max_bytes"], 268_435_456);
        assert_eq!(execution["requested_history_max_bytes"], 268_435_456);
        for invalid in [0, HARNESS_HISTORY_MAX_TOTAL_BYTES + 1] {
            assert!(
                ScaleRuntimeConfig {
                    history_max_bytes: Some(invalid),
                    ..explicit
                }
                .execution_json()
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn explicit_history_budget_reaches_the_daemon_seal() -> AnyResult<()> {
        let mut runtime = scale_runtime(ScaleRuntimeConfig {
            client_timeout: None,
            history_max_bytes: Some(1),
        })?;
        let owner = runtime.repo();
        runtime.ingest_text(owner.as_str(), "source.rs", "fn budget_fixture() {}\n")?;
        let error = runtime
            .seal()
            .expect_err("one byte cannot retain a lexical generation");
        let message = format!("{error:#}");
        runtime.stop()?;
        assert!(
            message.contains("SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn manifest_has_one_row_per_tier() {
        for tier in [
            ScaleTier::Small,
            ScaleTier::Medium,
            ScaleTier::Large,
            ScaleTier::Xlarge,
        ] {
            let params = params_for(tier);
            assert_eq!(params.tier, tier);
            assert!(
                params.repo_count >= 1,
                "tier {} has no repos",
                tier.as_str()
            );
            assert!(
                params.files_per_repo >= 1,
                "tier {} has no files",
                tier.as_str()
            );
            assert!(
                params.avg_file_lines >= 1,
                "tier {} has zero lines",
                tier.as_str()
            );
        }
        assert_eq!(TIER_MANIFEST.len(), 4, "exactly four declared tiers");
    }

    #[test]
    fn tiers_grow_monotonically_in_total_files() {
        let small = params_for(ScaleTier::Small).total_files();
        let medium = params_for(ScaleTier::Medium).total_files();
        let large = params_for(ScaleTier::Large).total_files();
        let xlarge = params_for(ScaleTier::Xlarge).total_files();
        assert!(small < medium, "small {small} !< medium {medium}");
        assert!(medium < large, "medium {medium} !< large {large}");
        assert!(large < xlarge, "large {large} !< xlarge {xlarge}");
    }

    #[test]
    fn same_seed_same_tier_is_byte_identical() {
        let a = generate_corpus(ScaleTier::Small, 42);
        let b = generate_corpus(ScaleTier::Small, 42);
        assert_eq!(a, b, "same (tier, seed) must reproduce byte-identically");
    }

    #[test]
    fn different_seed_diverges() {
        let a = generate_corpus(ScaleTier::Small, 42);
        let b = generate_corpus(ScaleTier::Small, 43);
        assert_eq!(a.len(), b.len(), "file count is seed-independent");
        assert_ne!(a, b, "different seed must change content");
    }

    #[test]
    fn different_tier_diverges() {
        // Same seed, different tier tag => different content stream.
        let small = generate_corpus(ScaleTier::Small, 7);
        let medium = generate_corpus(ScaleTier::Medium, 7);
        // Compare the first file of each (paths overlap at repo0/file_0).
        let small_first = &small.first().expect("small non-empty").1;
        let medium_first = &medium.first().expect("medium non-empty").1;
        assert_ne!(
            small_first, medium_first,
            "tier tag must perturb the content stream"
        );
    }

    #[test]
    fn generated_file_count_matches_declared_shape() {
        let params = params_for(ScaleTier::Small);
        let corpus = generate_corpus(ScaleTier::Small, 1);
        let expected = usize::try_from(params.total_files()).expect("small total_files fits usize");
        assert_eq!(corpus.len(), expected);
    }

    #[test]
    fn every_file_carries_the_query_token() {
        let corpus = generate_corpus(ScaleTier::Small, 99);
        for (path, content) in &corpus {
            assert!(
                content.contains(SCALE_QUERY_TOKEN),
                "file {path} missing planted query token"
            );
        }
    }

    #[test]
    fn paths_are_unique() {
        let corpus = generate_corpus(ScaleTier::Small, 5);
        let mut paths: Vec<&str> = corpus.iter().map(|(p, _)| p.as_str()).collect();
        paths.sort_unstable();
        let count_before = paths.len();
        paths.dedup();
        assert_eq!(count_before, paths.len(), "generated paths must be unique");
    }

    #[test]
    fn medium_source_repositories_keep_the_same_relative_path_distinct() {
        let files = generate_scoped_corpus(ScaleTier::Medium, 7).expect("seeded fixture");
        let oracle = ScopedOracle::from_source(&files, ScaleTier::Medium).expect("source oracle");
        assert_eq!(files.len(), 256);
        assert_eq!(oracle.paths_by_repo.len(), 4);
        let shared_path = "src/file_0.rs";
        assert!(oracle.paths_by_repo["repo0"].contains(shared_path));
        assert!(oracle.paths_by_repo["repo1"].contains(shared_path));
        assert_ne!(files[0].content, files[64].content);
        assert_eq!(
            oracle.expected_count(Some("repo0")).expect("fixture count"),
            10
        );
        assert_eq!(oracle.expected_count(None).expect("fixture count"), 10);
        let repo1_rows = files
            .iter()
            .filter(|file| file.source_repo_id == "repo1")
            .take(10)
            .map(|file| {
                (
                    file.source_repo_id.as_str(),
                    file.repo_relative_path.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            oracle
                .verify_identities(Some("repo1"), repo1_rows.iter().copied())
                .expect("ten source-derived repo1 rows"),
            10
        );
        assert!(
            oracle
                .verify_identities(Some("repo0"), repo1_rows.iter().copied())
                .is_err(),
            "repo1 rows cannot satisfy the repo0 probe"
        );
        let mut duplicated = repo1_rows;
        duplicated[9] = duplicated[0];
        assert!(
            oracle
                .verify_identities(Some("repo1"), duplicated.iter().copied())
                .is_err(),
            "duplicate returned identity cannot satisfy top 10"
        );
    }

    #[test]
    fn scoped_fixture_rejects_missing_repo_anchor_duplicate_identity_and_wrong_shape() {
        let files = generate_scoped_corpus(ScaleTier::Medium, 7).expect("seeded fixture");
        let mut missing_anchor = files.clone();
        missing_anchor[0].content = missing_anchor[0]
            .content
            .replace(&repo_query_token(0), "removed");
        assert!(ScopedOracle::from_source(&missing_anchor, ScaleTier::Medium).is_err());

        let mut duplicate = files.clone();
        duplicate[64].source_repo_id = "repo0".to_string();
        assert!(ScopedOracle::from_source(&duplicate, ScaleTier::Medium).is_err());

        let mut absent = files;
        let _removed = absent.pop();
        assert!(ScopedOracle::from_source(&absent, ScaleTier::Medium).is_err());

        let mut wrong_path = generate_scoped_corpus(ScaleTier::Medium, 7).expect("seeded fixture");
        wrong_path[0].repo_relative_path = "src/other.rs".to_string();
        assert!(ScopedOracle::from_source(&wrong_path, ScaleTier::Medium).is_err());
    }

    #[test]
    fn scoped_digest_binds_source_repo_identity_and_source_bytes() {
        let files = generate_scoped_corpus(ScaleTier::Medium, 11).expect("seeded fixture");
        let original = scoped_corpus_digest(DIMENSION, &files);
        let mut changed_repo = files.clone();
        changed_repo[0].source_repo_id = "repo2".to_string();
        assert_ne!(original, scoped_corpus_digest(DIMENSION, &changed_repo));
        let mut changed_content = files;
        changed_content[0].content.push_str("// changed\n");
        assert_ne!(original, scoped_corpus_digest(DIMENSION, &changed_content));
    }

    #[test]
    fn deletion_oracle_excludes_only_one_source_identity_and_cpu_deltas_are_exact() -> AnyResult<()>
    {
        let files = generate_scoped_corpus(ScaleTier::Medium, 11)?;
        let oracle = ScopedOracle::from_source(&files, ScaleTier::Medium)?;
        let after = oracle.without_file("repo0", "src/file_0.rs")?;
        assert_eq!(after.paths_by_repo["repo0"].len(), 63);
        assert_eq!(after.paths_by_repo["repo1"].len(), 64);
        assert!(!after.paths_by_repo["repo0"].contains("src/file_0.rs"));
        assert!(after.paths_by_repo["repo1"].contains("src/file_0.rs"));
        assert!(after.without_file("repo0", "src/file_0.rs").is_err());

        let before = CpuSnapshot {
            user_us: 1_000,
            system_us: 2_000,
        };
        let later = CpuSnapshot {
            user_us: 3_500,
            system_us: 2_750,
        };
        assert_eq!(
            later.elapsed_since(before)?,
            CpuUsageV1 {
                user_ms: 2.5,
                system_ms: 0.75,
            }
        );
        assert!(before.elapsed_since(later).is_err());
        Ok(())
    }

    #[test]
    fn phase_rss_oracle_requires_interior_coverage_and_preserves_decreases() -> AnyResult<()> {
        let origin = Instant::now();
        let point = |ms: u64, bytes| RssPoint {
            at: origin + Duration::from_millis(ms),
            bytes,
        };
        let cpu = CpuUsageV1 {
            user_ms: 3.0,
            system_ms: 1.0,
        };
        let summarize = |end_ms, samples: &[RssPoint]| {
            summarize_phase_resources(
                origin + Duration::from_millis(5),
                origin + Duration::from_millis(end_ms),
                point(0, 4_096),
                point(end_ms + 5, 2_048),
                samples,
                cpu,
                1.0,
                2.0,
                3.0,
            )
        };
        let observed = summarize(
            350,
            &[point(100, 3_072), point(200, 8_192), point(300, 1_024)],
        )?;
        assert_eq!(observed.sampled_max_rss_bytes, 8_192);
        assert_eq!(observed.rss_end_bytes, 2_048);
        assert_eq!(observed.interior_samples, 3);
        assert_eq!(observed.observed_max_gap_ms, 55.0_f64.max(100.0));
        assert_eq!(observed.observer_setup_ms, 1.0);
        assert_eq!(observed.observer_teardown_ms, 2.0);
        assert_eq!(observed.observer_periodic_probe_wall_ms, 3.0);
        assert_eq!(observed.discarded_outside_phase_samples, 0);

        assert!(
            summarize(350, &[]).is_err(),
            "long phase needs an interior sample"
        );
        assert!(summarize(350, &[point(100, 1_024), point(100, 2_048)]).is_err());
        assert!(summarize(350, &[point(200, 1_024), point(100, 2_048)]).is_err());
        assert!(summarize(350, &[point(100, 0)]).is_err());
        assert!(
            summarize(900, &[point(100, 1_024)]).is_err(),
            "large sample gap is invalid"
        );
        let short = summarize(80, &[])?;
        assert_eq!(short.interior_samples, 0);
        assert_eq!(short.sampled_max_rss_bytes, 4_096);
        let raced = summarize(
            350,
            &[point(100, 3_072), point(200, 8_192), point(351, 9_999)],
        )?;
        assert_eq!(raced.discarded_outside_phase_samples, 1);
        assert_eq!(raced.sampled_max_rss_bytes, 8_192);
        assert!(
            summarize(350, &[point(360, 1_024)]).is_err(),
            "sample after end observation is invalid"
        );
        Ok(())
    }

    #[test]
    fn phase_observation_failure_does_not_mask_operation_stage() -> AnyResult<()> {
        let primary = anyhow::Error::new(ScaleStageError::operation(
            "build_seal",
            anyhow::anyhow!("fixed operation fault"),
        ));
        let observation = stage_or_preserve(
            "resource_observation",
            anyhow::anyhow!("fixed sampler fault"),
        );
        let combined = combine_phase_result::<()>(Err(primary), Err(observation))
            .expect_err("both faults reject the phase");
        let binding = source_binding_for_failure(ScaleTier::Medium, 13)?;
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let nested = finish_runtime_measurement::<()>(
            Err(stage_or_preserve("delta", combined)),
            Err(anyhow::anyhow!("driver stop fault")),
        )
        .expect_err("daemon cleanup also rejects the tier");
        let record = refusal_json(&binding, &head, &host, &nested);
        assert_eq!(record["failure"]["stage"], "build_seal");
        assert!(
            record["failure"]["primary"]["message"]
                .as_str()
                .unwrap()
                .contains("fixed sampler fault")
        );
        assert_eq!(record["failure"]["cleanup"]["message"], "driver stop fault");
        assert_eq!(
            record["failure"]["secondary_failures"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            record["failure"]["secondary_failures"][1]["context"],
            "phase resource observation"
        );
        Ok(())
    }

    #[test]
    fn refusal_record_binds_the_source_and_only_names_typed_limits() -> AnyResult<()> {
        let binding = source_binding_for_failure(ScaleTier::Medium, 13)?;
        assert_eq!(binding.source_repo_count, 4);
        assert_eq!(binding.file_count, 256);
        assert_eq!(binding.source_bytes, binding.corpus_bytes + 256);
        assert_eq!(
            binding.corpus_digest,
            scoped_corpus_digest(DIMENSION, &generate_scoped_corpus(ScaleTier::Medium, 13)?)
        );
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let capacity =
            check_admission_counts(1, MAX_SCALE_SOURCE_BYTES + 1, 1).expect_err("source bound");
        let typed = anyhow::Error::new(ScaleStageError::source_admission(anyhow::Error::new(
            capacity,
        )));
        let refused = refusal_json(&binding, &head, &host, &typed);
        assert_eq!(refused["status"], "refused");
        assert_eq!(refused["failure"]["stage"], "source_preflight");
        assert_eq!(
            refused["failure"]["limit"],
            "lexical_total_source_bytes_128m"
        );
        assert_eq!(refused["failure"]["observed"], MAX_SCALE_SOURCE_BYTES + 1);
        assert_eq!(refused["source"]["corpus_digest"], binding.corpus_digest);
        assert!(refused.get("latency").is_none());

        let open_loop_binding =
            source_binding_for_failure_in_dimension("open-loop", ScaleTier::Medium, 13)?;
        assert_ne!(open_loop_binding.corpus_digest, binding.corpus_digest);
        let execution = json!({"arrival_model": "seeded_poisson", "rates_qps": [25, 50]});
        let open_loop = refusal_json_with_context(
            &open_loop_binding,
            &head,
            &host,
            &typed,
            "open-loop",
            Some(&execution),
        );
        assert_eq!(open_loop["kind"], "quanta-index-open-loop-failure");
        assert_eq!(open_loop["execution"], execution);
        assert_eq!(
            open_loop["source"]["corpus_digest"],
            open_loop_binding.corpus_digest
        );
        assert_eq!(open_loop["failure"]["limit"], refused["failure"]["limit"]);
        assert!(open_loop.get("latency").is_none());

        let unknown = refusal_json(&binding, &head, &host, &anyhow::anyhow!("runtime failure"));
        assert_eq!(unknown["status"], "failed");
        assert_eq!(unknown["failure"]["stage"], "execution_unclassified");
        assert!(unknown["failure"]["limit"].is_null());
        let ingest_error =
            stage_or_preserve("build_ingest", anyhow::anyhow!("fixed ingest failure"));
        let ingest = refusal_json(&binding, &head, &host, &ingest_error);
        assert_eq!(ingest["failure"]["stage"], "build_ingest");
        assert!(ingest["failure"]["limit"].is_null());
        assert_eq!(ingest["status"], "failed");

        let nested = stage_or_preserve(
            "delta",
            anyhow::Error::new(ScaleStageError::source_admission(anyhow::Error::new(
                check_admission_counts(1, MAX_SCALE_SOURCE_BYTES + 1, 1).expect_err("source bound"),
            ))),
        );
        let preserved = refusal_json(&binding, &head, &host, &nested);
        assert_eq!(preserved["failure"]["stage"], "source_preflight");
        assert_eq!(
            preserved["failure"]["limit"],
            "lexical_total_source_bytes_128m"
        );

        let cleanup = finish_runtime_measurement::<()>(
            Ok(()),
            Err(stage_or_preserve(
                "cleanup",
                anyhow::anyhow!("fixed cleanup failure"),
            )),
        )
        .expect_err("cleanup failure must fail the tier");
        let cleanup_record = refusal_json(&binding, &head, &host, &cleanup);
        assert_eq!(cleanup_record["failure"]["stage"], "cleanup");
        assert!(cleanup_record["failure"]["limit"].is_null());
        assert_eq!(cleanup_record["status"], "failed");
        let build_error = anyhow::Error::new(ScaleStageError::operation(
            "build_seal",
            anyhow::anyhow!("SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED"),
        ));
        let failed = refusal_json(&binding, &head, &host, &build_error);
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["failure"]["stage"], "build_seal");
        assert!(failed["failure"]["limit"].is_null());
        assert!(
            failed["failure"]["message"].as_str().is_some_and(
                |message| message.contains("SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED")
            )
        );
        Ok(())
    }

    #[test]
    fn measurement_and_cleanup_failures_are_both_preserved_without_a_latency_row() -> AnyResult<()>
    {
        let failure = finish_runtime_measurement::<()>(
            Err(anyhow::anyhow!("primary measurement marker")),
            Err(anyhow::anyhow!("driver cleanup marker")),
        )
        .expect_err("both failures must reject the tier");
        let binding = source_binding_for_failure(ScaleTier::Medium, 13)?;
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let record = refusal_json(&binding, &head, &host, &failure);
        assert_eq!(record["status"], "failed");
        assert_eq!(record["failure"]["message"], "primary measurement marker");
        assert_eq!(
            record["failure"]["primary"]["message"],
            "primary measurement marker"
        );
        assert_eq!(
            record["failure"]["cleanup"]["message"],
            "driver cleanup marker"
        );
        assert!(record["failure"]["limit"].is_null());
        assert!(record.get("latency").is_none());
        assert!(finish_runtime_measurement(Ok(()), Err(anyhow::anyhow!("cleanup only"))).is_err());
        Ok(())
    }

    #[test]
    fn scoped_source_admission_counts_the_newline_and_rejects_each_hard_limit() {
        let files = generate_scoped_corpus(ScaleTier::Medium, 11).expect("seeded fixture");
        let expected_source_bytes: u64 = files
            .iter()
            .map(|file| u64::try_from(file.content.len() + 1).expect("fixture length"))
            .sum();
        let admitted = preflight_scoped_corpus(&files).expect("medium source is admissible");
        assert_eq!(admitted.source_bytes, expected_source_bytes);
        assert!(
            admitted.posting_memberships > u64::from(params_for(ScaleTier::Medium).total_files())
        );
        assert_eq!(
            check_admission_counts(MAX_SCALE_FILE_BYTES + 1, 1, 1)
                .expect_err("file bound")
                .limit,
            ScaleAdmissionLimit::SourceFileBytes
        );
        assert_eq!(
            check_admission_counts(1, MAX_SCALE_SOURCE_BYTES + 1, 1)
                .expect_err("source bound")
                .limit,
            ScaleAdmissionLimit::TotalSourceBytes
        );
        assert_eq!(
            check_admission_counts(1, 1, MAX_SCALE_POSTING_MEMBERSHIPS + 1)
                .expect_err("posting bound")
                .limit,
            ScaleAdmissionLimit::TrigramPostingMemberships
        );
        assert!(
            check_admission_counts(
                MAX_SCALE_FILE_BYTES,
                MAX_SCALE_SOURCE_BYTES,
                MAX_SCALE_POSTING_MEMBERSHIPS
            )
            .is_ok()
        );

        let mut non_ascii = files;
        non_ascii[0].content.push('é');
        assert!(preflight_scoped_corpus(&non_ascii).is_err());
    }

    #[test]
    fn same_relative_path_in_two_source_repos_survives_publish_and_delta() -> AnyResult<()> {
        let mut rt = E2eRuntime::boot_with_history_max_generations(2)?;
        let shared_path = "src/shared.rs";
        let content0 = format!(
            "// {SCALE_QUERY_TOKEN}\n// {} anchor\n// {} anchor\n",
            repo_query_token(0),
            file_query_token(0, 0)
        );
        let content1 = format!(
            "// {SCALE_QUERY_TOKEN}\n// {} anchor\n// {} anchor\n",
            repo_query_token(1),
            file_query_token(1, 0)
        );
        let specs0 = [E2eTextChunkSpec {
            content: &content0,
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("repo0"),
        }];
        let specs1 = [E2eTextChunkSpec {
            content: &content1,
            start_line: 1,
            end_line: 2,
            source_repo_id: Some("repo1"),
        }];
        let _ids =
            rt.ingest_text_files_one_batch(&[(shared_path, &specs0), (shared_path, &specs1)])?;
        let _wire = rt.preview_pending_search_corpus_wire_bytes()?;
        let _base = rt.seal()?;
        rt.activate_last_sealed_generation()?;
        for repo_index in 0..2 {
            let response =
                rt.query_text(TextQuerySyntax::Native, &repo_query_token(repo_index), 10);
            assert!(response.typed_error.is_none());
            assert_eq!(response.candidates.len(), 1);
            assert_eq!(
                response.candidates[0].source_repo_id.as_str(),
                format!("repo{repo_index}")
            );
            assert_eq!(
                response.candidates[0].repo_relative_path.as_str(),
                shared_path
            );
        }
        require_single_source_file(
            &rt.query_text(TextQuerySyntax::Native, &file_query_token(0, 0), 10),
            "repo0",
            shared_path,
        )?;
        require_single_source_file(
            &rt.query_text(TextQuerySyntax::Native, &file_query_token(1, 0), 10),
            "repo1",
            shared_path,
        )?;

        let changed0 = format!("{content0}// changed\n");
        let serving_owner = rt.repo();
        let _ids = rt.ingest_text_chunks(
            serving_owner.as_str(),
            shared_path,
            &[E2eTextChunkSpec {
                content: &changed0,
                start_line: 1,
                end_line: 2,
                source_repo_id: Some("repo0"),
            }],
        )?;
        let _delta = rt.seal()?;
        rt.activate_last_sealed_generation()?;
        let unaffected = rt.query_text(TextQuerySyntax::Native, &repo_query_token(1), 10);
        assert!(unaffected.typed_error.is_none());
        assert_eq!(unaffected.candidates.len(), 1);
        assert_eq!(unaffected.candidates[0].source_repo_id.as_str(), "repo1");
        assert_eq!(
            unaffected.candidates[0].repo_relative_path.as_str(),
            shared_path
        );

        rt.delete_chunk_for_source_file("repo0", shared_path)?;
        let _delete = rt.seal()?;
        rt.activate_last_sealed_generation()?;
        let deleted = rt.query_text(TextQuerySyntax::Native, &repo_query_token(0), 10);
        assert!(deleted.typed_error.is_none());
        assert!(deleted.candidates.is_empty());
        let retained = rt.query_text(TextQuerySyntax::Native, &repo_query_token(1), 10);
        assert!(retained.typed_error.is_none());
        assert_eq!(retained.candidates.len(), 1);
        assert_eq!(retained.candidates[0].source_repo_id.as_str(), "repo1");
        require_no_source_file(&rt.query_text(
            TextQuerySyntax::Native,
            &file_query_token(0, 0),
            10,
        ))?;
        require_single_source_file(
            &rt.query_text(TextQuerySyntax::Native, &file_query_token(1, 0), 10),
            "repo1",
            shared_path,
        )?;

        rt.try_reopen_in_place()?;
        rt.start()?;
        let still_deleted = rt.query_text(TextQuerySyntax::Native, &repo_query_token(0), 10);
        assert!(still_deleted.typed_error.is_none());
        assert!(still_deleted.candidates.is_empty());
        let still_retained = rt.query_text(TextQuerySyntax::Native, &repo_query_token(1), 10);
        assert!(still_retained.typed_error.is_none());
        assert_eq!(still_retained.candidates.len(), 1);
        assert_eq!(
            still_retained.candidates[0].source_repo_id.as_str(),
            "repo1"
        );
        require_no_source_file(&rt.query_text(
            TextQuerySyntax::Native,
            &file_query_token(0, 0),
            10,
        ))?;
        require_single_source_file(
            &rt.query_text(TextQuerySyntax::Native, &file_query_token(1, 0), 10),
            "repo1",
            shared_path,
        )?;
        Ok(())
    }

    #[test]
    fn small_tier_count_is_source_derived_and_every_measured_response_must_match() {
        let corpus = generate_corpus(ScaleTier::Small, 5);
        let expected =
            expected_small_result_count(&corpus).expect("all source files have the term");
        assert_eq!(
            expected, 10,
            "16 matching source files are capped by top_k=10"
        );
        let mut invalid_corpus = corpus;
        invalid_corpus[0].1 = invalid_corpus[0].1.replace(SCALE_QUERY_TOKEN, "absent");
        assert!(
            expected_small_result_count(&invalid_corpus).is_err(),
            "an incomplete planted source fixture must be rejected"
        );

        let mut calls = 0;
        let samples = collect_warm_samples(expected, 2, || {
            calls += 1;
            Ok(expected)
        })
        .expect("two complete pages");
        assert_eq!(samples.len(), 2);
        assert_eq!(calls, 2);

        calls = 0;
        let error = collect_warm_samples(expected, 3, || {
            calls += 1;
            Ok(if calls == 2 { expected - 1 } else { expected })
        })
        .expect_err("a partial second page must stop aggregation");
        assert_eq!(calls, 2, "the third query must not be measured");
        let message = error.to_string();
        assert!(message.contains("warm query sample 2/3"), "{message}");
        assert!(message.contains("returned 9 candidates"), "{message}");
    }

    #[test]
    fn manifest_json_schema_is_well_formed() {
        let value = tier_manifest_json();
        assert_eq!(value["kind"], "quanta-index-scale-tier-manifest");
        assert_eq!(value["manifest_schema_version"], 2);
        assert_eq!(value["dimension"], "scale");
        assert_eq!(value["query_token"], SCALE_QUERY_TOKEN);
        let tiers = value["tiers"].as_array().expect("tiers is an array");
        assert_eq!(tiers.len(), 4);
        for row in tiers {
            assert!(row["tier"].is_string());
            assert!(row["repo_count"].is_u64());
            assert!(row["files_per_repo"].is_u64());
            assert!(row["avg_file_lines"].is_u64());
            assert!(row["hit_density_per_mille"].is_u64());
            assert!(row["symbol_density_per_mille"].is_u64());
            assert!(row["total_files"].is_u64());
            assert!(row["default_run"].is_boolean());
            assert_eq!(row["selectable"], true);
            assert_eq!(row["serving_owner_count"], 1);
            assert_eq!(row["source_repo_count"], row["repo_count"]);
        }
        // Exactly the small tier is measured here.
        let measured: Vec<&Value> = tiers
            .iter()
            .filter(|r| r["default_run"] == Value::Bool(true))
            .collect();
        assert_eq!(measured.len(), 1, "only the small tier is measured here");
        assert_eq!(measured[0]["tier"], "small");
    }

    fn sample_measurement() -> TierMeasurement {
        let sample_resource = PhaseResourceV1 {
            cpu: CpuUsageV1 {
                user_ms: 1.0,
                system_ms: 0.25,
            },
            rss_start_bytes: 1_024,
            rss_end_bytes: 2_048,
            sampled_max_rss_bytes: 3_072,
            interior_samples: 2,
            observed_max_gap_ms: 100.0,
            observation_span_ms: 200.0,
            observer_setup_ms: 1.0,
            observer_teardown_ms: 2.0,
            observer_periodic_probe_wall_ms: 3.0,
            discarded_outside_phase_samples: 0,
        };
        TierMeasurement {
            client_request_timeout_ms: 30_000,
            requested_client_request_timeout_ms: None,
            history_max_bytes: HARNESS_HISTORY_MAX_BYTES,
            requested_history_max_bytes: None,
            cpu: Some(CpuUsageV1 {
                user_ms: 3.0,
                system_ms: 2.0,
            }),
            phase_resources: [
                ("full_ingest_seal", sample_resource.clone()),
                ("full_activate", sample_resource.clone()),
                ("delta_ingest_seal", sample_resource.clone()),
                ("delta_activate", sample_resource),
            ]
            .into_iter()
            .collect(),
            delete_reopen: None,
            tier: ScaleTier::Small,
            seed: 3,
            file_count: 16,
            corpus_digest: corpus_digest(DIMENSION, &generate_corpus(ScaleTier::Small, 3)),
            source_repo_count: 1,
            corpus_bytes: 4_096,
            ingest_decoded_bytes: None,
            ingest_wire_bytes: None,
            build_ms: 1.5,
            build_bytes_written: 8_192,
            activation_ms: 0.5,
            first_query_ms: 0.75,
            warm_query: LatencySummary::from_samples_ms(&[0.2, 0.25, 0.3]).expect("samples"),
            daemon: DaemonPhaseTimingV1 {
                cold_open_ms: Some(1.0),
                first_route_ms: 1.0,
                warm_route_mean_ms: 0.0,
            },
            adapter: AdapterPhaseTimingV1 {
                open_ms: 0.4,
                plan_ms: 0.01,
                execute_ms: 0.05,
            },
            delta: DeltaMeasurementV1 {
                update_ms: 0.9,
                changed_bytes: 300,
                bytes_written: 900,
                activation_with_reclaim_ms: 0.6,
                reclaimed_bytes: 8_000,
            },
            result_count: 10,
            model_revision: Some("model@rev:d16".to_string()),
        }
    }

    #[test]
    fn artifact_rejects_missing_or_malformed_phase_resource_proof() -> AnyResult<()> {
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let mut missing = sample_measurement();
        missing.phase_resources.remove("delta_activate");
        assert!(artifact(&missing, head.clone(), host.clone()).is_err());

        let mut nonfinite = sample_measurement();
        nonfinite
            .phase_resources
            .get_mut("full_activate")
            .unwrap()
            .cpu
            .user_ms = f64::NAN;
        assert!(artifact(&nonfinite, head.clone(), host.clone()).is_err());

        let mut no_interior = sample_measurement();
        no_interior
            .phase_resources
            .get_mut("full_ingest_seal")
            .unwrap()
            .interior_samples = 0;
        assert!(artifact(&no_interior, head, host).is_err());
        let observation = sample_measurement().phase_resources["full_activate"].clone();
        let mut phases = BTreeMap::new();
        record_phase(&mut phases, "full_activate", observation.clone())?;
        assert!(record_phase(&mut phases, "full_activate", observation).is_err());
        Ok(())
    }

    #[test]
    fn detail_json_records_every_phase_and_the_advisory_split() {
        let value = detail_json(&sample_measurement());
        assert_eq!(
            value["passed"], true,
            "a measured run with retrieved hits must record passed=true"
        );
        let measured = value["measured_tiers"].as_array().expect("measured array");
        assert_eq!(measured.len(), 1);
        let tier = &measured[0];
        assert_eq!(tier["tier"], "small");
        assert_eq!(tier["status"], "measured");
        assert_eq!(tier["build"]["build_ms"], 1.5);
        assert_eq!(tier["activation_ms"], 0.5);
        assert_eq!(tier["wall_across_socket"]["first_query_ms"], 0.75);
        assert_eq!(tier["daemon_phases_ms"]["cold_open_ms"], 1.0);
        assert_eq!(tier["adapter_only_phases_ms"]["execute_ms"], 0.05);
        assert_eq!(tier["delta"]["reclaimed_bytes"], 8_000);
        assert_eq!(tier["client_request_timeout_ms"], 30_000);
        assert!(tier["requested_client_request_timeout_ms"].is_null());
        assert_eq!(tier["history_max_bytes"], HARNESS_HISTORY_MAX_BYTES);
        assert_eq!(
            tier["phase_resources"]["full_ingest_seal"]["sampled_max_rss_bytes"],
            3_072
        );
        assert_eq!(
            tier["phase_resources"]["full_ingest_seal"]["sampled_max_is_true_peak"],
            false
        );
        assert_eq!(
            tier["phase_resources"]["full_ingest_seal"]["interior_samples"],
            2
        );
        assert!(tier["requested_history_max_bytes"].is_null());
        assert_eq!(
            tier["history_max_total_bytes"],
            HARNESS_HISTORY_MAX_TOTAL_BYTES
        );
        assert_eq!(
            tier["history_max_revision_pairs"],
            HARNESS_HISTORY_MAX_REVISION_PAIRS
        );
        assert_eq!(tier["cpu_process"]["user_ms"], 3.0);
        assert_eq!(tier["cpu_process"]["system_ms"], 2.0);
        assert!(tier["delete_reopen"].is_null());
        assert!(
            tier.get("open_ms").is_none(),
            "the misnamed activation field is gone"
        );
        let advisory = value["declared_advisory_tiers"]
            .as_array()
            .expect("advisory array");
        assert_eq!(advisory.len(), 3, "medium/large/xlarge are advisory");
        for row in advisory {
            assert_eq!(row["status"], "declared-advisory");
        }
    }

    #[test]
    fn the_artifact_carries_the_phases_the_amplification_and_the_corpus_digest() {
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567").expect("a head");
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let report =
            artifact(&sample_measurement(), head.clone(), host.clone()).expect("observable");
        let value = report.to_json().expect("serializes");
        assert_eq!(value["dimension"], "scale");
        assert_eq!(value["phases"]["build_ms"], 1.5);
        assert_eq!(value["phases"]["update_ms"], 0.9);
        assert_eq!(value["phases"]["gc_ms"], 0.6);
        assert_eq!(value["disk_amplification"]["bytes_written"], 8_192);
        assert_eq!(value["disk_amplification"]["changed_bytes"], 4_096);
        assert_eq!(value["disk_amplification"]["ratio"], 2.0);
        assert_eq!(
            value["provenance"]["corpus_digest"],
            corpus_digest(DIMENSION, &generate_corpus(ScaleTier::Small, 3))
        );
        assert_ne!(
            value["provenance"]["corpus_digest"],
            corpus_digest(DIMENSION, &generate_corpus(ScaleTier::Small, 4)),
            "the corpus digest follows the seed"
        );
        assert_eq!(value["rows"][0]["scenario_id"], "scale.small.warm_query");
        assert_eq!(value["rows"][0]["latency"]["samples"], 3);
        let mut longer_timeout = sample_measurement();
        longer_timeout.client_request_timeout_ms = 300_000;
        let changed = artifact(&longer_timeout, head.clone(), host.clone())
            .expect("observable")
            .to_json()
            .expect("serializes");
        assert_ne!(
            value["provenance"]["config_digest"],
            changed["provenance"]["config_digest"]
        );
        let mut larger_history = sample_measurement();
        larger_history.history_max_bytes = 268_435_456;
        larger_history.requested_history_max_bytes = Some(268_435_456);
        let changed = artifact(&larger_history, head.clone(), host.clone())
            .expect("observable")
            .to_json()
            .expect("serializes");
        assert_ne!(
            value["provenance"]["config_digest"],
            changed["provenance"]["config_digest"]
        );
        let mut missing_cpu = sample_measurement();
        missing_cpu.cpu = None;
        assert!(artifact(&missing_cpu, head, host).is_err());
    }
}
