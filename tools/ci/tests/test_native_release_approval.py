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


def signed_product_active(key, root, manifest_sha, *, generation=1, role_digests=None,
                          native_recovery=None, native_facts_publication=None, domain=None):
    payload = {
        "schema_version": ("semantica-installed-product-active/v3" if native_facts_publication is not None
                           else "semantica-installed-product-active/v2" if native_recovery is not None
                           else "semantica-installed-product-active/v1"),
        "install_root": str(root),
        "generation": generation,
        "ag5_active_record_sha256": "a" * 64,
        "release_manifest_sha256": manifest_sha,
        "cli_sha256": "b" * 64,
        "server_sha256": "c" * 64,
        "sql_rulepack_worker_sha256": "d" * 64,
        "parser_native_worker_sha256": "e" * 64,
    }
    if role_digests is not None:
        payload.update(role_digests)
    if native_recovery is not None:
        payload["native_recovery"] = native_recovery
    if native_facts_publication is not None:
        payload["native_facts_publication"] = native_facts_publication
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("ascii")
    if domain is None:
        domain = (b"semantica.installed-product-active.v3\n" if native_facts_publication is not None
                  else b"semantica.installed-product-active.v2\n" if native_recovery is not None
                  else b"semantica.installed-product-active.v1\n")
    signature = key.sign(domain + canonical).hex()
    return json.dumps(
        {"payload": payload, "signature_hex": signature}, sort_keys=True, separators=(",", ":"),
    ).encode("ascii") + b"\n"


def native_recovery_fixture(selected, root):
    return {
        "analysis_family": "taint_rows_ifds",
        "repository_root": str(root / "repository"),
        "q_image_sha256": selected.binary_sha256,
        "q_config_sha256": selected.native_config_sha256,
    }


def native_facts_publication_fixture(selected, root):
    return {
        "repository_root": str(root / "repository"),
        "q_image_sha256": selected.binary_sha256,
        "q_config_sha256": selected.native_config_sha256,
    }


def test_f_accepts_signed_v3_native_facts_publication(tmp_path):
    selected, _ = selection_fixture(tmp_path.resolve())
    key = Ed25519PrivateKey.generate()
    facts = native_facts_publication_fixture(selected, tmp_path.resolve())
    active = signed_product_active(key, tmp_path, "f" * 64, native_facts_publication=facts)
    verified = approval.verify_installed_product_generation(
        active, key.public_key().public_bytes_raw(),
    )
    assert verified["schema_version"] == "semantica-installed-product-active/v3"
    assert verified["native_facts_publication"] == facts
    assert "native_recovery" not in verified


def test_f_rejects_v3_wrong_domain_and_native_facts_publication_fields(tmp_path):
    selected, _ = selection_fixture(tmp_path.resolve())
    key = Ed25519PrivateKey.generate()
    public = key.public_key().public_bytes_raw()
    facts = native_facts_publication_fixture(selected, tmp_path.resolve())
    wrong_domain = signed_product_active(
        key, tmp_path, "f" * 64, native_facts_publication=facts,
        domain=b"semantica.installed-product-active.v2\n",
    )
    with pytest.raises(ValueError, match="signature differs"):
        approval.verify_installed_product_generation(wrong_domain, public)
    for changed in (
        {**facts, "repository_root": "/srv//repo"},
        {**facts, "repository_root": "/srv/repo/../other"},
        {**facts, "repository_root": "/"},
        {**facts, "q_image_sha256": "0" * 64},
        {**facts, "q_config_sha256": "A" * 64},
        {**facts, "analysis_family": "facts_graph_view"},
        {field: value for field, value in facts.items() if field != "q_config_sha256"},
    ):
        active = signed_product_active(key, tmp_path, "f" * 64, native_facts_publication=changed)
        with pytest.raises(ValueError, match="payload differs"):
            approval.verify_installed_product_generation(active, public)
    mixed = signed_product_active(
        key, tmp_path, "f" * 64, native_recovery=native_recovery_fixture(selected, tmp_path.resolve()),
        native_facts_publication=facts,
    )
    with pytest.raises(ValueError, match="payload differs"):
        approval.verify_installed_product_generation(mixed, public)


