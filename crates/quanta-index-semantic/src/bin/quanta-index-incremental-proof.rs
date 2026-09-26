//! Machine-readable owner proof entry point.
use std::error::Error;
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [plan, fresh_state] = args.as_slice() else {
        return Err("usage: quanta-index-incremental-proof <plan.json> <fresh-state-root>".into());
    };
    let payload = quanta_index_semantic::proof::incremental_proof_v1(
        &std::fs::read(plan)?,
        Path::new(fresh_state),
    )?;
    serde_json::to_writer(std::io::stdout().lock(), &payload)?;
    std::io::stdout().lock().write_all(b"\n")?;
    Ok(())
}
