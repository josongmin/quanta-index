"""Retrieval proof custody keeps terminal test counts separate from relevance."""

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import retrieval_capture as capture
from evidence import sample_evidence
from registry import load_registry

from tools.ci.tests.test_portable_proof import fake_execution  # noqa: F401
from tools.ci.tests.test_portable_proof import proof_actor_environment as proof_actor_environment


def test_proof_payload_bounds_control_summary_before_decode(tmp_path):
    from evidence import CONTROL_DOCUMENT_BYTES

    with (tmp_path / "sdk_results.json").open("wb") as stream:
        stream.truncate(CONTROL_DOCUMENT_BYTES + 1)
    with pytest.raises(capture.EvidenceError, match="control document exceeds"):
        capture.proof_payload(tmp_path, {"rail": "sdk"}, {})


def test_bridge_fixture_preserves_canonical_evidence_identity():
    from tools.ci.tests.test_benchmark_evidence_bridge import _bridge

    evidence_module = sys.modules["evidence"]
    assert capture.EvidenceError is evidence_module.EvidenceError
    bridge = _bridge()
    assert _bridge() is bridge
    assert sys.modules["evidence"] is evidence_module
    assert bridge.EvidenceError is capture.EvidenceError


@pytest.mark.parametrize("rail", ["sdk", "contract"])
def test_registration_requires_current_execution_context_version(rail):
    from evidence import EvidenceError

    registry = load_registry()
    entry = registry["families"][f"retrieval-{rail}"]
    producer = registry["producers"][entry["producer"]]
    assert entry["native_schema"] == "retrieval-execution-context:v2"
    capture.require_registered_owner(entry, producer, rail)
    with pytest.raises(EvidenceError, match="registration"):
        capture.require_registered_owner(
            {**entry, "native_schema": "retrieval-execution-context:v1"}, producer, rail
        )


def fixture(tmp_path, rail="contract"):
    source = sample_evidence()["source"]
    native = tmp_path / "proof"
    native.mkdir()
    context = {"rail": rail, "revision": source["revision"]}
    (native / "execution-context.json").write_text(json.dumps(context))
    (native / "source-closure.json").write_text(
        json.dumps({"revision": source["revision"], "digest": "a" * 64})
    )
    names = (
        ["sdk_results.json"]
        if rail == "sdk"
        else ["contract_python_results.json", "contract_rust_results.json"]
    )
    for name in names:
        (native / name).write_text(
            json.dumps({"selected": 3, "executed": 3, "passed": 3, "failed": 0})
        )
    return native, context, source


@pytest.mark.parametrize("rail,expected", [("contract", 6), ("sdk", 3)])
def test_counts_keep_proof_semantics(tmp_path, rail, expected):
    native, context, source = fixture(tmp_path, rail)
    result = capture.proof_payload(native, context, source)
    assert result["kind"] == "proof"
    assert result["selected"] == result["executed"] == result["passed"] == expected
    assert result["failed"] == 0
    assert "metric_space" not in result and "value" not in result
    assert result["source_digest"] == source["closure_digest"]


@pytest.mark.parametrize(
    "changes",
    [
        {"selected": 0},
        {"executed": 2},
        {"passed": 2, "failed": 1},
        {"failed": True},
        {"selected": "3"},
        {"passed": -1},
        {"selected": 2**64, "executed": 2**64, "passed": 2**64},
    ],
)
def test_bad_or_partial_terminal_counts_refuse(tmp_path, changes):
    native, context, source = fixture(tmp_path)
    path = native / "contract_python_results.json"
    summary = json.loads(path.read_text())
    summary.update(changes)
    path.write_text(json.dumps(summary))
    with pytest.raises(ValueError):
        capture.proof_payload(native, context, source)


def test_source_mismatch_refuses(tmp_path):
    native, context, source = fixture(tmp_path)
    context["revision"] = "b" * 40
    with pytest.raises(ValueError, match="current frozen source"):
        capture.proof_payload(native, context, source)


