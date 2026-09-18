//! The history text index port (QI-BB-023 follow-up #1, plan §8.3).
//!
//! The history route's `relevance` order ranks commits and diff hunks by
//! a real BM25 score over their text. That score comes from a per-epoch
//! text index the lexical adapter owns: every durable history mutation
//! of one generation publishes the next auxiliary epoch (QI-BB-020 W2)
//! *and* the index of that epoch, so the rows a page reads and the index
//! that scores them are the same immutable snapshot. A continuation is
//! served at its cursor's epoch from that epoch's index, so no ingest can
//! change a score mid-walk.
//!
//! The port is vendor-neutral: the search plane hands over documents and
//! a text expression and receives ranked hits under the relevance total
//! order (score descending, committer time descending, sha ascending,
//! path ascending). What engine builds the index, how its files are
//! laid out, and how unchanged segments are shared between epochs are the
//! adapter's business.

use std::cmp::Ordering;
use std::sync::Arc;

use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{AuxEpochV1, HistoryScoreV1, LqExpr, LqOptions};

use crate::domains::auxiliary::AuxiliaryGenerationKeyV1;
use crate::error::CoreError;
use crate::request_budget::RequestBudgetV1;

/// Wire code for a relevance query whose text expression has no score.
///
/// No expression at all, a raw-string / regex / predicate / structural
/// leaf, a regexp or literal pattern mode, or a negation with no positive
/// clause beside it: relevance scores token leaves (keyword, phrase) and
/// everything else has no BM25 score and is never given a heuristic one.
pub const HISTORY_TEXT_QUERY_UNSCORABLE_CODE: &str = "HISTORY_TEXT_QUERY_UNSCORABLE";

/// Wire code for a relevance read at an epoch that has no text index.
///
/// The generation predates the index or the epoch's index was never
/// published; the next history batch of the generation builds it.
pub const HISTORY_TEXT_INDEX_NOT_READY_CODE: &str = "HISTORY_TEXT_INDEX_NOT_READY";

/// Wire code for an epoch index built under another text normalizer.
///
/// It is never served with mismatched text semantics; the next history
/// batch rebuilds it.
pub const HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE: &str =
    "HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED";

/// Wire code for an epoch index whose manifest does not describe the
/// files on disk.
pub const HISTORY_TEXT_INDEX_CORRUPT_CODE: &str = "HISTORY_TEXT_INDEX_CORRUPT";

/// Which history rows a text index holds.
///
/// The two kinds are separate indexes with separate BM25 statistics, so a
/// commit score is a function of the generation's commits alone and a diff
/// score of its diff hunks alone.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HistoryTextKindV1 {
    Commit,
    Diff,
}

impl HistoryTextKindV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Diff => "diff",
        }
    }
}

/// The identity of one indexed document: the row it scores.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HistoryTextDocKeyV1 {
    Commit { sha: CommitSha },
    Diff { sha: CommitSha, file_path: String },
}

impl HistoryTextDocKeyV1 {
    #[must_use]
    pub const fn kind(&self) -> HistoryTextKindV1 {
        match self {
            Self::Commit { .. } => HistoryTextKindV1::Commit,
            Self::Diff { .. } => HistoryTextKindV1::Diff,
        }
    }

    #[must_use]
    pub const fn sha(&self) -> CommitSha {
        match self {
            Self::Commit { sha } | Self::Diff { sha, .. } => *sha,
        }
    }

    #[must_use]
    pub fn file_path(&self) -> Option<&str> {
        match self {
            Self::Commit { .. } => None,
            Self::Diff { file_path, .. } => Some(file_path.as_str()),
        }
    }

    /// The recency order between two keys of one kind: sha ascending,
    /// then path ascending.
    #[must_use]
    pub fn cmp_recency(&self, other: &Self) -> Ordering {
        self.sha()
            .cmp(&other.sha())
            .then_with(|| self.file_path().cmp(&other.file_path()))
    }
}

