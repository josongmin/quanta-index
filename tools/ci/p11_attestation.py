"""Detached OpenSSH attestations from the registered P11 producer identity."""

from __future__ import annotations

import base64
import binascii
import json
import os
import stat
import subprocess
import tempfile
from collections.abc import MutableMapping
from pathlib import Path

SOCKET_ENV = "QUANTA_P11_SIGNER_SOCKET"
KEYGEN = "/usr/bin/ssh-keygen"


def remove_signing_agent(
    environment: MutableMapping[str, str], signer_socket: str | None
) -> str | None:
    """Keep an ambient agent only if it is a separate Unix socket."""
    ambient = environment.get("SSH_AUTH_SOCK")
    if not signer_socket or not ambient:
        return None
    try:
        signer_stat, ambient_stat = os.stat(signer_socket), os.stat(ambient)
        if (
            stat.S_ISSOCK(signer_stat.st_mode)
            and stat.S_ISSOCK(ambient_stat.st_mode)
            and not os.path.samestat(signer_stat, ambient_stat)
        ):
            return None
    except OSError:
        pass
    return environment.pop("SSH_AUTH_SOCK")


def public_key(value: object) -> str:
    if not isinstance(value, str):
        raise ValueError("registered P11 Ed25519 public key required")
    fields = value.split(" ")
    try:
        blob = base64.b64decode(fields[1], validate=True) if len(fields) == 2 else b""
    except (ValueError, binascii.Error):
        blob = b""
    prefix = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20"
    if fields[0] != "ssh-ed25519" or not blob.startswith(prefix) or len(blob) != len(prefix) + 32:
        raise ValueError("invalid registered P11 Ed25519 public key")
    return value


def _keygen(args: list[str], message: bytes, *, socket: str | None = None) -> bytes:
    env = {"PATH": os.defpath, "LC_ALL": "C", "SSH_ASKPASS_REQUIRE": "never"}
    if socket is not None:
        env["SSH_AUTH_SOCK"] = socket
    try:
        proc = subprocess.run(
            [KEYGEN, *args], input=message, capture_output=True, timeout=15, env=env
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError("P11 attestation tool unavailable") from error
    if proc.returncode:
        raise ValueError("P11 producer attestation could not be signed or verified")
    return proc.stdout


def sign(key: str, proof_id: str, message: bytes, *, socket: str | None = None) -> bytes:
    """Sign only with the public key's matching identity in the producer's agent."""
    socket = socket or os.environ.get(SOCKET_ENV)
    if not socket or not Path(socket).is_absolute():
        raise ValueError("registered P11 producer signing agent required")
    with tempfile.TemporaryDirectory() as tmp:
        identity = Path(tmp) / "issuer.pub"
        identity.write_text(public_key(key) + "\n", encoding="ascii")
        signature = _keygen(
            ["-Y", "sign", "-f", str(identity), "-n", f"quanta/{proof_id}/v1"],
            message,
            socket=socket,
        )
    verify(key, proof_id, message, signature)
    return signature


def verify(key: str, proof_id: str, message: bytes, signature: bytes) -> None:
    if not 0 < len(signature) <= 4096:
        raise ValueError("invalid P11 producer attestation length")
    with tempfile.TemporaryDirectory() as tmp:
        signers = Path(tmp) / "allowed"
        signers.write_text("producer " + public_key(key) + "\n", encoding="ascii")
        signature_file = Path(tmp) / "signature"
        signature_file.write_bytes(signature)
        _keygen(
            [
                "-Y",
                "verify",
                "-I",
                "producer",
                "-n",
                f"quanta/{proof_id}/v1",
                "-f",
                str(signers),
                "-s",
                str(signature_file),
            ],
            message,
        )


def artifact_statement(digests: dict[str, str]) -> bytes:
    return json.dumps(digests, sort_keys=True, separators=(",", ":")).encode("ascii")
