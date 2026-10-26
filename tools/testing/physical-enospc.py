#!/usr/bin/env python3
"""Run an explicit test command on a disposable, bounded macOS filesystem.

Only the ENOSPC destination uses the image. Binaries, source fixtures, Cargo
outputs and logs remain on the host filesystem. The Rust test independently
checks the mount boundary and capacity before exhausting the volume.
The command must await its own workers; custody covers its owned process group.
"""

from __future__ import annotations

import argparse
import os
import platform
import plistlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools.benchmark.producer_execution import _execute_owned  # noqa: E402


class ImageAttachment:
    def __init__(self, device: str, mounts: tuple[Path, ...]) -> None:
        self.device = device
        self.mounts = mounts


def volume_command(*arguments: str) -> None:
    subprocess.run(["/usr/bin/hdiutil", *arguments], check=True, timeout=60)


def attached_image(image: Path) -> ImageAttachment | None:
    """Identify this backing image through hdiutil's device inventory."""
    result = subprocess.run(
        ["/usr/bin/hdiutil", "info", "-plist"],
        check=True,
        capture_output=True,
        timeout=60,
    )
    info = plistlib.loads(result.stdout)
    if not isinstance(info, dict) or not isinstance(info.get("images"), list):
        raise RuntimeError("hdiutil info did not return an image inventory")
    own_image = image.resolve()
    matches = []
    for record in info["images"]:
        if not isinstance(record, dict) or not isinstance(record.get("image-path"), str):
            raise RuntimeError("hdiutil info contains an image without a path")
        if Path(record["image-path"]).resolve() != own_image:
            continue
        entities = record.get("system-entities")
        if not isinstance(entities, list):
            raise RuntimeError("attached image has no device inventory")
        whole_disks = []
        mounts = []
        for entity in entities:
            if not isinstance(entity, dict):
                raise RuntimeError("attached image has a malformed device entry")
            device = entity.get("dev-entry")
            if isinstance(device, str) and re.fullmatch(r"/dev/disk[0-9]+", device):
                whole_disks.append(device)
            mount = entity.get("mount-point")
            if isinstance(mount, str):
                mounts.append(Path(mount).resolve())
        if len(set(whole_disks)) != 1:
            raise RuntimeError("attached image has no unique whole-disk device")
        matches.append(ImageAttachment(whole_disks[0], tuple(mounts)))
    if len(matches) > 1:
        raise RuntimeError("hdiutil reports duplicate attachments for one backing image")
    return matches[0] if matches else None


