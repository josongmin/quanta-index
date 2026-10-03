"""Focused checks for the shell-free retrieval proof prerequisite producer."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import stat
import sys
import zipfile
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark.evidence import write_raw_file
from tools.benchmark.retrieval import portable_proof
from tools.benchmark.retrieval import run as pairrun


def _write_external_zip(path, members, *, compressed=False, symlink=False):
    """Independent stdlib fixture with the streaming archive's fixed metadata."""

    class NonSeekable:
        def __init__(self, handle):
            self.handle = handle

        def tell(self):
            return self.handle.tell()

        def write(self, data):
            return self.handle.write(data)

        def flush(self):
            self.handle.flush()

    with path.open("wb") as handle:
        with zipfile.ZipFile(NonSeekable(handle), "w") as archive:
            for name, data in members:
                entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                entry.create_system = 3
                entry.external_attr = (
                    stat.S_IFLNK | 0o777 if symlink else stat.S_IFREG | 0o600
                ) << 16
                entry.compress_type = zipfile.ZIP_DEFLATED if compressed else zipfile.ZIP_STORED
                entry.file_size = len(data)
                with archive.open(entry, "w") as sink:
                    sink.write(data)


def _paired_context_fixture(fake_execution, rail, *, large_log=False):
    out, _, _ = fake_execution
    path = portable_proof.produce(rail, out)
    context = json.loads(path.read_text())
    if large_log:
        name = "source-closure.stderr"
        log = out / name
        log.unlink()
        raw = write_raw_file(log, [b"x" * (1024 * 1024)] * 20)
        context["commands"][0]["stderr_sha256"] = raw.sha256.removeprefix("sha256:")
        path.write_text(json.dumps(context))
    spec = {"receipts": {f"{rail}_execution_context": str(path)}}
    kwargs = {
        "rail": rail,
        "raw": {
            name: out / name for name in context["raw_evidence"] if name != "source-closure.json"
        },
    }
    if rail == "sdk":
        kwargs.update(
            runner_sha=context["binaries"]["runner"]["sha256"],
            searchd_sha=context["binaries"]["searchd"]["sha256"],
        )
    return out, spec, kwargs


@pytest.mark.parametrize("rail", ["contract", "sdk"])
@pytest.mark.parametrize("phase", ["freeze", "verify"])
def test_paired_context_streams_large_transcripts(fake_execution, monkeypatch, rail, phase):
    out, spec, kwargs = _paired_context_fixture(fake_execution, rail, large_log=True)
    stage = out.parent / "paired"

    def no_whole_reads():
        monkeypatch.setattr(Path, "read_bytes", lambda *_: pytest.fail("whole file read"))
        monkeypatch.setattr(
            zipfile.ZipFile, "read", lambda *_a, **_k: pytest.fail("whole ZIP entry read")
        )

    if phase == "freeze":
        no_whole_reads()
    frozen = pairrun.freeze_receipts(spec, stage)
    no_whole_reads()
    result = pairrun._verify_execution_context(
        Path(frozen[f"{rail}_execution_context"]),
        out / "source-closure.json",
        Path(frozen[f"{rail}_execution_logs"]),
        **kwargs,
    )
    assert result["revision"] == "b" * 40


