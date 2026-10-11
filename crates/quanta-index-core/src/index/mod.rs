//! Native incremental application contracts. No backend or runtime dependency.
mod attempt;
pub mod budget;
pub mod effort;
mod mutations;
pub mod ports;
mod records;
mod work;
mod work_cursor;
pub use budget::{IndexError, WorkBudget, WorkLimits, WorkUsage};
pub use mutations::{OwnerMutationCursor, OwnerMutationDigest, SourceChangeCursor};
pub use ports::AttemptContext;
pub use quanta_index_contract::index::*;
pub use records::{
    AdmissionOutcome, BaseMember, DeltaInput, MapBatchRequest, MapBatchWrite, MapEdit, MapPage,
    MapWrite, ObjectBatchInput, OrdinalMapEdit, PlanContext, PlanDecision, PreparedPublication,
    RegisteredObject, SourceChange, SourceDelta, StoredObject, TicketRecord, owner_map_key,
};
pub use work::{WorkAction, WorkActionDigest, WorkActionSet};
pub use work_cursor::WorkActionManifestCursor;
