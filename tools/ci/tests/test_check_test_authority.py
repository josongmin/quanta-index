"""Contract tests for the fail-closed test-authority guard."""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-test-authority.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_test_authority", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_test_authority"] = module
    spec.loader.exec_module(module)
    return module


def _write_catalog(path: Path, body: str) -> Path:
    body = body.replace("format_version = 1", "format_version = 2")
    commands = re.findall(r'^\s*command = "([^"]+)"$', body, flags=re.MULTILINE)
    body = body.replace(
        'target_kind = "integration"',
        'target_kind = "integration"\n        workflow = ".github/workflows/test.yml"\n        job = "test"\n        step = "run"',
    ).replace(
        'target_kind = "fuzz"',
        'target_kind = "fuzz"\n        workflow = ".github/workflows/test.yml"\n        job = "test"\n        step = "run"',
    )
    if "[[invariants]]" in body:
        body = body.replace(
            "consumer_target = \"demo-covered\"",
            'consumer_target = "demo-covered"\n        pr_rail = "pr-workspace"\n        merge_rail = "merge-workspace"\n        nightly_rail = "nightly-workspace"',
        )
        body = body.replace(
            '[rails.pr-workspace]\n        tier = "pr"',
            '[rails.merge-workspace]\n        tier = "merge"\n        command = "./scripts/cargow nextest run --workspace --all-features --locked"\n        target_kind = "integration"\n        workflow = ".github/workflows/test.yml"\n        job = "test"\n        step = "run"\n\n        [rails.nightly-workspace]\n        tier = "nightly"\n        command = "./scripts/cargow nextest run --workspace --all-features --locked"\n        target_kind = "integration"\n        workflow = ".github/workflows/test.yml"\n        job = "test"\n        step = "run"\n\n        [rails.pr-workspace]\n        tier = "pr"',
        )
        invariant_rows = re.findall(
            r'\[\[invariants\]\]\s+id = "([^"]+)"\s+risk = "([^"]+)"\s+owner = "([^"]+)"\s+source = "([^"]+)"',
            body,
        )
        body += "\n".join(
            f'\n[[invariant_universe]]\nid = "{ident}"\nrisk = "{risk}"\nowner = "{owner}"\nsource = "{source}"\n'
            for ident, risk, owner, source in invariant_rows
        )
    catalog = path / "test-authority.toml"
    catalog.write_text(body, encoding="utf-8")
    workflow = path / ".github" / "workflows" / "test.yml"
    workflow.parent.mkdir(parents=True)
    workflow.write_text(
        "jobs:\n  test:\n    steps:\n      - name: run\n        run: |\n"
        + "\n".join(f"          {command}" for command in commands),
        encoding="utf-8",
    )
    return catalog


def test_orphan_integration_target_fails_closed(tmp_path: Path):
    module = _load_module()
    (tmp_path / "crates" / "demo" / "tests").mkdir(parents=True)
    (tmp_path / "crates" / "demo" / "tests" / "covered.rs").write_text("", encoding="utf-8")
    (tmp_path / "crates" / "demo" / "tests" / "orphan.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any("orphan integration test target" in violation.message for violation in violations)


def test_invalid_target_reference_and_missing_p0_proof_fail_closed(tmp_path: Path):
    module = _load_module()
    (tmp_path / "crates" / "demo" / "tests").mkdir(parents=True)
    (tmp_path / "crates" / "demo" / "tests" / "covered.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"

        [[invariants]]
        id = "DEMO-P0"
        risk = "P0"
        owner = "demo"
        source = "crates/demo/src/lib.rs"
        positive_target = "demo-covered"
        negative_target = "does-not-exist"
        recovery_target = "demo-covered"
        consumer_target = "demo-covered"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any("unknown target id" in violation.message for violation in violations)


def test_fuzz_manifest_target_must_be_cataloged_and_bound_to_fuzz_rail(tmp_path: Path):
    module = _load_module()
    fuzz = tmp_path / "crates" / "demo" / "fuzz"
    (fuzz / "fuzz_targets").mkdir(parents=True)
    (fuzz / "fuzz_targets" / "decode.rs").write_text("", encoding="utf-8")
    (fuzz / "Cargo.toml").write_text(
        """
        [package]
        name = "demo-fuzz"
        version = "0.0.0"

        [[bin]]
        name = "decode"
        path = "fuzz_targets/decode.rs"
        """,
        encoding="utf-8",
    )
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any("orphan fuzz" in violation.message for violation in violations)


def test_valid_catalog_is_green(tmp_path: Path):
    module = _load_module()
    tests = tmp_path / "crates" / "demo" / "tests"
    tests.mkdir(parents=True)
    (tests / "covered.rs").write_text("", encoding="utf-8")
    source = tmp_path / "crates" / "demo" / "src"
    source.mkdir(parents=True)
    (source / "lib.rs").write_text("", encoding="utf-8")
    fuzz = tmp_path / "crates" / "demo" / "fuzz"
    (fuzz / "fuzz_targets").mkdir(parents=True)
    (fuzz / "fuzz_targets" / "decode.rs").write_text("", encoding="utf-8")
    (fuzz / "Cargo.toml").write_text(
        """
        [package]
        name = "demo-fuzz"
        version = "0.0.0"

        [[bin]]
        name = "decode"
        path = "fuzz_targets/decode.rs"
        """,
        encoding="utf-8",
    )
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [rails.correctness-fuzz]
        tier = "correctness"
        command = "just rust-fuzz-smoke"
        target_kind = "fuzz"

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"

        [[fuzz_targets]]
        id = "demo-decode"
        path = "crates/demo/fuzz/fuzz_targets/decode.rs"
        manifest = "crates/demo/fuzz/Cargo.toml"
        target = "decode"
        owner = "demo"
        rail = "correctness-fuzz"

        """,
    )

    assert module.audit_catalog(tmp_path, catalog) == []


@pytest.mark.parametrize("risk", ["P0", "P1"])
def test_p0_p1_invariants_require_all_four_proof_roles(tmp_path: Path, risk: str):
    module = _load_module()
    (tmp_path / "crates" / "demo" / "tests").mkdir(parents=True)
    (tmp_path / "crates" / "demo" / "tests" / "covered.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        f"""
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"

        [[invariants]]
        id = "DEMO-{risk}"
        risk = "{risk}"
        owner = "demo"
        source = "crates/demo/src/lib.rs"
        positive_target = "demo-covered"
        negative_target = "demo-covered"
        recovery_target = "demo-covered"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any(
        "missing required proof role consumer_target" in violation.message
        for violation in violations
    )
