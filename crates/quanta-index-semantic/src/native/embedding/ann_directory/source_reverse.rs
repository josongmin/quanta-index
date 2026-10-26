//! Complete, selected-coverage source-to-projection membership.
//!
//! This is a derived search layout in the original owner-publication attempt.
//! A partial diff or failed COW apply never becomes a Catalog child.

use quanta_index_core::index::{
    AttemptContext, IndexError, InputDomain, InstanceSpec, MapEdit, MapRoot, MapValue, MemoryLease,
    MemoryLifetime, OwnerKey, WorkBudget, owner_from_map_key, owner_map_key,
    ports::{MapDiffRange, MapDiffVisitor},
    read_owner_coverage,
};

use super::super::load_contribution_from_objects;

const DIFF_PAGE: u32 = 64;
const OWNER_KEY_BYTES: usize = 38;
const REVERSE_KEY_BYTES: usize = OWNER_KEY_BYTES * 2;
const MAX_PAGE_EDITS: usize = 128;

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

/// The source prefix and complete pair share the canonical owner-key codec.
pub(super) fn key(source_owner: OwnerKey, projected_owner: OwnerKey) -> [u8; REVERSE_KEY_BYTES] {
    let mut key = [0; REVERSE_KEY_BYTES];
    key[..OWNER_KEY_BYTES].copy_from_slice(&owner_map_key(source_owner));
    key[OWNER_KEY_BYTES..].copy_from_slice(&owner_map_key(projected_owner));
    key
}

#[derive(Clone, Copy)]
struct ChangedOwner {
    key: [u8; OWNER_KEY_BYTES],
    before: Option<MapValue>,
    after: Option<MapValue>,
}

struct ChangedPage {
    rows: Vec<ChangedOwner>,
    _memory: Option<MemoryLease>,
}

impl ChangedPage {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = usize::try_from(DIFF_PAGE)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(std::mem::size_of::<ChangedOwner>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(
            usize::try_from(DIFF_PAGE).map_err(|_error| IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        if rows.capacity()
            > usize::try_from(DIFF_PAGE).map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            rows,
            _memory: memory,
        })
    }
}

impl MapDiffVisitor for ChangedPage {
    fn changed(
        &mut self,
        key: &[u8],
        before: Option<MapValue>,
        after: Option<MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let key: [u8; OWNER_KEY_BYTES] = key
            .try_into()
            .map_err(|_error| corrupt("semantic reverse owner key width differs"))?;
        let _owner = owner_from_map_key(&key)?;
        if self.rows.len()
            >= usize::try_from(DIFF_PAGE).map_err(|_error| IndexError::ResourceLimited)?
            || self.rows.last().is_some_and(|last| last.key >= key)
            || before == after
            || (before.is_none() && after.is_none())
        {
            return Err(corrupt("semantic reverse diff page is invalid"));
        }
        self.rows.push(ChangedOwner { key, before, after });
        Ok(())
    }
}

struct ReverseEdits {
    edits: Vec<MapEdit>,
    key_memory: Vec<Option<MemoryLease>>,
    _allocation_memory: Option<MemoryLease>,
}

