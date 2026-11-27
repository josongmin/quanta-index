//! Native process fixtures for integration tests.

mod process_entry;

fn main() -> std::process::ExitCode {
    use quanta_index_searchd_runtime::native_product;

    process_entry::run_native_process(
        native_product::run_fixture_command,
        native_product::run_fixture_command_with_capture,
    )
}