def test_f_accepts_signed_v2_native_recovery_and_preserves_v1(tmp_path):
    selected, _ = selection_fixture(tmp_path.resolve())
    key = Ed25519PrivateKey.generate()
    manifest_sha = "f" * 64
    v1 = signed_product_active(key, tmp_path, manifest_sha)
    assert "native_recovery" not in approval.verify_installed_product_generation(
        v1, key.public_key().public_bytes_raw(),
    )
    recovery = native_recovery_fixture(selected, tmp_path.resolve())
    v2 = signed_product_active(key, tmp_path, manifest_sha, native_recovery=recovery)
    assert approval.verify_installed_product_generation(
        v2, key.public_key().public_bytes_raw(),
    )["native_recovery"] == recovery


def test_f_rejects_v2_wrong_domain_schema_and_native_recovery_fields(tmp_path):
    selected, _ = selection_fixture(tmp_path.resolve())
    key = Ed25519PrivateKey.generate()
    public = key.public_key().public_bytes_raw()
    recovery = native_recovery_fixture(selected, tmp_path.resolve())
    wrong_domain = signed_product_active(
        key, tmp_path, "f" * 64, native_recovery=recovery,
        domain=b"semantica.installed-product-active.v1\n",
    )
    with pytest.raises(ValueError, match="signature differs"):
        approval.verify_installed_product_generation(wrong_domain, public)
    wrong_schema = signed_product_active(
        key, tmp_path, "f" * 64, native_recovery=recovery,
        role_digests={"schema_version": "semantica-installed-product-active/v3"},
    )
    with pytest.raises(ValueError, match="payload differs"):
        approval.verify_installed_product_generation(wrong_schema, public)
    for changed in (
        {**recovery, "analysis_family": "facts_graph_view"},
        {**recovery, "repository_root": "/srv//repo"},
        {**recovery, "repository_root": "/srv/repo/../other"},
        {**recovery, "repository_root": "/"},
        {**recovery, "q_image_sha256": "0" * 64},
        {**recovery, "q_config_sha256": "A" * 64},
        {**recovery, "extra": "forbidden"},
    ):
        active = signed_product_active(key, tmp_path, "f" * 64, native_recovery=changed)
        with pytest.raises(ValueError, match="payload differs"):
            approval.verify_installed_product_generation(active, public)


def selection_fixture(root, *, binary_bytes=b"test Q ELF", retired_policy=False):
    binary, state, config = (root / name for name in ("searchd", "native-state", "native.json"))
    binary.write_bytes(binary_bytes)
    binary.chmod(0o700)
    state.mkdir()
    config.write_text(json.dumps({"version": 1, "state_root": str(state)}))
    config.chmod(0o600)
    identifier = list(range(16))
    receipt_wire = {
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
    }
    if retired_policy:
        receipt_wire.update(
            retired_policy_object_hex="e" * 64,
            retired_policy_sha256_hex="f" * 64,
            retired_policy_identity_count=2,
        )
    receipt = launcher.NativeBootReceipt.from_wire(receipt_wire)
    selected = launcher.proposed_native_selection(binary, state, config, {}, receipt)
    selection_path, policy = root / "active.json", root / "policy.json"
    selection_path.write_bytes(launcher._selection_payload(selected))
    selection_path.chmod(0o600)
    policy.write_bytes(b'{"decision":"approved"}')
    request = dict(deployment_root=str(root), selection_path=str(selection_path),
                   selection_sha256=sha(selection_path.read_bytes()),
                   decision_policy_path=str(policy), decision_policy_sha256=sha(policy.read_bytes()))
    return selected, request


