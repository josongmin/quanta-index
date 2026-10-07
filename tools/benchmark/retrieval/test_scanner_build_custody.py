"""Scanner owner producer boundaries with fixed fixture bytes; root runs later."""

import copy
import fcntl
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import run
from tools.benchmark.retrieval import scanner_build_custody as custody
from tools.benchmark.retrieval.scanner_source_identity import CustodyError


def _git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root)


@pytest.fixture
def admitted(tmp_path, monkeypatch):
    # Fixture paths may exceed the host UDS limit. Dedicated path tests below
    # restore the platform limit and exercise the real runner preflight.
    monkeypatch.setattr(run, "_unix_socket_path_limit", lambda: 4096)
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q")
    wrapper = repo / "scripts" / "cargow"
    wrapper.parent.mkdir()
    wrapper.write_bytes(b"#!/bin/sh\nexit 0\n")
    wrapper.chmod(0o755)
    (repo / "Cargo.toml").write_bytes(b"[workspace]\n")
    _git(repo, "add", ".")
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
    base = _git(repo, "rev-parse", "HEAD").decode().strip()
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    (corpus / "src.rs").write_bytes(b"fn source() {}\n")
    inputs = tmp_path / "inputs"
    inputs.mkdir()
    for name in ("manifest", "suite", "query_pack"):
        (inputs / name).write_bytes(name.encode())
    template = {
        "spec_version": 2,
        "repo": str(corpus),
        "manifest": str(inputs / "manifest"),
        "suite": str(inputs / "suite"),
        "query_pack": str(inputs / "query_pack"),
        "execution_profiles": {"quanta": {"policy": "code_search_file"}},
        "top_k": 10,
        "strategies": [{"name": "fixed_window_strict"}],
        "routes": ["lexical"],
        "scope": "exploratory",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
        "embedder": "hash-dev",
        "query_repetitions_per_root": 2,
        "query_warmup_passes": 1,
        "query_stage_observation": "enabled",
    }
    template_path = inputs / "template.json"
    template_path.write_text(json.dumps(template))
    out = tmp_path / "fresh"
    spec = {
        "schema_version": 2,
        "repo": str(repo),
        "base_git_revision": base,
        "overlay": None,
        "output_root": str(out),
        "run_template": str(template_path),
        "env_overrides": {},
    }
    monkeypatch.setattr(custody, "_tools", lambda *_: {"fixture_tool": "fixed"})
    real_run = subprocess.run

    def owner_command(argv, **kwargs):
        if argv == custody._build_argv(repo):
            for role, relative in custody.BIN_RELPATHS.items():
                path = out / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(role.encode())
            return subprocess.CompletedProcess(argv, 0, b"build", b"")
        if argv == custody._capture_argv(repo, out / "run-spec.json", kwargs["env"]):
            for role, relative in custody.CAP_RELPATHS.items():
                path = out / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(role.encode())
            return subprocess.CompletedProcess(argv, 0, b"capture", b"")
        return real_run(argv, **kwargs)

    monkeypatch.setattr(custody.subprocess, "run", owner_command)
    return spec, tmp_path / "receipt.json"


def test_canonical_build_capture_and_replay(admitted, monkeypatch):
    spec, receipt_path = admitted
    receipt = custody.capture(spec, receipt_path)
    assert receipt["build_argv"] == custody._build_argv(Path(spec["repo"]))
    assert receipt["capture_argv"] == custody._capture_argv(
        Path(spec["repo"]),
        Path(spec["output_root"]) / "run-spec.json",
        custody._effective_env(
            Path(spec["repo"]), Path(spec["output_root"]), spec["env_overrides"]
        ),
    )
    assert custody.verify(receipt) == receipt["binaries"]
    monkeypatch.setenv("UNRELATED_SHELL_CHANGE", "ignored by closed execution environment")
    assert custody.verify(receipt) == receipt["binaries"]


def test_build_resource_controls_are_fixed_despite_inherited_environment(admitted, monkeypatch):
    spec, _ = admitted
    monkeypatch.setenv("CARGO_BUILD_JOBS", "16")
    monkeypatch.setenv("QUANTA_INDEX_TARGET_GC", "1")
    monkeypatch.setenv("QUANTA_INDEX_SCCACHE", "1")
    env = custody._effective_env(Path(spec["repo"]), Path(spec["output_root"]), {})
    assert env["CARGO_BUILD_JOBS"] == "1"
    assert env["QUANTA_INDEX_TARGET_GC"] == "0"
    assert env["QUANTA_INDEX_SCCACHE"] == "0"


