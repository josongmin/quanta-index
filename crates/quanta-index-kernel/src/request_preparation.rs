//! Request serialization, physical edges and publication-copy preparation.

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectChildren, ProducerCompleteness, SourceEvent,
    UpdateKind, UpdateRequest, WorkBudget, WorkScope,
};

use crate::staging_payload::StagedPayload;

pub(crate) fn encode(
    request: &UpdateRequest,
    upper_bound: u64,
    budget: &WorkBudget,
) -> Result<StagedPayload, IndexError> {
    budget.checkpoint()?;
    let limit = usize::try_from(upper_bound).map_err(|_error| IndexError::ResourceLimited)?;
    crate::staging_payload::encode(request, limit, budget)
}

/// Request dependencies are a sorted, deduplicated set. Historical expected
/// roots and stamps are deliberately absent, matching the existing wire.
pub(crate) fn children(request: &UpdateRequest) -> ObjectChildren {
    let references = match &request.kind {
        UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. } => {
            [transition.owners.root.0, None, None, None]
        }
        UpdateKind::ProjectionAdvance(transition) => {
            let certificate = match transition.completeness {
                ProducerCompleteness::CompleteAffectedDomain { certificate } => Some(certificate),
                ProducerCompleteness::Partial { .. } => None,
            };
            [transition.owners.root.0, certificate, None, None]
        }
        UpdateKind::Reconfigure { profile } => [Some(*profile), None, None, None],
        UpdateKind::Materialize { claim } => match claim.intent.scope {
            WorkScope::Owner { expected, .. } => [expected.source, expected.projection, None, None],
            WorkScope::ProjectionDomain { .. } => [None; 4],
        },
    };
    ObjectChildren::from_references(references)
}

/// Temporary backing for the separate event copy consumed synchronously by
/// Catalog commit. Keep this owner alive across commit after taking its data.
pub(crate) struct PreparedEvents {
    events: Vec<SourceEvent>,
    // Data is destroyed before its reservation if preparation is abandoned.
    _memory: Option<MemoryLease>,
}

