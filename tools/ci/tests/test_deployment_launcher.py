"""Independent selection-wire and real child-lock oracles for deployment."""

from __future__ import annotations

import hashlib
import json
import os
import select
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator

from tools.deployment import launcher

FAKE_SEARCHD = b"""#!/bin/sh
set -eu
if [ "$1" = verify-state ] && [ "$2" = --source ]; then
    [ ! -e "$3/refuse" ] || exit 23
    [ "${OPENAI_API_KEY+x}" != x ] || exit 24
    printf 'verify:%s\\n' "$3" >> "$QUANTA_INDEX_CACHE_ROOT/events"
    exit 0
fi
if [ "$1" = serve ] && [ "$2" = --state-root ]; then
    printf 'serve:%s\\n' "$3" >> "$QUANTA_INDEX_CACHE_ROOT/events"
    echo $$ > "$QUANTA_INDEX_CACHE_ROOT/child-pid"
    while :; do sleep 1; done
fi
exit 25
"""

SIGNAL_SEARCHD = """import os
import signal
import sys
import time
from pathlib import Path

cache = Path(os.environ["QUANTA_INDEX_CACHE_ROOT"])
if sys.argv[1] == "verify-state":
    if (Path(sys.argv[3]) / "refuse").exists():
        raise SystemExit(23)
    with (cache / "events").open("a", encoding="utf-8") as stream:
        stream.write("verify:" + sys.argv[3] + "\\n")
    raise SystemExit(0)
if sys.argv[1] != "serve":
    raise SystemExit(25)
with (cache / "events").open("a", encoding="utf-8") as stream:
    stream.write("serve:" + sys.argv[3] + "\\n")

if (cache / "ignore-signals").exists():
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
else:
    def record(signum, _frame):
        with (cache / "signals").open("a", encoding="utf-8") as stream:
            stream.write(signal.Signals(signum).name + "\\n")

    signal.signal(signal.SIGINT, record)
    signal.signal(signal.SIGTERM, record)

(cache / "child-pid").write_text(str(os.getpid()), encoding="utf-8")
while not (cache / "release").exists():
    time.sleep(0.01)
"""


def fixture(tmp_path: Path) -> tuple[Path, Path, Path, Path, dict[str, str]]:
    deployment = tmp_path / "deployment"
    deployment.mkdir(mode=0o700)
    root = tmp_path / "root"
    root.mkdir(mode=0o700)
    cache = tmp_path / "cache"
    cache.mkdir(mode=0o700)
    binary = tmp_path / "fake-searchd"
    binary.write_bytes(FAKE_SEARCHD)
    binary.chmod(0o700)
    environment = {"QUANTA_INDEX_CACHE_ROOT": str(cache), "PATH": "/usr/bin:/bin"}
    return deployment / "selection.json", binary, root, cache, environment


def raw_selection(binary: Path, root: Path, environment: dict[str, str]) -> dict[str, object]:
    # Fixed independent wire shape and SHA computed from the fixture bytes,
    # without calling the product's encoder or binary-hash helper.
    return {
        "schema_version": 1,
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(FAKE_SEARCHD).hexdigest(),
        "state_root": str(root),
        "environment": environment,
    }