@pytest.mark.parametrize("rail", ["contract", "sdk"])
@pytest.mark.parametrize(
    "mutation",
    [
        "missing",
        "extra",
        "duplicate",
        "reordered",
        "alias",
        "compressed",
        "symlink",
        "crc",
        "truncated",
        "payload_limit",
        "directory_limit",
        "envelope_limit",
    ],
)
def test_paired_context_archive_refuses_mutants(tmp_path, monkeypatch, rail, mutation):
    names = sorted(
        f"{name}.{stream}"
        for name in pairrun.CONTEXT_COMMAND_NAMES[rail]
        for stream in ("stdout", "stderr")
    )
    if mutation == "missing":
        names.pop()
    elif mutation == "extra":
        names.append("unexpected.stdout")
        names.sort()
    elif mutation == "duplicate":
        names[-1] = names[-2]
    elif mutation == "reordered":
        names.reverse()
    elif mutation == "alias":
        names[0] = "../escape"
    path = tmp_path / "logs.zip"

    def write():
        _write_external_zip(
            path,
            [(name, b"transcript") for name in names],
            compressed=mutation == "compressed",
            symlink=mutation == "symlink",
        )

    if mutation == "duplicate":
        with pytest.warns(UserWarning, match="Duplicate name"):
            write()
    else:
        write()
    if mutation == "crc":
        path.write_bytes(path.read_bytes().replace(b"transcript", b"trXnscript", 1))
    elif mutation == "truncated":
        path.write_bytes(path.read_bytes()[:-10])
    elif mutation == "payload_limit":
        monkeypatch.setattr(pairrun, "MAX_CONTEXT_LOG_BYTES", 10 * len(names) - 1)
    elif mutation == "directory_limit":
        monkeypatch.setattr(pairrun, "CONTEXT_ZIP_DIRECTORY_BYTES", 1)
        monkeypatch.setattr(
            zipfile, "ZipFile", lambda *_a, **_k: pytest.fail("parsed before metadata admission")
        )
    elif mutation == "envelope_limit":
        monkeypatch.setattr(pairrun, "MAX_CONTEXT_LOG_BYTES", 1)
        monkeypatch.setattr(pairrun, "CONTEXT_ZIP_OVERHEAD_BYTES", 1)
        monkeypatch.setattr(
            zipfile, "ZipFile", lambda *_a, **_k: pytest.fail("parsed oversized archive")
        )
    with pytest.raises(pairrun.RunError):
        with pairrun._frozen_context_logs(path, rail):
            pytest.fail("mutated command logs admitted")
    assert not (tmp_path.parent / "escape").exists()


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_paired_context_payload_ceiling_is_inclusive(tmp_path, monkeypatch, rail):
    names = sorted(
        f"{name}.{stream}"
        for name in pairrun.CONTEXT_COMMAND_NAMES[rail]
        for stream in ("stdout", "stderr")
    )
    path = tmp_path / "logs.zip"
    _write_external_zip(path, [(name, b"ten bytes!") for name in names])
    monkeypatch.setattr(pairrun, "MAX_CONTEXT_LOG_BYTES", 10 * len(names))
    with pairrun._frozen_context_logs(path, rail) as logs:
        assert set(logs) == set(names)
        assert sum(log.size for log in logs.values()) == pairrun.MAX_CONTEXT_LOG_BYTES
        assert {log.sha256 for log in logs.values()} == {
            "sha256:" + hashlib.sha256(b"ten bytes!").hexdigest()
        }
        extracted = next(iter(logs.values())).path
    assert not extracted.exists()


@pytest.mark.parametrize(
    "mutation", ["log_symlink", "binary_symlink", "receipt_symlink", "oversized", "source_changed"]
)
def test_paired_freeze_refuses_unsafe_or_changed_sources(fake_execution, monkeypatch, mutation):
    out, spec, _ = _paired_context_fixture(fake_execution, "contract")
    context = json.loads((out / "execution-context.json").read_text())
    if mutation.endswith("symlink"):
        source = (
            out / "source-closure.stderr"
            if mutation == "log_symlink"
            else Path(next(iter(context["binaries"].values()))["path"])
            if mutation == "binary_symlink"
            else out / "execution-context.json"
        )
        moved = source.with_name(source.name + "-real")
        source.rename(moved)
        source.symlink_to(moved)
    elif mutation == "oversized":
        monkeypatch.setattr(pairrun, "MAX_CONTEXT_LOG_BYTES", 1)
    else:
        original = pairrun.raw_archive.pack

        def changed(files, target, **kwargs):
            (out / "source-closure.stderr").write_bytes(b"changed after capture")
            return original(files, target, **kwargs)

        monkeypatch.setattr(pairrun.raw_archive, "pack", changed)
    with pytest.raises(pairrun.RunError):
        pairrun.freeze_receipts(spec, out.parent / "paired")


@pytest.mark.parametrize("rail", ["contract", "sdk"])
@pytest.mark.parametrize(
    "mutation", ["raw", "binary", "context", "closure", "archive", "binary_inventory"]
)
def test_paired_verification_rechecks_committed_inputs(fake_execution, monkeypatch, rail, mutation):
    out, spec, kwargs = _paired_context_fixture(fake_execution, rail)
    frozen = pairrun.freeze_receipts(spec, out.parent / "paired")
    context_path = Path(frozen[f"{rail}_execution_context"])
    context = json.loads(context_path.read_text())
    target = {
        "raw": next(iter(kwargs["raw"].values())),
        "binary": context_path.parent / f"{rail}-binaries" / next(iter(context["binaries"])),
        "context": context_path,
        "closure": out / "source-closure.json",
        "archive": Path(frozen[f"{rail}_execution_logs"]),
        "binary_inventory": context_path.parent / f"{rail}-binaries" / "unexpected-binary",
    }[mutation]
    verify = pairrun._verify_context_commands

    def changed(*args):
        verify(*args)
        target.write_bytes(b"mutated after inspection")

    monkeypatch.setattr(pairrun, "_verify_context_commands", changed)
    with pytest.raises(pairrun.RunError, match="changed during verification"):
        pairrun._verify_execution_context(
            context_path,
            out / "source-closure.json",
            Path(frozen[f"{rail}_execution_logs"]),
            **kwargs,
        )