@pytest.mark.parametrize("context", [{}, {"rail": "unknown"}, None])
def test_malformed_context_refuses(tmp_path, context):
    native, _, source = fixture(tmp_path)
    with pytest.raises(ValueError, match="rail"):
        capture.proof_payload(native, context, source)


def test_build_target_comes_from_rustc_and_recorded_environment():
    context = {
        "tools": {"rustc": {"version": "rustc 1.92.0\nhost: aarch64-apple-darwin"}},
        "commands": [{"inherited_environment": {}}],
    }
    assert capture.target_identity(context) == "aarch64-apple-darwin"
    context["commands"][0]["inherited_environment"]["CARGO_BUILD_TARGET"] = (
        "x86_64-unknown-linux-gnu"
    )
    assert capture.target_identity(context) == "x86_64-unknown-linux-gnu"
    context["commands"].append({"inherited_environment": {}})
    with pytest.raises(ValueError, match="inconsistent"):
        capture.target_identity(context)
    context["tools"]["rustc"]["version"] = "rustc without host identity"
    with pytest.raises(ValueError, match="unique host triple"):
        capture.target_identity(context)


@pytest.mark.parametrize(
    "field,value",
    [
        ("validator", "evidence-protocol"),
        ("scorer", "retrieval-relevance"),
        ("native_schema", "retrieval-run-manifest:v5"),
        ("gate_tier", "authority"),
        ("host_policy", "canonical-linux"),
        ("result_unit", "ratio"),
    ],
)
def test_owner_registration_drift_refuses(field, value):
    registry = load_registry()
    entry = registry["families"]["retrieval-sdk"]
    producer = registry["producers"][entry["producer"]]
    capture.require_registered_owner(entry, producer, "sdk")
    with pytest.raises(ValueError, match="implemented owner contract"):
        capture.require_registered_owner({**entry, field: value}, producer, "sdk")


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_common_custody_replays_owner_raw_and_rejects_tampering(fake_execution, rail):  # noqa: F811
    from evidence import RunStore, canonical_json
    from evidence_bridge import promote_native_run

    from tools.benchmark.retrieval import portable_proof

    native, _, _ = fake_execution
    portable_proof._tools()["rustc"]["version"] = "rustc fixture\nhost: fixture-triple"
    portable_proof.produce(rail, native)
    context = portable_proof.validate(native / "execution-context.json")
    template = sample_evidence()
    source = {**template["source"], "revision": "b" * 40}
    payload = capture.proof_payload(native, context, source)
    command = {**template["command"], "argv": ["just", f"retrieval-{rail}-proof", str(native)]}
    capture_id = "retrieval-fixture"
    raw = {path.name: path.read_bytes() for path in native.iterdir()}
    raw["capture-origin.json"] = canonical_json(
        {
            "capture_id": capture_id,
            "execution_root": str(native),
            "producer": command,
        }
    ).encode()
    for name, entry in context["binaries"].items():
        raw[f"frozen-binary-{name}"] = Path(entry["path"]).read_bytes()
    build = {
        **template["build"],
        "toolchain": context["tools"]["rustc"]["version"],
        "target_triple": "fixture-triple",
        "flags": portable_proof.FLAGS,
        "binaries": [
            {"name": name, "sha256": "sha256:" + entry["sha256"]}
            for name, entry in sorted(context["binaries"].items())
        ],
    }
    closure = json.loads((native / "source-closure.json").read_text())
    inputs = [
        {"id": name, "availability": "present", "digest": digest, "reason": None}
        for name, digest in [
            ("execution-context", payload["execution_context_digest"]),
            ("native-source", "sha256:" + closure["digest"]),
        ]
    ]
    items = list(raw.items())
    root = native.parent / "common-evidence"
    result = promote_native_run(
        evidence_root=root,
        run_id=f"{capture_id}-retrieval-{rail}",
        family=f"retrieval-{rail}",
        profile=capture.PROFILE,
        case_id=None,
        created_utc=template["created_utc"],
        raw_files={
            name: capture.write_raw_file(root / "work" / rail / name, [data])
            for name, data in items
        },
        payload=payload,
        source=source,
        build=build,
        inputs=inputs,
        host=template["host"],
        command=command,
        boundary=template["boundary"],
        verdict={"scope": "contract", "status": "pass", "reason": None, "metrics": []},
    )
    store = RunStore(root)
    evidence = store.load(result["run_id"])
    capture.replay_run(store, evidence)
    bad = {**evidence, "payload": {**payload, "passed": payload["passed"] + 1}}
    with pytest.raises(ValueError, match="typed counts"):
        capture.replay_run(store, bad)
    original = store.run_dir(result["run_id"]) / "raw" / "execution-context.json"
    altered = json.loads(original.read_text())
    del altered["rail"]
    original.write_text(json.dumps(altered))
    with pytest.raises(ValueError, match="command/rail"):
        capture.replay_run(store, evidence)


