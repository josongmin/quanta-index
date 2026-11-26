"""Bind an admitted Native selection to the existing installed Product key."""

import hashlib
import json
import os
import re
import stat
from pathlib import Path

SCHEMA = "semantica-installed-native-q-approval/v1"
DOMAIN = b"semantica.installed-native-q-approval.v1\n"
RETIREMENT_SCHEMA = "semantica-installed-native-q-approval/v2"
RETIREMENT_DOMAIN = b"semantica.installed-native-q-approval.v2\n"
PRODUCT_ACTIVE_SCHEMA = "semantica-installed-product-active/v1"
PRODUCT_ACTIVE_DOMAIN = b"semantica.installed-product-active.v1\n"
PRODUCT_ACTIVE_SCHEMA_V2 = "semantica-installed-product-active/v2"
PRODUCT_ACTIVE_DOMAIN_V2 = b"semantica.installed-product-active.v2\n"
PRODUCT_ACTIVE_SCHEMA_V3 = "semantica-installed-product-active/v3"
PRODUCT_ACTIVE_DOMAIN_V3 = b"semantica.installed-product-active.v3\n"
PRODUCT_ACTIVE_MAX_BYTES = 8 * 1024
TRUST = Path("/etc/semantica-codegraph-v2/trust")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SIGNATURE_HEX = re.compile(r"[0-9a-f]{128}\Z")
PRODUCT_ACTIVE_DIGESTS = frozenset(
    {
        "ag5_active_record_sha256",
        "release_manifest_sha256",
        "cli_sha256",
        "server_sha256",
        "sql_rulepack_worker_sha256",
        "parser_native_worker_sha256",
    }
)
PRODUCT_ACTIVE_FIELDS = PRODUCT_ACTIVE_DIGESTS | {"schema_version", "install_root", "generation"}
REQUEST_FIELDS = {
    "deployment_root",
    "selection_path",
    "selection_sha256",
    "decision_policy_path",
    "decision_policy_sha256",
}


def _canonical_bytes(value: object) -> bytes:
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
        allow_nan=False,
    ).encode("ascii")


def signing_bytes(payload: dict) -> bytes:
    schema = payload.get("schema_version")
    if schema == SCHEMA:
        domain = DOMAIN
    elif schema == RETIREMENT_SCHEMA:
        domain = RETIREMENT_DOMAIN
    else:
        raise ValueError("Native release approval schema differs")
    return domain + _canonical_bytes(payload)


def _valid_repository_root(root: object) -> bool:
    return (
        isinstance(root, str)
        and root.isascii()
        and root.startswith("/")
        and all(32 <= ord(character) < 127 for character in root)
        and all(part not in {"", ".", ".."} for part in root.split("/")[1:])
    )


def _valid_native_q_digests(value: dict) -> bool:
    return all(
        isinstance(value[name], str)
        and SHA256.fullmatch(value[name]) is not None
        and value[name] != "0" * 64
        for name in ("q_image_sha256", "q_config_sha256")
    )


def _valid_native_recovery(value: object) -> bool:
    return (
        isinstance(value, dict)
        and set(value)
        == {
            "analysis_family",
            "repository_root",
            "q_image_sha256",
            "q_config_sha256",
        }
        and value["analysis_family"] == "taint_rows_ifds"
        and _valid_repository_root(value["repository_root"])
        and _valid_native_q_digests(value)
    )


def _valid_native_facts_publication(value: object) -> bool:
    return (
        isinstance(value, dict)
        and set(value) == {"repository_root", "q_image_sha256", "q_config_sha256"}
        and _valid_repository_root(value["repository_root"])
        and _valid_native_q_digests(value)
    )


