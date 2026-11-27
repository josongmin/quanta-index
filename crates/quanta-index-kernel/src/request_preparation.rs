//! Request serialization, physical edges and publication-copy preparation.

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectChildren, SourceEvent, UpdateRequest, WorkBudget,
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
    ObjectChildren::from_references(
        quanta_index_core::index::native_update_request_direct_children_v1(request),
    )
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
        OwnerKey, OwnerMutationSet, ProducerCompleteness, ProducerId, ProjectionId,
        ProjectionTransition, SchemaId, SlotId, SourceCut, SourceTransition, StableOwnerId,
        StreamId, TenantId, UpdateKind, ViewRef, WorkClaim, WorkIntent, WorkScope, WorkspaceKey,
        WorktreeId,
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

    pub(crate) fn requests() -> [UpdateRequest; 11] {
        let history_cut: quanta_index_core::index::HistorySourceCutV1 =
            serde_json::from_value(serde_json::json!({
                "generation": {"repo_id": "repo-a", "revision_id": "rev-a", "generation": 1},
                "durable_epoch": 2,
                "snapshot_digest": vec![7_u8; 32],
                "replay_start": vec![8_u8; 32]
            }))
            .expect("fixed canonical History metadata fixture");
        let transition = SourceTransition {
            expected_source: historical_cut(),
            owners: owner_set(Some(reference(4))),
            events: vec![event()],
        };
        let combined = quanta_index_core::index::SourceHistoryTransitionV1 {
            source: transition.clone(),
            history: quanta_index_core::index::HistoryGenerationAdvanceV1 {
                prior_instance: IndexInstanceId([254; 16]),
                expected: history_cut.clone(),
                instance: IndexInstanceId([255; 16]),
                input: reference(3),
                profile: reference(5),
            },
        };
        [
            UpdateKind::SourceAdvance(transition.clone()),
            UpdateKind::SourceResync {
                prior_epoch: u64::MAX,
                transition: transition.clone(),
            },
            projection(ProducerCompleteness::CompleteAffectedDomain {
                certificate: reference(1),
            }),
            UpdateKind::Reconfigure {
                profile: reference(4),
            },
            UpdateKind::CompactSourceLocal {
                instance: IndexInstanceId([255; 16]),
                expected_artifact: reference(98),
                owner: OwnerKey {
                    producer: ProducerId([255; 16]),
                    stable_id: StableOwnerId([254; 16]),
                },
            },
            UpdateKind::HistoryAdvance(quanta_index_core::index::HistoryTransition {
                instance: IndexInstanceId([255; 16]),
                expected: Some(history_cut.clone()),
                next: history_cut.clone(),
                input: reference(3),
                profile: reference(4),
            }),
            UpdateKind::HistoryAppend(quanta_index_core::index::HistoryAppendV1 {
                instance: IndexInstanceId([255; 16]),
                expected: history_cut.clone(),
                input: reference(3),
                profile: reference(4),
            }),
            UpdateKind::SourceHistoryAdvance(Box::new(combined.clone())),
            UpdateKind::SourceHistoryResync {
                prior_epoch: u64::MAX,
                transition: Box::new(combined),
            },
            UpdateKind::SourceHistoryBootstrap(Box::new(
                quanta_index_core::index::SourceHistoryBootstrapV1 {
                    source: transition,
                    history: quanta_index_core::index::HistoryTransition {
                        instance: IndexInstanceId([253; 16]),
                        expected: None,
                        next: history_cut,
                        input: reference(3),
                        profile: reference(5),
                    },
                },
            )),
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
        let combined_resync = requests()
            .into_iter()
            .find(|sample| matches!(&sample.kind, UpdateKind::SourceHistoryResync { .. }))
            .ok_or(IndexError::Conflict)?;
        require(
            children(&combined_resync).as_slice()
                == [reference(3), reference(4), reference(5)],
        )?;
        let combined_bootstrap = requests()
            .into_iter()
            .find(|sample| matches!(&sample.kind, UpdateKind::SourceHistoryBootstrap(_)))
            .ok_or(IndexError::Conflict)?;
        require(
            children(&combined_bootstrap).as_slice()
                == [reference(3), reference(4), reference(5)],
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
        require(
            children(&request(UpdateKind::CompactSourceLocal {
                instance: IndexInstanceId([255; 16]),
                expected_artifact: reference(98),
                owner: OwnerKey {
                    producer: ProducerId([255; 16]),
                    stable_id: StableOwnerId([254; 16]),
                },
            }))
            .as_slice()
            .is_empty(),
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
    fn source_request_ref_matches_actual_identification_and_refuses_cancelled_work()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{
            RegisteredObject, native_immutable_object_ref_v1,
            native_update_request_object_ref_with_work_v1,
        };

        let mut samples: Vec<_> = requests().into_iter().filter(|sample| {
            matches!(
                &sample.kind,
                UpdateKind::SourceAdvance(_)
                    | UpdateKind::SourceResync { .. }
                    | UpdateKind::SourceHistoryAdvance(_)
                    | UpdateKind::SourceHistoryBootstrap(_)
                    | UpdateKind::SourceHistoryResync { .. }
            )
        }).collect();
        let mut duplicate_roots = (*samples.iter()
            .find(|sample| matches!(&sample.kind, UpdateKind::SourceHistoryAdvance(_)))
            .ok_or(IndexError::ResourceLimited)?).clone();
        if let UpdateKind::SourceHistoryAdvance(pair) = &mut duplicate_roots.kind {
            pair.history.input = reference(4);
            pair.history.profile = reference(4);
        }
        samples.push(duplicate_roots);
        for sample in samples {
            let (mut budget, ledger) = scoped_budget(1_000_000)?;
            let staged_ref = crate::update::identify_update_request_with(
                &sample,
                &mut budget,
                |payload, children, _budget| {
                    let reference = native_immutable_object_ref_v1(payload, children)
                        .ok_or(IndexError::ResourceLimited)?;
                    let bytes = payload
                        .len()
                        .checked_add(children.len() * 32)
                        .and_then(|value| value.checked_add(16))
                        .and_then(|value| u64::try_from(value).ok())
                        .ok_or(IndexError::ResourceLimited)?;
                    Ok(RegisteredObject::unscoped(
                        reference,
                        bytes,
                        children.to_vec(),
                    ))
                },
            )?;
            let pure_ref = native_update_request_object_ref_with_work_v1(&sample, &mut || {
                budget.checkpoint()
            })
            .map_err(|error| error.into_error(|_| IndexError::ResourceLimited))?
            .ok_or(IndexError::ResourceLimited)?;
            require(pure_ref == staged_ref && ledger.live() == 0)?;
        }

        let sample = requests().into_iter().next().ok_or(IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(0)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            native_update_request_object_ref_with_work_v1(&sample, &mut || budget.checkpoint()),
            Err(quanta_index_core::index::NativeProtectedEncodingErrorV1::Work(
                IndexError::Cancelled
            ))
        ))?;
        require(ledger.live() == 0)
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
