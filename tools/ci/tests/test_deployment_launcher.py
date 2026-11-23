"""Independent selection-wire and real child-lock oracles for deployment."""

from __future__ import annotations

import hashlib
import json
import os
import select
import signal
import socket
import stat
import subprocess
import sys
import time
from dataclasses import replace
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

NATIVE_CANDIDATE_SEARCHD = b"""#!/bin/sh
set -eu
if [ "$1" = select-native-candidate ]; then
    [ "$#" -eq 13 ]
    [ "$2" = --config ] && [ "$4" = --config-digest ]
    [ "$6" = --root-nonce ] && [ "$8" = --manifest-operation ]
    [ "${10}" = --source-cut-digest ] && [ "${12}" = --receipt ]
    [ ! -e "${13}" ]
    umask 077
    cp "$QUANTA_INDEX_CACHE_ROOT/fixed-receipt.json" "${13}"
    printf 'select-candidate\n' >> "$QUANTA_INDEX_CACHE_ROOT/events"
    exit 0
fi
if [ "$1" = verify-native-candidate ]; then
    [ "$#" -eq 15 ]
    [ "$2" = --config ] && [ "$4" = --root-nonce ]
    [ "$6" = --manifest-operation ] && [ "$8" = --manifest-object ]
    [ "${10}" = --source-cut-digest ] && [ "${12}" = --catalog-content-digest ]
    [ "${14}" = --config-digest ]
    printf 'verify-candidate\n' >> "$QUANTA_INDEX_CACHE_ROOT/events"
    [ ! -e "$QUANTA_INDEX_CACHE_ROOT/refuse-candidate-verify" ] || exit 61
    exit 0
fi
""" + NATIVE_SEARCHD.split(b"set -eu\n", 1)[1]


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


def test_candidate_handoff_preserves_optional_protected_route_identity() -> None:
    receipt = native_receipt().to_wire()
    handoff = {
        "root_nonce_hex": receipt["root_nonce_hex"],
        "manifest_operation": receipt["manifest_operation"],
        "source_cut_digest_hex": receipt["source_cut_digest_hex"],
        "original_full_sequence": 17,
    }
    plain = launcher._candidate_source_handoff(json.dumps(handoff).encode())
    assert plain[-1] is None
    assert "protected_source_routes_hex" not in json.loads(
        launcher._candidate_handoff_envelope("bound", plain)
    )["publication"]
    handoff["protected_source_routes_hex"] = "33" * 32
    routed = launcher._candidate_source_handoff(json.dumps(handoff).encode())
    assert routed[-2] == "33" * 32
    assert launcher._decode_candidate_handoff(
        launcher._candidate_handoff_envelope("bound", routed), "bound"
    ) == routed
    handoff["protected_source_routes_hex"] = "0" * 64
    with pytest.raises(launcher.SelectionError, match="empty identity"):
        launcher._candidate_source_handoff(json.dumps(handoff).encode())


def test_candidate_handoff_preserves_complete_retired_policy_reference() -> None:
    receipt = native_receipt().to_wire()
    handoff = {
        "root_nonce_hex": receipt["root_nonce_hex"],
        "manifest_operation": receipt["manifest_operation"],
        "source_cut_digest_hex": receipt["source_cut_digest_hex"],
        "original_full_sequence": 17,
        "retired_policy_object_hex": "44" * 32,
        "retired_policy_sha256_hex": "55" * 32,
        "retired_policy_identity_count": 4096,
    }
    parsed = launcher._candidate_source_handoff(json.dumps(handoff).encode())
    assert parsed[-1] == ("44" * 32, "55" * 32, 4096)
    assert launcher._decode_candidate_handoff(
        launcher._candidate_handoff_envelope("bound", parsed), "bound"
    ) == parsed
    del handoff["retired_policy_sha256_hex"]
    with pytest.raises(launcher.SelectionError):
        launcher._candidate_source_handoff(json.dumps(handoff).encode())


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


def test_native_receipt_binds_protected_routes_through_selected_command() -> None:
    original = native_receipt()
    wire = original.to_wire()
    assert "protected_source_routes_hex" not in wire
    wire["protected_source_routes_hex"] = "7a" * 32
    protected = launcher.NativeBootReceipt.from_wire(wire)
    assert protected != original
    assert protected.to_wire() == wire
    args = launcher._native_identity_args(protected, install=True)
    assert args[args.index("--protected-source-routes") + 1] == "7a" * 32
    for invalid in (None, "00" * 32, "7A" * 32, "broken"):
        changed = dict(wire, protected_source_routes_hex=invalid)
        with pytest.raises(launcher.SelectionError):
            launcher.NativeBootReceipt.from_wire(changed)


def test_schema3_retired_policy_roundtrip_and_identity_args(tmp_path: Path) -> None:
    _selection_path, binary, root, _cache, environment = fixture(tmp_path)
    config = tmp_path / "native.json"
    native_config(config, root)
    wire = native_receipt().to_wire()
    wire.update(
        retired_policy_object_hex="44" * 32,
        retired_policy_sha256_hex="55" * 32,
        retired_policy_identity_count=4096,
    )
    receipt = launcher.NativeBootReceipt.from_wire(wire)
    selection = launcher.Selection(
        binary, hashlib.sha256(FAKE_SEARCHD).hexdigest(), root,
        tuple(sorted(environment.items())), config,
        hashlib.sha256(config.read_bytes()).hexdigest(), receipt,
    )
    encoded = selection.to_wire()
    assert encoded["schema_version"] == 3
    assert launcher.selection_from_wire(encoded) == selection
    args = launcher._native_identity_args(receipt, install=True)
    assert args[-6:] == [
        "--retired-policy-object", "44" * 32,
        "--retired-policy-sha256", "55" * 32,
        "--retired-policy-identity-count", "4096",
    ]
    encoded["schema_version"] = 2
    with pytest.raises(launcher.SelectionError):
        launcher.selection_from_wire(encoded)
    encoded["schema_version"] = 3
    del encoded["native_boot_receipt"]["retired_policy_sha256_hex"]
    with pytest.raises(launcher.SelectionError):
        launcher.selection_from_wire(encoded)


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