/// One document of the history text index: the row's key, the committer
/// time the order breaks score ties by, and the text BM25 scores — the
/// commit message, or the diff hunk's search text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTextDocV1 {
    pub key: HistoryTextDocKeyV1,
    pub committer_time_ms: u64,
    pub text: String,
}

/// How the index of one epoch is produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryTextBuildV1 {
    /// Every document of the generation at this epoch, from nothing: the
    /// first index of a generation, or a rebuild over a base the adapter
    /// cannot serve.
    Full { docs: Vec<HistoryTextDocV1> },
    /// The index of `base` — the epoch this one supersedes — plus these
    /// documents, each replacing the document under its key if any. The
    /// adapter shares `base`'s unchanged storage rather than copying it.
    Incremental {
        base: AuxEpochV1,
        upserts: Vec<HistoryTextDocV1>,
    },
}

/// What publishing an epoch's index did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryTextEpochReceiptV1 {
    /// Documents written into the epoch's index (a full build writes every
    /// document, an incremental one its upserts).
    pub docs_written: u64,
}

/// Whether one epoch's index exists and can be served by this build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryTextEpochStatusV1 {
    /// No index for the epoch.
    Absent,
    /// An index this build serves.
    Servable,
    /// An index built under another text normalizer; this build never
    /// serves it and the next mutation rebuilds from scratch.
    Unsupported { built_with: String },
}

/// What discarding an epoch's (or a generation's) index did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryTextDiscardOutcomeV1 {
    /// Nothing was on disk.
    Absent,
    /// The index was removed; this many bytes went with it.
    Discarded { bytes: u64 },
}

/// The text expression one relevance query scores.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryTextQueryV1 {
    pub kind: HistoryTextKindV1,
    /// Keyword and phrase leaves under `All` / `Any`, with `Not` admitted
    /// only beside a positive sibling; anything else is refused
    /// [`HISTORY_TEXT_QUERY_UNSCORABLE_CODE`].
    pub expr: LqExpr,
    /// The query's options; `case` selects the folded or case-preserving
    /// terms (absent means folded, as on the lexical route) and a
    /// `pattern_type` other than standard / keyword is unscorable.
    pub options: LqOptions,
}

/// One scored hit: the document's key, its committer time, and its score.
///
/// [`Ord`] is the relevance total order — score descending, committer
/// time descending, sha ascending, path ascending — so a page is a sorted
/// run of hits and "after the cursor" is `Greater`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTextHitV1 {
    pub key: HistoryTextDocKeyV1,
    pub committer_time_ms: u64,
    pub score: HistoryScoreV1,
}

impl PartialOrd for HistoryTextHitV1 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HistoryTextHitV1 {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| other.committer_time_ms.cmp(&self.committer_time_ms))
            .then_with(|| self.key.cmp_recency(&other.key))
    }
}

/// One page of hits in relevance order plus what the index saw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTextPageV1 {
    /// At most `limit` hits, in relevance order, all strictly after the
    /// cursor and all admitted by the caller's predicate.
    pub hits: Vec<HistoryTextHitV1>,
    /// Documents the index visited for the expression, cursor and
    /// predicate included.
    pub examined: u64,
    /// The exact number of documents after the cursor the predicate
    /// admitted; `hits.len() < matched` means a next page exists.
    pub matched: u64,
}

/// Which visited documents belong on the page.
///
/// The index scores the expression; every other constraint of the
/// history query (author, committer, time window, ref, path, diff sides,
/// content filters) is the search plane's, evaluated against the row the
/// hit names in the epoch's snapshot. The predicate runs inside the
/// collect so that the page stays bounded and the match count exact.
///
/// An error it returns aborts the search with that error.
pub type HistoryTextAdmitFn = dyn Fn(&HistoryTextHitV1) -> Result<bool, CoreError> + Send + Sync;

/// An opened epoch index, shareable across concurrent queries.
pub trait HistoryTextSearcher: Send + Sync {
    /// The `limit` best hits strictly after `after` (all hits when it is
    /// `None`) that `admit` accepts, in relevance order, with the exact
    /// count of admitted hits after the cursor.
    ///
    /// The budget is observed inside the collect (W5 phase 2): an
    /// interruption answers typed rather than at the end of the index.
    fn search(
        &self,
        query: &HistoryTextQueryV1,
        after: Option<&HistoryTextHitV1>,
        limit: usize,
        admit: Arc<HistoryTextAdmitFn>,
        budget: &RequestBudgetV1,
    ) -> Result<HistoryTextPageV1, CoreError>;