impl ReverseEdits {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = MAX_PAGE_EDITS
            .checked_mul(
                std::mem::size_of::<MapEdit>()
                    .checked_add(std::mem::size_of::<Option<MemoryLease>>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut edits = Vec::new();
        edits
            .try_reserve_exact(MAX_PAGE_EDITS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if edits.capacity() > MAX_PAGE_EDITS {
            return Err(IndexError::ResourceLimited);
        }
        let mut key_memory = Vec::new();
        key_memory
            .try_reserve_exact(MAX_PAGE_EDITS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if key_memory.capacity() > MAX_PAGE_EDITS {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            edits,
            key_memory,
            _allocation_memory: memory,
        })
    }

    fn push(
        &mut self,
        source_owner: OwnerKey,
        projected_owner: OwnerKey,
        expected: Option<MapValue>,
        next: Option<MapValue>,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.edits.len() >= MAX_PAGE_EDITS || expected == next {
            return Err(corrupt("semantic reverse edit count or identity differs"));
        }
        let memory = budget.reserve_memory(
            u64::try_from(REVERSE_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut owned_key = Vec::new();
        owned_key
            .try_reserve_exact(REVERSE_KEY_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if owned_key.capacity() > REVERSE_KEY_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        owned_key.extend_from_slice(&key(source_owner, projected_owner));
        self.edits.push(MapEdit {
            key: owned_key,
            expected,
            next,
        });
        self.key_memory.push(memory);
        Ok(())
    }

    fn seal(&mut self) -> Result<(), IndexError> {
        self.edits
            .sort_unstable_by(|left, right| left.key.cmp(&right.key));
        if self
            .edits
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.key == right.key))
        {
            return Err(corrupt("semantic reverse edits repeat a source/owner pair"));
        }
        Ok(())
    }
}

/// Reconstruct the source key only from a selected coverage value and its
/// authenticated producer contribution.
///
/// The source map is independently checked against the pinned ticket when this
/// search hint is consumed.
fn source_owner(
    value: MapValue,
    key: &[u8; OWNER_KEY_BYTES],
    instance: &InstanceSpec,
    context: &mut AttemptContext<'_>,
) -> Result<Option<OwnerKey>, IndexError> {
    let InputDomain::Projection {
        projection,
        producer,
    } = instance.input_domain
    else {
        return Err(IndexError::BackendViolation(
            "semantic reverse index requires a projection instance".into(),
        ));
    };
    let owner = read_owner_coverage(value, key, context.objects, context.budget)?;
    if owner.desired.derivation_policy != instance.policy {
        return Err(corrupt("semantic reverse owner policy differs"));
    }
    let reference = owner
        .desired
        .projection
        .ok_or_else(|| corrupt("semantic reverse projection owner lacks contribution"))?;
    let contribution = load_contribution_from_objects(reference, context.objects, context.budget)?;
    if contribution.projection != projection
        || contribution.producer != producer
        || contribution.owner != owner.owner
        || contribution.source != owner.desired.source
        || contribution.source_owner.is_some() != contribution.source.is_some()
        || contribution.materialization_context() != owner.desired.producer_context
    {
        return Err(corrupt(
            "semantic reverse contribution differs from selected owner",
        ));
    }
    Ok(contribution.source_owner)
}

/// Update one persisted reverse root from the complete immutable owner-root
/// difference. Every page is tentative until the original publication CAS.
pub(super) fn refresh(
    reverse: MapRoot,
    before: MapRoot,
    after: MapRoot,
    instance: &InstanceSpec,
    context: &mut AttemptContext<'_>,
) -> Result<MapRoot, IndexError> {
    if !matches!(instance.input_domain, InputDomain::Projection { .. }) {
        return Err(IndexError::BackendViolation(
            "semantic reverse refresh requires projected input".into(),
        ));
    }
    context.budget.checkpoint()?;
    if before == after {
        return Ok(reverse);
    }
    let mut root = reverse;
    let mut cursor: Option<[u8; OWNER_KEY_BYTES]> = None;
    loop {
        let mut page = ChangedPage::new(context.budget)?;
        let more = context.maps.visit_diff(
            before,
            after,
            MapDiffRange {
                prefix: b"",
                after: cursor.as_ref().map(<[u8; OWNER_KEY_BYTES]>::as_slice),
                limit: DIFF_PAGE,
            },
            &mut page,
            context.budget,
        )?;
        if page.rows.is_empty() && more {
            return Err(corrupt("semantic reverse diff made no progress"));
        }
        if page
            .rows
            .first()
            .is_some_and(|row| cursor.is_some_and(|prior| row.key <= prior))
        {
            return Err(corrupt("semantic reverse diff repeated a continuation key"));
        }
        let mut edits = ReverseEdits::new(context.budget)?;
        for changed in &page.rows {
            context.budget.checkpoint()?;
            let semantic_owner = owner_from_map_key(&changed.key)?;
            let old_source = changed
                .before
                .map(|value| source_owner(value, &changed.key, instance, context))
                .transpose()?
                .flatten();
            let new_source = changed
                .after
                .map(|value| source_owner(value, &changed.key, instance, context))
                .transpose()?
                .flatten();
            match (old_source, new_source) {
                (Some(old), Some(new)) if old == new => {
                    edits.push(
                        old,
                        semantic_owner,
                        changed.before,
                        changed.after,
                        context.budget,
                    )?;
                }
                (old, new) => {
                    if let Some(source) = old {
                        edits.push(source, semantic_owner, changed.before, None, context.budget)?;
                    }
                    if let Some(source) = new {
                        edits.push(source, semantic_owner, None, changed.after, context.budget)?;
                    }
                }
            }
        }
        edits.seal()?;
        if !edits.edits.is_empty() {
            root = context.apply(root, &edits.edits)?;
        }
        cursor = page.rows.last().map(|row| row.key).or(cursor);
        if !more {
            break;
        }
    }
    context.budget.checkpoint()?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{Digest, ObjectRef, ProducerId, StableOwnerId, WorkLimits};
    use std::time::{Duration, Instant};

    #[test]
    fn reverse_key_is_source_prefix_then_canonical_projected_owner() {
        let source = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let projected = OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: StableOwnerId([4; 16]),
        };
        let pair = key(source, projected);
        assert_eq!(&pair[..OWNER_KEY_BYTES], owner_map_key(source).as_slice());
        assert_eq!(
            &pair[OWNER_KEY_BYTES..],
            owner_map_key(projected).as_slice()
        );
    }

    #[test]
    fn duplicate_reverse_pair_refuses_before_map_apply() -> Result<(), IndexError> {
        let source = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let projected = OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: StableOwnerId([4; 16]),
        };
        let budget = WorkBudget::new(
            WorkLimits {
                reads: u64::MAX,
                writes: u64::MAX,
                bytes: u64::MAX,
                mutations: u64::MAX,
            },
            Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let mut edits = ReverseEdits::new(&budget)?;
        let value = Some(MapValue::Object(ObjectRef(Digest([5; 32]))));
        edits.push(source, projected, None, value, &budget)?;
        edits.push(source, projected, None, value, &budget)?;
        if !matches!(edits.seal(), Err(IndexError::CorruptObject(_))) {
            return Err(IndexError::BackendViolation(
                "duplicate reverse edit unexpectedly sealed".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod behavior_tests;