def test_prepare_native_candidate_cli_keeps_active_legacy_and_reuses_exact_document(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch,
) -> None:
    active, binary, old_root, cache, environment = fixture(tmp_path)
    launcher.publish_selection(active, launcher.proposed_selection(binary, old_root, environment))
    before = active.read_bytes()
    root = tmp_path / "candidate-root"
    root.mkdir(mode=0o700)
    image = tmp_path / "candidate-searchd"
    image.write_bytes(NATIVE_CANDIDATE_SEARCHD)
    image.chmod(0o700)
    config = tmp_path / "native.json"
    native_config(config, root)
    receipt = native_receipt()
    deployment = tmp_path / "candidate-deployment"
    deployment.mkdir(mode=0o700)
    receipt_path = deployment / "boot-receipt.json"
    write_wire(receipt_path, json.dumps(receipt.to_wire()))
    candidate = deployment / "candidate.json"
    args = [
        "prepare-native-candidate", "--selection", str(active),
        "--expected-selection-sha256", hashlib.sha256(before).hexdigest(),
        "--candidate", str(candidate), "--binary", str(image),
        "--binary-sha256", hashlib.sha256(NATIVE_CANDIDATE_SEARCHD).hexdigest(),
        "--state-root", str(root), "--config", str(config),
        "--config-sha256", hashlib.sha256(config.read_bytes()).hexdigest(),
        "--receipt", str(receipt_path), "--root-nonce", receipt.root_nonce_hex,
        "--manifest-operation", receipt.manifest_operation_json,
        "--source-cut-digest", receipt.source_cut_digest_hex,
        "--env", f"QUANTA_INDEX_CACHE_ROOT={cache}", "--env", "PATH=/usr/bin:/bin",
    ]

    for flag in (
        "--binary-sha256", "--config-sha256", "--root-nonce", "--source-cut-digest",
        "--expected-selection-sha256",
    ):
        wrong = args.copy()
        wrong[wrong.index(flag) + 1] = "ff" * 32
        assert launcher.main(wrong) == 1
        assert not candidate.exists()
    wrong_operation = args.copy()
    foreign_operation = json.loads(receipt.manifest_operation_json)
    foreign_operation["sequence"] += 1
    wrong_operation[wrong_operation.index("--manifest-operation") + 1] = json.dumps(foreign_operation)
    assert launcher.main(wrong_operation) == 1
    assert not candidate.exists()
    assert (cache / "events").read_text().splitlines() == [f"verify:{old_root}"]
    capsys.readouterr()

    write_wire(cache / "fixed-receipt.json", json.dumps(receipt.to_wire()))
    receipt_path.unlink()
    (cache / "refuse-candidate-verify").touch()
    with launcher.deployment_lock(active):
        assert launcher.main(args) == 1
    assert receipt_path.exists() and not candidate.exists()
    (cache / "refuse-candidate-verify").unlink()
    with launcher.deployment_lock(active):
        assert launcher.main(args) == 0
    payload = candidate.read_bytes()
    metadata = candidate.stat()
    assert stat.S_IMODE(metadata.st_mode) == 0o600
    assert json.loads(capsys.readouterr().out) == {
        "candidate_path": str(candidate),
        "candidate_sha256": hashlib.sha256(payload).hexdigest(),
    }
    assert active.read_bytes() == before
    assert launcher.main(args) == 0
    assert candidate.read_bytes() == payload
    assert candidate.stat().st_ino == metadata.st_ino
    assert (cache / "events").read_text().splitlines().count("select-candidate") == 1
    assert (cache / "events").read_text().splitlines().count("verify-candidate") == 3
    changed = args + ["--env", "LANG=C"]
    assert launcher.main(changed) == 1
    assert candidate.read_bytes() == payload
    assert active.read_bytes() == before

    def assert_refuses_unsynced_rewrite(target: Path) -> None:
        original = target.read_bytes()
        target_inode = target.stat().st_ino
        synced_files = []
        real_fsync = os.fsync

        def rewrite_after_sync(descriptor: int) -> None:
            real_fsync(descriptor)
            observed = os.fstat(descriptor)
            if observed.st_ino == target_inode:
                synced_files.append(observed.st_ino)
                write_wire(target, original.decode() + " ")

        with monkeypatch.context() as injection:
            injection.setattr(launcher.os, "fsync", rewrite_after_sync)
            assert launcher.main(args) == 1
        assert synced_files == [target_inode]
        assert "durability is uncertain" in capsys.readouterr().err
        assert active.read_bytes() == before
        write_wire(target, original.decode())

    for target in [receipt_path, candidate]:
        assert_refuses_unsynced_rewrite(target)


def test_prepare_native_candidate_from_selected_native_keeps_active_head(
    tmp_path: Path, capsys: pytest.CaptureFixture[str],
) -> None:
    active, _binary, old_root, cache, environment = fixture(tmp_path)
    image = tmp_path / "candidate-searchd"
    image.write_bytes(NATIVE_CANDIDATE_SEARCHD)
    image.chmod(0o700)
    old_config = tmp_path / "old-native.json"
    native_config(old_config, old_root)
    old = replace(
        launcher.proposed_selection(image, old_root, environment),
        native_config=old_config,
        native_config_sha256=hashlib.sha256(old_config.read_bytes()).hexdigest(),
        native_boot_receipt=native_receipt(),
    )
    launcher.publish_selection(active, old)
    before = active.read_bytes()
    before_inode = active.stat().st_ino

    root = tmp_path / "next-root"
    root.mkdir(mode=0o700)
    config = tmp_path / "next-native.json"
    native_config(config, root)
    receipt_wire = native_receipt().to_wire()
    receipt_wire["root_nonce_hex"] = "22" * 32
    receipt_wire["manifest_operation"]["sequence"] += 1
    write_wire(cache / "fixed-receipt.json", json.dumps(receipt_wire))
    receipt = launcher.NativeBootReceipt.from_wire(receipt_wire)
    deployment = tmp_path / "next-deployment"
    deployment.mkdir(mode=0o700)
    candidate = deployment / "candidate.json"
    receipt_path = deployment / "boot.json"
    args = [
        "prepare-native-candidate", "--selection", str(active),
        "--expected-selection-sha256", hashlib.sha256(before).hexdigest(),
        "--candidate", str(candidate), "--binary", str(image),
        "--binary-sha256", hashlib.sha256(image.read_bytes()).hexdigest(),
        "--state-root", str(root), "--config", str(config),
        "--config-sha256", hashlib.sha256(config.read_bytes()).hexdigest(),
        "--receipt", str(receipt_path), "--root-nonce", receipt.root_nonce_hex,
        "--manifest-operation", receipt.manifest_operation_json,
        "--source-cut-digest", receipt.source_cut_digest_hex,
        "--env", f"QUANTA_INDEX_CACHE_ROOT={cache}", "--env", "PATH=/usr/bin:/bin",
    ]
    stale = args.copy()
    stale[stale.index("--expected-selection-sha256") + 1] = "ff" * 32
    assert launcher.main(stale) == 1
    assert "current selection changed; inspect before switching" in capsys.readouterr().err
    assert not candidate.exists() and not receipt_path.exists()
    same_root = args.copy()
    same_root[same_root.index("--state-root") + 1] = str(old_root)
    same_root[same_root.index("--config") + 1] = str(old_config)
    same_root[same_root.index("--config-sha256") + 1] = hashlib.sha256(
        old_config.read_bytes()
    ).hexdigest()
    assert launcher.main(same_root) == 1
    assert "switch roots must be distinct and non-nested" in capsys.readouterr().err
    assert not candidate.exists() and not receipt_path.exists()
    assert active.read_bytes() == before
    assert active.stat().st_ino == before_inode

    assert launcher.main(args) == 0
    assert json.loads(capsys.readouterr().out) == {
        "candidate_path": str(candidate),
        "candidate_sha256": hashlib.sha256(candidate.read_bytes()).hexdigest(),
    }
    assert active.read_bytes() == before
    assert active.stat().st_ino == before_inode
    assert (cache / "events").read_text().splitlines() == [
        f"verify-native:{old_config}:{hashlib.sha256(old_config.read_bytes()).hexdigest()}",
        "select-candidate", "verify-candidate",
    ]


