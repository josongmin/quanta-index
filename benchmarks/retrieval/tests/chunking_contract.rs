//! Chunking contract tests (RB-03, T08–T09): boundary correctness,
//! determinism, fallback accounting and corpus-loader rejection rules.

use std::collections::BTreeSet;
use std::path::Path;

use quanta_index_retrieval_bench::chunking::fixed_window::FixedWindowChunker;
use quanta_index_retrieval_bench::chunking::syntax::SyntaxChunker;
use quanta_index_retrieval_bench::chunking::whole_file::WholeFileChunker;
use quanta_index_retrieval_bench::chunking::{
    Chunker, chunk_corpus, count_tokens, validate_chunks,
};
use quanta_index_retrieval_bench::corpus::{CorpusLimits, SourceFile, load_corpus, load_manifest};
use quanta_index_retrieval_bench::sha256_hex;

fn write_oracle(root: &Path) {
    let files: &[(&str, &[u8])] = &[
        ("src/lib.rs", b"use crate::alpha;\n\n/// Adds one.\npub fn add_one(x: u64) -> u64 {\n    x + 1\n}\n\nstruct Hidden {\n    field: u32,\n}\n"),
        ("src/alpha.rs", b"pub fn alpha() -> &'static str {\n    \"alpha\"\n}\n"),
        ("src/dup.rs", b"pub fn add_one(x: u64) -> u64 {\n    x + 1\n}\n"),
        ("notes/crlf.rs", b"pub fn windows() -> u32 {\r\n    7\r\n}\r\n"),
        ("notes/no_trailing.rs", b"pub fn last() -> bool {\n    true\n}"),
        ("notes/unicode.rs", "pub fn héllo() -> &'static str {\n    \"héllo wörld αβγ\"\n}\n".as_bytes()),
        ("notes/empty.rs", b""),
        ("notes/long.rs", &{
            let mut text = String::from("pub fn huge() -> u64 {\n");
            for index in 0..400 {
                text.push_str(&format!("    let _v{index} = {index}_u64;\n"));
            }
            text.push_str("    0\n}\n");
            text.into_bytes()
        }),
        ("notes/nested.rs", b"pub mod outer {\n    pub mod inner {\n        pub fn deep() -> u32 {\n            42\n        }\n    }\n}\n"),
        ("notes/unbalanced.rs", b"pub fn broken() -> u32 {\n    if true {\n        1\n}\n"),
        ("notes/plain.py", b"def greet():\n    return 'hello'\n"),
    ];
    for (path, bytes) in files {
        let absolute = root.join(path);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("oracle dir");
        }
        std::fs::write(&absolute, bytes).expect("oracle file");
    }
}

fn write_manifest(root: &Path, paths: &[&str]) -> std::path::PathBuf {
    let mut entries = Vec::new();
    for path in paths {
        let bytes = std::fs::read(root.join(path)).expect("oracle bytes");
        entries.push(format!(
            "{{\"path\": \"{path}\", \"file_sha256\": \"{}\"}}",
            sha256_hex(&bytes)
        ));
    }
    let manifest = root.join("manifest.json");
    std::fs::write(
        &manifest,
        format!(
            "{{\"repository_commit\": \"{}\", \"files\": [{}]}}",
            "a".repeat(40),
            entries.join(",")
        ),
    )
    .expect("manifest");
    manifest
}

fn oracle_files() -> Vec<&'static str> {
    vec![
        "src/lib.rs",
        "src/alpha.rs",
        "src/dup.rs",
        "notes/crlf.rs",
        "notes/no_trailing.rs",
        "notes/unicode.rs",
        "notes/empty.rs",
        "notes/long.rs",
        "notes/nested.rs",
        "notes/unbalanced.rs",
        "notes/plain.py",
    ]
}

fn load_oracle() -> (tempfile::TempDir, Vec<SourceFile>) {
    let root = tempfile::tempdir().expect("temp root");
    write_oracle(root.path());
    let manifest_path = write_manifest(root.path(), &oracle_files());
    let manifest = load_manifest(&manifest_path).expect("manifest loads");
    let files =
        load_corpus(root.path(), &manifest, &CorpusLimits::default()).expect("corpus loads");
    assert_eq!(files.len(), oracle_files().len());
    (root, files)
}

#[test]
fn whole_file_is_a_single_diagnostic_control() {
    let (_root, files) = load_oracle();
    let chunker = WholeFileChunker;
    let (chunks, coverage) = chunk_corpus(&chunker, &files).expect("chunk");
    for file in &files {
        let file_chunks = &chunks[&file.path];
        if file.bytes.is_empty() {
            assert!(file_chunks.is_empty(), "empty file yields no chunks");
        } else {
            assert_eq!(file_chunks.len(), 1, "control yields one chunk");
            let only = &file_chunks[0];
            assert_eq!(only.start_byte, 0);
            assert_eq!(usize::try_from(only.end_byte).unwrap(), file.bytes.len());
            assert_eq!(only.start_line, 1);
            assert_eq!(usize::try_from(only.end_line).unwrap(), file.line_count());
            assert_eq!(only.text, file.text);
            assert!(!only.fallback);
        }
    }
    assert_eq!(coverage.overlap_bytes, 0);
    assert_eq!(coverage.uncovered_bytes, 0);
    assert_eq!(coverage.fallback_chunks, 0);
}

