//! Attempt-local ordered token traversal over one live scratch cursor.

use std::cell::Cell;

use quanta_index_core::index::{IndexError, WorkBudget, WorkUsage};

use super::{TextSpool, check_term_key, sql};
use crate::native::{text::PreparedPositions, text_memory::PreparedKey};

struct TokenReadGuard<'a>(&'a Cell<bool>);

impl<'a> TokenReadGuard<'a> {
    fn enter(active: &'a Cell<bool>) -> Result<Self, IndexError> {
        if active.replace(true) {
            return Err(IndexError::BackendViolation(
                "reentrant ordered token traversal".into(),
            ));
        }
        Ok(Self(active))
    }
}

impl Drop for TokenReadGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(super) fn visit(
    spool: &TextSpool,
    limit: usize,
    budget: &mut WorkBudget,
    mut consume: impl FnMut(PreparedKey, PreparedPositions, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let _bounds = PreparedPositions::new(limit)?;
    let _guard = TokenReadGuard::enter(&spool.token_read_active)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let io_scope = spool.db.scoped(budget)?;
    let mut statement = io_scope
        .prepare_cached("SELECT term,pos FROM tokens ORDER BY term,pos")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    let mut current: Option<(PreparedKey, PreparedPositions)> = None;
    loop {
        budget.checkpoint()?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let term = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(0).map_err(|error| sql(&error))?,
            usize::try_from(super::MAX_TERM_KEY_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?,
        )?;
        if let Some((key, positions)) = current.as_mut() {
            match term.cmp(key.as_slice()) {
                std::cmp::Ordering::Less => {
                    return Err(IndexError::CorruptObject(
                        "ordered token groups regressed".into(),
                    ));
                }
                std::cmp::Ordering::Equal => {
                    if positions.as_slice().len() >= limit {
                        return Err(IndexError::ResourceLimited);
                    }
                    budget.charge(WorkUsage {
                        reads: 1,
                        bytes: 16,
                        ..WorkUsage::default()
                    })?;
                    let raw: i64 = row.get(1).map_err(|error| sql(&error))?;
                    let position = u32::try_from(raw).map_err(|_error| {
                        IndexError::CorruptObject("invalid token position".into())
                    })?;
                    if positions
                        .as_slice()
                        .last()
                        .is_some_and(|prior| *prior >= position)
                    {
                        return Err(IndexError::CorruptObject(
                            "token positions are not strictly ordered".into(),
                        ));
                    }
                    positions.push(position, budget)?;
                    continue;
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        check_term_key(term)?;
        let key = PreparedKey::copy(term, budget)?;
        let mut positions = PreparedPositions::new(limit)?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 16,
            ..WorkUsage::default()
        })?;
        let raw: i64 = row.get(1).map_err(|error| sql(&error))?;
        let position = u32::try_from(raw)
            .map_err(|_error| IndexError::CorruptObject("invalid token position".into()))?;
        positions.push(position, budget)?;
        if let Some((prior_key, prior_positions)) = current.replace((key, positions)) {
            consume(prior_key, prior_positions, budget)?;
        }
    }
    if let Some((key, positions)) = current {
        consume(key, positions, budget)?;
    }
    budget.checkpoint()
}
