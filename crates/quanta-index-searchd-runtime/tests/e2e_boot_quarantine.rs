//! QI-BB-029: the boot inventory names an orphan sealed on just one track.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, QuarantineDiscardOutcomeDtoV1, QuarantineTargetV1, RepoId,
    RevisionId, SearchCorpusActiveHeadV1,
};
use quanta_index_core::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IntegrityScrubPolicyV1,
};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, LegacySearchdCommand, SearchdBinaryProcess, publish_legacy_source,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};

type TestResult = Result<(), Box<dyn Error>>;

const POOL_OBJECTS: &str = "indexes/lexical/lexical-object-pool/objects";

fn boot_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    root: &Path,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    let mut command = binary.benchmark_command(root, DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let _configured = command
        .env("QUANTA_INDEX_MAINTENANCE_TICK_MS", "50")
        .env("QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS", "86400000")
        .env(
            "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP",
            IntegrityScrubPolicyV1::DEFAULT
                .max_bytes_per_step
                .to_string(),
        );
    Ok(command)
}

fn seed_pair(
    binary: &LegacySearchdBinary,
    root: &Path,
    pin: &GenerationPin,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let command = boot_command(binary, root)?;
    let daemon = SearchdBinaryProcess::start_with_pinned_command(root, command)?;
    let client = daemon.connect()?;
    let (head, receipt) = publish_legacy_source(
        &client,
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        None,
        &[("src/first.rs", "fn first() { needle_first }", 1)],
        None,
    )?;
    if !receipt.sealed || receipt.accepted_semantic_replace_scopes != 1 {
        return Err(format!("fixture did not seal both tracks: {receipt:?}").into());
    }
    drop(client);
    daemon.stop()?;
    Ok(head)
}

fn generation_dir(root: &Path, track: &str, pin: &GenerationPin) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&pin.repo_id, &pin.revision_id)
        .generation_dir(&root.join("indexes").join(track), pin.manifest_generation)
}

/// Install content-addressed dependencies without replacing any existing
/// object bytes in the receiving root.
fn copy_pool_objects(donor: &Path, receiver: &Path) -> TestResult {
    for shard in fs::read_dir(donor.join(POOL_OBJECTS))? {
        let shard = shard?;
        let destination = receiver.join(POOL_OBJECTS).join(shard.file_name());
        fs::create_dir_all(&destination)?;
        for entry in fs::read_dir(shard.path())? {
            let entry = entry?;
            let from = entry.path();
            let to = destination.join(entry.file_name());
            if to.try_exists()? {
                if fs::read(&from)? != fs::read(&to)? {
                    return Err(format!("different bytes at pooled object {to:?}").into());
                }
            } else {
                let _copied = fs::copy(&from, &to)?;
            }
        }
    }
    Ok(())
}

fn require_boot_counts(client: &QuantaIndex, halves: f64) -> TestResult {
    let metrics = client.observability().metrics_snapshot()?;
    for (name, expected) in [
        ("boot_half_sealed_pairs", halves),
        ("boot_active_pairs_validated", 1.0),
    ] {
        let actual = metrics.gauges.iter().find(|gauge| gauge.name == name);
        if actual.map(|point| point.value.to_bits()) != Some(expected.to_bits()) {
            return Err(format!("expected {name}={expected}, got {actual:?}").into());
        }
    }
    Ok(())
}

/// Move a real sealed Lexical generation into a stopped root. The donor's
/// Semantic generation and its catalog stay behind, leaving the physical
/// half-seal that boot must report against the receiver's unchanged G1.
#[test]
fn a_generation_sealed_on_one_track_only_is_named_at_boot() -> TestResult {
    let parent = private_tempdir()?;
    let root = parent.path().join("receiver");
    let donor = parent.path().join("donor");
    let binary = LegacySearchdBinary::from_environment()?;
    let first = GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    );
    let half = GenerationPin::new(
        first.repo_id.clone(),
        first.revision_id.clone(),
        ManifestGeneration::new(7),
    );
    let active = seed_pair(&binary, &root, &first)?;
    let _donor_head = seed_pair(&binary, &donor, &half)?;
    let root = fs::canonicalize(root)?;
    let half_dir = generation_dir(&root, "lexical", &half);
    let donor_semantic = generation_dir(&donor, "semantic", &half);
    if !donor_semantic.is_dir() || half_dir.try_exists()? {
        return Err("the donor must have a paired seal and the receiver no half yet".into());
    }
    copy_pool_objects(&donor, &root)?;
    fs::rename(generation_dir(&donor, "lexical", &half), &half_dir)?;
    if !half_dir.is_dir() || generation_dir(&root, "semantic", &half).try_exists()? {
        return Err("the receiver must have only the G7 Lexical seal".into());
    }

    let boot_log = parent.path().join("half-seal.stderr");
    let mut command = boot_command(&binary, &root)?;
    let _configured = command.stderr(File::create(&boot_log)?);
    let daemon = SearchdBinaryProcess::start_with_pinned_command(&root, command)?;
    let client = daemon.connect()?;
    require_boot_counts(&client, 1.0)?;
    if client
        .generations()
        .active_head(first.repo_id.clone(), first.revision_id.clone())?
        != Some(active)
    {
        return Err("the lexical half displaced the original G1 active pair".into());
    }
    let listing = client.quarantine().inventory()?;
    let [entry] = listing.lexical.as_slice() else {
        return Err(format!("expected exactly the lexical half in quarantine: {listing:?}").into());
    };
    if Path::new(&entry.path) != half_dir
        || entry.reason != GenerationQuarantineReasonV1::Orphaned.as_code_str()
        || !listing.semantic.is_empty()
    {
        return Err(format!("the listing did not name exactly the G7 half: {listing:?}").into());
    }
    let ack = client
        .quarantine()
        .discard(&QuarantineTargetV1::Generation(entry.clone()))?;
    if !matches!(ack.outcome, QuarantineDiscardOutcomeDtoV1::Discarded { .. })
        || half_dir.try_exists()?
    {
        return Err(format!("discard did not remove the half: {ack:?}").into());
    }
    drop(client);
    daemon.stop()?;

    let notice = format!(
        "half-sealed pair: repo={} revision={} generation=7 is sealed on the Lexical track only, at {};",
        first.repo_id.as_str(),
        first.revision_id.as_str(),
        half_dir.display()
    );
    if !fs::read_to_string(&boot_log)?.contains(&notice) {
        return Err(format!("the real boot notice did not identify the half: {notice}").into());
    }

    let command = boot_command(&binary, &root)?;
    let daemon = SearchdBinaryProcess::start_with_pinned_command(&root, command)?;
    let client = daemon.connect()?;
    require_boot_counts(&client, 0.0)?;
    let listing = client.quarantine().inventory()?;
    if half_dir.try_exists()? || !listing.lexical.is_empty() || !listing.semantic.is_empty() {
        return Err(format!("the discarded half reappeared after reboot: {listing:?}").into());
    }
    drop(client);
    daemon.stop()?;
    binary.verify()
}
