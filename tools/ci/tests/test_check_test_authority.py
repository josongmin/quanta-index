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
            'consumer_target = "demo-covered"',
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


def test_grouped_integration_sources_require_manifest_and_launcher_binding(tmp_path: Path):
    module = _load_module()
    crate = tmp_path / "crates" / "demo"
    tests = crate / "tests"
    tests.mkdir(parents=True)
    (tests / "case.rs").write_text("#[test]\nfn case() {}\n", encoding="utf-8")
    (tests / "fast_suite.rs").write_text('#[path = "case.rs"]\nmod case;\n', encoding="utf-8")
    (crate / "Cargo.toml").write_text(
        """
        [package]
        name = "demo"
        version = "0.1.0"
        autotests = false

        [[test]]
        name = "fast_suite"
        path = "tests/fast_suite.rs"
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

        [[integration_targets]]
        id = "demo-case"
        path = "crates/demo/tests/case.rs"
        owner = "demo"
        target = "fast_suite"
        rail = "pr-workspace"

        [[integration_targets]]
        id = "demo-fast-suite"
        path = "crates/demo/tests/fast_suite.rs"
        owner = "demo"
        target = "fast_suite"
        rail = "pr-workspace"
        """,
    )

    assert module.audit_catalog(tmp_path, catalog) == []

    (tests / "fast_suite.rs").write_text("", encoding="utf-8")
    violations = module.audit_catalog(tmp_path, catalog)

    assert any("omits cataloged source case.rs" in violation.message for violation in violations)


def test_local_scope_rejects_unknown_target(tmp_path: Path):
    module = _load_module()
    tests = tmp_path / "crates" / "demo" / "tests"
    tests.mkdir(parents=True)
    (tests / "covered.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [local_scopes.fast]
        lane = "test-fast-lane"
        test_threads = 2
        targets = ["missing"]

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any(
        "unknown integration target missing" in violation.message for violation in violations
    )


def test_local_scope_owner_selection_is_valid(tmp_path: Path):
    module = _load_module()
    tests = tmp_path / "crates" / "demo" / "tests"
    tests.mkdir(parents=True)
    (tests / "covered.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [local_scopes.all-demo]
        lane = "test-demo-lane"
        test_threads = 2
        owners = ["demo"]

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"
        """,
    )

    assert module.audit_catalog(tmp_path, catalog) == []


def test_local_scope_rejects_mixed_library_and_integration_selectors(tmp_path: Path):
    module = _load_module()
    crate = tmp_path / "crates" / "demo"
    tests = crate / "tests"
    tests.mkdir(parents=True)
    (crate / "Cargo.toml").write_text("[package]\nname='demo'\nversion='0.1.0'\n", encoding="utf-8")
    (tests / "covered.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [local_scopes.mixed]
        lane = "test-mixed-lane"
        test_threads = 2
        targets = ["demo-covered"]
        packages = ["demo"]
        lib = true

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any(
        "must keep library and integration selectors separate" in violation.message
        for violation in violations
    )


def test_local_scope_include_cycle_fails_closed(tmp_path: Path):
    module = _load_module()
    tests = tmp_path / "crates" / "demo" / "tests"
    tests.mkdir(parents=True)
    (tests / "covered.rs").write_text("", encoding="utf-8")
    catalog = _write_catalog(
        tmp_path,
        """
        format_version = 1

        [rails.pr-workspace]
        tier = "pr"
        command = "./scripts/cargow nextest run --workspace --all-features --locked"
        target_kind = "integration"

        [local_scopes.first]
        lane = "test-first-lane"
        test_threads = 2
        includes = ["second"]

        [local_scopes.second]
        lane = "test-second-lane"
        test_threads = 2
        includes = ["first"]

        [[integration_targets]]
        id = "demo-covered"
        path = "crates/demo/tests/covered.rs"
        owner = "demo"
        rail = "pr-workspace"
        """,
    )

    violations = module.audit_catalog(tmp_path, catalog)

    assert any("local scope include cycle" in violation.message for violation in violations)


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
