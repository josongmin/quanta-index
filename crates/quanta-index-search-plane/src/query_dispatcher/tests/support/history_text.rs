//! An in-memory [`HistoryTextIndexPort`] double: a reference BM25 over a
//! whitespace/punctuation tokenizer, so the history route's relevance
//! path is exercised against an oracle that is not the engine.
//!
//! Only what the route tests need is scorable: a single keyword leaf, a
//! phrase, or an `All` of those (scores add) — with a raw string beside a
//! scored clause dropped from the scoring the way the real adapter drops
//! it (the route's predicate filters it). Everything else is refused
//! [`HISTORY_TEXT_QUERY_UNSCORABLE_CODE`], as the real adapter refuses it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    AuxEpochV1, HistoryScoreV1, LqExpr, LqLeaf, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HISTORY_TEXT_INDEX_NOT_READY_CODE,
    HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextAdmitFn, HistoryTextBuildV1,
    HistoryTextDiscardOutcomeV1, HistoryTextDocKeyV1, HistoryTextDocV1, HistoryTextEpochReceiptV1,
    HistoryTextEpochStatusV1, HistoryTextHitV1, HistoryTextIndexPort, HistoryTextPageV1,
    HistoryTextQueryV1, HistoryTextSearcher, RequestBudgetV1,
};

/// BM25 constants of the reference; the engine's own.
const K1: f32 = 1.2;
const B: f32 = 0.75;

type EpochDocs = BTreeMap<HistoryTextDocKeyV1, HistoryTextDocV1>;