@pytest.mark.parametrize("rail", ["contract", "sdk"])
@pytest.mark.parametrize("control", ["context", "collection", "build"])
def test_paired_context_refuses_oversized_control(fake_execution, monkeypatch, rail, control):
    out, spec, kwargs = _paired_context_fixture(fake_execution, rail)
    context_path = Path(spec["receipts"][f"{rail}_execution_context"])
    target = (
        context_path
        if control == "context"
        else out / ("rust-collection.stdout" if control == "collection" else "rust-build.stdout")
    )
    target.write_bytes(b" " * (16 * 1024 * 1024 + 1))
    if control == "build":
        context = json.loads(context_path.read_text())
        for command in context["commands"]:
            if command["name"] == "rust-build":
                command["stdout_sha256"] = portable_proof._sha(target)
        context_path.write_text(json.dumps(context))
        frozen = pairrun.freeze_receipts(spec, out.parent / "paired")
        with pytest.raises(pairrun.RunError, match="control document exceeds"):
            pairrun._verify_execution_context(
                Path(frozen[f"{rail}_execution_context"]),
                out / "source-closure.json",
                Path(frozen[f"{rail}_execution_logs"]),
                **kwargs,
            )
    else:
        with pytest.raises(pairrun.RunError, match="control document exceeds"):
            pairrun.freeze_receipts(spec, out.parent / "paired")


def test_runner_bundle_uses_shared_file_archive_owner(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "read_bytes", lambda *_: pytest.fail("whole source/bundle read"))
    monkeypatch.setattr(
        zipfile.ZipFile, "read", lambda *_a, **_k: pytest.fail("whole bundle entry read")
    )
    path = tmp_path / "runner.pyz"
    expected = pairrun.build_runner_bundle(path)
    pairrun.validate_runner_bundle(path, expected)
    assert expected["sha256"] == portable_proof._sha(path)


@pytest.mark.parametrize(
    "mutation",
    ["extra", "bootstrap", "compressed", "crc", "reordered", "metadata", "archive_limit"],
)
def test_runner_bundle_rejects_rehashed_unadmitted_inputs(tmp_path, monkeypatch, mutation):
    path = tmp_path / "runner.pyz"
    expected = pairrun.build_runner_bundle(path)
    with zipfile.ZipFile(path) as archive:
        members = {name: archive.read(name) for name in archive.namelist()}
    if mutation == "extra":
        members["zz-extra-payload"] = b"unadmitted payload"
    elif mutation == "bootstrap":
        members["__main__.py"] = b"raise SystemExit(0)\n"
        for row in expected["manifest"]["members"]:
            if row["path"] == "__main__.py":
                row.update(
                    sha256=hashlib.sha256(members["__main__.py"]).hexdigest(),
                    size=len(members["__main__.py"]),
                )
        members["bundle-manifest.json"] = pairrun.canonical_bytes(expected["manifest"]) + b"\n"
        expected["manifest_sha256"] = hashlib.sha256(members["bundle-manifest.json"]).hexdigest()
    _write_external_zip(
        path,
        [(name, members[name]) for name in sorted(members, reverse=mutation == "reordered")],
        compressed=mutation == "compressed",
    )
    if mutation == "crc":
        path.write_bytes(
            path.read_bytes().replace(b"raise SystemExit(main())", b"raise SystemExit(Main())", 1)
        )
    expected["sha256"] = portable_proof._sha(path)
    if mutation in {"metadata", "archive_limit"}:
        monkeypatch.setattr(
            pairrun,
            "RUNNER_BUNDLE_LIMITS",
            pairrun.raw_archive.ArchiveLimits(
                max_bytes=1 if mutation == "archive_limit" else 16 * 1024 * 1024,
                max_entries=6,
                max_directory_bytes=1 if mutation == "metadata" else 16 * 1024,
            ),
        )
        monkeypatch.setattr(
            zipfile, "ZipFile", lambda *_a, **_k: pytest.fail("ZIP parsed before admission")
        )
    else:
        monkeypatch.setattr(
            zipfile.ZipFile,
            "read",
            lambda *_a, **_k: pytest.fail("unadmitted payload materialized"),
        )
    with pytest.raises(
        pairrun.RunError, match="prescribed bootstrap" if mutation == "bootstrap" else None
    ):
        pairrun.validate_runner_bundle(path, expected)


