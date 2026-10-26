//! Product `ReadSession` custody for the fixed Native mixed-load point.
//! The daemon's Catalog ticket is the authority; this module owns only caller evidence.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use quanta_index_contract::{
    QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
    index::{ObjectRef, ViewRef, ViewSelection},
    native_protocol::{
        NativeReadSessionState, NativeReadSessionStatus, NativeReadSettlementState,
        NativeTextResult,
    },
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::NativeUdsClient;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{FIRST_PATH, owner, text_spec};

pub(super) const QUERY_DEADLINE: Duration = Duration::from_secs(2);
const HELD_DURATION: Duration = Duration::from_secs(600);
const RENEW_PERIOD: Duration = Duration::from_secs(120);
const MAX_RENEW_GAP: Duration = Duration::from_secs(180);
const SETTLEMENT_LIMIT: Duration = Duration::from_secs(60);

pub(super) fn text_query(token: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: token.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

pub(super) fn retained_witness(
    client: &NativeUdsClient,
    status: &NativeReadSessionStatus,
    token: Option<&str>,
    expected_ref: Option<ObjectRef>,
    budget: &RequestBudgetV1,
) -> Result<()> {
    let selected = token.unwrap_or("scalefileneedle000file00000");
    let page = client.search_text_in_read_session(
        status,
        text_spec().instance,
        text_query(selected, 1),
        budget,
    )?;
    verify_witness(&page, status.view, token, expected_ref)
}

pub(super) fn verify_witness(
    page: &NativeTextResult,
    view: ViewRef,
    token: Option<&str>,
    expected_ref: Option<ObjectRef>,
) -> Result<()> {
    ensure!(
        page.view == view && page.exhaustive,
        "mixed first-owner witness view differs"
    );
    match (token, expected_ref) {
        (Some(_), Some(reference)) => {
            let hit = page
                .hits
                .first()
                .context("mixed first-owner witness absent")?;
            ensure!(
                page.matched == 1 && page.hits.len() == 1,
                "mixed first-owner witness count differs"
            );
            ensure!(
                hit.path == FIRST_PATH && hit.owner == owner(0)? && hit.source == reference,
                "mixed first-owner witness binding differs"
            );
        }
        (None, None) => ensure!(
            page.matched == 0 && page.hits.is_empty(),
            "mixed deleted owner remained searchable"
        ),
        _ => bail!("mixed first-owner token/reference inconsistent"),
    }
    Ok(())
}

pub(super) fn session_key(view: ViewRef, purpose: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index mixed retained read v1");
    hasher.update(purpose);
    hasher.update(view.sequence.to_le_bytes());
    hasher.update(view.digest.0);
    hasher.finalize().into()
}

fn elapsed_ms(origin: Instant) -> f64 {
    origin.elapsed().as_secs_f64() * 1_000.0
}

pub(super) struct HeldRead {
    initial: ViewRef,
    token: Option<String>,
    reference: Option<ObjectRef>,
    status: NativeReadSessionStatus,
    initial_revision: u64,
    initial_expiry_ms: u64,
    origin: Instant,
    last_renew: Instant,
    renewals_ms: Vec<f64>,
    renewal_expiries_ms: Vec<u64>,
    held_queries: u64,
    last_held_query_ms: f64,
    first_release_attempt_ms: Option<f64>,
    confirmed_release_ms: Option<f64>,
    confirmed_last_pin: Option<bool>,
    settled_ms: Option<f64>,
    settlement_polls: u64,
    last_settlement_state: Option<NativeReadSettlementState>,
    settlement_head: Option<u64>,
    settlement_floor: Option<u64>,
}

impl HeldRead {
    pub(super) fn open(
        client: &NativeUdsClient,
        view: ViewRef,
        token: Option<&str>,
        reference: Option<ObjectRef>,
    ) -> Result<Self> {
        let status = client.open_read_session(
            ViewSelection::Exact(view),
            session_key(view, b"long"),
            &RequestBudgetV1::for_duration(QUERY_DEADLINE),
        )?;
        ensure!(status.view == view, "mixed held initial view differs");
        // Successful Open ACK is the conservative start of verified pin custody.
        let origin = Instant::now();
        let mut held = Self {
            initial: view,
            token: token.map(str::to_owned),
            reference,
            status,
            initial_revision: status.revision,
            initial_expiry_ms: status.expires_at_ms,
            origin,
            last_renew: origin,
            renewals_ms: Vec::new(),
            renewal_expiries_ms: Vec::new(),
            held_queries: 0,
            last_held_query_ms: 0.0,
            first_release_attempt_ms: None,
            confirmed_release_ms: None,
            confirmed_last_pin: None,
            settled_ms: None,
            settlement_polls: 0,
            last_settlement_state: None,
            settlement_head: None,
            settlement_floor: None,
        };
        if let Err(error) = held.query(client, &RequestBudgetV1::for_duration(QUERY_DEADLINE)) {
            let cleanup = held.release(client, &RequestBudgetV1::for_duration(QUERY_DEADLINE));
            return Err(match cleanup {
                Ok(()) => error,
                Err(release_error) => error.context(format!(
                    "mixed initial held ticket release also failed: {release_error:#}"
                )),
            });
        }
        Ok(held)
    }

    fn query(&mut self, client: &NativeUdsClient, budget: &RequestBudgetV1) -> Result<()> {
        retained_witness(
            client,
            &self.status,
            self.token.as_deref(),
            self.reference,
            budget,
        )?;
        self.held_queries = self
            .held_queries
            .checked_add(1)
            .context("mixed held query count overflow")?;
        self.last_held_query_ms = elapsed_ms(self.origin);
        Ok(())
    }

    pub(super) fn release(
        &mut self,
        client: &NativeUdsClient,
        budget: &RequestBudgetV1,
    ) -> Result<()> {
        if self.first_release_attempt_ms.is_none() {
            self.first_release_attempt_ms = Some(elapsed_ms(self.origin));
        }
        let (status, remaining) = client.release_read_session(&self.status, budget)?;
        self.status = status;
        self.confirmed_release_ms = Some(elapsed_ms(self.origin));
        self.confirmed_last_pin = Some(!remaining);
        ensure!(!remaining, "mixed held release left another target pin");
        Ok(())
    }

    pub(super) fn tick(
        &mut self,
        client: &NativeUdsClient,
        budget: &RequestBudgetV1,
    ) -> Result<()> {
        let now = Instant::now();
        if self.status.state == NativeReadSessionState::Active {
            ensure!(
                now.saturating_duration_since(self.last_renew) <= MAX_RENEW_GAP,
                "mixed held renewal exceeded bounded gap"
            );
            if now.saturating_duration_since(self.origin) >= HELD_DURATION {
                self.query(client, budget)?;
                self.release(client, budget)?;
            } else if now.saturating_duration_since(self.last_renew) >= RENEW_PERIOD {
                let renewed = client.renew_read_session(&self.status, budget)?;
                ensure!(
                    renewed.expires_at_ms > self.status.expires_at_ms,
                    "mixed held renewal did not extend the lease"
                );
                self.status = renewed;
                self.last_renew = Instant::now();
                self.renewals_ms.push(elapsed_ms(self.origin));
                self.renewal_expiries_ms.push(renewed.expires_at_ms);
                self.query(client, budget)?;
            }
        }
        if self.status.state == NativeReadSessionState::Released && self.settled_ms.is_none() {
            let attempt = self
                .first_release_attempt_ms
                .context("mixed held release clock absent")?;
            ensure!(
                elapsed_ms(self.origin) - attempt <= SETTLEMENT_LIMIT.as_secs_f64() * 1_000.0,
                "mixed held settlement exceeded 60 seconds"
            );
            let settlement = client.read_session_settlement(&self.status, budget)?;
            let returned_ms = elapsed_ms(self.origin);
            ensure!(
                returned_ms - attempt <= SETTLEMENT_LIMIT.as_secs_f64() * 1_000.0,
                "mixed held settlement response exceeded 60 seconds"
            );
            self.settlement_polls = self
                .settlement_polls
                .checked_add(1)
                .context("mixed held settlement poll count overflow")?;
            self.last_settlement_state = Some(settlement.state);
            ensure!(
                settlement.view == self.initial,
                "mixed held settlement target differs"
            );
            if settlement.state == NativeReadSettlementState::SettledAtSnapshot {
                ensure!(
                    !settlement.has_remaining_target_pins
                        && settlement.exact_floor > self.initial.sequence,
                    "mixed held settlement lacks last-pin/floor proof"
                );
                self.settled_ms = Some(returned_ms);
                self.settlement_head = Some(settlement.head.sequence);
                self.settlement_floor = Some(settlement.exact_floor);
            }
        }
        Ok(())
    }

    pub(super) fn complete(&self) -> Result<()> {
        let bounded_settlement = self
            .first_release_attempt_ms
            .zip(self.settled_ms)
            .is_some_and(|(attempt, settled)| {
                settled >= attempt && settled - attempt <= SETTLEMENT_LIMIT.as_secs_f64() * 1_000.0
            });
        ensure!(
            self.first_release_attempt_ms
                .is_some_and(|ms| ms >= 600_000.0)
                && self.confirmed_last_pin == Some(true)
                && bounded_settlement
                && self.held_queries >= 2,
            "mixed held read did not prove 600-second release and settlement"
        );
        Ok(())
    }

    pub(super) fn detail(&self) -> Value {
        json!({
            "scope":"product ReadSession Exact; monotonic 600-second held ticket and bounded settlement, not continuous resource peak",
            "lease_millis":300_000,
            "renew_period_ms":RENEW_PERIOD.as_millis(),
            "max_renew_gap_ms":MAX_RENEW_GAP.as_millis(),
            "initial_view_sequence":self.initial.sequence,
            "initial_revision":self.initial_revision,
            "final_revision":self.status.revision,
            "initial_expiry_ms":self.initial_expiry_ms,
            "renewals_ms":self.renewals_ms,
            "renewal_expiries_ms":self.renewal_expiries_ms,
            "held_queries":self.held_queries,
            "last_held_query_ms":self.last_held_query_ms,
            "first_release_attempt_ms":self.first_release_attempt_ms,
            "confirmed_release_ms":self.confirmed_release_ms,
            "confirmed_last_pin":self.confirmed_last_pin,
            "settled_ms":self.settled_ms,
            "settlement_polls":self.settlement_polls,
            "last_settlement_state":self.last_settlement_state.map(|state|format!("{state:?}")),
            "settlement_head_sequence":self.settlement_head,
            "settlement_exact_floor":self.settlement_floor,
            "last_status":format!("{:?}",self.status.state),
        })
    }

    pub(super) fn is_active(&self) -> bool {
        self.status.state == NativeReadSessionState::Active
    }
}
