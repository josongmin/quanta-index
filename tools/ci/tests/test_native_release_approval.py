import hashlib
import json
import os
import stat
from pathlib import Path
from unittest.mock import Mock

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from tools.deployment import launcher
from tools.deployment import native_release_approval as approval


def sha(data):
    return hashlib.sha256(data).hexdigest()


def selection_fixture(root, *, binary_bytes=b"test Q ELF"):
    binary, state, config = (root / name for name in ("searchd", "native-state", "native.json"))
    binary.write_bytes(binary_bytes)
    binary.chmod(0o700)
    state.mkdir()
    config.write_text(json.dumps({"version": 1, "state_root": str(state)}))
    config.chmod(0o600)
    identifier = list(range(16))
    receipt = launcher.NativeBootReceipt.from_wire({
        "root_nonce_hex": "a" * 64,
        "manifest_operation": {
            "workspace": dict.fromkeys(("tenant", "collection", "worktree", "slot"), identifier),
            "epoch": 1,
            "producer": identifier,
            "stream": identifier,
            "stream_epoch": 1,
            "sequence": 1,
        },
        "manifest_object_hex": "b" * 64,
        "source_cut_digest_hex": "c" * 64,
        "install_catalog_digest_hex": "d" * 64,
    })
    selected = launcher.proposed_native_selection(binary, state, config, {}, receipt)
    selection_path, policy = root / "active.json", root / "policy.json"
    selection_path.write_bytes(launcher._selection_payload(selected))
    selection_path.chmod(0o600)
    policy.write_bytes(b'{"decision":"approved"}')
    request = dict(deployment_root=str(root), selection_path=str(selection_path),
                   selection_sha256=sha(selection_path.read_bytes()),
                   decision_policy_path=str(policy), decision_policy_sha256=sha(policy.read_bytes()))
    return selected, request


def test_d_approves_the_selected_q_and_f_requires_the_same_install(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve())
    manifest = b"installed four-role release"
    manifest_sha = sha(manifest)
    active = json.dumps({"payload": {
        "schema_version": "semantica-installed-product-active/v1",
        "install_root": str(tmp_path), "release_manifest_sha256": manifest_sha,
    }}).encode()
    private_key = Ed25519PrivateKey.generate()
    boot = Mock()
    monkeypatch.setattr(launcher, "_run_selected_command", boot)
    boot.return_value = 0
    issued = approval.approve_selected_install(
        request, q_sha256=selected.binary_sha256, active_sha256=sha(active),
        manifest_sha256=manifest_sha, sign=private_key.sign, launcher=launcher,
    )
    boot.assert_called_once()
    assert boot.call_args.args[:2] == (selected, "verify-native-state")
    assert issued["payload"]["boot_receipt"] == selected.native_boot_receipt.to_wire()
    installed = {
        "active-generation.json": active,
        "deployment-public-key.bin": private_key.public_key().public_bytes_raw(),
        "release-manifest.json": manifest,
    }
    monkeypatch.setattr(approval, "_installed_bytes", lambda path, _limit: installed[path.name])
    approval.verify_installed_approval(
        issued, selected, request["selection_sha256"], request["decision_policy_sha256"],
    )
    with pytest.raises(ValueError, match="differs from the selected installation"):
        approval.verify_installed_approval(issued, selected, request["selection_sha256"], "0" * 64)
    foreign = {**issued, "signature_hex": Ed25519PrivateKey.generate().sign(
        approval.signing_bytes(issued["payload"])).hex()}
    with pytest.raises(ValueError, match="signature differs"):
        approval.verify_installed_approval(
            foreign, selected, request["selection_sha256"], request["decision_policy_sha256"],
        )
    installed["active-generation.json"] = active + b"\n"
    with pytest.raises(ValueError, match="differs from the selected installation"):
        approval.verify_installed_approval(
            issued, selected, request["selection_sha256"], request["decision_policy_sha256"],
        )


def test_d_preflights_the_selected_elf_and_config_without_running_boot(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve())
    boot = Mock(side_effect=AssertionError("preflight must leave BOOT to the Linux signer"))
    monkeypatch.setattr(launcher, "_run_selected_command", boot)
    inputs = dict(q_sha256=selected.binary_sha256, launcher=launcher)
    approval.verify_approval_request(request, **inputs)

    config = selected.native_config
    original = config.read_bytes()
    config.write_bytes(original + b" ")
    with pytest.raises(launcher.SelectionError, match="native config SHA-256 changed"):
        approval.verify_approval_request(request, **inputs)
    config.write_bytes(original)
    selected.binary.write_bytes(b"changed Q ELF")
    with pytest.raises(launcher.SelectionError, match="selected binary SHA-256 changed"):
        approval.verify_approval_request(request, **inputs)
    boot.assert_not_called()


def test_d_refuses_a_foreign_elf_or_changed_policy_before_signing(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve())
    boot, sign = Mock(), Mock()
    monkeypatch.setattr(launcher, "_run_selected_command", boot)
    kwargs = dict(active_sha256="1" * 64, manifest_sha256="2" * 64, sign=sign, launcher=launcher)
    with pytest.raises(ValueError, match="approved Q ELF"):
        approval.approve_selected_install(request, q_sha256="3" * 64, **kwargs)
    with pytest.raises(ValueError, match="decision policy changed"):
        approval.approve_selected_install(
            dict(request, decision_policy_sha256="4" * 64), q_sha256=selected.binary_sha256, **kwargs,
        )
    boot.assert_not_called()
    sign.assert_not_called()


def test_d_refuses_a_policy_change_during_boot_verification(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve())
    policy = tmp_path / "policy.json"

    def change_policy(*_args, **_kwargs):
        policy.write_bytes(b'{"decision":"changed during boot"}')
        return 0

    monkeypatch.setattr(launcher, "_run_selected_command", change_policy)
    sign = Mock()
    with pytest.raises(ValueError, match="decision policy changed"):
        approval.approve_selected_install(
            request, q_sha256=selected.binary_sha256, active_sha256="1" * 64,
            manifest_sha256="2" * 64, sign=sign, launcher=launcher,
        )
    sign.assert_not_called()


def test_f_requires_the_installed_trust_directory_chain_to_be_protected(tmp_path, monkeypatch):
    trust = tmp_path.resolve() / "trust"
    trust.mkdir(mode=0o770)
    key = trust / "deployment-public-key.bin"
    key.write_bytes(b"x" * 32)
    key.chmod(0o644)
    trust.chmod(0o770)
    lstat, fstat = Path.lstat, os.fstat

    def owned_metadata(info, *, trusted):
        values = list(info)
        values[4] = 0
        if trusted and stat.S_ISDIR(info.st_mode):
            values[0] &= ~0o022
        return os.stat_result(values)

    monkeypatch.setattr(Path, "lstat", lambda path: owned_metadata(lstat(path), trusted=path != trust))
    monkeypatch.setattr(os, "fstat", lambda fd: owned_metadata(fstat(fd), trusted=True))
    with pytest.raises(ValueError, match="protected installed directory"):
        approval._installed_bytes(key, 32)
    trust.chmod(0o750)
    assert approval._installed_bytes(key, 32) == b"x" * 32
