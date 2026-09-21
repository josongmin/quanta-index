//! G0-L — Tantivy snapshot reuse probe (W0 decision gate).
//!
//! The structural plan may only move the lexical lane off whole-directory
//! generation copies if Tantivy itself supports the properties that make reuse
//! safe. This target answers that with a real index rather than API reading:
//!
//! 1. Are committed segment files immutable? (a reused file must never be
//!    rewritten under a generation that still points at it)
//! 2. Can a generation be materialized by hard-linking a base generation's
//!    files, then mutated, without touching the base bytes?
//! 3. Does a searcher pinned before a delete/commit/GC keep serving its
//!    snapshot?
//! 4. Does a hard-linked base + delta produce the same ranking as an
//!    independent full rebuild of the identical final corpus?
//!
//! (4) is the DA-06 obligation: an incremental result is only admissible when
//! it is checked against an independently built oracle, not against itself.
//!
//! This probe uses a minimal local schema on purpose. It measures the vendor's
//! guarantees, not the production adapter's schema choices, so an adapter
//! change cannot silently turn the gate green or red.
//!
//! The probe prints a machine-readable `G0L-EVIDENCE` line per test. Run with
//! `-- --nocapture` to capture it into the gate ADR.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use tantivy::collector::TopDocs;
use tantivy::query::TermQuery;
use tantivy::schema::{
    Field, IndexRecordOption, OwnedValue, STORED, STRING, Schema, TEXT, TantivyDocument, Value,
};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Searcher, Term};

type ProbeResult = Result<(), Box<dyn Error>>;

const WRITER_BUDGET_BYTES: usize = 15_000_000;
const BASE_DOC_COUNT: usize = 2_000;
/// Term carried by every document, so ranking depends on corpus-wide statistics.
const SHARED_TERM: &str = "quartz";

struct ProbeSchema {
    schema: Schema,
    doc_id: Field,
    body: Field,
}

impl ProbeSchema {
    fn build() -> Self {
        let mut builder = Schema::builder();
        let doc_id = builder.add_text_field("doc_id", STRING | STORED);
        let body = builder.add_text_field("body", TEXT);
        Self {
            schema: builder.build(),
            doc_id,
            body,
        }
    }

    fn open(&self, path: &Path) -> Result<Index, Box<dyn Error>> {
        std::fs::create_dir_all(path)?;
        let directory = tantivy::directory::MmapDirectory::open(path)?;
        Ok(Index::builder()
            .schema(self.schema.clone())
            .open_or_create(directory)?)
    }

    fn writer(&self, index: &Index) -> Result<IndexWriter, Box<dyn Error>> {
        Ok(index.writer(WRITER_BUDGET_BYTES)?)
    }

    fn document(&self, doc_id: &str, body: &str) -> TantivyDocument {
        let mut doc = TantivyDocument::new();
        doc.add_text(self.doc_id, doc_id);
        doc.add_text(self.body, body);
        doc
    }
}

/// Deterministic body text; `repeats` controls the shared term's frequency so
/// BM25 has something to rank on.
fn body_for(index: usize, repeats: usize) -> String {
    let mut text = format!("doc_{index:05} alpha beta gamma ");
    for _ in 0..repeats {
        text.push_str(SHARED_TERM);
        text.push(' ');
    }
    text.push_str("delta epsilon");
    text
}

fn doc_id_for(index: usize) -> String {
    format!("d{index:05}")
}

/// Every document repeats the shared term a different number of times, so the
/// BM25 ordering is a real signal rather than a tie.
fn repeats_for(index: usize) -> usize {
    index
        .checked_rem(7)
        .map_or(1, |slot| slot.saturating_add(1))
}

fn seed_base_corpus(probe: &ProbeSchema, writer: &IndexWriter) -> Result<(), Box<dyn Error>> {
    for index in 0..BASE_DOC_COUNT {
        let _opstamp = writer.add_document(
            probe.document(&doc_id_for(index), &body_for(index, repeats_for(index))),
        )?;
    }
    Ok(())
}

fn reader_for(index: &Index) -> Result<IndexReader, Box<dyn Error>> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()?;
    reader.reload()?;
    Ok(reader)
}

