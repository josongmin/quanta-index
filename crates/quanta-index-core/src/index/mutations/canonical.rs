//! Canonical JSON commitment without a second owned serialization buffer.
use std::io::{self, Write};

use sha2::{Digest as _, Sha256};

use super::{IndexError, OwnerMutation, WorkBudget};

// Bound cancellation/deadline observation by canonical bytes, not by serde's
// fragment count. A single large Write buffer is split at this boundary.
const CHECKPOINT_BYTES: usize = 256;

enum Target<'a> {
    Count,
    Hash(&'a mut Sha256),
    Compare(&'a [u8]),
}
struct Writer<'a> {
    target: Target<'a>,
    len: usize,
    equal: bool,
    budget: Option<&'a WorkBudget>,
    since_checkpoint: usize,
    error: Option<IndexError>,
}
impl Writer<'_> {
    fn refuse(&mut self, error: IndexError) -> io::Error {
        self.error = Some(error);
        io::Error::other("mutation commitment refused")
    }

    fn apply(&mut self, bytes: &[u8], next: usize) {
        match &mut self.target {
            Target::Count => {}
            Target::Hash(hash) => hash.update(bytes),
            Target::Compare(expected) => {
                self.equal &= expected.get(self.len..next) == Some(bytes);
            }
        }
        self.len = next;
    }
}
impl Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(next) = self.len.checked_add(bytes.len()) else {
            if let Some(budget) = self.budget
                && let Err(error) = budget.checkpoint()
            {
                return Err(self.refuse(error));
            }
            return Err(self.refuse(IndexError::ResourceLimited));
        };
        let Some(budget) = self.budget else {
            self.apply(bytes, next);
            return Ok(bytes.len());
        };
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let available = CHECKPOINT_BYTES
                .checked_sub(self.since_checkpoint)
                .filter(|bytes| *bytes != 0)
                .ok_or_else(|| self.refuse(IndexError::ResourceLimited))?;
            let take = remaining.len().min(available);
            let (chunk, rest) = remaining.split_at(take);
            let chunk_end = self
                .len
                .checked_add(take)
                .ok_or_else(|| self.refuse(IndexError::ResourceLimited))?;
            let since_checkpoint = self
                .since_checkpoint
                .checked_add(take)
                .ok_or_else(|| self.refuse(IndexError::ResourceLimited))?;
            self.apply(chunk, chunk_end);
            self.since_checkpoint = since_checkpoint;
            if self.since_checkpoint == CHECKPOINT_BYTES {
                if let Err(error) = budget.checkpoint() {
                    return Err(self.refuse(error));
                }
                self.since_checkpoint = 0;
            }
            remaining = rest;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn write(
    mutation: &OwnerMutation,
    target: Target<'_>,
    budget: Option<&WorkBudget>,
) -> Result<(u64, bool), IndexError> {
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    let mut writer = Writer {
        target,
        len: 0,
        equal: true,
        budget,
        since_checkpoint: 0,
        error: None,
    };
    let result = serde_json::to_writer(&mut writer, mutation);
    if let Some(error) = writer.error {
        return Err(error);
    }
    result
        .map_err(|error| IndexError::InvalidInput(format!("owner mutation encoding: {error}")))?;
    if let Some(budget) = writer.budget {
        budget.checkpoint()?;
    }
    let complete = match writer.target {
        Target::Compare(expected) => expected.len() == writer.len,
        Target::Count | Target::Hash(_) => true,
    };
    Ok((
        u64::try_from(writer.len).map_err(|_error| IndexError::ResourceLimited)?,
        writer.equal && complete,
    ))
}
pub(super) fn length(
    mutation: &OwnerMutation,
    budget: Option<&WorkBudget>,
) -> Result<u64, IndexError> {
    write(mutation, Target::Count, budget).map(|(len, _equal)| len)
}
pub(super) fn hash(
    mutation: &OwnerMutation,
    hash: &mut Sha256,
    budget: Option<&WorkBudget>,
) -> Result<u64, IndexError> {
    write(mutation, Target::Hash(hash), budget).map(|(len, _equal)| len)
}
pub(super) fn matches(
    mutation: &OwnerMutation,
    expected: &[u8],
    budget: &WorkBudget,
) -> Result<bool, IndexError> {
    write(mutation, Target::Compare(expected), Some(budget)).map(|(_len, equal)| equal)
}
