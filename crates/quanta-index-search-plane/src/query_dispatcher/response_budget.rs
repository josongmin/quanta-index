//! The byte budget of a ranked lexical page (QI-BB-005 보완 #5).
//!
//! A page travels in one IPC frame body, capped at `MAX_FRAME_BODY_BYTES`.
//! A page that would not fit used to be computed in full and then refused
//! at encode. Now the dispatcher measures a ranked page's encoded size
//! before answering and, when it does not fit, cuts it at the last row
//! that does: the window says more rows exist and the cursor names the
//! last row returned, so the caller continues exactly where the bytes ran
//! out. A first row that alone does not fit is refused typed
//! (`RESULT_TOO_LARGE`) before anything is encoded. Routes without a
//! continuation keep the transport's typed refusal as their bound.

use quanta_index_contract::{
    ContinuationTokenV2, LexicalCursor, LexicalRowOrderKey, PlannerStage, PlannerTraceEntry,
    QueryResultWindowV2, SymbolCandidate, SymbolQueryResponse, TextQueryResponse,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::{MAX_FRAME_BODY_BYTES, cbor_payload_len};
use serde::Serialize;

/// Bytes kept free in a frame for the envelope around a page: the request
/// id, the status and the payload's variant tag encode in far less.
pub const RESPONSE_ENVELOPE_RESERVE_BYTES: u64 = 4_096;

/// Same page overhead allowance under enabled and disabled stage observation.
pub(super) const LEXICAL_STAGE_RESERVE_BYTES: u64 = 512;

/// How many encoded bytes one ranked page may take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponsePayloadBudget {
    max_payload_bytes: u64,
}

impl ResponsePayloadBudget {
    /// Everything a frame holds beside the envelope reserve.
    pub const DEFAULT: Self = Self {
        max_payload_bytes: frame_payload_bytes(),
    };

    /// A budget of `max_payload_bytes`, between one byte and what a frame
    /// holds beside the envelope reserve.
    pub fn new(max_payload_bytes: u64) -> Result<Self, CoreError> {
        if max_payload_bytes == 0 || max_payload_bytes > frame_payload_bytes() {
            return Err(CoreError::InvalidContract(format!(
                "query response payload budget {max_payload_bytes} is outside 1..={}",
                frame_payload_bytes()
            )));
        }
        Ok(Self { max_payload_bytes })
    }

    #[must_use]
    pub const fn max_payload_bytes(self) -> u64 {
        self.max_payload_bytes
    }
}

#[expect(
    clippy::as_conversions,
    reason = "MAX_FRAME_BODY_BYTES is a 16 MiB usize constant; widening it to u64 in a const context has no fallible path"
)]
const fn frame_payload_bytes() -> u64 {
    (MAX_FRAME_BODY_BYTES as u64).saturating_sub(RESPONSE_ENVELOPE_RESERVE_BYTES)
}

/// A ranked page the budget can cut: its rows, and the page it becomes
/// when only the first `returned` rows stay.
pub(super) trait RankedPage: Serialize + Sized {
    type Row: Serialize;

    fn rows(&self) -> &[Self::Row];

    /// The encoded bytes that ride with row `index` beyond the row itself
    /// (a projection row paired with it).
    fn paired_bytes(&self, index: usize) -> Result<u64, CoreError>;

    fn window(&self) -> &QueryResultWindowV2;

    fn last_key(&self, returned: usize) -> Option<LexicalRowOrderKey<'_>>;

    fn generation(&self) -> quanta_index_contract::ManifestGeneration;

    /// Timing observations are not pagination authority. Routes carrying them
    /// use the same bounded reserve with instrumentation enabled or disabled.
    fn budget_encoded_len(&self) -> Result<u64, CoreError> {
        encoded_len(self, "ranked page")
    }

    /// Keep the first `returned` rows under `window`, continued by `cursor`.
    fn cut(&mut self, returned: usize, window: QueryResultWindowV2, cursor: ContinuationTokenV2);
}

fn encoded_len<T: Serialize>(value: &T, what: &str) -> Result<u64, CoreError> {
    cbor_payload_len(value)
        .map_err(|err| CoreError::InvalidContract(format!("measure {what}: {err}")))
}