fn stored_doc_id(
    searcher: &Searcher,
    probe: &ProbeSchema,
    address: DocAddress,
) -> Result<String, Box<dyn Error>> {
    let doc: TantivyDocument = searcher.doc(address)?;
    let value: &OwnedValue = doc
        .get_first(probe.doc_id)
        .ok_or("probe document is missing its stored doc_id")?;
    Value::as_str(&value)
        .map(str::to_owned)
        .ok_or_else(|| "probe doc_id field is not text".into())
}

/// Top-`limit` `(doc_id, score)` rows for the shared term, in ranked order.
fn ranked_shared_term(
    searcher: &Searcher,
    probe: &ProbeSchema,
    limit: usize,
) -> Result<Vec<(String, f32)>, Box<dyn Error>> {
    let query = TermQuery::new(
        Term::from_field_text(probe.body, SHARED_TERM),
        IndexRecordOption::WithFreqs,
    );
    let hits = searcher.search(&query, &TopDocs::with_limit(limit))?;
    let mut ranked = Vec::with_capacity(hits.len());
    for (score, address) in hits {
        ranked.push((stored_doc_id(searcher, probe, address)?, score));
    }
    Ok(ranked)
}

/// Describes the first way two ranked lists disagree, or `None` if they agree.
///
/// Scores are compared with a tolerance rather than for bit equality: BM25 is
/// computed in floating point and the question this gate asks is whether the
/// ranking is preserved, not whether two f32 bit patterns match.
fn ranking_divergence(left: &[(String, f32)], right: &[(String, f32)]) -> Option<String> {
    const SCORE_TOLERANCE: f32 = 1e-5;
    if left.len() != right.len() {
        return Some(format!("row count {} vs {}", left.len(), right.len()));
    }
    for (rank, (left_row, right_row)) in left.iter().zip(right.iter()).enumerate() {
        if left_row.0 != right_row.0 {
            return Some(format!("rank {rank}: doc `{}` vs `{}`", left_row.0, right_row.0));
        }
        let difference = (left_row.1 - right_row.1).abs();
        if difference > SCORE_TOLERANCE {
            return Some(format!(
                "rank {rank} doc `{}`: score {} vs {} (delta {difference})",
                left_row.0, left_row.1, right_row.1
            ));
        }
    }
    None
}

fn term_hit_count(searcher: &Searcher, field: Field, value: &str) -> Result<usize, Box<dyn Error>> {
    let query = TermQuery::new(Term::from_field_text(field, value), IndexRecordOption::Basic);
    Ok(searcher.search(&query, &TopDocs::with_limit(64))?.len())
}

/// One directory entry as the probe judges immutability: identity, size, bytes.
///
/// Link count is deliberately absent. Hard-linking a base into a new
/// generation raises `nlink` on the base's own inodes, which is not a content
/// change; including it would make the probe report every successful reuse as
/// a base mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFacts {
    inode: u64,
    len: u64,
    digest: String,
}

fn file_digest(path: &Path) -> Result<String, Box<dyn Error>> {
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(&bytes);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        write!(&mut encoded, "{byte:02x}")?;
    }
    Ok(encoded)
}

fn inventory(root: &Path) -> Result<BTreeMap<String, FileFacts>, Box<dyn Error>> {
    let mut facts = BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let _prior = facts.insert(
            name,
            FileFacts {
                inode: metadata.ino(),
                len: metadata.len(),
                digest: file_digest(&entry.path())?,
            },
        );
    }
    Ok(facts)
}

/// Files Tantivy owns as mutable bookkeeping rather than immutable segment data.
///
/// `meta.json` is rewritten on every commit; `.managed.json` is the managed
/// file list and is rewritten whenever the file set changes; `.tantivy-*` are
/// lock files. Everything else is a candidate for cross-generation reuse,
/// which is exactly what this gate is deciding.
fn is_mutable_bookkeeping(name: &str) -> bool {
    matches!(name, "meta.json" | ".managed.json") || name.starts_with(".tantivy")
}

fn hard_link_tree(source: &Path, target: &Path) -> Result<u64, Box<dyn Error>> {
    std::fs::create_dir_all(target)?;
    let mut linked_bytes = 0_u64;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        std::fs::hard_link(entry.path(), target.join(entry.file_name()))?;
        linked_bytes = linked_bytes.saturating_add(metadata.len());
    }
    Ok(linked_bytes)
}

