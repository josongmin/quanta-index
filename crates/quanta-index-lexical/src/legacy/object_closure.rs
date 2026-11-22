//! One bounded lexical dependency walk for ordinary and admitted existing reads.
use super::object_codec::ObjectRef;
use quanta_index_core::{CoreError, publication_capacity::publication_inventory_map_node_bytes_v1};
use std::{alloc::Layout, collections::BTreeSet};

pub(crate) const MAX_CLOSURE_REFS: usize = 4_000_000;
fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical object store: {message}"))
}
pub(crate) trait ClosureControl {
    type Error: From<CoreError>;
    type Grant;
    type Dependencies: AsRef<[ObjectRef]>;
    fn checkpoint(&mut self) -> Result<(), Self::Error>;
    fn reserve(&mut self, bytes: u64) -> Result<Self::Grant, Self::Error>;
    fn dependencies(&mut self, reference: &ObjectRef) -> Result<Self::Dependencies, Self::Error>;
    fn visit(&mut self, reference: &ObjectRef) -> Result<(), Self::Error>;
}
struct Grants<G> {
    leases: Vec<G>,
}
impl<G> Grants<G> {
    fn reserve<C: ClosureControl<Grant = G>>(
        &mut self,
        bytes: u64,
        control: &mut C,
    ) -> Result<(), C::Error> {
        let growth = if self.leases.len() == self.leases.capacity() {
            self.leases
                .capacity()
                .checked_mul(2)
                .ok_or_else(|| invalid("closure grant capacity overflow"))?
                .max(1)
        } else {
            0
        };
        let backing = u64::try_from(
            Layout::array::<G>(growth)
                .map_err(|_error| invalid("closure grant layout overflow"))?
                .size(),
        )
        .map_err(|_error| invalid("closure grant width overflow"))?;
        let lease = control.reserve(
            bytes
                .checked_add(backing)
                .ok_or_else(|| invalid("closure grant sum overflow"))?,
        )?;
        if growth > 0 {
            self.leases
                .try_reserve_exact(growth.saturating_sub(self.leases.len()))
                .map_err(|_error| invalid("reserve closure grants"))?;
        }
        self.leases.push(lease);
        Ok(())
    }
}
fn grow<T, C: ClosureControl>(
    v: &mut Vec<T>,
    extra: usize,
    limit: usize,
    grants: &mut Grants<C::Grant>,
    control: &mut C,
) -> Result<(), C::Error> {
    let needed = v
        .len()
        .checked_add(extra)
        .ok_or_else(|| invalid("closure pending work overflow"))?;
    if needed > limit {
        return Err(invalid("object closure pending work exceeds bound").into());
    }
    if needed > v.capacity() {
        let growth = v
            .capacity()
            .checked_mul(2)
            .ok_or_else(|| invalid("closure vector growth overflow"))?
            .max(needed)
            .min(limit);
        let bytes = Layout::array::<T>(growth)
            .map_err(|_error| invalid("closure vector layout overflow"))?
            .size();
        grants.reserve(
            u64::try_from(bytes).map_err(|_error| invalid("closure vector width overflow"))?,
            control,
        )?;
        v.try_reserve_exact(growth.saturating_sub(v.len()))
            .map_err(|_error| invalid("reserve closure work"))?;
    }
    Ok(())
}
pub(crate) fn walk<C: ClosureControl>(
    roots: &[ObjectRef],
    maximum: usize,
    control: &mut C,
) -> Result<(), C::Error> {
    let maximum = maximum.min(MAX_CLOSURE_REFS);
    // Collections drop before their original grants, including on every error.
    let mut grants = Grants { leases: Vec::new() };
    let mut pending = Vec::new();
    grow(
        &mut pending,
        roots.len(),
        MAX_CLOSURE_REFS,
        &mut grants,
        control,
    )?;
    pending.extend_from_slice(roots);
    let mut visited = BTreeSet::new();
    while let Some(reference) = pending.pop() {
        control.checkpoint()?;
        if visited.contains(&reference) {
            continue;
        }
        if visited.len() >= maximum {
            return Err(invalid("object closure exceeds traversal bound").into());
        }
        grants.reserve(
            publication_inventory_map_node_bytes_v1::<ObjectRef, ()>()?,
            control,
        )?;
        if !visited.insert(reference.clone()) {
            return Err(invalid("closure insertion differs from lookup").into());
        }
        let dependencies = control.dependencies(&reference)?;
        control.visit(&reference)?;
        let dependencies = dependencies.as_ref();
        grow(
            &mut pending,
            dependencies.len(),
            MAX_CLOSURE_REFS,
            &mut grants,
            control,
        )?;
        pending.extend_from_slice(dependencies);
    }
    control.checkpoint()?;
    Ok(())
}
