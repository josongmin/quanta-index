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
//! - a **measured run of the SMALL tier only** — `measure_small_tier` boots one
//!   [`E2eRuntime`], ingests the seeded small corpus, seals, activates, and runs
//!   one query, capturing ingest / open / query wall-times into a
//!   [`TierMeasurement`]. Medium/large/xlarge are emitted as
//!   `declared-advisory`: the canonical Linux perf runner owns the *blocking*
//!   timing; a macbook number here would be advisory noise, so the rail records
//!   the declared shape without fabricating a latency for those tiers.
//!
//! Fail-closed posture: a query that the runtime rejects on the small tier is a
//! rail error (typed error -> `Err`), never a zero-latency "pass".

use std::path::Path;
use std::time::Instant;

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;
use serde_json::{Value, json};

use crate::harness::E2eRuntime;

/// Repo id used for every generated scale corpus.
const SCALE_REPO: &str = "repo-scale";

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

    /// Whether this tier is measured end-to-end in this rail.
    ///
    /// Only `small` is measured here; the larger tiers are declared-advisory and
    /// owned by the canonical Linux perf runner.
    #[must_use]
    pub fn is_measured_here(self) -> bool {
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
/// - a deterministic set of filler lines drawn from [`VOCAB`];
/// - the query token [`SCALE_QUERY_TOKEN`] planted at the tier's `hit_density`;
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
// Small-tier measurement (the only end-to-end timed tier here).
// ---------------------------------------------------------------------------

/// Captured wall-times for one measured tier run.
#[derive(Clone, Debug)]
pub struct TierMeasurement {
    pub tier: ScaleTier,
    pub seed: u64,
    pub file_count: usize,
    pub ingest_ms: f64,
    pub open_ms: f64,
    pub query_ms: f64,
    pub result_count: usize,
}

/// Lossless-in-practice `usize -> f64` for small corpus counts.
///
/// File and result counts in this rail are far below `2^53`, so the precision
/// loss the workspace denies cannot occur; the localized `expect` documents that
/// invariant instead of hiding it behind an `as` cast.
#[cfg(test)]
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "scale corpus file/result counts are far below 2^53, so usize->f64 is exact on these values"
)]
fn usize_to_f64(n: usize) -> f64 {
    n as f64
}

/// Convert an elapsed `Instant` span to milliseconds.
fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// Boot, seed the SMALL tier, seal, activate, run one query — capturing
/// ingest / open / query wall-times.
///
/// Fail-closed: a typed error on the measured query is a rail error, never a
/// zero-latency pass over an empty result.
pub fn measure_small_tier(seed: u64) -> AnyResult<TierMeasurement> {
    let corpus = generate_corpus(ScaleTier::Small, seed);
    let file_count = corpus.len();

    let mut rt = E2eRuntime::boot()?;

    let ingest_started = Instant::now();
    for (path, content) in &corpus {
        rt.ingest_text(SCALE_REPO, path, content)?;
    }
    let _generation = rt.seal()?;
    let ingest_ms = elapsed_ms(ingest_started);

    let open_started = Instant::now();
    rt.activate_last_sealed_generation()?;
    let open_ms = elapsed_ms(open_started);

    let query_started = Instant::now();
    let result = rt.query_text(TextQuerySyntax::Native, SCALE_QUERY_TOKEN, SCALE_TOP_K);
    let query_ms = elapsed_ms(query_started);

    if let Some(error) = result.typed_error {
        return Err(anyhow::anyhow!(
            "scale small-tier query returned typed error {}: {}",
            error.code,
            error.message
        ));
    }
    let result_count = result.candidates.len();
    if result_count == 0 {
        return Err(anyhow::anyhow!(
            "scale small-tier query returned an empty ordering; planted `{SCALE_QUERY_TOKEN}` was not retrievable"
        ));
    }

    Ok(TierMeasurement {
        tier: ScaleTier::Small,
        seed,
        file_count,
        ingest_ms,
        open_ms,
        query_ms,
        result_count,
    })
}

// ---------------------------------------------------------------------------
// Artifact emission (manual json!, mirrors relevance/ambiguity rails).
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
        "measured_here": params.tier.is_measured_here(),
        "measurement_owner": if params.tier.is_measured_here() {
            "macbook-advisory-and-linux-blocking"
        } else {
            "linux-perf-runner-blocking"
        },
    })
}

/// The checked-in tier manifest, serialized.
#[must_use]
pub fn tier_manifest_json() -> Value {
    json!({
        "schema_version": 1,
        "dimension": "scale",
        "query_token": SCALE_QUERY_TOKEN,
        "tiers": TIER_MANIFEST.iter().map(tier_params_json).collect::<Vec<_>>(),
    })
}

