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
        "schema_version": 2,
        "mode": "legacy",
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


NATIVE_SEARCHD = b"""#!/bin/sh
set -eu
[ "$2" = --config ]
[ "$4" = --root-nonce ]
[ "$6" = --manifest-operation ]
[ "$8" = --manifest-object ]
[ "${10}" = --source-cut-digest ]
case "$1" in
  verify-native-state)
    [ "$#" -eq 15 ]
    [ "${12}" = --catalog-content-digest ]
    [ "${14}" = --config-digest ]
    printf 'verify-native:%s:%s\\n' "$3" "${15}" >> "$QUANTA_INDEX_CACHE_ROOT/events"
    exit 0 ;;
  serve-native)
    [ "$#" -eq 13 ]
    [ "${12}" = --config-digest ]
    printf 'serve-native:%s:%s\\n' "$3" "${13}" >> "$QUANTA_INDEX_CACHE_ROOT/events"
    exit 0 ;;
esac
exit 25
"""


def native_config(path: Path, root: Path) -> None:
    path.write_text(json.dumps({"version": 1, "state_root": str(root)}), encoding="utf-8")
    path.chmod(0o600)


def native_receipt() -> launcher.NativeBootReceipt:
    identifier = list(range(16))
    return launcher.NativeBootReceipt.from_wire(
        {
            "root_nonce_hex": "11" * 32,
            "manifest_operation": {
                "workspace": {
                    "tenant": identifier,
                    "collection": identifier,
                    "worktree": identifier,
                    "slot": identifier,
                },
                "epoch": 1,
                "producer": identifier,
                "stream": identifier,
                "stream_epoch": 1,
                "sequence": 1,
            },
            "manifest_object_hex": "22" * 32,
            "source_cut_digest_hex": "33" * 32,
            "install_catalog_digest_hex": "44" * 32,
        }
    )


@pytest.mark.parametrize(
    "name",
    [
        "QUANTA_INDEX_LEXICAL_MERGE_SOFT_RUNS",
        "QUANTA_INDEX_LEXICAL_MERGE_HARD_RUNS",
        "QUANTA_INDEX_LEXICAL_MERGE_SOFT_INDEX_BYTES",
        "QUANTA_INDEX_LEXICAL_MERGE_HARD_INDEX_BYTES",
        "QUANTA_INDEX_LEXICAL_MERGE_MAX_FAN_IN",
        "QUANTA_INDEX_LEXICAL_MERGE_MAX_INPUT_INDEX_BYTES",
        "QUANTA_INDEX_LEXICAL_STAGING_MAX_BYTES",
        "QUANTA_INDEX_LEXICAL_STAGING_MAX_AGE_MS",
        "QUANTA_INDEX_LEXICAL_POOL_MAX_ENCODED_BYTES",
        "QUANTA_INDEX_LEXICAL_POOL_MAX_FILES",
        "QUANTA_INDEX_RETENTION_MEASUREMENT_SCRATCH_BYTES",
        "QUANTA_INDEX_RETENTION_MEASUREMENT_TIMEOUT_MS",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS",
        "QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES",
        "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES",
    ],
)
def test_selection_preserves_declared_resource_policy_knobs(tmp_path: Path, name: str) -> None:
    _path, binary, root, _cache, environment = fixture(tmp_path)
    environment[name] = "17"
    candidate = launcher.proposed_selection(binary, root, environment)
    assert candidate.child_environment()[name] == "17"
    assert candidate.to_wire()["environment"][name] == "17"


@pytest.mark.parametrize(
    "axis", ["bool_epoch", "wide_id", "zero_nonce", "uppercase_object", "unknown_field"]
)
def test_native_receipt_refuses_malformed_identity(axis: str) -> None:
    wire = native_receipt().to_wire()
    if axis == "bool_epoch":
        wire["manifest_operation"]["epoch"] = True
    elif axis == "wide_id":
        wire["manifest_operation"]["producer"] = [256] * 16
    elif axis == "zero_nonce":
        wire["root_nonce_hex"] = "00" * 32
    elif axis == "uppercase_object":
        wire["manifest_object_hex"] = "AB" * 32
    else:
        wire["unbound_scope"] = []
    with pytest.raises(launcher.SelectionError):
        launcher.NativeBootReceipt.from_wire(wire)