def write_wire(path: Path, payload: str) -> None:
    descriptor = os.open(path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        stream.write(payload)


def read_wire(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


@pytest.mark.parametrize("version", [0, 2, True])
def test_unsupported_selection_version_is_refused(tmp_path: Path, version: int) -> None:
    _path, binary, root, _cache, environment = fixture(tmp_path)
    expected = raw_selection(binary, root, environment)
    expected["schema_version"] = version
    with pytest.raises(launcher.SelectionError, match="unsupported selection format"):
        launcher.selection_from_wire(expected)
    schema = json.loads(Path(launcher.__file__).with_name("selection.schema.json").read_text())
    assert not Draft202012Validator(schema).is_valid(expected)


def test_selection_schema_accepts_fixed_wire_contract(tmp_path: Path) -> None:
    _path, binary, root, _cache, environment = fixture(tmp_path)
    schema = json.loads(Path(launcher.__file__).with_name("selection.schema.json").read_text())
    Draft202012Validator.check_schema(schema)
    Draft202012Validator(schema).validate(raw_selection(binary, root, environment))


@pytest.mark.parametrize("digest", ["A" * 64, "a" * 64 + "\n"])
def test_digest_wire_requires_exact_lowercase_sha256(tmp_path: Path, digest: str) -> None:
    _path, binary, root, _cache, environment = fixture(tmp_path)
    wire = raw_selection(binary, root, environment)
    wire["binary_sha256"] = digest
    schema = json.loads(Path(launcher.__file__).with_name("selection.schema.json").read_text())
    assert not Draft202012Validator(schema).is_valid(wire)
    with pytest.raises(launcher.SelectionError, match="lowercase SHA-256"):
        launcher.selection_from_wire(wire)


def wait_for(path: Path, *, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists():
        if time.monotonic() > deadline:
            raise AssertionError(f"child did not write {path}")
        time.sleep(0.02)


def wait_for_signal(path: Path, expected: str, *, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while True:
        if path.exists() and expected in path.read_text(encoding="utf-8").splitlines():
            return
        if time.monotonic() > deadline:
            raise AssertionError(f"child did not record {expected} in {path}")
        time.sleep(0.02)


def signal_fixture(tmp_path: Path) -> tuple[Path, Path, Path, dict[str, str]]:
    path, binary, root, cache, environment = fixture(tmp_path)
    binary.write_text(f"#!{sys.executable}\n{SIGNAL_SEARCHD}", encoding="utf-8")
    binary.chmod(0o700)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    return path, root, cache, environment


def test_publish_uses_exact_selected_binary_root_and_closed_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    monkeypatch.setenv("OPENAI_API_KEY", "ambient-must-not-enter-child")
    expected = raw_selection(binary, root, environment)
    selection = launcher.proposed_selection(binary, root, environment)
    assert selection.to_wire() == expected
    subprocess.run(
        [
            sys.executable,
            str(Path(launcher.__file__)),
            "install",
            "--selection",
            str(path),
            "--binary",
            str(binary),
            "--state-root",
            str(root),
            "--env",
            f"QUANTA_INDEX_CACHE_ROOT={cache}",
            "--env",
            "PATH=/usr/bin:/bin",
        ],
        check=True,
    )
    assert read_wire(path) == expected
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert (cache / "events").read_text().splitlines() == [f"verify:{root}"]
    assert launcher.verify_bootstrap_selection(path) == selection
    assert (cache / "events").read_text().splitlines() == [f"verify:{root}", f"verify:{root}"]


def test_bad_candidate_or_changed_binary_never_serves_or_replaces(
    tmp_path: Path,
) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    old = launcher.proposed_selection(binary, root, environment)
    launcher.publish_selection(path, old)
    before = path.read_bytes()
    bad = tmp_path / "bad"
    bad.mkdir(mode=0o700)
    (bad / "refuse").write_bytes(b"x")
    with pytest.raises(launcher.SelectionError, match="verify-state failed"):
        launcher.publish_selection(path, launcher.proposed_selection(binary, bad, environment))
    assert path.read_bytes() == before
    assert (cache / "events").read_text().splitlines() == [f"verify:{root}"]
    binary.write_bytes(FAKE_SEARCHD + b"# mutated\n")
    binary.chmod(0o700)
    with pytest.raises(launcher.SelectionError, match="SHA-256 changed"):
        launcher.verify_bootstrap_selection(path)


@pytest.mark.parametrize("cut", ["after_file_fsync", "after_rename", "after_directory_fsync"])
def test_publish_cut_preserves_old_or_new_complete_wire_and_reports_unknown(
    tmp_path: Path, cut: str
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    old = launcher.proposed_selection(binary, root, environment)
    launcher.publish_selection(path, old)
    old_bytes = path.read_bytes()
    next_root = tmp_path / "next-root"
    next_root.mkdir(mode=0o700)
    next_selection = launcher.proposed_selection(binary, next_root, environment)

    def stop(point: str) -> None:
        if point == cut:
            raise OSError(f"cut: {point}")

    error = launcher.PersistenceUncertain if cut != "after_file_fsync" else launcher.SelectionError
    with pytest.raises(error):
        launcher.publish_selection(path, next_selection, fault_hook=stop)
    observed = path.read_bytes()
    if cut == "after_file_fsync":
        assert observed == old_bytes
    else:
        assert observed != old_bytes
        assert read_wire(path) == raw_selection(binary, next_root, environment)
    assert read_wire(path) in [old.to_wire(), next_selection.to_wire()]
    assert not list(path.parent.glob(".selection.json.tmp-*"))


@pytest.mark.parametrize("cut", ["after_file_fsync", "after_rename", "after_directory_fsync"])
def test_crash_at_publish_cut_leaves_complete_verifiable_selection(
    tmp_path: Path, cut: str
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    old_wire = raw_selection(binary, root, environment)
    assert read_wire(path) == old_wire
    next_root = tmp_path / "next-root"
    next_root.mkdir(mode=0o700)
    new_wire = raw_selection(binary, next_root, environment)

    # The child acknowledges the cut over a pipe and then waits. Killing it
    # distinguishes a real process crash from an exception unwinding publish.
    wrapper = """import os
import signal
import sys
from tools.deployment import launcher

path, binary, next_root, cache, cut = sys.argv[1:]
selection = launcher.proposed_selection(
    binary, next_root, {"QUANTA_INDEX_CACHE_ROOT": cache, "PATH": "/usr/bin:/bin"}
)

def at_cut(point):
    if point == cut:
        os.write(1, ("cut:" + point + "\\n").encode())
        while True:
            signal.pause()

launcher.publish_selection(path, selection, fault_hook=at_cut)
os.write(1, b"publish-returned\\n")
"""
    repo = Path(launcher.__file__).resolve().parents[2]
    child = subprocess.Popen(
        [
            sys.executable,
            "-c",
            wrapper,
            str(path),
            str(binary),
            str(next_root),
            environment["QUANTA_INDEX_CACHE_ROOT"],
            cut,
        ],
        env=dict(os.environ, PYTHONPATH=str(repo)),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        assert child.stdout is not None
        ready, _, _ = select.select([child.stdout], [], [], 5)
        assert ready, f"publisher did not reach {cut}"
        assert os.read(child.stdout.fileno(), 128) == f"cut:{cut}\n".encode()
        assert child.poll() is None
        child.kill()
        stdout, stderr = child.communicate(timeout=5)
        assert child.returncode == -signal.SIGKILL, stderr.decode(errors="replace")
        assert stdout == b"", "publisher unexpectedly acknowledged success"

        expected = old_wire if cut == "after_file_fsync" else new_wire
        assert read_wire(path) == expected
        metadata = path.lstat()
        assert stat.S_ISREG(metadata.st_mode)
        assert stat.S_IMODE(metadata.st_mode) == 0o600
        assert metadata.st_uid == os.geteuid()
        assert metadata.st_nlink == 1
        with launcher.deployment_lock(path):
            pass
        assert launcher.verify_bootstrap_selection(path).to_wire() == expected
        # A pre-rename crash may leave a private temp file. There is no
        # automatic cleanup or rollback in this cooperative contract.
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)


def test_strict_wire_and_private_path_refusals(tmp_path: Path) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    expected = raw_selection(binary, root, environment)
    unknown = dict(expected, extra=True)
    for candidate in (
        json.dumps(unknown),
        json.dumps(expected).replace(
            '"schema_version": 1,', '"schema_version": 1, "schema_version": 1,'
        ),
        json.dumps(dict(expected, environment={"DYLD_INSERT_LIBRARIES": "/bad"})),
        json.dumps(dict(expected, environment={"QUANTA_INDEX_STATE_ROOT": str(root)})),
        json.dumps(dict(expected, binary=str(binary) + "\x00bad")),
        json.dumps(dict(expected, binary_sha256="A" * 64)),
    ):
        write_wire(path, candidate)
        with pytest.raises(launcher.SelectionError):
            launcher.verify_bootstrap_selection(path)
    path.unlink()
    path.symlink_to(binary)
    with pytest.raises(launcher.SelectionError):
        launcher.verify_bootstrap_selection(path)
    with pytest.raises(launcher.SelectionError):
        launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    alias_parent = tmp_path / "alias-deployment"
    alias_parent.symlink_to(path.parent, target_is_directory=True)
    with pytest.raises(launcher.SelectionError, match="parent is not canonical"):
        launcher.verify_bootstrap_selection(alias_parent / path.name)


def test_live_child_keeps_cooperative_lock_after_launcher_is_killed(tmp_path: Path) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    selection = launcher.proposed_selection(binary, root, environment)
    launcher.publish_selection(path, selection)
    command = [sys.executable, str(Path(launcher.__file__)), "serve", "--selection", str(path)]
    parent = subprocess.Popen(command)
    child_pid: int | None = None
    try:
        wait_for(cache / "child-pid")
        child_pid = int((cache / "child-pid").read_text())
        assert parent.poll() is None
        assert (cache / "events").read_text().splitlines() == [
            f"verify:{root}",
            f"serve:{root}",
        ]
        with pytest.raises(launcher.DeploymentBusy):
            launcher.publish_selection(path, selection)
        with pytest.raises(launcher.DeploymentBusy):
            launcher.publish_selection(path.with_name("another-selection.json"), selection)
        os.kill(parent.pid, signal.SIGKILL)
        parent.wait(timeout=5)
        with pytest.raises(launcher.DeploymentBusy):
            launcher.publish_selection(path, selection)
    finally:
        if parent.poll() is None:
            parent.kill()
            parent.wait(timeout=5)
        if child_pid is not None:
            try:
                os.kill(child_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                with launcher.deployment_lock(path):
                    break
            except launcher.DeploymentBusy:
                time.sleep(0.05)
        else:
            raise AssertionError("child did not release the deployment lock")


def test_stale_bootstrap_check_does_not_gate_ordinary_serve_restart(tmp_path: Path) -> None:
    path, root, cache, _environment = signal_fixture(tmp_path)
    selected_bytes = path.read_bytes()
    (root / "refuse").write_text("mutable root no longer matches bootstrap", encoding="utf-8")
    with pytest.raises(launcher.SelectionError, match="verify-state failed"):
        launcher.verify_bootstrap_selection(path)
    failed_cli = subprocess.run(
        [
            sys.executable,
            str(Path(launcher.__file__)),
            "verify-bootstrap",
            "--selection",
            str(path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert failed_cli.returncode == 1
    assert "verify-state failed" in failed_cli.stderr

    command = [sys.executable, str(Path(launcher.__file__)), "serve", "--selection", str(path)]
    for _ in range(2):
        (cache / "child-pid").unlink(missing_ok=True)
        (cache / "release").unlink(missing_ok=True)
        parent = subprocess.Popen(command)
        child_pid: int | None = None
        completed = False
        try:
            wait_for(cache / "child-pid")
            child_pid = int((cache / "child-pid").read_text(encoding="utf-8"))
            assert parent.poll() is None
            with pytest.raises(launcher.DeploymentBusy):
                launcher.publish_selection(path, launcher.selection_from_wire(read_wire(path)))
            (cache / "release").write_text("done", encoding="utf-8")
            assert parent.wait(timeout=5) == 0
            completed = True
        finally:
            if parent.poll() is None:
                parent.kill()
                parent.wait(timeout=5)
            if not completed and child_pid is not None:
                try:
                    os.killpg(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    assert path.read_bytes() == selected_bytes
    assert (cache / "events").read_text(encoding="utf-8").splitlines() == [
        f"verify:{root}",
        f"serve:{root}",
        f"serve:{root}",
    ]


@pytest.mark.parametrize("operator_signal", [signal.SIGTERM, signal.SIGINT])
def test_operator_signal_is_forwarded_once_without_extra_termination(
    tmp_path: Path, operator_signal: signal.Signals
) -> None:
    path, _root, cache, _environment = signal_fixture(tmp_path)
    command = [sys.executable, str(Path(launcher.__file__)), "serve", "--selection", str(path)]
    # A terminal sends SIGINT to the foreground group. The selected child
    # has a separate group, so receiving SIGINT proves launcher forwarding.
    parent = subprocess.Popen(command, start_new_session=True)
    child_pid: int | None = None
    try:
        wait_for(cache / "child-pid")
        child_pid = int((cache / "child-pid").read_text(encoding="utf-8"))
        if operator_signal == signal.SIGINT:
            os.killpg(parent.pid, operator_signal)
        else:
            os.kill(parent.pid, operator_signal)
        wait_for_signal(cache / "signals", operator_signal.name)
        with pytest.raises(launcher.DeploymentBusy):
            launcher.verify_bootstrap_selection(path)
        (cache / "release").write_text("done", encoding="utf-8")
        assert parent.wait(timeout=5) == 0
        assert (cache / "signals").read_text(encoding="utf-8").splitlines() == [
            operator_signal.name
        ]
        with launcher.deployment_lock(path):
            pass
    finally:
        if parent.poll() is None:
            parent.kill()
            parent.wait(timeout=5)
        if child_pid is not None:
            try:
                os.killpg(child_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def test_ignored_signal_times_out_and_kills_only_owned_child_group(tmp_path: Path) -> None:
    path, _root, cache, _environment = signal_fixture(tmp_path)
    (cache / "ignore-signals").write_text("yes", encoding="utf-8")
    repo = Path(launcher.__file__).resolve().parents[2]
    wrapper = """import sys
from tools.deployment import launcher

path = sys.argv[1]
with launcher.deployment_lock(path) as lock_fd:
    selection = launcher._read_selection_locked(launcher._selection_path(path))
    launcher._verify_bootstrap_locked(selection, lock_fd)
    try:
        launcher._run_selected_command(
            selection, "serve", lock_fd, shutdown_timeout_seconds=0.3
        )
    except launcher.SelectionError as error:
        if "signal drain deadline" not in str(error):
            raise
        print(error, file=sys.stderr)
        raise SystemExit(47)
raise SystemExit(0)
"""
    environment = dict(os.environ, PYTHONPATH=str(repo))
    parent = subprocess.Popen(
        [sys.executable, "-c", wrapper, str(path)],
        env=environment,
        start_new_session=True,
        stderr=subprocess.PIPE,
        text=True,
    )
    child_pid: int | None = None
    try:
        wait_for(cache / "child-pid")
        child_pid = int((cache / "child-pid").read_text(encoding="utf-8"))
        os.kill(parent.pid, signal.SIGTERM)
        stderr = parent.communicate(timeout=5)[1]
        assert parent.returncode == 47
        assert "owned process group was killed" in stderr
        with pytest.raises(ProcessLookupError):
            os.kill(child_pid, 0)
        with launcher.deployment_lock(path):
            pass
    finally:
        if parent.poll() is None:
            parent.kill()
            parent.wait(timeout=5)
        if child_pid is not None:
            try:
                os.killpg(child_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
