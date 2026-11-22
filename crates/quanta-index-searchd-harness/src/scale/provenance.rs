//! Source identity and manifest authority shared by Legacy and Native scale rails.

use anyhow::Result;
use serde_json::{Value, json};

use crate::artifact::{corpus_digest, corpus_digest_refs};
use crate::source_fixture::{
    SCALE_QUERY_TOKEN, ScaleTier, ScopedFile, TIER_MANIFEST, TierParams, generate_corpus,
    generate_scoped_corpus, params_for,
};

pub const DIMENSION: &str = "scale";

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

pub fn source_binding_for_failure(tier: ScaleTier, seed: u64) -> Result<ScaleSourceBinding> {
    source_binding_for_failure_in_dimension(DIMENSION, tier, seed)
}

/// Reuse the scale fixture identity for another rail without reusing its
/// domain-separated digest as if it were the same measurement.
pub fn source_binding_for_failure_in_dimension(
    dimension: &str,
    tier: ScaleTier,
    seed: u64,
) -> Result<ScaleSourceBinding> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    #[test]
    fn scoped_digest_binds_source_repo_identity_and_source_bytes() -> Result<()> {
        let files = generate_scoped_corpus(ScaleTier::Medium, 11)?;
        let original = scoped_corpus_digest(DIMENSION, &files);
        let mut changed_repo = files.clone();
        changed_repo
            .first_mut()
            .context("seeded fixture is empty")?
            .source_repo_id = "repo2".to_string();
        assert_ne!(original, scoped_corpus_digest(DIMENSION, &changed_repo));
        let mut changed_content = files;
        changed_content
            .first_mut()
            .context("seeded fixture is empty")?
            .content
            .push_str("// changed\n");
        assert_ne!(original, scoped_corpus_digest(DIMENSION, &changed_content));
        Ok(())
    }

    #[test]
    fn manifest_json_schema_is_well_formed() -> Result<()> {
        let value = tier_manifest_json();
        assert_eq!(value["kind"], "quanta-index-scale-tier-manifest");
        assert_eq!(value["manifest_schema_version"], 2);
        assert_eq!(value["dimension"], "scale");
        assert_eq!(value["query_token"], SCALE_QUERY_TOKEN);
        let tiers = value["tiers"].as_array().context("tiers is an array")?;
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
        let default_tier = measured
            .first()
            .context("default measured tier is absent")?;
        assert_eq!(default_tier["tier"], "small");
        Ok(())
    }
}
