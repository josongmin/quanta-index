//! Engine-independent, pinned Legacy executable and process fixture.
//!
//! Maintained migration tests supply an already built Legacy executable.
//! This module does not compile a serving engine or grant installation authority.

#[path = "legacy_reference/binary.rs"]
mod binary;
#[path = "legacy_reference/environment.rs"]
mod environment;
#[path = "legacy_reference/launch.rs"]
mod launch;
#[path = "legacy_reference/process.rs"]
mod process;
#[path = "legacy_reference/source.rs"]
mod source;

pub(crate) use binary::RelevanceProviderInputs;
pub use binary::{LegacySearchdBinary, LegacySearchdCommand, VerifiedInstallerBinary};
#[doc(hidden)]
pub use environment::legacy_physical_env;
pub use environment::{DEFAULT_HISTORY_MAX_GENERATIONS, apply_searchd_env};
#[cfg(any(target_os = "macos", test))]
pub use process::peak_rss_from_darwin_metrics;
pub use process::{SearchdBinaryProcess, daemon_socket_paths};
pub use source::{
    legacy_source_batch, legacy_source_delta_from_chunks, legacy_source_replacement_from_chunks,
    legacy_source_replacement_from_chunks_and_symbols, publish_legacy_source,
    publish_legacy_source_batch,
};

// The Runtime test facade retains its existing current-Cargo-binary helpers.
#[doc(hidden)]
pub use process::{SOCKET_TIMEOUT, remove_socket_files, terminate_child, wait_for_sockets};

pub fn request_when_ready<T>(
    mut request: impl FnMut() -> Result<T, quanta_index_sdk::SdkError>,
) -> Result<T, quanta_index_sdk::SdkError> {
    use std::time::{Duration, Instant};

    let started = Instant::now();
    loop {
        match request() {
            Err(quanta_index_sdk::SdkError::Remote {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::NotReady,
                ..
            }) if started.elapsed() < Duration::from_secs(15) => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => return result,
        }
    }
}
