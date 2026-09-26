//! Local-only full-vector export through the production encoder contract.
use std::error::Error;
use std::io::Write;
use std::path::Path;

use quanta_index_core::{L2UnitEmbeddingProvider, TextEmbeddingProvider};
use quanta_index_embed::PotionCodeEmbeddingProvider;

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [model_dir, inputs_path] = args.as_slice() else {
        return Err("usage: quanta-index-vector-proof <absolute-model-dir> <inputs.json>".into());
    };
    if !Path::new(model_dir).is_absolute() {
        return Err("vector proof model directory must be absolute".into());
    }
    let input_bytes = std::fs::read(inputs_path)?;
    if input_bytes.len() > 32 * 1024 * 1024 {
        return Err("vector proof inputs exceed 32 MiB".into());
    }
    let inputs: Vec<String> = serde_json::from_slice(&input_bytes)?;
    if inputs.is_empty() || inputs.len() > 4096 {
        return Err("vector proof requires 1..4096 input texts".into());
    }
    let provider = L2UnitEmbeddingProvider::new(PotionCodeEmbeddingProvider::from_local_dir(
        Path::new(model_dir),
    )?)?;
    let texts = inputs.iter().map(String::as_str).collect::<Vec<_>>();
    let vectors = provider.embed_batch(&texts)?;
    let reverse = texts.iter().copied().rev().collect::<Vec<_>>();
    let reversed_vectors = provider.embed_batch(&reverse)?;
    let payload = serde_json::json!({
        "schema_version": 1,
        "model_id": provider.model_id(),
        "model_revision": provider.model_revision(),
        "dimension": provider.dimension(),
        "normalization": "l2_unit",
        "max_length": null,
        "inputs": inputs,
        "vectors": vectors,
        "reversed_vectors": reversed_vectors,
    });
    serde_json::to_writer(std::io::stdout().lock(), &payload)?;
    std::io::stdout().lock().write_all(b"\n")?;
    Ok(())
}
