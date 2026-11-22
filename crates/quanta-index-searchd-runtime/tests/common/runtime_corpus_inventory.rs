use std::path::Path;

use quanta_index_corpus_smoke::{Corpus, load_corpus};
use sha2::{Digest as _, Sha256};

// These reviewed literals bind the fixture independently of the inventory.
const RUNTIME_ROW_COUNT: usize = 242;
const RUNTIME_ROW_IDS_SHA256: &str =
    "46bbb4517f77f6954cdf7720e76872f6b082c5a249f60cd091efda3f922477e4";

pub(super) fn load_pinned_runtime_rows() -> anyhow::Result<Corpus> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lexical_corpus/runtime_rows.toml");
    let corpus = load_corpus(&path)?;
    anyhow::ensure!(
        corpus.rows.len() == RUNTIME_ROW_COUNT,
        "runtime corpus row count changed: expected {RUNTIME_ROW_COUNT}, got {}",
        corpus.rows.len()
    );
    let mut ids: Vec<&str> = corpus.rows.iter().map(|row| row.id.as_str()).collect();
    ids.sort_unstable();
    let mut input = ids.join("\n");
    input.push('\n');
    let digest = format!("{:x}", Sha256::digest(input.as_bytes()));
    anyhow::ensure!(
        digest == RUNTIME_ROW_IDS_SHA256,
        "runtime corpus row IDs changed: expected {RUNTIME_ROW_IDS_SHA256}, got {digest}"
    );
    Ok(corpus)
}
