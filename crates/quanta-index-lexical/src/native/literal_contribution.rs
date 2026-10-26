//! Bounded preflight and exact array custody for the canonical Literal record.

use std::{fmt, ops::Deref};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, StoredObject, WorkBudget,
    ports::ImmutableObjectPort,
};
use serde::{
    Deserialize,
    de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use super::literal_grams::{MAX_GRAMS, PreparedGrams};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Contribution {
    pub(super) record: ObjectRef,
    pub(super) grams: Vec<[u8; 3]>,
}
quanta_index_contract::native_serde_struct!(Contribution {
    record: ObjectRef,
    grams: Vec<[u8; 3]>,
});

pub(super) struct LoadedContribution {
    value: Contribution,
    // Data is destroyed before its live charge, including parse/error unwind.
    _memory: Option<MemoryLease>,
}

impl LoadedContribution {
    pub(super) fn new(record: ObjectRef, data: PreparedGrams) -> Self {
        let (grams, memory) = data.into_parts();
        Self {
            value: Contribution { record, grams },
            _memory: memory,
        }
    }
}

impl Deref for LoadedContribution {
    type Target = Contribution;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

struct GramLayout {
    count: usize,
    data: Option<PreparedGrams>,
}

struct GramsSeed<'a> {
    budget: &'a WorkBudget,
    failure: &'a mut Option<IndexError>,
    data: Option<PreparedGrams>,
    expected: Option<usize>,
}

impl<'de> DeserializeSeed<'de> for GramsSeed<'_> {
    type Value = GramLayout;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for GramsSeed<'_> {
    type Value = GramLayout;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("at most 262144 strictly increasing three-byte grams")
    }
    fn visit_seq<A: SeqAccess<'de>>(mut self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut count = 0_usize;
        let mut prior = None;
        loop {
            if let Err(error) = self.budget.checkpoint() {
                *self.failure = Some(error);
                return Err(serde::de::Error::custom("literal gram request stopped"));
            }
            let Some(gram) = sequence.next_element::<[u8; 3]>()? else {
                break;
            };
            if count == MAX_GRAMS {
                *self.failure = Some(IndexError::ResourceLimited);
                return Err(serde::de::Error::custom("literal gram count exceeds limit"));
            }
            if prior.is_some_and(|prior| prior >= gram) {
                return Err(serde::de::Error::custom(
                    "literal grams must be sorted and unique",
                ));
            }
            if let Some(data) = &mut self.data {
                if self.expected.is_none_or(|expected| count >= expected) {
                    return Err(serde::de::Error::custom(
                        "literal grams differ from preflight",
                    ));
                }
                if let Err(error) = data.push(gram) {
                    *self.failure = Some(error);
                    return Err(serde::de::Error::custom("literal gram array is exhausted"));
                }
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| serde::de::Error::custom("literal gram count overflows"))?;
            prior = Some(gram);
        }
        if self.expected.is_some_and(|expected| expected != count) {
            return Err(serde::de::Error::custom(
                "literal grams differ from preflight",
            ));
        }
        Ok(GramLayout {
            count,
            data: self.data,
        })
    }
}

enum Field {
    Record,
    Grams,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct FieldVisitor;
        impl Visitor<'_> for FieldVisitor {
            type Value = Field;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("record or grams")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Field, E> {
                match value {
                    "record" => Ok(Field::Record),
                    "grams" => Ok(Field::Grams),
                    _ => Err(E::unknown_field(value, &["record", "grams"])),
                }
            }
        }
        decoder.deserialize_identifier(FieldVisitor)
    }
}

struct ContributionLayout {
    record: ObjectRef,
    grams: GramLayout,
}

struct ContributionSeed<'a>(GramsSeed<'a>);

impl<'de> DeserializeSeed<'de> for ContributionSeed<'_> {
    type Value = ContributionLayout;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct("Contribution", &["record", "grams"], self)
    }
}

