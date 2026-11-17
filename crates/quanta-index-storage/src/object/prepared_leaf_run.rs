//! Effect-free optional admission; original buffers survive through installation.

use super::*;
use quanta_index_core::index::object_install::PreparedObjectInstallRun;
use quanta_index_core::{
    RequestBudgetV1,
    index::{MemoryLease, MemoryLifetime, StagedObjectReferences},
};

const OBJECTS: usize = MAX_BATCH_INPUTS;
const PACKETS: usize = 2;

struct LeafRun<'a> {
    store: &'a FileObjectStore,
    attempt: AttemptRef,
    request: RequestBudgetV1,
    maximum_payload: usize,
    packet_bytes: usize,
    bytes: [usize; PACKETS],
    split: Option<usize>,
    poisoned: bool,
    objects: Vec<PreparedObject>,
    // Each unused buffer owns its original grant, not a headroom promise.
    vacant: Vec<Option<AdmittedWire>>,
    registrations: Vec<Option<MemoryLease>>,
    groups: [Option<PreparedInstallGroup>; PACKETS],
    references: Vec<ObjectRef>,
    output_memory: Option<MemoryLease>,
    // Fixed handle, vector backing and install-time borrowed slots die first.
    _metadata_memory: MemoryLease,
}

fn try_vec<T>(capacity: usize) -> Option<Vec<T>> {
    let mut result = Vec::new();
    if result.try_reserve_exact(capacity).is_err() || result.capacity() > capacity {
        return None;
    }
    Some(result)
}