/// Normalize policy-controlled `CodeSearch` clocks to their maximum wire shape.
///
/// Both observation policies reserve the same bytes before selecting a prefix
/// or minting its continuation cursor.
fn code_search_clock_reserve_delta(trace: &[PlannerTraceEntry]) -> Result<u64, CoreError> {
    const ORDINARY_CLOCKS: [&str; 3] = ["candidate_ns", "sort_page_ns", "preview_ns"];
    const TYPO_CLOCKS: [&str; 6] = [
        "candidate_ns",
        "sort_page_ns",
        "preview_ns",
        "typo_shortlist_admission_ns",
        "typo_source_token_scan_ns",
        "typo_materialize_ns",
    ];
    const TYPO_CLOCK_PREFIXES: [&str; 3] = [
        "code_search.execution.typo_shortlist_admission_ns=",
        "code_search.execution.typo_source_token_scan_ns=",
        "code_search.execution.typo_materialize_ns=",
    ];
    let has_code_search = trace
        .iter()
        .any(|entry| entry.detail.starts_with("code_search.execution.scope="));
    if !has_code_search {
        if trace
            .iter()
            .any(|entry| entry.detail.starts_with("code_search.execution."))
        {
            return Err(CoreError::InvalidContract(
                "code-search work trace lacks execution scope".into(),
            ));
        }
        return Ok(0);
    }
    let modes: Vec<_> = trace
        .iter()
        .filter_map(|entry| entry.detail.strip_prefix("code_search.execution.mode="))
        .collect();
    let clocks: &[&str] = match modes.as_slice() {
        ["ordinary" | "components"] => &ORDINARY_CLOCKS,
        ["typo_explicit" | "typo_fallback"] => &TYPO_CLOCKS,
        _ => {
            return Err(CoreError::InvalidContract(
                "code-search clock reserve requires one known execution mode".into(),
            ));
        }
    };
    let mut canonical = Vec::with_capacity(trace.len().saturating_add(clocks.len()));
    let mut seen = vec![false; clocks.len()];
    for entry in trace {
        let clock = clocks.iter().enumerate().find_map(|(index, name)| {
            entry
                .detail
                .strip_prefix("code_search.execution.")
                .and_then(|detail| detail.strip_prefix(name))
                .and_then(|value| value.strip_prefix('='))
                .map(|value| (index, value))
        });
        if let Some((index, value)) = clock {
            let parsed = value.parse::<u64>().map_err(|error| {
                CoreError::InvalidContract(format!("code-search work clock is malformed: {error}"))
            })?;
            let duplicate = seen.get(index).copied().unwrap_or(true);
            if entry.stage != PlannerStage::Merge || duplicate || parsed.to_string() != value {
                return Err(CoreError::InvalidContract(
                    "code-search work clock is duplicated or malformed".into(),
                ));
            }
            if let Some(clock_seen) = seen.get_mut(index) {
                *clock_seen = true;
            }
        } else {
            if clocks.len() == ORDINARY_CLOCKS.len()
                && TYPO_CLOCK_PREFIXES
                    .iter()
                    .any(|prefix| entry.detail.starts_with(*prefix))
            {
                return Err(CoreError::InvalidContract(
                    "non-typo code-search trace carries typo clock".into(),
                ));
            }
            canonical.push(entry.clone());
        }
    }
    for name in clocks {
        canonical.push(PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("code_search.execution.{name}={}", u64::MAX),
        });
    }
    let observed = encoded_len(&trace, "code-search planner trace")?;
    encoded_len(&canonical, "reserved code-search planner trace")?
        .checked_sub(observed)
        .ok_or_else(|| CoreError::InvalidContract("code-search clock reserve underflow".into()))
}

/// The window of a page cut to `returned` rows.
///
/// The bytes ran out: more rows exist, and at least one more than were
/// returned. `page` as it fits `budget`: whole when it fits, else its
/// longest prefix that does, continued by a cursor at the prefix's last row.
///
/// The row bytes bound which prefixes could possibly fit. Check those
/// prefixes from longest to shortest with their actual cursor and window:
/// cursor size depends on the last row, so it is not monotone in page length.
pub(super) fn fit_ranked_page<P: RankedPage>(
    page: P,
    budget: ResponsePayloadBudget,
    mint: impl Fn(&LexicalCursor) -> Result<ContinuationTokenV2, CoreError>,
) -> Result<P, CoreError> {
    let limit = budget.max_payload_bytes();
    let whole = page.budget_encoded_len()?;
    if whole <= limit {
        return Ok(page);
    }
    let mut returned = 0_usize;
    let mut row_bytes_total = 0_u64;
    for (index, row) in page.rows().iter().enumerate() {
        let row_bytes = encoded_len(row, "ranked row")?.saturating_add(page.paired_bytes(index)?);
        let next = row_bytes_total.saturating_add(row_bytes);
        if next > limit {
            break;
        }
        row_bytes_total = next;
        returned = returned.saturating_add(1);
    }
    let window = page.window().clone();
    let generation = page.generation();
    let mut page = page;
    loop {
        let Some(last) = returned.checked_sub(1) else {
            return Err(too_large(whole, limit));
        };
        let Some(key) = page.last_key(returned) else {
            return Err(CoreError::InvalidContract(format!(
                "ranked page has no row {last} to continue from"
            )));
        };
        let cursor = LexicalCursor::at(generation, key);
        let token = mint(&cursor)?;
        page.cut(
            returned,
            crate::query_dispatcher::window::cut_pageable_window_v2(&window, returned)?,
            token,
        );
        if page.budget_encoded_len()? <= limit {
            return Ok(page);
        }
        returned = last;
    }
}