def test_runner_bundle_rechecks_original_archive_after_domain_validation(tmp_path, monkeypatch):
    path = tmp_path / "runner.pyz"
    expected = pairrun.build_runner_bundle(path)
    validate = pairrun._validate_runner_bundle_members

    def changed(*args):
        validate(*args)
        with path.open("ab") as stream:
            stream.write(b"post-validation mutation")

    monkeypatch.setattr(pairrun, "_validate_runner_bundle_members", changed)
    with pytest.raises(pairrun.RunError, match="changed during validation"):
        pairrun.validate_runner_bundle(path, expected)


def test_direct_proof_command_retains_large_output_without_control_decode(tmp_path):
    commands = []
    size = 20 * 1024 * 1024
    result = portable_proof._run(
        "large",
        [sys.executable, "-c", "import sys; sys.stdout.buffer.write(b'x' * (20 * 1024 * 1024))"],
        tmp_path,
        commands,
    )
    assert result.size == size
    assert result.sha256 == "sha256:" + hashlib.sha256(b"x" * size).hexdigest()
    assert result.path == tmp_path / "large.stdout"
    assert commands[0]["stdout_sha256"] == result.sha256.removeprefix("sha256:")


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_large_nextest_output_survives_production_and_relocated_replay(
    fake_execution, monkeypatch, rail
):
    out, _, _ = fake_execution
    execute = portable_proof.execute
    expected_digest = None

    def large_events(argv, **kwargs):
        nonlocal expected_digest
        stdout, stderr, terminal = execute(argv, **kwargs)
        if argv[3:5] == ["nextest", "run"]:
            rows = [json.loads(row) for row in stdout.read_control().splitlines()]
            digest = hashlib.sha256()

            def chunks():
                for row in rows:
                    block = json.dumps({**row, "stdout": "x" * (5 * 1024 * 1024)}).encode() + b"\n"
                    digest.update(block)
                    yield block

            stdout = write_raw_file(kwargs["log_dir"] / "large-events", chunks())
            expected_digest = digest.hexdigest()
        return stdout, stderr, terminal

    monkeypatch.setattr(portable_proof, "execute", large_events)
    receipt = portable_proof.produce(rail, out)
    name = "rust-nextest.jsonl" if rail == "contract" else "nextest.jsonl"
    assert (out / name).stat().st_size > 20 * 1024 * 1024
    summary = out / ("contract_rust_results.json" if rail == "contract" else "sdk_results.json")
    result = json.loads(summary.read_text())
    assert {key: result[key] for key in ("selected", "executed", "passed", "failed")} == {
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
    }
    relocated = out.parent / "relocated"
    shutil.copytree(out, relocated)
    monkeypatch.setattr(
        Path, "read_bytes", lambda *_: pytest.fail("replay materialized whole file")
    )
    context = portable_proof.validate(relocated / receipt.name, execution_root=out)
    assert context["raw_evidence"][name] == expected_digest


def test_direct_proof_command_has_a_finite_execution_timeout(tmp_path, monkeypatch):
    observed = []

    def run(argv, **kwargs):
        observed.append(kwargs.get("timeout"))
        return (
            write_raw_file(kwargs["log_dir"] / "stdout", [b"complete"]),
            write_raw_file(kwargs["log_dir"] / "stderr", [b""]),
            {"exit_code": 0},
        )

    monkeypatch.setattr(portable_proof, "execute", run)
    commands = []
    assert (
        portable_proof._run("fixture", [sys.executable, "-V"], tmp_path, commands).read_control()
        == b"complete"
    )
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
        return (
            write_raw_file(kwargs["log_dir"] / "stdout", [b"fixture-version"]),
            write_raw_file(kwargs["log_dir"] / "stderr", [b""]),
            {"exit_code": 0},
        )

    monkeypatch.setattr(portable_proof, "execute", execute)
    assert portable_proof._git("rev-parse", "HEAD") == "fixture-version"
    commands = []
    portable_proof._run_fresh_recipe(["fixture-just", "recipe"], out, commands)
    assert calls == [(["git", "rev-parse", "HEAD"], 30), (["fixture-just", "recipe"], 7200)]
    assert commands[0]["exit_code"] == 0


def test_git_probe_logs_are_removed_on_success_and_retained_on_failure(tmp_path, monkeypatch):
    import tempfile

    scratch = tmp_path / "system-tmp"
    scratch.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(scratch))
    failing = []

    def execute(argv, **kwargs):
        stdout = write_raw_file(kwargs["log_dir"] / "stdout", [b"fixture-version"])
        stderr = write_raw_file(kwargs["log_dir"] / "stderr", [b""])
        if failing:
            raise ValueError("fixture probe failed")
        return stdout, stderr, {"exit_code": 0}

    monkeypatch.setattr(portable_proof, "execute", execute)
    assert portable_proof._git("rev-parse", "HEAD") == "fixture-version"
    assert not list(scratch.iterdir())
    failing.append(True)
    with pytest.raises(ValueError, match="fixture probe failed"):
        portable_proof._git("rev-parse", "HEAD")
    [retained] = scratch.iterdir()
    assert retained.name.startswith("quanta-proof-git-")
    assert (retained / "stdout").read_bytes() == b"fixture-version"