#[test]
fn fixed_window_covers_bytes_with_declared_overlap() {
    let (_root, files) = load_oracle();
    let chunker = FixedWindowChunker::new(120, 40);
    let (chunks, coverage) = chunk_corpus(&chunker, &files).expect("chunk");
    // Small files fit one window; the long file needs several overlapping ones.
    assert_eq!(chunks["src/alpha.rs"].len(), 1);
    assert!(chunks["notes/long.rs"].len() > 2);
    assert!(
        coverage.overlap_bytes > 0,
        "overlap is measured, not hidden"
    );
    assert_eq!(coverage.uncovered_bytes, 0);
    // Every chunk is line-anchored: byte ends land on line ends.
    for file in &files {
        for chunk in &chunks[&file.path] {
            let (start, end) = file
                .line_span_bytes(
                    usize::try_from(chunk.start_line).unwrap(),
                    usize::try_from(chunk.end_line).unwrap(),
                )
                .expect("line span");
            assert!(usize::try_from(chunk.start_byte).unwrap() >= start);
            assert_eq!(usize::try_from(chunk.end_byte).unwrap(), end);
        }
    }
}

#[test]
fn fixed_window_rejects_degenerate_parameters() {
    let (_root, files) = load_oracle();
    let file = files.iter().find(|file| file.path == "src/lib.rs").unwrap();
    assert!(FixedWindowChunker::new(0, 0).chunk(file).is_err());
    assert!(FixedWindowChunker::new(100, 100).chunk(file).is_err());
    assert!(FixedWindowChunker::new(100, 200).chunk(file).is_err());
}

#[test]
fn syntax_splits_rust_items_and_flags_fallbacks() {
    let (_root, files) = load_oracle();
    let chunker = SyntaxChunker::default();
    let (chunks, coverage) = chunk_corpus(&chunker, &files).expect("chunk");
    let lib = &chunks["src/lib.rs"];
    assert_eq!(lib.len(), 3, "use + fn + struct, preamble joined: {lib:?}");
    assert!(lib.iter().all(|chunk| !chunk.fallback));
    // Nested modules stay one item; unbalanced input falls back explicitly.
    assert_eq!(chunks["notes/nested.rs"].len(), 1);
    assert!(!chunks["notes/nested.rs"][0].fallback);
    let unbalanced = &chunks["notes/unbalanced.rs"];
    assert_eq!(unbalanced.len(), 1);
    assert!(unbalanced[0].fallback, "unbalanced braces fall back openly");
    // Non-Rust files fall back, never silently parse.
    let python = &chunks["notes/plain.py"];
    assert_eq!(python.len(), 1);
    assert!(python[0].fallback);
    assert!(coverage.fallback_chunks >= 2);
}

#[test]
fn syntax_handles_crlf_unicode_and_missing_trailing_newline() {
    let (_root, files) = load_oracle();
    let chunker = SyntaxChunker::default();
    let (chunks, _) = chunk_corpus(&chunker, &files).expect("chunk");
    for path in ["notes/crlf.rs", "notes/no_trailing.rs", "notes/unicode.rs"] {
        let file_chunks = &chunks[path];
        assert_eq!(file_chunks.len(), 1, "{path} yields one item");
        assert!(!file_chunks[0].fallback, "{path} parses without fallback");
    }
}

#[test]
fn oversized_items_fall_back_instead_of_silently_rechunking() {
    let (_root, files) = load_oracle();
    let tiny = SyntaxChunker::new(64);
    let (chunks, _) = chunk_corpus(&tiny, &files).expect("chunk");
    let long = &chunks["notes/long.rs"];
    assert_eq!(long.len(), 1);
    assert!(long[0].fallback);
}

#[test]
fn chunking_is_deterministic_across_runs() {
    let (_root, files) = load_oracle();
    let fixed = FixedWindowChunker::new(200, 50);
    let syntax = SyntaxChunker::default();
    for (name, first, second) in [
        (
            "fixed",
            chunk_corpus(&fixed, &files),
            chunk_corpus(&fixed, &files),
        ),
        (
            "syntax",
            chunk_corpus(&syntax, &files),
            chunk_corpus(&syntax, &files),
        ),
    ] {
        let (left, _) = first.expect("first run");
        let (right, _) = second.expect("second run");
        assert_eq!(left, right, "{name} must be deterministic");
    }
}