    /// Bytes this handle keeps resident while open (mapped index files).
    fn resident_bytes_estimate(&self) -> u64;
}

/// The per-epoch history text index of one generation.
///
/// Epoch indexes are immutable once published and named by the epoch
/// they belong to. An epoch is published once, before the rows of that
/// epoch become durable, and discarded only when the epoch is no longer
/// retained and no reader holds it (the search plane proves both).
pub trait HistoryTextIndexPort: Send + Sync {
    /// Whether `epoch`'s index exists and whether this build can serve it.
    fn epoch_status(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextEpochStatusV1, CoreError>;

    /// Publish the index of `epoch`, durable on return.
    ///
    /// A leftover under the same epoch from an attempt that never became
    /// the generation's epoch (the rows failed to land, or the process
    /// crashed before they did) is replaced: an epoch number is claimed
    /// only when its rows are durable, so nothing can hold the leftover.
    fn publish_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        build: HistoryTextBuildV1,
    ) -> Result<HistoryTextEpochReceiptV1, CoreError>;

    /// Open the index of `epoch` for query.
    ///
    /// Refused typed when it is absent
    /// ([`HISTORY_TEXT_INDEX_NOT_READY_CODE`]), built under another
    /// normalizer ([`HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE`]) or
    /// does not match its manifest ([`HISTORY_TEXT_INDEX_CORRUPT_CODE`]).
    fn open_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Box<dyn HistoryTextSearcher>, CoreError>;

    /// Every epoch with an index on disk for `generation`, ascending.
    fn durable_epochs(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<AuxEpochV1>, CoreError>;

    /// Remove the index of `epoch`. The caller has proven no reader holds
    /// it and no continuation can name it.
    fn discard_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError>;

    /// Remove every epoch index of `generation` (the generation was
    /// forgotten by retention).
    fn discard_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError>;
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::HistoryScoreV1;
    use quanta_index_contract::lex::CommitSha;

    use super::{HistoryTextDocKeyV1, HistoryTextHitV1};

    fn hit(score: f32, time: u64, sha_byte: u8, path: Option<&str>) -> HistoryTextHitV1 {
        let sha = CommitSha::from_bytes([sha_byte; 20]);
        HistoryTextHitV1 {
            key: path.map_or(HistoryTextDocKeyV1::Commit { sha }, |file_path| {
                HistoryTextDocKeyV1::Diff {
                    sha,
                    file_path: file_path.to_string(),
                }
            }),
            committer_time_ms: time,
            score: HistoryScoreV1::try_new(score).expect("finite"),
        }
    }

    #[test]
    fn the_relevance_order_is_score_desc_time_desc_sha_asc_path_asc() {
        let mut hits = [
            hit(1.0, 5, 3, None),
            hit(2.0, 1, 9, None),
            hit(1.0, 9, 7, None),
            hit(1.0, 9, 2, None),
        ];
        hits.sort();
        let order: Vec<(u64, u8)> = hits
            .iter()
            .map(|hit| {
                (
                    hit.committer_time_ms,
                    hit.key
                        .sha()
                        .as_bytes()
                        .first()
                        .copied()
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(order, [(1, 9), (9, 2), (9, 7), (5, 3)]);
        let scores: Vec<HistoryScoreV1> = hits.iter().map(|hit| hit.score).collect();
        assert!(
            scores
                .windows(2)
                .all(|pair| matches!(pair, [a, b] if a >= b))
        );

        let mut diffs = [hit(1.0, 1, 1, Some("z.rs")), hit(1.0, 1, 1, Some("a.rs"))];
        diffs.sort();
        assert_eq!(
            diffs.first().and_then(|hit| hit.key.file_path()),
            Some("a.rs")
        );
    }
}