impl<'de> Visitor<'de> for ContributionSeed<'_> {
    type Value = ContributionLayout;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a complete Contribution record")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut record = None;
        let mut grams = None;
        let mut seed = Some(self.0);
        while let Some(field) = map.next_key::<Field>()? {
            match field {
                Field::Record => {
                    if record.is_some() {
                        return Err(serde::de::Error::duplicate_field("record"));
                    }
                    record = Some(map.next_value::<ObjectRef>()?);
                }
                Field::Grams => {
                    let selected = seed
                        .take()
                        .ok_or_else(|| serde::de::Error::duplicate_field("grams"))?;
                    grams = Some(map.next_value_seed(selected)?);
                }
            }
        }
        Ok(ContributionLayout {
            record: record.ok_or_else(|| serde::de::Error::missing_field("record"))?,
            grams: grams.ok_or_else(|| serde::de::Error::missing_field("grams"))?,
        })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let record = sequence
            .next_element::<ObjectRef>()?
            .ok_or_else(|| serde::de::Error::missing_field("record"))?;
        let grams = sequence
            .next_element_seed(self.0)?
            .ok_or_else(|| serde::de::Error::missing_field("grams"))?;
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom("literal record has extra fields"));
        }
        Ok(ContributionLayout { record, grams })
    }
}

fn parse(
    bytes: &[u8],
    budget: &WorkBudget,
    data: Option<PreparedGrams>,
    expected: Option<usize>,
) -> Result<ContributionLayout, IndexError> {
    let mut failure = None;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = ContributionSeed(GramsSeed {
        budget,
        failure: &mut failure,
        data,
        expected,
    })
    .deserialize(&mut decoder);
    // Preserve original cancellation/deadline and typed capacity failures.
    budget.checkpoint()?;
    if let Some(error) = failure {
        return Err(error);
    }
    let result = result.map_err(|error| corrupt(&error))?;
    decoder.end().map_err(|error| corrupt(&error))?;
    budget.checkpoint()?;
    Ok(result)
}