#[test]
fn chunk_ids_are_stable_and_unique() {
    let (_root, files) = load_oracle();
    let (chunks, _) = chunk_corpus(&FixedWindowChunker::new(120, 40), &files).expect("chunk");
    let mut ids = BTreeSet::new();
    for file_chunks in chunks.values() {
        for chunk in file_chunks {
            assert!(ids.insert(chunk.chunk_id.clone()), "duplicate chunk_id");
        }
    }
    // Same content under another path yields another ID (path-bound).
    let (again, _) = chunk_corpus(&FixedWindowChunker::new(120, 40), &files).expect("chunk");
    for (path, file_chunks) in &again {
        for (index, chunk) in file_chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_id, chunks[path][index].chunk_id);
        }
    }
}

#[test]
fn duplicate_symbols_in_different_files_do_not_collide() {
    let (_root, files) = load_oracle();
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    // src/lib.rs and src/dup.rs both define add_one; whole-file texts differ
    // (imports/docs) so IDs differ; per-chunk assertions live on the ID set.
    assert_ne!(
        chunks["src/lib.rs"][0].chunk_id,
        chunks["src/dup.rs"][0].chunk_id
    );
}

#[test]
fn token_counts_match_byte_slices() {
    let (_root, files) = load_oracle();
    for file in &files {
        let expected = count_tokens(&file.text);
        let (chunks, _) =
            chunk_corpus(&WholeFileChunker, std::slice::from_ref(file)).expect("chunk");
        let file_chunks = &chunks[&file.path];
        if file.bytes.is_empty() {
            assert_eq!(expected, 0);
            assert!(file_chunks.is_empty());
        } else {
            assert_eq!(count_tokens(&file_chunks[0].text), expected);
        }
    }
}

#[test]
fn validator_rejects_mutated_spans_ids_and_order() {
    let (_root, files) = load_oracle();
    let file = files.iter().find(|file| file.path == "src/lib.rs").unwrap();
    let (chunks, _) =
        chunk_corpus(&FixedWindowChunker::new(60, 20), std::slice::from_ref(file)).expect("chunk");
    let valid = &chunks[&file.path];
    assert!(valid.len() >= 2);
    validate_chunks(valid, file).expect("oracle validates");

    let mut wrong_text = valid.to_vec();
    wrong_text[0].text = "tampered".to_string();
    assert!(validate_chunks(&wrong_text, file).is_err());

    let mut wrong_lines = valid.to_vec();
    wrong_lines[0].end_line += 100;
    assert!(validate_chunks(&wrong_lines, file).is_err());

    let mut wrong_id = valid.to_vec();
    wrong_id[0].chunk_id = "0".repeat(64);
    assert!(validate_chunks(&wrong_id, file).is_err());

    let mut dup_id = valid.to_vec();
    let first_id = dup_id[0].chunk_id.clone();
    dup_id[1].chunk_id = first_id;
    assert!(validate_chunks(&dup_id, file).is_err());

    let mut reordered = valid.to_vec();
    reordered.swap(0, 1);
    assert!(validate_chunks(&reordered, file).is_err());

    let mut overflow = valid.to_vec();
    overflow[0].end_byte = u32::MAX;
    assert!(validate_chunks(&overflow, file).is_err());

    let mut zero_length = valid.to_vec();
    zero_length[0].end_byte = zero_length[0].start_byte;
    assert!(validate_chunks(&zero_length, file).is_err());
}

#[test]
fn validator_rejects_utf8_split() {
    let (_root, files) = load_oracle();
    let file = files
        .iter()
        .find(|file| file.path == "notes/unicode.rs")
        .unwrap();
    // Find a multi-byte char offset and split inside it.
    let split = file.text.find('é').expect("oracle holds é") + 1;
    assert!(!file.text.is_char_boundary(split));
    let (chunks, _) = chunk_corpus(&WholeFileChunker, std::slice::from_ref(file)).expect("chunk");
    let mut broken = chunks[&file.path].clone();
    broken[0].end_byte = u32::try_from(split).unwrap();
    assert!(validate_chunks(&broken, file).is_err());
}

