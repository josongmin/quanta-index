//! Fixed counterexamples establish that the checker can reject broken histories.
use super::model::{self, Cas, Event, Head, Observation, Operation};
use super::{publication, row};

fn head(generation: u64, sequence: u64) -> Head {
    Head {
        generation,
        incarnation: [7; 16],
        sequence,
    }
}

fn pair(events: &mut Vec<Event>, operation: Operation, observation: Observation) {
    let id = events.len();
    events.push(Event::Invoke { id, operation });
    events.push(Event::Respond { id, observation });
}

fn prefix() -> Vec<Event> {
    let mut events = Vec::new();
    pair(
        &mut events,
        Operation::Publish(publication(1, None, false, &["a", "b"])),
        Observation::Published,
    );
    pair(
        &mut events,
        Operation::Change {
            kind: Cas::Activate,
            generation: 1,
            expected: None,
        },
        Observation::Changed {
            previous: None,
            active: head(1, 1),
        },
    );
    pair(
        &mut events,
        Operation::Publish(publication(2, Some(1), false, &["c"])),
        Observation::Published,
    );
    events
}

fn activate(events: &mut Vec<Event>, previous: Head, active: Head) {
    pair(
        events,
        Operation::Change {
            kind: Cas::Activate,
            generation: active.generation,
            expected: Some(previous),
        },
        Observation::Changed {
            previous: Some(previous),
            active,
        },
    );
}

fn old_query() -> Observation {
    Observation::Rows {
        generation: 1,
        selected: Some(head(1, 1)),
        rows: vec![row("a"), row("b")],
    }
}

#[test]
fn accepts_overlapping_old_query_even_when_it_completes_after_activation() {
    let mut events = prefix();
    events.push(Event::Invoke {
        id: 100,
        operation: Operation::Query { pinned: None },
    });
    activate(&mut events, head(1, 1), head(2, 2));
    events.push(Event::Respond {
        id: 100,
        observation: old_query(),
    });
    assert!(model::check(&events, 100_000).is_ok());
}

#[test]
fn checks_source_publication_order_and_accepted_event_refusal() {
    let mut pending = prefix();
    let attempt = Operation::Publish(publication(3, None, false, &["d"]));
    let mut refused = pending.clone();
    pair(
        &mut refused,
        attempt.clone(),
        Observation::SourceEventRefusal,
    );
    assert!(model::check(&refused, 100_000).is_ok());
    pair(&mut pending, attempt.clone(), Observation::Published);
    assert!(model::check(&pending, 100_000).is_err());
    activate(&mut refused, head(1, 1), head(2, 2));
    let mut unnecessary = refused.clone();
    pair(
        &mut unnecessary,
        attempt.clone(),
        Observation::SourceEventRefusal,
    );
    assert!(model::check(&unnecessary, 100_000).is_err());
    let mut changed_retry = refused.clone();
    pair(
        &mut changed_retry,
        Operation::Publish(publication(3, None, false, &["foreign"])),
        Observation::Published,
    );
    assert!(model::check(&changed_retry, 100_000).is_err());
    pair(&mut refused, attempt, Observation::Published);
    activate(&mut refused, head(2, 2), head(3, 3));
    assert!(model::check(&refused, 100_000).is_ok());
    pair(
        &mut refused,
        Operation::Change {
            kind: Cas::Rollback,
            generation: 1,
            expected: Some(head(3, 3)),
        },
        Observation::Changed {
            previous: Some(head(3, 3)),
            active: head(1, 4),
        },
    );
    pair(
        &mut refused,
        Operation::Change {
            kind: Cas::Activate,
            generation: 2,
            expected: Some(head(1, 4)),
        },
        Observation::SourceEventRefusal,
    );
    assert!(model::check(&refused, 100_000).is_ok());
}

#[test]
fn rejects_stale_query_after_completed_activation_and_duplicate_cas_winners() {
    let mut stale = prefix();
    activate(&mut stale, head(1, 1), head(2, 2));
    pair(&mut stale, Operation::Query { pinned: None }, old_query());
    assert!(model::check(&stale, 100_000).is_err());

    let mut double = prefix();
    for id in [100, 101] {
        double.push(Event::Invoke {
            id,
            operation: Operation::Change {
                kind: Cas::Activate,
                generation: 2,
                expected: Some(head(1, 1)),
            },
        });
    }
    for id in [100, 101] {
        double.push(Event::Respond {
            id,
            observation: Observation::Changed {
                previous: Some(head(1, 1)),
                active: head(2, 2),
            },
        });
    }
    assert!(model::check(&double, 100_000).is_err());
}

