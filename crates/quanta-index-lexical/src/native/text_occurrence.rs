//! Exact position-vector admission for the canonical immutable occurrence.
//! Query heads and callbacks use this only within their execution scope.

pub(super) mod block;

use std::{fmt, ops::Deref};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, StoredObject, WorkBudget,
    ports::ImmutableObjectPort,
};
use serde::{
    Deserialize,
    de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use super::text::{MAX_POSITIONS_PER_TERM, Occurrence, Positions};
use super::text_memory::PreparedRows;

/// Execution-local bounded block DATA window.
///
/// This grants no pin/custody authority. Ref-only scalar memoization is valid only for a leaf, since a
/// block reference has a distinct count for every canonical term selector.
pub(super) struct OccurrenceReader {
    blocks: PreparedRows<block::CheckedBlock>,
    capacity: usize,
    next: usize,
}

pub(super) struct OccurrenceCount {
    pub(super) value: u64,
    pub(super) leaf: bool,
}

impl OccurrenceReader {
    pub(super) fn new(capacity: usize) -> Result<Self, IndexError> {
        if !(1..=64).contains(&capacity) {
            return Err(IndexError::InvalidInput(
                "invalid occurrence read window".into(),
            ));
        }
        Ok(Self {
            blocks: PreparedRows::new(capacity)?,
            capacity,
            next: 0,
        })
    }

    /// Retain only the current call's parsed DATA. The next call must use the
    /// original object port, including its authentication and logical charge.
    pub(super) fn uncached() -> Result<Self, IndexError> {
        let mut reader = Self::new(1)?;
        reader.capacity = 0;
        Ok(reader)
    }

    pub(super) fn contains(&self, reference: ObjectRef) -> bool {
        if self.capacity == 0 {
            return false;
        }
        self.blocks
            .as_slice()
            .iter()
            .any(|block| block.object().registration.reference == reference)
    }

    /// Derived counts from a completely validated block are immutable DATA,
    /// like a leaf's validated scalar. Only the prepare attempt's confirmed
    /// memo uses this; query/prior reads still cross `read_retained`.
    pub(super) fn validated_count(
        &self,
        reference: ObjectRef,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<Option<OccurrenceCount>, IndexError> {
        budget.checkpoint()?;
        if self.capacity == 0 {
            return Ok(None);
        }
        let Some(block) = self
            .blocks
            .as_slice()
            .iter()
            .find(|block| block.object().registration.reference == reference)
        else {
            return Ok(None);
        };
        let range = block.selection(term, budget)?;
        Ok(Some(OccurrenceCount {
            value: u64::try_from(
                range
                    .len()
                    .checked_div(4)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            leaf: false,
        }))
    }

    fn block(
        &mut self,
        reference: ObjectRef,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<Result<&block::CheckedBlock, StoredObject>, IndexError> {
        budget.checkpoint()?;
        if self.capacity == 0 {
            self.blocks.clear();
        }
        if let Some(slot) = self
            .blocks
            .as_slice()
            .iter()
            .position(|block| block.object().registration.reference == reference)
        {
            let checked = self
                .blocks
                .as_slice()
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?;
            // Retained DATA still crosses the adapter's charge/cancellation
            // boundary. A fallback adapter may perform its original read.
            let retained = objects.read_retained(checked.object(), budget)?;
            budget.checkpoint()?;
            if retained != *checked.object() {
                return Err(IndexError::CorruptObject(
                    "retained text block differs from authenticated DATA".into(),
                ));
            }
            return Ok(Ok(checked));
        }
        let stored = checked_object(reference, objects, budget)?;
        if !stored.payload.starts_with(b"QITXOC01") {
            return Ok(Err(stored));
        }
        let checked = block::CheckedBlock::parse(stored, budget)?;
        // A full ring replaces one already-admitted slot. No cache-wide
        // growth or eviction of a live returned position vector is possible.
        let slot = if self.blocks.len() < self.capacity.max(1) {
            let slot = self.blocks.len();
            self.blocks.push(checked, budget)?;
            slot
        } else {
            let slot = self.next;
            let entry = self
                .blocks
                .as_mut_slice()
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?;
            *entry = checked;
            self.next = slot
                .checked_add(1)
                .and_then(|next| next.checked_rem(self.blocks.len()))
                .ok_or(IndexError::ResourceLimited)?;
            slot
        };
        Ok(Ok(self
            .blocks
            .as_slice()
            .get(slot)
            .ok_or(IndexError::ResourceLimited)?))
    }

    pub(super) fn count(
        &mut self,
        reference: ObjectRef,
        term: &[u8],
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<OccurrenceCount, IndexError> {
        super::text_spool::check_term_key(term)?;
        match self.block(reference, objects, budget)? {
            Ok(block) => {
                let range = block.selection(term, budget)?;
                Ok(OccurrenceCount {
                    value: u64::try_from(
                        range
                            .len()
                            .checked_div(4)
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_error| IndexError::ResourceLimited)?,
                    leaf: false,
                })
            }
            Err(stored) => {
                let (layout, _parse_memory) = preflight(&stored.payload, budget)?;
                Ok(OccurrenceCount {
                    value: u64::try_from(layout.count)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    leaf: true,
                })
            }
        }
    }

    pub(super) fn load(
        &mut self,
        reference: ObjectRef,
        term: &[u8],
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<ScopedOccurrence, IndexError> {
        super::text_spool::check_term_key(term)?;
        match self.block(reference, objects, budget)? {
            Ok(block) => {
                let range = block.selection(term, budget)?;
                let bytes = block
                    .object()
                    .payload
                    .get(range)
                    .ok_or(IndexError::ResourceLimited)?;
                let memory = budget.reserve_memory(
                    u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?;
                let mut positions = Vec::new();
                positions
                    .try_reserve_exact(
                        bytes
                            .len()
                            .checked_div(4)
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_error| IndexError::ResourceLimited)?;
                for raw in bytes.chunks_exact(4) {
                    budget.checkpoint()?;
                    let position = raw.try_into().map_err(|_width| {
                        IndexError::BackendViolation(
                            "checked text block position width changed".into(),
                        )
                    })?;
                    positions.push(u32::from_be_bytes(position));
                }
                budget.checkpoint()?;
                Ok(ScopedOccurrence {
                    value: Occurrence {
                        positions: Positions(positions),
                    },
                    _memory: memory,
                })
            }
            Err(stored) => decode_payload(&stored.payload, budget),
        }
    }
}

/// The canonical value moves with its charge; no copied position allocation or
/// execution guard is hidden in a returned hit. This wrapper is not wire data.
#[derive(Debug)]
pub(super) struct ScopedOccurrence {
    value: Occurrence,
    // Drop the position vector before releasing its live charge.
    _memory: Option<MemoryLease>,
}

impl Deref for ScopedOccurrence {
    type Target = Occurrence;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[cfg(test)]
fn load(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<ScopedOccurrence, IndexError> {
    let stored = checked_object(reference, objects, budget)?;
    decode_payload(&stored.payload, budget)
}

/// Validate the complete immutable leaf when only its position count is used.
/// Query heads still use `load` and retain their admitted position vector.
#[cfg(test)]
fn load_count(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<u64, IndexError> {
    let stored = checked_object(reference, objects, budget)?;
    let (layout, _parse_memory) = preflight(&stored.payload, budget)?;
    u64::try_from(layout.count).map_err(|_error| IndexError::ResourceLimited)
}

fn checked_object(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    budget.checkpoint()?;
    let stored = objects.get(reference, budget)?;
    budget.checkpoint()?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject(
            "invalid text occurrence object or edges".into(),
        ));
    }
    Ok(stored)
}

struct PositionLayout {
    count: usize,
    positions: Option<Vec<u32>>,
}

struct PositionsSeed<'a> {
    budget: &'a WorkBudget,
    positions: Option<Vec<u32>>,
    expected: Option<usize>,
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for PositionsSeed<'_> {
    type Value = PositionLayout;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for PositionsSeed<'_> {
    type Value = PositionLayout;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("1..=8192 increasing u32 text positions")
    }
    fn visit_seq<A: SeqAccess<'de>>(mut self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut count = 0_usize;
        let mut prior = None;
        loop {
            self.budget.checkpoint().map_err(|_error| {
                serde::de::Error::custom("occurrence request stopped during decode")
            })?;
            let Some(position) = sequence.next_element::<u32>()? else {
                break;
            };
            if count == self.limit || prior.is_some_and(|prior| prior >= position) {
                return Err(serde::de::Error::custom(
                    "text positions exceed their sorted leaf envelope",
                ));
            }
            if let Some(positions) = &mut self.positions {
                if self.expected.is_none_or(|expected| count == expected)
                    || positions.len() == positions.capacity()
                {
                    return Err(serde::de::Error::custom(
                        "occurrence differs from its preflight",
                    ));
                }
                positions.push(position);
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| serde::de::Error::custom("text position count overflows"))?;
            prior = Some(position);
        }
        if count == 0 || self.expected.is_some_and(|expected| expected != count) {
            return Err(serde::de::Error::custom(
                "text positions are empty or differ from preflight",
            ));
        }
        Ok(PositionLayout {
            count,
            positions: self.positions,
        })
    }
}

enum Field {
    Positions,
}
impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct FieldVisitor;
        impl Visitor<'_> for FieldVisitor {
            type Value = Field;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("the positions field")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Field, E> {
                if value == "positions" {
                    Ok(Field::Positions)
                } else {
                    Err(E::unknown_field(value, &["positions"]))
                }
            }
        }
        decoder.deserialize_identifier(FieldVisitor)
    }
}

struct OccurrenceSeed<'a>(PositionsSeed<'a>);
impl<'de> DeserializeSeed<'de> for OccurrenceSeed<'_> {
    type Value = PositionLayout;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct("Occurrence", &["positions"], self)
    }
}
impl<'de> Visitor<'de> for OccurrenceSeed<'_> {
    type Value = PositionLayout;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a complete Occurrence record")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut seed = Some(self.0);
        let mut layout = None;
        while matches!(map.next_key::<Field>()?, Some(Field::Positions)) {
            let selected = seed
                .take()
                .ok_or_else(|| serde::de::Error::duplicate_field("positions"))?;
            layout = Some(map.next_value_seed(selected)?);
        }
        layout.ok_or_else(|| serde::de::Error::missing_field("positions"))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let layout = sequence
            .next_element_seed(self.0)?
            .ok_or_else(|| serde::de::Error::missing_field("positions"))?;
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom(
                "occurrence record has extra fields",
            ));
        }
        Ok(layout)
    }
}

