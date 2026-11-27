//! The release daemon process (SEP-21 P08 / S21-09).

mod process_entry;

fn main() -> std::process::ExitCode {
    use quanta_index_searchd_runtime::native_product;

    process_entry::run_native_process(
        native_product::run_command,
        native_product::run_command_with_capture,
    )
}
