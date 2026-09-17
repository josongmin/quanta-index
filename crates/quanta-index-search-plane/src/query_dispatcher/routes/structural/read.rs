//! The structural read one query is pinned to (QI-BB-020 W2).

use quanta_index_contract::{AuxEpochV1, GenerationPin};

use crate::readiness::StructuralAuthorityState;

/// The generation and the structural authority snapshot every leaf of one
/// structural query evaluates against.
///
/// That is the pinned universe, each parse-tree match the producer
/// executes at `epoch`, and each symbol projection over `state`. Taken
/// once at route entry so a mutation that lands mid-query cannot split
/// the query across two snapshots.
#[derive(Clone, Copy)]
pub(super) struct StructuralRead<'a> {
    pub(super) pin: &'a GenerationPin,
    pub(super) epoch: AuxEpochV1,
    pub(super) state: &'a StructuralAuthorityState,
}
