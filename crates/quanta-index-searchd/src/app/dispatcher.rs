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
use quanta_index_contract::{LexicalChannelOp, SemanticChannelOp};
use quanta_index_core::{ChannelDispatchPolicy, LexicalIndexBuildPort, SemanticIndexBuildPort};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_semantic::SemanticAdapter;

use crate::runtime::Ledger;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct ChannelDispatcher {
    lex_sub: LexicalWalSubscriber,
    sem_sub: SemanticWalSubscriber,
    lex_adapter: Arc<LexicalAdapter>,
    sem_adapter: Arc<SemanticAdapter>,
    ledger: Arc<RwLock<Ledger>>,
    poll_interval: Duration,
}

impl ChannelDispatcher {
    #[must_use]
    pub fn new(
        lex_sub: LexicalWalSubscriber,
        sem_sub: SemanticWalSubscriber,
        lex_adapter: Arc<LexicalAdapter>,
        sem_adapter: Arc<SemanticAdapter>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            lex_sub,
            sem_sub,
            lex_adapter,
            sem_adapter,
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
        let mut last_emitted = {
            let guard = self
                .ledger
                .read()
                .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
            guard.lexical_last_seen()
        };
        while let Some(event) = self.lex_sub.next_event()? {
            ChannelDispatchPolicy::validate_monotonic_seq(event.seq, last_emitted)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            last_emitted = event.seq;
            apply_lex(&event, &self.lex_adapter, &self.ledger)?;
            self.lex_sub.ack(event.seq)?;
            {
                let mut guard = self
                    .ledger
                    .write()
                    .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
                guard.set_lexical_last_seen(event.seq);
            }
            progressed = true;
        }
        Ok(progressed)
    }

    fn drain_sem(&mut self) -> Result<bool> {
        let mut progressed = false;
        let mut last_emitted = {
            let guard = self
                .ledger
                .read()
                .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
            guard.semantic_last_seen()
        };
        while let Some(event) = self.sem_sub.next_event()? {
            ChannelDispatchPolicy::validate_monotonic_seq(event.seq, last_emitted)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            last_emitted = event.seq;
            apply_sem(&event, &self.sem_adapter, &self.ledger)?;
            self.sem_sub.ack(event.seq)?;
            {
                let mut guard = self
                    .ledger
                    .write()
                    .map_err(|err| anyhow::anyhow!("ledger poisoned: {err}"))?;
                guard.set_semantic_last_seen(event.seq);
            }
            progressed = true;
        }
        Ok(progressed)
    }
}

fn apply_lex(
    event: &LexicalChannelEvent,
    adapter: &LexicalAdapter,
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let repo = event.op.repo_id().clone();
    let revision = event.op.revision_id().clone();
    let generation = event.op.generation();
    let ops = [event.op.clone()];
    adapter
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
    adapter: &SemanticAdapter,
    ledger: &Arc<RwLock<Ledger>>,
) -> Result<()> {
    let repo = event.op.repo_id().clone();
    let revision = event.op.revision_id().clone();
    let generation = event.op.generation();
    let ops = [event.op.clone()];
    adapter
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