def test_retirement_approval_binds_schema3_policy_and_uses_distinct_domain(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve(), retired_policy=True)
    assert selected.to_wire()["schema_version"] == 3
    manifest = b"installed four-role release"
    key = Ed25519PrivateKey.generate()
    active = signed_product_active(key, tmp_path, sha(manifest))
    payload = approval.binding(
        selected, request["selection_sha256"], request["decision_policy_sha256"],
        sha(active), sha(manifest), selection_path=request["selection_path"],
    )
    assert payload["schema_version"] == approval.RETIREMENT_SCHEMA
    assert payload["selection_path"] == request["selection_path"]
    assert payload["retired_projection_policy"] == {
        "object_hex": "e" * 64, "sha256_hex": "f" * 64, "identity_count": 2,
    }
    receipt = {"payload": payload, "signature_hex": key.sign(approval.signing_bytes(payload)).hex()}
    installed = {
        "active-generation.json": active,
        "deployment-public-key.bin": key.public_key().public_bytes_raw(),
        "release-manifest.json": manifest,
    }
    monkeypatch.setattr(approval, "_installed_bytes", lambda path, _limit: installed[path.name])
    approval.verify_installed_approval(
        receipt, selected, request["selection_sha256"], request["decision_policy_sha256"],
        selection_path=request["selection_path"],
    )
    approval_file = tmp_path / "approved-native-q.json"
    approval_file.write_bytes(approval._canonical_bytes(receipt) + b"\n")
    approval_file.chmod(0o600)
    launched = Mock(return_value=0)
    monkeypatch.setattr(launcher, "_run_selected_command", launched)
    with pytest.raises(launcher.SelectionError, match="requires signed release approval"):
        launcher.serve_selection(request["selection_path"])
    assert launcher.serve_selection(
        request["selection_path"], approval_path=approval_file,
        decision_policy_path=Path(request["decision_policy_path"]),
    ) == 0
    launched.assert_called_once()
    assert launched.call_args.kwargs["retirement_approval"] == (
        approval_file, Path(request["decision_policy_path"]),
    )
    with pytest.raises(launcher.SelectionError, match="release approval differs"):
        launcher.serve_selection(
            request["selection_path"], approval_path=approval_file,
            decision_policy_path=Path(request["selection_path"]),
        )
    wrong_domain = {
        "payload": payload,
        "signature_hex": key.sign(approval.DOMAIN + approval._canonical_bytes(payload)).hex(),
    }
    with pytest.raises(ValueError, match="signature differs"):
        approval.verify_installed_approval(
            wrong_domain, selected, request["selection_sha256"],
            request["decision_policy_sha256"], selection_path=request["selection_path"],
        )
    for changed in (
        {**payload, "retired_projection_policy": {**payload["retired_projection_policy"], "identity_count": 1}},
        {**payload, "schema_version": approval.SCHEMA},
    ):
        with pytest.raises(ValueError, match="differs from the selected installation"):
            approval.verify_installed_approval(
                {"payload": changed, "signature_hex": receipt["signature_hex"]},
                selected, request["selection_sha256"], request["decision_policy_sha256"],
                selection_path=request["selection_path"],
            )
    with pytest.raises(ValueError, match="selection path differs"):
        approval.verify_installed_approval(
            receipt, selected, request["selection_sha256"], request["decision_policy_sha256"],
            selection_path=tmp_path / "missing-selection.json",
        )


def test_retired_serve_child_inherits_exact_deployment_lock_and_locators(tmp_path, monkeypatch):
    import sys

    output = tmp_path / "child-observed.json"
    program = (
        f"#!{sys.executable}\n"
        "import fcntl, json, os, sys\n"
        "from pathlib import Path\n"
        "fd = int(os.environ['QUANTA_NATIVE_DEPLOYMENT_LOCK_FD_V2'])\n"
        "selection = Path(sys.argv[0]).parent / 'active.json'\n"
        "named = selection.parent / '.quanta-index-deployment.lock'\n"
        "assert os.path.samestat(os.fstat(fd), named.stat())\n"
        "fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\n"
        f"Path({str(output)!r}).write_text(json.dumps({{'approval': os.environ['QUANTA_NATIVE_RETIREMENT_APPROVAL_PATH_V2'], 'policy': os.environ['QUANTA_NATIVE_RETIREMENT_DECISION_POLICY_PATH_V2']}}))\n"
    )
    _selected, request = selection_fixture(
        tmp_path.resolve(), binary_bytes=program.encode(), retired_policy=True,
    )
    approval_file = tmp_path / "approved-native-q.json"
    approval_file.write_text('{"payload":{},"signature_hex":"00"}')
    approval_file.chmod(0o600)
    monkeypatch.setattr(approval, "verify_installed_approval", lambda *_args, **_kwargs: None)
    assert launcher.serve_selection(
        request["selection_path"], approval_path=approval_file,
        decision_policy_path=Path(request["decision_policy_path"]),
    ) == 0
    assert json.loads(output.read_text()) == {
        "approval": str(approval_file), "policy": request["decision_policy_path"],
    }


