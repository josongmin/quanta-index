//! Attempt-local source-frontier preparation and canonical edit emission.
//! Source owns producer epoch admission; this module owns event rules and backing.

use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapRoot, MemoryLease, MemoryLifetime, ObjectRef,
    OperationKey, OrdinalMapEdit, SourceEvent, WorkBudget,
};

use crate::staging_payload::{StagedObjectBatch, encode};

const BATCH: usize = 64;
// SourceEvent consists solely of fixed-width identities and u64 fields.
const MAX_EVENT_BYTES: usize = 16 * 1024;

fn key(event: SourceEvent) -> [u8; 41] {
    let mut key = [0; 41];
    for (target, byte) in key.iter_mut().zip(
        b"frontier\0"
            .iter()
            .chain(&event.producer.0)
            .chain(&event.stream.0),
    ) {
        *target = *byte;
    }
    key
}

struct FrontierRow {
    event: SourceEvent,
    expected: Option<ObjectRef>,
    next: Option<ObjectRef>,
}

/// One admitted array crosses validation, installation and edit-cursor phases.
pub(crate) struct PreparedFrontiers {
    rows: Vec<FrontierRow>,
    memory: Option<MemoryLease>,
}

impl PreparedFrontiers {
    fn new(events: &[SourceEvent], budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = events
            .len()
            .checked_mul(std::mem::size_of::<FrontierRow>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            budget.checkpoint()?;
            None
        } else {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
        };
        let mut rows = Vec::new();
        rows.try_reserve_exact(events.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for event in events {
            budget.checkpoint()?;
            rows.push(FrontierRow {
                event: *event,
                expected: None,
                next: None,
            });
        }
        Ok(Self { rows, memory })
    }

    pub(crate) fn prepare(
        operation: OperationKey,
        root: MapRoot,
        events: &[SourceEvent],
        context: &mut AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        validate_event_shape(operation, events)?;
        let mut prepared = Self::new(events, context.budget)?;
        for row in &mut prepared.rows {
            let event = row.event;
            let expected = context.maps.lookup_object(root, &key(event), context.budget)?;
            match expected {
                Some(reference) => validate_previous(event, read(reference, context)?)?,
                None if event.predecessor != 0 => return Err(IndexError::Conflict),
                None => {}
            }
            row.expected = expected;
        }
        Ok(prepared)
    }

    pub(crate) fn stage(
        mut self,
        context: &mut AttemptContext<'_>,
    ) -> Result<FrontierEdits, IndexError> {
        for rows in self.rows.chunks_mut(BATCH) {
            let mut inputs = StagedObjectBatch::new(rows.len(), context.budget)?;
            let mut predicted = [ObjectRef(Digest([0; 32])); BATCH];
            for (row, prediction) in rows.iter().zip(&mut predicted) {
                let payload = encode(&row.event, MAX_EVENT_BYTES, context.budget)?;
                *prediction = context
                    .objects
                    .identify(&payload.bytes, &[], context.budget)?
                    .reference;
                inputs.push(payload, &[], context.budget)?;
            }
            let installed = context.put_batch(inputs.inputs())?;
            if installed.len() != rows.len() {
                return Err(IndexError::BackendViolation(
                    "source frontier batch lost an ordinal result".into(),
                ));
            }
            if installed
                .iter()
                .zip(&predicted)
                .any(|(actual, expected)| actual != expected)
            {
                return Err(IndexError::BackendViolation(
                    "source frontier batch identity or order mismatch".into(),
                ));
            }
            for (row, reference) in rows.iter_mut().zip(installed) {
                row.next = Some(reference);
            }
        }
        Ok(FrontierEdits {
            rows: self.rows.into_iter(),
            _memory: self.memory,
        })
    }
}

pub(crate) struct FrontierEdits {
    rows: std::vec::IntoIter<FrontierRow>,
    // Iterator backing stays charged even after its last emitted item.
    _memory: Option<MemoryLease>,
}

impl FrontierEdits {
    pub(crate) fn next_edit(
        &mut self,
        budget: &WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        let Some(row) = self.rows.next() else {
            return Ok(None);
        };
        let next = row.next.ok_or_else(|| {
            IndexError::BackendViolation("source frontier was not installed".into())
        })?;
        OrdinalMapEdit::copy_object_key(2, &key(row.event), row.expected, Some(next), budget).map(Some)
    }
}

/// Refuse malformed envelopes before admission can seed producer authority.
pub(crate) fn validate_event_shape(
    operation: OperationKey,
    events: &[SourceEvent],
) -> Result<(), IndexError> {
    if events.is_empty() {
        return Err(IndexError::InvalidInput(
            "source advance has no event".into(),
        ));
    }
    let mut last = None;
    let mut operation_event = false;
    for event in events {
        let pair = (event.producer, event.stream);
        if last.is_some_and(|previous| previous >= pair) {
            return Err(IndexError::InvalidInput(
                "source events must be unique and sorted by producer/stream".into(),
            ));
        }
        last = Some(pair);
        if pair == (operation.producer, operation.stream)
            && event.stream_epoch == operation.stream_epoch
        {
            operation_event = true;
        }
        if event.sequence
            != event
                .predecessor
                .checked_add(1)
                .ok_or_else(|| IndexError::InvalidInput("source sequence overflow".into()))?
        {
            return Err(IndexError::Conflict);
        }
    }
    if !operation_event {
        return Err(IndexError::InvalidInput(
            "operation key is not bound to a source event".into(),
        ));
    }
    Ok(())
}

fn validate_previous(event: SourceEvent, previous: SourceEvent) -> Result<(), IndexError> {
    if previous.producer != event.producer || previous.stream != event.stream {
        return Err(IndexError::CorruptObject(
            "source frontier key mismatch".into(),
        ));
    }
    match previous.stream_epoch.cmp(&event.stream_epoch) {
        std::cmp::Ordering::Equal if event.predecessor != previous.sequence => {
            Err(IndexError::Conflict)
        }
        // Catalog's producer authority decides whether this epoch is committed.
        // A prior stream may join it only from ordinal one.
        std::cmp::Ordering::Less if event.predecessor != 0 => Err(IndexError::Conflict),
        std::cmp::Ordering::Greater => Err(IndexError::Conflict),
        std::cmp::Ordering::Equal | std::cmp::Ordering::Less => Ok(()),
    }
}

fn read(reference: ObjectRef, context: &mut AttemptContext<'_>) -> Result<SourceEvent, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject("invalid frontier marker".into()));
    }
    if stored.payload.len() > MAX_EVENT_BYTES {
        return Err(IndexError::CorruptObject(
            "source frontier exceeds fixed-field encoding bound".into(),
        ));
    }
    let allowance = stored
        .payload
        .len()
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(512))
        .ok_or(IndexError::ResourceLimited)?;
    let _decode_memory = context.budget.reserve_memory(
        u64::try_from(allowance).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("source frontier: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging_payload::tests::{require, scoped_budget};
    use quanta_index_core::index::{ProducerId, StreamId, WorkspaceKey};

    fn event() -> SourceEvent {
        SourceEvent {
            producer: ProducerId([1; 16]),
            stream: StreamId([2; 16]),
            stream_epoch: 4,
            predecessor: 0,
            sequence: 1,
            payload: Digest([3; 32]),
        }
    }

    fn operation() -> OperationKey {
        OperationKey {
            workspace: WorkspaceKey {
                tenant: quanta_index_core::index::TenantId([1; 16]),
                collection: quanta_index_core::index::CollectionId([1; 16]),
                worktree: quanta_index_core::index::WorktreeId([1; 16]),
                slot: quanta_index_core::index::SlotId([1; 16]),
            },
            epoch: 1,
            producer: event().producer,
            stream: event().stream,
            stream_epoch: event().stream_epoch,
            // Journal retry identity is independent from event ordinal one.
            sequence: 99,
        }
    }

    #[test]
    fn frontier_array_denies_before_birth_and_empty_input_uses_no_lease() -> Result<(), IndexError>
    {
        let (budget, ledger) = scoped_budget(0)?;
        require(matches!(
            PreparedFrontiers::new(&[event()], &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == 0)?;
        let empty = PreparedFrontiers::new(&[], &budget)?;
        require(empty.rows.is_empty() && empty.memory.is_none() && ledger.live() == 0)
    }

    #[test]
    fn frozen_events_and_iterator_keep_one_array_charge_through_eof() -> Result<(), IndexError> {
        let bytes = u64::try_from(std::mem::size_of::<FrontierRow>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(bytes)?;
        let mut source = [event()];
        let prepared = PreparedFrontiers::new(&source, &budget)?;
        source.first_mut().ok_or(IndexError::Conflict)?.payload = Digest([9; 32]);
        let frozen = prepared.rows.first().ok_or(IndexError::Conflict)?.event;
        require(frozen == event() && ledger.live() == bytes)?;
        let mut cursor = FrontierEdits {
            rows: prepared.rows.into_iter(),
            _memory: prepared.memory,
        };
        require(cursor.rows.next().is_some() && ledger.live() == bytes)?;
        require(cursor.rows.next().is_none() && ledger.live() == bytes)?;
        drop(cursor);
        require(ledger.live() == 0)
    }

    #[test]
    fn predecessor_epoch_identity_and_journal_independence_keep_existing_contract()
    -> Result<(), IndexError> {
        validate_event_shape(operation(), &[event()])?;
        let mut previous = event();
        previous.sequence = 3;
        let mut next = event();
        next.predecessor = 3;
        next.sequence = 4;
        validate_previous(next, previous)?;
        next.predecessor = 2;
        require(matches!(
            validate_previous(next, previous),
            Err(IndexError::Conflict)
        ))?;
        next = event();
        next.stream_epoch = 5;
        validate_previous(next, previous)?;
        next.predecessor = 1;
        require(matches!(
            validate_previous(next, previous),
            Err(IndexError::Conflict)
        ))?;
        next = event();
        next.stream_epoch = 3;
        require(matches!(
            validate_previous(next, previous),
            Err(IndexError::Conflict)
        ))?;
        next = event();
        next.stream = StreamId([9; 16]);
        require(matches!(
            validate_previous(next, previous),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(matches!(
            validate_event_shape(operation(), &[event(), event()]),
            Err(IndexError::InvalidInput(_))
        ))
    }

    #[test]
    fn fixed_key_and_maximal_event_wire_match_independent_old_gold()
    -> Result<(), Box<dyn std::error::Error>> {
        let gold = b"frontier\0\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x01\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02\x02";
        require(key(event()) == *gold)?;
        let maximal = SourceEvent {
            producer: ProducerId([255; 16]),
            stream: StreamId([255; 16]),
            stream_epoch: u64::MAX,
            predecessor: u64::MAX,
            sequence: u64::MAX,
            payload: Digest([255; 32]),
        };
        let old_wire = serde_json::to_vec(&maximal)?;
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let encoded = encode(&maximal, MAX_EVENT_BYTES, &budget)?;
        require(encoded.bytes == old_wire && old_wire.len() <= MAX_EVENT_BYTES)?;
        drop(encoded);
        require(ledger.live() == 0)?;
        Ok(())
    }
}
