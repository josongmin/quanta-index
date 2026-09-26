"""Focused checks for the shell-free retrieval proof prerequisite producer."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import portable_proof
from tools.benchmark.retrieval import run as pairrun


def test_direct_proof_command_has_a_finite_execution_timeout(tmp_path, monkeypatch):
    observed = []

    def run(argv, **kwargs):
        observed.append(kwargs.get("timeout"))
        return b"complete", b"", {"exit_code": 0}

    monkeypatch.setattr(portable_proof, "execute", run)
    commands = []
    assert portable_proof._run("fixture", [sys.executable, "-V"], tmp_path, commands) == b"complete"
    assert len(observed) == 1
    assert type(observed[0]) in (int, float) and 0 < observed[0] <= 7200


def test_direct_proof_failure_uses_real_owner_and_emits_no_command(tmp_path):
    commands = []
    with pytest.raises(ValueError, match="exit 7"):
        portable_proof._run(
            "fixture", [sys.executable, "-c", "raise SystemExit(7)"], tmp_path, commands
        )
    assert commands == []
    assert not list(tmp_path.iterdir())


def test_tool_and_sdk_entrypoints_share_finite_execution_owner(tmp_path, monkeypatch):
    calls = []
    out = tmp_path / "fresh"

    def execute(argv, **kwargs):
        calls.append((argv, kwargs["timeout"]))
        if argv[0] == "fixture-just":
            out.mkdir()
        return b"fixture-version", b"", {"exit_code": 0}

    monkeypatch.setattr(portable_proof, "execute", execute)
    assert portable_proof._git("rev-parse", "HEAD") == "fixture-version"
    commands = []
    portable_proof._run_fresh_recipe(["fixture-just", "recipe"], out, commands)
    assert calls == [(["git", "rev-parse", "HEAD"], 30), (["fixture-just", "recipe"], 7200)]
    assert commands[0]["exit_code"] == 0


def test_collected_pytest_identity_normalizes_windows_separator() -> None:
    nodeid = r"tools\ci\tests\test_retrieval_benchmark.py::test_one"
    assert portable_proof.proof_inventory.junit_identity(nodeid) == (
        "tools.ci.tests.test_retrieval_benchmark.test_one"
    )


def _rust_inventory(binary: str, test: str) -> bytes:
    return json.dumps(
        {
            "test-count": 1,
            "rust-suites": {
                f"quanta-index-retrieval-bench::{binary}": {
                    "package-name": "quanta-index-retrieval-bench",
                    "binary-name": binary,
                    "kind": "test",
                    "status": "listed",
                    "testcases": {test: {"ignored": False, "filter-match": {"status": "matches"}}},
                }
            },
        }
    ).encode()


def _events(binary: str, test: str) -> bytes:
    identity = f"quanta-index-retrieval-bench::{binary}${test}"
    meta = {"crate": "quanta-index-retrieval-bench", "test_binary": binary, "kind": "test"}
    rows = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": meta},
        {"type": "test", "event": "started", "name": identity},
        {"type": "test", "event": "ok", "name": identity},
        {"type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0, "nextest": meta},
    ]
    return ("\n".join(json.dumps(row) for row in rows) + "\n").encode()


@pytest.fixture
def fake_execution(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    out = tmp_path / "proof"
    target = tmp_path / "target"
    runner = target / "debug" / "quanta-index-retrieval-bench"
    searchd = target / "debug" / "quanta-index-searchd"
    tools = {
        name: {
            "path": f"/fake/{name}",
            "realpath": f"/fake/{name}",
            "sha256": "a" * 64,
            "version": "fixture",
        }
        for name in ("python", "cargo", "cargo-nextest", "rustc", "git", "bash", "just", "cargow")
    }
    monkeypatch.setattr(portable_proof, "_source_revision", lambda: "b" * 40)
    monkeypatch.setattr(portable_proof, "_tools", lambda: tools)
    monkeypatch.setattr(
        portable_proof,
        "_os_identity",
        lambda: {
            "system": "fixture",
            "release": "1",
            "machine": "fixture",
            "python_version": "3.9",
        },
    )
    monkeypatch.setattr(
        portable_proof.proof_inventory, "verify_inventory_authority", lambda *_: None
    )
    calls = []

    def write_closure() -> None:
        closure = {
            "schema_version": 1,
            "profile": "retrieval",
            "revision": "b" * 40,
            "roots": ["tools/benchmark/retrieval"],
            "files": [{"path": "tools/benchmark/retrieval/portable_proof.py", "sha256": "a" * 64}],
        }
        closure["digest"] = portable_proof.source_closure._digest(closure)
        (out / "source-closure.json").write_text(json.dumps(closure), encoding="utf-8")

    def write_receipt(argv: list[str]) -> None:
        def option(name: str) -> str:
            return argv[argv.index(name) + 1]

        values = [
            argv[index + 1] for index, value in enumerate(argv) if value == "--input-evidence"
        ]
        inputs = []
        for value in values:
            role, path = value.split("=", 1)
            inputs.append({"role": role, "sha256": portable_proof._sha(Path(path))})
        summary = Path(option("--evidence"))
        payload = json.loads(summary.read_text(encoding="utf-8"))
        canonical = {
            "schema_version": 2,
            "revision": "b" * 40,
            "rail": option("--rail"),
            "tier": "correctness",
            "command": option("--command"),
            "evidence_path": str(summary),
            "evidence_sha256": portable_proof._sha(summary),
            "test_event_count": payload["executed"],
            "source_closure": json.loads((out / "source-closure.json").read_text(encoding="utf-8")),
            "input_evidence": sorted(inputs, key=lambda row: row["role"]),
        }
        Path(option("--out")).write_text(json.dumps(canonical), encoding="utf-8")

    def write_sdk_record() -> None:
        digest = hashlib.sha256(runner.read_bytes()).hexdigest()
        (out / "actual-runner-record.json").write_text(
            json.dumps(
                {
                    "schema_version": 5,
                    "span_accounting_version": 1,
                    "captures": {
                        "run": {
                            "runner_binary": {"name": "runner", "digest": digest},
                            "searchd_binary": {
                                "binary_digest": hashlib.sha256(searchd.read_bytes()).hexdigest()
                            },
                            "receipt_digest": "c" * 64,
                            "activation_digest": "d" * 64,
                        }
                    },
                    "route_provenance": {"lexical": {"capture_id": "run"}},
                }
            ),
            encoding="utf-8",
        )

    def run(argv, **kwargs):
        assert kwargs["cwd"] == portable_proof.ROOT
        assert kwargs["timeout"] == 7200
        calls.append((argv, kwargs["env"]))
        if argv[1] == str(portable_proof.SOURCE_CLOSURE_SCRIPT):
            write_closure()
            raw = b""
        elif argv[1] == str(portable_proof.RECEIPT_WRITER):
            assert (out / "execution-context.json").is_file()
            assert not Path(argv[argv.index("--out") + 1]).exists()
            write_receipt(argv)
            raw = b""
        elif argv[1] == "_retrieval-sdk-proof-raw":
            out.mkdir()
            write_closure()
            runner.parent.mkdir(parents=True, exist_ok=True)
            runner.write_bytes(b"runner")
            searchd.write_bytes(b"searchd")
            (out / "nextest-inventory.json").write_bytes(
                _rust_inventory("sdk_roundtrip", portable_proof.sdk_proof.PROOF_TEST)
            )
            (out / "nextest.jsonl").write_bytes(
                _events("sdk_roundtrip", portable_proof.sdk_proof.PROOF_TEST)
            )
            write_sdk_record()
            summary = portable_proof.sdk_proof.build_summary(
                out / "actual-runner-record.json",
                out / "nextest.jsonl",
                runner,
                out / "nextest-inventory.json",
                searchd_path=searchd,
            )
            (out / "sdk_results.json").write_text(json.dumps(summary), encoding="utf-8")
            raw = b""
        elif "proof_inventory.py" in argv[1]:
            (out / "python-inventory.json").write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "kind": "pytest",
                        "selector": portable_proof.proof_inventory.PYTHON_SELECTOR,
                        "tests": ["tools.ci.tests.test_retrieval_benchmark.test_one"],
                    }
                ),
                encoding="utf-8",
            )
            raw = b""
        elif argv[3:5] == ["nextest", "list"]:
            binary = "sdk_roundtrip" if "sdk_roundtrip" in argv else "chunking_contract"
            test = portable_proof.sdk_proof.PROOF_TEST if binary == "sdk_roundtrip" else "one"
            raw = _rust_inventory(binary, test)
        elif argv[3:5] == ["nextest", "run"]:
            binary = "sdk_roundtrip" if "sdk_roundtrip" in argv else "chunking_contract"
            test = portable_proof.sdk_proof.PROOF_TEST if binary == "sdk_roundtrip" else "one"
            raw = _events(binary, test)
            if binary == "sdk_roundtrip":
                write_sdk_record()
        elif argv[1] == "-m":
            (out / "python-junit.xml").write_text(
                '<testsuite tests="1" failures="0" errors="0" skipped="0">'
                '<testcase classname="tools.ci.tests.test_retrieval_benchmark" name="test_one"/>'
                "</testsuite>",
                encoding="utf-8",
            )
            raw = b""
        elif argv[3] == "build":
            runner.parent.mkdir(parents=True, exist_ok=True)
            runner.write_bytes(b"runner")
            searchd.write_bytes(b"searchd")
            raw = b""
        elif argv[3] == "metadata":
            raw = json.dumps({"target_directory": str(target)}).encode()
        else:
            raise AssertionError(argv)
        return raw, b"", {"exit_code": 0}

    monkeypatch.setattr(portable_proof, "execute", run)
    return out, runner, calls


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_relocated_custody_preserves_commands_and_paths(fake_execution, rail):
    import shutil

    out, _, _ = fake_execution
    receipt = portable_proof.produce(rail, out)
    copied = out.with_name(out.name + "-immutable-copy")
    shutil.copytree(out, copied)
    context = json.loads(receipt.read_text())
    frozen_bins = {}
    for name, binary in context["binaries"].items():
        target = copied / ("frozen-" + name)
        shutil.copyfile(binary["path"], target)
        frozen_bins[name] = target
    relocated = copied / receipt.name
    assert (
        portable_proof.validate(relocated, execution_root=out, binary_files=frozen_bins) == context
    )
    assert relocated.read_bytes() == receipt.read_bytes()
    with pytest.raises(ValueError):
        portable_proof.validate(
            relocated, execution_root=out.with_name("wrong-root"), binary_files=frozen_bins
        )
    if frozen_bins:
        next(iter(frozen_bins.values())).write_bytes(b"tampered binary")
        with pytest.raises(ValueError, match="binary identity changed"):
            portable_proof.validate(relocated, execution_root=out, binary_files=frozen_bins)


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_producer_and_validator_bind_execution_and_inputs(fake_execution, rail: str) -> None:
    out, runner, calls = fake_execution
    receipt = portable_proof.produce(rail, out)
    assert portable_proof.validate(receipt)["rail"] == rail
    assert len(calls) == (7 if rail == "contract" else 3)
    assert all(isinstance(argv, list) for argv, _ in calls)
    context = json.loads(receipt.read_text(encoding="utf-8"))
    assert set(context) == {
        "schema_version",
        "rail",
        "revision",
        "os",
        "tools",
        "binaries",
        "commands",
        "raw_evidence",
    }
    assert not any("results" in name or "receipt" in name for name in context["raw_evidence"])
    assert all("receipt" not in row["name"] for row in context["commands"])
    for name in ("sdk_receipt.json", "contract_python_receipt.json", "contract_rust_receipt.json"):
        artifact = out / name
        if artifact.exists():
            assert portable_proof._sha(artifact) not in receipt.read_text(encoding="utf-8")
    for name in ("sdk_results.json", "contract_python_results.json", "contract_rust_results.json"):
        artifact = out / name
        if artifact.exists():
            assert portable_proof._sha(artifact) not in receipt.read_text(encoding="utf-8")
    canonical = out / ("sdk_receipt.json" if rail == "sdk" else "contract_rust_receipt.json")
    canonical_payload = json.loads(canonical.read_text(encoding="utf-8"))
    assert canonical_payload["schema_version"] == 2
    assert {row["role"]: row["sha256"] for row in canonical_payload["input_evidence"]}[
        "execution-context"
    ] == portable_proof._sha(receipt)
    if rail == "contract":
        python_receipt = json.loads(
            (out / "contract_python_receipt.json").read_text(encoding="utf-8")
        )
        assert {row["role"]: row["sha256"] for row in python_receipt["input_evidence"]}[
            "execution-context"
        ] == portable_proof._sha(receipt)
    pairrun._validate_receipt_shape(canonical_payload, "proof")
    if rail == "sdk":
        assert not (out / "sdk_receipt.recipe-unbound.json").exists()
    if rail == "sdk":
        runner.write_bytes(b"changed")
        with pytest.raises(ValueError, match="binary identity changed"):
            portable_proof.validate(receipt)
    else:
        (out / "rust-test.stdout").write_bytes(b"truncated")
        with pytest.raises(ValueError, match="command output changed"):
            portable_proof.validate(receipt)


def test_validator_rejects_rewritten_command_and_missing_evidence(fake_execution) -> None:
    out, _, _ = fake_execution
    receipt = portable_proof.produce("contract", out)
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["commands"][4]["argv"][4] = "list"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="prescribed rail"):
        portable_proof.validate(receipt)
    data["commands"][4]["argv"][4] = "run"
    data["raw_evidence"].pop("python-junit.xml")
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="raw evidence set"):
        portable_proof.validate(receipt)


def test_validator_rejects_environment_and_sdk_record_substitution(fake_execution) -> None:
    out, _, _ = fake_execution
    receipt = portable_proof.produce("sdk", out)
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["commands"][0]["argv"][1] = "other-recipe"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="prescribed rail"):
        portable_proof.validate(receipt)

    data["commands"][0]["argv"][1] = "_retrieval-sdk-proof-raw"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    record = out / "actual-runner-record.json"
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload["captures"]["run"]["searchd_binary"]["binary_digest"] = "e" * 64
    record.write_text(json.dumps(payload), encoding="utf-8")
    data["raw_evidence"]["actual-runner-record.json"] = portable_proof._sha(record)
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(SystemExit, match="searchd binary digest differs"):
        portable_proof.validate(receipt)


def test_validation_uses_recorded_environment_across_processes(
    fake_execution, monkeypatch: pytest.MonkeyPatch
) -> None:
    out, _, _ = fake_execution
    monkeypatch.setenv("RUSTFLAGS", "-Copt-level=1")
    receipt = portable_proof.produce("contract", out)
    monkeypatch.setenv("RUSTFLAGS", "-Copt-level=2")
    assert portable_proof.validate(receipt)["rail"] == "contract"
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["commands"][4]["inherited_environment"]["RUSTFLAGS"] = "-Copt-level=3"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="environment identity changed"):
        portable_proof.validate(receipt)


def test_canonical_receipt_rejects_command_and_raw_digest_tampering(fake_execution) -> None:
    out, _, _ = fake_execution
    context = portable_proof.produce("contract", out)
    canonical = out / "contract_rust_receipt.json"
    payload = json.loads(canonical.read_text(encoding="utf-8"))
    payload["command"] = "cargo nextest run --lib"
    canonical.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="canonical receipt differs"):
        portable_proof.validate(context)
    payload["command"] = portable_proof.RUST_COMMAND
    payload["input_evidence"][0]["sha256"] = "f" * 64
    canonical.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="canonical receipt differs"):
        portable_proof.validate(context)


def test_existing_writer_emits_context_bound_schema2_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    out = tmp_path / "proof"
    out.mkdir()
    closure = {
        "schema_version": 1,
        "profile": "retrieval",
        "revision": "b" * 40,
        "roots": ["tools/benchmark/retrieval"],
        "files": [{"path": "tools/benchmark/retrieval/portable_proof.py", "sha256": "a" * 64}],
    }
    closure["digest"] = portable_proof.source_closure._digest(closure)
    (out / "source-closure.json").write_text(json.dumps(closure), encoding="utf-8")
    (out / "execution-context.json").write_bytes(b"pre-receipt context")
    (out / "python-junit.xml").write_bytes(b"raw junit")
    (out / "python-inventory.json").write_bytes(b"raw inventory")
    (out / "contract_python_results.json").write_text(
        json.dumps(
            {
                "command": portable_proof.PYTHON_COMMAND,
                "selected": 1,
                "executed": 1,
                "passed": 1,
                "failed": 0,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.syspath_prepend(str(portable_proof.RECEIPT_WRITER.parent))
    spec = importlib.util.spec_from_file_location(
        "retrieval_receipt_writer", portable_proof.RECEIPT_WRITER
    )
    assert spec is not None and spec.loader is not None
    writer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(writer)
    monkeypatch.setattr(writer, "load_and_verify", lambda _: closure)
    argv = portable_proof._receipt_argv("python", out, sys.executable)
    monkeypatch.setattr(sys, "argv", argv[1:])
    assert writer.main() == 0
    receipt = json.loads((out / "contract_python_receipt.json").read_text(encoding="utf-8"))
    pairrun._validate_receipt_shape(receipt, "contract python receipt")
    with pytest.raises(pairrun.RunError, match="raw input evidence mismatch"):
        pairrun._verify_receipt_inputs(
            receipt,
            {
                "pytest-junit": out / "python-junit.xml",
                "pytest-inventory": out / "python-inventory.json",
            },
            "contract python receipt",
        )
    portable_proof._canonical_receipt(
        out / "contract_python_receipt.json",
        rail="retrieval-contract-python",
        command=portable_proof.PYTHON_COMMAND,
        summary=out / "contract_python_results.json",
        inputs={
            "pytest-junit": out / "python-junit.xml",
            "pytest-inventory": out / "python-inventory.json",
            "execution-context": out / "execution-context.json",
        },
        closure=closure,
    )
    (out / "python-junit.xml").write_bytes(b"tampered")
    with pytest.raises(pairrun.RunError, match="raw input evidence mismatch"):
        pairrun._verify_receipt_inputs(
            receipt,
            {
                "pytest-junit": out / "python-junit.xml",
                "pytest-inventory": out / "python-inventory.json",
                "execution-context": out / "execution-context.json",
            },
            "contract python receipt",
        )


def test_bound_receipt_refuses_context_mutation_and_missing_role(fake_execution) -> None:
    out, _, _ = fake_execution
    context_path = portable_proof.produce("contract", out)
    canonical_path = out / "contract_python_receipt.json"
    original_context = context_path.read_bytes()
    context = json.loads(original_context)
    context["os"]["system"] = "different-host"
    context_path.write_text(json.dumps(context), encoding="utf-8")
    with pytest.raises(ValueError, match="canonical receipt differs"):
        portable_proof.validate(context_path)

    context_path.write_bytes(original_context)
    assert portable_proof.validate(context_path)["rail"] == "contract"
    canonical = json.loads(canonical_path.read_text(encoding="utf-8"))
    canonical["input_evidence"] = [
        row for row in canonical["input_evidence"] if row["role"] != "execution-context"
    ]
    canonical_path.write_text(json.dumps(canonical), encoding="utf-8")
    with pytest.raises(ValueError, match="canonical receipt differs"):
        portable_proof.validate(context_path)


def test_sdk_record_rejects_duplicate_json_key(fake_execution) -> None:
    out, _, _ = fake_execution
    receipt = portable_proof.produce("sdk", out)
    record = out / "actual-runner-record.json"
    record.write_bytes(
        record.read_bytes().replace(
            b'"schema_version": 5', b'"schema_version": 5, "schema_version": 5'
        )
    )
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["raw_evidence"]["actual-runner-record.json"] = portable_proof._sha(record)
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(SystemExit, match="duplicate runner record JSON key"):
        portable_proof.validate(receipt)


def test_failed_command_cannot_emit_receipt(
    fake_execution, monkeypatch: pytest.MonkeyPatch
) -> None:
    out, _, _ = fake_execution

    def fail(*_args, **_kwargs):
        raise ValueError("benchmark producer failed with exit 1: failure")

    monkeypatch.setattr(portable_proof, "execute", fail)
    with pytest.raises(ValueError, match="exit 1"):
        portable_proof.produce("contract", out)
    assert not (out / "execution-context.json").exists()


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_validator_captures_each_artifact_once_before_path_replacement(fake_execution, monkeypatch, rail):
    out, _, _ = fake_execution
    receipt = portable_proof.produce(rail, out)
    reader = portable_proof._read_repo_regular_bytes
    reads = {}

    def replace_after_capture(root, name, *, label):
        raw = reader(root, name, label=label)
        reads[name] = reads.get(name, 0) + 1
        (root / name).write_bytes(b"tampered after descriptor capture")
        return raw

    monkeypatch.setattr(portable_proof, "_read_repo_regular_bytes", replace_after_capture)
    assert portable_proof.validate(receipt)["rail"] == rail
    assert reads and all(count == 1 for count in reads.values())
    with pytest.raises((ValueError, OSError)):
        portable_proof.validate(receipt)


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_validator_refuses_symlink_for_every_consumed_proof_artifact(fake_execution, monkeypatch, rail):
    out, _, _ = fake_execution
    receipt = portable_proof.produce(rail, out)
    reader = portable_proof._read_repo_regular_bytes
    consumed = set()

    def track(root, name, *, label):
        consumed.add(name)
        return reader(root, name, label=label)

    monkeypatch.setattr(portable_proof, "_read_repo_regular_bytes", track)
    portable_proof.validate(receipt)
    artifacts = [out / name for name in consumed]
    for path in artifacts:
        saved = path.read_bytes()
        copy = out / "custody-symlink-target"
        copy.write_bytes(saved)
        path.unlink()
        path.symlink_to(copy.name)
        try:
            with pytest.raises((ValueError, OSError)):
                portable_proof.validate(receipt)
        finally:
            path.unlink()
            path.write_bytes(saved)
            copy.unlink()


def test_canonical_summary_digest_and_parse_use_one_capture(tmp_path, monkeypatch):
    summary = tmp_path / "summary.json"
    receipt = tmp_path / "receipt.json"
    summary.write_text(json.dumps({"command": "fixture", "executed": 1}))
    different = json.dumps({"command": "forged", "executed": 1}).encode()
    closure = {"revision": "b" * 40}
    receipt.write_text(json.dumps({
        "schema_version": 2, "revision": "b" * 40, "rail": "fixture",
        "tier": "correctness", "command": "fixture", "evidence_path": str(summary),
        "evidence_sha256": hashlib.sha256(different).hexdigest(), "test_event_count": 1,
        "source_closure": closure, "input_evidence": [],
    }))
    original_json = portable_proof._json

    def replace_after_json(path):
        parsed = original_json(path)
        if path == summary:
            summary.write_bytes(different)
        return parsed

    monkeypatch.setattr(portable_proof, "_json", replace_after_json)
    with pytest.raises(ValueError, match="differs from source and machine evidence"):
        portable_proof._canonical_receipt(receipt, rail="fixture", command="fixture",
            summary=summary, inputs={}, closure=closure)
