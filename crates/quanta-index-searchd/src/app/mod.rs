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
pub mod umask;

pub use boot_inventory::{
    BootInventoryReportV1, HalfSealedPair, SealedDirectory, SealedGenerationKey,
    TrackInventoryReportV1, half_sealed_pairs,
};
pub use config::{
    ALLOW_DEV_EMBEDDER_ENV, DEV_HASH_EMBEDDER_SELECTOR, MaintenancePolicy, ProcessMemoryCeilings,
    SearchdConfig, SemanticEmbedderProfile,
};
pub use integrity_scrub::{PacedIntegrityScrubV1, ScrubSchedulerV1, ScrubTalliesV1, ScrubTickV1};
pub use legacy_semantic_migration::LegacySemanticJournalStore;
pub use process_memory::KernelResidentMemoryProbe;
pub use runtime::{SearchdRuntime, StateRootAccessV1};
pub use searchd::drive;
pub use server::QueryServer;
pub use socket_access::{SocketAccessPolicies, SocketRole};
pub use umask::{DAEMON_UMASK, harden_umask};
