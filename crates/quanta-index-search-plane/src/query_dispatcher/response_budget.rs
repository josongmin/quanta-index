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
    ContinuationTokenV2, LexicalCursor, LexicalRowOrderKey, QueryResultWindowV2, SymbolCandidate,
    SymbolQueryResponse, TextQueryResponse,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::{MAX_FRAME_BODY_BYTES, cbor_payload_len};
use serde::Serialize;

/// Bytes kept free in a frame for the envelope around a page: the request
/// id, the status and the payload's variant tag encode in far less.
pub const RESPONSE_ENVELOPE_RESERVE_BYTES: u64 = 4_096;

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
pub(super) trait RankedPage: Serialize + Clone {
    type Row: Serialize;

    fn rows(&self) -> &[Self::Row];

    /// The encoded bytes that ride with row `index` beyond the row itself
    /// (a projection row paired with it).
    fn paired_bytes(&self, index: usize) -> Result<u64, CoreError>;

    fn window(&self) -> &QueryResultWindowV2;

    fn last_key(&self, returned: usize) -> Option<LexicalRowOrderKey<'_>>;

    fn generation(&self) -> quanta_index_contract::ManifestGeneration;

    /// Keep the first `returned` rows under `window`, continued by `cursor`.
    fn cut(
        self,
        returned: usize,
        window: QueryResultWindowV2,
        cursor: ContinuationTokenV2,
    ) -> Self;
}

fn encoded_len<T: Serialize>(value: &T, what: &str) -> Result<u64, CoreError> {
    cbor_payload_len(value)
        .map_err(|err| CoreError::InvalidContract(format!("measure {what}: {err}")))
}

/// The window of a page cut to `returned` rows because the bytes ran out:
/// more rows exist, and at least one more than were returned.
/// `page` as it fits `budget`: whole when it fits, else its longest
/// prefix that does, continued by a cursor at the prefix's last row.
///
/// The cut is found from the rows' encoded sizes and then proved by
/// measuring the page it produces, so the answer never exceeds the budget.
pub(super) fn fit_ranked_page<P: RankedPage>(
    page: P,
    budget: ResponsePayloadBudget,
    mint: impl Fn(&LexicalCursor) -> Result<ContinuationTokenV2, CoreError>,
) -> Result<P, CoreError> {
    let limit = budget.max_payload_bytes();
    let whole = encoded_len(&page, "ranked page")?;
    if whole <= limit {
        return Ok(page);
    }
    let mut row_bytes: Vec<u64> = Vec::with_capacity(page.rows().len());
    for (index, row) in page.rows().iter().enumerate() {
        row_bytes.push(encoded_len(row, "ranked row")?.saturating_add(page.paired_bytes(index)?));
    }
    let rows_total = row_bytes
        .iter()
        .fold(0_u64, |total, bytes| total.saturating_add(*bytes));
    // What the page costs without its rows, plus room for the cursor the
    // cut adds: bounded by the largest row, whose key a cursor repeats.
    let cursor_room = row_bytes.iter().copied().max().map_or(0, |bytes| bytes);
    let skeleton = whole.saturating_sub(rows_total).saturating_add(cursor_room);
    let mut returned = 0_usize;
    let mut used = skeleton;
    for bytes in &row_bytes {
        let next = used.saturating_add(*bytes);
        if next > limit {
            break;
        }
        used = next;
        returned = returned.saturating_add(1);
    }
    let window = page.window().clone();
    let generation = page.generation();
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
        let cut = page
            .clone()
            .cut(
                returned,
                crate::query_dispatcher::window::cut_pageable_window_v2(&window, returned)?,
                token,
            );
        if encoded_len(&cut, "cut ranked page")? <= limit {
            return Ok(cut);
        }
        returned = last;
    }
}

fn too_large(encoded: u64, limit: u64) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::ResultTooLarge,
        message: format!(
            "the page's first row alone does not fit the {limit}-byte response budget (the whole page encodes to {encoded} bytes); narrow the query or the projection"
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

    fn cut(
        mut self,
        returned: usize,
        window: QueryResultWindowV2,
        cursor: ContinuationTokenV2,
    ) -> Self {
        self.results.truncate(returned);
        if let Some(rows) = self.file_owner_rows.as_mut() {
            rows.truncate(returned);
        }
        self.window = window;
        self.next_cursor = Some(cursor);
        self
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

    fn cut(
        mut self,
        returned: usize,
        window: QueryResultWindowV2,
        cursor: ContinuationTokenV2,
    ) -> Self {
        self.results.truncate(returned);
        self.window = window;
        self.next_cursor = Some(cursor);
        self
    }
}
