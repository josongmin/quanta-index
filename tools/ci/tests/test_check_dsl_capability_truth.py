"""Tests for tools/ci/lint/check-dsl-capability-truth.py (ADV-04).

Hardened after two adversarial self-audit rounds. The decisive tests mutate a
copy of the REAL crates/.../lowering.rs and assert the gate fails closed —
proving the guard bites against real-shaped source, not just synthetic snippets.
The round-2 attack (a guarded `Predicate` widening whose body mentions a decoy
`TypedFail` token before returning `PreserveLexical`) is reproduced and asserted
to be rejected.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-dsl-capability-truth.py"
REAL_LOWERING = REPO_ROOT / "crates" / "quanta-index-search-plane" / "src" / "lowering.rs"

CANON_TYPEDFAIL_ARM = "LqLeaf::StructuralBlock(_) => StructuralLeafVerdict::TypedFail,"
CANON_PHRASE_ARM = "LqLeaf::Phrase(body) => StructuralLeafVerdict::LowerPhraseBody(body),"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_dsl_capability_truth", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_dsl_capability_truth"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _verdict_fn(arms: str) -> str:
    return (
        "fn structural_leaf_verdict(leaf: &LqLeaf) -> StructuralLeafVerdict<'_> {\n"
        "    match leaf {\n"
        f"{arms}\n"
        "    }\n"
        "}\n"
    )


CANONICAL_FN = _verdict_fn(
    "        LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Predicate { .. } => StructuralLeafVerdict::PreserveLexical,\n"
    "        LqLeaf::Phrase(body) => StructuralLeafVerdict::LowerPhraseBody(body),\n"
    "        LqLeaf::Regex(body) => StructuralLeafVerdict::LowerRegexBody(body),\n"
    "        LqLeaf::StructuralBlock(_) => StructuralLeafVerdict::TypedFail,"
)


# --- comment stripping is literal-aware --------------------------------------


def test_strip_keeps_double_slash_inside_string():
    src = 'let url = "https://example.com"; // a real comment\n'
    out = MODULE.strip_rust_comments(src)
    assert '"https://example.com"' in out
    assert "a real comment" not in out


def test_strip_keeps_block_open_inside_string():
    src = 'let s = "/* not a comment */"; /* real */ x\n'
    out = MODULE.strip_rust_comments(src)
    assert '"/* not a comment */"' in out
    assert "real" not in out


def test_strip_handles_raw_string_with_interior_quotes_and_slashes():
    # r#"..."# with interior quotes AND a // must be preserved whole; the real
    # trailing comment is removed. Regression for the hash-delimited raw-string
    # tokenizer (cycle-2 finding).
    src = 'let s = r#"has "inner" and // not-a-comment"#; // real comment\n'
    out = MODULE.strip_rust_comments(src)
    assert 'r#"has "inner" and // not-a-comment"#' in out
    assert "real comment" not in out


def test_strip_handles_nested_hash_raw_string():
    src = 'let s = r##"contains "# not a close"##; // c\n'
    out = MODULE.strip_rust_comments(src)
    assert 'r##"contains "# not a close"##' in out
    assert "// c" not in out


def test_strip_does_not_eat_lifetimes():
    # `'_` / `'a` must not be parsed as an (unterminated) char literal.
    src = "fn f(x: &T) -> StructuralLeafVerdict<'_> { x }\n"
    assert MODULE.strip_rust_comments(src) == src


def test_slice_fn_not_truncated_by_brace_in_comment():
    src = (
        "fn structural_leaf_verdict(leaf: &LqLeaf) -> V {\n"
        "    // a stray }\n"
        "    /* and a block one\n} */\n"
        "    LqLeaf::Keyword(_) => StructuralLeafVerdict::PreserveLexical\n"
        "}\n"
    )
    body = MODULE._slice_fn(src, "fn structural_leaf_verdict")
    assert "PreserveLexical" in body


# --- registry extraction -----------------------------------------------------


def test_extract_registry_predicates_ignores_commented_rows():
    src = """
    pub const PREDICATE_REGISTRY: &[PredicateSpec] = &[
        PredicateSpec { name: "file.contains", kind: PredicateKind::ContentLeaf },
        // PredicateSpec { name: "repo.has.commit", kind: PredicateKind::RepoFileGate },
    ];
    """
    assert MODULE.extract_registry_predicates(src) == {"file.contains"}


# --- canonical matrix shape (the round-2 fix) --------------------------------


def test_shape_canonical_real_source_has_no_violations():
    assert MODULE.structural_matrix_shape_violations(REAL_LOWERING.read_text()) == []


def test_shape_canonical_synthetic_has_no_violations():
    assert MODULE.structural_matrix_shape_violations(CANONICAL_FN) == []


def test_shape_rejects_guard_arm():
    src = _verdict_fn(
        '        LqLeaf::Predicate { name, .. } if name == "repo.has.file" => StructuralLeafVerdict::PreserveLexical,\n'
        "        LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Phrase(_) | LqLeaf::Regex(_) | LqLeaf::StructuralBlock(_) | LqLeaf::Predicate { .. } => StructuralLeafVerdict::TypedFail,"
    )
    violations = MODULE.structural_matrix_shape_violations(src)
    assert any("guard" in v or "if`/`else" in v for v in violations)


def test_shape_rejects_block_body_branch():
    # The round-2 decoy: block body mentions TypedFail before returning PreserveLexical.
    src = _verdict_fn(
        "        LqLeaf::Predicate { .. } => {\n"
        "            if false { StructuralLeafVerdict::TypedFail } else { StructuralLeafVerdict::PreserveLexical }\n"
        "        }\n"
        "        LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Phrase(_) | LqLeaf::Regex(_) | LqLeaf::StructuralBlock(_) => StructuralLeafVerdict::TypedFail,"
    )
    assert MODULE.structural_matrix_shape_violations(src) != []


def test_shape_rejects_helper_delegated_arm():
    # An arm with no inline verdict token (delegates) => arm/verdict mismatch.
    src = _verdict_fn(
        "        LqLeaf::Predicate { .. } => helper(leaf),\n"
        "        LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Phrase(_) | LqLeaf::Regex(_) | LqLeaf::StructuralBlock(_) => StructuralLeafVerdict::TypedFail,"
    )
    assert any("mismatch" in v for v in MODULE.structural_matrix_shape_violations(src))


def test_shape_rejects_nested_match():
    src = _verdict_fn(
        "        LqLeaf::Predicate { .. } => match other { _ => StructuralLeafVerdict::PreserveLexical },\n"
        "        LqLeaf::Keyword(_) | LqLeaf::RawString(_) | LqLeaf::Phrase(_) | LqLeaf::Regex(_) | LqLeaf::StructuralBlock(_) => StructuralLeafVerdict::TypedFail,"
    )
    assert MODULE.structural_matrix_shape_violations(src) != []


# --- extraction under the canonical precondition -----------------------------


def test_extractor_matches_real_source():
    """The checker stays honest: real source extracts to exactly the snapshot."""
    assert (
        MODULE.extract_structural_verdicts(REAL_LOWERING.read_text())
        == MODULE.EXPECTED_STRUCTURAL_VERDICTS
    )


# --- legality on the extracted map -------------------------------------------


def test_structural_legality_passes_on_frozen_map():
    assert MODULE.check_structural_legality(dict(MODULE.EXPECTED_STRUCTURAL_VERDICTS)) == []


def test_structural_legality_flags_narrowing():
    v = dict(MODULE.EXPECTED_STRUCTURAL_VERDICTS)
    v["Phrase"] = "TypedFail"
    assert any("Phrase" in x for x in MODULE.check_structural_legality(v))


def test_structural_legality_flags_new_untriaged_leaf():
    v = dict(MODULE.EXPECTED_STRUCTURAL_VERDICTS)
    v["Glob"] = "PreserveLexical"
    assert any("Glob" in x for x in MODULE.check_structural_legality(v))


# --- predicate parity --------------------------------------------------------


def test_predicate_parity_passes_when_equal():
    names = {"file.contains", "repo.has.file"}
    assert MODULE.check_predicate_parity(names, names) == []


def test_predicate_parity_flags_docs_only_widening():
    v = MODULE.check_predicate_parity(set(), {"repo.has.commit"})
    assert len(v) == 1 and "docs-only widening is forbidden" in v[0]


# --- advanced-claim gate -----------------------------------------------------


def test_advanced_claim_without_section_flagged():
    text = "Status: `advanced`\n\nWe widened it.\n"
    assert len(MODULE.advanced_claim_violations("ADV-XX.md", text)) == 1


def test_advanced_claim_bare_word_in_prose_does_not_satisfy():
    text = "Status: `advanced`\n\nWe will add a benchmark later, maybe.\n"
    assert len(MODULE.advanced_claim_violations("ADV-XX.md", text)) == 1


def test_advanced_claim_with_heading_passes():
    text = "Status: `advanced`\n\n## Benchmark Evidence\n`just rust-bench`\n"
    assert MODULE.advanced_claim_violations("ADV-XX.md", text) == []


def test_advanced_claim_bold_status_is_caught():
    text = "**Status:** advanced\n\nno section here\n"
    assert len(MODULE.advanced_claim_violations("ADV-XX.md", text)) == 1


def test_sota_claim_is_gated():
    text = "Status: `SOTA++`\n\nno section\n"
    assert len(MODULE.advanced_claim_violations("ADV-XX.md", text)) == 1


def test_non_advanced_status_not_gated():
    assert MODULE.advanced_claim_violations("ADV-XX.md", "Status: `in-progress`\nx\n") == []


# --- fail-closed against the REAL source, mutated ----------------------------


def test_main_passes_on_real_tree():
    assert MODULE.main() == 0


def test_main_fails_closed_on_guarded_predicate_widening(tmp_path, monkeypatch):
    """The exact round-2 attack: a guarded Predicate widening of the real file."""
    real = REAL_LOWERING.read_text()
    assert CANON_TYPEDFAIL_ARM in real
    decoy = (
        'LqLeaf::Predicate { name, .. } if name == "repo.has.file" => StructuralLeafVerdict::PreserveLexical,\n'
        "        " + CANON_TYPEDFAIL_ARM
    )
    mutated = tmp_path / "lowering.rs"
    mutated.write_text(real.replace(CANON_TYPEDFAIL_ARM, decoy), encoding="utf-8")
    monkeypatch.setattr(MODULE, "LOWERING_RS", mutated)
    assert MODULE.main() == 1


def test_main_fails_closed_on_flipped_verdict(tmp_path, monkeypatch):
    """A flat (non-guarded) verdict flip must trip the snapshot legality check."""
    real = REAL_LOWERING.read_text()
    assert CANON_PHRASE_ARM in real
    flipped = "LqLeaf::Phrase(_body) => StructuralLeafVerdict::TypedFail,"
    mutated = tmp_path / "lowering.rs"
    mutated.write_text(real.replace(CANON_PHRASE_ARM, flipped), encoding="utf-8")
    monkeypatch.setattr(MODULE, "LOWERING_RS", mutated)
    assert MODULE.main() == 1