fn too_large(encoded: u64, limit: u64) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::ResultTooLarge,
        message: format!(
            "no nonempty continued page fits the {limit}-byte response budget (the whole page accounts for {encoded} bytes including reserved observations); narrow the query or the projection"
        ),
    }
}

impl RankedPage for TextQueryResponse {
    type Row = quanta_index_contract::LexicalCandidate;

    fn rows(&self) -> &[Self::Row] {
        &self.results
    }

    fn paired_bytes(&self, index: usize) -> Result<u64, CoreError> {
        self.file_owner_rows
            .as_ref()
            .and_then(|rows| rows.get(index))
            .map_or(Ok(0), |row| encoded_len(row, "file owner row"))
    }

    fn window(&self) -> &QueryResultWindowV2 {
        &self.window
    }

    fn last_key(&self, returned: usize) -> Option<LexicalRowOrderKey<'_>> {
        returned
            .checked_sub(1)
            .and_then(|last| self.results.get(last))
            .map(quanta_index_contract::LexicalCandidate::order_key)
    }

    fn generation(&self) -> quanta_index_contract::ManifestGeneration {
        self.generation.manifest_generation
    }

    fn budget_encoded_len(&self) -> Result<u64, CoreError> {
        // At most four lexical stages, each with bounded kind, u64 timing and
        // candidate count, and u32 calls. 512 bytes dominates the CBOR shape
        // even at every scalar maximum (covered by the reserve test).
        let whole = encoded_len(self, "ranked page")?;
        let measured_slot = encoded_len(&self.explanation.stage_timings, "lexical stage slot")?;
        let code_search_clock_delta =
            code_search_clock_reserve_delta(&self.explanation.planner_trace)?;
        if measured_slot > LEXICAL_STAGE_RESERVE_BYTES {
            return Err(CoreError::InvalidContract(
                "lexical stages exceed reserved shape".to_string(),
            ));
        }
        let empty_slot: Option<&[quanta_index_contract::QueryStageTimingV1]> = None;
        let empty_slot_bytes = encoded_len(&empty_slot, "unobserved lexical stage slot")?;
        // The explanation always serializes this slot. CBOR values encode
        // independently, avoiding candidate clones or a parallel wire schema.
        whole
            .checked_sub(measured_slot)
            .and_then(|bytes| bytes.checked_add(empty_slot_bytes))
            .and_then(|bytes| bytes.checked_add(LEXICAL_STAGE_RESERVE_BYTES))
            .and_then(|bytes| bytes.checked_add(code_search_clock_delta))
            .ok_or_else(|| CoreError::InvalidContract("lexical stage reserve overflow".to_string()))
    }

    fn cut(&mut self, returned: usize, window: QueryResultWindowV2, cursor: ContinuationTokenV2) {
        self.results.truncate(returned);
        if let Some(rows) = self.file_owner_rows.as_mut() {
            rows.truncate(returned);
        }
        self.window = window;
        self.next_cursor = Some(cursor);
    }
}

impl RankedPage for SymbolQueryResponse {
    type Row = SymbolCandidate;

    fn rows(&self) -> &[Self::Row] {
        &self.results
    }

    fn paired_bytes(&self, _index: usize) -> Result<u64, CoreError> {
        Ok(0)
    }

    fn window(&self) -> &QueryResultWindowV2 {
        &self.window
    }

    fn last_key(&self, returned: usize) -> Option<LexicalRowOrderKey<'_>> {
        returned
            .checked_sub(1)
            .and_then(|last| self.results.get(last))
            .map(SymbolCandidate::order_key)
    }

    fn generation(&self) -> quanta_index_contract::ManifestGeneration {
        self.generation.manifest_generation
    }

    fn cut(&mut self, returned: usize, window: QueryResultWindowV2, cursor: ContinuationTokenV2) {
        self.results.truncate(returned);
        self.window = window;
        self.next_cursor = Some(cursor);
    }
}