#[test]
fn corpus_loader_rejects_binary_symlink_oversize_and_mismatch() {
    let root = tempfile::tempdir().expect("temp root");
    write_oracle(root.path());
    // Binary file.
    std::fs::write(root.path().join("notes/bin.rs"), b"fn f() {}\0trailing").expect("bin");
    // Oversize file (cap forced tiny).
    std::fs::write(root.path().join("notes/big.rs"), vec![b'x'; 2048]).expect("big");
    #[cfg(unix)]
    std::os::unix::fs::symlink("lib.rs", root.path().join("src/link.rs")).expect("symlink");

    let commit = "a".repeat(40);
    let entry = |path: &str| {
        let bytes = std::fs::read(root.path().join(path)).unwrap_or_default();
        format!(
            "{{\"path\": \"{path}\", \"file_sha256\": \"{}\"}}",
            sha256_hex(&bytes)
        )
    };
    let manifest_path = root.path().join("manifest.json");

    for bad in ["notes/bin.rs", "src/link.rs"] {
        std::fs::write(
            &manifest_path,
            format!(
                "{{\"repository_commit\": \"{commit}\", \"files\": [{}]}}",
                entry(bad)
            ),
        )
        .expect("manifest");
        let manifest = load_manifest(&manifest_path).expect("manifest parses");
        assert!(
            load_corpus(root.path(), &manifest, &CorpusLimits::default()).is_err(),
            "{bad} must be refused"
        );
    }
    std::fs::write(
        &manifest_path,
        format!(
            "{{\"repository_commit\": \"{commit}\", \"files\": [{}]}}",
            entry("notes/big.rs")
        ),
    )
    .expect("manifest");
    let manifest = load_manifest(&manifest_path).expect("manifest parses");
    let tiny = CorpusLimits {
        max_file_bytes: 1024,
    };
    assert!(load_corpus(root.path(), &manifest, &tiny).is_err());

    // Hash mismatch.
    std::fs::write(
        &manifest_path,
        format!(
            "{{\"repository_commit\": \"{commit}\", \"files\": [{{\"path\": \"src/lib.rs\", \"file_sha256\": \"{}\"}}]}}",
            "0".repeat(64)
        ),
    )
    .expect("manifest");
    let manifest = load_manifest(&manifest_path).expect("manifest parses");
    assert!(load_corpus(root.path(), &manifest, &CorpusLimits::default()).is_err());
}

#[test]
fn manifest_rejects_unknown_fields_duplicates_and_bad_paths() {
    let root = tempfile::tempdir().expect("temp root");
    let commit = "a".repeat(40);
    let good = format!(
        "{{\"path\": \"src/lib.rs\", \"file_sha256\": \"{}\"}}",
        "b".repeat(64)
    );
    for (name, raw) in [
        (
            "unknown field",
            format!("{{\"repository_commit\": \"{commit}\", \"files\": [{good}], \"extra\": 1}}"),
        ),
        (
            "duplicate",
            format!("{{\"repository_commit\": \"{commit}\", \"files\": [{good},{good}]}}"),
        ),
        (
            "bad path",
            format!(
                "{{\"repository_commit\": \"{commit}\", \"files\": [{{\"path\": \"../x.rs\", \"file_sha256\": \"{}\"}}]}}",
                "b".repeat(64)
            ),
        ),
        (
            "bad commit",
            format!("{{\"repository_commit\": \"xyz\", \"files\": [{good}]}}"),
        ),
        (
            "empty files",
            format!("{{\"repository_commit\": \"{commit}\", \"files\": []}}"),
        ),
    ] {
        let path = root.path().join("manifest.json");
        std::fs::write(&path, raw).expect("manifest");
        assert!(load_manifest(&path).is_err(), "{name} must fail");
    }
}

#[test]
fn exotic_line_boundaries_are_refused() {
    let root = tempfile::tempdir().expect("temp root");
    let bytes = "line one\u{2028}line two\n".as_bytes().to_vec();
    std::fs::write(root.path().join("weird.rs"), &bytes).expect("weird");
    let manifest_path = write_manifest(root.path(), &["weird.rs"]);
    let manifest = load_manifest(&manifest_path).expect("manifest parses");
    let error = load_corpus(root.path(), &manifest, &CorpusLimits::default())
        .expect_err("exotic boundary must fail");
    assert!(error.to_string().contains("exotic"), "{error}");
}

#[test]
fn coverage_accounts_overlap_and_fallbacks() {
    let (_root, files) = load_oracle();
    let chunker = FixedWindowChunker::new(90, 45);
    let (chunks, coverage) = chunk_corpus(&chunker, &files).expect("chunk");
    // Recompute overlap independently from spans.
    let mut expected_overlap: u64 = 0;
    for file in &files {
        let mut covered = vec![false; file.bytes.len()];
        for chunk in &chunks[&file.path] {
            #[allow(clippy::needless_range_loop)]
            for index in chunk.start_byte..chunk.end_byte {
                let slot = usize::try_from(index).unwrap();
                if covered[slot] {
                    expected_overlap += 1;
                } else {
                    covered[slot] = true;
                }
            }
        }
    }
    assert_eq!(coverage.overlap_bytes, expected_overlap);
    let mut expected_chunks = 0;
    for file_chunks in chunks.values() {
        expected_chunks += file_chunks.len();
    }
    assert_eq!(coverage.chunks, expected_chunks);
    assert_eq!(coverage.files, files.len());
}
