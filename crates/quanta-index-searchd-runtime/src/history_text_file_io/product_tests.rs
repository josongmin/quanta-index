use std::collections::BTreeSet;
use std::error::Error;
use std::fs::{self, Metadata};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::Arc;

use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{
    AuxEpochV1, LqExpr, LqLeaf, LqOptions, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, HistoryTextBuildV1, HistoryTextDiscardOutcomeV1, HistoryTextDocKeyV1,
    HistoryTextDocV1, HistoryTextIndexPort as _, HistoryTextKindV1, HistoryTextQueryV1,
    RequestBudgetV1,
};
use quanta_index_lexical::history_text_index::HistoryTextIndexAdapter;
use quanta_index_searchd::app::runtime::StateRootLease;

use super::CanonicalHistoryTextFileAdmissionFactory;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

fn document(byte: u8, text: &str) -> HistoryTextDocV1 {
    HistoryTextDocV1 {
        key: HistoryTextDocKeyV1::Commit {
            sha: CommitSha::from_bytes([byte; 20]),
        },
        committer_time_ms: u64::from(byte),
        text: text.into(),
    }
}

pub(super) fn disk_census(root: &Path) -> Result<(u64, u64), Box<dyn Error>> {
    if !root.exists() {
        return Ok((0, 0));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut seen = BTreeSet::new();
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata: Metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                if seen.insert((metadata.dev(), metadata.ino())) {
                    bytes = bytes
                        .checked_add(metadata.len())
                        .ok_or("disk byte overflow")?;
                    files = files.checked_add(1).ok_or("disk file overflow")?;
                }
            } else {
                return Err("history census encountered a non-file entry".into());
            }
        }
    }
    Ok((bytes, files))
}

#[test]
fn real_history_full_incremental_reopen_query_and_retire_match_disk_census() -> TestResult {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let held = lease.try_clone_root_directory_v1()?;
    let authority: PublicationCapacity = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 67_108_864,
            max_files: 2048,
            max_pending: 2048,
            max_entries: 4096,
            max_depth: 16,
            max_pending_frames: 128,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 64, 4096, 4096)?;
    let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
    let factory = Arc::new(CanonicalHistoryTextFileAdmissionFactory::new(
        deletions, held, 16_777_216, 512, 1024,
    )?);
    let history = root.join("authorities").join("history-text");
    let boot = authority.snapshot()?;
    let adapter = HistoryTextIndexAdapter::with_file_admission(history.clone(), factory.clone())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let _full = adapter.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full {
            docs: vec![document(1, "needle alpha"), document(2, "beta")],
        },
        &RequestBudgetV1::unbounded(),
    )?;
    let _incremental = adapter.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        HistoryTextBuildV1::Incremental {
            base: AuxEpochV1::new(1),
            upserts: vec![document(2, "needle beta")],
        },
        &RequestBudgetV1::unbounded(),
    )?;
    drop(adapter);
    let (physical_bytes, physical_files) = disk_census(&history)?;
    let charged = authority.snapshot()?;
    if physical_bytes == 0
        || physical_files == 0
        || charged.used_bytes
            != boot
                .used_bytes
                .checked_add(physical_bytes)
                .ok_or("byte overflow")?
        || charged.used_files
            != boot
                .used_files
                .checked_add(physical_files)
                .ok_or("file overflow")?
    {
        return Err("published history ledger differs from independent inode census".into());
    }

    let reopened = HistoryTextIndexAdapter::with_file_admission(history.clone(), factory)?;
    if reopened.durable_epochs(&generation)? != vec![AuxEpochV1::new(1), AuxEpochV1::new(2)] {
        return Err("cold history reopen lost one published epoch".into());
    }
    let query = HistoryTextQueryV1 {
        kind: HistoryTextKindV1::Commit,
        expr: LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
        options: LqOptions::defaults(),
    };
    let searcher = reopened.open_epoch(&generation, AuxEpochV1::new(2))?;
    let page = searcher.search(
        &query,
        None,
        2,
        Arc::new(|_hit| Ok(true)),
        &RequestBudgetV1::unbounded(),
    )?;
    let alpha = HistoryTextDocKeyV1::Commit {
        sha: CommitSha::from_bytes([1; 20]),
    };
    let beta = HistoryTextDocKeyV1::Commit {
        sha: CommitSha::from_bytes([2; 20]),
    };
    if page.hits.len() != 2
        || page.matched != 2
        || !page.hits.iter().any(|hit| hit.key == alpha)
        || !page.hits.iter().any(|hit| hit.key == beta)
    {
        return Err("cold reopened incremental index did not return both fixed commits".into());
    }
    drop(searcher);
    if !matches!(
        reopened.discard_generation(&generation, None)?,
        HistoryTextDiscardOutcomeV1::Discarded { bytes } if bytes > 0
    ) {
        return Err("history generation retirement returned no removed bytes".into());
    }
    drop(reopened);
    if disk_census(&history)? != (0, 0)
        || authority.snapshot()? != boot
        || !binder.is_quiescent()?
    {
        return Err("retired history generation did not restore the boot ledger".into());
    }
    Ok(())
}
