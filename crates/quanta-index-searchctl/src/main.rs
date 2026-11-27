use std::io::{stderr, stdin, stdout};
use std::process::ExitCode;

mod runtime_catalog_digest;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    if args
        .next_if(|arg| arg == "verify-runtime-catalog-digest")
        .is_some()
    {
        if args.next().is_some() {
            eprintln!("verify-runtime-catalog-digest accepts a facts record on stdin");
            return ExitCode::from(2);
        }
        return match runtime_catalog_digest::verify_facts_record(stdin().lock()) {
            Ok(digest) => {
                println!("{digest}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let mut stdout = stdout().lock();
    let mut stderr = stderr().lock();
    quanta_index_searchctl::run(args, &mut stdout, &mut stderr)
}
