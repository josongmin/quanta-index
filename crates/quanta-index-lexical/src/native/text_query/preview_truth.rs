//! Positive body leaves of a ranked match, selected from the ranker's truth.

use quanta_index_core::index::IndexError;

use super::{Clause, MAX_QUERY_DEPTH, MAX_QUERY_NODES};

#[derive(Clone, Copy, Default)]
pub(super) struct ClauseTruth([u64; 4]);

impl ClauseTruth {
    pub(super) fn record(&mut self, ordinal: usize, matched: bool) -> Result<(), IndexError> {
        if matched {
            let word = self.0.get_mut(ordinal >> 6).ok_or_else(bad_program)?;
            *word |= 1u64 << (ordinal & 63);
        }
        Ok(())
    }

    fn matched(&self, ordinal: usize) -> Result<bool, IndexError> {
        let word = self.0.get(ordinal >> 6).ok_or_else(bad_program)?;
        Ok(word & (1u64 << (ordinal & 63)) != 0)
    }

    /// Walk the existing postfix program backward. Every child of a true AND
    /// contributes; a true OR contributes its true children. No descendant of
    /// NOT supplies a body range, regardless of its matcher result.
    pub(super) fn visit_positive_body(
        &self,
        program: &[Clause],
        mut visit: impl FnMut(Clause) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if program.is_empty() || program.len() > MAX_QUERY_NODES {
            return Err(bad_program());
        }
        let mut cursors = [(0usize, false); MAX_QUERY_DEPTH + 2];
        let mut depth = 1usize;
        *cursors.first_mut().ok_or_else(bad_program)? = (1, true);
        for (ordinal, &clause) in program.iter().enumerate().rev() {
            let contributes = loop {
                let last = depth.checked_sub(1).ok_or_else(bad_program)?;
                let (left, enabled) = cursors.get_mut(last).ok_or_else(bad_program)?;
                if let Some(next) = left.checked_sub(1) {
                    *left = next;
                    break *enabled && self.matched(ordinal)?;
                }
                depth = last;
            };
            let (children, include) = match clause {
                Clause::All(count) | Clause::Any(count) => (count, contributes),
                Clause::Not => (1, false),
                Clause::Term(_) | Clause::Phrase { .. } | Clause::Regex(_) => {
                    if contributes {
                        visit(clause)?;
                    }
                    continue;
                }
                Clause::Contains(_) | Clause::Repo(_) => continue,
            };
            *cursors.get_mut(depth).ok_or_else(bad_program)? = (children, include);
            depth = depth.checked_add(1).ok_or_else(bad_program)?;
        }
        if cursors
            .iter()
            .take(depth)
            .any(|(left, _enabled)| *left != 0)
        {
            return Err(bad_program());
        }
        Ok(())
    }
}

fn bad_program() -> IndexError {
    IndexError::BackendViolation("native preview truth differs from compiled program".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::text_query::{MatchState, evaluate_boolean_with_repo};

    #[test]
    fn rank_truth_projects_only_contributing_body_leaves() -> Result<(), IndexError> {
        // (A AND B) OR regex OR phrase OR NOT (contains AND repository).
        // A matches alone. Both the false AND and the true NOT must be silent.
        let program = [
            Clause::Term(0),
            Clause::Term(1),
            Clause::All(2),
            Clause::Regex(0),
            Clause::Phrase {
                first: 2,
                count: 2,
                cap: 5,
            },
            Clause::Contains(0),
            Clause::Repo(0),
            Clause::All(2),
            Clause::Not,
            Clause::Any(4),
        ];
        let mut truth = ClauseTruth::default();
        let ranked = evaluate_boolean_with_repo(
            &program,
            0b1101,
            1,
            1,
            0,
            |ordinal, matched| truth.record(ordinal, matched),
            |_, _, _, _| {
                Ok(MatchState {
                    matched: true,
                    positive_terms: 0b1100,
                })
            },
        )?;
        let mut witnesses = Vec::new();
        truth.visit_positive_body(&program, |clause| {
            match clause {
                Clause::Phrase { first, count, .. } => witnesses.push(("phrase", first, count)),
                Clause::Regex(index) => witnesses.push(("regex", index, 0)),
                _ => return Err(bad_program()),
            }
            Ok(())
        })?;
        if !ranked.matched
            || ranked.positive_terms != 0b1100
            || witnesses != [("phrase", 2, 2), ("regex", 0, 0)]
        {
            return Err(bad_program());
        }
        Ok(())
    }
}
