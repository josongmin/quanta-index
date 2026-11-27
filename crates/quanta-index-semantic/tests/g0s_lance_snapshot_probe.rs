//! G0-S — Lance snapshot reuse probe (W0 decision gate).
//!
//! The semantic lane currently materializes a delta generation by byte-copying
//! the whole base dataset (`prepare_staging_dataset` -> `copy_dir`). The
//! structural plan may only replace that with reuse if `LanceDB` supports it.
//! This target answers the question against a real dataset:
//!
//! 1. Are dataset files immutable across versions? (a reused file must never be
//!    rewritten under a generation that still points at it)
//! 2. Can a generation be materialized by hard-linking a base dataset, then
//!    mutated, without touching the base bytes?
//! 3. After pruning, does a pinned old version fail closed rather than
//!    silently serving a different row set?
//!
//! The retired LanceDB Table checkout mutation rule is tested separately in
//! the isolated `tools/lance-upstream-oracle` package.
//!
//! ANN recall under delete/refill is deliberately not probed here: the semantic
//! owner tests already hold an exhaustive exact-cosine oracle, and mixing a
//! quality threshold into a storage-capability gate would let one mask the
//! other.
//!
//! Emits `G0S-EVIDENCE` lines; run with `-- --nocapture` for the gate ADR.

#![forbid(unsafe_code)]
// The probe owns a current-thread runtime for direct Lance inspection.
#![expect(
    clippy::future_not_send,
    reason = "the probe owns a current-thread runtime and never moves dataset futures across threads"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::types::Float32Type;
use arrow_array::{Array as _, FixedSizeListArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use lance::Dataset;
use lance::dataset::{builder::DatasetBuilder, cleanup::CleanupPolicyBuilder};
use sha2::{Digest as _, Sha256};

type ProbeResult = Result<(), Box<dyn Error>>;

const DIMENSION: i32 = 8;
const TABLE_NAME: &str = "probe";
const BASE_ROW_COUNT: usize = 512;

fn probe_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("row_id", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                DIMENSION,
            ),
            false,
        ),
    ]))
}

fn row_id_for(index: usize) -> String {
    format!("r{index:05}")
}

fn delete_row_predicate(index: usize) -> String {
    format!("row_id = '{}'", row_id_for(index))
}

/// Deterministic unit-ish vector; distinct per row so nearest-neighbour order
/// is well defined.
fn vector_for(index: usize) -> Vec<Option<f32>> {
    (0..DIMENSION)
        .map(|slot| {
            let slot_usize = usize::try_from(slot).unwrap_or(0);
            let raw = index
                .wrapping_mul(31)
                .wrapping_add(slot_usize.wrapping_mul(7));
            let bounded = raw.checked_rem(997).unwrap_or(0);
            let scaled = f64::from(u32::try_from(bounded).unwrap_or(0)) / 997.0;
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "probe vectors only need f32 precision; the f64 intermediate keeps the division exact enough for deterministic ordering, and f64->f32 has no checked form"
            )]
            Some(scaled as f32)
        })
        .collect()
}

fn batch_for(indices: &[usize]) -> Result<RecordBatch, Box<dyn Error>> {
    let ids: Vec<String> = indices.iter().map(|index| row_id_for(*index)).collect();
    let vectors: Vec<Option<Vec<Option<f32>>>> = indices
        .iter()
        .map(|index| Some(vector_for(*index)))
        .collect();
    Ok(RecordBatch::try_new(
        probe_schema(),
        vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(
                FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(vectors, DIMENSION),
            ),
        ],
    )?)
}

fn runtime() -> Result<tokio::runtime::Runtime, Box<dyn Error>> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn dataset_uri(path: &Path) -> Result<String, Box<dyn Error>> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "probe dataset path is not valid UTF-8".into())
}

async fn create_base_table(path: &Path) -> Result<Dataset, Box<dyn Error>> {
    std::fs::create_dir_all(path)?;
    let uri = dataset_uri(&path.join(format!("{TABLE_NAME}.lance")))?;
    let indices: Vec<usize> = (0..BASE_ROW_COUNT).collect();
    let batch = batch_for(&indices)?;
    let reader = arrow_array::RecordBatchIterator::new(std::iter::once(Ok(batch)), probe_schema());
    Ok(Dataset::write(reader, uri.as_str(), None).await?)
}

async fn open_table(path: &Path) -> Result<Dataset, Box<dyn Error>> {
    let uri = dataset_uri(&path.join(format!("{TABLE_NAME}.lance")))?;
    Ok(DatasetBuilder::from_uri(&uri).load().await?)
}