impl PreparedEvents {
    pub(crate) fn copy(events: &[SourceEvent], budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let bytes = events
            .len()
            .checked_mul(std::mem::size_of::<SourceEvent>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut copied = Vec::new();
        copied
            .try_reserve_exact(events.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        copied.extend_from_slice(events);
        budget.checkpoint()?;
        Ok(Self {
            events: copied,
            _memory: memory,
        })
    }

    pub(crate) fn take(&mut self) -> Vec<SourceEvent> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::staging_payload::tests::{require, scoped_budget};
    use quanta_index_core::index::{
        CollectionId, Digest, IndexInstanceId, MapRoot, ObjectRef, OperationKey, OwnerInputBinding,
        OwnerKey, OwnerMutationSet, ProducerId, ProjectionId, ProjectionTransition, SchemaId,
        SlotId, SourceCut, SourceTransition, StableOwnerId, StreamId, TenantId, ViewRef, WorkClaim,
        WorkIntent, WorkspaceKey, WorktreeId,
    };

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    fn workspace() -> WorkspaceKey {
        WorkspaceKey {
            tenant: TenantId([255; 16]),
            collection: CollectionId([255; 16]),
            worktree: WorktreeId([255; 16]),
            slot: SlotId([255; 16]),
        }
    }

    fn request(kind: UpdateKind) -> UpdateRequest {
        UpdateRequest {
            operation: OperationKey {
                workspace: workspace(),
                epoch: u64::MAX,
                producer: ProducerId([255; 16]),
                stream: StreamId([255; 16]),
                stream_epoch: u64::MAX,
                sequence: u64::MAX,
            },
            expected_view: ViewRef {
                workspace: workspace(),
                epoch: u64::MAX,
                sequence: u64::MAX,
                digest: Digest([255; 32]),
            },
            kind,
        }
    }

    fn historical_cut() -> SourceCut {
        SourceCut {
            owners: MapRoot(Some(reference(90))),
            paths: MapRoot(Some(reference(91))),
            frontiers: MapRoot(Some(reference(92))),
        }
    }

    fn owner_set(root: Option<ObjectRef>) -> OwnerMutationSet {
        OwnerMutationSet {
            root: MapRoot(root),
            count: u64::MAX,
            digest: Digest([255; 32]),
        }
    }

    fn event() -> SourceEvent {
        SourceEvent {
            producer: ProducerId([255; 16]),
            stream: StreamId([255; 16]),
            stream_epoch: u64::MAX,
            predecessor: u64::MAX,
            sequence: u64::MAX,
            payload: Digest([255; 32]),
        }
    }

    fn projection(completeness: ProducerCompleteness) -> UpdateKind {
        UpdateKind::ProjectionAdvance(Box::new(ProjectionTransition {
            projection: ProjectionId([255; 16]),
            producer: ProducerId([255; 16]),
            expected: None,
            target_source: historical_cut(),
            schema: SchemaId([255; 16]),
            producer_policy: Digest([255; 32]),
            dependency_context: Digest([255; 32]),
            completeness,
            owners: owner_set(Some(reference(4))),
            events: vec![event()],
        }))
    }

    fn materialize(scope: WorkScope) -> UpdateKind {
        UpdateKind::Materialize {
            claim: WorkClaim {
                intent: WorkIntent {
                    workspace: workspace(),
                    epoch: u64::MAX,
                    instance: IndexInstanceId([255; 16]),
                    scope,
                },
                fence: u64::MAX,
                lease_until_ms: u64::MAX,
            },
        }
    }

    pub(crate) fn requests() -> [UpdateRequest; 5] {
        let transition = SourceTransition {
            expected_source: historical_cut(),
            owners: owner_set(Some(reference(4))),
            events: vec![event()],
        };
        [
            UpdateKind::SourceAdvance(transition.clone()),
            UpdateKind::SourceResync {
                prior_epoch: u64::MAX,
                transition,
            },
            projection(ProducerCompleteness::CompleteAffectedDomain {
                certificate: reference(1),
            }),
            UpdateKind::Reconfigure {
                profile: reference(4),
            },
            materialize(WorkScope::ProjectionDomain {
                projection: ProjectionId([255; 16]),
                source: historical_cut(),
                producer_policy: Digest([255; 32]),
                dependency_context: Digest([255; 32]),
            }),
        ]
        .map(request)
    }

    #[test]
    fn request_wires_admit_exact_backing_and_refund_all_variants()
    -> Result<(), Box<dyn std::error::Error>> {
        for request in requests() {
            let old = serde_json::to_vec(&request)?;
            let floor = u64::try_from(old.len())?;
            let (budget, ledger) = scoped_budget(floor)?;
            let payload = encode(&request, floor, &budget)?;
            require(payload.bytes == old && ledger.live() == floor)?;
            drop(payload);
            require(ledger.live() == 0)?;
            let (denied, denied_ledger) =
                scoped_budget(floor.checked_sub(1).ok_or(IndexError::Conflict)?)?;
            require(matches!(
                encode(&request, floor, &denied),
                Err(IndexError::ResourceLimited)
            ))?;
            require(denied_ledger.live() == 0)?;
            // The census refuses overflow before any output reservation.
            require(matches!(
                encode(
                    &request,
                    floor.checked_sub(1).ok_or(IndexError::Conflict)?,
                    &budget
                ),
                Err(IndexError::ResourceLimited)
            ))?;
            require(ledger.live() == 0)?;
        }
        Ok(())
    }

    #[test]
    fn request_edge_gold_preserves_sets_and_excludes_historical_roots() -> Result<(), IndexError> {
        let source = |root| SourceTransition {
            expected_source: historical_cut(),
            owners: owner_set(root),
            events: Vec::new(),
        };
        require(
            children(&request(UpdateKind::SourceAdvance(source(Some(
                reference(4),
            )))))
            .as_slice()
                == [reference(4)],
        )?;
        require(
            children(&request(UpdateKind::SourceResync {
                prior_epoch: 1,
                transition: source(None),
            }))
            .as_slice()
            .is_empty(),
        )?;
        require(
            children(&request(projection(
                ProducerCompleteness::CompleteAffectedDomain {
                    certificate: reference(1),
                },
            )))
            .as_slice()
                == [reference(1), reference(4)],
        )?;
        require(
            children(&request(projection(
                ProducerCompleteness::CompleteAffectedDomain {
                    certificate: reference(4),
                },
            )))
            .as_slice()
                == [reference(4)],
        )?;
        require(
            children(&request(projection(ProducerCompleteness::Partial {
                missing: MapRoot(Some(reference(99))),
            })))
            .as_slice()
                == [reference(4)],
        )?;
        require(
            children(&request(UpdateKind::Reconfigure {
                profile: reference(3),
            }))
            .as_slice()
                == [reference(3)],
        )?;
        for (source, projection, gold) in [
            (
                Some(reference(4)),
                Some(reference(1)),
                vec![reference(1), reference(4)],
            ),
            (Some(reference(4)), Some(reference(4)), vec![reference(4)]),
            (None, None, vec![]),
        ] {
            let kind = materialize(WorkScope::Owner {
                owner: OwnerKey {
                    producer: ProducerId([255; 16]),
                    stable_id: StableOwnerId([255; 16]),
                },
                expected: OwnerInputBinding {
                    source,
                    projection,
                    producer_context: Digest([255; 32]),
                    derivation_policy: Digest([255; 32]),
                },
            });
            require(children(&request(kind)).as_slice() == gold)?;
        }
        Ok(())
    }

    #[test]
    fn publication_copy_reserves_before_birth_and_keeps_charge_through_commit_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = [event(); 65];
        let floor = u64::try_from(std::mem::size_of_val(&input))?;
        let (denied, denied_ledger) =
            scoped_budget(floor.checked_sub(1).ok_or(IndexError::Conflict)?)?;
        require(matches!(
            PreparedEvents::copy(&input, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(denied_ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(floor)?;
        let mut prepared = PreparedEvents::copy(&input, &budget)?;
        require(ledger.live() == floor && prepared.events == input)?;
        let commit_data = prepared.take();
        require(prepared.events.is_empty() && ledger.live() == floor && commit_data == input)?;
        drop(commit_data);
        require(ledger.live() == floor)?;
        drop(prepared);
        require(ledger.live() == 0)?;
        let empty = PreparedEvents::copy(&[], &denied)?;
        require(empty.events.is_empty() && denied_ledger.live() == 0)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            PreparedEvents::copy(&input, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(matches!(
            encode(
                &request(UpdateKind::Reconfigure {
                    profile: reference(4)
                }),
                u64::MAX,
                &budget
            ),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.live() == 0)?;
        Ok(())
    }
}
