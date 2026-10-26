//! Original-funded decoding of durable requests and their escaping backing.

use std::{fmt, ops::Deref};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, OperationKey, OwnerMutationSet, ProducerCompleteness,
    ProducerId, ProjectionBinding, ProjectionId, ProjectionTransition, SchemaId, SourceCut,
    SourceEvent, SourceTransition, UpdateKind, UpdateRequest, ViewRef, WorkBudget, WorkClaim,
};
use serde::{
    Deserialize,
    de::{DeserializeSeed, EnumAccess, IgnoredAny, SeqAccess, VariantAccess, Visitor},
};
use serde_json::value::RawValue;

pub(crate) struct LoadedRequest {
    request: UpdateRequest,
    // The request's Vec/Box backing dies before its retained allocation grant.
    _memory: Option<MemoryLease>,
}

impl Deref for LoadedRequest {
    type Target = UpdateRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header<'a> {
    operation: OperationKey,
    expected_view: ViewRef,
    #[serde(borrow)]
    kind: RawKind<'a>,
}

#[derive(Deserialize)]
enum KindTag {
    SourceAdvance,
    SourceResync,
    ProjectionAdvance,
    Reconfigure,
    Materialize,
}

struct RawKind<'a> {
    tag: KindTag,
    payload: &'a RawValue,
}

impl<'de: 'a, 'a> Deserialize<'de> for RawKind<'a> {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct KindVisitor;

        impl<'de> Visitor<'de> for KindVisitor {
            type Value = RawKind<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("one closed UpdateKind variant")
            }

            fn visit_enum<A: EnumAccess<'de>>(self, variants: A) -> Result<Self::Value, A::Error> {
                let (tag, payload) = variants.variant::<KindTag>()?;
                Ok(RawKind {
                    tag,
                    payload: payload.newtype_variant()?,
                })
            }
        }

        decoder.deserialize_enum(
            "UpdateKind",
            &[
                "SourceAdvance",
                "SourceResync",
                "ProjectionAdvance",
                "Reconfigure",
                "Materialize",
            ],
            KindVisitor,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceParts<'a> {
    expected_source: SourceCut,
    owners: OwnerMutationSet,
    #[serde(borrow)]
    events: &'a RawValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResyncParts<'a> {
    prior_epoch: u64,
    #[serde(borrow)]
    transition: SourceParts<'a>,
}

// Copy is a compile-time constraint on these heap-free variant payloads.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconfigureParts {
    profile: quanta_index_core::index::ObjectRef,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterializeParts {
    claim: WorkClaim,
}

fn required_optional<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(decoder)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionParts<'a> {
    projection: ProjectionId,
    producer: ProducerId,
    #[serde(deserialize_with = "required_optional")]
    expected: Option<ProjectionBinding>,
    target_source: SourceCut,
    schema: SchemaId,
    producer_policy: quanta_index_core::index::Digest,
    dependency_context: quanta_index_core::index::Digest,
    completeness: ProducerCompleteness,
    owners: OwnerMutationSet,
    #[serde(borrow)]
    events: &'a RawValue,
}

struct CountEvents<'a>(&'a WorkBudget);

impl<'de> DeserializeSeed<'de> for CountEvents<'_> {
    type Value = usize;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<usize, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for CountEvents<'_> {
    type Value = usize;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a durable source event array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<usize, A::Error> {
        let mut count = 0usize;
        loop {
            self.0.checkpoint().map_err(serde::de::Error::custom)?;
            if sequence.next_element::<IgnoredAny>()?.is_none() {
                return Ok(count);
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| serde::de::Error::custom("request event count overflow"))?;
        }
    }
}

struct EventsSeed<'a> {
    events: Vec<SourceEvent>,
    count: usize,
    budget: &'a WorkBudget,
}

impl<'de> DeserializeSeed<'de> for EventsSeed<'_> {
    type Value = Vec<SourceEvent>;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for EventsSeed<'_> {
    type Value = Vec<SourceEvent>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the admitted durable event array")
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut sequence: A) -> Result<Self::Value, A::Error> {
        loop {
            self.budget.checkpoint().map_err(serde::de::Error::custom)?;
            // Check the bound before decoding another record or growing output.
            if self.events.len() == self.count {
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom("request event census changed"));
                }
                return Ok(self.events);
            }
            let event = sequence
                .next_element::<SourceEvent>()?
                .ok_or_else(|| serde::de::Error::custom("request event census changed"))?;
            self.events.push(event);
        }
    }
}

fn read<'de, T: Deserialize<'de>>(bytes: &'de [u8], budget: &WorkBudget) -> Result<T, IndexError> {
    budget.checkpoint()?;
    let result = serde_json::from_slice(bytes)
        .map_err(|error| IndexError::CorruptObject(format!("durable request: {error}")));
    budget.checkpoint()?;
    result
}

