//! Application wiring.

pub mod boot_inventory;
pub mod config;
mod ipc_dispatcher;
mod legacy_semantic_migration;
pub mod runtime;
pub mod searchd;
pub mod semantic_boot;
pub mod server;

pub use boot_inventory::{BootInventoryReportV1, TrackInventoryReportV1};
pub use config::{SearchdConfig, SemanticEmbedderProfile};
pub use legacy_semantic_migration::LegacySemanticJournalStore;
pub use runtime::SearchdRuntime;
pub use searchd::drive;
pub use server::QueryServer;
