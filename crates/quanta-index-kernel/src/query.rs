//! Typed reader composition. The pinned profile chooses the descriptor;
//! clients cannot supply schema, format, policy or an execution reservation.

use std::{collections::BTreeMap, sync::Arc};

use quanta_index_core::index::{
    IndexError, IndexKind, IndexQueryPort, InstanceSpec, effort::ResourceNeed,
};

use crate::AdapterDescriptor;

type SharedQueryPort<Q, R> = Arc<dyn IndexQueryPort<Q, R>>;
type ResolvedQueryReader<Q, R> = (SharedQueryPort<Q, R>, ResourceNeed);

struct Reader<Q, R> {
    descriptor: AdapterDescriptor,
    reservation_ceiling: ResourceNeed,
    port: SharedQueryPort<Q, R>,
}

pub struct QueryAdapterRegistry<Q, R> {
    by_kind: BTreeMap<IndexKind, Reader<Q, R>>,
}

impl<Q, R> Default for QueryAdapterRegistry<Q, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Q, R> QueryAdapterRegistry<Q, R> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_kind: BTreeMap::new(),
        }
    }

    /// The registered ceiling bounds request-specific sizing policies. The
    /// selected request owns an exact need no larger than this ceiling.
    pub fn register<A: IndexQueryPort<Q, R> + 'static>(
        &mut self,
        descriptor: AdapterDescriptor,
        reservation_ceiling: ResourceNeed,
        port: A,
    ) -> Result<(), IndexError> {
        if !valid_need(reservation_ceiling) {
            return Err(IndexError::InvalidInput(
                "query reservation must declare CPU, IO and memory; the request owns its deadline"
                    .into(),
            ));
        }
        match self.by_kind.entry(descriptor.kind) {
            std::collections::btree_map::Entry::Occupied(_) => Err(IndexError::InvalidInput(
                "duplicate query reader kind".into(),
            )),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let _registered = entry.insert(Reader {
                    descriptor,
                    reservation_ceiling,
                    port: Arc::new(port),
                });
                Ok(())
            }
        }
    }

    /// Check each reader against the service's Interactive envelope. Execution
    /// reserves the selected reader's exact declaration, not this ceiling.
    pub fn validate_reservation(&self, admitted: ResourceNeed) -> Result<(), IndexError> {
        if self
            .by_kind
            .values()
            .any(|reader| !reader.reservation_ceiling.fits(admitted))
        {
            return Err(IndexError::InvalidInput(
                "query reader reservation exceeds the service class".into(),
            ));
        }
        Ok(())
    }

    /// An absent kind is an optional-stage miss. A present but incompatible
    /// reader is an error and must never be reported as unavailable capacity.
    pub(crate) fn resolve_optional(
        &self,
        instance: &InstanceSpec,
    ) -> Result<Option<ResolvedQueryReader<Q, R>>, IndexError> {
        let Some(reader) = self.by_kind.get(&instance.kind) else {
            return Ok(None);
        };
        if reader.descriptor.schema != instance.schema
            || reader.descriptor.format != instance.format
        {
            return Err(IndexError::SnapshotUnavailable);
        }
        reader.port.validate_instance(instance)?;
        Ok(Some((Arc::clone(&reader.port), reader.reservation_ceiling)))
    }
}

fn valid_need(need: ResourceNeed) -> bool {
    need.cpu_units > 0 && need.io_bytes > 0 && need.memory_bytes > 0
}

pub(crate) fn validate_request_need(
    actual: ResourceNeed,
    ceiling: ResourceNeed,
) -> Result<(), IndexError> {
    if !valid_need(actual) || !actual.fits(ceiling) {
        return Err(IndexError::BackendViolation(
            "query capability reservation is invalid or exceeds its registered ceiling".into(),
        ));
    }
    Ok(())
}
