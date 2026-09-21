"""Keep local changed-file lint selection aligned with each linter's inputs."""

import re
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
CONFIG = ROOT / ".pre-commit-config.yaml"


def test_scoped_repository_lints_skip_unrelated_docs_and_cover_their_inputs() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    inputs = {
        "lock-freshness": ("Cargo.lock", "scripts/check-lock-freshness.sh"),
        "workspace-lints": ("Cargo.toml", "scripts/check_workspace_lints.py"),
        "hexagonal-boundaries": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/lint-hexagonal-boundaries.py",
        ),
        "rust-no-allow": (
            "crates/quanta-index-core/src/lib.rs",
            "scripts/check-rust-allow-attributes.sh",
        ),
        "rust-derive-allowlist": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-rust-derive-allowlist.py",
        ),
        "rust-cargo-toml-hygiene": (
            "crates/quanta-index-core/Cargo.toml",
            "tools/ci/lint/check-cargo-toml-hygiene.py",
        ),
        "rust-module-discipline": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-module-discipline.py",
        ),
        "rust-module-cycles": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-module-cycles.py",
        ),
        "rust-error-shape": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-error-shape.py",
        ),
        "rust-wire-inventory": (
            "crates/quanta-index-contract/src/ipc/split.rs",
            "tools/ci/proof-aggregate.schema.json",
            "tools/ci/tests/test_write_proof_aggregate.py",
        ),
        "rust-digest-fallibility": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-digest-fallibility.py",
        ),
        "semgrep": ("crates/quanta-index-core/src/lib.rs", "scripts/run-semgrep.sh"),
    }

    for hook_id, positive_paths in inputs.items():
        hook = hooks[hook_id]
        assert not hook.get("always_run", False), hook_id
        assert hook["pass_filenames"] is False, hook_id
        pattern = re.compile(hook["files"])
        for path in positive_paths:
            assert pattern.search(path), (hook_id, path)
        assert not pattern.search("docs/analysis/unrelated.md"), hook_id


def test_wire_inventory_scope_includes_tool_format_dependencies() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hook = next(
        hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
        if hook["id"] == "rust-wire-inventory"
    )
    pattern = re.compile(hook["files"])
    for path in (
        "Cargo.toml",
        "tools/ci/inventory/wire-surface.toml",
        "tools/ci/proof-authority.toml",
        "tools/ci/proof-manifest.schema.json",
        "tools/ci/proof-aggregate.schema.json",
        "tools/ci/error-authority-inventory.schema.json",
        "tools/ci/verification-receipt.schema.json",
        "tools/ci/tests/test_check_proof_authority.py",
        "tools/ci/tests/test_write_proof_aggregate.py",
        "tools/ci/tests/test_write_error_authority_inventory.py",
    ):
        assert pattern.search(path), path
