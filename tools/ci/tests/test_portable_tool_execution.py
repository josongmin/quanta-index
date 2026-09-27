"""Independent lifecycle and publication oracles for controlled proof execution."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from tools.benchmark.evidence import RawFile, write_raw_file
from tools.benchmark.retrieval import portable_proof
from tools.ci.tests import test_portable_proof as producer_fixtures
from tools.ci.tests.test_tool_custody import custody

fake_execution = producer_fixtures.fake_execution
proof_actor_environment = producer_fixtures.proof_actor_environment


@pytest.mark.parametrize("failed_stage", range(7))
def test_each_sdk_subcommand_failure_blocks_context_publication(
    fake_execution, monkeypatch, failed_stage
):
    out, _, calls = fake_execution
    execute = portable_proof.execute
    observed = []

    def fail_one(argv, **kwargs):
        observed.append(argv)
        if len(observed) - 1 == failed_stage:
            raise ValueError("producer failed with exit 23")
        return execute(argv, **kwargs)

    monkeypatch.setattr(portable_proof, "execute", fail_one)
    with pytest.raises(ValueError, match="exit 23"):
        portable_proof.produce("sdk", out)
    assert len(observed) == failed_stage + 1
    assert len(calls) == failed_stage
    assert not (out / "execution-context.json").exists()
    assert not (out / "sdk_receipt.json").exists()


@pytest.mark.parametrize("name", ["rust-build.stdout", "metadata.stdout"])
@pytest.mark.parametrize("mutation", ["restore", "replace", "symlink", "content"])
def test_reuse_input_mutation_cannot_publish_context(fake_execution, monkeypatch, name, mutation):
    out, _, _ = fake_execution
    execute = portable_proof.execute

    def mutate_during_run(argv, **kwargs):
        if "nextest" not in argv or "run" not in argv:
            return execute(argv, **kwargs)
        path = out / name
        original, stamp = path.read_bytes(), path.stat()
        result = execute(argv, **kwargs)
        if mutation == "restore":
            path.write_bytes(b"different build selection")
            path.write_bytes(original)
            os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        elif mutation == "replace":
            replacement = out / "replacement"
            replacement.write_bytes(original)
            replacement.replace(path)
        elif mutation == "symlink":
            replacement = out / "replacement"
            replacement.write_bytes(original)
            path.unlink()
            path.symlink_to(replacement)
        else:
            path.write_bytes(b"different build selection")
        return result

    monkeypatch.setattr(portable_proof, "execute", mutate_during_run)
    with pytest.raises(ValueError, match="nextest reuse input"):
        portable_proof.produce("sdk", out)
    assert not (out / "execution-context.json").exists()
    assert not (out / "sdk_receipt.json").exists()


@pytest.mark.parametrize("name", ["rust-build.stdout", "metadata.stdout"])
def test_changed_reuse_input_is_refused_before_child_launch(tmp_path, monkeypatch, name):
    build, metadata = b"original build", b"original metadata"
    (tmp_path / "rust-build.stdout").write_bytes(build)
    (tmp_path / "metadata.stdout").write_bytes(metadata)
    build = RawFile.capture(tmp_path / "rust-build.stdout")
    metadata = RawFile.capture(tmp_path / "metadata.stdout")
    (tmp_path / name).write_bytes(b"foreign build selection")
    monkeypatch.setattr(portable_proof, "_run", lambda *_a, **_k: pytest.fail("child launched"))
    with pytest.raises(ValueError, match="nextest reuse input"):
        portable_proof._run_reused_nextest(
            "/wrapper", tmp_path, [], build, metadata, env_overrides={}
        )


def test_live_tool_environment_reaches_real_child(tmp_path):
    inherited = dict(os.environ)
    guard = custody(tmp_path, environment=inherited)
    output = tmp_path / "output"
    output.mkdir()
    token = portable_proof._ACTIVE_CUSTODY.set((guard, inherited))
    commands = []
    try:
        raw = portable_proof._run(
            "observed",
            [
                sys.executable,
                "-c",
                'import json,os; print(json.dumps({k:os.environ[k] for k in ("PATH","RUSTC","RUSTC_WRAPPER","RUSTC_WORKSPACE_WRAPPER","QUANTA_INDEX_SCCACHE")}))',
            ],
            output,
            commands,
        )
    finally:
        portable_proof._ACTIVE_CUSTODY.reset(token)
    actual = json.loads(raw.read_control())
    assert actual["PATH"].split(os.pathsep)[0] == str(guard.bin_dir)
    assert actual["RUSTC"] == guard.tools()["rustc"]["realpath"]
    assert actual["RUSTC_WRAPPER"] == actual["RUSTC_WORKSPACE_WRAPPER"] == ""
    assert actual["QUANTA_INDEX_SCCACHE"] == "0"
    assert commands[0]["environment_sha256"] == portable_proof._environment_digest(
        portable_proof._relevant_environment({**inherited, **actual, "CARGO_NET_OFFLINE": "true"})
    )


def test_pre_execution_guard_refuses_changed_tool_without_launch(tmp_path, monkeypatch):
    inherited = dict(os.environ)
    guard = custody(tmp_path, environment=inherited)
    Path(guard.tools()["cargo"]["realpath"]).write_text("replaced")
    monkeypatch.setattr(
        portable_proof, "execute", lambda *_a, **_k: pytest.fail("changed tool launched")
    )
    token = portable_proof._ACTIVE_CUSTODY.set((guard, inherited))
    try:
        with pytest.raises(ValueError, match="custody"):
            portable_proof._run("refused", [sys.executable, "-V"], tmp_path, [])
    finally:
        portable_proof._ACTIVE_CUSTODY.reset(token)


def test_post_execution_guard_refuses_restored_tool_and_emits_no_success(tmp_path, monkeypatch):
    inherited = dict(os.environ)
    guard = custody(tmp_path, environment=inherited)
    path = Path(guard.tools()["cargo"]["realpath"])
    original = path.read_bytes()
    stamp = path.stat()
    execute = portable_proof.execute

    def replace_and_restore(*args, **kwargs):
        result = execute(*args, **kwargs)
        path.write_bytes(b"wrong selected tool")
        path.write_bytes(original)
        os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        return result

    monkeypatch.setattr(portable_proof, "execute", replace_and_restore)
    out = tmp_path / "output"
    out.mkdir()
    commands = []
    token = portable_proof._ACTIVE_CUSTODY.set((guard, inherited))
    try:
        with pytest.raises(ValueError, match="custody"):
            portable_proof._run("refused", [sys.executable, "-V"], out, commands)
    finally:
        portable_proof._ACTIVE_CUSTODY.reset(token)
    assert not commands and not list(out.iterdir())


def test_inherited_environment_drift_blocks_launch(tmp_path, monkeypatch):
    inherited = dict(os.environ)
    guard = custody(tmp_path, environment=inherited)
    token = portable_proof._ACTIVE_CUSTODY.set((guard, inherited))
    monkeypatch.setenv("RUSTFLAGS", "changed after custody")
    monkeypatch.setattr(
        portable_proof, "execute", lambda *_a, **_k: pytest.fail("changed environment launched")
    )
    try:
        with pytest.raises(ValueError, match="inherited environment changed"):
            portable_proof._run("refused", [sys.executable, "-V"], tmp_path, [])
    finally:
        portable_proof._ACTIVE_CUSTODY.reset(token)


def test_command_cannot_override_selected_compiler(tmp_path):
    inherited = dict(os.environ)
    guard = custody(tmp_path, environment=inherited)
    token = portable_proof._ACTIVE_CUSTODY.set((guard, inherited))
    try:
        with pytest.raises(ValueError, match="override selected tool"):
            portable_proof._run(
                "refused",
                [sys.executable, "-V"],
                tmp_path,
                [],
                env_overrides={"RUSTC": "/wrong/compiler"},
            )
    finally:
        portable_proof._ACTIVE_CUSTODY.reset(token)


def test_source_change_after_passing_commands_cannot_publish_context(fake_execution, monkeypatch):
    out, _, _ = fake_execution
    revisions = iter(["b" * 40, "c" * 40])
    monkeypatch.setattr(portable_proof, "_source_revision", lambda: next(revisions))
    with pytest.raises(ValueError, match="source revision changed"):
        portable_proof.produce("sdk", out)
    assert not (out / "execution-context.json").exists()
    pending = out / "execution-context.pending.json"
    assert pending.is_file()
    with pytest.raises(ValueError, match="unpublished"):
        portable_proof.validate(pending)


def test_receipt_writer_failure_cannot_publish_context(fake_execution, monkeypatch):
    out, _, _ = fake_execution
    execute = portable_proof.execute

    def fail_writer(argv, **kwargs):
        if argv[1] == str(portable_proof.RECEIPT_WRITER):
            raise ValueError("receipt writer failed with exit 31")
        return execute(argv, **kwargs)

    monkeypatch.setattr(portable_proof, "execute", fail_writer)
    with pytest.raises(ValueError, match="exit 31"):
        portable_proof.produce("sdk", out)
    assert not (out / "execution-context.json").exists()


def test_publication_does_not_replace_preexisting_context(fake_execution, monkeypatch):
    out, _, _ = fake_execution

    execute = portable_proof.execute

    def replace_destination(argv, **kwargs):
        result = execute(argv, **kwargs)
        if argv[1] == str(portable_proof.SOURCE_CLOSURE_SCRIPT) and argv[2] == "verify":
            (out / "execution-context.json").write_bytes(b"foreign context")
        return result

    monkeypatch.setattr(portable_proof, "execute", replace_destination)
    with pytest.raises(FileExistsError):
        portable_proof.produce("sdk", out)
    assert (out / "execution-context.json").read_bytes() == b"foreign context"


@pytest.mark.parametrize(
    "key,value",
    [
        ("BASH_ENV", "wrong-startup"),
        ("ENV", "wrong-startup"),
        ("BASH_FUNC_cargo%%", "() { printf wrong; }"),
        ("BASH_FUNC_python3%%", ""),
        ("BASH_FUNC_malformed", ""),
    ],
)
def test_producer_rejects_startup_and_functions_before_tool_selection(
    tmp_path, monkeypatch, key, value
):
    monkeypatch.setenv(key, value)
    monkeypatch.setattr(
        portable_proof, "_tools", lambda: pytest.fail("tool selected before environment admission")
    )
    with pytest.raises(ValueError, match="environment|Bash|shell|startup"):
        portable_proof.produce("sdk", tmp_path / "proof")
    assert not (tmp_path / "proof").exists()


def test_final_source_verification_failure_cannot_publish_context(fake_execution, monkeypatch):
    out, _, _ = fake_execution
    execute = portable_proof.execute

    def fail_source_verify(argv, **kwargs):
        if argv[1] == str(portable_proof.SOURCE_CLOSURE_SCRIPT) and argv[2] == "verify":
            raise ValueError("terminal source verification failed")
        return execute(argv, **kwargs)

    monkeypatch.setattr(portable_proof, "execute", fail_source_verify)
    with pytest.raises(ValueError, match="source verification failed"):
        portable_proof.produce("sdk", out)
    assert not (out / "execution-context.json").exists()
    with pytest.raises(ValueError, match="unpublished"):
        portable_proof.validate(out / "execution-context.pending.json")


def test_command_digests_bind_execute_bytes_instead_of_reopened_paths(tmp_path, monkeypatch):
    import hashlib

    stdout, stderr = b"actual executed output", b"actual executed errors"

    def execute(*_args, **kwargs):
        return (
            write_raw_file(kwargs["log_dir"] / "stdout", [stdout]),
            write_raw_file(kwargs["log_dir"] / "stderr", [stderr]),
            {"exit_code": 0},
        )

    monkeypatch.setattr(portable_proof, "execute", execute)
    copy = RawFile.copy_to

    def substitute(self, path):
        result = copy(self, path)
        path.write_bytes(b"replaced after persistence")
        return result

    monkeypatch.setattr(RawFile, "copy_to", substitute)
    commands = []
    retained = portable_proof._run("observed", [sys.executable, "-V"], tmp_path, commands)
    assert retained.sha256 == "sha256:" + hashlib.sha256(stdout).hexdigest()
    with pytest.raises(ValueError, match="commitment"):
        retained.read_control()
    assert commands[0]["stdout_sha256"] == hashlib.sha256(stdout).hexdigest()
    assert commands[0]["stderr_sha256"] == hashlib.sha256(stderr).hexdigest()
    assert (
        commands[0]["stdout_sha256"]
        != hashlib.sha256((tmp_path / "observed.stdout").read_bytes()).hexdigest()
    )


def test_pythonpath_cannot_select_external_pytest_producer(tmp_path, monkeypatch):
    (tmp_path / "pytest.py").write_text('print("wrong-producer")')
    monkeypatch.setattr(
        portable_proof, "_tools", lambda: pytest.fail("tool selection reached with injected pytest")
    )
    # The runner's canonical dot path is also forbidden for the producer;
    # opt-in positive fixtures cannot weaken this admission boundary.
    for key, value in (
        ("PYTHONPATH", "."),
        ("PYTHONPATH", str(tmp_path)),
        ("PYTHONHOME", str(tmp_path)),
        ("PYTEST_ADDOPTS", "-k injected"),
        ("PYTEST_PLUGINS", "injected"),
    ):
        with monkeypatch.context() as actor:
            actor.delenv("PYTHONPATH", raising=False)
            actor.setenv(key, value)
            with pytest.raises(ValueError, match="Python startup"):
                with portable_proof.controlled_execution():
                    pytest.fail("injected producer admitted")


def test_sdk_binds_both_native_executables_before_test(fake_execution, monkeypatch):
    out, runner, _ = fake_execution
    create = portable_proof.ToolCustody.create
    seen = []

    def observe_create(*args, **kwargs):
        guard = create(*args, **kwargs)
        guard.bind_executable = lambda path, **options: (
            seen.append((str(path), options.get("expected_sha256"))) or {}
        )
        return guard

    monkeypatch.setattr(portable_proof.ToolCustody, "create", observe_create)
    execute = portable_proof.execute

    def observe_test(argv, **kwargs):
        if argv[3:5] == ["nextest", "run"]:
            bound = {path for path, digest in seen if digest is not None}
            expected_tests = portable_proof.selected_test_binaries(
                (out / "rust-collection.stdout").read_bytes()
            )
            assert bound == {
                str(runner),
                str(runner.with_name("quanta-index-searchd")),
                *(str(path) for path in expected_tests.values()),
            }
        return execute(argv, **kwargs)

    monkeypatch.setattr(portable_proof, "execute", observe_test)
    context = portable_proof.produce("sdk", out)
    records = json.loads(context.read_bytes())["binaries"]
    assert {(row["path"], row["sha256"]) for row in records.values()} <= set(seen)


def test_installed_tool_version_probes_allow_normal_access_and_bind_real_files():
    import hashlib
    import platform

    tools = portable_proof._tools()
    assert set(tools) == {
        "python",
        "cargo",
        "rustc",
        "cargo-nextest",
        "git",
        "bash",
        "just",
        "cargow",
    }
    assert tools["python"]["version"] == "Python " + platform.python_version()
    for row in tools.values():
        assert Path(row["path"]).resolve(strict=True) == Path(row["realpath"])
        assert row["sha256"] == hashlib.sha256(Path(row["realpath"]).read_bytes()).hexdigest()
        assert row["version"]
    assert Path(tools["cargo"]["realpath"]).name == "cargo"
    assert Path(tools["rustc"]["realpath"]).name == "rustc"
    assert tools["cargo"]["realpath"] != tools["rustc"]["realpath"]
