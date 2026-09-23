//! E2E-00 — fixtures for the live DSL matrix harness.
//!
//! Tiny corpus that intentionally varies repo/path/language/content so a
//! single ingest can exercise multiple filter dimensions in later E2E
//! tickets (repo, path, lang, content). The smoke self-test asserts the
//! exact candidate identity set below; the broader columns sit here so
//! each follow-up ticket does not need its own fixture file.

use anyhow::Result as AnyResult;

use super::e2e_harness::E2eRuntime;

#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct CorpusRow {
    /// Stable per-row identifier consumed by later E2E rows when they
    /// assert "candidate set equals expected ids". E2E-00 smoke does not
    /// read this field directly; the `dead_code` expectation keeps the
    /// workspace lint happy until the first follow-up ticket lands.
    #[expect(
        dead_code,
        reason = "consumed by E2E-01..07, not by the E2E-00 smoke row"
    )]
    pub(super) id: &'static str,
    pub(super) repo: &'static str,
    pub(super) path: &'static str,
    pub(super) content: &'static str,
    #[expect(
        dead_code,
        reason = "consumed by E2E-01..07 (lang: filters); not by the E2E-00 smoke row"
    )]
    pub(super) lang: &'static str,
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) const SMOKE_CORPUS: &[CorpusRow] = &[
    CorpusRow {
        id: "alpha",
        repo: "repo-e2e",
        path: "src/lib.rs",
        content: "fn smoke_needle_rust() {}",
        lang: "rust",
    },
    CorpusRow {
        id: "beta",
        repo: "repo-e2e",
        path: "src/util.py",
        content: "def smoke_needle_python(): pass",
        lang: "python",
    },
    CorpusRow {
        id: "gamma",
        repo: "repo-e2e",
        path: "docs/intro.md",
        content: "smoke documentation lives here",
        lang: "markdown",
    },
];

#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) fn ingest_all(rt: &mut E2eRuntime, rows: &[CorpusRow]) -> AnyResult<()> {
    for row in rows {
        rt.ingest_text(row.repo, row.path, row.content)?;
    }
    Ok(())
}

/// The smoke query text. Exactly one corpus row carries it (see
/// [`SMOKE_NEEDLE_RUST_IDENTITY`]); the other rows carry lookalike or
/// unrelated content so a wrong-row match cannot hide.
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) const SMOKE_NEEDLE_RUST: &str = "smoke_needle_rust";

/// Fixture contract: the exact `(repo, path)` identity set a
///
/// [`SMOKE_NEEDLE_RUST`] query must return. `alpha` carries the rust
/// needle; `beta` carries the python needle; `gamma` carries no needle.
/// The smoke oracle asserts this set exactly — missing rows and extras
/// both fail — and the consistency test below keeps the contract pinned
/// to the corpus contents.
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) const SMOKE_NEEDLE_RUST_IDENTITY: &[(&str, &str)] = &[("repo-e2e", "src/lib.rs")];

#[test]
fn smoke_needle_contract_matches_corpus_contents() {
    let mut derived: Vec<(&str, &str)> = SMOKE_CORPUS
        .iter()
        .filter(|row| row.content.contains(SMOKE_NEEDLE_RUST))
        .map(|row| (row.repo, row.path))
        .collect();
    derived.sort_unstable();
    let mut expected = SMOKE_NEEDLE_RUST_IDENTITY.to_vec();
    expected.sort_unstable();
    assert_eq!(
        derived, expected,
        "the smoke identity contract must pin exactly the rows carrying the needle"
    );
}
