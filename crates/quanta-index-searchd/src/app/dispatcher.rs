//! Channel event dispatcher.
//!
//! Drains lexical / semantic subscribers, applies each op to the respective
//! adapter, and on `Seal` marks the generation sealed in the [`Ledger`].
//!
//! Apply granularity is per-event (`build(&[op])`) — the reference adapters
//! treat that as cheap; production adapters can batch internally.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use quanta_index_channel::{
    BundleChannelSubscriber, LexicalChannelEvent, LexicalWalSubscriber, SemanticChannelEvent,
    SemanticWalSubscriber,
};
use quanta_index_contract::{ChannelSeq, LexicalChannelOp, SemanticChannelOp};
use quanta_index_core::{ChannelDispatchPolicy, LexicalIndexBuildPort, SemanticIndexBuildPort};

use crate::runtime::Ledger;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Identifies which indexing track a ledger read/write targets.
#[derive(Debug, Clone, Copy)]
enum ChannelTrack {
    Lexical,
    Semantic,
}

pub struct ChannelDispatcher {
    lex_sub: LexicalWalSubscriber,
    sem_sub: SemanticWalSubscriber,
    lex_builder: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
    sem_builder: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    poll_interval: Duration,
}

impl ChannelDispatcher {
    #[must_use]
    pub fn new(
        lex_sub: LexicalWalSubscriber,
        sem_sub: SemanticWalSubscriber,
        lex_builder: Arc<dyn LexicalIndexBuildPort + Send + Sync>,
        sem_builder: Arc<dyn SemanticIndexBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            lex_sub,
            sem_sub,
            lex_builder,
            sem_builder,
            ledger,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Drain both subscribers once (non-blocking). Returns `true` if any
    /// event was processed. Useful for tests.
    pub fn poll_once(&mut self) -> Result<bool> {
        let lex = self.drain_lex()?;
        let sem = self.drain_sem()?;
        Ok(lex || sem)
    }

    /// Run until `should_stop` returns `true`. Sleeps `poll_interval` between
    /// empty drains; otherwise drains greedily.
    pub fn run_until<F>(&mut self, mut should_stop: F) -> Result<()>
    where
        F: FnMut() -> bool,
    {
        while !should_stop() {
            let progressed = self.poll_once()?;
            if !progressed {
                std::thread::sleep(self.poll_interval);
            }
        }
        Ok(())
    }

    fn drain_lex(&mut self) -> Result<bool> {
        let mut progressed = false;
        let mut last_emitted = read_last_seen(ChannelTrack::Lexical, &self.ledger)?;
        while let Some(event) = self.lex_sub.next_event()? {
            ChannelDispatchPolicy::validate_monotonic_seq(event.seq, last_emitted)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            last_emitted = event.seq;
            apply_lex(&event, self.lex_builder.as_ref(), &self.ledger)?;
            self.lex_sub.ack(event.seq)?;
            record_observed(ChannelTrack::Lexical, event.seq, &self.ledger)?;
            progressed = true;
        }
        Ok(progressed)
    }

    fn drain_sem(&mut self) -> Result<bool> {
        let mut progressed = false;
        let mut last_emitted = read_last_seen(ChannelTrack::Semantic, &self.ledger)?;
        while let Some(event) = self.sem_sub.next_event()? {
            ChannelDispatchPolicy::validate_monotonic_seq(event.seq, last_emitted)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            last_emitted = event.seq;
            apply_sem(&event, self.sem_builder.as_ref(), &self.ledger)?;
            self.sem_sub.ack(event.seq)?;
            record_observed(ChannelTrack::Semantic, event.seq, &self.ledger)?;
            progressed = true;
        }
        Ok(progressed)
    }
}

/// Read the `last_seen` cursor for the given track. Centralises the lock-poison
/// translation so both drain loops share one implementation.
fn read_last_seen(track: ChannelTrack, ledger: &Arc<RwLock<Ledger>>) -> Result<ChannelSeq> {
    let guard = ledger
        .read()
        .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
    Ok(match track {
        ChannelTrack::Lexical => guard.lexical_last_seen(),
        ChannelTrack::Semantic => guard.semantic_last_seen(),
    })
}

/// Persist the observed seq into the ledger's per-track cursor. Used after a
/// successful `apply_*` + `ack` so the next drain validates monotonicity
/// against a fresh baseline.
fn record_observed(
    track: ChannelTrack,
    seq: ChannelSeq,
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let mut guard = ledger
        .write()
        .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
    match track {
        ChannelTrack::Lexical => guard.set_lexical_last_seen(seq),
        ChannelTrack::Semantic => guard.set_semantic_last_seen(seq),
    }
    drop(guard);
    Ok(())
}

fn apply_lex(
    event: &LexicalChannelEvent,
    builder: &(dyn LexicalIndexBuildPort + Send + Sync),
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let repo = event.op.repo_id().clone();
    let revision = event.op.revision_id().clone();
    let generation = event.op.generation();
    let ops = [event.op.clone()];
    builder
        .build(&repo, &revision, generation, &ops)
        .map_err(|e| anyhow::anyhow!("lexical build: {e}"))?;
    if matches!(event.op, LexicalChannelOp::Seal(_)) {
        let mut guard = ledger
            .write()
            .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
        guard.lexical_seal(generation);
    }
    Ok(())
}

fn apply_sem(
    event: &SemanticChannelEvent,
    builder: &(dyn SemanticIndexBuildPort + Send + Sync),
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let repo = event.op.repo_id().clone();
    let revision = event.op.revision_id().clone();
    let generation = event.op.generation();
    let ops = [event.op.clone()];
    builder
        .build(&repo, &revision, generation, &ops)
        .map_err(|e| anyhow::anyhow!("semantic build: {e}"))?;
    if matches!(event.op, SemanticChannelOp::Seal(_)) {
        let mut guard = ledger
            .write()
            .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
        guard.semantic_seal(generation);
    }
    Ok(())
}
