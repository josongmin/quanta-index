//! Sparse token phrase matching for one History scoring plan.

use std::collections::BTreeMap;

use quanta_index_core::CoreError;

/// Trie edges and failure links are bounded by the query's prepared terms.
pub(super) struct PhraseAutomaton {
    edges: BTreeMap<(usize, usize), usize>,
    failure: Vec<usize>,
    breadth_first: Vec<usize>,
    terminals: Vec<usize>,
}

impl PhraseAutomaton {
    pub(super) fn new(phrases: &[Vec<usize>]) -> Self {
        let mut edges = BTreeMap::new();
        let mut terminals = Vec::with_capacity(phrases.len());
        for phrase in phrases {
            let mut state = 0;
            for &term in phrase {
                let next = edges.len() + 1;
                state = *edges.entry((state, term)).or_insert(next);
            }
            terminals.push(state);
        }

        let mut failure = vec![0; edges.len() + 1];
        let mut breadth_first = Vec::with_capacity(edges.len());
        breadth_first.extend(
            edges
                .range((0, 0)..=(0, usize::MAX))
                .map(|(_, &child)| child),
        );
        let mut head = 0;
        while head < breadth_first.len() {
            let state = breadth_first[head];
            head += 1;
            for (&(_, term), &child) in edges.range((state, 0)..=(state, usize::MAX)) {
                failure[child] = transition(&edges, &failure, failure[state], term);
                breadth_first.push(child);
            }
        }
        Self {
            edges,
            failure,
            breadth_first,
            terminals,
        }
    }

    pub(super) fn counter(&self) -> PhraseCounter<'_> {
        PhraseCounter {
            automaton: self,
            state: 0,
            visits: vec![0; self.failure.len()],
        }
    }
}

fn transition(
    edges: &BTreeMap<(usize, usize), usize>,
    failure: &[usize],
    mut state: usize,
    term: usize,
) -> usize {
    loop {
        if let Some(&next) = edges.get(&(state, term)) {
            return next;
        }
        if state == 0 {
            return 0;
        }
        state = failure[state];
    }
}

pub(super) struct PhraseCounter<'a> {
    automaton: &'a PhraseAutomaton,
    state: usize,
    visits: Vec<u64>,
}

impl PhraseCounter<'_> {
    /// Unknown or oversized tokens break phrase adjacency.
    pub(super) fn push(&mut self, term: Option<usize>) -> Result<(), CoreError> {
        let Some(term) = term else {
            self.state = 0;
            return Ok(());
        };
        self.state = transition(
            &self.automaton.edges,
            &self.automaton.failure,
            self.state,
            term,
        );
        if self.state != 0 {
            self.visits[self.state] = self.visits[self.state]
                .checked_add(1)
                .ok_or_else(|| CoreError::InvalidContract("History token count overflow".into()))?;
        }
        Ok(())
    }

    /// Failure accumulation counts terminal suffixes without per-token output walks.
    pub(super) fn finish(mut self) -> Result<Vec<u32>, CoreError> {
        for &state in self.automaton.breadth_first.iter().rev() {
            let parent = self.automaton.failure[state];
            if parent != 0 {
                self.visits[parent] = self.visits[parent]
                    .checked_add(self.visits[state])
                    .ok_or_else(|| {
                        CoreError::InvalidContract("History term frequency overflow".into())
                    })?;
            }
        }
        self.automaton
            .terminals
            .iter()
            .map(|&state| {
                u32::try_from(self.visits[state]).map_err(|_| {
                    CoreError::InvalidContract("History term frequency overflow".into())
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::PhraseAutomaton;
    use quanta_index_core::CoreError;

    #[test]
    fn sparse_automaton_matches_fixed_window_reference() -> Result<(), CoreError> {
        let phrases = vec![
            vec![1, 1],
            vec![1, 1],
            vec![1],
            vec![2, 1],
            vec![1, 2, 1],
            vec![3],
        ];
        let stream = [
            Some(1),
            Some(1),
            Some(2),
            Some(1),
            None,
            Some(1),
            Some(1),
            Some(3),
        ];
        let reference: Vec<u32> = phrases
            .iter()
            .map(|phrase| {
                stream
                    .windows(phrase.len())
                    .filter(|window| {
                        window
                            .iter()
                            .zip(phrase)
                            .all(|(found, wanted)| *found == Some(*wanted))
                    })
                    .count() as u32
            })
            .collect();
        assert_eq!(reference, vec![2, 2, 5, 1, 1, 1]);
        let automaton = PhraseAutomaton::new(&phrases);
        for _ in 0..2 {
            let mut counter = automaton.counter();
            for term in stream {
                counter.push(term)?;
            }
            assert_eq!(counter.finish()?, reference);
        }
        Ok(())
    }
}
