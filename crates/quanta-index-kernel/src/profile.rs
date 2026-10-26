//! Bounded decoding and shared allocation custody for the canonical profile.

use std::{fmt, ops::Deref, sync::Arc};

use quanta_index_core::index::{
    IndexError, IndexProfile, InstanceSpec, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget,
    WorkUsage, ports::ImmutableObjectPort,
};
use serde::{
    Deserialize,
    de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

pub(crate) const MAX_PROFILE_INSTANCES: usize = 4096;

#[derive(Clone)]
pub(crate) struct LoadedProfile(Arc<ProfileData>);

struct ProfileData {
    profile: IndexProfile,
    // Charged data is destroyed before the last allocation lease.
    _memory: Option<MemoryLease>,
}

impl Deref for LoadedProfile {
    type Target = IndexProfile;

    fn deref(&self) -> &Self::Target {
        &self.0.profile
    }
}

pub(crate) fn load(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<LoadedProfile, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject("invalid profile object".into()));
    }
    decode_payload(&stored.payload, budget)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileLayout {
    #[serde(deserialize_with = "count_instances")]
    instances: usize,
}

fn count_instances<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<usize, D::Error> {
    struct Count;
    impl<'de> Visitor<'de> for Count {
        type Value = usize;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an index instance array")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<usize, A::Error> {
            let mut count = 0_usize;
            while sequence.next_element::<IgnoredAny>()?.is_some() {
                count = count
                    .checked_add(1)
                    .ok_or_else(|| serde::de::Error::custom("index instance count overflow"))?;
            }
            Ok(count)
        }
    }
    decoder.deserialize_seq(Count)
}

struct InstancesSeed {
    instances: Vec<InstanceSpec>,
    count: usize,
}

impl<'de> DeserializeSeed<'de> for InstancesSeed {
    type Value = Vec<InstanceSpec>;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for InstancesSeed {
    type Value = Vec<InstanceSpec>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the preflighted index instance array")
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut sequence: A) -> Result<Self::Value, A::Error> {
        while let Some(instance) = sequence.next_element::<InstanceSpec>()? {
            if self.instances.len() == self.count {
                return Err(serde::de::Error::custom("profile changed after preflight"));
            }
            self.instances.push(instance);
        }
        if self.instances.len() != self.count {
            return Err(serde::de::Error::custom("profile changed after preflight"));
        }
        Ok(self.instances)
    }
}

struct ProfileSeed(InstancesSeed);

impl<'de> DeserializeSeed<'de> for ProfileSeed {
    type Value = IndexProfile;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct("IndexProfile", &["instances"], self)
    }
}

impl<'de> Visitor<'de> for ProfileSeed {
    type Value = IndexProfile;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a complete IndexProfile record")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut seed = Some(self.0);
        let mut instances = None;
        while let Some(key) = map.next_key::<String>()? {
            if key != "instances" {
                return Err(serde::de::Error::unknown_field(&key, &["instances"]));
            }
            let selected = seed
                .take()
                .ok_or_else(|| serde::de::Error::duplicate_field("instances"))?;
            instances = Some(map.next_value_seed(selected)?);
        }
        Ok(IndexProfile {
            instances: instances.ok_or_else(|| serde::de::Error::missing_field("instances"))?,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let instances = sequence
            .next_element_seed(self.0)?
            .ok_or_else(|| serde::de::Error::missing_field("instances"))?;
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom("profile record has extra fields"));
        }
        Ok(IndexProfile { instances })
    }
}