fn measurement_json(measurement: &TierMeasurement) -> Value {
    json!({
        "tier": measurement.tier.as_str(),
        "seed": measurement.seed,
        "file_count": measurement.file_count,
        "ingest_ms": measurement.ingest_ms,
        "open_ms": measurement.open_ms,
        "query_ms": measurement.query_ms,
        "result_count": measurement.result_count,
        "status": "measured",
    })
}

/// Advisory row for a tier that is declared but not measured here.
fn declared_advisory_json(params: &TierParams) -> Value {
    json!({
        "tier": params.tier.as_str(),
        "total_files": params.total_files(),
        "status": "declared-advisory",
        "note": "blocking timing owned by the canonical Linux perf runner; not measured on this host",
    })
}

/// Build the scale summary value over the one measured tier plus the advisory
/// declared tiers.
#[must_use]
pub fn summary_json(measurement: &TierMeasurement, git_rev: &str) -> Value {
    let advisory: Vec<Value> = TIER_MANIFEST
        .iter()
        .filter(|p| !p.tier.is_measured_here())
        .map(declared_advisory_json)
        .collect();
    json!({
        "schema_version": 1,
        "dimension": "scale",
        "git_rev": git_rev,
        "host_class": "macbook-advisory",
        // Rail pass condition: the measured small-tier query retrieved the planted
        // token (an empty ordering is a rail error upstream, never written here).
        // Surfaced as a top-level flag so the J7Q-08 integration summary can treat
        // scale as a live dimension without re-deriving the verdict.
        "passed": measurement.result_count > 0,
        "measured_tiers": [measurement_json(measurement)],
        "declared_advisory_tiers": advisory,
        "blocking_note": "only the small tier is measured end-to-end here and on advisory terms; medium/large/xlarge blocking latency is owned by the Linux perf runner",
    })
}

/// Write the two canonical scale artifacts under `dir`:
/// `tier_manifest.json` and `summary.json`.
pub fn write_artifacts(measurement: &TierMeasurement, dir: &Path, git_rev: &str) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("tier_manifest.json"), &tier_manifest_json())?;
    crate::artifact::write_json_pretty(
        &dir.join("summary.json"),
        &summary_json(measurement, git_rev),
    )?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index into JSON values and slices whose shape this module constructs and asserts directly; an out-of-range index is a legitimate test failure"
)]
mod tests {
    //! Generator determinism + manifest schema.
    //!
    //! These never boot a runtime: they pin the pure construction layer so a
    //! perf number can always be attributed to a known, reproducible corpus.
    use super::*;

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
    fn manifest_json_schema_is_well_formed() {
        let value = tier_manifest_json();
        assert_eq!(value["schema_version"], 1);
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
            assert!(row["measured_here"].is_boolean());
        }
        // Exactly the small tier is measured here.
        let measured: Vec<&Value> = tiers
            .iter()
            .filter(|r| r["measured_here"] == Value::Bool(true))
            .collect();
        assert_eq!(measured.len(), 1, "only the small tier is measured here");
        assert_eq!(measured[0]["tier"], "small");
    }

    #[test]
    fn summary_json_records_measured_and_advisory_split() {
        let measurement = TierMeasurement {
            tier: ScaleTier::Small,
            seed: 3,
            file_count: 16,
            ingest_ms: 1.5,
            open_ms: 0.5,
            query_ms: 0.25,
            result_count: 10,
        };
        let value = summary_json(&measurement, "deadbeef");
        assert_eq!(value["dimension"], "scale");
        assert_eq!(value["git_rev"], "deadbeef");
        assert_eq!(
            value["passed"], true,
            "a measured run with retrieved hits must record passed=true"
        );
        let measured = value["measured_tiers"].as_array().expect("measured array");
        assert_eq!(measured.len(), 1);
        assert_eq!(measured[0]["tier"], "small");
        assert_eq!(measured[0]["status"], "measured");
        let advisory = value["declared_advisory_tiers"]
            .as_array()
            .expect("advisory array");
        assert_eq!(advisory.len(), 3, "medium/large/xlarge are advisory");
        for row in advisory {
            assert_eq!(row["status"], "declared-advisory");
        }
    }

    #[test]
    fn usize_to_f64_is_exact_on_small_counts() {
        assert!((usize_to_f64(0) - 0.0).abs() < f64::EPSILON);
        assert!((usize_to_f64(16) - 16.0).abs() < f64::EPSILON);
    }
}