#[test]
fn rejects_missing_clear_duplicate_append_and_foreign_pinned_rows() {
    let mut cleared = prefix();
    activate(&mut cleared, head(1, 1), head(2, 2));
    pair(
        &mut cleared,
        Operation::Publish(publication(3, Some(2), true, &[])),
        Observation::Published,
    );
    let mut correct = cleared.clone();
    pair(
        &mut correct,
        Operation::Query { pinned: Some(3) },
        Observation::Rows {
            generation: 3,
            selected: None,
            rows: Vec::new(),
        },
    );
    assert!(model::check(&correct, 100_000).is_ok());
    pair(
        &mut cleared,
        Operation::Query { pinned: Some(3) },
        Observation::Rows {
            generation: 3,
            selected: None,
            rows: vec![row("a")],
        },
    );
    assert!(model::check(&cleared, 100_000).is_err());
    for rows in [
        vec![row("a"), row("b"), row("c"), row("c")],
        vec![row("foreign")],
    ] {
        let mut appended = prefix();
        pair(
            &mut appended,
            Operation::Query { pinned: Some(2) },
            Observation::Rows {
                generation: 2,
                selected: None,
                rows,
            },
        );
        assert!(model::check(&appended, 100_000).is_err());
    }
}

#[test]
fn rejects_aba_stale_token_sequence_jump_and_restart_head_loss() {
    let mut events = prefix();
    activate(&mut events, head(1, 1), head(2, 2));
    pair(
        &mut events,
        Operation::Change {
            kind: Cas::Rollback,
            generation: 1,
            expected: Some(head(2, 2)),
        },
        Observation::Changed {
            previous: Some(head(2, 2)),
            active: head(1, 3),
        },
    );
    assert!(model::check(&events, 100_000).is_ok());
    let mut stale = events.clone();
    activate(&mut stale, head(1, 1), head(2, 4));
    assert!(model::check(&stale, 100_000).is_err());
    let mut lost = events;
    pair(
        &mut lost,
        Operation::Restart,
        Observation::Head(Some(head(1, 1))),
    );
    assert!(model::check(&lost, 100_000).is_err());
    for active in [
        head(2, 10),
        Head {
            incarnation: [8; 16],
            ..head(2, 2)
        },
    ] {
        let mut jump = prefix();
        activate(&mut jump, head(1, 1), active);
        assert!(model::check(&jump, 100_000).is_err());
    }
}

#[test]
fn rejects_changed_replay_unknown_target_and_overlong_history() {
    let mut replay = prefix();
    pair(
        &mut replay,
        Operation::Publish(publication(2, Some(1), false, &["c"])),
        Observation::Published,
    );
    assert!(model::check(&replay, 100_000).is_ok());
    pair(
        &mut replay,
        Operation::Publish(publication(2, Some(1), false, &["foreign"])),
        Observation::Published,
    );
    assert!(model::check(&replay, 100_000).is_err());

    let mut unknown = prefix();
    pair(
        &mut unknown,
        Operation::Change {
            kind: Cas::Activate,
            generation: 99,
            expected: Some(head(1, 1)),
        },
        Observation::Conflict(Cas::Activate),
    );
    assert!(model::check(&unknown, 100_000).is_err());

    let mut overlong = prefix();
    for _ in 0..62 {
        pair(
            &mut overlong,
            Operation::ReadHead,
            Observation::Head(Some(head(1, 1))),
        );
    }
    assert!(model::check(&overlong, 100_000).is_err());
}

#[test]
fn rejects_partial_duplicate_orphan_empty_and_exhausted_histories() {
    assert!(model::check(&[], 100_000).is_err());
    let mut missing = prefix();
    missing.push(Event::Invoke {
        id: 100,
        operation: Operation::ReadHead,
    });
    assert!(model::check(&missing, 100_000).is_err());
    let mut duplicate = prefix();
    duplicate.push(Event::Respond {
        id: 0,
        observation: Observation::Published,
    });
    assert!(model::check(&duplicate, 100_000).is_err());
    let orphan = vec![Event::Respond {
        id: 100,
        observation: Observation::Published,
    }];
    assert!(model::check(&orphan, 100_000).is_err());
    let mut duplicate_invocation = prefix();
    duplicate_invocation.push(Event::Invoke {
        id: 0,
        operation: Operation::ReadHead,
    });
    assert!(model::check(&duplicate_invocation, 100_000).is_err());
    assert!(model::check(&prefix(), 0).is_err());
}
