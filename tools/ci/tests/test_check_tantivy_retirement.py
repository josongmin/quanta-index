from __future__ import annotations

import importlib.util
import json
from pathlib import Path

import pytest


def load_checker():
    path = Path(__file__).resolve().parents[1] / "check-tantivy-retirement.py"
    spec = importlib.util.spec_from_file_location("check_tantivy_retirement", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def checkout(tmp_path, monkeypatch):
    checker = load_checker()
    daemon = "quanta-index-searchd-runtime"
    (tmp_path / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
    (tmp_path / "Cargo.lock").write_text(f'[[package]]\nname = "{daemon}"\n')
    cargo = {
        "tree": f"{daemon} v0.1.0\n",
        "metadata": json.dumps({"packages": [{"name": daemon}]}),
    }

    def cargo_output(root, command, *args):
        assert root == tmp_path and "--all-features" in args
        if command == "tree":
            assert "--workspace" in args
            assert args[args.index("--edges") + 1] == "normal,build,dev"
            assert args[args.index("--target") + 1] == "all"
        return cargo[command]

    monkeypatch.setattr(checker, "ROOT", tmp_path)
    monkeypatch.setattr(checker, "cargo_output", cargo_output)
    return checker, cargo


def test_keeps_tantivy_free_lexical_package(checkout):
    checker, cargo = checkout
    cargo["tree"] += "quanta-index-lexical v0.1.0\n"
    assert checker.main() == 0


def test_independent_dependency_and_vendor_residue(checkout, capsys):
    checker, cargo = checkout
    cargo["tree"] += "tantivy-fst v0.5.0\n"
    metadata = json.loads(cargo["metadata"])
    metadata["packages"].append({"name": "tantivy-columnar"})
    cargo["metadata"] = json.dumps(metadata)
    lock = checker.ROOT / "Cargo.lock"
    lock.write_text(lock.read_text() + '[[package]]\nname = "tantivy_common"\n')
    (checker.ROOT / "Cargo.toml").write_text(
        '[workspace]\nexclude = ["vendor/tantivy-old"]\n'
        '[workspace.dependencies]\nsearch = { package = "tantivy", version = "0.22" }\n'
        '[patch.crates-io]\ntantivy = { path = "vendor/renamed" }\n'
    )
    renamed = checker.ROOT / "vendor/renamed"
    renamed.mkdir(parents=True)
    (renamed / "Cargo.toml").write_text('[package]\nname = "tantivy"\n')
    assert checker.main() == 1
    assert capsys.readouterr().err.splitlines() == [
        "Cargo tree: tantivy-fst",
        "Cargo metadata: tantivy-columnar",
        "Cargo.lock: tantivy_common",
        "Cargo.toml workspace.dependencies: search",
        "Cargo.toml patch.crates-io: tantivy",
        "Cargo.toml workspace.exclude: vendor/tantivy-old",
        "vendor: vendor/renamed",
    ]


@pytest.mark.parametrize(
    ("command", "response"),
    [("tree", ""), ("metadata", '{"packages": []}'), ("metadata", "{broken")],
)
def test_empty_or_malformed_resolution_is_unavailable(checkout, command, response):
    checker, cargo = checkout
    cargo[command] = response
    assert checker.main() == 2


@pytest.fixture
def release_build(checkout):
    checker, cargo = checkout
    evidence = checker.ROOT / "captured-release"
    evidence.mkdir()
    binary = evidence / checker.DAEMON_BINARY
    binary.write_bytes(b"retained daemon bytes")
    compiler_path = checker.ROOT / "target/release" / checker.DAEMON_BINARY
    daemon_id = "daemon-package"
    cargo["metadata"] = json.dumps({"packages": [
        {"id": daemon_id, "name": checker.WORKSPACE_PACKAGE},
        {"id": "dependency-package", "name": "tantivy-fst"},
    ]})
    receipt = {
        "status": "VERIFIED",
        "exit_code": 0,
        "source_before_after_equal": True,
        "command": [
            "./scripts/cargow", "build", "--release", "--locked", "--offline",
            "--message-format=json-render-diagnostics",
        ],
        "binaries": {checker.DAEMON_BINARY: {
            "path": str(binary),
            "compiler_artifact": str(compiler_path),
            "sha256": checker.file_sha256(binary),
        }},
    }
    (evidence / "build-result.json").write_text(json.dumps(receipt))
    for name in ("source-before.json", "source-after-build.json"):
        (evidence / name).write_text('{"files":{"Cargo.toml":"source-sha"}}')
    events = [
        {
            "reason": "compiler-artifact",
            "package_id": daemon_id,
            "manifest_path": str(checker.ROOT / "crates" / checker.WORKSPACE_PACKAGE / "Cargo.toml"),
            "target": {"name": checker.DAEMON_BINARY, "kind": ["bin"]},
            "profile": {"test": False, "opt_level": "3"},
            "executable": str(compiler_path),
        },
        {"reason": "build-finished", "success": True},
    ]
    return checker, evidence, binary, events


@pytest.mark.parametrize("fault, expected", [
    (None, 0),
    ("tantivy", 1),
    ("missing_terminal", 2),
    ("binary_changed", 2),
    ("source_changed", 2),
])
def test_observed_release_compiler_packages(release_build, fault, expected, capsys):
    checker, evidence, binary, events = release_build
    if fault == "tantivy":
        events.insert(0, {"reason": "compiler-artifact", "package_id": "dependency-package"})
    elif fault == "missing_terminal":
        events.pop()
    elif fault == "binary_changed":
        binary.write_bytes(b"different daemon bytes")
    elif fault == "source_changed":
        (evidence / "source-after-build.json").write_text('{"files":{"Cargo.toml":"different"}}')
    (evidence / "build-artifacts.jsonl").write_text("".join(json.dumps(row) + "\n" for row in events))
    assert checker.main(evidence) == expected
    if fault == "tantivy":
        assert "Release compiler artifact: tantivy-fst" in capsys.readouterr().err


@pytest.mark.parametrize("expected", ["same", "different", "invalid"])
def test_release_matches_installed_daemon_digest(release_build, expected, capsys):
    checker, evidence, binary, events = release_build
    (evidence / "build-artifacts.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in events)
    )
    actual = checker.file_sha256(binary)
    anchor = {"same": actual, "different": "0" * 64, "invalid": "missing"}[expected]
    status = checker.main(evidence, anchor)
    observed = capsys.readouterr()
    if expected == "same":
        assert status == 0
        assert actual in observed.out
    elif expected == "different":
        assert status == 2
        assert "differs from expected" in observed.err
    else:
        assert status == 2
        assert "64 lowercase hex characters" in observed.err


def test_installed_digest_requires_release_evidence(checkout, capsys):
    checker, _ = checkout
    assert checker.main(expected_daemon_sha256="a" * 64) == 2
    assert "requires --release-build" in capsys.readouterr().err


@pytest.mark.parametrize(("fault", "expected"), [
    (None, 0),
    ("forbidden_dependency", 1),
    ("wrong_executable", 2),
    ("missing_terminal", 2),
])
def test_current_compiler_stream_binds_built_daemon(release_build, fault, expected, capsys):
    checker, evidence, binary, events = release_build
    events[0]["executable"] = str(binary)
    if fault == "forbidden_dependency":
        events.insert(0, {"reason": "compiler-artifact", "package_id": "dependency-package"})
    elif fault == "wrong_executable":
        events[0]["executable"] = str(evidence / "other-daemon")
    elif fault == "missing_terminal":
        events.pop()
    artifacts = evidence / "build-artifacts.jsonl"
    artifacts.write_text("".join(json.dumps(row) + "\n" for row in events))
    assert checker.main(compiler_artifacts=artifacts, built_daemon=binary) == expected
    output = capsys.readouterr()
    if expected == 0:
        assert output.out.strip() == checker.file_sha256(binary)
    elif fault == "forbidden_dependency":
        assert "Release compiler artifact: tantivy-fst" in output.err
    else:
        assert not output.out