@pytest.mark.parametrize("fault", ["duplicate", "missing"])
def test_native_receipt_file_refuses_duplicate_or_incomplete_wire(
    tmp_path: Path, fault: str
) -> None:
    receipt_path = tmp_path / "receipt.json"
    wire = native_receipt().to_wire()
    if fault == "duplicate":
        payload = '{"root_nonce_hex":"' + "11" * 32 + '",' + json.dumps(wire)[1:]
    else:
        del wire["manifest_object_hex"]
        payload = json.dumps(wire)
    write_wire(receipt_path, payload)
    with pytest.raises(launcher.SelectionError):
        launcher._read_native_receipt(receipt_path)


def test_native_selection_refuses_missing_verifier_before_publication(tmp_path: Path) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    original = path.read_bytes()
    config = tmp_path / "native.json"
    native_config(config, root)
    candidate = launcher.proposed_native_selection(
        binary, root, config, environment, native_receipt()
    )
    with pytest.raises(launcher.SelectionError, match="selection transition requires"):
        launcher.publish_selection(path, candidate)
    assert path.read_bytes() == original
    fresh_path = path.with_name("fresh-selection.json")
    with pytest.raises(launcher.SelectionError, match="verify-native-state failed"):
        launcher.publish_selection(fresh_path, candidate)
    assert not fresh_path.exists()