def test_cli_requires_explicit_external_root_before_capture(capsys):
    import benchctl

    assert benchctl.main(["run", "retrieval-contract"]) == 2
    assert "requires --evidence-root" in capsys.readouterr().err


def test_cli_refuses_mixed_controls_without_producer(capsys, tmp_path):
    import benchctl

    root = tmp_path / "evidence"
    assert (
        benchctl.main(
            ["run", "retrieval-contract", "--evidence-root", str(root), "--criterion-samples", "10"]
        )
        == 2
    )
    assert "unrelated producer controls" in capsys.readouterr().err
    assert not root.exists()


def test_cli_dirty_source_is_typed_refusal_before_producer(monkeypatch, capsys, tmp_path):
    import benchctl

    def dirty(_repo):
        raise RuntimeError("worktree is dirty: injected admission refusal")

    monkeypatch.setattr(benchctl, "require_clean_worktree", dirty)
    root = tmp_path / "evidence"
    assert benchctl.main(["run", "retrieval-contract", "--evidence-root", str(root)]) == 2
    assert "retrieval proof refused: worktree is dirty" in capsys.readouterr().err
    failures = list((root / "failures").glob("*.json"))
    assert len(failures) == 1
    failure = json.loads(failures[0].read_text())
    assert failure["phase"] == "source"
    assert "worktree is dirty" in failure["error"]["message"]
    assert not (root / "profiles").exists()


@pytest.mark.parametrize(
    "recipe",
    ["retrieval-contract-proof", "retrieval-sdk-proof", "retrieval-quanta", "retrieval-pair"],
)
def test_just_external_parameter_is_not_shell_syntax(tmp_path, recipe):
    import os
    import subprocess

    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    captured = tmp_path / "argv.json"
    sentinel = tmp_path / "shell-injection"
    stub = (
        f"#!{sys.executable}\nimport json,os,sys\nfrom pathlib import Path\n"
        "Path(os.environ['PARAM_CAPTURE']).write_text(json.dumps(sys.argv[1:]))\n"
    )
    for name in ("uv", "python3"):
        path = fake_bin / name
        path.write_text(stub)
        path.chmod(0o755)
    external = str(tmp_path / 'external"') + f"; touch {sentinel}; #"
    completed = subprocess.run(
        [
            "just",
            "--justfile",
            str(capture.ROOT / "Justfile"),
            "--shell",
            "/bin/sh",
            "--clear-shell-args",
            "--shell-arg",
            "-cu",
            recipe,
            external,
        ],
        env={
            **os.environ,
            "PATH": str(fake_bin) + os.pathsep + os.environ["PATH"],
            "PARAM_CAPTURE": str(captured),
        },
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    argv = json.loads(captured.read_text())
    assert argv[-1] == external
    assert not sentinel.exists()