/// Bytes in `root` that are not hard links to an inode already present in `base`.
/// Bytes and file count in `root` that are not hard links into `base_inodes`.
fn fresh_against(
    root: &Path,
    base_inodes: &BTreeSet<u64>,
) -> Result<(u64, usize, usize), Box<dyn Error>> {
    let mut fresh_bytes = 0_u64;
    let mut fresh_files = 0_usize;
    let mut shared_files = 0_usize;
    for (_name, facts) in inventory(root)? {
        if base_inodes.contains(&facts.inode) {
            shared_files = shared_files.saturating_add(1);
        } else {
            fresh_bytes = fresh_bytes.saturating_add(facts.len);
            fresh_files = fresh_files.saturating_add(1);
        }
    }
    Ok((fresh_bytes, fresh_files, shared_files))
}

/// `part` as a whole-percent share of `whole`, or `"unknown"` when undefined.
fn percent_of(part: u64, whole: u64) -> String {
    let Some(scaled) = part.checked_mul(100) else {
        return "unknown".to_string();
    };
    if whole == 0 {
        return "unknown".to_string();
    }
    scaled
        .checked_div(whole)
        .map_or_else(|| "unknown".to_string(), |pct| pct.to_string())
}

fn total_bytes(root: &Path) -> Result<u64, Box<dyn Error>> {
    let mut total = 0_u64;
    for (_name, facts) in inventory(root)? {
        total = total.saturating_add(facts.len);
    }
    Ok(total)
}

#[expect(
    clippy::print_stdout,
    reason = "the probe's whole purpose is to emit machine-readable gate evidence for the G0-L ADR"
)]
fn evidence(label: &str, fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("G0L-EVIDENCE {label} {}", rendered.join(" "));
}

/// (1) Committed segment files must not be rewritten by a later commit.
#[test]
fn committed_segment_files_are_immutable_across_later_commits() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let probe = ProbeSchema::build();
    let index = probe.open(dir.path())?;
    let mut writer = probe.writer(&index)?;
    seed_base_corpus(&probe, &writer)?;
    let _first = writer.commit()?;
    let before = inventory(dir.path())?;

    let _added = writer.add_document(probe.document("d99999", &body_for(99_999, 3)))?;
    let _second = writer.commit()?;
    let after = inventory(dir.path())?;

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
        if current.digest != prior.digest
            || current.len != prior.len
            || current.inode != prior.inode
        {
            rewritten.push(name.clone());
        }
    }

    let introduced = after
        .keys()
        .filter(|name| !before.contains_key(*name))
        .count();
    evidence(
        "immutability",
        &[
            ("files_before", before.len().to_string()),
            ("files_after", after.len().to_string()),
            ("retained_segment_files", retained.to_string()),
            ("rewritten_segment_files", rewritten.len().to_string()),
            ("introduced_files", introduced.to_string()),
        ],
    );

    if retained == 0 {
        return Err("probe is vacuous: no segment file survived the second commit".into());
    }
    if !rewritten.is_empty() {
        return Err(
            format!("tantivy rewrote committed segment files in place: {rewritten:?}").into()
        );
    }
    Ok(())
}