@pytest.mark.parametrize(
    "long_endpoint",
    [
        False,
        pytest.param(
            True,
            marks=pytest.mark.skipif(sys.platform != "linux", reason="Linux procfd socket address"),
        ),
    ],
)
@pytest.mark.parametrize("current_mode_v1", ["legacy", "legacy_source", "native"])
def test_build_native_candidate_reaps_before_boot_and_reuses_durable_full(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], long_endpoint: bool,
    current_mode_v1: str,
) -> None:
    active, binary, old_root, cache, environment = fixture(tmp_path)
    deployment = tmp_path / "candidate-deployment"
    deployment.mkdir(mode=0o700)
    root = tmp_path / "candidate-root"
    root.mkdir(mode=0o700)
    endpoint = tmp_path / "candidate-ipc" / "q.sock"
    if long_endpoint:
        parent = tmp_path / ("socket-owner-" * 12)
        endpoint = parent / endpoint.name
    assert not endpoint.parent.exists()
    config = tmp_path / "native.json"
    write_wire(config, json.dumps({"state_root": str(root), "socket_path": str(endpoint)}))
    receipt = native_receipt().to_wire()
    if current_mode_v1 == "native":
        receipt["root_nonce_hex"] = "22" * 32
        receipt["manifest_operation"]["sequence"] += 1
    if current_mode_v1 == "legacy_source":
        receipt["protected_source_routes_hex"] = "33" * 32
    write_wire(cache / "fixed-receipt.json", json.dumps(receipt))
    expected_handoff = {
        field: receipt[field]
        for field in ("root_nonce_hex", "manifest_operation", "source_cut_digest_hex")
    }
    expected_handoff["original_full_sequence"] = 17
    if "protected_source_routes_hex" in receipt:
        expected_handoff["protected_source_routes_hex"] = receipt["protected_source_routes_hex"]

    image = tmp_path / "candidate-searchd"
    python = Path(sys.executable).resolve()
    image.write_text(f"#!{python}\n" + """
import json, os, signal, socket, sys
from pathlib import Path
cache = Path(os.environ["QUANTA_INDEX_CACHE_ROOT"])
with (cache / "events").open("a") as output:
    output.write(sys.argv[1] + "\\n")
if sys.argv[1] == "serve-native-candidate":
    endpoint = Path(json.loads(Path(sys.argv[3]).read_bytes())["socket_path"])
    endpoint.parent.mkdir(mode=0o700, exist_ok=True)
    stopping = False
    def drain(*_):
        global stopping
        stopping = True
    signal.signal(signal.SIGTERM, drain)
    (cache / "owned-pid").write_text(str(os.getpid()))
    directory = None
    address = endpoint
    if len(os.fsencode(endpoint)) >= 108 and sys.platform == "linux":
        directory = os.open(endpoint.parent, os.O_PATH | os.O_DIRECTORY)
        address = Path(f"/proc/self/fd/{directory}") / endpoint.name
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
            server.bind(str(address))
            server.listen()
            server.settimeout(0.1)
            while not stopping:
                try:
                    with server.accept()[0]:
                        pass
                except socket.timeout:
                    continue
    finally:
        endpoint.unlink()
        if directory is not None:
            os.close(directory)
    refused = (cache / "refuse-drain").exists()
    with (cache / "events").open("a") as output:
        output.write("drain-refused\\n" if refused else "drain-complete\\n")
    sys.exit(47 if refused else 0)
elif sys.argv[1] == "select-native-candidate":
    try:
        os.kill(int((cache / "owned-pid").read_text()), 0)
    except ProcessLookupError:
        pass
    else:
        sys.exit(49)
    assert not Path(json.loads(Path(sys.argv[3]).read_bytes())["socket_path"]).exists()
    assert json.loads(sys.argv[9]) == json.loads((cache / "fixed-receipt.json").read_bytes())["manifest_operation"]
    if not (cache / "allow-boot").exists():
        sys.exit(25)
    os.umask(0o077)
    Path(sys.argv[13]).write_bytes((cache / "fixed-receipt.json").read_bytes())
elif sys.argv[1] == "verify-native-candidate":
    assert len(sys.argv) in {16, 18}
    assert sys.argv[12] == "--catalog-content-digest"
    if len(sys.argv) == 18:
        assert sys.argv[-4:-2] == ["--protected-source-routes", "33" * 32]
elif sys.argv[1] != "verify-native-state":
    sys.exit(3)
""")
    image.chmod(0o700)
    if current_mode_v1 == "native":
        old_config = tmp_path / "old-native.json"
        native_config(old_config, old_root)
        old = replace(
            launcher.proposed_selection(image, old_root, environment),
            native_config=old_config,
            native_config_sha256=hashlib.sha256(old_config.read_bytes()).hexdigest(),
            native_boot_receipt=native_receipt(),
        )
    else:
        old = launcher.proposed_selection(binary, old_root, environment)
    launcher.publish_selection(active, old)
    before = active.read_bytes()
    producer = tmp_path / "producer"
    producer.write_text(f"#!{python}\n" + """
import json, os, subprocess, sys
from hashlib import sha256
from pathlib import Path
assert sys.argv[1] == "--prepare-native-candidate-full"
flags = dict(zip(sys.argv[2::2], sys.argv[3::2]))
cache = Path(os.environ["FIXTURE_CACHE"])
assert flags["--native-candidate-q-pid"] == (cache / "owned-pid").read_text()
assert flags["--native-candidate-resident-socket"] == str(cache / "resident.sock")
assert flags["--native-candidate-runtime-session-id"] == "retained-session"
if flags.get("--native-candidate-history-source") in {"snapshot", "native"}:
    active = Path(flags["--native-candidate-active-selection-path"])
    assert active.is_absolute()
    assert flags["--native-candidate-active-selection-sha256"] == sha256(active.read_bytes()).hexdigest()
    config = Path(flags["--native-candidate-q-config"])
    assert config.is_absolute()
    assert flags["--native-candidate-q-config-sha256"] == sha256(config.read_bytes()).hexdigest()
else:
    assert "--native-candidate-active-selection-path" not in flags
    assert "--native-candidate-active-selection-sha256" not in flags
    assert "--native-candidate-q-config" not in flags
    assert "--native-candidate-q-config-sha256" not in flags
if flags.get("--native-candidate-source-mode") == "source-only":
    image = Path(os.environ["FIXTURE_LEGACY_BINARY"])
    assert flags["--native-candidate-source-exporter-executable"] == str(image)
    assert flags["--native-candidate-source-exporter-executable-sha256"] == sha256(image.read_bytes()).hexdigest()
else:
    assert "--native-candidate-source-exporter-executable" not in flags
    assert "--native-candidate-source-exporter-executable-sha256" not in flags
producer_image = Path(sys.argv[0])
assert flags["--quanta-internal-filesystem-scan-worker-path-v1"] == str(producer_image)
assert flags["--quanta-internal-filesystem-scan-worker-sha256-v1"] == sha256(producer_image.read_bytes()).hexdigest()
os.kill(int(flags["--native-candidate-q-pid"]), 0)
assert Path(flags["--native-source-socket"]).is_socket()
lock = Path(os.environ["FIXTURE_DEPLOYMENT_LOCK"]).stat()
for entry in Path("/dev/fd").iterdir():
    try:
        descriptor = os.fstat(int(entry.name))
    except OSError:
        continue
    if os.path.samestat(descriptor, lock):
        break
else:
    raise AssertionError("the FULL producer must retain the candidate deployment lock")
with (cache / "events").open("a") as output:
    output.write("producer\\n")
subprocess.Popen([sys.executable, "-c", "import os,time; from pathlib import Path; "
                  "time.sleep(0.3); Path(os.environ['FIXTURE_CACHE'], 'late-worker').touch()"])
print(os.environ["FIXTURE_HANDOFF"])
""")
    producer.chmod(0o700)
    candidate = deployment / "candidate.json"
    receipt_path = deployment / "boot.json"
    args = [
        "build-native-candidate", "--selection", str(active),
        "--expected-selection-sha256", hashlib.sha256(before).hexdigest(),
        "--candidate", str(candidate), "--binary", str(image),
        "--binary-sha256", hashlib.sha256(image.read_bytes()).hexdigest(),
        "--state-root", str(root), "--config", str(config),
        "--config-sha256", hashlib.sha256(config.read_bytes()).hexdigest(),
        "--receipt", str(receipt_path), "--producer", str(producer),
        "--producer-sha256", hashlib.sha256(producer.read_bytes()).hexdigest(),
        "--env", f"QUANTA_INDEX_CACHE_ROOT={cache}",
        "--producer-env", f"FIXTURE_CACHE={cache}",
        "--producer-env", f"FIXTURE_DEPLOYMENT_LOCK={deployment / launcher.DEPLOYMENT_LOCK_NAME}",
        "--producer-env", f"FIXTURE_HANDOFF={json.dumps(expected_handoff)}",
        "--producer-env", f"FIXTURE_LEGACY_BINARY={binary}",
        "--timeout-seconds", "5", "--", "--native-candidate-expected-snapshot", "1",
        "--native-candidate-resident-socket", str(cache / "resident.sock"),
        "--native-candidate-runtime-session-id", "retained-session",
    ]
    if current_mode_v1 != "native":
        args.extend(("--native-candidate-history-source", "snapshot"))
    else:
        args.extend(("--native-candidate-history-source", "native"))
    if current_mode_v1 == "legacy_source":
        args.extend(("--native-candidate-source-mode", "source-only"))
    assert launcher.main([
        *args, "--quanta-internal-filesystem-scan-worker-path-v1", str(image),
    ]) == 1
    assert launcher.main([
        *args, "--native-candidate-active-selection-path", str(active),
    ]) == 1
    assert launcher.main([
        *args, "--native-candidate-q-config", str(config),
    ]) == 1
    if current_mode_v1 == "legacy_source":
        assert launcher.main([
            *args, "--native-candidate-source-exporter-executable", str(binary),
        ]) == 1
        assert launcher.main([
            *args, "--native-candidate-source-exporter-executable-sha256", "11" * 32,
        ]) == 1
    assert not endpoint.parent.exists(), "invalid worker pin must fail before starting Q"
    if current_mode_v1 == "native":
        assert launcher.main([*args, "--native-candidate-history-source", "snapshot"]) == 1
        assert not endpoint.parent.exists()
    else:
        assert launcher.main([*args, "--native-candidate-history-source", "native"]) == 1
        assert not endpoint.parent.exists()
    (cache / "refuse-drain").touch()
    with launcher.deployment_lock(active):
        assert launcher.main(args) == 1
        assert not candidate.exists() and not receipt_path.exists() and not endpoint.exists()
        handoff = deployment / ".candidate.json.full-handoff.json"
        retained_handoff = handoff.read_bytes()
        assert not (deployment / ".candidate.json.clean-drain.json").exists()
        retained_events = (cache / "events").read_text()
        assert "select-native-candidate" not in retained_events.splitlines()
        capsys.readouterr()
        foreign_replacements = [
            ("--native-candidate-resident-socket", str(cache / "other.sock")),
            ("--native-candidate-runtime-session-id", "foreign-session"),
        ]
        if current_mode_v1 != "native":
            foreign_replacements.append(("--native-candidate-history-source", "replay"))
        else:
            foreign_replacements.append(("--native-candidate-history-source", "replay"))
        if current_mode_v1 == "legacy_source":
            invalid_mode = args.copy()
            invalid_mode[invalid_mode.index("--native-candidate-source-mode") + 1] = "replay"
            assert launcher.main(invalid_mode) == 1
            assert "candidate Source mode must be source-only" in capsys.readouterr().err
        for flag, replacement in foreign_replacements:
            foreign_args = args.copy()
            foreign_args[foreign_args.index(flag) + 1] = replacement
            assert launcher.main(foreign_args) == 1
            assert "candidate FULL handoff differs from preparation inputs" in capsys.readouterr().err
            assert handoff.read_bytes() == retained_handoff
            assert (cache / "events").read_text() == retained_events
            assert active.read_bytes() == before
            assert not candidate.exists() and not receipt_path.exists() and not endpoint.exists()
        (cache / "refuse-drain").unlink()
        assert launcher.main(args) == 1
        assert (deployment / ".candidate.json.clean-drain.json").exists()
        assert not candidate.exists() and not receipt_path.exists()
        (cache / "allow-boot").touch()
        assert launcher.main(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result == {
        "candidate_path": str(candidate),
        "candidate_sha256": hashlib.sha256(candidate.read_bytes()).hexdigest(),
        "original_full_sequence": 17,
    }
    assert active.read_bytes() == before
    initial_event_v1 = (
        "verify-native-state" if current_mode_v1 == "native" else f"verify:{old_root}"
    )
    assert (cache / "events").read_text().splitlines() == [
        initial_event_v1, "serve-native-candidate", "producer",
        "drain-refused", "serve-native-candidate", "drain-complete",
        "select-native-candidate", "select-native-candidate", "verify-native-candidate",
    ]
    assert launcher.main(args) == 0
    capsys.readouterr()
    assert active.read_bytes() == before
    events_v1 = (cache / "events").read_text().splitlines()
    assert events_v1.count("producer") == 1
    assert events_v1.count("serve-native-candidate") == 2
    assert events_v1.count("select-native-candidate") == 2
    time.sleep(0.5)
    assert not (cache / "late-worker").exists()


@pytest.mark.skipif(sys.platform != "linux", reason="Linux peer credentials")
def test_candidate_readiness_rejects_a_listener_owned_by_another_process(tmp_path: Path) -> None:
    endpoint = tmp_path / "foreign.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as foreign:
        foreign.bind(str(endpoint))
        foreign.listen()
        with subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"]) as child:
            try:
                with pytest.raises(launcher.SelectionError, match="different process"):
                    launcher._wait_candidate_endpoint(child, lambda: False, endpoint, time.monotonic() + 2)
            finally:
                child.terminate()


def test_build_native_candidate_interrupt_during_owned_drain_refuses_boot(tmp_path: Path) -> None:
    active, _binary, old_root, cache, environment = fixture(tmp_path)
    launcher.publish_selection(active, launcher.proposed_selection(_binary, old_root, environment))
    deployment = tmp_path / "candidate-deployment"
    deployment.mkdir(mode=0o700)
    root = tmp_path / "candidate-root"
    root.mkdir(mode=0o700)
    endpoint = tmp_path / "q.sock"
    config = tmp_path / "native.json"
    write_wire(config, json.dumps({"state_root": str(root), "socket_path": str(endpoint)}))
    image = tmp_path / "candidate-searchd"
    python = Path(sys.executable).resolve()
    image.write_text(f"#!{python}\n" + """
import json, os, signal, socket, sys, time
from pathlib import Path
cache = Path(os.environ["QUANTA_INDEX_CACHE_ROOT"])
if sys.argv[1] == "select-native-candidate":
    (cache / "boot-invoked").touch()
    sys.exit(41)
if sys.argv[1] != "serve-native-candidate":
    sys.exit(42)
endpoint = Path(json.loads(Path(sys.argv[3]).read_bytes())["socket_path"])
signal.signal(signal.SIGINT, lambda *_: (cache / "interrupted").touch())
def drain(*_):
    (cache / "draining").touch()
    while not (cache / "release-drain").exists():
        time.sleep(0.01)
    sys.exit(0)
signal.signal(signal.SIGTERM, drain)
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
    server.bind(str(endpoint))
    server.listen()
    while True:
        with server.accept()[0]:
            pass
""")
    image.chmod(0o700)
    producer = tmp_path / "producer"
    handoff = native_receipt().to_wire()
    handoff = {field: handoff[field] for field in (
        "root_nonce_hex", "manifest_operation", "source_cut_digest_hex"
    )}
    handoff["original_full_sequence"] = 17
    producer.write_text(f"#!{python}\n" + """
import os
print(os.environ["FIXTURE_HANDOFF"])
""")
    producer.chmod(0o700)
    candidate = deployment / "candidate.json"
    receipt_path = deployment / "boot.json"
    args = [
        sys.executable, str(Path(launcher.__file__)), "build-native-candidate",
        "--selection", str(active),
        "--expected-selection-sha256", hashlib.sha256(active.read_bytes()).hexdigest(),
        "--candidate", str(candidate), "--binary", str(image),
        "--binary-sha256", hashlib.sha256(image.read_bytes()).hexdigest(),
        "--state-root", str(root), "--config", str(config),
        "--config-sha256", hashlib.sha256(config.read_bytes()).hexdigest(),
        "--receipt", str(receipt_path), "--producer", str(producer),
        "--producer-sha256", hashlib.sha256(producer.read_bytes()).hexdigest(),
        "--env", f"QUANTA_INDEX_CACHE_ROOT={cache}",
        "--producer-env", f"FIXTURE_HANDOFF={json.dumps(handoff)}",
        "--timeout-seconds", "5",
    ]
    parent = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        wait_for(cache / "draining")
        parent.send_signal(signal.SIGINT)
        wait_for(cache / "interrupted")
        (cache / "release-drain").touch()
        _out, err = parent.communicate(timeout=5)
        assert parent.returncode == 1
        assert b"candidate preparation interrupted" in err
        assert not (cache / "boot-invoked").exists()
        assert not receipt_path.exists() and not candidate.exists()
        assert (deployment / ".candidate.json.full-handoff.json").exists()
    finally:
        (cache / "release-drain").touch()
        if parent.poll() is None:
            parent.kill()
            parent.communicate(timeout=5)


def test_create_only_publication_interrupt_has_single_link_and_is_retryable(tmp_path: Path) -> None:
    target = tmp_path / "candidate.json"
    payload = b'{"candidate":"fixed"}\n'

    def interrupt(stage: str) -> None:
        if stage == "after_rename":
            raise KeyboardInterrupt()

    with launcher.deployment_lock(target) as lock_fd:
        with pytest.raises(launcher.PersistenceUncertain):
            launcher._publish_control_file_locked(
                target, payload, lock_fd, fault_hook=interrupt, create_only=True
            )
    assert target.read_bytes() == payload
    assert target.stat().st_nlink == 1
    assert not list(tmp_path.glob(".candidate.json.tmp-*"))
    with launcher.deployment_lock(target) as lock_fd:
        launcher._sync_retained_publication(
            target, lock_fd, payload, 1024, lambda body: body
        )
        with pytest.raises(launcher.SelectionError, match="candidate already exists"):
            launcher._publish_control_file_locked(target, b"replacement", lock_fd, create_only=True)
    assert target.read_bytes() == payload
    assert not list(tmp_path.glob(".candidate.json.tmp-*"))


def test_candidate_boot_signal_after_receipt_refuses_selection_then_retries(tmp_path: Path) -> None:
    active, legacy_binary, old_root, cache, environment = fixture(tmp_path)
    launcher.publish_selection(active, launcher.proposed_selection(legacy_binary, old_root, environment))
    before = active.read_bytes()
    deployment = tmp_path / "candidate-deployment"
    deployment.mkdir(mode=0o700)
    root = tmp_path / "candidate-root"
    root.mkdir(mode=0o700)
    config = tmp_path / "native.json"
    native_config(config, root)
    receipt = native_receipt()
    write_wire(cache / "fixed-receipt.json", json.dumps(receipt.to_wire()))
    image = tmp_path / "candidate-searchd"
    python = Path(sys.executable).resolve()
    image.write_text(f"#!{python}\n" + """
import os, signal, sys, time
from pathlib import Path
cache = Path(os.environ["QUANTA_INDEX_CACHE_ROOT"])
with (cache / "events").open("a") as stream:
    stream.write(sys.argv[1] + "\\n")
if sys.argv[1] == "select-native-candidate":
    signal.signal(signal.SIGINT, lambda *_: (cache / "boot-interrupted").touch())
    os.umask(0o077)
    Path(sys.argv[13]).write_bytes((cache / "fixed-receipt.json").read_bytes())
    (cache / "boot-written").touch()
    while not (cache / "release-boot").exists():
        time.sleep(0.01)
elif sys.argv[1] != "verify-native-candidate":
    sys.exit(43)
""")
    image.chmod(0o700)
    candidate = deployment / "candidate.json"
    receipt_path = deployment / "boot.json"
    args = [
        "prepare-native-candidate", "--selection", str(active),
        "--expected-selection-sha256", hashlib.sha256(before).hexdigest(),
        "--candidate", str(candidate), "--binary", str(image),
        "--binary-sha256", hashlib.sha256(image.read_bytes()).hexdigest(),
        "--state-root", str(root), "--config", str(config),
        "--config-sha256", hashlib.sha256(config.read_bytes()).hexdigest(),
        "--receipt", str(receipt_path), "--root-nonce", receipt.root_nonce_hex,
        "--manifest-operation", receipt.manifest_operation_json,
        "--source-cut-digest", receipt.source_cut_digest_hex,
        "--env", f"QUANTA_INDEX_CACHE_ROOT={cache}",
    ]
    parent = subprocess.Popen(
        [sys.executable, str(Path(launcher.__file__)), *args],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    try:
        wait_for(cache / "boot-written")
        parent.send_signal(signal.SIGINT)
        wait_for(cache / "boot-interrupted")
        (cache / "release-boot").touch()
        _out, err = parent.communicate(timeout=5)
        assert parent.returncode == 1
        assert b"select-native-candidate interrupted" in err
        assert receipt_path.exists() and not candidate.exists()
    finally:
        (cache / "release-boot").touch()
        if parent.poll() is None:
            parent.kill()
            parent.communicate(timeout=5)
    assert launcher.main(args) == 0
    assert candidate.exists() and active.read_bytes() == before
    assert (cache / "events").read_text().splitlines() == [
        f"verify:{old_root}",
        "select-native-candidate", "verify-native-candidate",
    ]


@pytest.mark.parametrize("current_mode_v1", ["legacy", "native"])
def test_serve_native_candidate_pins_config_and_reaps_before_releasing_lock(
    tmp_path: Path, current_mode_v1: str,
) -> None:
    active, binary, old_root, cache, environment = fixture(tmp_path)
    deployment = tmp_path / "candidate-deployment"
    deployment.mkdir(mode=0o700)
    candidate = deployment / "candidate.json"
    root = tmp_path / "candidate-root"
    root.mkdir(mode=0o700)
    config = tmp_path / "native.json"
    native_config(config, root)
    digest = hashlib.sha256(config.read_bytes()).hexdigest()
    image = tmp_path / "candidate-searchd"
    image.write_text(
        "#!/bin/sh\nset -eu\n"
        '[ "$1" = serve-native-candidate ] && [ "$2" = --config ] && [ "$4" = --config-digest ]\n'
        f'[ "$3" = "{config}" ] && [ "$5" = {digest} ] && [ "$#" -eq 5 ]\n'
        'trap \'exit 0\' TERM\n'
        'echo $$ > "$QUANTA_INDEX_CACHE_ROOT/child-pid"\n'
        'while :; do sleep 1; done\n',
        encoding="utf-8",
    )
    image.chmod(0o700)
    if current_mode_v1 == "native":
        old_image = tmp_path / "old-native-searchd"
        old_image.write_bytes(NATIVE_CANDIDATE_SEARCHD)
        old_image.chmod(0o700)
        old_config = tmp_path / "old-native.json"
        native_config(old_config, old_root)
        old = replace(
            launcher.proposed_selection(old_image, old_root, environment),
            native_config=old_config,
            native_config_sha256=hashlib.sha256(old_config.read_bytes()).hexdigest(),
            native_boot_receipt=native_receipt(),
        )
    else:
        old = launcher.proposed_selection(binary, old_root, environment)
    launcher.publish_selection(active, old)
    before = active.read_bytes()
    args = [
        sys.executable, str(Path(launcher.__file__)), "serve-native-candidate",
        "--selection", str(active), "--expected-selection-sha256", hashlib.sha256(before).hexdigest(),
        "--candidate", str(candidate), "--binary", str(image),
        "--binary-sha256", hashlib.sha256(image.read_bytes()).hexdigest(),
        "--state-root", str(root), "--config", str(config), "--config-sha256", digest,
        "--env", f"QUANTA_INDEX_CACHE_ROOT={cache}", "--env", "PATH=/usr/bin:/bin",
    ]
    with launcher.deployment_lock(active):
        parent = subprocess.Popen(args)
        child_pid = None
        try:
            wait_for(cache / "child-pid")
            child_pid = int((cache / "child-pid").read_text())
            with pytest.raises(launcher.DeploymentBusy), launcher.deployment_lock(candidate):
                raise AssertionError("running candidate lost its lock")
            parent.send_signal(signal.SIGTERM)
            assert parent.wait(timeout=5) == 0
            with pytest.raises(ProcessLookupError):
                os.kill(child_pid, 0)
            child_pid = None
            with launcher.deployment_lock(candidate):
                assert not candidate.exists()
        finally:
            if parent.poll() is None:
                parent.kill()
                parent.wait(timeout=5)
            if child_pid is not None:
                try:
                    os.killpg(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
    assert active.read_bytes() == before


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

    def interrupted_replace(source: str | Path, destination: str | Path, **kwargs) -> None:
        replace(source, destination, **kwargs)
        raise KeyboardInterrupt

    monkeypatch.setattr(launcher.os, "replace", interrupted_replace)
    with pytest.raises(launcher.PersistenceUncertain, match="inspect and verify"):
        launcher.publish_selection(path, candidate)
    assert read_wire(path) == raw_selection(binary, next_root, environment)
    assert not list(path.parent.glob(".selection.json.tmp-*"))


def test_install_refuses_the_deployment_lock_as_a_selection(tmp_path: Path) -> None:
    path, binary, root, cache, environment = fixture(tmp_path)
    lock = path.with_name(launcher.DEPLOYMENT_LOCK_NAME)
    lock.touch(mode=0o600)
    inode = lock.stat().st_ino
    with pytest.raises(launcher.SelectionError, match="aliases the deployment lock"):
        launcher.publish_selection(lock, launcher.proposed_selection(binary, root, environment))
    assert lock.stat().st_ino == inode
    assert lock.read_bytes() == b""
    assert not (cache / "events").exists()


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


def test_candidate_producer_receives_only_the_selected_q_config_digest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    active, old, selected, _ = native_switch_fixture(tmp_path)
    config = selected.native_config
    assert config is not None
    socket = selected.state_root / "native.sock"
    config_bytes = json.dumps({
        "version": 1, "state_root": str(selected.state_root), "socket_path": str(socket),
    }).encode()
    config.write_bytes(config_bytes)
    digest = hashlib.sha256(config_bytes).hexdigest()
    selected = replace(selected, native_config_sha256=digest, native_boot_receipt=None)
    controls = tmp_path / "candidate-control"
    controls.mkdir(mode=0o700)

    class ReachedProducer(Exception):
        pass

    def next_stage(selection, mode, lock_fd, *, with_candidate):
        assert selection == selected and mode == "serve-native-candidate"
        with_candidate(object(), lambda: False)

    def observe_arguments(child, stopping, lock_fd, endpoint, producer, argv, env, timeout):
        assert endpoint == socket
        values = [value for flag, value in zip(argv[::2], argv[1::2])
                  if flag == "--native-q-config-sha256"]
        assert values == [digest]
        raise ReachedProducer

    monkeypatch.setattr(launcher, "_run_selected_command", next_stage)
    monkeypatch.setattr(launcher, "_run_candidate_source", observe_arguments)
    inputs = {
        "receipt_path": controls / "boot.json",
        "producer": selected.binary,
        "producer_sha256": selected.binary_sha256,
        "producer_environment": {},
    }
    candidate = controls / "candidate.json"
    expected = independent_selection_fingerprint(active)
    with pytest.raises(ReachedProducer):
        launcher.build_native_candidate(
            active, candidate, selected, expected, producer_arguments=[], **inputs,
        )
    with pytest.raises(launcher.SelectionError, match="launcher owns the candidate Q endpoint"):
        launcher.build_native_candidate(
            active, candidate, selected, expected,
            producer_arguments=["--native-q-config-sha256", "12" * 32], **inputs,
        )

    def observe_old_and_new(child, stopping, lock_fd, endpoint, producer, argv, env, timeout):
        flags = dict(zip(argv[::2], argv[1::2]))
        assert flags["--native-candidate-q-config"] == str(selected.native_config)
        assert flags["--native-candidate-q-config-sha256"] == digest
        assert flags["--native-candidate-old-q-config"] == str(old.native_config)
        assert flags["--native-candidate-old-q-config-sha256"] == old.native_config_sha256
        assert old.native_config != selected.native_config
        assert old.native_config_sha256 != digest
        raise ReachedProducer

    monkeypatch.setattr(launcher, "_run_candidate_source", observe_old_and_new)
    native_args = [
        "--native-candidate-history-source", "native",
        "--native-candidate-source-mode", "source-only",
    ]
    with pytest.raises(ReachedProducer):
        launcher.build_native_candidate(
            active, candidate, selected, expected, producer_arguments=native_args, **inputs,
        )
    with pytest.raises(launcher.SelectionError, match="launcher owns the candidate Q endpoint"):
        launcher.build_native_candidate(
            active, candidate, selected, expected,
            producer_arguments=native_args + ["--native-candidate-old-q-config", str(old.native_config)],
            **inputs,
        )


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


def native_published_handoff(selection: launcher.Selection) -> dict[str, str]:
    receipt = selection.native_boot_receipt
    assert receipt is not None
    return {
        "target_operation_json": receipt.manifest_operation_json,
        "published_operation_json": receipt.manifest_operation_json,
        "published_source_cut_digest_hex": receipt.source_cut_digest_hex,
    }


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
        **native_published_handoff(new),
    )
    assert read_wire(path) == new.to_wire()
    events = (cache / "events").read_text().splitlines()
    assert events == [
        f"verify:{old.state_root}",
        f"verify-native:{new.native_config}:{new.native_config_sha256}",
    ]
    assert original != path.read_bytes()
    assert previous.read_bytes() == original
    with pytest.raises(launcher.SelectionError, match="offline.*producer"):
        launcher.rollback_legacy_selection(
            path, previous, independent_selection_fingerprint(path), before
        )
    assert read_wire(path) == new.to_wire()
    assert previous.read_bytes() == original
    assert (cache / "events").read_text().splitlines() == events


@pytest.mark.parametrize(
    "fault",
    [
        "missing_all",
        "missing_target",
        "missing_published",
        "missing_source",
        "target",
        "published",
        "source",
    ],
)
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
    if fault == "missing_all":
        axes = {}
    elif fault.startswith("missing_"):
        missing_axis = {
            "missing_target": "target_operation_json",
            "missing_published": "published_operation_json",
            "missing_source": "published_source_cut_digest_hex",
        }[fault]
        axes.pop(missing_axis)
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


def test_legacy_switch_cli_requires_published_handoff_before_verifier(tmp_path: Path) -> None:
    path, _old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    receipt_path = tmp_path / "candidate-receipt.json"
    assert new.native_boot_receipt is not None
    write_wire(receipt_path, json.dumps(new.native_boot_receipt.to_wire()))
    expected = independent_selection_fingerprint(path)
    status = launcher.main(
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
            str(receipt_path),
            *[item for name, value in new.environment for item in ("--env", f"{name}={value}")],
        ]
    )
    assert status == 1
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
            **native_published_handoff(new),
        )
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