def test_collected_pytest_identity_normalizes_windows_separator() -> None:
    nodeid = r"tools\ci\tests\test_retrieval_benchmark.py::test_one"
    assert portable_proof.proof_inventory.junit_identity(nodeid) == (
        "tools.ci.tests.test_retrieval_benchmark.test_one"
    )


def _rust_inventory(binary: str, test: str, binary_path: Path) -> bytes:
    return json.dumps(
        {
            "test-count": 1,
            "rust-suites": {
                f"quanta-index-retrieval-bench::{binary}": {
                    "binary-id": f"quanta-index-retrieval-bench::{binary}",
                    "package-id": "fixture-retrieval-package",
                    "build-platform": "target",
                    "package-name": "quanta-index-retrieval-bench",
                    "binary-name": binary,
                    "binary-path": str(binary_path),
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


def test_selected_test_binary_roles_are_deterministic_and_path_bound(tmp_path):
    binary = tmp_path / "compiled"
    raw = _rust_inventory("sdk_roundtrip", "test_one", binary)
    role = "nextest-" + hashlib.sha256(b"quanta-index-retrieval-bench::sdk_roundtrip").hexdigest()
    assert portable_proof.selected_test_binaries(raw) == {role: binary}


@pytest.mark.parametrize(
    "mutation",
    ["missing", "relative", "traversal", "alias", "nul", "id", "duplicate_path", "duplicate_json"],
)
def test_selected_test_binary_inventory_refuses_malformed_paths(tmp_path, mutation):
    payload = json.loads(_rust_inventory("sdk_roundtrip", "test_one", tmp_path / "compiled"))
    suite = next(iter(payload["rust-suites"].values()))
    if mutation == "missing":
        del suite["binary-path"]
    elif mutation == "relative":
        suite["binary-path"] = "target/test"
    elif mutation == "traversal":
        suite["binary-path"] = "/target/../test"
    elif mutation == "alias":
        suite["binary-path"] = "/target//test"
    elif mutation == "nul":
        suite["binary-path"] = "/target/test\x00"
    elif mutation == "id":
        suite["binary-id"] = "different-id"
    elif mutation == "duplicate_path":
        other = {
            **suite,
            "binary-name": "chunking_contract",
            "binary-id": "quanta-index-retrieval-bench::chunking_contract",
        }
        payload["rust-suites"][other["binary-id"]] = other
        payload["test-count"] = 2
    raw = json.dumps(payload).encode()
    if mutation == "duplicate_json":
        raw = raw.replace(b'"test-count": 1', b'"test-count": 1, "test-count": 1')
    with pytest.raises(ValueError):
        portable_proof.selected_test_binaries(raw)


@pytest.mark.parametrize("mutation", ["replace", "restore"])
def test_compiled_executable_custody_refuses_real_epoch_mutants(tmp_path, mutation):
    import os

    path = tmp_path / "compiled"
    path.write_bytes(b"compiled executable marker")
    path.chmod(0o755)
    custody = portable_proof.ToolCustody(tmp_path, tmp_path / "bin", {}, {}, {})
    token = portable_proof._ACTIVE_CUSTODY.set((custody, {}))
    try:
        records = portable_proof._bind_test_binaries(_rust_inventory("sdk_roundtrip", "one", path))
        assert (
            next(iter(records.values()))["sha256"] == hashlib.sha256(path.read_bytes()).hexdigest()
        )
        before = path.stat()
        if mutation == "replace":
            replacement = tmp_path / "replacement"
            replacement.write_bytes(path.read_bytes())
            replacement.chmod(0o755)
            replacement.replace(path)
        else:
            raw = path.read_bytes()
            path.write_bytes(b"different executable marker")
            path.write_bytes(raw)
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        with pytest.raises(ValueError, match="changed"):
            custody.check()
    finally:
        portable_proof._ACTIVE_CUSTODY.reset(token)


@pytest.mark.parametrize(
    "mutation",
    [
        "workspace",
        "target",
        "package",
        "manifest",
        "duplicate_package",
        "path",
        "missing",
        "extra_field",
    ],
)
def test_reused_build_metadata_rejects_consistent_shape_forgeries(fake_execution, mutation):
    out, _, _ = fake_execution
    portable_proof.produce("sdk", out)
    build = json.loads((out / "rust-build.stdout").read_bytes())
    metadata = json.loads((out / "metadata.stdout").read_bytes())
    collection = (out / "rust-collection.stdout").read_bytes()
    assert portable_proof.verify_reused_build(
        json.dumps(build).encode(),
        json.dumps(metadata).encode(),
        collection,
        workspace_root=portable_proof.ROOT,
    )
    if mutation == "workspace":
        metadata["workspace_root"] = "/different/workspace"
    elif mutation == "target":
        metadata["target_directory"] = "/different/target"
    elif mutation == "package":
        metadata["packages"][0]["name"] = "different-package"
    elif mutation == "manifest":
        metadata["packages"][0]["manifest_path"] = "/different/Cargo.toml"
    elif mutation == "duplicate_package":
        metadata["packages"].append(metadata["packages"][0])
    elif mutation == "path":
        next(iter(build["rust-binaries"].values()))["binary-path"] = "/different/executable"
    elif mutation == "missing":
        build["rust-binaries"] = {}
    else:
        build["extra"] = True
    with pytest.raises(ValueError):
        portable_proof.verify_reused_build(
            json.dumps(build).encode(),
            json.dumps(metadata).encode(),
            collection,
            workspace_root=portable_proof.ROOT,
        )


@pytest.mark.parametrize("rail", ["contract", "sdk"])
@pytest.mark.parametrize("mutation", ["missing", "tampered", "path", "legacy"])
def test_context_requires_actual_compiled_test_executable(fake_execution, rail, mutation):
    out, _, _ = fake_execution
    receipt = portable_proof.produce(rail, out)
    context = json.loads(receipt.read_bytes())
    role = next(name for name in context["binaries"] if name.startswith("nextest-"))
    binary = context["binaries"][role]
    if mutation == "missing":
        del context["binaries"][role]
    elif mutation == "tampered":
        Path(binary["path"]).write_bytes(b"wrong compiled executable")
    elif mutation == "path":
        binary["path"] = str(out / "different-binary")
    else:
        context["schema_version"] = 1
    receipt.write_text(json.dumps(context))
    with pytest.raises(ValueError):
        portable_proof.validate(receipt)


@pytest.fixture
def proof_actor_environment(monkeypatch: pytest.MonkeyPatch):
    """Separate the canonical pytest import path from the controlled actor.

    This opt-in boundary only removes the runner's known import setting.
    Other startup/selection overrides still reach the production refusal.
    Monkeypatch restores the runner environment after the requesting test.
    """
    assert os.environ.get("PYTHONPATH") in (None, "."), "noncanonical pytest import path"
    monkeypatch.delenv("PYTHONPATH", raising=False)


@pytest.fixture
def fake_execution(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, proof_actor_environment):
    out = tmp_path / "proof"
    target = tmp_path / "target"
    runner = target / "debug" / "quanta-index-retrieval-bench"
    searchd = target / "debug" / "quanta-index-searchd"
    compiled_test = target / "debug" / "deps" / "compiled-test-fixture"
    compiled_test.parent.mkdir(parents=True)
    compiled_test.write_bytes(b"compiled-test-marker")
    compiled_test.chmod(0o755)
    built = {
        "target": target,
        "runner": runner,
        "searchd": searchd,
        "compiled_test": compiled_test,
        "profile_dir": "debug",
    }
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
        portable_proof.ToolCustody,
        "create",
        lambda *_, **kwargs: SimpleNamespace(
            tools=lambda: tools,
            environment=lambda: {
                **kwargs["environment"],
                **portable_proof.execution_overrides(tools, kwargs["environment"]),
            },
            check=lambda: None,
            bind_executable=lambda *_a, **_k: {},
        ),
    )
    monkeypatch.setattr(portable_proof.source_closure, "load_and_verify", lambda _: None)
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
        digest = hashlib.sha256(built["runner"].read_bytes()).hexdigest()
        (out / "actual-runner-record.json").write_text(
            json.dumps(
                {
                    "schema_version": 5,
                    "span_accounting_version": 1,
                    "captures": {
                        "run": {
                            "runner_binary": {"name": "runner", "digest": digest},
                            "searchd_binary": {
                                "binary_digest": hashlib.sha256(built["searchd"].read_bytes()).hexdigest()
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
        fresh_target = kwargs["env"].get("CARGO_TARGET_DIR")
        if fresh_target is not None:
            profile_dir = "release" if "--release" in argv or built["profile_dir"] == "release" else "debug"
            active_target = Path(fresh_target)
            built.update(
                target=active_target,
                runner=active_target / profile_dir / portable_proof.PACKAGE,
                searchd=active_target / profile_dir / "quanta-index-searchd",
                compiled_test=active_target / profile_dir / "deps" / "compiled-test-fixture",
                profile_dir=profile_dir,
            )
            built["compiled_test"].parent.mkdir(parents=True, exist_ok=True)
            built["compiled_test"].write_bytes(b"compiled-test-marker")
            built["compiled_test"].chmod(0o755)
        if argv[1] == str(portable_proof.SOURCE_CLOSURE_SCRIPT):
            if argv[2] == "capture":
                write_closure()
            raw = b""
        elif argv[1] == str(portable_proof.RECEIPT_WRITER):
            assert (out / "execution-context.pending.json").is_file()
            assert not (out / "execution-context.json").exists()
            assert not Path(argv[argv.index("--out") + 1]).exists()
            write_receipt(argv)
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
            if "--binaries-metadata" in argv:
                prepared = json.loads(
                    Path(argv[argv.index("--binaries-metadata") + 1]).read_bytes()
                )
                binary = next(iter(prepared["rust-binaries"].values()))["binary-name"]
            else:
                binary = "sdk_roundtrip" if "sdk_roundtrip" in argv else "chunking_contract"
            test = portable_proof.sdk_proof.PROOF_TEST if binary == "sdk_roundtrip" else "one"
            raw = _rust_inventory(binary, test, built["compiled_test"])
            if "--list-type" in argv:
                if binary == "sdk_roundtrip":
                    built["runner"].parent.mkdir(parents=True, exist_ok=True)
                    built["runner"].write_bytes(b"runner")
                full = json.loads(raw)
                fields = {
                    "binary-id",
                    "binary-name",
                    "package-id",
                    "kind",
                    "binary-path",
                    "build-platform",
                }
                build_meta = {"target-directory": str(built["target"])}
                if binary == "sdk_roundtrip":
                    build_meta["non-test-binaries"] = {
                        "fixture-retrieval-package": [
                            {
                                "name": portable_proof.PACKAGE,
                                "kind": "bin-exe",
                                "path": f"{built['profile_dir']}/{portable_proof.PACKAGE}",
                            }
                        ]
                    }
                raw = json.dumps(
                    {
                        "rust-build-meta": build_meta,
                        "rust-binaries": {
                            key: {field: value[field] for field in fields}
                            for key, value in full["rust-suites"].items()
                        },
                    }
                ).encode()
        elif argv[3:5] == ["nextest", "run"]:
            assert "--binaries-metadata" in argv and "--cargo-metadata" in argv
            assert "--all-features" not in argv and "--locked" not in argv
            binary = (
                "sdk_roundtrip"
                if (out / "nextest-inventory.json").exists()
                else "chunking_contract"
            )
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
            assert "quanta-index-searchd-runtime" in argv
            built["searchd"].parent.mkdir(parents=True, exist_ok=True)
            built["searchd"].write_bytes(b"searchd")
            raw = b""
        elif argv[3] == "metadata":
            raw = json.dumps(
                {
                    "target_directory": str(built["target"]),
                    "workspace_root": str(portable_proof.ROOT),
                    "packages": [
                        {
                            "id": "fixture-retrieval-package",
                            "name": portable_proof.PACKAGE,
                            "manifest_path": str(
                                portable_proof.ROOT / "benchmarks/retrieval/Cargo.toml"
                            ),
                        }
                    ],
                }
            ).encode()
        else:
            raise AssertionError(argv)
        return (
            write_raw_file(kwargs["log_dir"] / "stdout", [raw]),
            write_raw_file(kwargs["log_dir"] / "stderr", [b""]),
            {"exit_code": 0},
        )

    monkeypatch.setattr(portable_proof, "execute", run)
    return out, runner, calls


def test_fresh_release_sdk_proof_binds_source_and_binaries(fake_execution) -> None:
    out, _legacy_runner, calls = fake_execution
    receipt = portable_proof.produce("sdk", out, build_profile="release-fresh")
    context = portable_proof.validate(receipt)
    assert context["schema_version"] == 3
    assert context["build_profile"] == "release-fresh"
    assert context["revision"] == "b" * 40
    assert context["binaries"]["runner"]["path"] == str(
        out / "target" / "release" / portable_proof.PACKAGE
    )
    assert context["binaries"]["searchd"]["path"] == str(
        out / "target" / "release" / "quanta-index-searchd"
    )
    build_commands = [row for row in context["commands"] if row["name"] in {"build-searchd", "rust-build"}]
    assert len(build_commands) == 2
    assert all("--release" in row["argv"] for row in build_commands)
    assert all(row["environment"]["CARGO_TARGET_DIR"] == str(out / "target") for row in build_commands)
    assert any("--all-features" in argv for argv, _ in calls if "quanta-index-searchd-runtime" in argv)


@pytest.mark.parametrize("occupied", ["directory", "symlink"])
def test_fresh_build_refuses_occupied_target(tmp_path, monkeypatch, occupied) -> None:
    for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "QUANTA_INDEX_SCCACHE"):
        monkeypatch.delenv(key, raising=False)
    out = tmp_path / "proof"
    out.mkdir()
    target = out / "target"
    if occupied == "directory":
        target.mkdir()
    else:
        target.symlink_to(tmp_path, target_is_directory=True)
    with pytest.raises(ValueError, match="target already exists"):
        portable_proof._fresh_build_environment(out)


@pytest.mark.parametrize("variable,value", [
    ("RUSTC_WRAPPER", "/opaque/wrapper"),
    ("RUSTC_WORKSPACE_WRAPPER", "/opaque/wrapper"),
    ("QUANTA_INDEX_SCCACHE", "1"),
])
def test_fresh_build_refuses_unbound_compiler_path(tmp_path, monkeypatch, variable, value) -> None:
    for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "QUANTA_INDEX_SCCACHE"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv(variable, value)
    out = tmp_path / "proof"
    out.mkdir()
    with pytest.raises(ValueError, match="unbound|compiler cache"):
        portable_proof._fresh_build_environment(out)
    assert not (out / "target").exists()


def test_fresh_release_proof_rejects_command_profile_and_binary_tampering(fake_execution) -> None:
    out, _legacy_runner, _calls = fake_execution
    receipt = portable_proof.produce("sdk", out, build_profile="release-fresh")
    canonical = receipt.read_bytes()
    for mutation in (
        lambda row: row.update(build_profile="debug"),
        lambda row: row["commands"][1]["argv"].remove("--all-features"),
        lambda row: row["commands"][1]["environment"].update(CARGO_TARGET_DIR="/other/target"),
    ):
        changed = json.loads(canonical)
        mutation(changed)
        receipt.write_text(json.dumps(changed), encoding="utf-8")
        with pytest.raises(ValueError):
            portable_proof.validate(receipt)
    receipt.write_bytes(canonical)
    runner = out / "target" / "release" / portable_proof.PACKAGE
    runner.write_bytes(b"swapped-runner")
    with pytest.raises(ValueError, match="binary identity changed"):
        portable_proof.validate(receipt)


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
    assert len(calls) == (10 if rail == "contract" else 8)
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
    command = next(row for row in data["commands"] if row["name"] == "rust-test")
    original = command["argv"][4]
    command["argv"][4] = "list"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="prescribed rail"):
        portable_proof.validate(receipt)
    command["argv"][4] = original
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

    data["commands"][0]["argv"][1] = str(portable_proof.SOURCE_CLOSURE_SCRIPT)
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
def test_validator_refuses_mutation_after_file_commitment(fake_execution, monkeypatch, rail):
    out, _, _ = fake_execution
    receipt = portable_proof.produce(rail, out)
    reader = portable_proof.RawFile.capture

    def replace_after_capture(cls, path):
        raw = reader(path)
        path.write_bytes(b"tampered after descriptor capture")
        return raw

    monkeypatch.setattr(portable_proof.RawFile, "capture", classmethod(replace_after_capture))
    with pytest.raises((ValueError, OSError)):
        portable_proof.validate(receipt)


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_validator_refuses_symlink_for_every_consumed_proof_artifact(
    fake_execution, monkeypatch, rail
):
    out, _, _ = fake_execution
    receipt = portable_proof.produce(rail, out)
    reader = portable_proof.RawFile.capture
    consumed = set()

    def track(cls, path):
        if path.parent == out:
            consumed.add(path.name)
        return reader(path)

    monkeypatch.setattr(portable_proof.RawFile, "capture", classmethod(track))
    portable_proof.validate(receipt)
    artifacts = [out / name for name in consumed]
    assert artifacts
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
    receipt.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "revision": "b" * 40,
                "rail": "fixture",
                "tier": "correctness",
                "command": "fixture",
                "evidence_path": str(summary),
                "evidence_sha256": hashlib.sha256(different).hexdigest(),
                "test_event_count": 1,
                "source_closure": closure,
                "input_evidence": [],
            }
        )
    )
    original_json = portable_proof._json

    def replace_after_json(path):
        parsed = original_json(path)
        if path == summary:
            summary.write_bytes(different)
        return parsed

    monkeypatch.setattr(portable_proof, "_json", replace_after_json)
    with pytest.raises(ValueError, match="differs from source and machine evidence"):
        portable_proof._canonical_receipt(
            receipt, rail="fixture", command="fixture", summary=summary, inputs={}, closure=closure
        )