def test_retirement_approval_publishes_canonical_receipt_and_replays_exactly(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve(), retired_policy=True)
    manifest = b"installed four-role release"
    key = Ed25519PrivateKey.generate()
    active = signed_product_active(key, tmp_path, sha(manifest))
    installed = {
        "active-generation.json": active,
        "deployment-public-key.bin": key.public_key().public_bytes_raw(),
        "release-manifest.json": manifest,
    }
    monkeypatch.setattr(approval, "_installed_bytes", lambda path, _limit: installed[path.name])
    monkeypatch.setattr(launcher, "_run_selected_command", Mock(return_value=0))
    target = tmp_path / "approved-native-q.json"
    call = dict(
        q_sha256=selected.binary_sha256, active_sha256=sha(active),
        manifest_sha256=sha(manifest), sign=key.sign, launcher=launcher,
        receipt_path=target,
    )
    original = launcher._publish_control_file_locked

    def fail_before_rename(*args, **kwargs):
        def fault(point):
            if point == "after_file_fsync":
                raise RuntimeError("injected pre-rename failure")
        return original(*args, fault_hook=fault, **kwargs)

    monkeypatch.setattr(launcher, "_publish_control_file_locked", fail_before_rename)
    with pytest.raises(launcher.SelectionError, match="selection was not changed"):
        approval.approve_selected_install(request, **call)
    assert not target.exists()
    assert not list(tmp_path.glob(".approved-native-q.json.tmp-*"))
    monkeypatch.setattr(launcher, "_publish_control_file_locked", original)
    issued = approval.approve_selected_install(request, **call)
    exact = approval._canonical_bytes(issued) + b"\n"
    assert target.read_bytes() == exact
    assert approval.approve_selected_install(request, **call) == issued
    assert target.read_bytes() == exact
    with pytest.raises(ValueError, match="deployment-root file"):
        approval.approve_selected_install(
            request, **{**call, "receipt_path": tmp_path.parent / "foreign.json"},
        )
    target.write_bytes(b"foreign")
    with pytest.raises(launcher.PersistenceUncertain, match="retained control file"):
        approval.approve_selected_install(request, **call)
    assert target.read_bytes() == b"foreign"


def test_d_approves_the_selected_q_and_f_requires_the_same_install(tmp_path, monkeypatch):
    selected, request = selection_fixture(tmp_path.resolve())
    manifest = b"installed four-role release"
    manifest_sha = sha(manifest)
    private_key = Ed25519PrivateKey.generate()
    active = signed_product_active(private_key, tmp_path, manifest_sha)
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
    installed["active-generation.json"] = signed_product_active(private_key, tmp_path, manifest_sha, generation=2)
    with pytest.raises(ValueError, match="differs from the selected installation"):
        approval.verify_installed_approval(
            issued, selected, request["selection_sha256"], request["decision_policy_sha256"],
        )


@pytest.mark.parametrize("foreign_digest", [None, "q_image_sha256", "q_config_sha256"])
def test_f_requires_v2_recovery_to_approve_the_same_q(tmp_path, monkeypatch, foreign_digest):
    selected, request = selection_fixture(tmp_path.resolve())
    manifest = b"installed four-role release"
    key = Ed25519PrivateKey.generate()
    recovery = native_recovery_fixture(selected, tmp_path.resolve())
    if foreign_digest is not None:
        recovery[foreign_digest] = "1" * 64
    active = signed_product_active(key, tmp_path, sha(manifest), native_recovery=recovery)
    payload = approval.binding(
        selected, request["selection_sha256"], request["decision_policy_sha256"],
        sha(active), sha(manifest),
    )
    receipt = {"payload": payload, "signature_hex": key.sign(approval.signing_bytes(payload)).hex()}
    installed = {
        "active-generation.json": active,
        "deployment-public-key.bin": key.public_key().public_bytes_raw(),
        "release-manifest.json": manifest,
    }
    monkeypatch.setattr(approval, "_installed_bytes", lambda path, _limit: installed[path.name])
    if foreign_digest is None:
        approval.verify_installed_approval(
            receipt, selected, request["selection_sha256"], request["decision_policy_sha256"],
        )
    else:
        with pytest.raises(ValueError, match="differs from installed recovery approval"):
            approval.verify_installed_approval(
                receipt, selected, request["selection_sha256"], request["decision_policy_sha256"],
            )