def verify_installed_product_generation(raw: bytes, key: bytes) -> dict:
    """Authenticate the installed S Product record before consuming its Q approval."""
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    if len(raw) > PRODUCT_ACTIVE_MAX_BYTES:
        raise ValueError("installed Product generation exceeds its byte bound")
    envelope = json.loads(raw)
    if not isinstance(envelope, dict) or set(envelope) != {"payload", "signature_hex"}:
        raise ValueError("installed Product generation signature envelope differs")
    payload, signature = envelope["payload"], envelope["signature_hex"]
    if not isinstance(signature, str) or SIGNATURE_HEX.fullmatch(signature) is None:
        raise ValueError("installed Product generation signature is malformed")
    if not isinstance(payload, dict):
        raise ValueError("installed Product generation payload differs")
    schema = payload.get("schema_version")
    if schema == PRODUCT_ACTIVE_SCHEMA and set(payload) == PRODUCT_ACTIVE_FIELDS:
        domain = PRODUCT_ACTIVE_DOMAIN
    elif (
        schema == PRODUCT_ACTIVE_SCHEMA_V2
        and set(payload) == PRODUCT_ACTIVE_FIELDS | {"native_recovery"}
        and _valid_native_recovery(payload["native_recovery"])
    ):
        domain = PRODUCT_ACTIVE_DOMAIN_V2
    elif (
        schema == PRODUCT_ACTIVE_SCHEMA_V3
        and set(payload) == PRODUCT_ACTIVE_FIELDS | {"native_facts_publication"}
        and _valid_native_facts_publication(payload["native_facts_publication"])
    ):
        domain = PRODUCT_ACTIVE_DOMAIN_V3
    else:
        raise ValueError("installed Product generation payload differs")
    if (
        type(payload["generation"]) is not int
        or payload["generation"] < 0
        or not isinstance(payload["install_root"], str)
        or not payload["install_root"].isascii()
        or any(
            not isinstance(payload[field], str) or SHA256.fullmatch(payload[field]) is None
            for field in PRODUCT_ACTIVE_DIGESTS
        )
        or raw != _canonical_bytes(envelope) + b"\n"
    ):
        raise ValueError("installed Product generation payload differs")
    try:
        Ed25519PublicKey.from_public_bytes(key).verify(
            bytes.fromhex(signature),
            domain + _canonical_bytes(payload),
        )
    except (InvalidSignature, ValueError) as error:
        raise ValueError("installed Product generation signature differs") from error
    return payload


def require_matching_installed_recovery(active, q_image_sha256, q_config_sha256):
    for field in ("native_recovery", "native_facts_publication"):
        approval = active.get(field)
        if approval is not None and (
            approval["q_image_sha256"] != q_image_sha256
            or approval["q_config_sha256"] != q_config_sha256
        ):
            raise ValueError("Native release selection differs from installed recovery approval")


def binding(
    selection,
    selection_sha256,
    policy_sha256,
    active_sha256,
    manifest_sha256,
    *,
    selection_path=None,
):
    if selection.native_config is None or selection.native_boot_receipt is None:
        raise ValueError("Native release approval requires a selected BOOT")
    digests = (
        selection_sha256,
        policy_sha256,
        active_sha256,
        manifest_sha256,
        selection.binary_sha256,
        selection.native_config_sha256,
    )
    if any(not isinstance(value, str) or SHA256.fullmatch(value) is None for value in digests):
        raise ValueError("Native release approval digest differs")
    retirement = selection.native_boot_receipt.retired_policy_object_hex is not None
    if retirement and selection.to_wire().get("schema_version") != 3:
        raise ValueError("Native retirement approval requires schema-3 selection")
    payload = {
        "schema_version": RETIREMENT_SCHEMA if retirement else SCHEMA,
        "active_generation_sha256": active_sha256,
        "release_manifest_sha256": manifest_sha256,
        "q_elf_sha256": selection.binary_sha256,
        "selection_sha256": selection_sha256,
        "state_root": str(selection.state_root),
        "boot_receipt": selection.native_boot_receipt.to_wire(),
        "config_sha256": selection.native_config_sha256,
        "decision_policy_sha256": policy_sha256,
    }
    if retirement:
        if selection_path is None:
            raise ValueError("Native retirement approval requires its selected file path")
        selection_path = Path(selection_path)
        try:
            canonical_selection = selection_path.resolve(strict=True)
        except (OSError, RuntimeError) as error:
            raise ValueError("Native retirement approval selection path differs") from error
        if not selection_path.is_absolute() or canonical_selection != selection_path:
            raise ValueError("Native retirement approval selection path differs")
        receipt = selection.native_boot_receipt
        payload["selection_path"] = str(selection_path)
        payload["retired_projection_policy"] = {
            "object_hex": receipt.retired_policy_object_hex,
            "sha256_hex": receipt.retired_policy_sha256_hex,
            "identity_count": receipt.retired_policy_identity_count,
        }
    return payload