/// (2) A hard-linked base must support a delta without touching base bytes.
#[test]
fn hard_linked_base_supports_a_delta_without_rewriting_base_bytes() -> ProbeResult {
    let root = tempfile::tempdir()?;
    let base_path: PathBuf = root.path().join("g1");
    let delta_path: PathBuf = root.path().join("g2");
    let probe = ProbeSchema::build();

    let base_index = probe.open(&base_path)?;
    let mut base_writer = probe.writer(&base_index)?;
    seed_base_corpus(&probe, &base_writer)?;
    let _committed = base_writer.commit()?;
    base_writer.wait_merging_threads()?;

    let base_before = inventory(&base_path)?;
    let base_inodes: BTreeSet<u64> = base_before.values().map(|facts| facts.inode).collect();
    let base_reader = reader_for(&base_index)?;
    let base_ranked = ranked_shared_term(&base_reader.searcher(), &probe, 10)?;
    let base_total = total_bytes(&base_path)?;

    let linked_bytes = hard_link_tree(&base_path, &delta_path)?;

    // Mutate the delta generation only: retire d00007, introduce d90001.
    let delta_index = probe.open(&delta_path)?;
    let mut delta_writer = probe.writer(&delta_index)?;
    let _deleted = delta_writer.delete_term(Term::from_field_text(probe.doc_id, &doc_id_for(7)));
    let _added =
        delta_writer.add_document(probe.document("d90001", &body_for(90_001, repeats_for(3))))?;
    let _committed = delta_writer.commit()?;
    delta_writer.wait_merging_threads()?;

    let base_after = inventory(&base_path)?;
    let (fresh_bytes, fresh_files, shared_files) = fresh_against(&delta_path, &base_inodes)?;
    let delta_total = total_bytes(&delta_path)?;

    evidence(
        "hardlink_delta",
        &[
            ("base_total_bytes", base_total.to_string()),
            ("linked_bytes", linked_bytes.to_string()),
            ("delta_total_bytes", delta_total.to_string()),
            ("delta_fresh_bytes", fresh_bytes.to_string()),
            ("delta_fresh_files", fresh_files.to_string()),
            ("delta_files_shared_with_base", shared_files.to_string()),
            ("delta_fresh_ratio_pct", percent_of(fresh_bytes, base_total)),
        ],
    );

    if base_after != base_before {
        let mut changed: Vec<String> = Vec::new();
        for (name, prior) in &base_before {
            if base_after.get(name).map(|current| &current.digest) != Some(&prior.digest) {
                changed.push(name.clone());
            }
        }
        return Err(
            format!("delta generation mutated base generation bytes: changed={changed:?}").into()
        );
    }

    // The base must still answer exactly as before, including the deleted doc.
    let base_reader_after = reader_for(&probe.open(&base_path)?)?;
    let base_searcher_after = base_reader_after.searcher();
    if term_hit_count(&base_searcher_after, probe.doc_id, &doc_id_for(7))? != 1 {
        return Err("base generation lost the document that only the delta retired".into());
    }
    let base_ranked_after = ranked_shared_term(&base_searcher_after, &probe, 10)?;
    if let Some(divergence) = ranking_divergence(&base_ranked, &base_ranked_after) {
        return Err(format!(
            "base generation ranking changed after the delta generation committed: {divergence}"
        )
        .into());
    }

    // The delta must reflect its own mutations.
    let delta_reader = reader_for(&delta_index)?;
    let delta_searcher = delta_reader.searcher();
    if term_hit_count(&delta_searcher, probe.doc_id, &doc_id_for(7))? != 0 {
        return Err("delta generation still serves the retired document".into());
    }
    if term_hit_count(&delta_searcher, probe.doc_id, "d90001")? != 1 {
        return Err("delta generation does not serve its newly added document".into());
    }
    if fresh_bytes >= base_total {
        return Err(format!(
            "hard-linked delta wrote {fresh_bytes} fresh bytes against a {base_total}-byte base: no reuse"
        )
        .into());
    }
    if shared_files == 0 {
        return Err("no delta file is a hard link into the base: the probe proved nothing".into());
    }
    Ok(())
}

/// (3) A pinned searcher must survive delete + commit + garbage collection.
#[test]
fn pinned_searcher_survives_delete_commit_and_garbage_collection() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let probe = ProbeSchema::build();
    let index = probe.open(dir.path())?;
    let mut writer = probe.writer(&index)?;
    seed_base_corpus(&probe, &writer)?;
    let _committed = writer.commit()?;

    let pinned_reader = reader_for(&index)?;
    let pinned = pinned_reader.searcher();
    let pinned_ranked = ranked_shared_term(&pinned, &probe, 10)?;
    let pinned_docs = pinned.num_docs();

    let _deleted = writer.delete_term(Term::from_field_text(probe.doc_id, &doc_id_for(7)));
    let _committed = writer.commit()?;
    let collected = writer.garbage_collect_files().wait()?;
    writer.wait_merging_threads()?;

    let pinned_ranked_after = ranked_shared_term(&pinned, &probe, 10)?;
    let pinned_divergence = ranking_divergence(&pinned_ranked, &pinned_ranked_after);
    evidence(
        "pinned_reader",
        &[
            ("pinned_num_docs", pinned_docs.to_string()),
            ("pinned_num_docs_after_gc", pinned.num_docs().to_string()),
            ("gc_deleted_files", collected.deleted_files.len().to_string()),
        ],
    );

    if let Some(divergence) = pinned_divergence {
        return Err(format!(
            "pinned searcher observed a different ranking after delete/commit/GC: {divergence}"
        )
        .into());
    }
    if term_hit_count(&pinned, probe.doc_id, &doc_id_for(7))? != 1 {
        return Err("pinned searcher lost a document that was deleted after it was pinned".into());
    }
    Ok(())
}