/// The double: every published epoch's documents, and what was discarded.
#[derive(Default)]
pub(crate) struct MemoryHistoryTextIndex {
    epochs: Mutex<BTreeMap<(AuxiliaryGenerationKeyV1, AuxEpochV1), EpochDocs>>,
    discarded: Mutex<Vec<(AuxiliaryGenerationKeyV1, AuxEpochV1)>>,
    /// Every build the double was asked to publish, in order.
    builds: Mutex<Vec<(AuxEpochV1, &'static str)>>,
    /// When set, the next publish fails typed before writing anything.
    fail_next_publish: std::sync::atomic::AtomicBool,
    /// When set, the next discard fails before removing anything.
    fail_next_discard: std::sync::atomic::AtomicBool,
    /// When set, the next listing of a pair's generations is refused typed,
    /// as the adapter refuses an entry that is not a generation.
    refuse_next_listing: std::sync::atomic::AtomicBool,
}

impl MemoryHistoryTextIndex {
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock<'a, T>(
        mutex: &'a Mutex<T>,
        what: &str,
    ) -> Result<std::sync::MutexGuard<'a, T>, CoreError> {
        mutex.lock().map_err(|err| {
            CoreError::Storage(format!("memory history text index {what} poisoned: {err}"))
        })
    }

    /// The epochs discarded so far, in order.
    pub(crate) fn discarded(
        &self,
    ) -> Result<Vec<(AuxiliaryGenerationKeyV1, AuxEpochV1)>, CoreError> {
        Ok(Self::lock(&self.discarded, "discarded")?.clone())
    }

    /// The published builds so far: `(epoch, "full" | "incremental")`.
    pub(crate) fn builds(&self) -> Result<Vec<(AuxEpochV1, &'static str)>, CoreError> {
        Ok(Self::lock(&self.builds, "builds")?.clone())
    }

    /// Make the next `publish_epoch` fail before writing anything.
    pub(crate) fn fail_next_publish(&self) {
        self.fail_next_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make the next `discard_epoch` or `discard_generation` fail before
    /// removing anything, as an I/O error would.
    pub(crate) fn fail_next_discard(&self) {
        self.fail_next_discard
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn refuse_next_listing(&self) {
        self.refuse_next_listing
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn take_injected_discard_failure(&self) -> Result<(), CoreError> {
        if self
            .fail_next_discard
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CoreError::Storage(
                "memory history text index: injected discard failure".to_string(),
            ));
        }
        Ok(())
    }

    /// The epochs currently durable for `generation`.
    pub(crate) fn epochs_of(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<AuxEpochV1>, CoreError> {
        self.durable_epochs(generation)
    }
}

impl HistoryTextIndexPort for MemoryHistoryTextIndex {
    fn epoch_status(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextEpochStatusV1, CoreError> {
        let epochs = Self::lock(&self.epochs, "epochs")?;
        Ok(if epochs.contains_key(&(generation.clone(), epoch)) {
            HistoryTextEpochStatusV1::Servable
        } else {
            HistoryTextEpochStatusV1::Absent
        })
    }

    fn publish_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        build: HistoryTextBuildV1,
    ) -> Result<HistoryTextEpochReceiptV1, CoreError> {
        if self
            .fail_next_publish
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CoreError::Storage(
                "memory history text index: injected publish failure".to_string(),
            ));
        }
        let mut epochs = Self::lock(&self.epochs, "epochs")?;
        let (mut docs, upserts, label) = match build {
            HistoryTextBuildV1::Full { docs } => (EpochDocs::new(), docs, "full"),
            HistoryTextBuildV1::Incremental { base, upserts } => {
                let base_docs = epochs
                    .get(&(generation.clone(), base))
                    .cloned()
                    .ok_or_else(|| CoreError::Typed {
                        code: HISTORY_TEXT_INDEX_NOT_READY_CODE.to_string(),
                        message: format!("memory history text index: base epoch {base} is absent"),
                    })?;
                (base_docs, upserts, "incremental")
            }
        };
        let written =
            u64::try_from(upserts.len()).map_err(|err| CoreError::Storage(err.to_string()))?;
        for doc in upserts {
            let _replaced = docs.insert(doc.key.clone(), doc);
        }
        let _previous = epochs.insert((generation.clone(), epoch), docs);
        drop(epochs);
        Self::lock(&self.builds, "builds")?.push((epoch, label));
        Ok(HistoryTextEpochReceiptV1 {
            docs_written: written,
        })
    }

    fn open_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Box<dyn HistoryTextSearcher>, CoreError> {
        let epochs = Self::lock(&self.epochs, "epochs")?;
        let docs = epochs.get(&(generation.clone(), epoch)).cloned();
        drop(epochs);
        let docs = docs.ok_or_else(|| CoreError::Typed {
            code: HISTORY_TEXT_INDEX_NOT_READY_CODE.to_string(),
            message: format!("memory history text index: epoch {epoch} is absent"),
        })?;
        Ok(Box::new(MemoryEpochSearcher { docs }))
    }

    fn durable_epochs(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<AuxEpochV1>, CoreError> {
        Ok(Self::lock(&self.epochs, "epochs")?
            .keys()
            .filter(|(key, _epoch)| key == generation)
            .map(|(_key, epoch)| *epoch)
            .collect())
    }

    fn durable_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError> {
        if self
            .refuse_next_listing
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CoreError::Typed {
                code: "HISTORY_TEXT_INDEX_FOREIGN_ENTRY".to_string(),
                message: "memory history text index: injected foreign entry".to_string(),
            });
        }
        let generations: std::collections::BTreeSet<ManifestGeneration> =
            Self::lock(&self.epochs, "epochs")?
                .keys()
                .filter(|(key, _epoch)| key.repo_id == *repo_id && key.revision_id == *revision_id)
                .map(|(key, _epoch)| key.generation)
                .collect();
        Ok(generations.into_iter().collect())
    }

    fn discard_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        self.take_injected_discard_failure()?;
        let removed = Self::lock(&self.epochs, "epochs")?.remove(&(generation.clone(), epoch));
        Ok(match removed {
            None => HistoryTextDiscardOutcomeV1::Absent,
            Some(docs) => {
                Self::lock(&self.discarded, "discarded")?.push((generation.clone(), epoch));
                HistoryTextDiscardOutcomeV1::Discarded {
                    bytes: u64::try_from(docs.len())
                        .map_err(|err| CoreError::Storage(err.to_string()))?,
                }
            }
        })
    }

    fn discard_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        self.take_injected_discard_failure()?;
        let epochs = self.durable_epochs(generation)?;
        let mut bytes = 0_u64;
        let mut any = false;
        for epoch in epochs {
            if let HistoryTextDiscardOutcomeV1::Discarded { bytes: some } =
                self.discard_epoch(generation, epoch)?
            {
                any = true;
                bytes = bytes.saturating_add(some);
            }
        }
        Ok(if any {
            HistoryTextDiscardOutcomeV1::Discarded { bytes }
        } else {
            HistoryTextDiscardOutcomeV1::Absent
        })
    }
}

/// Lowercase alphanumeric-or-underscore runs: the shared normalizer's
/// boundaries and fold for ASCII text.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|run| !run.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn unscorable() -> CoreError {
    CoreError::Typed {
        code: HISTORY_TEXT_QUERY_UNSCORABLE_CODE.to_string(),
        message: "memory history text index: only keyword / phrase conjunctions are scorable"
            .to_string(),
    }
}