def _under_root(path, root):
    path = Path(path)
    if not path.is_absolute() or path.resolve(strict=True) != path or root not in path.parents:
        raise ValueError("Native release input is outside its canonical deployment root")
    return path


def _request_paths(request):
    if (
        not isinstance(request, dict)
        or set(request) != REQUEST_FIELDS
        or any(not isinstance(value, str) or not value for value in request.values())
    ):
        raise ValueError("Native release approval request differs")
    root = Path(request["deployment_root"])
    if not root.is_absolute() or root.resolve(strict=True) != root or not root.is_dir():
        raise ValueError("Native release approval deployment root differs")
    selection_path = _under_root(request["selection_path"], root)
    policy = _under_root(request["decision_policy_path"], root)
    return root, selection_path, policy


def _read_request_input(path, sha256):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("Native release input is not a regular file")
        data = stream.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024 or hashlib.sha256(data).hexdigest() != sha256:
        raise ValueError("Native release selection or decision policy changed")
    return data


def _verify_request_digests(request, selection_path, policy):
    selection = _read_request_input(selection_path, request["selection_sha256"])
    _read_request_input(policy, request["decision_policy_sha256"])
    return selection


def _require_native_selection(root, selection_path, expected, q_sha256, launcher):
    selection = launcher._require_current_selection(selection_path, expected)
    if selection.binary_sha256 != q_sha256:
        raise ValueError("Native release selection differs from the approved Q ELF")
    if selection.native_config is None or selection.native_boot_receipt is None:
        raise ValueError("Native release approval requires a selected BOOT")
    for path in (selection.binary, selection.state_root, selection.native_config):
        _under_root(path, root)
    return selection


def verify_approval_request(request, *, q_sha256=None, launcher=None):
    """Check the pinned inputs and selected files at an approval boundary."""
    root, selection_path, policy = _request_paths(request)
    data = _verify_request_digests(request, selection_path, policy)
    if launcher is not None:
        selection_path = launcher._selection_path(selection_path)
        selection = _require_native_selection(
            root,
            selection_path,
            request["selection_sha256"],
            q_sha256,
            launcher,
        )
        launcher._check_binary(selection)
    elif q_sha256 is not None:
        selected = json.loads(data)
        if not isinstance(selected, dict) or selected.get("binary_sha256") != q_sha256:
            raise ValueError("Native release selection differs from the approved Q ELF")


def approve_selected_install(
    request, *, q_sha256, active_sha256, manifest_sha256, sign, launcher, receipt_path=None
):
    root, selection_path, policy = _request_paths(request)
    request = request.copy()
    expected = request["selection_sha256"]
    policy_sha = request["decision_policy_sha256"]

    with launcher.deployment_lock(selection_path) as lock_fd:
        _verify_request_digests(request, selection_path, policy)
        selection = _require_native_selection(root, selection_path, expected, q_sha256, launcher)
        payload = binding(
            selection,
            expected,
            policy_sha,
            active_sha256,
            manifest_sha256,
            selection_path=selection_path,
        )
        launcher._verify_bootstrap_locked(selection, lock_fd)
        verify_approval_request(request, q_sha256=q_sha256, launcher=launcher)
        signature_hex = sign(signing_bytes(payload)).hex()
        verify_approval_request(request, q_sha256=q_sha256, launcher=launcher)
        receipt = {"payload": payload, "signature_hex": signature_hex}
        if receipt_path is not None:
            if payload["schema_version"] != RETIREMENT_SCHEMA:
                raise ValueError("Native retirement receipt output requires schema-3 selection")
            target = Path(receipt_path)
            if (
                not target.is_absolute()
                or target.parent != root
                or target in {selection_path, policy}
                or target.name.startswith(".")
            ):
                raise ValueError("Native release approval output must be a deployment-root file")
            target = launcher._selection_path(target)
            encoded = _canonical_bytes(receipt) + b"\n"
            if len(encoded) > 8 * 1024:
                raise ValueError("Native release approval exceeds its installed byte bound")
            verify_installed_approval(
                receipt,
                selection,
                expected,
                policy_sha,
                selection_path=selection_path,
            )
            try:
                target.lstat()
            except FileNotFoundError:
                launcher._publish_control_file_locked(
                    target,
                    encoded,
                    lock_fd,
                    create_only=True,
                )
            else:
                launcher._sync_retained_publication(
                    target,
                    lock_fd,
                    encoded,
                    8 * 1024,
                    lambda data: data,
                )
            verify_approval_request(request, q_sha256=q_sha256, launcher=launcher)
        return receipt