pub(crate) fn decode_payload(
    bytes: &[u8],
    budget: &mut WorkBudget,
) -> Result<LoadedProfile, IndexError> {
    budget.checkpoint()?;
    // Serde's escaped-string scratch and copied keys coexist during parsing.
    // This is a declared data allowance, not an allocator or process RSS cap.
    let parse_bytes = u64::try_from(bytes.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(3)
        .ok_or(IndexError::ResourceLimited)?;
    let _parse_memory = if parse_bytes == 0 {
        None
    } else {
        budget.reserve_memory(parse_bytes, MemoryLifetime::Temporary)?
    };
    let layout =
        serde_json::from_slice::<ProfileLayout>(bytes).map_err(|error| decode_error(&error));
    budget.checkpoint()?;
    let layout = layout?;
    if layout.instances > MAX_PROFILE_INSTANCES {
        return Err(IndexError::ResourceLimited);
    }
    budget.charge(WorkUsage {
        mutations: u64::try_from(layout.instances).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let data_bytes = layout
        .instances
        .checked_mul(std::mem::size_of::<InstanceSpec>())
        .and_then(|len| len.checked_add(std::mem::size_of::<ProfileData>()))
        .and_then(|len| len.checked_add(std::mem::size_of::<[usize; 2]>()))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(
        u64::try_from(data_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(layout.instances)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let profile = ProfileSeed(InstancesSeed {
        instances,
        count: layout.instances,
    })
    .deserialize(&mut decoder)
    .map_err(|error| decode_error(&error));
    budget.checkpoint()?;
    let profile = profile?;
    decoder.end().map_err(|error| decode_error(&error))?;
    budget.checkpoint()?;
    validate_instances(&profile.instances, budget)?;
    Ok(LoadedProfile(Arc::new(ProfileData {
        profile,
        _memory: memory,
    })))
}

pub(crate) fn validate_instances(
    instances: &[InstanceSpec],
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    if instances.len() > MAX_PROFILE_INSTANCES {
        return Err(IndexError::ResourceLimited);
    }
    if instances.is_empty() {
        return Ok(());
    }
    let index_bytes = instances
        .len()
        .checked_mul(std::mem::size_of::<&InstanceSpec>())
        .ok_or(IndexError::ResourceLimited)?;
    let index_bytes = u64::try_from(index_bytes).map_err(|_error| IndexError::ResourceLimited)?;
    let _validation_memory = budget.reserve_memory(index_bytes, MemoryLifetime::Temporary)?;
    let mut order = Vec::new();
    order
        .try_reserve_exact(instances.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    order.extend(instances.iter());
    order.sort_unstable_by_key(|instance| instance.instance);
    if order
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left.instance == right.instance))
    {
        return Err(IndexError::InvalidInput("duplicate index instance".into()));
    }
    order.sort_unstable_by_key(|instance| instance.group);
    if order.windows(2).any(|pair| {
        matches!(pair, [left, right] if left.group == right.group && left.activation != right.activation)
    }) {
        return Err(IndexError::InvalidInput(
            "publication group has mixed activation".into(),
        ));
    }
    budget.checkpoint()
}

fn decode_error(error: &serde_json::Error) -> IndexError {
    IndexError::CorruptObject(format!("invalid index profile: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if left != right => {
                    return Err(IndexError::BackendViolation(format!(
                        "assertion failed: {} == {}; left: {left:?}, right: {right:?}",
                        stringify!($left),
                        stringify!($right)
                    )));
                }
                _ => {}
            }
        }};
    }

    #[derive(Default)]
    struct Charges {
        retained_calls: AtomicU64,
        live: AtomicU64,
        refused: bool,
    }

    impl MemoryLeaseOwner for Charges {
        fn release(&self, bytes: u64) {
            let _previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    struct Scope(Arc<Charges>);

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime == MemoryLifetime::Retained {
                let _previous = self.0.retained_calls.fetch_add(1, Ordering::SeqCst);
                if self.0.refused {
                    return Err(IndexError::ResourceLimited);
                }
            }
            let _previous = self.0.live.fetch_add(bytes, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn budget(charges: Arc<Charges>) -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1024 * 1024,
                mutations: 8192,
            },
            deadline,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(charges)))));
        Ok(budget)
    }

    fn first_instance() -> InstanceSpec {
        use quanta_index_core::index::{
            Digest, GroupActivation, IndexInstanceId, IndexKind, InputDomain, PublicationGroupId,
            SchemaId, StorageFormatId,
        };
        InstanceSpec {
            instance: IndexInstanceId([1; 16]),
            kind: IndexKind([2; 16]),
            group: PublicationGroupId([3; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([4; 16]),
            policy: Digest([5; 32]),
            format: StorageFormatId([6; 16]),
        }
    }

    #[test]
    fn preflight_refuses_oversized_profile_before_retained_instance_allocation()
    -> Result<(), IndexError> {
        let charges = Arc::new(Charges::default());
        let mut budget = budget(charges.clone())?;
        // Publicly serialized, complete records discriminate preflight from
        // the old decode-first bound. Repeated identity is checked only later.
        let bytes = serde_json::to_vec(&IndexProfile {
            instances: vec![first_instance(); MAX_PROFILE_INSTANCES + 1],
        })
        .map_err(|error| decode_error(&error))?;
        check!(matches!(
            decode_payload(&bytes, &mut budget),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(charges.retained_calls.load(Ordering::SeqCst), 0);
        check_eq!(charges.live.load(Ordering::SeqCst), 0);
        check_eq!(budget.usage(), WorkUsage::default());
        Ok(())
    }

    #[test]
    fn decoded_profile_clone_keeps_the_same_charge_until_last_owner() -> Result<(), IndexError> {
        let charges = Arc::new(Charges::default());
        let mut budget = budget(charges.clone())?;
        let loaded = decode_payload(b"{\"instances\":[]}", &mut budget)?;
        let live_bytes = charges.live.load(Ordering::SeqCst);
        check!(live_bytes > 0);
        let clone = loaded.clone();
        check!(Arc::ptr_eq(&loaded.0, &clone.0));
        drop(loaded);
        check_eq!(charges.live.load(Ordering::SeqCst), live_bytes);
        drop(clone);
        check_eq!(charges.live.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn retained_refusal_and_malformed_profile_release_temporary_parse_charge()
    -> Result<(), IndexError> {
        let charges = Arc::new(Charges {
            refused: true,
            ..Charges::default()
        });
        let mut budget = budget(charges.clone())?;
        check!(matches!(
            decode_payload(b"{\"instances\":[]}", &mut budget),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(charges.live.load(Ordering::SeqCst), 0);
        for bytes in [
            b"{\"instances\":[],\"other\":1}".as_slice(),
            b"{\"instances\":[],\"instances\":[]}",
            b"{\"instances\":[]} false",
        ] {
            check!(matches!(
                decode_payload(bytes, &mut budget),
                Err(IndexError::CorruptObject(_))
            ));
            check_eq!(charges.live.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }

    #[test]
    fn bounded_profile_preserves_nonempty_gold_and_refuses_duplicate_identity_or_mixed_group()
    -> Result<(), IndexError> {
        use quanta_index_core::index::{GroupActivation, IndexInstanceId};
        let first = first_instance();
        let charges = Arc::new(Charges::default());
        let mut budget = budget(charges.clone())?;
        let gold = IndexProfile {
            instances: vec![first.clone()],
        };
        let wire = serde_json::to_vec(&gold).map_err(|error| decode_error(&error))?;
        let loaded = decode_payload(&wire, &mut budget)?;
        check_eq!(*loaded, gold);
        let clone = loaded.clone();
        check_eq!(clone.instances.as_ptr(), loaded.instances.as_ptr());
        drop(loaded);
        check!(charges.live.load(Ordering::SeqCst) > 0);
        drop(clone);
        check_eq!(charges.live.load(Ordering::SeqCst), 0);

        let mut mixed = first.clone();
        mixed.instance = IndexInstanceId([7; 16]);
        mixed.activation = GroupActivation::AsyncCoverage;
        for (second, expected) in [
            (first.clone(), "duplicate index instance"),
            (mixed, "publication group has mixed activation"),
        ] {
            let wire = serde_json::to_vec(&IndexProfile {
                instances: vec![first.clone(), second],
            })
            .map_err(|error| decode_error(&error))?;
            check!(matches!(decode_payload(&wire, &mut budget),
                Err(IndexError::InvalidInput(message)) if message == expected));
            check_eq!(charges.live.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }

    #[test]
    fn bounded_profile_decoder_preserves_canonical_map_and_sequence_shapes()
    -> Result<(), IndexError> {
        let charges = Arc::new(Charges::default());
        let mut budget = budget(charges.clone())?;
        for bytes in [b"{\"instances\":[]}".as_slice(), b"[[]]"] {
            let public: IndexProfile =
                serde_json::from_slice(bytes).map_err(|error| decode_error(&error))?;
            let loaded = decode_payload(bytes, &mut budget)?;
            check_eq!(*loaded, public);
            drop(loaded);
            check_eq!(charges.live.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }
}
