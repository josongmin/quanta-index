//! Emit diagnostic records directly from the linked grammar without projection.

use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("expected repo-relative path")?;
    let mut source = Vec::new();
    let _ = std::io::stdin()
        .take((1 << 20) + 1)
        .read_to_end(&mut source)?;
    if source.len() > 1 << 20 {
        return Err("source exceeds 1 MiB".into());
    }
    let source = String::from_utf8(source)?;
    let records = quanta_index_retrieval_bench::symbols::extract_symbols(&path, &source)?;
    serde_json::to_writer(std::io::stdout(), &records)?;
    Ok(())
}
