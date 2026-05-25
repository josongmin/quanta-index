use std::io::{stderr, stdout};

fn main() -> std::process::ExitCode {
    let mut stdout = stdout().lock();
    let mut stderr = stderr().lock();
    quanta_index_searchctl::run(std::env::args().skip(1), &mut stdout, &mut stderr)
}
