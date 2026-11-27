"""Go release-grammar candidate keeps original syntax and byte custody."""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import pytest

from tools.benchmark.retrieval import source_oracle
from tools.benchmark.retrieval.declaration_parsers import (
    get_parser,
    get_released_go_candidate_parser,
)
from tools.benchmark.retrieval.go_projection import GoProjectionError, GoProjector
from tools.benchmark.retrieval.projection_process import read_bounded_regular, run_bounded

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "tools/benchmark/retrieval/go_projection/main.go"


def test_verified_helper_read_rejects_path_swapped_to_fifo(tmp_path, monkeypatch):
    helper = tmp_path / "helper"
    helper.write_bytes(b"pinned")
    original_open = os.open

    def replace_at_open(path, flags, *args, **kwargs):
        if path == helper:
            assert flags & os.O_NONBLOCK
            helper.unlink()
            os.mkfifo(helper)
        return original_open(path, flags, *args, **kwargs)

    monkeypatch.setattr(os, "open", replace_at_open)
    with pytest.raises(OSError, match="bounded regular file"):
        read_bounded_regular(helper, 64)


@pytest.fixture(scope="module")
def projector_binary(tmp_path_factory):
    if shutil.which("go") is None:
        pytest.skip("pinned Go toolchain unavailable")
    version = subprocess.run(["go", "version"], check=True, capture_output=True, text=True).stdout
    if "go1.25.4" not in version:
        pytest.skip("pinned Go 1.25.4 toolchain unavailable")
    target = tmp_path_factory.mktemp("go-projection") / "projector"
    env = dict(os.environ, GOTOOLCHAIN="local", GOPROXY="off")
    subprocess.run(
        ["go", "build", "-trimpath", "-buildvcs=false", "-o", str(target), str(SOURCE)],
        env=env,
        check=True,
        capture_output=True,
        timeout=60,
    )
    return target, hashlib.sha256(target.read_bytes()).hexdigest()


def test_valid_original_projects_only_call_callees(projector_binary):
    from tree_sitter_language_pack import get_parser

    binary, digest = projector_binary
    source = (
        "package p\r\n// café\r\ntype Shape interface { Area() float64 }\r\n"
        "func Locate(){_=new(1);_=make([]int,2)}\r\n"
    ).encode()
    with GoProjector(binary, binary_sha256=digest) as projector:
        result = projector.project(source, timeout_seconds=5)
    assert result.source == source
    assert len(result.projected) == len(source)
    assert len(result.edits) == 1
    assert source[result.edits[0].start : result.edits[0].end] == b"new"
    assert result.projected[result.edits[0].start : result.edits[0].end] == b"foo"
    assert not get_parser("go").parse(result.projected).root_node.has_error
    name_start = source.index(b"Locate")
    assert not result.overlaps_name(name_start, name_start + len(b"Locate"))


def test_invalid_original_is_refused_before_tree_sitter(projector_binary):
    binary, digest = projector_binary
    with GoProjector(binary, binary_sha256=digest) as projector:
        with pytest.raises(GoProjectionError) as raised:
            projector.project(b"package p\nfunc Broken(){_=new(,2)}\n", timeout_seconds=5)
    assert raised.value.kind == "parse_failure"


def test_binary_digest_is_required_before_execution(projector_binary):
    binary, _digest = projector_binary
    with pytest.raises(GoProjectionError) as raised:
        with GoProjector(binary, binary_sha256="0" * 64):
            pass
    assert raised.value.kind == "binary_identity"


def test_cancelled_operation_never_starts_parser(projector_binary):
    binary, digest = projector_binary
    with GoProjector(binary, binary_sha256=digest) as projector:
        with pytest.raises(GoProjectionError) as raised:
            projector.project(b"package p\n", timeout_seconds=5, cancelled=lambda: True)
    assert raised.value.kind == "cancelled"


def test_corrupt_child_edit_is_rejected(tmp_path):
    child = tmp_path / "invalid-projector"
    child.write_text(
        "#!/bin/sh\n"
        'printf \'%s\\n\' \'{"schema":"quanta-go-grammar-projection-v1",'
        '"status":"ok","go_version":"go1.25.4",'
        '"source_sha256":"bad","edits":[]}\'\n'
    )
    child.chmod(0o700)
    digest = hashlib.sha256(child.read_bytes()).hexdigest()
    with GoProjector(child, binary_sha256=digest) as projector:
        with pytest.raises(GoProjectionError) as raised:
            projector.project(b"package p\n", timeout_seconds=5)
    assert raised.value.kind == "producer_defect"


def test_child_timeout_and_inflight_cancellation_kill_process(tmp_path):
    child = tmp_path / "slow-projector"
    child.write_text("#!/bin/sh\nsleep 5\n")
    child.chmod(0o700)
    digest = hashlib.sha256(child.read_bytes()).hexdigest()
    with GoProjector(child, binary_sha256=digest) as projector:
        with pytest.raises(GoProjectionError) as timed_out:
            projector.project(b"package p\n", timeout_seconds=0.06)
        started = time.monotonic()
        with pytest.raises(GoProjectionError) as cancelled:
            projector.project(
                b"package p\n",
                timeout_seconds=5,
                cancelled=lambda: time.monotonic() - started > 0.06,
            )
    assert timed_out.value.kind == "timeout"
    assert cancelled.value.kind == "cancelled"


