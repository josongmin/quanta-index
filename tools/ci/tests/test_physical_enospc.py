"""Unknown attachment must not destroy the backing image's custody."""

import importlib.util
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

DRIVER = Path(__file__).resolve().parents[2] / "testing" / "physical-enospc.py"
SPEC = importlib.util.spec_from_file_location("physical_enospc", DRIVER)
driver = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = driver
SPEC.loader.exec_module(driver)


@contextmanager
def fake_volume(directory: Path, *, before_detach=None):
    """Supply image inventory and a different device without mounting a volume."""
    attached = {"current": False}
    events = []

    def image_command(*arguments):
        events.append(arguments[0])
        mount = directory / "mount"
        if arguments[0] == "create":
            Path(arguments[-1]).write_bytes(b"owned backing fixture")
        elif arguments[0] == "attach":
            mount.rmdir()
            mount.symlink_to("/dev")
            attached["current"] = True
        elif arguments[0] == "detach":
            if before_detach is not None:
                before_detach()
            mount.unlink()
            mount.mkdir()
            attached["current"] = False
        else:
            raise AssertionError(f"unexpected image command {arguments}")

    def image_state(_image):
        return (
            driver.ImageAttachment("/dev/disk99", (Path("/dev"),)) if attached["current"] else None
        )

    with (
        patch.object(driver.platform, "system", return_value="Darwin"),
        patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
        patch.object(driver, "volume_command", side_effect=image_command),
        patch.object(driver, "attached_image", side_effect=image_state),
    ):
        yield events


def process_is_gone_or_zombie(pid: int) -> bool:
    observed = subprocess.run(
        ["ps", "-o", "stat=", "-p", str(pid)],
        capture_output=True,
        text=True,
        check=False,
        timeout=5,
    )
    return not observed.stdout.strip() or observed.stdout.strip().startswith("Z")


def nested_command(pid_file: Path, *, child_waits: bool) -> list[str]:
    grandchild = (
        "import os,time;from pathlib import Path;"
        f"Path({str(pid_file)!r}).write_text(str(os.getpid()));"
        "time.sleep(600)"
    )
    producer = (
        "import subprocess,sys,time;from pathlib import Path;"
        f"subprocess.Popen([sys.executable,'-c',{grandchild!r}]);"
        f"p=Path({str(pid_file)!r});"
        "\nwhile not p.exists(): time.sleep(0.01)\n"
        + ("time.sleep(600)" if child_waits else "sys.exit(0)")
    )
    return [sys.executable, "-c", producer]


