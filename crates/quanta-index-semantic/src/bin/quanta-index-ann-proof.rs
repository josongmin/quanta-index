//! Bounded external exact-versus-served diagnostic; not a qualification gate.
use std::error::Error;
use std::io::{Read, Write};
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [input, state] = args.as_slice() else {
        return Err(
            "usage: quanta-index-ann-proof <absolute-input.json> <absolute-fresh-state>".into(),
        );
    };
    if !Path::new(input).is_absolute() {
        return Err("proof input must be absolute".into());
    }
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("source root missing")?
        .canonicalize()?;
    if Path::new(input).canonicalize()?.starts_with(source_root) {
        return Err("proof input must be outside source repository".into());
    }
    let mut bytes = Vec::new();
    let _read_bytes = std::fs::File::open(input)?
        .take(quanta_index_semantic::ann_proof::MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let output = quanta_index_semantic::ann_proof::exact_vs_served_v1(&bytes, Path::new(state))?;
    std::io::stdout().lock().write_all(&output)?;
    std::io::stdout().lock().write_all(b"\n")?;
    Ok(())
}
