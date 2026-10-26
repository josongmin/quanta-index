//! Canonical JSON commitment without a second owned serialization buffer.
use std::io::{self, Write};

use sha2::{Digest as _, Sha256};

use super::{IndexError, OwnerMutation, WorkBudget};

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
    error: Option<IndexError>,
}
impl Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .budget
            .map_or(Ok(()), WorkBudget::checkpoint)
            .and_then(|()| {
                self.len
                    .checked_add(bytes.len())
                    .ok_or(IndexError::ResourceLimited)
            });
        let next = match next {
            Ok(next) => next,
            Err(error) => {
                self.error = Some(error);
                return Err(io::Error::other("mutation commitment refused"));
            }
        };
        match &mut self.target {
            Target::Count => {}
            Target::Hash(hash) => hash.update(bytes),
            Target::Compare(expected) => {
                self.equal &= expected.get(self.len..next) == Some(bytes);
            }
        }
        self.len = next;
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
    let mut writer = Writer {
        target,
        len: 0,
        equal: true,
        budget,
        error: None,
    };
    let result = serde_json::to_writer(&mut writer, mutation);
    if let Some(error) = writer.error {
        return Err(error);
    }
    result
        .map_err(|error| IndexError::InvalidInput(format!("owner mutation encoding: {error}")))?;
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