def _installed_bytes(path: Path, limit: int) -> bytes:
    if not path.is_absolute() or path.resolve(strict=True) != path:
        raise ValueError("Native release approval requires a canonical installed file")
    for parent in reversed(path.parents):
        directory = parent.lstat()
        if (
            not stat.S_ISDIR(directory.st_mode)
            or directory.st_uid != 0
            or directory.st_mode & 0o022
        ):
            raise ValueError("Native release approval requires a protected installed directory")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        named = path.lstat()
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid != 0
            or info.st_mode & 0o022
            or not 0 < info.st_size <= limit
            or (info.st_dev, info.st_ino) != (named.st_dev, named.st_ino)
        ):
            raise ValueError("Native release approval requires a protected installed file")
        data = stream.read(limit + 1)
        if len(data) != info.st_size:
            raise ValueError("Native release approval installed file changed")
        return data


def verify_installed_approval(
    receipt, selection, selection_sha256, policy_sha256, *, selection_path=None
):
    """Compare approval with this installation's protected trust and actual inputs."""
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    if (
        not isinstance(receipt, dict)
        or set(receipt) != {"payload", "signature_hex"}
        or not isinstance(receipt["signature_hex"], str)
        or SIGNATURE_HEX.fullmatch(receipt["signature_hex"]) is None
    ):
        raise ValueError("Native release approval envelope differs")
    active = _installed_bytes(TRUST / "active-generation.json", PRODUCT_ACTIVE_MAX_BYTES)
    key = _installed_bytes(TRUST / "deployment-public-key.bin", 32)
    payload = verify_installed_product_generation(active, key)
    require_matching_installed_recovery(
        payload,
        selection.binary_sha256,
        selection.native_config_sha256,
    )
    manifest_sha = payload["release_manifest_sha256"]
    expected = binding(
        selection,
        selection_sha256,
        policy_sha256,
        hashlib.sha256(active).hexdigest(),
        manifest_sha,
        selection_path=selection_path,
    )
    expected_bytes = signing_bytes(expected)
    if (
        not isinstance(receipt["payload"], dict)
        or signing_bytes(receipt["payload"]) != expected_bytes
    ):
        raise ValueError("Native release approval differs from the selected installation")
    try:
        Ed25519PublicKey.from_public_bytes(key).verify(
            bytes.fromhex(receipt["signature_hex"]),
            expected_bytes,
        )
    except InvalidSignature as exc:
        raise ValueError("Native release approval signature differs") from exc
    manifest = (
        Path(payload["install_root"])
        / "releases"
        / manifest_sha
        / "share/quanta/release-manifest.json"
    )
    if hashlib.sha256(_installed_bytes(manifest, 1024 * 1024)).hexdigest() != manifest_sha:
        raise ValueError("Native release approval manifest changed")
    if active != _installed_bytes(
        TRUST / "active-generation.json", PRODUCT_ACTIVE_MAX_BYTES
    ) or key != _installed_bytes(TRUST / "deployment-public-key.bin", 32):
        raise ValueError("Native release approval installation changed")
