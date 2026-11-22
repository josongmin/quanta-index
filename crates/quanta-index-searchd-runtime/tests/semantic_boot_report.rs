//! LDB-E2E-01 semantic seed counts and orphan classification on Legacy boot.

#![forbid(unsafe_code)]

use std::{error::Error, fs, path::Path};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{GenerationQuarantineReasonV1, GenerationStorageKeyV1};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, publish_legacy_source},
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};

type TestResult = Result<(), Box<dyn Error>>;

fn require_empty_semantic_seed(client: &QuantaIndex, orphans: u32) -> TestResult {
    // SemanticBootReport uses the same TrackInventoryReport as these gauges.
    let snapshot = client.observability().metrics_snapshot()?;
    for (name, expected) in [
        ("boot_semantic_sealed_generations", 0.0_f64),
        ("boot_semantic_quarantined_generations", 0.0),
        ("boot_semantic_orphaned_generations", f64::from(orphans)),
        ("boot_active_pairs_validated", 0.0),
    ] {
        let point = snapshot.gauges.iter().find(|gauge| gauge.name == name);
        if point.map(|gauge| gauge.value.to_bits()) != Some(expected.to_bits()) {
            return Err(format!("{name} should be {expected}, got {point:?}").into());
        }
    }
    Ok(())
}

#[test]
fn fresh_runtime_exposes_empty_semantic_boot_report() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let daemon = binary.start(root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let client = daemon.connect()?;
    require_empty_semantic_seed(&client, 0)?;
    drop(client);
    daemon.stop()?;
    binary.verify()
}

fn alter_row_count(manifest_path: &Path) -> TestResult {
    let bytes = fs::read(manifest_path)?;
    let mut value: ciborium::Value = ciborium::from_reader(bytes.as_slice())?;
    let ciborium::Value::Map(entries) = &mut value else {
        return Err("expected a semantic scope manifest map".into());
    };
    let (_, row_count) = entries
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("row_count"))
        .ok_or("expected row_count in the scope manifest")?;
    *row_count = ciborium::Value::Integer(99_u64.into());
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)?;
    fs::write(manifest_path, tampered)?;
    Ok(())
}

/// A bad orphan is listed by its physical path and stays out of readiness.
/// Move only the semantic directory from a stopped donor. The receiving root
/// has never published that generation and has no activation or seal record.
#[test]
fn runtime_boot_inventories_a_corrupted_inactive_semantic_generation() -> TestResult {
    let donor = private_tempdir()?;
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-bootrep")?;
    let revision = RevisionId::new("rev-bootrep")?;
    let generation = ManifestGeneration::new(9);
    let producer = binary.start(donor.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let client = producer.connect()?;
    let (_head, receipt) = publish_legacy_source(
        &client,
        &repo,
        &revision,
        generation.get(),
        None,
        &[("x.rs", "fn x() {}", 1)],
        None,
    )?;
    if !receipt.sealed || receipt.accepted_semantic_replace_scopes != 1 {
        return Err(format!("orphan donor did not seal one semantic file: {receipt:?}").into());
    }
    drop(client);
    producer.stop()?;
    let family = GenerationStorageKeyV1::for_repo_revision(&repo, &revision);
    let track = Path::new("indexes/semantic");
    let original = family.generation_dir(&donor.path().join(track), generation);
    let orphan = family.generation_dir(&root.path().join(track), generation);
    fs::create_dir_all(orphan.parent().ok_or("orphan has no generation family")?)?;
    fs::rename(original, &orphan)?;
    alter_row_count(&orphan.join("semantic-manifest.cbor"))?;

    let daemon = binary.start(root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let client = daemon.connect()?;
    require_empty_semantic_seed(&client, 1)?;
    let inventory = client.quarantine().inventory()?;
    let [entry] = inventory.semantic.as_slice() else {
        return Err(format!("boot must list exactly one semantic orphan: {inventory:?}").into());
    };
    if Path::new(&entry.path) != fs::canonicalize(&orphan)?
        || entry.reason != GenerationQuarantineReasonV1::Orphaned.as_code_str()
    {
        return Err(format!("boot listed the wrong semantic orphan: {entry:?}").into());
    }
    drop(client);
    daemon.stop()?;
    binary.verify()
}
