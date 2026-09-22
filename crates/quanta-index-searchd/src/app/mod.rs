//! Application wiring.

pub mod boot_inventory;
pub mod config;
pub mod integrity_scrub;
mod ipc_dispatcher;
mod legacy_semantic_migration;
pub mod maintenance;
pub mod process_memory;
pub mod runtime;
pub mod searchd;
pub mod semantic_boot;
pub mod server;
pub mod socket_access;
pub mod state_format;
pub mod state_migration;
pub mod supervisor;
pub mod umask;

pub use boot_inventory::{
    BootInventoryReportV1, HalfSealedPair, InterruptedReclaimsAtBoot, SealedDirectory,
    SealedGenerationKey, TrackInventoryReportV1, half_sealed_pairs,
};
pub use config::{
    DEV_HASH_EMBEDDER_SELECTOR, MaintenancePolicy, ProcessMemoryCeilings, SearchdConfig,
    SemanticEmbedderProfile,
};
pub use integrity_scrub::{PacedIntegrityScrubV1, ScrubSchedulerV1, ScrubTalliesV1, ScrubTickV1};
pub use legacy_semantic_migration::LegacySemanticJournalStore;
pub use process_memory::KernelResidentMemoryProbe;
pub use runtime::{RuntimeGuards, RuntimeServers};
pub use runtime::{SearchdRuntime, StateRootAccessV1};
pub use searchd::{drive, supervise_runtime};
pub use server::QueryServer;
pub use socket_access::{SocketAccessPolicies, SocketRole};
pub use supervisor::{
    CancelRoot, ChildContext, ChildExit, ChildExitKind, ChildSpawnFailure,
    DEFAULT_COOPERATIVE_DRAIN_DEADLINE, HARD_DRAIN_DEADLINE, SearchdSupervisor, SupervisionError,
    SupervisionOutcome,
};
pub use umask::{DAEMON_UMASK, harden_umask};