@pytest.mark.parametrize("foreign_digest", [None, "q_image_sha256", "q_config_sha256"])
def test_f_requires_v3_facts_publication_to_approve_the_same_q(tmp_path, monkeypatch, foreign_digest):
    selected, request = selection_fixture(tmp_path.resolve())
    manifest = b"installed four-role release"
    key = Ed25519PrivateKey.generate()
    facts = native_facts_publication_fixture(selected, tmp_path.resolve())
    if foreign_digest is not None:
        facts[foreign_digest] = "1" * 64
    active = signed_product_active(key, tmp_path, sha(manifest), native_facts_publication=facts)
    payload = approval.binding(
        selected, request["selection_sha256"], request["decision_policy_sha256"],
        sha(active), sha(manifest),
    )
    receipt = {"payload": payload, "signature_hex": key.sign(approval.signing_bytes(payload)).hex()}
    installed = {
        "active-generation.json": active,
        "deployment-public-key.bin": key.public_key().public_bytes_raw(),
        "release-manifest.json": manifest,
    }
    monkeypatch.setattr(approval, "_installed_bytes", lambda path, _limit: installed[path.name])
    if foreign_digest is None:
        approval.verify_installed_approval(
            receipt, selected, request["selection_sha256"], request["decision_policy_sha256"],
        )
    else:
        with pytest.raises(ValueError, match="differs from installed recovery approval"):
            approval.verify_installed_approval(
                receipt, selected, request["selection_sha256"], request["decision_policy_sha256"],
            )


@pytest.mark.parametrize("generation_signer", ["unsigned", "foreign"])
def test_f_requires_active_generation_signature_under_installed_key(tmp_path, monkeypatch, generation_signer):
    selected, request = selection_fixture(tmp_path.resolve())
    manifest = b"installed four-role release"
    key = Ed25519PrivateKey.generate()
    active = signed_product_active(Ed25519PrivateKey.generate(), tmp_path, sha(manifest))
    if generation_signer == "unsigned":
        active = json.dumps({"payload": json.loads(active)["payload"]}).encode()
    payload = approval.binding(selected, request["selection_sha256"], request["decision_policy_sha256"],
                               sha(active), sha(manifest))
    signed_q_approval = {"payload": payload, "signature_hex": key.sign(approval.signing_bytes(payload)).hex()}
    installed = {
        "active-generation.json": active,
        "deployment-public-key.bin": key.public_key().public_bytes_raw(),
        "release-manifest.json": manifest,
    }
    monkeypatch.setattr(approval, "_installed_bytes", lambda path, _limit: installed[path.name])

    with pytest.raises(ValueError, match="installed Product generation signature"):
        approval.verify_installed_approval(
            signed_q_approval, selected, request["selection_sha256"], request["decision_policy_sha256"],
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


@pytest.mark.parametrize(("changed_input", "expected_error"), [
    ("policy", "release selection or decision policy changed"),
    ("selection", "release selection or decision policy changed"),
    ("config", "native config SHA-256 changed"),
    ("binary", "selected binary SHA-256 changed"),
])
def test_d_refuses_changed_inputs_after_signing(tmp_path, monkeypatch, changed_input, expected_error):
    selected, request = selection_fixture(tmp_path.resolve())
    key = Ed25519PrivateKey.generate()
    path = {
        "policy": Path(request["decision_policy_path"]),
        "selection": Path(request["selection_path"]),
        "config": selected.native_config,
        "binary": selected.binary,
    }[changed_input]
    monkeypatch.setattr(launcher, "_run_selected_command", Mock(return_value=0))

    def sign_with_changed_input(message):
        path.write_bytes(path.read_bytes() + b"\n")
        return key.sign(message)

    with pytest.raises((ValueError, launcher.SelectionError), match=expected_error):
        approval.approve_selected_install(
            request, q_sha256=selected.binary_sha256, active_sha256="1" * 64,
            manifest_sha256="2" * 64, sign=sign_with_changed_input, launcher=launcher,
        )


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
