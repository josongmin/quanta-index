//! Canonical pure source fixture and source-derived oracle for scale tiers.
//! Mounted by the harness and Native Large runtime process test.

use std::fmt::Write as _;

use anyhow::Result as AnyResult;

/// The deterministic query the small-tier measurement issues.
///
/// Every generated file embeds this token at the configured hit density, so a
/// correct lexical engine returns a non-empty ordering. An empty result is a
/// rail failure, not a benign zero.
pub(super) const SCALE_QUERY_TOKEN: &str = "scale_needle_token";

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