/// (4) Hard-linked base + delta must match an independent full rebuild.
///
/// DA-06: an incremental commitment is only admissible against an oracle built
/// independently from the same final logical corpus.
#[test]
fn hard_linked_delta_matches_an_independent_full_rebuild() -> ProbeResult {
    let root = tempfile::tempdir()?;
    let probe = ProbeSchema::build();

    // Incremental: base then hard-linked delta.
    let base_path = root.path().join("incremental-g1");
    let delta_path = root.path().join("incremental-g2");
    let base_index = probe.open(&base_path)?;
    let mut base_writer = probe.writer(&base_index)?;
    seed_base_corpus(&probe, &base_writer)?;
    let _committed = base_writer.commit()?;
    base_writer.wait_merging_threads()?;
    let _linked = hard_link_tree(&base_path, &delta_path)?;
    let delta_index = probe.open(&delta_path)?;
    let mut delta_writer = probe.writer(&delta_index)?;
    let _deleted = delta_writer.delete_term(Term::from_field_text(probe.doc_id, &doc_id_for(7)));
    let _added =
        delta_writer.add_document(probe.document("d90001", &body_for(90_001, repeats_for(3))))?;
    let _committed = delta_writer.commit()?;
    delta_writer.wait_merging_threads()?;

    // Oracle: one build of the identical final corpus, no reuse.
    let oracle_path = root.path().join("oracle");
    let oracle_index = probe.open(&oracle_path)?;
    let mut oracle_writer = probe.writer(&oracle_index)?;
    for index in 0..BASE_DOC_COUNT {
        if index == 7 {
            continue;
        }
        let _op = oracle_writer.add_document(
            probe.document(&doc_id_for(index), &body_for(index, repeats_for(index))),
        )?;
    }
    let _op =
        oracle_writer.add_document(probe.document("d90001", &body_for(90_001, repeats_for(3))))?;
    let _committed = oracle_writer.commit()?;
    oracle_writer.wait_merging_threads()?;

    let delta_searcher = reader_for(&delta_index)?.searcher();
    let oracle_searcher = reader_for(&oracle_index)?.searcher();

    let delta_ranked = ranked_shared_term(&delta_searcher, &probe, 25)?;
    let oracle_ranked = ranked_shared_term(&oracle_searcher, &probe, 25)?;

    let delta_ids: Vec<&String> = delta_ranked.iter().map(|(id, _)| id).collect();
    let oracle_ids: Vec<&String> = oracle_ranked.iter().map(|(id, _)| id).collect();

    let mut max_score_delta = 0.0_f32;
    for ((_, left), (_, right)) in delta_ranked.iter().zip(oracle_ranked.iter()) {
        let difference = (left - right).abs();
        if difference > max_score_delta {
            max_score_delta = difference;
        }
    }

    evidence(
        "incremental_vs_full_rebuild",
        &[
            ("delta_num_docs", delta_searcher.num_docs().to_string()),
            ("oracle_num_docs", oracle_searcher.num_docs().to_string()),
            ("ranked_rows", delta_ranked.len().to_string()),
            ("max_abs_score_delta", format!("{max_score_delta:.6}")),
            ("ranking_identical", (delta_ids == oracle_ids).to_string()),
        ],
    );

    if delta_searcher.num_docs() != oracle_searcher.num_docs() {
        return Err(format!(
            "incremental doc count {} != independent rebuild {}",
            delta_searcher.num_docs(),
            oracle_searcher.num_docs()
        )
        .into());
    }
    if delta_ids != oracle_ids {
        return Err(format!(
            "incremental ranking diverged from the independent rebuild:\n  incremental={delta_ids:?}\n  oracle={oracle_ids:?}"
        )
        .into());
    }
    Ok(())
}