class UncertainAttachmentTest(unittest.TestCase):
    def test_failed_attach_retains_backing_path_without_starting_the_test(self):
        # No real hdiutil command or mount occurs here. This regression models
        # the service/client ambiguity that a device check alone cannot rule out.
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()

            def image_command(*arguments):
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")
                else:
                    self.assertEqual(arguments[0], "attach")
                    raise subprocess.TimeoutExpired("hdiutil attach", 60)

            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", return_value=None),
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/false"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "attachment outcome remains uncertain"):
                    driver.main()
                child.assert_not_called()
            self.assertEqual((directory / "bounded.dmg").read_bytes(), b"owned backing fixture")
            self.assertTrue((directory / "mount").is_dir())

    def test_uncertain_attach_never_detaches_even_when_device_is_visible(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            events = []

            def image_command(*arguments):
                events.append(arguments[0])
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")
                elif arguments[0] == "attach":
                    raise subprocess.TimeoutExpired("hdiutil attach", 60)
                else:
                    self.fail("uncertain attachment reached detach")

            known = driver.ImageAttachment("/dev/disk99", ())
            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", return_value=known) as inventory,
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/false"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "attachment outcome remains uncertain"):
                    driver.main()
                inventory.assert_not_called()
                child.assert_not_called()
            self.assertEqual(events, ["create", "attach"])
            self.assertEqual((directory / "bounded.dmg").read_bytes(), b"owned backing fixture")

    def test_success_without_a_witnessed_image_device_retains_backing(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()

            def image_command(*arguments):
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")
                else:
                    self.assertEqual(arguments[0], "attach")

            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", return_value=None),
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/false"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "lacked a witnessed image identity"):
                    driver.main()
                child.assert_not_called()
            self.assertEqual((directory / "bounded.dmg").read_bytes(), b"owned backing fixture")

    def test_known_image_without_requested_mount_detaches_by_device_before_cleanup(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            calls = []

            def image_command(*arguments):
                calls.append(arguments)
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")

            known = driver.ImageAttachment("/dev/disk99", ())
            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", side_effect=[known, known, None]),
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/false"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "did not mount at its requested path"):
                    driver.main()
                child.assert_not_called()
            self.assertIn(("detach", "/dev/disk99"), calls)
            self.assertFalse(directory.exists())

    def test_detach_error_retains_the_backing_image(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            known = driver.ImageAttachment("/dev/disk99", ())

            def image_command(*arguments):
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")
                elif arguments[0] == "detach":
                    raise subprocess.CalledProcessError(1, ["hdiutil", *arguments])

            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", side_effect=[known, known, known]),
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/false"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "image custody is uncertain"):
                    driver.main()
                child.assert_not_called()
            self.assertEqual((directory / "bounded.dmg").read_bytes(), b"owned backing fixture")

    def test_failed_detach_rechecks_image_identity_before_force(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            calls = []

            def image_command(*arguments):
                calls.append(arguments)
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")
                elif arguments[:2] == ("detach", "/dev/disk99"):
                    raise subprocess.CalledProcessError(1, ["hdiutil", *arguments])

            old = driver.ImageAttachment("/dev/disk99", ())
            current = driver.ImageAttachment("/dev/disk100", ())
            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", side_effect=[old, old, current, None]),
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/false"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "did not mount at its requested path"):
                    driver.main()
                child.assert_not_called()
            self.assertIn(("detach", "-force", "/dev/disk100"), calls)
            self.assertNotIn(("detach", "-force", "/dev/disk99"), calls)
            self.assertFalse(directory.exists())

    def test_separate_mountpoint_after_image_absence_preserves_backing(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()

            def image_command(*arguments):
                if arguments[0] == "create":
                    Path(arguments[-1]).write_bytes(b"owned backing fixture")
                elif arguments[0] == "attach":
                    mount = directory / "mount"
                    mount.rmdir()
                    os.symlink("/dev", mount)

            def image_state(_image):
                image_state.calls += 1
                if image_state.calls == 1:
                    return driver.ImageAttachment("/dev/disk99", (Path("/dev"),))
                return None

            image_state.calls = 0
            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=image_command),
                patch.object(driver, "attached_image", side_effect=image_state),
                patch.object(driver, "_execute_owned", return_value={"exit_code": 0}) as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/true"]),
            ):
                with self.assertRaisesRegex(
                    RuntimeError, "mountpoint still has a separate filesystem"
                ):
                    driver.main()
                child.assert_called_once()
            self.assertEqual((directory / "bounded.dmg").read_bytes(), b"owned backing fixture")


class OwnedCommandTest(unittest.TestCase):
    def test_cleanup_signal_is_reported_after_image_detach(self):
        for cancellation in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(cancellation=cancellation):
                with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
                    directory = Path(parent) / "owned"
                    directory.mkdir()
                    previous = (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT))

                    def interrupt_during_detach(selected_signal=cancellation):
                        signal.raise_signal(selected_signal)

                    with fake_volume(directory, before_detach=interrupt_during_detach) as events:
                        with (
                            patch.object(driver, "_execute_owned", return_value={"exit_code": 0}),
                            patch("sys.argv", [str(DRIVER), "--", "/usr/bin/true"]),
                        ):
                            expected = (
                                KeyboardInterrupt if cancellation == signal.SIGINT else RuntimeError
                            )
                            with self.assertRaises(expected):
                                driver.main()
                    self.assertEqual(events, ["create", "attach", "detach"])
                    self.assertFalse(directory.exists())
                    self.assertEqual(
                        (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT)),
                        previous,
                    )

    def test_signaled_command_exit_uses_shell_signal_code(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            with fake_volume(directory) as events:
                with (
                    patch.object(
                        driver, "_execute_owned", return_value={"exit_code": -signal.SIGTERM}
                    ),
                    patch("sys.argv", [str(DRIVER), "--", "/usr/bin/true"]),
                ):
                    self.assertEqual(driver.main(), 128 + signal.SIGTERM)
            self.assertEqual(events, ["create", "attach", "detach"])
            self.assertFalse(directory.exists())

    def test_grandchild_is_stopped_before_image_detach(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            pid_file = Path(parent) / "grandchild.pid"

            def assert_quiescent():
                self.assertTrue(pid_file.is_file())
                self.assertTrue(process_is_gone_or_zombie(int(pid_file.read_text())))

            command = nested_command(pid_file, child_waits=False)
            previous = (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT))
            with fake_volume(directory, before_detach=assert_quiescent) as events:
                with patch("sys.argv", [str(DRIVER), "--max-seconds", "5", "--", *command]):
                    self.assertEqual(driver.main(), 0)
            self.assertEqual(events, ["create", "attach", "detach"])
            self.assertFalse(directory.exists())
            self.assertEqual(
                (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT)), previous
            )

    def test_timeout_keeps_image_when_owned_execution_raises(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            pid_file = Path(parent) / "grandchild.pid"
            command = nested_command(pid_file, child_waits=True)
            with fake_volume(directory) as events:
                with patch("sys.argv", [str(DRIVER), "--max-seconds", "1", "--", *command]):
                    with self.assertRaisesRegex(RuntimeError, "cleanup is unconfirmed; retained"):
                        driver.main()
            self.assertEqual(events, ["create", "attach"])
            self.assertTrue((directory / "bounded.dmg").is_file())
            self.assertTrue(pid_file.is_file())
            self.assertTrue(process_is_gone_or_zombie(int(pid_file.read_text())))

    def test_sigint_or_sigterm_during_owned_child_preserves_image(self):
        for cancellation in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(cancellation=cancellation):
                with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
                    directory = Path(parent) / "owned"
                    directory.mkdir()
                    pid_file = Path(parent) / "grandchild.pid"
                    command = nested_command(pid_file, child_waits=True)
                    previous = (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT))

                    def interrupt_after_spawn(
                        selected_pid_file=pid_file, selected_signal=cancellation
                    ):
                        deadline = time.monotonic() + 4
                        while not selected_pid_file.exists() and time.monotonic() < deadline:
                            time.sleep(0.01)
                        if selected_pid_file.exists():
                            os.kill(os.getpid(), selected_signal)

                    interrupt = threading.Thread(target=interrupt_after_spawn, daemon=True)
                    with fake_volume(directory) as events:
                        with patch("sys.argv", [str(DRIVER), "--max-seconds", "5", "--", *command]):
                            interrupt.start()
                            with self.assertRaisesRegex(
                                RuntimeError, "cleanup is unconfirmed; retained"
                            ):
                                driver.main()
                            interrupt.join(timeout=5)
                    self.assertFalse(interrupt.is_alive())
                    self.assertEqual(events, ["create", "attach"])
                    self.assertTrue((directory / "bounded.dmg").is_file())
                    self.assertTrue(pid_file.is_file())
                    self.assertTrue(process_is_gone_or_zombie(int(pid_file.read_text())))
                    self.assertEqual(
                        (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGINT)),
                        previous,
                    )

    def test_sigterm_during_create_retains_uncertain_backing_without_starting_command(self):
        with tempfile.TemporaryDirectory(prefix="qidx-volume-driver-law-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()

            def interrupted_create(*arguments):
                self.assertEqual(arguments[0], "create")
                Path(arguments[-1]).write_bytes(b"unfinished image fixture")
                signal.raise_signal(signal.SIGTERM)

            with (
                patch.object(driver.platform, "system", return_value="Darwin"),
                patch.object(driver.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(driver, "volume_command", side_effect=interrupted_create),
                patch.object(driver, "_execute_owned") as child,
                patch("sys.argv", [str(DRIVER), "--", "/usr/bin/true"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "image creation outcome is uncertain"):
                    driver.main()
                child.assert_not_called()
            self.assertEqual((directory / "bounded.dmg").read_bytes(), b"unfinished image fixture")

    def test_nonpositive_deadline_refuses_before_any_image_command(self):
        with (
            patch.object(driver.platform, "system", return_value="Darwin"),
            patch.object(driver.tempfile, "mkdtemp") as create,
            patch("sys.argv", [str(DRIVER), "--max-seconds", "0", "--", "/usr/bin/true"]),
        ):
            with self.assertRaises(SystemExit) as refusal:
                driver.main()
            self.assertEqual(refusal.exception.code, 2)
            create.assert_not_called()


if __name__ == "__main__":
    unittest.main()