/// The scored terms of an expression, or the typed refusal.
///
/// A phrase contributes its tokens as terms (every one must occur: a
/// superset of the phrase's rows, which the route's predicate narrows);
/// a raw string contributes none. An expression with no term at all is
/// refused by the caller.
fn scored_terms(expr: &LqExpr) -> Result<Vec<String>, CoreError> {
    match expr {
        LqExpr::Leaf(LqLeaf::Keyword(text) | LqLeaf::Phrase(text)) => {
            let terms = tokens(text);
            if terms.is_empty() {
                Err(unscorable())
            } else {
                Ok(terms)
            }
        }
        LqExpr::Leaf(LqLeaf::RawString(_)) => Ok(Vec::new()),
        LqExpr::All(children) if !children.is_empty() => {
            let mut terms = Vec::new();
            for child in children {
                terms.extend(scored_terms(child)?);
            }
            Ok(terms)
        }
        LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::All(_) | LqExpr::Any(_) => {
            Err(unscorable())
        }
    }
}

/// The reference BM25 of one kind's documents for `terms` (every term must
/// occur): `(doc, score)` for the matching documents.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::imprecise_flops,
    reason = "the reference computes from integer counts in f32 with the engine's own `ln(1 + x)` idf, so its scores are the engine's bit for bit"
)]
pub(crate) fn reference_bm25(
    docs: &[&HistoryTextDocV1],
    terms: &[String],
) -> Vec<(HistoryTextDocKeyV1, u64, f32)> {
    let tokenized: Vec<Vec<String>> = docs.iter().map(|doc| tokens(&doc.text)).collect();
    let total_docs = tokenized.len() as f32;
    let total_tokens: usize = tokenized.iter().map(Vec::len).sum();
    let average_length = total_tokens as f32 / total_docs;
    let mut scored = Vec::new();
    for (doc, doc_tokens) in docs.iter().zip(&tokenized) {
        let mut total = 0.0_f32;
        let mut matched = true;
        for term in terms {
            let occurrences = doc_tokens.iter().filter(|token| *token == term).count();
            if occurrences == 0 {
                matched = false;
                break;
            }
            let tf = occurrences as f32;
            let df = tokenized
                .iter()
                .filter(|other| other.iter().any(|token| token == term))
                .count() as f32;
            let idf = (1.0_f32 + (total_docs - df + 0.5) / (df + 0.5)).ln();
            let norm = K1 * (1.0 - B + B * doc_tokens.len() as f32 / average_length);
            total += idf * (K1 + 1.0) * (tf / (tf + norm));
        }
        if matched {
            scored.push((doc.key.clone(), doc.committer_time_ms, total));
        }
    }
    scored
}

struct MemoryEpochSearcher {
    docs: EpochDocs,
}

impl HistoryTextSearcher for MemoryEpochSearcher {
    fn search(
        &self,
        query: &HistoryTextQueryV1,
        after: Option<&HistoryTextHitV1>,
        limit: usize,
        admit: Arc<HistoryTextAdmitFn>,
        _budget: &RequestBudgetV1,
    ) -> Result<HistoryTextPageV1, CoreError> {
        let terms = scored_terms(&query.expr)?;
        if terms.is_empty() {
            return Err(unscorable());
        }
        let of_kind: Vec<&HistoryTextDocV1> = self
            .docs
            .values()
            .filter(|doc| doc.key.kind() == query.kind)
            .collect();
        let mut hits: Vec<HistoryTextHitV1> = reference_bm25(&of_kind, &terms)
            .into_iter()
            .map(|(key, committer_time_ms, score)| {
                HistoryScoreV1::try_new(score)
                    .map(|score| HistoryTextHitV1 {
                        key,
                        committer_time_ms,
                        score,
                    })
                    .map_err(|err| CoreError::Storage(err.to_string()))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        hits.sort();
        let examined =
            u64::try_from(hits.len()).map_err(|err| CoreError::Storage(err.to_string()))?;
        let mut matched = 0_u64;
        let mut page = Vec::new();
        for hit in hits {
            if after.is_some_and(|after| hit.cmp(after) != std::cmp::Ordering::Greater) {
                continue;
            }
            if !admit(&hit)? {
                continue;
            }
            matched = matched.saturating_add(1);
            if page.len() < limit {
                page.push(hit);
            }
        }
        Ok(HistoryTextPageV1 {
            hits: page,
            examined,
            matched,
        })
    }

    fn resident_bytes_estimate(&self) -> u64 {
        0
    }
}