pub(super) fn try_prepare<'a>(
    store: &'a FileObjectStore,
    attempt: AttemptRef,
    maximum_payload: usize,
    packet_bytes: usize,
    budget: &mut WorkBudget,
) -> Result<Option<Box<dyn PreparedObjectInstallRun + 'a>>, IndexError> {
    budget.checkpoint()?;
    if !store
        .authority
        .as_ref()
        .is_some_and(|owner| owner.supports_atomic_attempt_advance())
    {
        return Ok(None);
    }
    let wire_capacity = maximum_payload
        .checked_add(HEADER_LEN)
        .ok_or(IndexError::ResourceLimited)?;
    let allocation = wire_capacity
        .checked_mul(OBJECTS)
        .ok_or(IndexError::ResourceLimited)?;
    let envelope = packet_bytes
        .checked_mul(PACKETS)
        .and_then(|bytes| {
            HEADER_LEN
                .checked_mul(OBJECTS)
                .and_then(|header| bytes.checked_add(header))
        })
        .ok_or(IndexError::ResourceLimited)?;
    if maximum_payload == 0
        || maximum_payload > MAX_PAYLOAD
        || packet_bytes == 0
        || maximum_payload > packet_bytes
        || u64::try_from(allocation).map_err(|_error| IndexError::ResourceLimited)?
            > MAX_BATCH_BYTES
        || u64::try_from(envelope).map_err(|_error| IndexError::ResourceLimited)? > MAX_BATCH_BYTES
    {
        return Ok(None);
    }
    let per_object = std::mem::size_of::<PreparedObject>()
        .checked_add(std::mem::size_of::<Option<AdmittedWire>>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<MemoryLease>>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<&PreparedObject>()))
        .ok_or(IndexError::ResourceLimited)?;
    let metadata_bytes = per_object
        .checked_mul(OBJECTS)
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<LeafRun<'_>>()))
        .ok_or(IndexError::ResourceLimited)?;
    let Some(metadata_memory) = budget.try_reserve_memory_for_optimization(
        u64::try_from(metadata_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?
    else {
        return Ok(None);
    };
    let Some(output_memory) = budget.try_reserve_memory_for_optimization(
        u64::try_from(
            OBJECTS
                .checked_mul(std::mem::size_of::<ObjectRef>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?
    else {
        return Ok(None);
    };
    let (Some(objects), Some(mut vacant), Some(mut registrations), Some(references)) = (
        try_vec(OBJECTS),
        try_vec(OBJECTS),
        try_vec(OBJECTS),
        try_vec(OBJECTS),
    ) else {
        return Ok(None);
    };
    let mut groups = std::array::from_fn(|_| None);
    for group in &mut groups {
        let Some(metadata) = RegistrationBatch::try_capacity(OBJECTS, budget)? else {
            return Ok(None);
        };
        let Some(shards) = BatchShards::try_all(&store.objects, budget)? else {
            return Ok(None);
        };
        *group = Some(PreparedInstallGroup {
            references: BatchReferences::new(&[], budget)?,
            metadata,
            shards,
        });
    }
    for _slot in 0..OBJECTS {
        let Some(wire) = AdmittedWire::try_capacity(wire_capacity, budget)? else {
            return Ok(None);
        };
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            RegisteredObject::allocation_bytes(0)?,
            MemoryLifetime::Retained,
        )?
        else {
            return Ok(None);
        };
        vacant.push(Some(wire));
        registrations.push(Some(memory));
    }
    budget.checkpoint()?;
    Ok(Some(Box::new(LeafRun {
        store,
        attempt,
        request: budget.request().clone(),
        maximum_payload,
        packet_bytes,
        bytes: [0; PACKETS],
        split: None,
        poisoned: false,
        objects,
        vacant,
        registrations,
        groups,
        references,
        output_memory: Some(output_memory),
        _metadata_memory: metadata_memory,
    })))
}

impl LeafRun<'_> {
    fn check_budget(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.request.is_cancelled() {
            return Err(IndexError::Cancelled);
        }
        if std::time::Instant::now() >= self.request.deadline() {
            return Err(IndexError::ResourceLimited);
        }
        if !self.request.same_request_as(budget.request())
            || self.request.deadline() != budget.request().deadline()
        {
            return Err(IndexError::BackendViolation(
                "prepared install request scope differs".into(),
            ));
        }
        Ok(())
    }
    fn push_admitted(
        &mut self,
        payload: &[u8],
        expected: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_budget(budget)?;
        if !self.accepts(payload.len()) {
            return Err(IndexError::ResourceLimited);
        }
        if self.references.contains(&expected) {
            return Err(IndexError::BackendViolation(
                "prepared leaf run repeats an identity".into(),
            ));
        }
        let slot = self.objects.len();
        let mut wire = self
            .vacant
            .get_mut(slot)
            .and_then(Option::take)
            .ok_or_else(|| IndexError::BackendViolation("prepared leaf wire absent".into()))?;
        let length = HEADER_LEN
            .checked_add(payload.len())
            .ok_or(IndexError::ResourceLimited)?;
        // Preserve install_batch's original aggregate and prepare fees exactly
        // once, only for an input actually adopted by the prepared DATA handle.
        let bytes = u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: bytes
                .checked_mul(2)
                .and_then(|n| n.checked_add(256))
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        wire.bytes.extend_from_slice(MAGIC);
        wire.bytes.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        wire.bytes.extend_from_slice(&0u32.to_be_bytes());
        wire.bytes.extend_from_slice(payload);
        if wire.bytes.len() != length || digest_wire(&wire.bytes) != expected {
            return Err(IndexError::BackendViolation(
                "prepared leaf identity differs".into(),
            ));
        }
        let memory = self
            .registrations
            .get_mut(slot)
            .and_then(Option::take)
            .ok_or_else(|| {
                IndexError::BackendViolation("prepared registration grant absent".into())
            })?;
        let registration = RegisteredObject::from_parts(expected, bytes, Vec::new(), Some(memory))?;
        let mut packet = usize::from(self.split.is_some());
        if self
            .bytes
            .get(packet)
            .ok_or(IndexError::ResourceLimited)?
            .checked_add(payload.len())
            .ok_or(IndexError::ResourceLimited)?
            > self.packet_bytes
        {
            self.split = Some(slot);
            packet = 1;
        }
        self.groups
            .get_mut(packet)
            .ok_or(IndexError::ResourceLimited)?
            .as_mut()
            .ok_or(IndexError::ResourceLimited)?
            .metadata
            .push_prepared(registration.clone())?;
        self.objects
            .push(PreparedObject::from_wire(registration, wire));
        self.references.push(expected);
        let total = self
            .bytes
            .get_mut(packet)
            .ok_or(IndexError::ResourceLimited)?;
        *total = total
            .checked_add(payload.len())
            .ok_or(IndexError::ResourceLimited)?;
        budget.checkpoint()
    }
}

impl PreparedObjectInstallRun for LeafRun<'_> {
    fn accepts(&self, payload_bytes: usize) -> bool {
        if self.poisoned || self.objects.len() == OBJECTS || payload_bytes > self.maximum_payload {
            return false;
        }
        let packet = usize::from(self.split.is_some());
        self.bytes
            .get(packet)
            .and_then(|bytes| bytes.checked_add(payload_bytes))
            .is_some_and(|bytes| {
                bytes <= self.packet_bytes || (packet == 0 && payload_bytes <= self.packet_bytes)
            })
    }

    fn push(
        &mut self,
        payload: &[u8],
        expected: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let result = self.push_admitted(payload, expected, budget);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn check_payload(&self, slot: usize, payload: &[u8]) -> Result<(), IndexError> {
        if self
            .objects
            .get(slot)
            .and_then(|object| object.wire.get(HEADER_LEN..))
            != Some(payload)
        {
            return Err(IndexError::BackendViolation(
                "prepared leaf alias input differs".into(),
            ));
        }
        Ok(())
    }

    fn references(&self) -> &[ObjectRef] {
        &self.references
    }

    fn install(
        mut self: Box<Self>,
        attempt: AttemptRef,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedObjectReferences, IndexError> {
        self.check_budget(budget)?;
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "prepared leaf run adoption failed".into(),
            ));
        }
        if attempt != self.attempt
            || sink.managed_scope() != Some(ObjectInstallScope::Attempt(attempt))
        {
            return Err(IndexError::FenceLost);
        }
        let first = self.objects.first().ok_or(IndexError::ResourceLimited)?;
        let mut borrowed = [first; OBJECTS];
        for (slot, object) in borrowed.iter_mut().zip(&self.objects) {
            *slot = object;
        }
        let all = borrowed
            .get(..self.objects.len())
            .ok_or(IndexError::ResourceLimited)?;
        let split = self.split.unwrap_or(all.len());
        let groups: [&[&PreparedObject]; PACKETS] = [
            all.get(..split).ok_or(IndexError::ResourceLimited)?,
            all.get(split..).ok_or(IndexError::ResourceLimited)?,
        ];
        let count = if self.split.is_some() { 2 } else { 1 };
        for (group, objects) in self.groups.iter_mut().zip(groups).take(count) {
            let group = group.as_mut().ok_or(IndexError::ResourceLimited)?;
            group.references = BatchReferences::new(objects, budget)?;
            group.shards.restrict_to(group.metadata.as_slice())?;
        }
        // Free unused allocations, never release and reacquire adopted grants.
        self.vacant.clear();
        self.registrations.clear();
        self.store.install_admitted_run(
            groups.get(..count).ok_or(IndexError::ResourceLimited)?,
            sink,
            budget,
            self.groups
                .get(..count)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let LeafRun {
            references,
            output_memory,
            ..
        } = *self;
        StagedObjectReferences::from_parts(references, output_memory)
    }
}