@pytest.mark.parametrize("current_mode", ["legacy", "native"])
def test_same_root_mode_transition_refused_before_verifier(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, current_mode: str
) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    config = tmp_path / "native.json"
    native_config(config, root)
    if current_mode == "native":
        binary.write_bytes(NATIVE_SEARCHD)
        binary.chmod(0o700)
        current = launcher.proposed_native_selection(
            binary, root, config, environment, native_receipt()
        )
        candidate = launcher.proposed_selection(binary, root, environment)
    else:
        current = launcher.proposed_selection(binary, root, environment)
        candidate = launcher.proposed_native_selection(
            binary, root, config, environment, native_receipt()
        )
    launcher.publish_selection(path, current)
    original, events = path.read_bytes(), (cache / "events").read_bytes()

    def unexpected_verifier(_selection, _lock):
        raise AssertionError("mode transition entered verifier")

    monkeypatch.setattr(launcher, "_verify_bootstrap_locked", unexpected_verifier)
    with pytest.raises(launcher.SelectionError, match="selection transition requires"):
        launcher.publish_selection(path, candidate)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_install_native_cli_refuses_same_root_legacy_bypass(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    config = tmp_path / "native.json"
    native_config(config, root)
    receipt = tmp_path / "receipt.json"
    write_wire(receipt, json.dumps(native_receipt().to_wire()))

    def unexpected_verifier(_selection, _lock):
        raise AssertionError("install-native entered verifier")

    monkeypatch.setattr(launcher, "_verify_bootstrap_locked", unexpected_verifier)
    status = launcher.main(
        [
            "install-native",
            "--selection",
            str(path),
            "--binary",
            str(binary),
            "--state-root",
            str(root),
            "--config",
            str(config),
            "--receipt",
            str(receipt),
            *[item for name, value in environment.items() for item in ("--env", f"{name}={value}")],
        ]
    )
    assert status == 1
    assert "selection transition requires" in capsys.readouterr().err
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_native_selection_binds_config_root_digest_and_process_route(tmp_path: Path) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    binary.write_bytes(NATIVE_SEARCHD)
    binary.chmod(0o700)
    config = tmp_path / "native.json"
    native_config(config, root)
    candidate = launcher.proposed_native_selection(
        binary, root, config, environment, native_receipt()
    )
    schema = json.loads(Path(launcher.__file__).with_name("selection.schema.json").read_text())
    Draft202012Validator.check_schema(schema)
    assert Draft202012Validator(schema).is_valid(candidate.to_wire())
    launcher.publish_selection(path, candidate)
    assert read_wire(path) == candidate.to_wire()
    assert launcher.serve_selection(path) == 0
    with pytest.raises(launcher.SelectionError, match="install-only"):
        launcher.verify_bootstrap_selection(path)
    expected_digest = hashlib.sha256(config.read_bytes()).hexdigest()
    assert candidate.native_config_sha256 == expected_digest
    assert (cache / "events").read_text().splitlines() == [
        f"verify-native:{config}:{expected_digest}",
        f"serve-native:{config}:{expected_digest}",
    ]
    native_config(config, tmp_path / "another-root")
    with pytest.raises(launcher.SelectionError, match="SHA-256 changed"):
        launcher.serve_selection(path)
    assert (cache / "events").read_text().splitlines() == [
        f"verify-native:{config}:{expected_digest}",
        f"serve-native:{config}:{expected_digest}",
    ]


def test_native_selection_refuses_config_for_other_root(tmp_path: Path) -> None:
    _path, binary, root, _cache, environment = fixture(tmp_path)
    config = tmp_path / "native.json"
    native_config(config, tmp_path / "different-root")
    with pytest.raises(launcher.SelectionError, match="differs from selected root"):
        launcher.proposed_native_selection(binary, root, config, environment, native_receipt())


def test_persisted_v1_selection_is_refused_before_child_process(tmp_path: Path) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    old = raw_selection(binary, root, environment)
    old["schema_version"] = 1
    del old["mode"]
    write_wire(path, json.dumps(old))
    with pytest.raises(launcher.SelectionError, match="unsupported selection format"):
        launcher.serve_selection(path)
    assert not (cache / "events").exists()


@pytest.mark.parametrize("version", [0, 1, 3, True])
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
def test_interrupted_publish_distinguishes_precommit_from_uncertain_authority(
    tmp_path: Path, cut: str
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    old_wire = raw_selection(binary, root, environment)
    next_root = tmp_path / "next-root"
    next_root.mkdir(mode=0o700)
    candidate = launcher.proposed_selection(binary, next_root, environment)

    def interrupt(point: str) -> None:
        if point == cut:
            raise KeyboardInterrupt

    expected_error = (
        KeyboardInterrupt if cut == "after_file_fsync" else launcher.PersistenceUncertain
    )
    with pytest.raises(expected_error):
        launcher.publish_selection(path, candidate, fault_hook=interrupt)
    expected_wire = (
        old_wire if cut == "after_file_fsync" else raw_selection(binary, next_root, environment)
    )
    assert read_wire(path) == expected_wire
    assert not list(path.parent.glob(".selection.json.tmp-*"))


def test_interruption_between_rename_and_return_reports_uncertain_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    next_root = tmp_path / "next-root"
    next_root.mkdir(mode=0o700)
    candidate = launcher.proposed_selection(binary, next_root, environment)
    replace = os.replace

    def interrupted_replace(source: str | Path, destination: str | Path) -> None:
        replace(source, destination)
        raise KeyboardInterrupt

    monkeypatch.setattr(launcher.os, "replace", interrupted_replace)
    with pytest.raises(launcher.PersistenceUncertain, match="inspect and verify"):
        launcher.publish_selection(path, candidate)
    assert read_wire(path) == raw_selection(binary, next_root, environment)
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
            '"schema_version": 2,', '"schema_version": 2, "schema_version": 2,'
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


def native_switch_fixture(tmp_path: Path):
    path, binary, old_root, cache, environment = fixture(tmp_path)
    binary.write_bytes(NATIVE_SEARCHD)
    new_root = tmp_path / "new-root"
    new_root.mkdir(mode=0o700)
    old_config, new_config = tmp_path / "old.json", tmp_path / "new.json"
    native_config(old_config, old_root)
    native_config(new_config, new_root)
    old = launcher.proposed_native_selection(
        binary, old_root, old_config, environment, native_receipt()
    )
    wire = native_receipt().to_wire()
    wire["root_nonce_hex"] = "55" * 32
    new = launcher.proposed_native_selection(
        binary, new_root, new_config, environment, launcher.NativeBootReceipt.from_wire(wire)
    )
    launcher.publish_selection(path, old)
    return path, old, new, cache


def legacy_to_native_switch_fixture(tmp_path: Path):
    path, old_binary, old_root, cache, environment = fixture(tmp_path)
    old = launcher.proposed_selection(old_binary, old_root, environment)
    launcher.publish_selection(path, old)
    previous = path.with_name("previous-legacy-selection.json")
    expected = independent_selection_fingerprint(path)
    assert launcher.preserve_legacy_selection(path, previous, expected) == expected
    new_binary = tmp_path / "native-searchd"
    new_binary.write_bytes(NATIVE_SEARCHD)
    new_binary.chmod(0o700)
    new_root = tmp_path / "native-root"
    new_root.mkdir(mode=0o700)
    config = tmp_path / "native.json"
    native_config(config, new_root)
    new = launcher.proposed_native_selection(
        new_binary, new_root, config, environment, native_receipt()
    )
    return path, old, new, cache, previous


def independent_selection_fingerprint(path: Path) -> str:
    wire = json.loads(path.read_bytes())
    payload = (json.dumps(wire, sort_keys=True, separators=(",", ":")) + "\n").encode()
    return hashlib.sha256(payload).hexdigest()


def test_preserve_legacy_cli_publishes_private_exact_selection(tmp_path: Path) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    before = path.read_bytes()
    expected = independent_selection_fingerprint(path)
    events = (cache / "events").read_bytes()
    command = [
        sys.executable,
        str(Path(launcher.__file__).resolve()),
        "preserve-legacy",
        "--selection",
        str(path),
        "--previous-selection",
        str(previous),
        "--expected-selection-sha256",
        expected,
    ]
    result = subprocess.run(command, cwd="/", capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"previous_selection_sha256": expected}
    assert previous.read_bytes() == before == path.read_bytes()
    assert stat.S_IMODE(previous.stat().st_mode) == 0o600
    assert (cache / "events").read_bytes() == events
    # A fresh process retries the same canonical Selection, even if the saved
    # file's whitespace changed. The identity is canonical wire, not formatting.
    write_wire(previous, json.dumps(read_wire(previous), indent=2))
    retry = subprocess.run(command, cwd="/", capture_output=True, text=True, timeout=10)
    assert retry.returncode == 0, retry.stderr
    assert json.loads(retry.stdout) == {"previous_selection_sha256": expected}
    assert previous.read_bytes() == before


@pytest.mark.parametrize("boundary", ["file", "directory"])
def test_preserve_legacy_fsync_failure_keeps_authority_classified(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, boundary: str
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    expected = independent_selection_fingerprint(path)
    original_fsync = os.fsync

    def fail_selected_sync(descriptor: int) -> None:
        kind = "directory" if stat.S_ISDIR(os.fstat(descriptor).st_mode) else "file"
        if kind == boundary:
            raise OSError(f"injected {kind} fsync failure")
        original_fsync(descriptor)

    monkeypatch.setattr(launcher.os, "fsync", fail_selected_sync)
    failure = launcher.PersistenceUncertain if boundary == "directory" else launcher.SelectionError
    with pytest.raises(failure):
        launcher.preserve_legacy_selection(path, previous, expected)
    assert independent_selection_fingerprint(path) == expected
    if boundary == "file":
        assert not previous.exists()
    else:
        assert previous.read_bytes() == path.read_bytes()


@pytest.mark.parametrize("changed", ["source", "destination"])
def test_preserve_legacy_rechecks_both_paths_after_precommit_hook(
    tmp_path: Path, changed: str
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    expected = independent_selection_fingerprint(path)
    foreign = read_wire(path)
    foreign["environment"]["LANG"] = "C"
    changed_path = path if changed == "source" else previous

    def change_before_rename(stage: str) -> None:
        if stage == "after_file_fsync":
            write_wire(changed_path, json.dumps(foreign))

    with pytest.raises(launcher.SelectionError):
        launcher.preserve_legacy_selection(
            path, previous, expected, fault_hook=change_before_rename
        )
    assert read_wire(changed_path) == foreign
    if changed == "source":
        assert not previous.exists()
    else:
        assert independent_selection_fingerprint(path) == expected


@pytest.mark.parametrize("boundary", ["after_rename", "after_directory_fsync"])
def test_preserve_legacy_postrename_uncertainty_requires_inspection_and_allows_retry(
    tmp_path: Path,
    boundary: str,
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    expected = independent_selection_fingerprint(path)

    def interrupt(stage: str) -> None:
        if stage == boundary:
            raise KeyboardInterrupt

    with pytest.raises(launcher.PersistenceUncertain):
        launcher.preserve_legacy_selection(path, previous, expected, fault_hook=interrupt)
    assert previous.read_bytes() == path.read_bytes()
    assert launcher.preserve_legacy_selection(path, previous, expected) == expected


@pytest.mark.parametrize(
    "saved", ["foreign", "native", "symlink", "hardlink", "lockname", "source", "outside"]
)
def test_preserve_legacy_refuses_unsafe_destination_without_overwrite(
    tmp_path: Path, saved: str
) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    expected = independent_selection_fingerprint(path)
    if saved == "foreign":
        wire = read_wire(path)
        wire["environment"]["LANG"] = "C"
        write_wire(previous, json.dumps(wire))
    elif saved == "native":
        native_dir = tmp_path / "native-fixture"
        native_dir.mkdir()
        new_path, _old, _new, _cache = native_switch_fixture(native_dir)
        write_wire(previous, new_path.read_text(encoding="utf-8"))
    elif saved == "symlink":
        previous.symlink_to(path)
    elif saved == "hardlink":
        os.link(path, previous)
    elif saved == "lockname":
        previous = path.with_name(launcher.DEPLOYMENT_LOCK_NAME)
    elif saved == "source":
        previous = path
    else:
        previous = tmp_path / "outside-previous-selection.json"
    original = previous.read_bytes() if previous.is_file() else None
    with pytest.raises(launcher.SelectionError):
        launcher.preserve_legacy_selection(path, previous, expected)
    if original is not None:
        assert previous.read_bytes() == original
    assert independent_selection_fingerprint(path) == expected


def test_preserve_legacy_refuses_stale_cas_and_native_source(tmp_path: Path) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    with pytest.raises(launcher.SelectionError, match="current selection changed"):
        launcher.preserve_legacy_selection(path, previous, "00" * 32)
    assert not previous.exists()
    native_dir = tmp_path / "native-fixture"
    native_dir.mkdir()
    native_path, _old, _new, _cache = native_switch_fixture(native_dir)
    with pytest.raises(launcher.SelectionError, match="requires a current Legacy"):
        launcher.preserve_legacy_selection(
            native_path,
            native_path.with_name("previous-legacy-selection.json"),
            independent_selection_fingerprint(native_path),
        )


def test_preserve_legacy_refuses_changed_selected_binary(tmp_path: Path) -> None:
    path, binary, root, _cache, environment = fixture(tmp_path)
    launcher.publish_selection(path, launcher.proposed_selection(binary, root, environment))
    previous = path.with_name("previous-legacy-selection.json")
    expected = independent_selection_fingerprint(path)
    binary.write_bytes(b"changed selected executable")
    with pytest.raises(launcher.SelectionError, match="binary SHA-256 changed"):
        launcher.preserve_legacy_selection(path, previous, expected)
    assert not previous.exists()
    assert independent_selection_fingerprint(path) == expected


def test_legacy_to_fresh_native_switch_uses_cas_and_candidate_verifier(tmp_path: Path) -> None:
    path, old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original = path.read_bytes()
    before = independent_selection_fingerprint(path)
    assert launcher.inspect_selection(path)["mode"] == "legacy"
    # A live Legacy root may have no verify-state manifest. The switch does not
    # submit a second verify-state request against that mutable root.
    launcher.switch_native_selection(
        path,
        new,
        before,
        previous_selection_path=previous,
        expected_previous_selection_sha256=before,
    )
    assert read_wire(path) == new.to_wire()
    events = (cache / "events").read_text().splitlines()
    assert events == [
        f"verify:{old.state_root}",
        f"verify-native:{new.native_config}:{new.native_config_sha256}",
    ]
    assert original != path.read_bytes()
    assert previous.read_bytes() == original
    launcher.rollback_legacy_selection(
        path, previous, independent_selection_fingerprint(path), before
    )
    assert path.read_bytes() == original
    assert (cache / "events").read_text().splitlines() == [
        *events,
        f"verify-native:{new.native_config}:{new.native_config_sha256}",
    ]


@pytest.mark.parametrize("fault", ["missing", "target", "published", "source"])
def test_native_switch_refuses_incomplete_or_foreign_published_handoff_before_verifier(
    tmp_path: Path, fault: str
) -> None:
    path, _old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    expected = independent_selection_fingerprint(path)
    receipt = new.native_boot_receipt
    assert receipt is not None
    operation = receipt.manifest_operation_json
    foreign = json.loads(operation)
    foreign["sequence"] += 1
    foreign_operation = json.dumps(foreign)
    axes = {
        "target_operation_json": foreign_operation if fault == "target" else operation,
        "published_operation_json": foreign_operation if fault == "published" else operation,
        "published_source_cut_digest_hex": (
            "55" * 32 if fault == "source" else receipt.source_cut_digest_hex
        ),
    }
    if fault == "missing":
        axes["published_source_cut_digest_hex"] = None
    with pytest.raises(launcher.SelectionError, match="handoff"):
        launcher.switch_native_selection(
            path,
            new,
            expected,
            previous_selection_path=previous,
            expected_previous_selection_sha256=expected,
            **axes,
        )
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_native_switch_compares_actual_published_handoff_with_verified_receipt(
    tmp_path: Path,
) -> None:
    path, _old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    expected = independent_selection_fingerprint(path)
    receipt = new.native_boot_receipt
    assert receipt is not None
    launcher.switch_native_selection(
        path,
        new,
        expected,
        previous_selection_path=previous,
        expected_previous_selection_sha256=expected,
        target_operation_json=receipt.manifest_operation_json,
        published_operation_json=receipt.manifest_operation_json,
        published_source_cut_digest_hex=receipt.source_cut_digest_hex,
    )
    assert read_wire(path) == new.to_wire()
    assert (cache / "events").read_text().splitlines()[-1].startswith("verify-native:")


def test_native_switch_cli_forwards_published_handoff_before_verifier(tmp_path: Path) -> None:
    path, _old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    receipt = tmp_path / "candidate-receipt.json"
    candidate = new.native_boot_receipt
    assert candidate is not None
    write_wire(receipt, json.dumps(candidate.to_wire()))
    expected = independent_selection_fingerprint(path)
    assert (
        launcher.main(
            [
                "switch-native",
                "--selection",
                str(path),
                "--expected-selection-sha256",
                expected,
                "--previous-selection",
                str(previous),
                "--expected-previous-selection-sha256",
                expected,
                "--binary",
                str(new.binary),
                "--state-root",
                str(new.state_root),
                "--config",
                str(new.native_config),
                "--receipt",
                str(receipt),
                "--target-operation",
                candidate.manifest_operation_json,
                "--published-operation",
                candidate.manifest_operation_json,
                "--published-source-cut-digest",
                "55" * 32,
                *[item for name, value in new.environment for item in ("--env", f"{name}={value}")],
            ]
        )
        == 1
    )
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


@pytest.mark.parametrize(
    "fault",
    ["stale_cas", "candidate_refusal", "old_binary_changed", "backup_mismatch", "backup_alias"],
)
def test_legacy_to_native_switch_refusal_preserves_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fault: str
) -> None:
    path, old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    expected = independent_selection_fingerprint(path)
    if fault == "stale_cas":
        expected = "00" * 32
    elif fault == "old_binary_changed":
        old.binary.write_bytes(b"changed selected binary")
    elif fault == "backup_mismatch":
        wire = read_wire(previous)
        wire["environment"]["LANG"] = "C"
        write_wire(previous, json.dumps(wire))
    elif fault == "backup_alias":
        previous = path
    else:

        def reject_candidate(selection, _lock):
            assert selection == new
            raise launcher.SelectionError("candidate Native verifier refused")

        monkeypatch.setattr(launcher, "_verify_bootstrap_locked", reject_candidate)
    with pytest.raises(launcher.SelectionError):
        launcher.switch_native_selection(
            path,
            new,
            expected,
            previous_selection_path=previous,
            expected_previous_selection_sha256=independent_selection_fingerprint(path),
        )
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


@pytest.mark.parametrize("fault", ["stale_cas", "backup_mismatch", "native_verifier"])
def test_legacy_rollback_refusal_preserves_native_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fault: str
) -> None:
    path, _old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    old_digest = independent_selection_fingerprint(path)
    launcher.switch_native_selection(
        path,
        new,
        old_digest,
        previous_selection_path=previous,
        expected_previous_selection_sha256=old_digest,
    )
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    current_digest = independent_selection_fingerprint(path)
    if fault == "stale_cas":
        current_digest = "00" * 32
    elif fault == "backup_mismatch":
        previous.write_bytes(b"changed snapshot")
    else:

        def reject_native(selection, _lock):
            assert selection == new
            raise launcher.SelectionError("current Native verifier refused")

        monkeypatch.setattr(launcher, "_verify_bootstrap_locked", reject_native)
    with pytest.raises(launcher.SelectionError):
        launcher.rollback_legacy_selection(path, previous, current_digest, old_digest)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_legacy_cutover_postrename_uncertainty_uses_saved_selection_for_recovery(
    tmp_path: Path,
) -> None:
    path, old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    old_digest = independent_selection_fingerprint(path)

    def interrupt(stage: str) -> None:
        if stage == "after_rename":
            raise KeyboardInterrupt()

    with pytest.raises(launcher.PersistenceUncertain):
        launcher.switch_native_selection(
            path,
            new,
            old_digest,
            previous_selection_path=previous,
            expected_previous_selection_sha256=old_digest,
            fault_hook=interrupt,
        )
    assert read_wire(path) == new.to_wire()
    with pytest.raises(launcher.SelectionError, match="current selection changed"):
        launcher.switch_native_selection(
            path,
            new,
            old_digest,
            previous_selection_path=previous,
            expected_previous_selection_sha256=old_digest,
        )
    launcher.rollback_legacy_selection(
        path, previous, independent_selection_fingerprint(path), old_digest
    )
    assert read_wire(path) == old.to_wire()


def test_legacy_cutover_and_rollback_cli_consume_saved_selection(tmp_path: Path) -> None:
    path, old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    receipt = tmp_path / "new-receipt.json"
    write_wire(receipt, json.dumps(new.native_boot_receipt.to_wire()))
    old_digest = independent_selection_fingerprint(path)
    assert (
        launcher.main(
            [
                "switch-native",
                "--selection",
                str(path),
                "--expected-selection-sha256",
                old_digest,
                "--previous-selection",
                str(previous),
                "--expected-previous-selection-sha256",
                old_digest,
                "--binary",
                str(new.binary),
                "--state-root",
                str(new.state_root),
                "--config",
                str(new.native_config),
                "--receipt",
                str(receipt),
                *[item for name, value in new.environment for item in ("--env", f"{name}={value}")],
            ]
        )
        == 0
    )
    assert read_wire(path) == new.to_wire()
    assert (
        launcher.main(
            [
                "rollback-legacy",
                "--selection",
                str(path),
                "--expected-selection-sha256",
                independent_selection_fingerprint(path),
                "--previous-selection",
                str(previous),
                "--expected-previous-selection-sha256",
                old_digest,
            ]
        )
        == 0
    )
    assert read_wire(path) == old.to_wire()


def test_native_cross_root_switch_and_explicit_rollback(tmp_path: Path) -> None:
    path, old, new, cache = native_switch_fixture(tmp_path)
    old_wire = read_wire(path)
    before = independent_selection_fingerprint(path)
    assert launcher.inspect_selection(path) == {
        "selection_sha256": before,
        "mode": "native",
        "state_root": str(old.state_root),
    }
    launcher.switch_native_selection(path, new, before)
    switched = read_wire(path)
    assert switched["state_root"] == str(new.state_root)
    assert switched["native_boot_receipt"]["root_nonce_hex"] == "55" * 32
    events = (cache / "events").read_text().splitlines()
    assert len(events) == 3
    assert events[1].startswith(f"verify-native:{old.native_config}:")
    assert events[2].startswith(f"verify-native:{new.native_config}:")
    launcher.switch_native_selection(path, old, independent_selection_fingerprint(path))
    assert read_wire(path) == old_wire
    assert len((cache / "events").read_text().splitlines()) == 5


@pytest.mark.parametrize("axis", ["stale_cas", "same_root", "nested", "copied_nonce", "workspace"])
def test_native_switch_refuses_foreign_or_stale_candidate_before_verifier(
    tmp_path: Path, axis: str
) -> None:
    from dataclasses import replace

    path, old, new, cache = native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    expected = independent_selection_fingerprint(path)
    if axis == "stale_cas":
        expected = "00" * 32
    elif axis == "same_root":
        new = old
    elif axis == "nested":
        nested = old.state_root / "nested"
        nested.mkdir(mode=0o700)
        config = tmp_path / "nested.json"
        native_config(config, nested)
        new = launcher.proposed_native_selection(
            new.binary, nested, config, dict(new.environment), new.native_boot_receipt
        )
    else:
        wire = new.native_boot_receipt.to_wire()
        if axis == "copied_nonce":
            wire["root_nonce_hex"] = old.native_boot_receipt.root_nonce_hex
        else:
            wire["manifest_operation"]["workspace"]["tenant"] = [99] * 16
        new = replace(new, native_boot_receipt=launcher.NativeBootReceipt.from_wire(wire))
    with pytest.raises(launcher.SelectionError):
        launcher.switch_native_selection(path, new, expected)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


@pytest.mark.parametrize("which", ["current", "candidate"])
def test_native_switch_verifier_refusal_preserves_selection(
    tmp_path: Path, monkeypatch, which: str
) -> None:
    path, old, new, _cache = native_switch_fixture(tmp_path)
    original = path.read_bytes()
    visits = []

    def verify(selection, _lock):
        visits.append(selection.state_root)
        if selection == (old if which == "current" else new):
            raise launcher.SelectionError("fixed stale/incomplete receipt refusal")

    monkeypatch.setattr(launcher, "_verify_bootstrap_locked", verify)
    with pytest.raises(launcher.SelectionError, match="stale/incomplete"):
        launcher.switch_native_selection(path, new, independent_selection_fingerprint(path))
    assert path.read_bytes() == original
    assert visits == ([old.state_root] if which == "current" else [old.state_root, new.state_root])


def test_native_cross_root_install_cannot_bypass_switch_cas(tmp_path: Path) -> None:
    path, _old, new, cache = native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    with pytest.raises(launcher.SelectionError, match="requires switch-native"):
        launcher.publish_selection(path, new)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_native_switch_rechecks_cas_after_preflight(tmp_path: Path) -> None:
    path, _old, new, _cache = native_switch_fixture(tmp_path)
    expected = independent_selection_fingerprint(path)
    competing = read_wire(path)
    competing["environment"]["LANG"] = "C"

    def change_current(stage):
        if stage == "after_file_fsync":
            write_wire(path, json.dumps(competing))

    with pytest.raises(launcher.SelectionError, match="current selection changed"):
        launcher.switch_native_selection(path, new, expected, fault_hook=change_current)
    assert read_wire(path) == competing
    assert not list(path.parent.glob(".selection.json.tmp-*"))


def test_native_switch_postrename_interrupt_requires_new_inspection(tmp_path: Path) -> None:
    path, _old, new, _cache = native_switch_fixture(tmp_path)
    expected = independent_selection_fingerprint(path)

    def interrupt(stage):
        if stage == "after_rename":
            raise KeyboardInterrupt()

    with pytest.raises(launcher.PersistenceUncertain):
        launcher.switch_native_selection(path, new, expected, fault_hook=interrupt)
    assert read_wire(path)["state_root"] == str(new.state_root)
    with pytest.raises(launcher.SelectionError, match="current selection changed"):
        launcher.switch_native_selection(path, new, expected)


def test_native_inspection_does_not_emit_environment_secrets(tmp_path: Path, capsys) -> None:
    from dataclasses import replace

    path, old, _new, _cache = native_switch_fixture(tmp_path)
    secret = "private-provider-secret"
    old = replace(old, environment=tuple(sorted((*old.environment, ("OPENAI_API_KEY", secret)))))
    launcher.publish_selection(path, old)
    assert launcher.main(["inspect-selection", "--selection", str(path)]) == 0
    output = capsys.readouterr().out
    assert secret not in output and "OPENAI_API_KEY" not in output
    assert json.loads(output)["selection_sha256"] == independent_selection_fingerprint(path)
