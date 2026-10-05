"""Fixed Rust-fragment parity and custody mutants for the scanner control.

The expected control is literal test data independent of the policy function.
"""

import hashlib
import json
import subprocess

import pytest

from tools.benchmark.retrieval.scanner_source_identity import (
    CONTROL_POLICY,
    CustodyError,
    canonical_control_bytes,
    capture,
    prepare_control,
    verify,
    verify_pair,
)

PATH = "crates/quanta-index-lexical/src/searcher/code_search.rs"
BASE = b"""// unrelated code may change without changing the control policy
fn typo_text_is_ascii(text: &str, budget: &RequestBudgetV1) -> Result<bool, CoreError> {
    for chunk in text.as_bytes().chunks(16_384) {
        budget.checkpoint("lexical:code-search-typo-ascii-detect")?;
        if !chunk.is_ascii() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn typo_witness(
    text: &str,
) -> Result<(), CoreError> {
    budget.checkpoint("lexical:code-search-typo-file-start")?;
    if typo_text_is_ascii(text, budget)? {
        // ASCII byte offsets are scalar ordinals, and the token predicate is
        // exactly ASCII alphanumeric or underscore. A non-ASCII scalar may
        // join an ASCII run, so mixed text retains the Unicode tokenizer.
        scan_typo_token_spans(
            text.bytes()
                .enumerate()
                .map(|(index, byte)| (index, byte.is_ascii_alphanumeric() || byte == b'_')),
            text.len(),
            budget,
            &mut check_token,
        )?;
    } else {
        scan_typo_token_spans(
            text.char_indices()
                .map(|(index, ch)| (index, normalize::is_token_char(ch))),
            text.len(),
            budget,
            &mut check_token,
        )?;
    }
    budget.checkpoint("lexical:code-search-typo-file-end")?;
    Ok(())
}

/// Conservatively shortlist files with shared trigrams.
fn typo_candidates() {}
"""
CONTROL = b"""// unrelated code may change without changing the control policy
#[cfg(test)]
fn typo_text_is_ascii(text: &str, budget: &RequestBudgetV1) -> Result<bool, CoreError> {
    for chunk in text.as_bytes().chunks(16_384) {
        budget.checkpoint("lexical:code-search-typo-ascii-detect")?;
        if !chunk.is_ascii() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn typo_witness(
    text: &str,
) -> Result<(), CoreError> {
    budget.checkpoint("lexical:code-search-typo-file-start")?;
    scan_typo_token_spans(
        text.char_indices()
            .map(|(index, ch)| (index, normalize::is_token_char(ch))),
        text.len(),
        budget,
        &mut check_token,
    )?;
    budget.checkpoint("lexical:code-search-typo-file-end")?;
    Ok(())
}

/// Conservatively shortlist files with shared trigrams.
fn typo_candidates() {}
"""


def _run(root, *argv):
    return subprocess.check_output(["git", *argv], cwd=root)


def _sha(data):
    return hashlib.sha256(data).hexdigest()


