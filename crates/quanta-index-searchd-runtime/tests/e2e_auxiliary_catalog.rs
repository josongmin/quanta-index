//! QI-BB-020 — the auxiliary authorities live as rows in the catalog:
//! durable before the receipt, restored whole after a restart, and
//! forgotten with the generation retention reaps.
//!
//! Oracles are external: the catalog file read back through the crate
//! that owns it (row counts per generation, never the daemon's own
//! memory), and what a query serves after a restart.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::TextQuerySyntax;
use quanta_index_core::{AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

/// Rows per `(domain, generation)` in the daemon's catalog, read with the
/// daemon stopped so the file is the only authority.
fn rows_by_domain_and_generation(
    rt: &E2eRuntime,
) -> Result<BTreeMap<(AuxiliaryDomainV1, u64), u64>, Box<dyn Error>> {
    let catalog = SqliteCatalog::open(rt.state_root(), Duration::from_millis(500))?;
    let mut counts: BTreeMap<(AuxiliaryDomainV1, u64), u64> = BTreeMap::new();
    catalog.for_each_row(&mut |row| {
        let count = counts
            .entry((row.key.domain, row.key.generation.generation.get()))
            .or_insert(0);
        *count = count.saturating_add(1);
        Ok(())
    })?;
    Ok(counts)
}

/// History, dirty and structural rows are durable before the receipt and
/// serve again after a restart, from the catalog alone.
#[test]
fn auxiliary_rows_survive_a_restart_from_the_catalog() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/aux_restart.rs";
    let content = "fn aux_restart_alpha() { aux_restart_needle }";
    rt.ingest_text("repo", path, content)?;
    rt.ingest_history_fixture(path)?;
    rt.ingest_dirty_for_path(path, 77)?;
    let sealed = rt.seal()?;
    // Parse trees are admitted against the published source chunk universe.
    // The staged harness chunk does not become that authority until seal.
    let tree = E2eRuntime::structural_function_tree_record(path, content, "aux_restart_alpha")?;
    let structural = rt.structural_tree_batch(path, tree, sealed)?;
    rt.publish_structural_batch(structural)?;
    rt.activate_last_sealed_generation()?;

    let before_restart = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 5);
    if let Some(error) = before_restart.typed_error {
        return Err(format!("history must serve before the restart: {error}").into());
    }
    if before_restart.commit_ids.len() != 1 {
        return Err(format!(
            "history must serve its one commit, served {}",
            before_restart.commit_ids.len()
        )
        .into());
    }

    let rt = rt.reopen();
    // With the daemon stopped, the catalog holds every row the receipts
    // acknowledged: the generation's history, runtime and structural rows.
    let counts = rows_by_domain_and_generation(&rt)?;
    for domain in AuxiliaryDomainV1::ALL {
        let rows = counts.get(&(domain, sealed.get())).copied().unwrap_or(0);
        if rows == 0 {
            return Err(format!(
                "{domain} rows for generation {} must be durable, found none in {counts:?}",
                sealed.get()
            )
            .into());
        }
    }

    let mut rt = rt;
    let after_restart = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 5);
    if let Some(error) = after_restart.typed_error {
        return Err(format!("history must serve after the restart: {error}").into());
    }
    if after_restart.commit_ids != before_restart.commit_ids {
        return Err(format!(
            "history restored from rows must serve the same commits: before={:?} after={:?}",
            before_restart.commit_ids, after_restart.commit_ids
        )
        .into());
    }
    let structural = rt.query_structural(
        TextQuerySyntax::Native,
        "aux_restart_alpha AND match { function_item }",
        5,
    );
    if let Some(error) = structural.typed_error {
        return Err(format!("structural must serve after the restart: {error}").into());
    }
    if structural.structural_results.len() != 1 {
        return Err(format!(
            "structural restored from rows must serve its one tree, served {}",
            structural.structural_results.len()
        )
        .into());
    }
    Ok(())
}

/// Generation retention forgets the reaped generations' auxiliary rows
/// and leaves the retained generations'.
#[test]
fn retention_forgets_the_reaped_generations_auxiliary_rows() -> TestResult {
    const KEEP: usize = 2;
    const SEALED: u32 = 4;
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let mut sealed = Vec::new();
    for index in 0..SEALED {
        let path = format!("src/aux_gc_{index}.rs");
        let content = format!("fn aux_gc_{index}() {{ aux_gc_needle_{index} }}");
        rt.ingest_text("repo", &path, &content)?;
        rt.ingest_history_fixture_spec(&e2e_harness::E2eHistoryFixtureSpec {
            commit_sha: &format!("{index:0>40}"),
            file_path: &path,
            author: "alice",
            committer: "alice",
            message: &format!("gc generation {index}"),
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            ref_name: "refs/heads/main",
            tag_name: &format!("v{index}"),
            added_text: "added",
            removed_text: "",
            touched_text: "touched",
        })?;
        rt.ingest_dirty_for_path(&path, 5)?;
        sealed.push(rt.seal()?);
        rt.activate_last_sealed_generation()?;
    }
    let retained: std::collections::BTreeSet<u64> = sealed
        .iter()
        .rev()
        .take(KEEP)
        .map(|generation| generation.get())
        .collect();
    let rt = rt.reopen();
    let counts = rows_by_domain_and_generation(&rt)?;
    let generations: std::collections::BTreeSet<u64> = counts
        .keys()
        .map(|(_domain, generation)| *generation)
        .collect();
    if generations != retained {
        return Err(format!(
            "only the retained generations {retained:?} may keep auxiliary rows, the catalog holds {generations:?}"
        )
        .into());
    }
    for generation in &retained {
        for domain in AuxiliaryDomainV1::ALL {
            if counts.get(&(domain, *generation)).copied().unwrap_or(0) == 0 {
                return Err(format!(
                    "retained generation {generation} must keep its {domain} rows"
                )
                .into());
            }
        }
    }
    Ok(())
}