/// Validate the object envelope and its exact source dependency before creating
/// the gram array. Raw payload decoding is not a production read boundary.
pub(super) fn load(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedContribution, IndexError> {
    let stored = read(reference, objects, budget)?;
    let _parser_memory = reserve_parser(&stored.payload, budget)?;
    let layout = parse(&stored.payload, budget, None, None)?;
    validate_source(&stored, layout.record)?;
    materialize(&stored.payload, budget, &layout)
}

pub(super) fn load_record(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<ObjectRef, IndexError> {
    let stored = read(reference, objects, budget)?;
    let record = record(&stored.payload, budget)?;
    validate_source(&stored, record)?;
    Ok(record)
}

fn read(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    budget.checkpoint()?;
    let stored = objects.get(reference, budget)?;
    budget.checkpoint()?;
    if stored.registration.reference != reference || stored.registration.children.len() != 1 {
        return Err(IndexError::CorruptObject(
            "invalid literal contribution object or child count".into(),
        ));
    }
    Ok(stored)
}

fn validate_source(stored: &StoredObject, source: ObjectRef) -> Result<(), IndexError> {
    if stored.registration.children.as_slice() != [source] {
        return Err(IndexError::CorruptObject(
            "literal contribution source edge mismatch".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn decode_payload(bytes: &[u8], budget: &WorkBudget) -> Result<LoadedContribution, IndexError> {
    let _parser_memory = reserve_parser(bytes, budget)?;
    let layout = parse(bytes, budget, None, None)?;
    materialize(bytes, budget, &layout)
}

fn materialize(
    bytes: &[u8],
    budget: &WorkBudget,
    layout: &ContributionLayout,
) -> Result<LoadedContribution, IndexError> {
    let data = PreparedGrams::allocate(layout.grams.count, budget)?;
    let decoded = parse(bytes, budget, Some(data), Some(layout.grams.count))?;
    if decoded.record != layout.record {
        return Err(IndexError::BackendViolation(
            "literal record differs from preflight".into(),
        ));
    }
    let data = decoded.grams.data.ok_or_else(|| {
        IndexError::BackendViolation("literal materialization returned only a count".into())
    })?;
    Ok(LoadedContribution::new(decoded.record, data))
}

/// Queries need the source reference, never an owned gram array. Validate the
/// complete record under the same bounded preflight without materializing it.
fn record(bytes: &[u8], budget: &WorkBudget) -> Result<ObjectRef, IndexError> {
    let _parser_memory = reserve_parser(bytes, budget)?;
    Ok(parse(bytes, budget, None, None)?.record)
}

fn reserve_parser(bytes: &[u8], budget: &WorkBudget) -> Result<Option<MemoryLease>, IndexError> {
    budget.checkpoint()?;
    let parser_bytes = u64::try_from(bytes.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(3)
        .ok_or(IndexError::ResourceLimited)?;
    // Field escape/error scratch is separate from the exact gram array. Both
    // parser passes are sequential under this conservative logical allowance.
    if parser_bytes == 0 {
        Ok(None)
    } else {
        budget.reserve_memory(parser_bytes, MemoryLifetime::Temporary)
    }
}

fn corrupt(error: &serde_json::Error) -> IndexError {
    IndexError::CorruptObject(format!("invalid literal contribution: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{
        RequestBudgetV1,
        index::{Digest, MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits},
    };
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    const REFERENCE: &str = "[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7]";

    #[derive(Clone, Default)]
    struct State {
        cap: u64,
        live: u64,
        peak: u64,
        requests: Vec<u64>,
        deny_on: Option<usize>,
        cancel_on: Option<usize>,
        invalid: bool,
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
                    IndexError::BackendViolation("literal decode tracker poisoned".into())
                })
        }
    }

    impl MemoryLeaseOwner for Tracker {
        fn release(&self, bytes: u64) {
            if let Ok(mut state) = self.state.lock() {
                if let Some(live) = state.live.checked_sub(bytes) {
                    state.live = live;
                } else {
                    state.invalid = true;
                }
            }
        }
    }

    struct Scope(Arc<Tracker>);

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            require(lifetime == MemoryLifetime::Temporary)?;
            let mut state = self.0.state.lock().map_err(|_error| {
                IndexError::BackendViolation("literal decode tracker poisoned".into())
            })?;
            state.requests.push(bytes);
            let call = state.requests.len();
            if state.deny_on == Some(call) {
                return Err(IndexError::ResourceLimited);
            }
            state.live = state
                .live
                .checked_add(bytes)
                .filter(|live| *live <= state.cap)
                .ok_or(IndexError::ResourceLimited)?;
            state.peak = state.peak.max(state.live);
            let cancel = state.cancel_on == Some(call);
            drop(state);
            if cancel {
                self.0.request.cancel_handle().cancel();
            }
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn funded(
        cap: u64,
        deny_on: Option<usize>,
        cancel_on: Option<usize>,
    ) -> Result<(WorkBudget, Arc<Tracker>), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000_000,
                mutations: 1_000_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let tracker = Arc::new(Tracker {
            state: Mutex::new(State {
                cap,
                deny_on,
                cancel_on,
                ..State::default()
            }),
            request: budget.request().clone(),
        });
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(
            &tracker,
        ))))));
        Ok((budget, tracker))
    }

    fn require(condition: bool) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "literal contribution custody law failed".into(),
            ))
        }
    }

    fn parser_bytes(bytes: &[u8]) -> Result<u64, IndexError> {
        u64::try_from(bytes.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(3)
            .ok_or(IndexError::ResourceLimited)
    }

    #[test]
    fn canonical_map_sequence_and_escaped_fields_keep_exact_wire_and_live_array()
    -> Result<(), IndexError> {
        let canonical = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[98,99,100]]}}");
        let sequence = format!("[{REFERENCE},[[97,98,99],[98,99,100]]]");
        let reordered =
            format!("{{\"gr\\u0061ms\":[[97,98,99],[98,99,100]],\"record\":{REFERENCE}}}");
        for wire in [&canonical, &sequence, &reordered] {
            let parser = parser_bytes(wire.as_bytes())?;
            let (budget, tracker) = funded(parser + 6, None, None)?;
            let decoded = decode_payload(wire.as_bytes(), &budget)?;
            require(decoded.record == ObjectRef(Digest([7; 32])))?;
            require(decoded.grams.as_slice() == [*b"abc", *b"bcd"])?;
            let state = tracker.snapshot()?;
            require(state.requests.as_slice() == [parser, 6])?;
            require(state.live == 6 && state.peak == parser + 6)?;
            let serialized =
                serde_json::to_string(&decoded.value).map_err(|error| corrupt(&error))?;
            require(serialized == canonical)?;
            drop(decoded);
            let state = tracker.snapshot()?;
            require(state.live == 0 && !state.invalid)?;
        }
        Ok(())
    }

    #[test]
    fn parser_and_array_floor_refuse_and_original_cancel_releases_credit() -> Result<(), IndexError>
    {
        let wire = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[98,99,100]]}}");
        let parser = parser_bytes(wire.as_bytes())?;
        for (cap, deny_on, expected_calls) in [
            (parser - 1, None, 1),
            (parser + 5, None, 2),
            (u64::MAX, Some(1), 1),
            (u64::MAX, Some(2), 2),
        ] {
            let (budget, tracker) = funded(cap, deny_on, None)?;
            require(matches!(
                decode_payload(wire.as_bytes(), &budget),
                Err(IndexError::ResourceLimited)
            ))?;
            let state = tracker.snapshot()?;
            require(state.requests.len() == expected_calls && state.live == 0 && !state.invalid)?;
        }
        for cancel_on in [1, 2] {
            let (budget, tracker) = funded(u64::MAX, None, Some(cancel_on))?;
            require(matches!(
                decode_payload(wire.as_bytes(), &budget),
                Err(IndexError::Cancelled)
            ))?;
            let state = tracker.snapshot()?;
            require(state.requests.len() == cancel_on && state.live == 0 && !state.invalid)?;
        }
        let (budget, tracker) = funded(u64::MAX, None, None)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            decode_payload(wire.as_bytes(), &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(tracker.snapshot()?.requests.is_empty())
    }

    #[test]
    fn malformed_closed_records_and_unsorted_grams_refuse_before_array_birth()
    -> Result<(), IndexError> {
        let malformed = [
            format!("{{\"record\":{REFERENCE}}}"),
            "{\"grams\":[]}".into(),
            "{\"record\":null,\"grams\":[]}".into(),
            format!("{{\"record\":{REFERENCE},\"grams\":null}}"),
            format!("{{\"record\":{REFERENCE},\"record\":{REFERENCE},\"grams\":[]}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[],\"grams\":[]}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[],\"extra\":0}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[[1,2]]}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[[1,2,3,4]]}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[[1,2,256]]}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[[1,2,3],[1,2,3]]}}"),
            format!("{{\"record\":{REFERENCE},\"grams\":[[1,2,4],[1,2,3]]}}"),
            format!("[{REFERENCE},[],0]"),
            format!("[{REFERENCE}]"),
            format!("{{\"record\":{REFERENCE},\"grams\":[]}} trailing"),
        ];
        for wire in malformed {
            let (budget, tracker) = funded(u64::MAX, None, None)?;
            require(matches!(
                decode_payload(wire.as_bytes(), &budget),
                Err(IndexError::CorruptObject(_))
            ))?;
            let state = tracker.snapshot()?;
            require(state.requests.len() == 1 && state.live == 0 && !state.invalid)?;
        }
        Ok(())
    }

    #[test]
    fn empty_and_maximum_envelopes_are_exact_and_overflow_has_no_array() -> Result<(), IndexError> {
        for count in [0, MAX_GRAMS, MAX_GRAMS + 1] {
            let mut grams = Vec::new();
            grams
                .try_reserve_exact(count)
                .map_err(|_error| IndexError::ResourceLimited)?;
            for ordinal in 0..count {
                let [_, first, second, third] = u32::try_from(ordinal)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes();
                grams.push([first, second, third]);
            }
            let value = Contribution {
                record: ObjectRef(Digest([7; 32])),
                grams,
            };
            let wire = serde_json::to_vec(&value).map_err(|error| corrupt(&error))?;
            let (budget, tracker) = funded(u64::MAX, None, None)?;
            let result = decode_payload(&wire, &budget);
            if count > MAX_GRAMS {
                require(matches!(result, Err(IndexError::ResourceLimited)))?;
                require(tracker.snapshot()?.requests.len() == 1)?;
            } else {
                let decoded = result?;
                require(decoded.value == value)?;
                let data = u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)? * 3;
                let state = tracker.snapshot()?;
                require(state.live == data)?;
                if count == 0 {
                    require(state.requests.as_slice() == [parser_bytes(&wire)?])?;
                } else {
                    require(state.requests.as_slice() == [parser_bytes(&wire)?, data])?;
                }
                drop(decoded);
            }
            let state = tracker.snapshot()?;
            require(state.live == 0 && !state.invalid)?;
        }
        Ok(())
    }

    #[test]
    fn failed_materialization_drops_array_before_refunding_its_lease() -> Result<(), IndexError> {
        let wire = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[97,98,99]]}}");
        let (budget, tracker) = funded(6, None, None)?;
        let data = PreparedGrams::allocate(2, &budget)?;
        require(tracker.snapshot()?.live == 6)?;
        require(matches!(
            parse(wire.as_bytes(), &budget, Some(data), Some(2)),
            Err(IndexError::CorruptObject(_))
        ))?;
        let state = tracker.snapshot()?;
        require(state.live == 0 && !state.invalid)
    }

    #[test]
    fn original_cancellation_is_observed_inside_gram_sequence() -> Result<(), IndexError> {
        use serde::de::value::SeqDeserializer;
        let (budget, tracker) = funded(6, None, None)?;
        let data = PreparedGrams::allocate(2, &budget)?;
        let mut failure = None;
        let mut calls = 0_usize;
        let rows = [
            serde_json::json!([97, 98, 99]),
            serde_json::json!([98, 99, 100]),
        ];
        let values = rows.into_iter().inspect(|_value| {
            calls += 1;
            if calls == 1 {
                budget.request().cancel_handle().cancel();
            }
        });
        let decoder = SeqDeserializer::<_, serde_json::Error>::new(values);
        let result = GramsSeed {
            budget: &budget,
            failure: &mut failure,
            data: Some(data),
            expected: Some(2),
        }
        .deserialize(decoder);
        require(result.is_err() && failure == Some(IndexError::Cancelled))?;
        require(calls == 1)?;
        let state = tracker.snapshot()?;
        require(state.live == 0 && !state.invalid)
    }

    #[test]
    fn record_lookup_checks_complete_wire_without_materializing_gram_array()
    -> Result<(), IndexError> {
        let wire = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[98,99,100]]}}");
        let parser = parser_bytes(wire.as_bytes())?;
        let (budget, tracker) = funded(parser, None, None)?;
        require(record(wire.as_bytes(), &budget)? == ObjectRef(Digest([7; 32])))?;
        let state = tracker.snapshot()?;
        require(state.requests.as_slice() == [parser] && state.live == 0 && !state.invalid)?;
        let duplicate = format!("{{\"record\":{REFERENCE},\"grams\":[[1,2,3],[1,2,3]]}}");
        require(matches!(
            record(duplicate.as_bytes(), &budget),
            Err(IndexError::CorruptObject(_))
        ))?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            record(wire.as_bytes(), &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(tracker.snapshot()?.requests.len() == 2)
    }

    #[test]
    fn expired_original_deadline_refuses_before_parser_and_cancellation_has_precedence()
    -> Result<(), IndexError> {
        let (_, tracker) = funded(u64::MAX, None, None)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(
            &tracker,
        ))))));
        for query in [false, true] {
            let outcome = if query {
                record(b"not-json", &budget).map(|_record| ())
            } else {
                decode_payload(b"not-json", &budget).map(|_value| ())
            };
            require(matches!(outcome, Err(IndexError::ResourceLimited)))?;
        }
        budget.request().cancel_handle().cancel();
        require(matches!(
            decode_payload(b"not-json", &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(matches!(
            record(b"not-json", &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(tracker.snapshot()?.requests.is_empty())
    }

    struct ReadOnlyObjects {
        requested: ObjectRef,
        stored: StoredObject,
        reads: AtomicUsize,
        writes: AtomicUsize,
        cancel_after_read: bool,
    }

    impl ReadOnlyObjects {
        fn unexpected_write<T>(&self) -> Result<T, IndexError> {
            let _previous = self.writes.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "unexpected literal read mutation".into(),
            ))
        }
    }

    impl ImmutableObjectPort for ReadOnlyObjects {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
            self.unexpected_write()
        }
        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
            self.unexpected_write()
        }
        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
            self.unexpected_write()
        }
        fn install_batch(
            &self,
            _: &[quanta_index_core::index::ObjectBatchInput],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            self.unexpected_write()
        }
        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.checkpoint()?;
            require(reference == self.requested)?;
            budget.charge(quanta_index_core::index::WorkUsage {
                reads: 1,
                ..quanta_index_core::index::WorkUsage::default()
            })?;
            let _previous = self.reads.fetch_add(1, Ordering::Relaxed);
            if self.cancel_after_read {
                budget.request().cancel_handle().cancel();
            }
            Ok(self.stored.clone())
        }
    }

    fn objects(
        returned: ObjectRef,
        children: Vec<ObjectRef>,
        wire: &[u8],
    ) -> Result<ReadOnlyObjects, IndexError> {
        Ok(ReadOnlyObjects {
            requested: ObjectRef(Digest([9; 32])),
            stored: StoredObject::new(
                quanta_index_core::index::RegisteredObject::unscoped(
                    returned,
                    u64::try_from(wire.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    children,
                ),
                wire.to_vec(),
                None,
            ),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            cancel_after_read: false,
        })
    }

    fn read_outcome(
        query: bool,
        objects: &ReadOnlyObjects,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if query {
            load_record(objects.requested, objects, budget).map(|_record| ())
        } else {
            load(objects.requested, objects, budget).map(|_data| ())
        }
    }

    #[test]
    fn object_loaders_bind_exact_source_and_query_does_not_materialize_grams()
    -> Result<(), IndexError> {
        let wire = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[98,99,100]]}}");
        let reference = ObjectRef(Digest([9; 32]));
        let source = ObjectRef(Digest([7; 32]));
        for query in [false, true] {
            let objects = objects(reference, vec![source], wire.as_bytes())?;
            let parser = parser_bytes(wire.as_bytes())?;
            let (mut budget, tracker) = funded(parser + 6, None, None)?;
            if query {
                require(load_record(reference, &objects, &mut budget)? == source)?;
                let state = tracker.snapshot()?;
                require(state.requests.as_slice() == [parser] && state.live == 0)?;
            } else {
                let data = load(reference, &objects, &mut budget)?;
                require(data.record == source && data.grams.as_slice() == [*b"abc", *b"bcd"])?;
                let state = tracker.snapshot()?;
                require(state.requests.as_slice() == [parser, 6] && state.live == 6)?;
                drop(data);
            }
            require(objects.reads.load(Ordering::Relaxed) == 1 && budget.usage().reads == 1)?;
            require(objects.writes.load(Ordering::Relaxed) == 0)?;
            let state = tracker.snapshot()?;
            require(state.live == 0 && !state.invalid)?;
        }
        Ok(())
    }

    #[test]
    fn wrong_object_identity_and_missing_extra_duplicate_or_wrong_edges_refuse_before_array()
    -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        let source = ObjectRef(Digest([7; 32]));
        let foreign = ObjectRef(Digest([8; 32]));
        let wire = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[98,99,100]]}}");
        for query in [false, true] {
            for (returned, children, expected_calls) in [
                (foreign, vec![source], 0),
                (reference, vec![], 0),
                (reference, vec![source, source], 0),
                (reference, vec![source, foreign], 0),
                (reference, vec![foreign], 1),
            ] {
                let objects = objects(returned, children, wire.as_bytes())?;
                let (mut budget, tracker) = funded(u64::MAX, None, None)?;
                require(matches!(
                    read_outcome(query, &objects, &mut budget),
                    Err(IndexError::CorruptObject(_))
                ))?;
                let state = tracker.snapshot()?;
                require(
                    state.requests.len() == expected_calls && state.live == 0 && !state.invalid,
                )?;
                require(
                    objects.reads.load(Ordering::Relaxed) == 1
                        && objects.writes.load(Ordering::Relaxed) == 0,
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn object_loader_preserves_original_cancel_before_and_after_read() -> Result<(), IndexError> {
        for query in [false, true] {
            for cancel_before in [false, true] {
                // A malformed envelope must not override original cancellation.
                let mut objects = objects(ObjectRef(Digest([8; 32])), vec![], b"not-json")?;
                objects.cancel_after_read = !cancel_before;
                let (mut budget, tracker) = funded(u64::MAX, None, None)?;
                if cancel_before {
                    budget.request().cancel_handle().cancel();
                }
                require(matches!(
                    read_outcome(query, &objects, &mut budget),
                    Err(IndexError::Cancelled)
                ))?;
                require(objects.reads.load(Ordering::Relaxed) == usize::from(!cancel_before))?;
                require(objects.writes.load(Ordering::Relaxed) == 0)?;
                let state = tracker.snapshot()?;
                require(state.requests.is_empty() && state.live == 0 && !state.invalid)?;
            }
        }
        Ok(())
    }

    #[test]
    fn object_loader_corrupt_payload_and_array_refusal_leave_no_mutation_or_credit()
    -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([9; 32]));
        let source = ObjectRef(Digest([7; 32]));
        let corrupt = format!("{{\"record\":{REFERENCE},\"grams\":[[1,2,3],[1,2,3]]}}");
        for query in [false, true] {
            let objects = objects(reference, vec![source], corrupt.as_bytes())?;
            let (mut budget, tracker) = funded(u64::MAX, None, None)?;
            require(matches!(
                read_outcome(query, &objects, &mut budget),
                Err(IndexError::CorruptObject(_))
            ))?;
            let state = tracker.snapshot()?;
            require(state.requests.len() == 1 && state.live == 0 && !state.invalid)?;
            require(objects.writes.load(Ordering::Relaxed) == 0)?;
        }
        let wire = format!("{{\"record\":{REFERENCE},\"grams\":[[97,98,99],[98,99,100]]}}");
        let objects = objects(reference, vec![source], wire.as_bytes())?;
        let (mut budget, tracker) = funded(parser_bytes(wire.as_bytes())? + 5, None, None)?;
        require(matches!(
            load(reference, &objects, &mut budget),
            Err(IndexError::ResourceLimited)
        ))?;
        let state = tracker.snapshot()?;
        require(state.requests.len() == 2 && state.live == 0 && !state.invalid)?;
        require(
            objects.reads.load(Ordering::Relaxed) == 1
                && objects.writes.load(Ordering::Relaxed) == 0,
        )
    }
}
