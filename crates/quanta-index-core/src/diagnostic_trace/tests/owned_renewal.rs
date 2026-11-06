use super::*;

fn observation() -> DiagnosticOwnedRenewalRefusalV1 {
    DiagnosticOwnedRenewalRefusalV1 {
        reason: DiagnosticOwnedRenewalRefusalReasonV1::LeaseExpired,
        now_ms: 2_000,
        lease_until_ms: Some(2_000),
        deadline_ms: None,
    }
}

#[test]
fn renewal_refusal_uses_the_bound_parent_and_original_values()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, _, sink) = context();
    let root = owner.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let parent = owner.child_context(&root);
    let renewal = parent.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(29))?;
    let renewal_id = renewal.span_id();
    let child = parent.child_context(&renewal);
    child.owned_renewal_refusal(observation());
    renewal.finish(DiagnosticTerminalV1::Error { code: 1 })?;
    drop(child);
    drop(parent);
    root.finish(DiagnosticTerminalV1::Ok)?;
    let events = recorded_events(&sink);
    let observations: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let DiagnosticEventKindV1::OwnedRenewalRefusal {
                span_id,
                observation,
            } = event.kind
            {
                Some((span_id, observation))
            } else {
                None
            }
        })
        .collect();
    if observations != [(renewal_id, observation())] {
        return Err("renewal observation lost its current parent or original values".into());
    }
    Ok(())
}

#[test]
fn renewal_refusal_rejects_parentless_and_foreign_contexts()
-> Result<(), Box<dyn std::error::Error>> {
    let (parentless, _, sink) = context();
    parentless.owned_renewal_refusal(observation());
    let events = recorded_events(&sink);
    if events.len() != 1
        || !events.iter().any(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::Health {
                    code: DiagnosticHealthCodeV1::MissingParentSpan,
                }
            )
        })
    {
        return Err("parentless refusal fabricated an owner".into());
    }
    let (owner, _, _) = context();
    let root = owner.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let (foreign, _, foreign_sink) = context();
    let invalid = foreign.child_context(&root);
    invalid.owned_renewal_refusal(observation());
    root.finish(DiagnosticTerminalV1::Ok)?;
    let events = recorded_events(&foreign_sink);
    if events.iter().any(|event| {
        matches!(
            event.kind,
            DiagnosticEventKindV1::OwnedRenewalRefusal { .. }
        )
    }) || !events.iter().any(|event| {
        matches!(
            event.kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::ForeignCaptureSpan,
            }
        )
    }) {
        return Err("foreign refusal fabricated an owner".into());
    }
    Ok(())
}
