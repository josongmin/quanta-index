//! Cross-language conformance vectors.
//!
//! Rust defines the canonical form; the Python writer in
//! `tools/benchmark/evidence.py` must reproduce the same canonical bytes and
//! digest for the same logical document. These committed fixtures are the
//! shared oracle: the Rust test asserts its own output against them, and the
//! Python test `tools/ci/tests/test_bench_protocol_conformance.py` asserts the
//! Python writer against the same files.
//!
//! Regenerate (only when the canonical form intentionally changes):
//!
//! ```sh
//! QUANTA_BENCH_UPDATE_VECTORS=1 ./scripts/cargow --lane bench-lane test \
//!     -p quanta-index-bench-protocol --test conformance_vectors
//! ```

#![expect(
    clippy::panic_in_result_fn,
    reason = "contract tests use Result-returning setup with assertion-style validation"
)]
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use quanta_index_bench_protocol::codec::{Wire, exact_keys, field, fields, object, parse_strict};
use quanta_index_bench_protocol::sample::sample_sealed;
use quanta_index_bench_protocol::{BenchmarkEvidenceV1, ProtocolError, canonical_json};
use serde_json::Value;

/// One committed canonical-form vector: an input document and its canonical text.
struct Vector {
    name: String,
    json: Value,
    canonical: String,
}

/// The committed vector file, decoded without any serde derive.
struct VectorsFile {
    schema_version: u64,
    vectors: Vec<Vector>,
}

impl Wire for Vector {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(object(vec![
            ("name", Value::String(self.name.clone())),
            ("json", self.json.clone()),
            ("canonical", Value::String(self.canonical.clone())),
        ]))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        let table = fields(value, "vector")?;
        exact_keys(table, &["name", "json", "canonical"], "vector")?;
        Ok(Self {
            name: String::decode(field(table, "name", "vector")?)?,
            json: field(table, "json", "vector")?.clone(),
            canonical: String::decode(field(table, "canonical", "vector")?)?,
        })
    }
}

impl Wire for VectorsFile {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(object(vec![
            ("schema_version", Value::from(self.schema_version)),
            ("vectors", self.vectors.encode()?),
        ]))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        let table = fields(value, "vectors file")?;
        exact_keys(table, &["schema_version", "vectors"], "vectors file")?;
        Ok(Self {
            schema_version: u64::decode(field(table, "schema_version", "vectors file")?)?,
            vectors: Vec::<Vector>::decode(field(table, "vectors", "vectors file")?)?,
        })
    }
}

fn read_vectors(text: &str) -> Result<VectorsFile, ProtocolError> {
    VectorsFile::decode(&parse_strict(text)?)
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn update_enabled() -> bool {
    std::env::var("QUANTA_BENCH_UPDATE_VECTORS").is_ok()
}

fn vector_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("empty_object", "{}"),
        ("key_order", "{\"b\":1,\"a\":2}"),
        ("nested", "{\"a\":{\"z\":[1,2,3],\"y\":{\"k\":\"v\"}}}"),
        ("scalars", "{\"t\":true,\"f\":false,\"n\":null}"),
        (
            "strings",
            "{\"s\":\"quote\\\" backslash\\\\ newline\\ntab\\tctrl\\u0001\"}",
        ),
        (
            "unicode",
            "{\"u\":\"h\\u00e9llo w\\u00f6rld \\u03b1\\u03b2\\u03b3 \\ud55c\\uae00\",\"emoji\":\"\\ud83e\\uddea\"}",
        ),
        ("integers", "{\"i\":[0,1,-1,1000000]}"),
        ("floats", "{\"f\":[0.5,0.42,1.0,1234.5678]}"),
        ("small_floats", "{\"s\":[0.0001,0.001,0.00001]}"),
        ("large_floats", "{\"l\":[1000000000000000.0,1e16,1.5e20]}"),
        (
            "retrieval_like",
            "{\"rows\":[{\"case_id\":\"lexical.keyword.native\",\"p50\":0.42}],\"errors\":0}",
        ),
        (
            "digest_like",
            "{\"digest\":\"sha256:0000000000000000000000000000000000000000000000000000000000000000\"}",
        ),
    ]
}

#[test]
fn canonical_vectors_match_the_committed_oracle() -> Result<(), Box<dyn Error>> {
    let directory = fixtures_dir();
    fs::create_dir_all(&directory)?;
    let path = directory.join("canonical-json-vectors.json");

    let mut vectors: Vec<Vector> = Vec::new();
    for (name, text) in vector_sources() {
        let value: Value = serde_json::from_str(text)?;
        vectors.push(Vector {
            name: name.to_owned(),
            canonical: canonical_json(&value),
            json: value,
        });
    }
    let file = VectorsFile {
        schema_version: 1,
        vectors,
    };

    if update_enabled() {
        let pretty = serde_json::to_string_pretty(&file.encode()?)?;
        fs::write(&path, format!("{pretty}\n"))?;
    }

    let committed = read_vectors(&fs::read_to_string(&path)?)?;
    assert_eq!(committed.schema_version, 1);
    assert_eq!(committed.vectors.len(), file.vectors.len());
    for (index, vector) in committed.vectors.iter().enumerate() {
        let Some(expected) = file.vectors.get(index) else {
            return Err("committed vector list changed".into());
        };
        assert_eq!(vector.name, expected.name);
        assert_eq!(
            vector.canonical,
            canonical_json(&vector.json),
            "canonical form drifted for vector {:?}",
            vector.name
        );
    }
    Ok(())
}

#[test]
fn sealed_sample_matches_the_committed_golden() -> Result<(), Box<dyn Error>> {
    let directory = fixtures_dir();
    fs::create_dir_all(&directory)?;
    let path = directory.join("sample-evidence.json");

    let sealed = sample_sealed()?;
    let canonical = sealed.to_canonical_json()?;
    if update_enabled() {
        fs::write(&path, format!("{canonical}\n"))?;
    }
    let golden = fs::read_to_string(&path)?;
    assert_eq!(golden.trim_end(), canonical);

    let reopened = BenchmarkEvidenceV1::open(&golden)?;
    assert_eq!(reopened.digest, sealed.digest);
    assert_eq!(reopened, sealed);
    Ok(())
}