@pytest.mark.parametrize("fault", ["stale_cas", "backup_mismatch", "old_binary_changed"])
def test_legacy_rollback_refusal_preserves_native_selection(
    tmp_path: Path, fault: str
) -> None:
    path, old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    old_digest = independent_selection_fingerprint(path)
    launcher.switch_native_selection(
        path,
        new,
        old_digest,
        previous_selection_path=previous,
        expected_previous_selection_sha256=old_digest,
        **native_published_handoff(new),
    )
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    current_digest = independent_selection_fingerprint(path)
    if fault == "stale_cas":
        current_digest = "00" * 32
        refusal = "current selection changed"
    elif fault == "backup_mismatch":
        previous.write_bytes(b"changed snapshot")
        refusal = "invalid selection JSON"
    else:
        old.binary.write_bytes(b"changed Legacy binary")
        refusal = "binary SHA-256 changed"
    with pytest.raises(launcher.SelectionError, match=refusal):
        launcher.rollback_legacy_selection(path, previous, current_digest, old_digest)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_rollback_refuses_absent_producer_and_catalog_quiescence(tmp_path: Path) -> None:
    path, _old, new, cache, previous = legacy_to_native_switch_fixture(tmp_path)
    old_digest = independent_selection_fingerprint(path)
    launcher.switch_native_selection(
        path,
        new,
        old_digest,
        previous_selection_path=previous,
        expected_previous_selection_sha256=old_digest,
        **native_published_handoff(new),
    )
    before, events = path.read_bytes(), (cache / "events").read_bytes()
    # BOOT succeeds, but no producer admission fence, durable high-water or
    # outbox evidence exists. Restoring Legacy could discard unseen work.
    with pytest.raises(launcher.SelectionError, match="offline.*producer"):
        launcher.rollback_legacy_selection(
            path, previous, independent_selection_fingerprint(path), old_digest
        )
    assert path.read_bytes() == before
    assert (cache / "events").read_bytes() == events


