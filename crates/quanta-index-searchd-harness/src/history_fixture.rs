//! Fixed History payloads shared by the DSL benchmark and Native SDK callers.

use anyhow::Result;
use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use quanta_index_contract::{
    DiffHunkSide, HistoryDiffHunkUpsert, HistoryIngestBatch, HistoryRefMutation, HistoryRefUpsert,
    ManifestGeneration, RepoId, RevisionId,
};

/// One commit, ref, tag and diff hunk supplied to a History test fixture.
pub struct HistoryFixtureSpec<'a> {
    pub commit_sha: &'a str,
    pub file_path: &'a str,
    pub author: &'a str,
    pub committer: &'a str,
    pub message: &'a str,
    pub author_time_ms: u64,
    pub committer_time_ms: u64,
    pub applied_at_ms: u64,
    pub ref_name: &'a str,
    pub tag_name: &'a str,
    pub added_text: &'a str,
    pub removed_text: &'a str,
    pub touched_text: &'a str,
}

/// Source bytes accompanying the fixed DSL History commit.
pub const HISTORY_LEDGER_TEXT: &str = "history lexical proof alpha_content_needle\n";

/// History fixture shared by the warm/cold DSL matrix and Native SDK probes.
pub const HISTORY_LEDGER: HistoryFixtureSpec<'static> = HistoryFixtureSpec {
    commit_sha: "0123456789abcdef0123456789abcdef01234567",
    file_path: "src/history.rs",
    author: "alice",
    committer: "alice",
    message: "fix: sample history alpha_content_needle",
    author_time_ms: 11,
    committer_time_ms: 12,
    applied_at_ms: 12,
    ref_name: "refs/heads/main",
    tag_name: "v1.0.0",
    added_text: "history added line",
    removed_text: "history removed line",
    touched_text: "history touched line",
};

/// Canonical source and History payload bytes used by DSL corpus provenance.
/// Keep the virtual History record framing identical to the Legacy fixture.
#[must_use]
pub fn history_fixture_corpus_files() -> Vec<(String, String)> {
    let HistoryFixtureSpec {
        commit_sha,
        file_path,
        author,
        committer,
        message,
        author_time_ms,
        committer_time_ms,
        applied_at_ms,
        ref_name,
        tag_name,
        added_text,
        removed_text,
        touched_text,
    } = HISTORY_LEDGER;
    vec![
        (file_path.to_string(), HISTORY_LEDGER_TEXT.to_string()),
        (
            format!("fixture://history/{commit_sha}"),
            format!(
                "file={file_path} author={author} committer={committer} message={message} author_time_ms={author_time_ms} committer_time_ms={committer_time_ms} applied_at_ms={applied_at_ms} ref={ref_name} tag={tag_name} added={added_text} removed={removed_text} touched={touched_text}"
            ),
        ),
    ]
}

impl HistoryFixtureSpec<'_> {
    /// Build a batch with the given generation identity. The caller stamps it
    /// with the canonical IPC batch digest before submitting it.
    pub fn build_batch(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
    ) -> Result<HistoryIngestBatch> {
        let commit_sha = CommitSha::from_hex(self.commit_sha).map_err(|err| {
            anyhow::anyhow!(
                "e2e-harness: invalid history fixture commit_sha `{}`: {err}",
                self.commit_sha
            )
        })?;
        Ok(HistoryIngestBatch {
            repo_id,
            revision_id,
            generation,
            manifest_digest: Some(format!("history:{}:{}", self.file_path, generation.get())),
            batch_digest: String::new(),
            commits: vec![CommitRecord {
                wire_version: 1,
                sha: commit_sha,
                parents: Vec::new(),
                author_time_ms: self.author_time_ms,
                committer_time_ms: self.committer_time_ms,
                applied_at_ms: self.applied_at_ms,
                author: self.author.to_string().into_boxed_str(),
                author_name: None,
                author_email: None,
                committer: self.committer.to_string().into_boxed_str(),
                committer_name: None,
                committer_email: None,
                message: self.message.to_string().into_boxed_str(),
                is_merge: false,
                tags: vec![self.tag_name.to_string().into_boxed_str()],
            }],
            refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: self.ref_name.to_string().into_boxed_str(),
                sha: commit_sha,
            })],
            tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: self.tag_name.to_string().into_boxed_str(),
                sha: commit_sha,
            })],
            diff_hunks: vec![HistoryDiffHunkUpsert {
                commit_sha,
                file_path: self.file_path.to_string().into_boxed_str(),
                record: DiffHunkRecord {
                    wire_version: 1,
                    hunk_header: "@@ -1 +1 @@".to_string().into_boxed_str(),
                    side: DiffHunkSide::After,
                    added_text: self.added_text.to_string().into_boxed_str(),
                    removed_text: self.removed_text.to_string().into_boxed_str(),
                    touched_text: self.touched_text.to_string().into_boxed_str(),
                    byte_start: 0,
                    byte_end: 20,
                },
            }],
        })
    }
}