/// Row ids present in the table, sorted. The independent oracle for every
/// membership assertion in this probe.
async fn live_row_ids(table: &Dataset) -> Result<Vec<String>, Box<dyn Error>> {
    use futures::TryStreamExt as _;

    let mut stream = table.scan().try_into_stream().await?;
    let mut ids: Vec<String> = Vec::new();
    while let Some(batch) = stream.try_next().await? {
        let column = batch
            .column_by_name("row_id")
            .ok_or("probe batch is missing row_id")?;
        let values = column
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or("probe row_id column is not Utf8")?;
        for slot in 0..values.len() {
            if values.is_valid(slot) {
                ids.push(values.value(slot).to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFacts {
    inode: u64,
    len: u64,
    digest: String,
}

fn file_digest(path: &Path) -> Result<String, Box<dyn Error>> {
    let digest = Sha256::digest(std::fs::read(path)?);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        write!(&mut encoded, "{byte:02x}")?;
    }
    Ok(encoded)
}

/// Recursive inventory keyed by path relative to `root`.
fn inventory(root: &Path) -> Result<BTreeMap<String, FileFacts>, Box<dyn Error>> {
    let mut facts = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|err| format!("probe inventory path escape: {err}"))?;
            let _prior = facts.insert(
                relative,
                FileFacts {
                    inode: metadata.ino(),
                    len: metadata.len(),
                    digest: file_digest(&entry.path())?,
                },
            );
        }
    }
    Ok(facts)
}

/// Files `LanceDB` owns as mutable bookkeeping rather than immutable data.
///
/// `_versions/latest_version_hint.json` is rewritten on every commit to point
/// at the newest manifest. The versioned manifests beside it, and every data
/// file, are the immutable content this gate is deciding about.
fn is_mutable_bookkeeping(relative_path: &str) -> bool {
    relative_path.ends_with("latest_version_hint.json")
}

fn hard_link_tree(source: &Path, target: &Path) -> Result<u64, Box<dyn Error>> {
    std::fs::create_dir_all(target)?;
    let mut linked = 0_u64;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let destination = target.join(entry.file_name());
        if metadata.is_dir() {
            linked = linked.saturating_add(hard_link_tree(&entry.path(), &destination)?);
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        std::fs::hard_link(entry.path(), &destination)?;
        linked = linked.saturating_add(metadata.len());
    }
    Ok(linked)
}

/// Bytes and file counts in `root` that are, or are not, hard links into `base_inodes`.
fn fresh_against(
    root: &Path,
    base_inodes: &BTreeSet<u64>,
) -> Result<(u64, usize, usize), Box<dyn Error>> {
    let mut fresh_bytes = 0_u64;
    let mut fresh_files = 0_usize;
    let mut shared_files = 0_usize;
    for facts in inventory(root)?.values() {
        if base_inodes.contains(&facts.inode) {
            shared_files = shared_files.saturating_add(1);
        } else {
            fresh_bytes = fresh_bytes.saturating_add(facts.len);
            fresh_files = fresh_files.saturating_add(1);
        }
    }
    Ok((fresh_bytes, fresh_files, shared_files))
}

fn total_bytes(root: &Path) -> Result<u64, Box<dyn Error>> {
    Ok(inventory(root)?
        .values()
        .fold(0_u64, |total, facts| total.saturating_add(facts.len)))
}

#[expect(
    clippy::print_stdout,
    reason = "the probe's whole purpose is to emit machine-readable gate evidence for the G0-S ADR"
)]
fn evidence(label: &str, fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("G0S-EVIDENCE {label} {}", rendered.join(" "));
}

/// (1) Files written by an earlier version must not be rewritten by a later one.
#[test]
fn lance_dataset_files_are_immutable_across_versions() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let dataset = dir.path().join("dataset");
    runtime()?.block_on(async {
        let mut table = create_base_table(&dataset).await?;
        let first_version = table.version().version;
        let before = inventory(&dataset)?;

        let batch = batch_for(&[90_001, 90_002, 90_003])?;
        let reader =
            arrow_array::RecordBatchIterator::new(std::iter::once(Ok(batch)), probe_schema());
        table.append(reader, None).await?;
        let _deleted = table.delete(delete_row_predicate(7).as_str()).await?;
        let second_version = table.version().version;
        let after = inventory(&dataset)?;

        let mut rewritten: Vec<String> = Vec::new();
        let mut retained = 0_usize;
        for (name, prior) in &before {
            if is_mutable_bookkeeping(name) {
                continue;
            }
            let Some(current) = after.get(name) else {
                continue;
            };
            retained = retained.saturating_add(1);
            if current != prior {
                rewritten.push(name.clone());
            }
        }

        evidence(
            "immutability",
            &[
                ("first_version", first_version.to_string()),
                ("second_version", second_version.to_string()),
                ("files_before", before.len().to_string()),
                ("files_after", after.len().to_string()),
                ("retained_files", retained.to_string()),
                ("rewritten_files", rewritten.len().to_string()),
            ],
        );

        if retained == 0 {
            return Err("probe is vacuous: no file survived the second version".into());
        }
        if !rewritten.is_empty() {
            return Err(format!(
                "Lance rewrote files that an earlier version still names: {rewritten:?}"
            )
            .into());
        }
        Ok::<(), Box<dyn Error>>(())
    })
}

