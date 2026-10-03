//! Query an already published active generation through the public SDK.
//!
//! ```sh
//! ./scripts/cargow run -p quanta-index-sdk --example query_active -- \
//!   /absolute/state-root repo-id revision-id 'search terms'
//! ```

use std::error::Error;
use std::io::{self, Write};

use quanta_index_sdk::{ConnectOptions, QuantaIndex, RepoId, RevisionId};

fn required_arg(args: &mut impl Iterator<Item = String>, name: &str) -> io::Result<String> {
    args.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("missing {name} argument"),
        )
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let state_root = required_arg(&mut args, "state-root")?;
    let repo_id = RepoId::new(required_arg(&mut args, "repo-id")?)?;
    let revision_id = RevisionId::new(required_arg(&mut args, "revision-id")?)?;
    let query = required_arg(&mut args, "query")?;
    if args.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unexpected argument").into());
    }

    let client = QuantaIndex::connect_query_only(ConnectOptions::from_state_root(state_root))?;
    let response = client
        .lexical()
        .query()
        .text(query)
        .active(repo_id, revision_id)
        .top_k(10)
        .execute()?;

    writeln!(io::stdout().lock(), "{response:#?}")?;
    Ok(())
}