@pytest.fixture
def source(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _run(repo, "init", "-q")
    path = repo / PATH
    path.parent.mkdir(parents=True)
    path.write_bytes(BASE)
    (repo / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    _run(repo, "add", ".")
    subprocess.check_call(
        [
            "git",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "base",
        ],
        cwd=repo,
    )
    base = _run(repo, "rev-parse", "HEAD").decode().strip()
    clean = capture(repo, base)
    path.write_bytes(CONTROL)
    patch = tmp_path / "control.patch"
    patch.write_bytes(_run(repo, "diff", "--binary", "--", PATH))
    overlay = {
        "policy_version": CONTROL_POLICY,
        "path": PATH,
        "base_sha256": _sha(BASE),
        "result_sha256": _sha(CONTROL),
        "patch_path": str(patch),
        "patch_sha256": _sha(patch.read_bytes()),
    }
    return repo, base, clean, path, patch, overlay


def test_fixed_fragment_parity_and_unrelated_source_change():
    assert canonical_control_bytes(BASE) == CONTROL
    altered = BASE.replace(b"// unrelated code may change", b"// unrelated code changed")
    assert canonical_control_bytes(altered) == CONTROL.replace(
        b"// unrelated code may change", b"// unrelated code changed"
    )


@pytest.mark.parametrize(
    "mutant",
    [
        BASE.replace(b"fn typo_witness(\n", b"fn renamed_witness(\n"),
        BASE.replace(b"fn typo_text_is_ascii", b"fn renamed_ascii"),
        BASE.replace(
            b"    if typo_text_is_ascii(text, budget)? {", b"    if other_branch(text, budget)? {"
        ),
        BASE.replace(b"byte.is_ascii_alphanumeric()", b"byte.is_ascii()"),
        BASE.replace(b"file-start", b"changed-start"),
        BASE.replace(b"file-end", b"changed-end"),
        BASE.replace(b"fn typo_text_is_ascii", b"#[cfg(test)]\nfn typo_text_is_ascii"),
        BASE + BASE,
        BASE.replace(
            b'    budget.checkpoint("lexical:code-search-typo-file-end")?;\n',
            b'    budget.checkpoint("lexical:code-search-typo-file-end")?;\n'
            b'    budget.checkpoint("lexical:code-search-typo-file-end")?;\n',
        ),
    ],
)
def test_unknown_missing_duplicate_or_pretransformed_anchor_refused(mutant):
    with pytest.raises(CustodyError, match="scanner control"):
        canonical_control_bytes(mutant)


def test_clean_and_one_overlay_have_distinct_real_identities(source):
    repo, base, clean, _, patch, overlay = source
    observed = capture(repo, base, overlay)
    assert observed["source_inventory_sha256"] != clean["source_inventory_sha256"]
    verify(repo, observed, patch_path=patch)
    verify_pair(observed, clean)


def test_prepare_control_uses_fixed_policy_not_caller_patch(source, tmp_path):
    repo, base, _, _, _, _ = source
    patch = tmp_path / "generated.patch"
    overlay_file = tmp_path / "generated.json"
    overlay = prepare_control(repo, base, patch, overlay_file)
    assert json.loads(overlay_file.read_bytes()) == overlay
    assert overlay["result_sha256"] == _sha(CONTROL)
    capture(repo, base, overlay)


def test_arbitrary_claimed_patch_and_extra_change_refused(source):
    repo, base, _, path, patch, overlay = source
    patch.write_bytes(patch.read_bytes() + b"\n")
    with pytest.raises(CustodyError, match="patch digest"):
        capture(repo, base, overlay)
    patch.write_bytes(patch.read_bytes()[:-1])
    for forged in (
        {**overlay, "result_sha256": "0" * 64},
        {**overlay, "policy_version": "unreviewed"},
        {**overlay, "base_sha256": "0" * 64},
    ):
        with pytest.raises(CustodyError):
            capture(repo, base, forged)
    path.write_bytes(CONTROL + b"// extra\n")
    patch.write_bytes(_run(repo, "diff", "--binary", "--", PATH))
    malicious = {
        **overlay,
        "result_sha256": _sha(path.read_bytes()),
        "patch_sha256": _sha(patch.read_bytes()),
    }
    with pytest.raises(CustodyError, match="canonical scanner control policy"):
        capture(repo, base, malicious)


def test_patch_touching_second_file_refused(source):
    repo, base, _, path, patch, overlay = source
    (repo / "Cargo.toml").write_text("[workspace]\n# extra\n", encoding="utf-8")
    patch.write_bytes(_run(repo, "diff", "--binary", "--", PATH, "Cargo.toml"))
    forged = {**overlay, "patch_sha256": _sha(patch.read_bytes())}
    with pytest.raises(CustodyError, match="patch does not apply|unexpected source bytes"):
        capture(repo, base, forged)


def test_pair_and_other_tracked_file_mutants(source):
    repo, base, clean, _, _, overlay = source
    control = capture(repo, base, overlay)
    forged = json.loads(json.dumps(clean))
    next(row for row in forged["source_inventory"] if row["path"] == "Cargo.toml")["sha256"] = (
        "0" * 64
    )
    with pytest.raises(CustodyError, match="outside CodeSearch scanner"):
        verify_pair(control, forged)
    with pytest.raises(CustodyError, match="approved control overlay"):
        verify_pair(clean, control)
    (repo / "Cargo.toml").write_text("[workspace]\n# drift\n", encoding="utf-8")
    with pytest.raises(CustodyError, match="unlisted dirty source"):
        capture(repo, base, overlay)


def test_untracked_staged_mode_head_and_stale_identity_refused(source):
    repo, base, _, path, patch, overlay = source
    observed = capture(repo, base, overlay)
    forged = json.loads(json.dumps(observed))
    forged["identity_sha256"] = "0" * 64
    with pytest.raises(CustodyError, match="identity differs"):
        verify(repo, forged, patch_path=patch)
    extra = repo / "foreign.rs"
    extra.write_text("fn foreign() {}\n", encoding="utf-8")
    with pytest.raises(CustodyError, match="untracked"):
        capture(repo, base, overlay)
    _run(repo, "add", "foreign.rs")
    with pytest.raises(CustodyError, match="staged"):
        capture(repo, base, overlay)
    _run(repo, "reset", "-q", "--", "foreign.rs")
    extra.unlink()
    path.chmod(path.stat().st_mode | 0o100)
    with pytest.raises(CustodyError, match="source mode differs"):
        capture(repo, base, overlay)
    path.chmod(path.stat().st_mode & ~0o111)
    _run(repo, "add", ".")
    subprocess.check_call(
        [
            "git",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "later",
        ],
        cwd=repo,
    )
    with pytest.raises(CustodyError, match="base revision differs"):
        capture(repo, base, overlay)
