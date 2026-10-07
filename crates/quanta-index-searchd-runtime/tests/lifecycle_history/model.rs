//! Independent set/head model and bounded invocation/response linearizer.
//! No production reducer, result mapper or CAS implementation is used here.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Row {
    pub id: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Head {
    pub generation: u64,
    pub incarnation: [u8; 16],
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Publication {
    pub generation: u64,
    pub base: Option<u64>,
    // Logical empty-corpus operation, implemented as complete source/scope deletion.
    pub clear: bool,
    pub rows: Vec<Row>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Cas {
    Activate,
    Rollback,
}

#[derive(Clone, Debug)]
pub(super) enum Operation {
    Publish(Publication),
    Change {
        kind: Cas,
        generation: u64,
        expected: Option<Head>,
    },
    Query {
        pinned: Option<u64>,
    },
    ReadHead,
    Restart,
}

#[derive(Clone, Debug)]
pub(super) enum Observation {
    Published,
    Changed {
        previous: Option<Head>,
        active: Head,
    },
    Conflict(Cas),
    SourceEventRefusal,
    Rows {
        generation: u64,
        selected: Option<Head>,
        rows: Vec<Row>,
    },
    Head(Option<Head>),
    Failure(String),
}

#[derive(Clone, Debug)]
pub(super) enum Event {
    Invoke { id: usize, operation: Operation },
    Respond { id: usize, observation: Observation },
}

#[derive(Clone, Debug)]
struct Call {
    start: usize,
    end: usize,
    operation: Operation,
    observation: Observation,
}

#[derive(Clone, Debug, Default)]
struct Model {
    generations: BTreeMap<u64, (Publication, BTreeSet<Row>)>,
    active: Option<Head>,
    accepted: BTreeSet<u64>,
    deferred: BTreeMap<u64, Publication>,
}

impl Model {
    fn apply(&mut self, operation: &Operation, observation: &Observation) -> bool {
        match (operation, observation) {
            (Operation::Publish(publication), observed) => {
                if let Some((prior, _)) = self.generations.get(&publication.generation) {
                    return prior == publication && matches!(observed, Observation::Published);
                }
                if self
                    .deferred
                    .get(&publication.generation)
                    .is_some_and(|prior| prior != publication)
                {
                    return false;
                }
                if self
                    .generations
                    .keys()
                    .any(|older| *older < publication.generation && !self.accepted.contains(older))
                {
                    if !matches!(observed, Observation::SourceEventRefusal) {
                        return false;
                    }
                    let prior = self
                        .deferred
                        .insert(publication.generation, publication.clone());
                    return prior.as_ref().is_none_or(|prior| prior == publication);
                }
                if !matches!(observed, Observation::Published) {
                    return false;
                }
                let mut rows = match publication.base {
                    Some(base) => match self.generations.get(&base) {
                        Some((_, rows)) => rows.clone(),
                        None => return false,
                    },
                    None => BTreeSet::new(),
                };
                if publication.clear {
                    rows.clear();
                }
                rows.extend(publication.rows.iter().cloned());
                self.generations
                    .insert(publication.generation, (publication.clone(), rows))
                    .is_none()
            }
            (
                Operation::Change {
                    kind,
                    generation,
                    expected,
                },
                observed,
            ) => {
                let valid_target = self.generations.contains_key(generation)
                    && match kind {
                        Cas::Activate => expected.is_none_or(|head| *generation > head.generation),
                        Cas::Rollback => {
                            self.accepted.contains(generation)
                                && expected.is_some_and(|head| *generation < head.generation)
                        }
                    };
                if !valid_target {
                    return false;
                }
                if self.active != *expected {
                    return matches!(observed, Observation::Conflict(observed_kind) if kind == observed_kind);
                }
                // Public source authority refuses activation of an accepted event,
                // or overtaking any older staged publication for this pair.
                if *kind == Cas::Activate
                    && (self.accepted.contains(generation)
                        || self
                            .generations
                            .keys()
                            .any(|older| older < generation && !self.accepted.contains(older)))
                {
                    return matches!(observed, Observation::SourceEventRefusal);
                }
                let Observation::Changed { previous, active } = observed else {
                    return false;
                };
                let next_sequence = match self.active {
                    Some(prior) => {
                        if prior.incarnation != active.incarnation {
                            return false;
                        }
                        let Some(next) = prior.sequence.checked_add(1) else {
                            return false;
                        };
                        next
                    }
                    None => 1,
                };
                if *previous != self.active
                    || active.generation != *generation
                    || active.sequence != next_sequence
                    || active.incarnation == [0; 16]
                {
                    return false;
                }
                self.active = Some(*active);
                if *kind == Cas::Activate && !self.accepted.insert(*generation) {
                    return false;
                }
                true
            }
            (
                Operation::Query { pinned },
                Observation::Rows {
                    generation,
                    selected,
                    rows,
                },
            ) => {
                let expected_generation = pinned.or(self.active.map(|head| head.generation));
                if expected_generation != Some(*generation)
                    || *selected != if pinned.is_some() { None } else { self.active }
                {
                    return false;
                }
                self.generations
                    .get(generation)
                    .is_some_and(|(_, expected)| {
                        // Preserve duplicates: a duplicated result must not vanish in set conversion.
                        rows.iter().eq(expected.iter())
                    })
            }
            (Operation::ReadHead | Operation::Restart, Observation::Head(head)) => {
                *head == self.active
            }
            _ => false,
        }
    }
}

fn calls(events: &[Event]) -> Result<Vec<Call>, String> {
    if events.is_empty() {
        return Err("empty history".into());
    }
    let mut pending = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut calls = Vec::new();
    for (tick, event) in events.iter().enumerate() {
        match event {
            Event::Invoke { id, operation } => {
                if !seen.insert(*id) {
                    return Err(format!("duplicate invocation {id}"));
                }
                if pending.insert(*id, (tick, operation.clone())).is_some() {
                    return Err(format!("duplicate pending invocation {id}"));
                }
            }
            Event::Respond { id, observation } => {
                if let Observation::Failure(message) = observation {
                    return Err(format!("operation {id} failed: {message}"));
                }
                let Some((start, operation)) = pending.remove(id) else {
                    return Err(format!("orphan or duplicate response {id}"));
                };
                calls.push(Call {
                    start,
                    end: tick,
                    operation,
                    observation: observation.clone(),
                });
            }
        }
    }
    if !pending.is_empty() {
        return Err(format!("missing responses: {:?}", pending.keys()));
    }
    if calls.len() > 64 {
        return Err("history exceeds 64-operation checker bound".into());
    }
    Ok(calls)
}

/// Search all legal orders subject to response-before-invocation precedence.
/// Exceeding the search bound is an error, never evidence of linearizability.
pub(super) fn check(events: &[Event], limit: usize) -> Result<(), String> {
    fn visit(
        calls: &[Call],
        done: u64,
        model: &Model,
        remaining: &mut usize,
    ) -> Result<bool, String> {
        *remaining = remaining
            .checked_sub(1)
            .ok_or("linearization search bound exhausted")?;
        let mut complete = true;
        for (index, call) in calls.iter().enumerate() {
            let bit = 1_u64
                .checked_shl(u32::try_from(index).map_err(|e| e.to_string())?)
                .ok_or("history bit overflow")?;
            if done & bit != 0 {
                continue;
            }
            complete = false;
            let mut eligible = true;
            for (other_index, other) in calls.iter().enumerate() {
                let other_bit = 1_u64
                    .checked_shl(u32::try_from(other_index).map_err(|e| e.to_string())?)
                    .ok_or("history bit overflow")?;
                if done & other_bit == 0 && other.end < call.start {
                    eligible = false;
                    break;
                }
            }
            if !eligible {
                continue;
            }
            let mut next = model.clone();
            if next.apply(&call.operation, &call.observation)
                && visit(calls, done | bit, &next, remaining)?
            {
                return Ok(true);
            }
        }
        Ok(complete)
    }
    let calls = calls(events)?;
    let mut remaining = limit;
    if visit(&calls, 0, &Model::default(), &mut remaining)? {
        Ok(())
    } else {
        Err(format!("no legal linearization: {events:#?}"))
    }
}