@pytest.mark.parametrize("failure", ["callback", "setup"])
def test_child_is_reaped_when_callback_or_setup_raises(monkeypatch, failure):
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(30)"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )

    def fail(*_args):
        raise RuntimeError("injected failure")

    if failure == "setup":
        monkeypatch.setattr(os, "set_blocking", fail)
    with pytest.raises(RuntimeError, match="injected failure"):
        run_bounded(
            child,
            b"source",
            deadline=time.monotonic() + 5,
            cancelled=fail if failure == "callback" else None,
            max_stdout=1024,
        )
    assert child.poll() is not None
    assert all(stream.closed for stream in (child.stdin, child.stdout, child.stderr))


@pytest.mark.parametrize("operation", ["read", "write"])
def test_bounded_child_io_retries_transient_would_block(monkeypatch, operation):
    child = subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read())"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    real_call = getattr(os, operation)
    injected = False

    def would_block_once(*args):
        nonlocal injected
        if not injected:
            injected = True
            raise BlockingIOError()
        return real_call(*args)

    monkeypatch.setattr(os, operation, would_block_once)
    output, errors, code = run_bounded(
        child, b"source", deadline=time.monotonic() + 15, cancelled=None, max_stdout=1024
    )
    assert injected
    assert (output, errors, code) == (b"source", b"", 0)


@pytest.mark.parametrize("stream", ["stdout", "stderr"])
def test_child_output_is_capped_while_reading(tmp_path, stream):
    child = tmp_path / "flood-projector"
    child.write_text(f"#!{sys.executable}\nimport sys\nsys.{stream}.buffer.write(b'x' * 9000000)\n")
    child.chmod(0o700)
    digest = hashlib.sha256(child.read_bytes()).hexdigest()
    with GoProjector(child, binary_sha256=digest) as projector:
        with pytest.raises(GoProjectionError) as raised:
            projector.project(b"package p\n", timeout_seconds=5)
    assert raised.value.kind == "resource_limit"


def test_deep_json_is_a_typed_producer_defect(tmp_path):
    child = tmp_path / "deep-projector"
    child.write_text(f"#!{sys.executable}\nprint('[' * 2000 + ']' * 2000)\n")
    child.chmod(0o700)
    digest = hashlib.sha256(child.read_bytes()).hexdigest()
    with GoProjector(child, binary_sha256=digest) as projector:
        with pytest.raises(GoProjectionError) as raised:
            projector.project(b"package p\n", timeout_seconds=5)
    assert raised.value.kind == "producer_defect"


@pytest.mark.parametrize("status", ["[]", '"ok","status":"ok"'])
def test_ambiguous_or_non_string_status_is_a_producer_defect(tmp_path, status):
    source = b"package p\n"
    digest = hashlib.sha256(source).hexdigest()
    output = (
        '{"schema":"quanta-go-grammar-projection-v1","status":'
        + status
        + ',"go_version":"go1.25.4","source_sha256":"'
        + digest
        + '","edits":[]}'
    )
    child = tmp_path / "bad-status-projector"
    child.write_text(f"#!{sys.executable}\nprint({output!r})\n")
    child.chmod(0o700)
    binary_digest = hashlib.sha256(child.read_bytes()).hexdigest()
    with GoProjector(child, binary_sha256=binary_digest) as projector:
        with pytest.raises(GoProjectionError) as raised:
            projector.project(source, timeout_seconds=15)
    assert raised.value.kind == "producer_defect"


def test_released_go_candidate_census_preserves_original_spans(projector_binary):
    binary, digest = projector_binary
    source = (
        "package p\r\n// café\r\ntype Shape interface { Area() float64 }\r\n"
        "func Locate(){_=new(1);_=make([]int,2)}\r\n"
    ).encode()
    with GoProjector(binary, binary_sha256=digest) as projector:
        rows, projection, extension_digest = source_oracle.candidate_go_declaration_census(
            "pkg.go", source, projector, timeout_seconds=5
        )
    assert [source[start:end] for start, end, *_ in rows] == [b"Shape", b"Area", b"Locate"]
    assert projection.source == source
    assert len(extension_digest) == 64
    assert (
        get_released_go_candidate_parser()[0].parse(projection.projected).root_node.has_error
        is False
    )
    assert get_parser("go").parse(source).root_node.has_error is True


def test_released_go_candidate_census_propagates_syntax_failure(projector_binary):
    binary, digest = projector_binary
    with GoProjector(binary, binary_sha256=digest) as projector:
        with pytest.raises(source_oracle.SourceOracleError, match="producer failure"):
            source_oracle.candidate_go_declaration_census(
                "bad.go", b"package p\nfunc broken( {\n", projector, timeout_seconds=5
            )