def detach_image(image: Path) -> None:
    """Detach only this image's observed disk and prove its later absence."""
    attachment = attached_image(image)
    if attachment is None:
        return
    try:
        volume_command("detach", attachment.device)
    except (subprocess.SubprocessError, OSError):
        current = attached_image(image)
        if current is not None:
            volume_command("detach", "-force", current.device)
    if attached_image(image) is not None:
        raise RuntimeError(f"image still attached after detaching {attachment.device}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--max-seconds", type=int, default=7200)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    command = arguments.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("an explicit test command is required after --")
    if arguments.max_seconds <= 0:
        parser.error("max-seconds must be a positive integer")
    if platform.system() != "Darwin":
        parser.error("this volume driver requires macOS hdiutil")

    if Path(tempfile.gettempdir()).resolve().is_relative_to(REPO_ROOT):
        parser.error("temporary volume storage must be outside the checkout")

    def interrupt_for_sigterm(_signum, _frame):
        raise RuntimeError("physical ENOSPC driver interrupted by SIGTERM")

    previous_term = signal.signal(signal.SIGTERM, interrupt_for_sigterm)
    previous_int = signal.signal(signal.SIGINT, signal.default_int_handler)
    try:
        directory = Path(tempfile.mkdtemp(prefix="qidx-physical-enospc-")).resolve()
        image = directory / "bounded.dmg"
        mount = directory / "mount"
        create_attempted = False
        create_succeeded = False
        attach_attempted = False
        attach_succeeded = False
        identity_observed = False
        command_started = False
        command_complete = False
        try:
            mount.mkdir()
            # A timed-out create can still leave work in the disk-image service.
            # Retain its backing path unless the create command completed.
            create_attempted = True
            volume_command(
                "create",
                "-size",
                "64m",
                "-fs",
                "HFS+",
                "-type",
                "UDIF",
                "-volname",
                "qidx-physical-enospc",
                "-quiet",
                str(image),
            )
            create_succeeded = True
            # A timed-out attach client may leave the disk-image service running.
            # Its backing image remains until that ambiguity is resolved outside
            # this process, even when a current inventory snapshot shows absence.
            attach_attempted = True
            volume_command("attach", "-nobrowse", "-mountpoint", str(mount), str(image))
            attach_succeeded = True
            attachment = attached_image(image)
            if attachment is None:
                raise RuntimeError("successful attach has no backing-image device in hdiutil info")
            identity_observed = True
            if mount.resolve() not in attachment.mounts:
                raise RuntimeError("backing image did not mount at its requested path")
            if mount.stat().st_dev == directory.stat().st_dev:
                raise RuntimeError("image attachment did not create a separate filesystem")
            environment = os.environ.copy()
            environment["QUANTA_INDEX_PHYSICAL_ENOSPC_ROOT"] = str(mount)
            command_started = True
            with os.fdopen(os.dup(1), "wb", buffering=0) as stdout:
                with os.fdopen(os.dup(2), "wb", buffering=0) as stderr:
                    result = _execute_owned(
                        command,
                        cwd=Path.cwd(),
                        env=environment,
                        timeout=arguments.max_seconds,
                        sinks=(stdout, stderr),
                    )
            command_complete = True
            exit_code = result["exit_code"]
            return 128 - exit_code if exit_code < 0 else exit_code
        finally:
            # Complete image custody before reporting cancellation received
            # during cleanup. Record signals instead of discarding them.
            primary_error = sys.exc_info()[1]
            cleanup_signals = set()

            def record_cleanup_signal(signum, _frame):
                cleanup_signals.add(signum)

            blocked = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGTERM, signal.SIGINT})
            signal.signal(signal.SIGTERM, record_cleanup_signal)
            signal.signal(signal.SIGINT, record_cleanup_signal)
            signal.pthread_sigmask(signal.SIG_SETMASK, blocked)
            try:
                if command_started and not command_complete:
                    cause = sys.exc_info()[1]
                    raise RuntimeError(
                        f"test command cleanup is unconfirmed; retained {directory}: {cause}"
                    ) from cause
                if create_attempted and not create_succeeded:
                    cause = sys.exc_info()[1]
                    raise RuntimeError(
                        f"image creation outcome is uncertain; retained {directory}: {cause}"
                    ) from cause
                # Mountpoint st_dev is not proof that an image is detached: a child
                # can unmount it while its /dev/disk device remains attached. Never
                # remove the backing file without an image-identity inventory proof.
                try:
                    if attach_attempted:
                        if not attach_succeeded:
                            raise RuntimeError("attachment outcome remains uncertain")
                        if not identity_observed:
                            raise RuntimeError(
                                "successful attach lacked a witnessed image identity"
                            )
                        detach_image(image)
                    try:
                        mount_device = mount.stat().st_dev
                    except FileNotFoundError:
                        mount_device = None
                    if mount_device is not None and mount_device != directory.stat().st_dev:
                        raise RuntimeError("mountpoint still has a separate filesystem")
                    shutil.rmtree(directory)
                except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
                    raise RuntimeError(
                        f"image custody is uncertain; retained {directory}: {error}"
                    ) from error
            finally:
                # Deliver pending signals to the recorder before restoring the
                # driver's handlers. The outer finally restores caller handlers.
                signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGTERM, signal.SIGINT})
                signal.pthread_sigmask(signal.SIG_SETMASK, blocked)
                signal.signal(signal.SIGTERM, interrupt_for_sigterm)
                signal.signal(signal.SIGINT, signal.default_int_handler)
            if cleanup_signals and primary_error is None:
                if signal.SIGINT in cleanup_signals:
                    raise KeyboardInterrupt
                raise RuntimeError("physical ENOSPC driver interrupted by SIGTERM during cleanup")
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        signal.signal(signal.SIGINT, previous_int)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"physical ENOSPC setup/cleanup failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