fn parse(bytes: &[u8], seed: PositionsSeed<'_>) -> Result<PositionLayout, IndexError> {
    let budget = seed.budget;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = OccurrenceSeed(seed).deserialize(&mut decoder);
    // Cancellation/deadline failure is monotonic; preserve its original typed
    // reason instead of promoting the visitor's stop to CorruptObject.
    budget.checkpoint()?;
    let result = result.map_err(|error| corrupt(&error))?;
    decoder.end().map_err(|error| corrupt(&error))?;
    budget.checkpoint()?;
    Ok(result)
}

fn preflight(
    bytes: &[u8],
    budget: &WorkBudget,
) -> Result<(PositionLayout, Option<MemoryLease>), IndexError> {
    budget.checkpoint()?;
    let parse_bytes = u64::try_from(bytes.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(3)
        .ok_or(IndexError::ResourceLimited)?;
    // Field-name escape scratch is bounded separately from positions. The two
    // parser instances are sequential and die before the other pass starts.
    let parse_memory = if parse_bytes == 0 {
        None
    } else {
        budget.reserve_memory(parse_bytes, MemoryLifetime::Temporary)?
    };
    let limit =
        usize::try_from(MAX_POSITIONS_PER_TERM).map_err(|_error| IndexError::ResourceLimited)?;
    let layout = parse(
        bytes,
        PositionsSeed {
            budget,
            positions: None,
            expected: None,
            limit,
        },
    )?;
    Ok((layout, parse_memory))
}

fn decode_payload(bytes: &[u8], budget: &WorkBudget) -> Result<ScopedOccurrence, IndexError> {
    let (layout, _parse_memory) = preflight(bytes, budget)?;
    let limit =
        usize::try_from(MAX_POSITIONS_PER_TERM).map_err(|_error| IndexError::ResourceLimited)?;
    let data_bytes = layout
        .count
        .checked_mul(std::mem::size_of::<u32>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(data_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(layout.count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let decoded = parse(
        bytes,
        PositionsSeed {
            budget,
            positions: Some(positions),
            expected: Some(layout.count),
            limit,
        },
    )?;
    let positions = decoded.positions.ok_or_else(|| {
        IndexError::BackendViolation("occurrence materialization returned only a count".into())
    })?;
    Ok(ScopedOccurrence {
        value: Occurrence {
            positions: Positions(positions),
        },
        _memory: memory,
    })
}

fn corrupt(error: &serde_json::Error) -> IndexError {
    IndexError::CorruptObject(format!("invalid text occurrence: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    mod reader;
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            Digest, MemoryLeaseOwner, MemoryScope, MemoryScopePort, ObjectBatchInput,
            RegisteredObject, RegisteredObjectBatch, WorkLimits, WorkUsage, ports::ObjectWriteSink,
        },
    };
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    #[derive(Clone, Debug)]
    struct State {
        maximum: u64,
        live: u64,
        peak: u64,
        requests: Vec<u64>,
        deny_on: Option<usize>,
        cancel_on: Option<usize>,
        invalid_release: bool,
    }
    struct Tracker {
        state: Mutex<State>,
        request: RequestBudgetV1,
    }
    impl Tracker {
        fn snapshot(&self) -> Result<State, IndexError> {
            self.state
                .lock()
                .map(|state| state.clone())
                .map_err(|_error| {
                    IndexError::BackendViolation("position test tracker poisoned".into())
                })
        }
    }
    impl MemoryLeaseOwner for Tracker {
        fn release(&self, bytes: u64) {
            if let Ok(mut state) = self.state.lock() {
                if let Some(live) = state.live.checked_sub(bytes) {
                    state.live = live;
                } else {
                    state.invalid_release = true;
                }
            }
        }
    }
    struct Scope(Arc<Tracker>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "positions must use their execution lifetime".into(),
                ));
            }
            let mut state = self.0.state.lock().map_err(|_error| {
                IndexError::BackendViolation("position test tracker poisoned".into())
            })?;
            state.requests.push(bytes);
            let call = state.requests.len();
            if state.deny_on == Some(call) {
                return Err(IndexError::ResourceLimited);
            }
            let live = state
                .live
                .checked_add(bytes)
                .filter(|live| *live <= state.maximum)
                .ok_or(IndexError::ResourceLimited)?;
            state.live = live;
            state.peak = state.peak.max(live);
            let cancel = state.cancel_on == Some(call);
            drop(state);
            if cancel {
                self.0.request.cancel_handle().cancel();
            }
            let owner = Arc::clone(&self.0);
            Ok(MemoryLease::new(bytes, owner))
        }
    }
    fn funded(
        maximum: u64,
        deny_on: Option<usize>,
        cancel_on: Option<usize>,
    ) -> Result<(WorkBudget, Arc<Tracker>), IndexError> {
        let request = RequestBudgetV1::until(
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let tracker = Arc::new(Tracker {
            state: Mutex::new(State {
                maximum,
                live: 0,
                peak: 0,
                requests: Vec::new(),
                deny_on,
                cancel_on,
                invalid_release: false,
            }),
            request: request.clone(),
        });
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1024 * 1024,
                mutations: 100,
            },
            request,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(
            &tracker,
        ))))));
        Ok((budget, tracker))
    }
    fn require(condition: bool, message: &str) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(message.into()))
        }
    }
    fn cleaned(tracker: &Tracker) -> Result<(), IndexError> {
        let state = tracker.snapshot()?;
        require(
            state.live == 0 && !state.invalid_release,
            "position charge did not cleanly release",
        )
    }

    struct CountPort {
        stored: StoredObject,
        cancel_after_get: bool,
    }

    impl ImmutableObjectPort for CountPort {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected count identification".into(),
            ))
        }

        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected count write".into(),
            ))
        }

        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected count install".into(),
            ))
        }

        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected count batch".into(),
            ))
        }

        fn get(&self, _: ObjectRef, budget: &mut WorkBudget) -> Result<StoredObject, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if self.cancel_after_get {
                budget.request().cancel_handle().cancel();
            }
            Ok(self.stored.clone())
        }
    }

    fn count_port(
        reference: ObjectRef,
        payload: &[u8],
        children: Vec<ObjectRef>,
    ) -> Result<CountPort, IndexError> {
        Ok(CountPort {
            stored: StoredObject::new(
                RegisteredObject::unscoped(
                    reference,
                    u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    children,
                ),
                payload.to_vec(),
                None,
            ),
            cancel_after_get: false,
        })
    }

    #[test]
    fn count_only_checks_complete_wire_without_materializing_positions() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        for bytes in [
            br#"{"positions":[0]}"#.as_slice(),
            br#"{"positions":[0,2]}"#.as_slice(),
            br"[[0,2]]",
            br#"{"po\u0073itions":[0,2]}"#,
        ] {
            let port = count_port(reference, bytes, Vec::new())?;
            let (mut budget, tracker) = funded(1024, None, None)?;
            let expected_count = if bytes == br#"{"positions":[0]}"# {
                1
            } else {
                2
            };
            require(
                load_count(reference, &port, &mut budget)? == expected_count,
                "count changed",
            )?;
            require(
                tracker.snapshot()?.requests.as_slice()
                    == [u64::try_from(bytes.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .checked_mul(3)
                        .ok_or(IndexError::ResourceLimited)?],
                "count reader allocated a position vector or skipped parser admission",
            )?;
            cleaned(&tracker)?;
        }
        let bytes = br#"{"positions":[0,2]}"#;
        let port = count_port(reference, bytes, Vec::new())?;
        let parser_ceiling = u64::try_from(bytes.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(3)
            .ok_or(IndexError::ResourceLimited)?;
        let (mut count_budget, count_tracker) = funded(parser_ceiling, None, None)?;
        require(
            load_count(reference, &port, &mut count_budget)? == 2,
            "count-only read exceeded its parser-only ceiling",
        )?;
        cleaned(&count_tracker)?;
        let (mut full_budget, full_tracker) = funded(parser_ceiling, None, None)?;
        require(
            matches!(
                load(reference, &port, &mut full_budget),
                Err(IndexError::ResourceLimited)
            ),
            "full position reader lost its independent vector admission",
        )?;
        require(
            full_tracker.snapshot()?.requests.as_slice() == [parser_ceiling, 8],
            "full reader did not request the exact two-position vector",
        )?;
        cleaned(&full_tracker)?;
        let boundary = serde_json::to_vec(
            &serde_json::json!({"positions": (0..8192_u32).collect::<Vec<_>>()}),
        )
        .map_err(|error| corrupt(&error))?;
        let port = count_port(reference, &boundary, Vec::new())?;
        let (mut budget, tracker) = funded(1024 * 1024, None, None)?;
        require(
            load_count(reference, &port, &mut budget)? == 8192,
            "boundary count changed",
        )?;
        require(
            tracker.snapshot()?.requests.len() == 1,
            "boundary allocated a vector",
        )?;
        cleaned(&tracker)?;
        for invalid in [
            br"{}".as_slice(),
            br#"{"positions":[]}"#,
            br#"{"positions":[2,1]}"#,
            br#"{"positions":[1,1]}"#,
            br#"{"positions":[-1]}"#,
            br#"{"positions":[4294967296]}"#,
            br#"{"positions":[1.5]}"#,
            br#"{"positions":[1],"positions":[2]}"#,
            br#"{"positions":[1],"extra":0}"#,
            br"[[1],0]",
            br#"{"positions":[1]} false"#,
        ] {
            let port = count_port(reference, invalid, Vec::new())?;
            let (mut budget, tracker) = funded(1024, None, None)?;
            require(
                matches!(
                    load_count(reference, &port, &mut budget),
                    Err(IndexError::CorruptObject(_))
                ),
                "invalid count leaf was admitted",
            )?;
            require(
                tracker.snapshot()?.requests.len() == 1,
                "invalid leaf allocated a vector",
            )?;
            cleaned(&tracker)?;
        }
        let excess = serde_json::to_vec(
            &serde_json::json!({"positions": (0..8193_u32).collect::<Vec<_>>()}),
        )
        .map_err(|error| corrupt(&error))?;
        let port = count_port(reference, &excess, Vec::new())?;
        let (mut budget, tracker) = funded(1024 * 1024, None, None)?;
        require(
            matches!(
                load_count(reference, &port, &mut budget),
                Err(IndexError::CorruptObject(_))
            ),
            "excess count leaf was admitted",
        )?;
        require(
            tracker.snapshot()?.requests.len() == 1,
            "excess leaf allocated a vector",
        )?;
        cleaned(&tracker)
    }

    #[test]
    fn count_only_preserves_object_authentication_and_original_refusal() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        let other = ObjectRef(Digest([8; 32]));
        let payload = br#"{"positions":[0,2]}"#;
        for port in [
            count_port(other, payload, Vec::new())?,
            count_port(reference, payload, vec![other])?,
        ] {
            let (mut budget, tracker) = funded(1024, None, None)?;
            require(
                matches!(
                    load_count(reference, &port, &mut budget),
                    Err(IndexError::CorruptObject(_))
                ),
                "count accepted a mismatched registration or child edge",
            )?;
            require(
                tracker.snapshot()?.requests.is_empty(),
                "invalid object reached parser",
            )?;
            cleaned(&tracker)?;
        }
        let (mut budget, tracker) = funded(1024, Some(1), None)?;
        let port = count_port(reference, payload, Vec::new())?;
        require(
            matches!(
                load_count(reference, &port, &mut budget),
                Err(IndexError::ResourceLimited)
            ),
            "parser refusal was replaced",
        )?;
        cleaned(&tracker)?;
        let (mut budget, tracker) = funded(1024, None, Some(1))?;
        require(
            matches!(
                load_count(reference, &port, &mut budget),
                Err(IndexError::Cancelled)
            ),
            "parser cancellation was replaced",
        )?;
        cleaned(&tracker)?;
        let mut cancelled = count_port(reference, payload, Vec::new())?;
        cancelled.cancel_after_get = true;
        let (mut budget, tracker) = funded(1024, None, None)?;
        require(
            matches!(
                load_count(reference, &cancelled, &mut budget),
                Err(IndexError::Cancelled)
            ),
            "post-read cancellation was replaced",
        )?;
        require(
            tracker.snapshot()?.requests.is_empty(),
            "cancelled read reached parser",
        )?;
        cleaned(&tracker)
    }

    #[test]
    fn exact_capacity_remains_charged_through_moves_until_last_use() -> Result<(), IndexError> {
        let (budget, tracker) = funded(1024, None, None)?;
        let data = decode_payload(br#"{"positions":[0,2,4294967295]}"#, &budget)?;
        require(
            *data.positions == [0, 2, u32::MAX],
            "position value differs from independent golden",
        )?;
        require(
            data.positions.0.capacity() == 3,
            "decoded capacity grew beyond the preflight",
        )?;
        let state = tracker.snapshot()?;
        require(
            state.live == 12 && state.requests.last() == Some(&12),
            "position charge is not the actual vector capacity",
        )?;
        let moved = data;
        require(
            tracker.snapshot()?.live == 12,
            "moving a head released its positions",
        )?;
        require(moved.positions.len() == 3, "moved positions changed")?;
        drop(moved);
        cleaned(&tracker)
    }

    #[test]
    fn malformed_closed_records_refuse_before_position_allocation() -> Result<(), IndexError> {
        for invalid in [
            br"{}".as_slice(),
            br#"{"positions":[]}"#,
            br#"{"positions":[2,1]}"#,
            br#"{"positions":[1,1]}"#,
            br#"{"positions":[-1]}"#,
            br#"{"positions":[4294967296]}"#,
            br#"{"positions":[1.5]}"#,
            br#"{"positions":[1],"positions":[2]}"#,
            br#"{"positions":[1],"extra":0}"#,
            br"[[1],0]",
            br#"{"positions":[1]} false"#,
        ] {
            let (budget, tracker) = funded(1024, None, None)?;
            require(
                matches!(
                    decode_payload(invalid, &budget),
                    Err(IndexError::CorruptObject(_))
                ),
                "invalid occurrence was not refused",
            )?;
            require(
                tracker.snapshot()?.requests.len() == 1,
                "invalid occurrence materialized a position vector",
            )?;
            cleaned(&tracker)?;
        }
        Ok(())
    }

    #[test]
    fn boundary_and_excess_sequences_have_independent_allocation_oracles() -> Result<(), IndexError>
    {
        let values: Vec<u32> = (0..8192).collect();
        let bytes = serde_json::to_vec(&serde_json::json!({"positions": values}))
            .map_err(|error| corrupt(&error))?;
        let (budget, tracker) = funded(1024 * 1024, None, None)?;
        let decoded = decode_payload(&bytes, &budget)?;
        require(
            decoded.positions.len() == 8192 && decoded.positions.0.capacity() == 8192,
            "exact boundary was not admitted",
        )?;
        require(
            tracker.snapshot()?.live == 32_768,
            "boundary position footprint is incorrect",
        )?;
        drop(decoded);
        cleaned(&tracker)?;
        let excess = serde_json::to_vec(
            &serde_json::json!({"positions": (0..8193_u32).collect::<Vec<_>>()}),
        )
        .map_err(|error| corrupt(&error))?;
        let (budget, tracker) = funded(1024 * 1024, None, None)?;
        require(
            matches!(
                decode_payload(&excess, &budget),
                Err(IndexError::CorruptObject(_))
            ),
            "excess leaf was admitted",
        )?;
        require(
            tracker.snapshot()?.requests.len() == 1,
            "excess leaf reached materialization",
        )?;
        cleaned(&tracker)
    }

    #[test]
    fn vector_denial_and_original_cancellation_release_parser_and_data() -> Result<(), IndexError> {
        for (deny, cancel, cancelled) in [(Some(2), None, false), (None, Some(2), true)] {
            let (mut budget, tracker) = funded(1024, deny, cancel)?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 7,
                ..WorkUsage::default()
            })?;
            let before = budget.usage();
            let deadline = budget.request().deadline();
            let result = decode_payload(br#"{"positions":[0,2]}"#, &budget);
            require(
                if cancelled {
                    matches!(result, Err(IndexError::Cancelled))
                } else {
                    matches!(result, Err(IndexError::ResourceLimited))
                },
                "allocation refusal lost its original reason",
            )?;
            require(
                tracker.snapshot()?.requests.len() == 2,
                "unexpected reservation phase",
            )?;
            require(
                budget.usage() == before && budget.request().deadline() == deadline,
                "decoder replaced original work authority",
            )?;
            cleaned(&tracker)?;
        }
        Ok(())
    }

    #[test]
    fn simultaneous_term_heads_sum_live_positions_and_failed_head_preserves_prior()
    -> Result<(), IndexError> {
        let bytes = br#"{"positions":[0,2]}"#;
        let (budget, tracker) = funded(128, None, None)?;
        let first = decode_payload(bytes, &budget)?;
        let second = decode_payload(bytes, &budget)?;
        require(
            tracker.snapshot()?.live == 16,
            "simultaneous position heads are not summed",
        )?;
        {
            let mut state = tracker.state.lock().map_err(|_error| {
                IndexError::BackendViolation("position test tracker poisoned".into())
            })?;
            state.deny_on = Some(6);
        }
        require(
            matches!(
                decode_payload(bytes, &budget),
                Err(IndexError::ResourceLimited)
            ),
            "third vector denial was not propagated",
        )?;
        require(
            tracker.snapshot()?.live == 16
                && first.positions.len() == 2
                && second.positions.len() == 2,
            "denied head changed prior live data",
        )?;
        drop(first);
        require(
            tracker.snapshot()?.live == 8,
            "first head did not release its own charge",
        )?;
        drop(second);
        cleaned(&tracker)
    }

    #[test]
    fn canonical_map_sequence_and_escaped_field_shapes_preserve_wire_values()
    -> Result<(), IndexError> {
        for bytes in [
            br#"{"positions":[0,2]}"#.as_slice(),
            br"[[0,2]]",
            br#"{"po\u0073itions":[0,2]}"#,
        ] {
            let raw: Occurrence = serde_json::from_slice(bytes).map_err(|error| corrupt(&error))?;
            let (budget, tracker) = funded(1024, None, None)?;
            let loaded = decode_payload(bytes, &budget)?;
            require(
                loaded.value == raw,
                "scoped reader changed canonical occurrence shape or values",
            )?;
            let wire = serde_json::to_vec(&loaded.value).map_err(|error| corrupt(&error))?;
            require(
                wire.as_slice() == br#"{"positions":[0,2]}"#,
                "canonical wire changed",
            )?;
            drop(loaded);
            cleaned(&tracker)?;
        }
        Ok(())
    }
}
