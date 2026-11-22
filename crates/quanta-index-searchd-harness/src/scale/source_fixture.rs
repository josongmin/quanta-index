//! Canonical pure source fixture and source-derived oracle for scale tiers.
//! Mounted by the harness and Native Large runtime process test.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::Result as AnyResult;
use quanta_index_contract::LexicalCandidate;

use crate::scale_failure::{ScaleAdmissionLimit, ScaleAdmissionRefusal};

/// The deterministic query the small-tier measurement issues.
///
/// Every generated file embeds this token at the configured hit density, so a
/// correct lexical engine returns a non-empty ordering. An empty result is a
/// rail failure, not a benign zero.
pub(super) const SCALE_QUERY_TOKEN: &str = "scale_needle_token";

/// Explicit scale benchmark history capacity profile; small unit fixtures keep their own
/// 16 MiB history policy. These are bounded supported-profile inputs, not
/// measured latency or memory guarantees.
pub const SCALE_HISTORY_MAX_BYTES: u64 = 1024 * 1024 * 1024;
pub const SCALE_HISTORY_MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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

/// Planted query token unique to one generated source repository.
#[must_use]
pub fn repo_query_token(repo_index: u32) -> String {
    format!("scalereponeedle{repo_index:03}")
}

pub(super) fn file_query_token(repo_index: u32, file_index: u32) -> String {
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

pub(super) fn parse_source_repo_index(source_repo_id: &str) -> AnyResult<u32> {
    let suffix = source_repo_id
        .strip_prefix("repo")
        .ok_or_else(|| anyhow::anyhow!("scale: invalid source repo ID"))?;
    suffix
        .parse::<u32>()
        .map_err(|error| anyhow::anyhow!("scale: invalid source repo ID {source_repo_id}: {error}"))
}

/// Validate source identity, planted tokens and exact tier inventory before a run.
pub fn scoped_source_paths(
    files: &[ScopedFile],
    tier: ScaleTier,
) -> AnyResult<BTreeMap<String, BTreeSet<String>>> {
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
            .ok_or_else(|| anyhow::anyhow!("scale: invalid scoped file path"))?
            .parse::<u32>()
            .map_err(|error| anyhow::anyhow!("scale: invalid scoped file path: {error}"))?;
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
    Ok(paths_by_repo)
}

/// Result cap for the measured small-tier query.
pub(crate) const SCALE_TOP_K: u32 = 10;

/// Source-derived identities and expected result counts, independent of the
/// engine's candidate list or ranking.
#[derive(Clone, Debug)]
pub struct ScopedOracle {
    paths_by_repo: BTreeMap<String, BTreeSet<String>>,
}

impl ScopedOracle {
    pub fn from_source(files: &[ScopedFile], tier: ScaleTier) -> AnyResult<Self> {
        Ok(Self {
            paths_by_repo: scoped_source_paths(files, tier)?,
        })
    }

    /// Derive the expected source identities after deleting one file.
    pub fn without_file(&self, source_repo_id: &str, path: &str) -> AnyResult<Self> {
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

pub(crate) fn require_result_count(observed: usize, expected: usize, phase: &str) -> AnyResult<()> {
    if observed != expected {
        return Err(anyhow::anyhow!(
            "scale: {phase} returned {observed} candidates, source oracle requires {expected}"
        ));
    }
    Ok(())
}

/// The scale fixture and source paths are ASCII.
///
/// Count distinct three-byte windows per file/surface, matching the file
/// authority's membership admission. These limits mirror
/// `lexical/file_authority.rs`; keep focused threshold tests in sync when that
/// authority changes.
const MAX_SCALE_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_SCALE_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
pub(super) fn scale_posting_membership_limit() -> u64 {
    u64::from(quanta_index_lexical::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopedCorpusAdmission {
    pub source_bytes: u64,
    pub posting_memberships: u64,
}

pub(super) fn check_admission_counts(
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
    if postings > scale_posting_membership_limit() {
        return Err(ScaleAdmissionRefusal {
            limit: ScaleAdmissionLimit::TrigramPostingMemberships,
            observed: postings,
            maximum: scale_posting_membership_limit(),
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

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixture tests index into generated rows whose declared count is the assertion"
)]
mod tests {
    use super::*;
    use anyhow::Context as _;

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
    fn deletion_oracle_removes_only_the_exact_source_identity() {
        let files = generate_scoped_corpus(ScaleTier::Medium, 7).expect("seeded fixture");
        let original = ScopedOracle::from_source(&files, ScaleTier::Medium).expect("source oracle");
        let deleted = original
            .without_file("repo0", "src/file_0.rs")
            .expect("known source identity");

        assert!(original.paths_by_repo["repo0"].contains("src/file_0.rs"));
        assert!(!deleted.paths_by_repo["repo0"].contains("src/file_0.rs"));
        assert!(deleted.paths_by_repo["repo1"].contains("src/file_0.rs"));
        assert_eq!(deleted.paths_by_repo["repo0"].len(), 63);
        assert_eq!(deleted.paths_by_repo["repo1"].len(), 64);
        assert!(deleted.without_file("repo0", "src/file_0.rs").is_err());
        assert!(deleted.without_file("absent", "src/file_0.rs").is_err());

        let rows = files
            .iter()
            .filter(|file| {
                file.source_repo_id == "repo0" && file.repo_relative_path != "src/file_0.rs"
            })
            .take(10)
            .map(|file| {
                (
                    file.source_repo_id.as_str(),
                    file.repo_relative_path.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(deleted.verify_identities(Some("repo0"), rows).unwrap(), 10);
        assert!(
            deleted
                .verify_identities(Some("repo0"), [("repo0", "src/file_0.rs")])
                .is_err()
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
    fn scoped_source_admission_counts_the_newline_and_rejects_each_hard_limit() -> AnyResult<()> {
        let files = generate_scoped_corpus(ScaleTier::Medium, 11)?;
        let expected_source_bytes = files.iter().try_fold(0_u64, |total, file| {
            let line_bytes = u64::try_from(file.content.bytes().chain([b'\n']).count())?;
            total
                .checked_add(line_bytes)
                .context("fixture length overflow")
        })?;
        let admitted = preflight_scoped_corpus(&files)?;
        assert_eq!(admitted.source_bytes, expected_source_bytes);
        assert!(
            admitted.posting_memberships > u64::from(params_for(ScaleTier::Medium).total_files())
        );
        assert!(matches!(
            check_admission_counts(MAX_SCALE_FILE_BYTES + 1, 1, 1),
            Err(ScaleAdmissionRefusal {
                limit: ScaleAdmissionLimit::SourceFileBytes,
                ..
            })
        ));
        assert!(matches!(
            check_admission_counts(1, MAX_SCALE_SOURCE_BYTES + 1, 1),
            Err(ScaleAdmissionRefusal {
                limit: ScaleAdmissionLimit::TotalSourceBytes,
                ..
            })
        ));
        assert_eq!(
            scale_posting_membership_limit(),
            u64::from(quanta_index_lexical::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT)
        );
        assert_eq!(scale_posting_membership_limit(), 20_000_000);
        assert!(check_admission_counts(1, 1, 17_715_020).is_ok());
        let Err(above_posting_limit) = check_admission_counts(1, 1, 20_000_001) else {
            anyhow::bail!("one posting above the maximum was accepted");
        };
        assert_eq!(
            above_posting_limit.limit,
            ScaleAdmissionLimit::TrigramPostingMemberships
        );
        assert_eq!(above_posting_limit.observed, 20_000_001);
        assert_eq!(
            above_posting_limit.limit.as_str(),
            "lexical_trigram_posting_memberships"
        );
        assert_eq!(
            above_posting_limit.maximum,
            scale_posting_membership_limit()
        );
        assert!(
            check_admission_counts(
                MAX_SCALE_FILE_BYTES,
                MAX_SCALE_SOURCE_BYTES,
                scale_posting_membership_limit()
            )
            .is_ok()
        );

        let mut non_ascii = files;
        non_ascii
            .first_mut()
            .context("missing fixture file")?
            .content
            .push('é');
        assert!(preflight_scoped_corpus(&non_ascii).is_err());
        Ok(())
    }
}
