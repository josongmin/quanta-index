"""Conditional producer wiring and archived custody counterexamples.

The context constructor below is synthetic validator input, not an execution
receipt: its fake source closure must fail current-source verification.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from argparse import Namespace
from pathlib import Path

import pytest

from tools.benchmark.retrieval import conditional_proof as cp
from tools.benchmark.retrieval import portable_proof, tool_custody
from tools.ci import source_closure
from tools.ci.tests.test_portable_proof import proof_actor_environment as proof_actor_environment
from tools.ci.tests.test_tool_custody import executable, selected_tools


def add_custody_unit_fixture(bundle: dict, tmp_path: Path) -> dict:
    """Construct the new custody fields on a synthetic T15/T16 unit bundle."""
    context = bundle["execution_context"]
    tools = selected_tools(tmp_path)
    wrapper = cp.ROOT / "scripts/cargow"
    tools["cargow"] = {
        "path": str(wrapper),
        "realpath": str(wrapper.resolve()),
        "sha256": hashlib.sha256(wrapper.read_bytes()).hexdigest(),
        "version": "fixture",
    }
    guard = tool_custody.ToolCustody.create(
        cp.ROOT, tmp_path / "private-bin", tools=tools, environment=dict(os.environ)
    )
    native = executable(tmp_path / "native/proof-binary", "printf fixture-only")
    native_epoch = guard.bind_executable(native)
    context["binary_sha256"] = native_epoch["sha256"]
    context["run"]["argv"][0] = str(native)
    context["run"]["executable_sha256"] = native_epoch["sha256"]
    events = [cp.load(line) for line in cp.decode(context["build"]["stdout"]).splitlines()]
    for event in events:
        if event.get("reason") == "compiler-artifact":
            event["executable"] = str(native)
    context["build"]["stdout"] = cp.artifact(b"\n".join(cp.canonical(event) for event in events))
    reference = context["reference_run"]
    if reference is not None:
        reference["argv"][0] = sys.executable
        reference["interpreter_sha256"] = guard.bind_executable(Path(sys.executable))["sha256"]
    context["tool_custody"] = guard.record()
    inherited = portable_proof._relevant_environment(dict(os.environ))
    overrides = {
        "CARGO_NET_OFFLINE": "true",
        **portable_proof.execution_overrides(guard.tools(), inherited),
    }
    command_identity = {
        "cwd": str(cp.ROOT),
        "inherited_environment": inherited,
        "environment": overrides,
        "environment_sha256": portable_proof._environment_digest({**inherited, **overrides}),
    }
    context["environment"]["relevant"] = inherited
    for command in (context["build"], context["run"], reference):
        if command is not None:
            command.update(command_identity)
    files = {entry["path"]: entry["sha256"] for entry in context["source_closure"]["files"]}
    files["scripts/cargow"] = tools["cargow"]["sha256"]
    context["source_closure"]["files"] = [
        {"path": path, "sha256": digest} for path, digest in sorted(files.items())
    ]
    context["source_closure"]["digest"] = source_closure._digest(
        {key: value for key, value in context["source_closure"].items() if key != "digest"}
    )
    context["source_manifest"] = cp.artifact(cp.canonical(context["source_closure"]))
    input_path = Path(context["run"]["argv"][2 if reference is not None else 1])
    manifest = input_path.parent / "source-closure.json"
    for phase in ("capture", "verify"):
        argv = [tools["python"]["path"], str(cp.ROOT / "tools/ci/source_closure.py"), phase]
        argv.extend(
            ["--profile", "retrieval", "--out", str(manifest)]
            if phase == "capture"
            else ["--manifest", str(manifest)]
        )
        stdout = f"source closure {phase} ok: retrieval {len(files)} files {context['source_closure']['digest']}\n".encode()
        context[f"source_{phase}"] = {
            **command_identity,
            "argv": argv,
            "exit_code": 0,
            "stdout": cp.artifact(stdout),
            "stderr": cp.artifact(b""),
        }
    bundle["execution_receipt"].update(
        runner_binary_sha256=native_epoch["sha256"], context_sha256=cp.sha(cp.canonical(context))
    )
    return bundle


def test_custody_unit_fixture_rebinds_manifest_without_self_hashing(tmp_path):
    from tools.ci.tests.test_retrieval_benchmark import (
        _conditional_context_unit_bundle,
        _conditional_incremental_unit_oracle,
    )

    plan, observed = _conditional_incremental_unit_oracle()
    bundle = add_custody_unit_fixture(_conditional_context_unit_bundle(plan, observed), tmp_path)
    context = bundle["execution_context"]
    manifest = source_closure.validate_manifest_shape(context["source_closure"])
    assert cp.load(cp.decode(context["source_manifest"])) == manifest
    assert bundle["execution_receipt"]["context_sha256"] == cp.sha(cp.canonical(context))
    # A synthetic closure is shape-valid, not current-source execution proof.
    with pytest.raises(source_closure.ClosureError):
        source_closure.verify_manifest(cp.ROOT, manifest)
    manifest["files"][0]["sha256"] = "0" * 64
    with pytest.raises(source_closure.ClosureError, match="digest mismatch"):
        source_closure.validate_manifest_shape(manifest)


@pytest.mark.parametrize("key", ["BASH_FUNC_cargo%%", "BASH_FUNC_python3%%", "BASH_FUNC_command%%"])
def test_conditional_produce_refuses_function_exports_before_inputs_or_tools(monkeypatch, key):
    monkeypatch.setenv(key, "")
    monkeypatch.setattr(portable_proof, "_tools", lambda: pytest.fail("tools must not be invoked"))
    with pytest.raises(tool_custody.ToolCustodyError, match="function exports"):
        cp.produce(None)


def test_conditional_produce_enters_shared_context_and_resets_it_on_failure(
    monkeypatch, tmp_path, proof_actor_environment
):
    tools = selected_tools(tmp_path)
    monkeypatch.setattr(portable_proof, "_tools", lambda: tools)
    reached = []

    def body(args, guard):
        guard.check()
        active = portable_proof._ACTIVE_CUSTODY.get()
        assert active is not None and active[0] is guard
        assert guard.environment()["RUSTC"] == tools["rustc"]["realpath"]
        assert guard.environment()["RUSTC_WRAPPER"] == ""
        assert guard.environment()["QUANTA_INDEX_SCCACHE"] == "0"
        reached.append(args)
        raise ValueError("intentional input refusal")

    monkeypatch.setattr(cp, "_produce_controlled", body)
    with pytest.raises(ValueError, match="input refusal"):
        cp.produce("sentinel")
    assert reached == ["sentinel"]
    assert portable_proof._ACTIVE_CUSTODY.get() is None


@pytest.mark.parametrize("drift", [False, True])
def test_conditional_publishes_only_after_terminal_custody(
    monkeypatch, tmp_path, drift, proof_actor_environment
):
    tools = selected_tools(tmp_path)
    monkeypatch.setattr(portable_proof, "_tools", lambda: tools)
    out = tmp_path / "output"
    out.mkdir()
    result = {"synthetic_wiring_only": True}

    def body(args, guard):
        assert not (out / "results.json").exists()
        if drift:
            Path(tools["cargo"]["path"]).write_text("changed tool", encoding="utf-8")
        return result

    monkeypatch.setattr(cp, "_produce_controlled", body)
    if drift:
        with pytest.raises(tool_custody.ToolCustodyError):
            cp.produce(Namespace(out=out))
        assert not (out / "results.json").exists()
        assert not (out / "results.pending.json").exists()
    else:
        assert cp.produce(Namespace(out=out)) == result
        assert cp.load((out / "results.json").read_bytes()) == result
        assert (out / "results.json").stat().st_ino == (out / "results.pending.json").stat().st_ino
    assert portable_proof._ACTIVE_CUSTODY.get() is None


def test_command_frame_refuses_stderr_changes_against_execution_bytes(tmp_path):
    stdout, stderr = b"actual stdout", b"actual stderr"
    (tmp_path / "stderr").write_bytes(stderr)
    command = {
        "argv": ["/tool"],
        "exit_code": 0,
        "stderr": "stderr",
        "stdout_sha256": cp.sha(stdout),
        "stderr_sha256": cp.sha(stderr),
        "cwd": "/source",
        "inherited_environment": {},
        "environment": {},
        "environment_sha256": cp.sha(b""),
    }
    frame = cp._command_frame(command, tmp_path, stdout)
    assert cp.decode(frame["stdout"]) == stdout
    assert cp.decode(frame["stderr"]) == stderr
    (tmp_path / "stderr").write_bytes(b"substituted")
    with pytest.raises(ValueError, match="bytes differ"):
        cp._command_frame(command, tmp_path, stdout)


def test_command_environment_rejects_ambient_compiler_and_cargo_selection(tmp_path):
    guard = tool_custody.ToolCustody.create(
        cp.ROOT,
        tmp_path / "private-bin",
        tools=selected_tools(tmp_path),
        environment=dict(os.environ),
    )
    inherited = portable_proof._relevant_environment(dict(os.environ))
    context = {"tool_custody": guard.record(), "environment": {"relevant": inherited}}
    overrides = {
        "CARGO_NET_OFFLINE": "true",
        **portable_proof.execution_overrides(guard.tools(), inherited),
    }
    command = {
        "cwd": str(cp.ROOT),
        "inherited_environment": inherited,
        "environment": overrides,
        "environment_sha256": portable_proof._environment_digest({**inherited, **overrides}),
    }
    cp.command_identity(command, context, cp.ROOT)
    for key in (
        "PATH",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "QUANTA_INDEX_SCCACHE",
    ):
        mutant = json.loads(json.dumps(command))
        mutant["environment"][key] = "ambient-override"
        mutant["environment_sha256"] = portable_proof._environment_digest(
            {**inherited, **mutant["environment"]}
        )
        with pytest.raises(ValueError, match="environment"):
            cp.command_identity(mutant, context, cp.ROOT)