def test_legacy_cutover_postrename_uncertainty_does_not_authorize_rollback(
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
            **native_published_handoff(new),
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
            **native_published_handoff(new),
        )
    with pytest.raises(launcher.SelectionError, match="offline.*producer"):
        launcher.rollback_legacy_selection(
            path, previous, independent_selection_fingerprint(path), old_digest
        )
    assert read_wire(path) == new.to_wire()
    assert read_wire(previous) == old.to_wire()


def test_legacy_cutover_cli_preserves_prior_head_and_rollback_cli_refuses_missing_authorities(
    tmp_path: Path, capsys: pytest.CaptureFixture
) -> None:
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
                "--target-operation",
                new.native_boot_receipt.manifest_operation_json,
                "--published-operation",
                new.native_boot_receipt.manifest_operation_json,
                "--published-source-cut-digest",
                new.native_boot_receipt.source_cut_digest_hex,
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
        == 1
    )
    assert "offline Catalog" in capsys.readouterr().err
    assert read_wire(path) == new.to_wire()
    assert read_wire(previous) == old.to_wire()


def test_native_cross_root_switch_with_full_handoff_still_requires_quiescence(
    tmp_path: Path,
) -> None:
    path, old, new, cache = native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    before = independent_selection_fingerprint(path)
    assert launcher.inspect_selection(path) == {
        "selection_sha256": before,
        "mode": "native",
        "state_root": str(old.state_root),
    }
    with pytest.raises(launcher.SelectionError, match="offline.*producer"):
        launcher.switch_native_selection(path, new, before, **native_published_handoff(new))
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_native_cross_root_switch_refuses_absent_producer_and_catalog_quiescence(
    tmp_path: Path,
) -> None:
    path, _old, new, cache = native_switch_fixture(tmp_path)
    before, events = path.read_bytes(), (cache / "events").read_bytes()
    with pytest.raises(launcher.SelectionError, match="offline.*producer"):
        launcher.switch_native_selection(path, new, independent_selection_fingerprint(path))
    assert path.read_bytes() == before
    assert (cache / "events").read_bytes() == events


