//! Query a producer-selected Native view through the ordinary public SDK.
//!
//! `binding.cbor` contains the caller's `(ViewSelection, InstanceSpec, [u8; 32])`.
//! Supply the actual selected workspace/view and configured Text instance, plus
//! the request owner's stable retry key. Do not derive these from Legacy IDs.
//!
//! ```sh
//! ./scripts/cargow run -p quanta-index-sdk --example query_active -- \
//!   /absolute/native.sock binding.cbor 'search terms'
//! ```

use std::error::Error;
use std::io::{self, Read, Write};
use std::time::Duration;

use quanta_index_contract::{
    QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
    index::{InstanceSpec, ViewSelection},
};
use quanta_index_ipc::RequestBudgetV1;
use quanta_index_sdk::{ConnectOptions, QuantaIndex};

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
    let socket = required_arg(&mut args, "native-socket")?;
    let binding_path = required_arg(&mut args, "binding.cbor")?;
    let query_text = required_arg(&mut args, "query")?;
    if args.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unexpected argument").into());
    }
    let mut binding = std::fs::File::open(binding_path)?;
    let (selection, instance, open_key): (ViewSelection, InstanceSpec, [u8; 32]) =
        ciborium::de::from_reader(&mut binding)?;
    let mut trailing = [0_u8; 1];
    if binding.read(&mut trailing)? != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "trailing binding bytes").into());
    }
    let client =
        QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(socket, selection))?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let mut session = client.reader().native()?.open(open_key, &budget)?;
    let result = (|| {
        let _captured_profile = session.load_profile(&budget)?;
        session.text(
            &instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text,
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 10,
                cursor: None,
            },
            &budget,
        )
    })();
    // Keep the acknowledged identity on an uncertain release; opening Latest
    // again would hide the original lifecycle rather than reconcile it.
    let release = session.release(&budget);
    if let Err(error) = &release {
        writeln!(
            io::stderr().lock(),
            "release pending: {:?}: {error}",
            session.status()
        )?;
    }
    let response = result?;
    let _has_remaining_target_pins = release?;
    writeln!(io::stdout().lock(), "{response:#?}")?;
    Ok(())
}
