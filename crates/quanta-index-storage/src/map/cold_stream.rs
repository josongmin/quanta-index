//! Validate an empty-base edit stream while building one sorted root at a time.
//! One pending edit crosses a root boundary; no edit history is retained.

use quanta_index_core::index::ports::MapEditCursor;
use quanta_index_core::index::{IndexError, OrdinalMapEdit, WorkBudget, WorkUsage};

use super::MAX_KEY_BYTES;

pub(super) struct EmptyStream<'a> {
    input: &'a mut dyn MapEditCursor,
    roots: usize,
    previous: Option<OrdinalMapEdit>,
    pending: Option<OrdinalMapEdit>,
    exhausted: bool,
}

impl<'a> EmptyStream<'a> {
    pub(super) fn new(input: &'a mut dyn MapEditCursor, roots: usize) -> Self {
        Self {
            input,
            roots,
            previous: None,
            pending: None,
            exhausted: false,
        }
    }

    pub(super) fn root_cursor(&mut self, ordinal: u32) -> RootCursor<'_, 'a> {
        RootCursor {
            stream: self,
            ordinal,
        }
    }

    fn next_for(
        &mut self,
        ordinal: u32,
        budget: &mut WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.exhausted {
            return Ok(None);
        }
        let position = if let Some(position) = self.pending.take() {
            position
        } else if let Some(position) = self.input.next_edit(budget)? {
            let root =
                usize::try_from(position.root()).map_err(|_error| IndexError::ResourceLimited)?;
            if root >= self.roots {
                return Err(IndexError::InvalidInput(
                    "map edit root ordinal is out of bounds".into(),
                ));
            }
            if position.key().len() > MAX_KEY_BYTES
                || self.previous.as_ref().is_some_and(|prior| {
                    prior.root() > position.root()
                        || (prior.root() == position.root()
                            && prior.key() >= position.key())
                })
            {
                return Err(IndexError::InvalidInput(
                    "map edits must be globally sorted and unique".into(),
                ));
            }
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: u64::try_from(position.key().len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            if position.expected()?.is_some() {
                return Err(IndexError::Conflict);
            }
            self.previous = Some(position.clone());
            position
        } else {
            self.exhausted = true;
            return Ok(None);
        };
        if position.root() < ordinal {
            return Err(IndexError::InvalidInput(
                "map edit root ordinal moved backwards".into(),
            ));
        }
        if position.root() == ordinal {
            Ok(Some(position))
        } else {
            self.pending = Some(position);
            Ok(None)
        }
    }

    /// Probe once after the final root; late errors cannot hide behind an empty
    /// last root or a duplicate content-addressed node.
    pub(super) fn finish(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let end = u32::try_from(self.roots).map_err(|_error| IndexError::ResourceLimited)?;
        if self.next_for(end, budget)?.is_some() {
            return Err(IndexError::BackendViolation(
                "empty-root stream continued after final ordinal".into(),
            ));
        }
        Ok(())
    }
}

pub(super) struct RootCursor<'a, 'b> {
    stream: &'a mut EmptyStream<'b>,
    ordinal: u32,
}

impl MapEditCursor for RootCursor<'_, '_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        self.stream.next_for(self.ordinal, budget)
    }
}