def test_native_to_native_switch_refuses_partial_published_handoff(tmp_path: Path) -> None:
    path, _old, new, cache = native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    axes = native_published_handoff(new)
    axes.pop("published_source_cut_digest_hex")
    with pytest.raises(launcher.SelectionError, match="handoff"):
        launcher.switch_native_selection(
            path,
            new,
            independent_selection_fingerprint(path),
            **axes,
        )
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


@pytest.mark.parametrize(
    ("axis", "refusal"),
    [
        ("stale_cas", "current selection changed"),
        ("same_root", "distinct and non-nested"),
        ("nested", "distinct and non-nested"),
        ("copied_nonce", "independent Native incarnations"),
        ("workspace", "another workspace"),
    ],
)
def test_native_switch_refuses_foreign_or_stale_candidate_before_verifier(
    tmp_path: Path, axis: str, refusal: str
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
    with pytest.raises(launcher.SelectionError, match=refusal):
        launcher.switch_native_selection(path, new, expected)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


def test_legacy_cutover_candidate_verifier_refusal_preserves_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, _old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original = path.read_bytes()
    expected = independent_selection_fingerprint(path)
    visits = []

    def verify(selection, _lock):
        visits.append(selection.state_root)
        raise launcher.SelectionError("fixed stale/incomplete receipt refusal")

    monkeypatch.setattr(launcher, "_verify_bootstrap_locked", verify)
    with pytest.raises(launcher.SelectionError, match="stale/incomplete"):
        launcher.switch_native_selection(
            path,
            new,
            expected,
            previous_selection_path=previous,
            expected_previous_selection_sha256=expected,
            **native_published_handoff(new),
        )
    assert path.read_bytes() == original
    assert visits == [new.state_root]


def test_native_cross_root_install_cannot_bypass_switch_cas(tmp_path: Path) -> None:
    path, _old, new, cache = native_switch_fixture(tmp_path)
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    with pytest.raises(launcher.SelectionError, match="requires switch-native"):
        launcher.publish_selection(path, new)
    assert path.read_bytes() == original
    assert (cache / "events").read_bytes() == events


@pytest.mark.parametrize("axis", ["operation", "object", "source", "catalog", "nonce"])
def test_same_root_native_install_cannot_replace_installed_boot_baseline(
    tmp_path: Path, axis: str
) -> None:
    path, old, _unused, cache = native_switch_fixture(tmp_path)
    assert old.native_boot_receipt is not None and old.native_config is not None
    wire = old.native_boot_receipt.to_wire()
    if axis == "operation":
        wire["manifest_operation"]["sequence"] = 2
    else:
        field = {
            "object": "manifest_object_hex",
            "source": "source_cut_digest_hex",
            "catalog": "install_catalog_digest_hex",
            "nonce": "root_nonce_hex",
        }[axis]
        wire[field] = "ab" * 32
    reissued = launcher.proposed_native_selection(
        old.binary, old.state_root, old.native_config, dict(old.environment),
        launcher.NativeBootReceipt.from_wire(wire),
    )
    original, events = path.read_bytes(), (cache / "events").read_bytes()
    refusal = None
    try:
        launcher.publish_selection(path, reissued)
    except launcher.SelectionError as error:
        refusal = error
    # A verifier accepting a newly issued BOOT cannot move the first installed
    # baseline. Check bytes and subprocess side effects before the diagnostic.
    assert path.read_bytes() == original, "reissued BOOT replaced the installed baseline"
    assert (cache / "events").read_bytes() == events
    assert refusal is not None and "installed Native baseline" in str(refusal)


def test_legacy_switch_rechecks_cas_after_preflight(tmp_path: Path) -> None:
    path, _old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    expected = independent_selection_fingerprint(path)
    competing = read_wire(path)
    competing["environment"]["LANG"] = "C"

    def change_current(stage):
        if stage == "after_file_fsync":
            write_wire(path, json.dumps(competing))

    with pytest.raises(launcher.SelectionError, match="current selection changed"):
        launcher.switch_native_selection(
            path,
            new,
            expected,
            previous_selection_path=previous,
            expected_previous_selection_sha256=expected,
            **native_published_handoff(new),
            fault_hook=change_current,
        )
    assert read_wire(path) == competing
    assert not list(path.parent.glob(".selection.json.tmp-*"))


def test_legacy_switch_postrename_interrupt_requires_new_inspection(tmp_path: Path) -> None:
    path, _old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    expected = independent_selection_fingerprint(path)

    def interrupt(stage):
        if stage == "after_rename":
            raise KeyboardInterrupt()

    with pytest.raises(launcher.PersistenceUncertain):
        launcher.switch_native_selection(
            path,
            new,
            expected,
            previous_selection_path=previous,
            expected_previous_selection_sha256=expected,
            **native_published_handoff(new),
            fault_hook=interrupt,
        )
    assert read_wire(path)["state_root"] == str(new.state_root)
    with pytest.raises(launcher.SelectionError, match="current selection changed"):
        launcher.switch_native_selection(path, new, expected)


@pytest.mark.parametrize(
    "stage", ["after_file_fsync", "after_rename", "after_directory_fsync"]
)
def test_legacy_switch_checks_the_actual_published_selection(
    tmp_path: Path, stage: str
) -> None:
    path, _old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original = path.read_bytes()
    expected = independent_selection_fingerprint(path)
    replacement = b"{}\n"

    def replace_publication(reached: str) -> None:
        if reached != stage:
            return
        target = path
        if reached == "after_file_fsync":
            (target,) = path.parent.glob(f".{path.name}.tmp-*")
            target.unlink()
        write_wire(target, replacement.decode())

    failure = launcher.SelectionError if stage == "after_file_fsync" else launcher.PersistenceUncertain
    with pytest.raises(failure) as caught:
        launcher.switch_native_selection(
            path,
            new,
            expected,
            previous_selection_path=previous,
            expected_previous_selection_sha256=expected,
            **native_published_handoff(new),
            fault_hook=replace_publication,
        )
    if stage == "after_file_fsync":
        assert not isinstance(caught.value, launcher.PersistenceUncertain)
        assert path.read_bytes() == original
        (temporary,) = path.parent.glob(f".{path.name}.tmp-*")
        assert temporary.read_bytes() == replacement
    else:
        assert path.read_bytes() == replacement


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


def test_selection_temporary_stays_in_the_locked_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, _old, new, _cache, previous = legacy_to_native_switch_fixture(tmp_path)
    original = path.read_bytes()
    expected = independent_selection_fingerprint(path)
    parent = path.parent
    parked = parent.with_name(f"{parent.name}-parked")
    real_open = os.open
    swapped = False

    def open_after_directory_swap(name, flags, *args, **kwargs):
        nonlocal swapped
        if not swapped and flags & os.O_EXCL and Path(name).name.startswith(f".{path.name}.tmp-"):
            swapped = True
            parent.rename(parked)
            parent.mkdir(mode=0o700)
        return real_open(name, flags, *args, **kwargs)

    with monkeypatch.context() as patch:
        patch.setattr(os, "open", open_after_directory_swap)
        with pytest.raises(launcher.SelectionError) as caught:
            launcher.switch_native_selection(
                path, new, expected, previous_selection_path=previous,
                expected_previous_selection_sha256=expected, **native_published_handoff(new),
            )

    assert swapped
    assert not isinstance(caught.value, launcher.PersistenceUncertain)
    assert (parked / path.name).read_bytes() == original
    assert list(parent.iterdir()) == []
    assert list(parked.glob(f".{path.name}.tmp-*")) == []