/// (2) A hard-linked base must support a delta without touching base bytes.
#[test]
fn hard_linked_lance_base_supports_a_delta_without_rewriting_base_bytes() -> ProbeResult {
    let root = tempfile::tempdir()?;
    let base: PathBuf = root.path().join("g1");
    let delta: PathBuf = root.path().join("g2");
    runtime()?.block_on(async {
        let base_table = create_base_table(&base).await?;
        let base_ids_before = live_row_ids(&base_table).await?;
        let base_inventory_before = inventory(&base)?;
        let base_inodes: BTreeSet<u64> =
            base_inventory_before.values().map(|facts| facts.inode).collect();
        let base_total = total_bytes(&base)?;
        drop(base_table);

        let linked = hard_link_tree(&base, &delta)?;

        let mut delta_table = open_table(&delta).await?;
        let _deleted = delta_table
            .delete(delete_row_predicate(7).as_str())
            .await?;
        let batch = batch_for(&[90_001])?;
        let reader = arrow_array::RecordBatchIterator::new(
            std::iter::once(Ok(batch)),
            probe_schema(),
        );
        delta_table.append(reader, None).await?;

        let base_inventory_after = inventory(&base)?;
        let (fresh, fresh_files, shared_files) = fresh_against(&delta, &base_inodes)?;

        evidence(
            "hardlink_delta",
            &[
                ("base_total_bytes", base_total.to_string()),
                ("linked_bytes", linked.to_string()),
                ("delta_total_bytes", total_bytes(&delta)?.to_string()),
                ("delta_fresh_bytes", fresh.to_string()),
                ("delta_fresh_files", fresh_files.to_string()),
                ("delta_files_shared_with_base", shared_files.to_string()),
            ],
        );

        let changed: Vec<&String> = base_inventory_before
            .keys()
            .filter(|name| !is_mutable_bookkeeping(name))
            .filter(|name| base_inventory_before.get(*name) != base_inventory_after.get(*name))
            .collect();
        if !changed.is_empty() {
            return Err(
                format!("delta generation mutated base dataset bytes: changed={changed:?}").into(),
            );
        }

        let base_table_after = open_table(&base).await?;
        let base_ids_after = live_row_ids(&base_table_after).await?;
        if base_ids_after != base_ids_before {
            return Err("base dataset row set changed after the delta committed".into());
        }

        let delta_ids = live_row_ids(&delta_table).await?;
        if delta_ids.contains(&row_id_for(7)) {
            return Err("delta dataset still serves the row it deleted".into());
        }
        if !delta_ids.contains(&row_id_for(90_001)) {
            return Err("delta dataset does not serve the row it added".into());
        }
        if fresh >= base_total {
            return Err(format!(
                "hard-linked delta wrote {fresh} fresh bytes against a {base_total}-byte base: no reuse"
            )
            .into());
        }
        if shared_files == 0 {
            return Err("no delta file is a hard link into the base: the probe proved nothing".into());
        }
        Ok::<(), Box<dyn Error>>(())
    })
}

/// (3) A pruned version must fail closed, never serve a different row set.
#[test]
fn pruned_version_fails_closed_instead_of_serving_a_different_row_set() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let dataset = dir.path().join("dataset");
    runtime()?.block_on(async {
        let mut table = create_base_table(&dataset).await?;
        let base_version = table.version().version;
        let base_ids = live_row_ids(&table).await?;
        let base_row_count = base_ids.len();

        let _deleted = table.delete(delete_row_predicate(7).as_str()).await?;
        let policy = CleanupPolicyBuilder::default()
            .before_timestamp(table.version().timestamp + std::time::Duration::from_secs(1))
            .delete_unverified(true)
            .error_if_tagged_old_versions(false)
            .build();
        let stats = table.cleanup_with_policy(policy).await?;

        let pruned_versions = stats.old_versions;
        let checkout_attempt = table.checkout_version(base_version).await;
        let post_prune_ids = if let Ok(checked_out) = &checkout_attempt {
            live_row_ids(checked_out).await?
        } else {
            Vec::new()
        };

        evidence(
            "prune_fail_closed",
            &[
                ("base_version", base_version.to_string()),
                ("base_row_count", base_row_count.to_string()),
                ("pruned_old_versions", pruned_versions.to_string()),
                ("checkout_after_prune_ok", checkout_attempt.is_ok().to_string()),
                ("post_prune_row_count", post_prune_ids.len().to_string()),
            ],
        );

        // Either the pinned version survived pruning intact, or the checkout
        // failed. Serving a *different* row set under the same version number
        // is the one outcome that would be a silent-correctness violation.
        if checkout_attempt.is_ok() && post_prune_ids != base_ids {
            return Err(format!(
                "pruned dataset served version {base_version} with different rows: expected {base_ids:?}, got {post_prune_ids:?}"
            )
            .into());
        }
        Ok::<(), Box<dyn Error>>(())
    })
}
