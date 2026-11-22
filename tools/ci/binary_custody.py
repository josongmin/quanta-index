"""Pin the exact release daemon bytes used by a paired-process proof.

The private copy protects against shared build-output replacement. It is not
isolation from an adversary controlling the same UID or the host kernel.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.ci.lint.handoff_validation import (  # noqa: E402
    _open_repo_output_parent,
    _read_repo_regular_bytes,
    _sha256_repo_regular_file,
)


def _absolute_parts(path: Path) -> tuple[Path, str]:
    # Do not resolve symlinks: the descriptor walker must reject them.
    absolute = path.absolute()
    root = Path(absolute.anchor)
    return root, absolute.relative_to(root).as_posix()


def verify(digest: str, paths: list[Path], *, archive: Path | None = None) -> None:
    if re.fullmatch(r"[0-9a-f]{64}", digest) is None or not paths:
        raise ValueError("binary custody requires a SHA-256 and named files")
    if archive is not None and archive not in paths:
        raise ValueError("P10 archive is missing from binary custody")
    for path in dict.fromkeys(paths):
        root, relative = _absolute_parts(path)
        if _sha256_repo_regular_file(root, relative, label="proof daemon") != digest:
            raise ValueError(f"release daemon bytes changed during proof: {path}")
        if path != archive and not os.access(path, os.X_OK):
            raise ValueError(f"proof daemon is no longer executable: {path}")


def pin(
    built: Path, provided: Path, destination: Path, *, archive_sha256: str | None = None
) -> str:
    root, relative = _absolute_parts(built)
    raw = _read_repo_regular_bytes(root, relative, label="release daemon source")
    digest = hashlib.sha256(raw).hexdigest()
    archive = built if archive_sha256 is not None else None
    if archive_sha256 is not None and digest != archive_sha256:
        raise ValueError("P10 archived daemon differs from its approved SHA-256")
    verify(digest, [built, provided], archive=archive)
    output_root, output_relative = _absolute_parts(destination)
    parent_fd = _open_repo_output_parent(output_root, output_relative, create=False)
    try:
        parent = os.fstat(parent_fd)
        if parent.st_uid != os.geteuid() or parent.st_mode & 0o077:
            raise ValueError("daemon custody directory must be private and owned by this UID")
        descriptor = os.open(
            destination.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o500,
            dir_fd=parent_fd,
        )
        try:
            with os.fdopen(descriptor, "wb") as handle:
                handle.write(raw)
                handle.flush()
                os.fsync(handle.fileno())
            os.fsync(parent_fd)
            verify(digest, [built, provided, destination], archive=archive)
        except BaseException:
            # Cleanup belongs only to the invocation that won O_EXCL above.
            os.unlink(destination.name, dir_fd=parent_fd)
            os.fsync(parent_fd)
            raise
    finally:
        os.close(parent_fd)
    return digest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    pin_command = commands.add_parser("pin")
    for name in ("built", "provided", "destination"):
        pin_command.add_argument(name, type=Path)
    pin_command.add_argument("--archive-sha256")
    verify_command = commands.add_parser("verify")
    verify_command.add_argument("digest")
    verify_command.add_argument("paths", nargs="+", type=Path)
    verify_command.add_argument("--archive", type=Path)
    args = parser.parse_args()
    try:
        if args.operation == "pin":
            print(pin(args.built, args.provided, args.destination, archive_sha256=args.archive_sha256))
        else:
            verify(args.digest, args.paths, archive=args.archive)
    except (OSError, ValueError) as error:
        parser.exit(1, f"binary custody: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