@pytest.mark.parametrize(
    "key", ["CARGO_BUILD_JOBS", "QUANTA_INDEX_TARGET_GC", "QUANTA_INDEX_SCCACHE"]
)
def test_fixed_build_controls_cannot_be_overridden(admitted, key):
    spec, _ = admitted
    repo, out = Path(spec["repo"]), Path(spec["output_root"])
    assert custody._effective_env(repo, out, {})[key] in ("0", "1")
    with pytest.raises(CustodyError, match="unsupported|cannot be overridden"):
        custody._effective_env(repo, out, {key: "8"})


def test_capture_waits_for_build_slot_before_starting_leaf(tmp_path):
    repo = Path(__file__).resolve().parents[3]
    cache = tmp_path / "cache"
    lock = cache / "resource-admission/build-test.lock"
    lock.parent.mkdir(parents=True)
    env = {
        "HOME": str(tmp_path),
        "PATH": os.environ["PATH"],
        "QUANTA_INDEX_CACHE_ROOT": str(cache),
        "QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "1",
        "QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS": "5",
    }
    argv = custody._capture_argv(repo, tmp_path / "run-spec.json", env)
    marker = tmp_path / "leaf-started"
    leaf = [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"]
    argv = argv[: argv.index("--") + 1] + leaf
    fd = os.open(lock, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        refused = subprocess.run(argv, env=env, capture_output=True, timeout=10, check=False)
        assert refused.returncode == 124
        assert not marker.exists()
        fcntl.flock(fd, fcntl.LOCK_UN)
        admitted = subprocess.run(argv, env=env, capture_output=True, timeout=10, check=False)
        assert admitted.returncode == 0
        assert marker.is_file()
    finally:
        os.close(fd)


def test_default_capture_lock_matches_cargow_cache_layout(tmp_path):
    repo = Path(__file__).resolve().parents[3]
    env = {"HOME": str(tmp_path), "PATH": os.environ["PATH"]}
    root = subprocess.check_output(
        ["bash", str(repo / "scripts/quanta-index-env.sh")], env=env, text=True
    ).strip()
    argv = custody._capture_argv(repo, tmp_path / "run-spec.json", env)
    assert Path(argv[argv.index("--lock") + 1]) == (
        Path(root) / "resource-admission/build-test.lock"
    )


@pytest.mark.parametrize(
    "overrides",
    [
        {"QUANTA_INDEX_CACHE_ROOT": "relative/cache"},
        {"QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "0"},
        {"QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "-1"},
        {"QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "invalid"},
        {"QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS": "0"},
    ],
)
def test_capture_refuses_invalid_admission_before_build(admitted, overrides):
    spec, receipt_path = admitted
    spec["env_overrides"].update(overrides)
    with pytest.raises(CustodyError, match="scanner resource"):
        custody.capture(spec, receipt_path)
    assert not Path(spec["output_root"]).exists()


@pytest.mark.parametrize("component", ["x" * 120, "é" * 60])
def test_capture_refuses_long_socket_path_before_build(admitted, monkeypatch, component):
    spec, receipt_path = admitted
    out = Path(spec["output_root"]).parent / component
    spec["output_root"] = str(out)
    monkeypatch.setattr(run, "_unix_socket_path_limit", lambda: 103)

    def unexpected_build(*_):
        pytest.fail("socket path refusal must precede build registration")

    monkeypatch.setattr(custody, "_build_argv", unexpected_build)
    with pytest.raises(CustodyError, match="Unix socket path.*limit 103"):
        custody.capture(spec, receipt_path)
    assert not out.exists()
    assert not receipt_path.exists()


def test_capture_resolves_parent_symlink_before_socket_preflight(admitted, monkeypatch):
    spec, receipt_path = admitted
    parent = Path(spec["output_root"]).parent
    target = parent / ("x" * 120)
    target.mkdir()
    alias = parent / "short"
    alias.symlink_to(target, target_is_directory=True)
    out = alias / "fresh"
    spec["output_root"] = str(out)
    monkeypatch.setattr(run, "_unix_socket_path_limit", lambda: 103)

    def unexpected_build(*_):
        pytest.fail("resolved socket path refusal must precede build registration")

    monkeypatch.setattr(custody, "_build_argv", unexpected_build)
    with pytest.raises(CustodyError, match="Unix socket path.*limit 103"):
        custody.capture(spec, receipt_path)
    assert not out.exists()
    assert not receipt_path.exists()


def test_reused_target_and_arbitrary_capture_command_refused(admitted):
    spec, receipt_path = admitted
    Path(spec["output_root"]).mkdir()
    with pytest.raises(CustodyError, match="must be new"):
        custody.capture(spec, receipt_path)
    forged = {**spec, "capture_argvs": [["/bin/true"]]}
    with pytest.raises(CustodyError, match="spec schema differs"):
        custody._validate_spec(forged)


@pytest.mark.parametrize("role", ["runner", "searchd"])
def test_built_binary_swap_refused(admitted, role):
    spec, receipt_path = admitted
    receipt = custody.capture(spec, receipt_path)
    with open(receipt["binaries"][role]["path"], "ab") as stream:
        stream.write(b"tamper")
    with pytest.raises(CustodyError, match="binary drifted"):
        custody.verify(receipt)


@pytest.mark.parametrize("role", ["corpus", "manifest", "suite", "query_pack", "run_template"])
def test_referenced_input_drift_refused(admitted, role):
    spec, receipt_path = admitted
    receipt = custody.capture(spec, receipt_path)
    root = Path(receipt["inputs"][role]["root"])
    changed = root / "src.rs" if root.is_dir() else root
    with changed.open("ab") as stream:
        stream.write(b"tamper")
    with pytest.raises(CustodyError):
        custody.verify(receipt)


def test_generated_run_spec_and_capture_swaps_refused(admitted):
    spec, receipt_path = admitted
    receipt = custody.capture(spec, receipt_path)
    with (Path(spec["output_root"]) / "run-spec.json").open("ab") as stream:
        stream.write(b"tamper")
    with pytest.raises(CustodyError, match="run spec differs"):
        custody.verify(receipt)


def test_capture_symlink_refused(admitted):
    spec, receipt_path = admitted
    receipt = custody.capture(spec, receipt_path)
    record = Path(receipt["capture_outputs"]["record"]["path"])
    saved = record.with_suffix(".saved")
    record.rename(saved)
    record.symlink_to(saved)
    with pytest.raises(CustodyError, match="non-file artifact"):
        custody.verify(receipt)


def test_input_symlink_refused(admitted):
    spec, receipt_path = admitted
    template_path = Path(spec["run_template"])
    template = json.loads(template_path.read_text())
    pack = Path(template["query_pack"])
    saved = pack.with_suffix(".saved")
    pack.rename(saved)
    pack.symlink_to(saved)
    with pytest.raises(CustodyError, match="invalid input root"):
        custody.capture(spec, receipt_path)


def test_boolean_top_k_refused(admitted):
    spec, _ = admitted
    template_path = Path(spec["run_template"])
    template = json.loads(template_path.read_text())
    template["top_k"] = True
    template_path.write_text(json.dumps(template))
    with pytest.raises(CustodyError, match="scanner template"):
        custody._validate_spec(spec)


def test_relevant_environment_and_forged_receipt_refused(admitted, monkeypatch):
    spec, receipt_path = admitted
    receipt = custody.capture(spec, receipt_path)
    monkeypatch.setenv("RUSTFLAGS", "-C opt-level=0")
    with pytest.raises(CustodyError, match="environment or toolchain drifted"):
        custody.verify(receipt)
    monkeypatch.delenv("RUSTFLAGS")
    forged = copy.deepcopy(receipt)
    forged["binaries"]["searchd"]["sha256"] = "0" * 64
    with pytest.raises(CustodyError, match="receipt digest differs"):
        custody.verify(forged)


@pytest.mark.parametrize("value", [True, 1.0, "2"])
def test_malformed_schema_version_refused(admitted, value):
    spec, _ = admitted
    forged = {**spec, "schema_version": value}
    with pytest.raises(CustodyError, match="spec schema differs"):
        custody._validate_spec(forged)