fn events(
    raw: &RawValue,
    boxed_bytes: usize,
    budget: &WorkBudget,
) -> Result<(Vec<SourceEvent>, Option<MemoryLease>), IndexError> {
    let mut decoder = serde_json::Deserializer::from_str(raw.get());
    let count = CountEvents(budget)
        .deserialize(&mut decoder)
        .map_err(|error| IndexError::CorruptObject(format!("request event census: {error}")));
    budget.checkpoint()?;
    let count = count?;
    decoder.end().map_err(|error| {
        IndexError::CorruptObject(format!("request event census trailing input: {error}"))
    })?;
    let backing_bytes = count
        .checked_mul(std::mem::size_of::<SourceEvent>())
        .and_then(|bytes| bytes.checked_add(boxed_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if backing_bytes == 0 {
        None
    } else {
        budget.reserve_memory(
            u64::try_from(backing_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?
    };
    let mut events = Vec::new();
    events
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut decoder = serde_json::Deserializer::from_str(raw.get());
    let result = EventsSeed {
        events,
        count,
        budget,
    }
    .deserialize(&mut decoder)
    .map_err(|error| IndexError::CorruptObject(format!("request events: {error}")));
    budget.checkpoint()?;
    let event_rows = result?;
    decoder.end().map_err(|error| {
        IndexError::CorruptObject(format!("request events trailing input: {error}"))
    })?;
    Ok((event_rows, memory))
}

pub(crate) fn decode(bytes: &[u8], budget: &WorkBudget) -> Result<LoadedRequest, IndexError> {
    budget.checkpoint()?;
    // This parser allowance funds copied/escaped serde keys and scratch. It
    // does not stand in for the separately retained event/Box backing below.
    let allowance = bytes
        .len()
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let _parser = budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let header: Header<'_> = read(bytes, budget)?;
    let raw = header.kind.payload.get().as_bytes();
    let (kind, memory) = match header.kind.tag {
        KindTag::SourceAdvance => {
            let parts: SourceParts<'_> = read(raw, budget)?;
            let (events, memory) = events(parts.events, 0, budget)?;
            (
                UpdateKind::SourceAdvance(SourceTransition {
                    expected_source: parts.expected_source,
                    owners: parts.owners,
                    events,
                }),
                memory,
            )
        }
        KindTag::SourceResync => {
            let parts: ResyncParts<'_> = read(raw, budget)?;
            let (events, memory) = events(parts.transition.events, 0, budget)?;
            (
                UpdateKind::SourceResync {
                    prior_epoch: parts.prior_epoch,
                    transition: SourceTransition {
                        expected_source: parts.transition.expected_source,
                        owners: parts.transition.owners,
                        events,
                    },
                },
                memory,
            )
        }
        KindTag::ProjectionAdvance => {
            let parts: ProjectionParts<'_> = read(raw, budget)?;
            let (events, memory) = events(
                parts.events,
                std::mem::size_of::<ProjectionTransition>(),
                budget,
            )?;
            (
                UpdateKind::ProjectionAdvance(Box::new(ProjectionTransition {
                    projection: parts.projection,
                    producer: parts.producer,
                    expected: parts.expected,
                    target_source: parts.target_source,
                    schema: parts.schema,
                    producer_policy: parts.producer_policy,
                    dependency_context: parts.dependency_context,
                    completeness: parts.completeness,
                    owners: parts.owners,
                    events,
                })),
                memory,
            )
        }
        KindTag::Reconfigure => {
            let parts: ReconfigureParts = read(raw, budget)?;
            (
                UpdateKind::Reconfigure {
                    profile: parts.profile,
                },
                None,
            )
        }
        KindTag::Materialize => {
            let parts: MaterializeParts = read(raw, budget)?;
            (UpdateKind::Materialize { claim: parts.claim }, None)
        }
    };
    let loaded = LoadedRequest {
        request: UpdateRequest {
            operation: header.operation,
            expected_view: header.expected_view,
            kind,
        },
        _memory: memory,
    };
    budget.checkpoint()?;
    Ok(loaded)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_core::{
        CancelHandleV1,
        index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits},
    };

    use super::*;
    use crate::{request_preparation::tests::requests, staging_payload::tests::require};

    pub(crate) struct Ledger {
        cap: u64,
        live: AtomicU64,
        retained_attempts: AtomicU64,
        cancel_on_retained: Option<CancelHandleV1>,
    }

    impl Ledger {
        pub(crate) fn live(&self) -> u64 {
            self.live.load(Ordering::SeqCst)
        }
    }

    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    struct Scope(Arc<Ledger>);

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime == MemoryLifetime::Retained {
                let _prior = self.0.retained_attempts.fetch_add(1, Ordering::SeqCst);
            }
            let _previous_live = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes).filter(|next| *next <= self.0.cap)
                })
                .map_err(|_prior| IndexError::ResourceLimited)?;
            if lifetime == MemoryLifetime::Retained
                && let Some(cancel) = &self.0.cancel_on_retained
            {
                cancel.cancel();
            }
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    pub(crate) fn scoped(
        cap: u64,
        late_cancel: bool,
    ) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let ledger = Arc::new(Ledger {
            cap,
            live: AtomicU64::new(0),
            retained_attempts: AtomicU64::new(0),
            cancel_on_retained: late_cancel.then(|| budget.request().cancel_handle()),
        });
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        Ok((budget, ledger))
    }

    fn retained_gold(request: &UpdateRequest) -> Result<u64, Box<dyn std::error::Error>> {
        let bytes = match &request.kind {
            UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. } => {
                std::mem::size_of_val(transition.events.as_slice())
            }
            UpdateKind::ProjectionAdvance(transition) => {
                std::mem::size_of_val(transition.events.as_slice())
                    .checked_add(std::mem::size_of_val(transition.as_ref()))
                    .ok_or(IndexError::ResourceLimited)?
            }
            UpdateKind::Reconfigure { .. } | UpdateKind::Materialize { .. } => 0,
        };
        Ok(u64::try_from(bytes)?)
    }

    #[test]
    fn all_request_kinds_match_old_codec_and_keep_only_actual_retained_backing()
    -> Result<(), Box<dyn std::error::Error>> {
        for request in requests() {
            let bytes = serde_json::to_vec(&request)?;
            let old: UpdateRequest = serde_json::from_slice(&bytes)?;
            let (budget, ledger) = scoped(u64::MAX, false)?;
            let loaded = decode(&bytes, &budget)?;
            require(*loaded == old && old == request && ledger.live() == retained_gold(&request)?)?;
            drop(budget);
            require(*loaded == request && ledger.live() == retained_gold(&request)?)?;
            drop(loaded);
            require(ledger.live() == 0)?;
        }
        Ok(())
    }

    #[test]
    fn sixty_five_events_refuse_retained_floor_before_vector_and_refund_parser()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut request = requests()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::SourceAdvance(_)))
            .ok_or(IndexError::Conflict)?;
        let UpdateKind::SourceAdvance(transition) = &mut request.kind else {
            return Err(IndexError::Conflict.into());
        };
        let event = *transition.events.first().ok_or(IndexError::Conflict)?;
        transition.events = vec![event; 65];
        let bytes = serde_json::to_vec(&request)?;
        // The parser is admitted; no byte remains for the independent event Vec.
        let parser_floor = u64::try_from(bytes.len())?
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or(IndexError::ResourceLimited)?;
        let (denied, ledger) = scoped(parser_floor, false)?;
        require(matches!(
            decode(&bytes, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == 0 && ledger.retained_attempts.load(Ordering::SeqCst) == 1)?;
        let floor = parser_floor
            .checked_add(retained_gold(&request)?)
            .ok_or(IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped(floor, false)?;
        let loaded = decode(&bytes, &budget)?;
        require(*loaded == request && ledger.live() == retained_gold(&request)?)?;
        drop(loaded);
        require(ledger.live() == 0)?;
        Ok(())
    }

    #[test]
    fn closed_record_corruption_matches_old_decoder_and_releases_all_custody()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = requests()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::SourceAdvance(_)))
            .ok_or(IndexError::Conflict)?;
        let source_json = serde_json::to_string(&source)?;
        let projection = requests()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::ProjectionAdvance(_)))
            .ok_or(IndexError::Conflict)?;
        let projection_json = serde_json::to_string(&projection)?;
        let duplicate = source_json.replacen("\"events\":[", "\"events\":[],\"events\":[", 1);
        let unknown = source_json.replacen("\"events\":[", "\"unexpected\":0,\"events\":[", 1);
        let missing_nullable = projection_json.replacen("\"expected\":null,", "", 1);
        let invalid_event = source_json.replacen("\"events\":[{", "\"events\":[null,{", 1);
        for input in [
            duplicate,
            unknown,
            missing_nullable,
            invalid_event,
            format!("{source_json} null"),
            "{".into(),
        ] {
            require(serde_json::from_str::<UpdateRequest>(&input).is_err())?;
            let (budget, ledger) = scoped(u64::MAX, false)?;
            require(matches!(
                decode(input.as_bytes(), &budget),
                Err(IndexError::CorruptObject(_))
            ))?;
            require(ledger.live() == 0)?;
        }
        Ok(())
    }

    #[test]
    fn initial_and_post_reservation_original_cancel_never_become_corruption()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = requests()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::ProjectionAdvance(_)))
            .ok_or(IndexError::Conflict)?;
        let bytes = serde_json::to_vec(&request)?;
        let (budget, ledger) = scoped(0, false)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            decode(&bytes, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.live() == 0 && ledger.retained_attempts.load(Ordering::SeqCst) == 0)?;
        let (budget, ledger) = scoped(u64::MAX, true)?;
        require(matches!(
            decode(&bytes, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.live() == 0 && ledger.retained_attempts.load(Ordering::SeqCst) == 1)?;
        Ok(())
    }
}
