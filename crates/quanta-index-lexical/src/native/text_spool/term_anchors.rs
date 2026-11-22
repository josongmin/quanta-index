//! Disposable upper-fence edits for immutable term dictionary pages.
//!
//! Term pages are installed before the final map apply. This table holds only
//! their root-zero map edits; root-one owner edits remain in `top` unchanged.

use super::{
    BatchDemands, EditPage, MAX_TERM_KEY_BYTES, TextSpool, TopCursor, check_term_key,
    map_field_reservation, read_edit, sql,
};
use quanta_index_core::index::{
    IndexError, MapEdit, MapValue, OrdinalMapEdit, WorkBudget, WorkUsage,
    ports::{MapEditCursor, RootPackingPlan},
};

const CREATE: &str = "CREATE TABLE term_anchor(
    root INTEGER NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
    PRIMARY KEY(root,k)) WITHOUT ROWID;";
const INSERT: &str = "INSERT INTO term_anchor(root,k,p,n) VALUES ";
const FIRST: &str = "SELECT k,p,n FROM term_anchor WHERE root=0 ORDER BY k LIMIT 64";
const NEXT: &str = "SELECT k,p,n FROM term_anchor WHERE root=0 AND k>?1 ORDER BY k LIMIT 64";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum State {
    Empty,
    Preparing,
    Ready,
    Failed,
}

impl TextSpool {
    /// Open the one disposable anchor table before any term-page map apply.
    /// A failed DDL attempt poisons this attempt; no alternate term map exists.
    pub(in crate::native) fn begin_term_anchors(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.term_anchor_state.get() != State::Empty {
            return Err(IndexError::BackendViolation(
                "term anchor preparation was already attempted".into(),
            ));
        }
        self.check_posting_usable()?;
        self.postings.check_page_source(self)?;
        budget.charge(WorkUsage {
            reads: 1,
            writes: 1,
            bytes: 4096_u64
                .checked_add(
                    u64::try_from(CREATE.len()).map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            4096,
            budget,
        )?;
        self.term_anchor_state.set(State::Preparing);
        if let Err(error) = db.execute_batch(CREATE) {
            self.invalidate_term_anchors();
            return Err(sql(&error));
        }
        if let Err(error) = self.postings.accept_top_changes(self) {
            self.invalidate_term_anchors();
            return Err(error);
        }
        Ok(())
    }

    /// Persist at most one canonical page of root-zero anchor edits.
    pub(in crate::native) fn term_anchor_batch(
        &self,
        rows: &[MapEdit],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let result = self.term_anchor_batch_inner(rows, budget);
        if result.is_err() {
            self.invalidate_term_anchors();
        }
        result
    }

    fn term_anchor_batch_inner(
        &self,
        rows: &[MapEdit],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.term_anchor_state.get() != State::Preparing {
            return Err(IndexError::BackendViolation(
                "term anchor batch lacks preparing state".into(),
            ));
        }
        self.check_posting_usable()?;
        self.postings.check_page_source(self)?;
        if rows.is_empty() {
            return Ok(());
        }
        if rows.len() > 64 {
            return Err(IndexError::InvalidInput(
                "term anchor batch exceeds 64 rows".into(),
            ));
        }
        let mut prior: Option<&[u8]> = None;
        for edit in rows {
            budget.checkpoint()?;
            check_term_key(&edit.key)?;
            let object_or_absent =
                |value: Option<MapValue>| matches!(value, None | Some(MapValue::Object(_)));
            if !object_or_absent(edit.expected)
                || !object_or_absent(edit.next)
                || (edit.expected.is_none() && edit.next.is_none())
            {
                return Err(IndexError::InvalidInput(
                    "term anchor is not a changed object reference".into(),
                ));
            }
            if let (Some(expected), Some(next)) = (edit.expected, edit.next) {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(expected.encoded_len()?.min(next.encoded_len()?))
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
            }
            if edit.expected == edit.next {
                return Err(IndexError::InvalidInput(
                    "term anchor is not a changed object reference".into(),
                ));
            }
            if let Some(key) = prior {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(key.len().min(edit.key.len()))
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                if key >= edit.key.as_slice() {
                    return Err(IndexError::InvalidInput(
                        "term anchors are not strictly sorted".into(),
                    ));
                }
            }
            prior = Some(&edit.key);
        }
        let demands = BatchDemands::new(rows, |edit| {
            u64::try_from(edit.key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(map_field_reservation(2, 64)?)
                .ok_or(IndexError::ResourceLimited)
        })?;
        let db = self.db.scoped(budget)?;
        let written = quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            INSERT,
            4096,
            demands.as_slice()?,
            |slot| {
                let edit = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((rusqlite::types::ValueRef::Integer(0), edit, 0))
            },
            budget,
        );
        written?;
        self.postings.accept_top_changes(self)
    }

    pub(in crate::native) fn seal_term_anchors(&self) -> Result<(), IndexError> {
        if self.term_anchor_state.get() != State::Preparing {
            return Err(IndexError::BackendViolation(
                "term anchors cannot seal before complete preparation".into(),
            ));
        }
        self.check_posting_usable()?;
        self.postings.check_page_source(self)?;
        self.term_anchor_state.set(State::Ready);
        Ok(())
    }

    pub(in crate::native) fn invalidate_term_anchors(&self) {
        self.term_anchor_state.set(State::Failed);
        self.inventory_poisoned.set(true);
    }

    /// Read root-zero page anchors followed by the existing root-one owner
    /// rows. The original term-surface `top` rows are never replayed.
    pub(in crate::native) fn term_anchor_owner_cursor(&self) -> TermAnchorOwnerCursor<'_> {
        TermAnchorOwnerCursor {
            spool: self,
            after: None,
            page: EditPage::default(),
            anchors_done: false,
            owner: TopCursor {
                spool: self,
                surface: 1,
                after: None,
                page: EditPage::default(),
            },
        }
    }
}

pub(in crate::native) struct TermAnchorOwnerCursor<'a> {
    spool: &'a TextSpool,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
    anchors_done: bool,
    owner: TopCursor<'a>,
}

impl MapEditCursor for TermAnchorOwnerCursor<'_> {
    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        self.owner.subtree_packing()
    }

    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.spool.term_anchor_state.get() != State::Ready {
            return Err(IndexError::BackendViolation(
                "term anchor stream was not sealed".into(),
            ));
        }
        self.spool.check_posting_usable()?;
        self.spool.postings.check_page_source(self.spool)?;
        while !self.anchors_done {
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                self.anchors_done = true;
                self.after = None;
                self.page = EditPage::default();
                break;
            }
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let db = self.spool.db.scoped(budget)?;
            let (query, after) = self
                .after
                .as_ref()
                .map_or((FIRST, None), |prior| (NEXT, Some(prior.key())));
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            let bound = usize::try_from(MAX_TERM_KEY_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?;
            self.page = match after {
                Some(key) => {
                    EditPage::read(&mut statement, &[&key], 4096, budget, |row, budget| {
                        read_edit(row, bound, 0, budget)
                    })?
                }
                None => EditPage::read(&mut statement, &[], 4096, budget, |row, budget| {
                    read_edit(row, bound, 0, budget)
                })?,
            };
        }
        self.owner.next_edit(budget)
    }
}
