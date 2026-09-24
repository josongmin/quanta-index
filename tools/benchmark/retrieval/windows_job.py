"""Windows Job Object ownership prerequisite for benchmark sidecars.

The child is created suspended and cannot run before Job assignment succeeds.
This module is deliberately independent of the POSIX sampler in run.py; a
caller must supply its own output capture and resource evidence.
"""

from __future__ import annotations

import ctypes
import subprocess
import sys
import time
from ctypes import wintypes
from typing import Protocol

CREATE_SUSPENDED = 0x00000004
CREATE_UNICODE_ENVIRONMENT = 0x00000400
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JOB_OBJECT_EXTENDED_LIMIT_INFORMATION = 9
JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION = 1
WAIT_OBJECT_0 = 0
WAIT_TIMEOUT = 0x00000102
INFINITE = 0xFFFFFFFF


class JobError(RuntimeError):
    """Ownership or cleanup could not be established."""


class _BasicLimitInformation(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", ctypes.c_int64),
        ("PerJobUserTimeLimit", ctypes.c_int64),
        ("LimitFlags", wintypes.DWORD),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", wintypes.DWORD),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", wintypes.DWORD),
        ("SchedulingClass", wintypes.DWORD),
    ]


class _IoCounters(ctypes.Structure):
    _fields_ = [
        (name, ctypes.c_uint64)
        for name in (
            "ReadOperationCount",
            "WriteOperationCount",
            "OtherOperationCount",
            "ReadTransferCount",
            "WriteTransferCount",
            "OtherTransferCount",
        )
    ]


class _ExtendedLimitInformation(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", _BasicLimitInformation),
        ("IoInfo", _IoCounters),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


class _BasicAccountingInformation(ctypes.Structure):
    _fields_ = [
        ("TotalUserTime", ctypes.c_int64),
        ("TotalKernelTime", ctypes.c_int64),
        ("ThisPeriodTotalUserTime", ctypes.c_int64),
        ("ThisPeriodTotalKernelTime", ctypes.c_int64),
        ("TotalPageFaultCount", wintypes.DWORD),
        ("TotalProcesses", wintypes.DWORD),
        ("ActiveProcesses", wintypes.DWORD),
        ("TotalTerminatedProcesses", wintypes.DWORD),
    ]


class _StartupInfo(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("lpReserved", wintypes.LPWSTR),
        ("lpDesktop", wintypes.LPWSTR),
        ("lpTitle", wintypes.LPWSTR),
        ("dwX", wintypes.DWORD),
        ("dwY", wintypes.DWORD),
        ("dwXSize", wintypes.DWORD),
        ("dwYSize", wintypes.DWORD),
        ("dwXCountChars", wintypes.DWORD),
        ("dwYCountChars", wintypes.DWORD),
        ("dwFillAttribute", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("wShowWindow", wintypes.WORD),
        ("cbReserved2", wintypes.WORD),
        ("lpReserved2", ctypes.POINTER(ctypes.c_byte)),
        ("hStdInput", wintypes.HANDLE),
        ("hStdOutput", wintypes.HANDLE),
        ("hStdError", wintypes.HANDLE),
    ]


class _ProcessInformation(ctypes.Structure):
    _fields_ = [
        ("hProcess", wintypes.HANDLE),
        ("hThread", wintypes.HANDLE),
        ("dwProcessId", wintypes.DWORD),
        ("dwThreadId", wintypes.DWORD),
    ]


class _Backend(Protocol):
    def create_job(self) -> int: ...
    def spawn_suspended(
        self, command: list[str], cwd: str | None, env: dict[str, str] | None
    ) -> tuple[int, int, int]: ...
    def assign(self, job: int, process: int) -> None: ...
    def resume(self, thread: int) -> None: ...
    def terminate_job(self, job: int) -> None: ...
    def terminate_process(self, process: int) -> None: ...
    def active_processes(self, job: int) -> int: ...
    def wait(self, handle: int, timeout_ms: int) -> bool: ...
    def exit_code(self, process: int) -> int: ...
    def close(self, handle: int) -> None: ...


class _Win32Backend:
    def __init__(self) -> None:
        if sys.platform != "win32":
            raise JobError("Windows Job Objects require a native Windows host")
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        signatures = {
            "CreateJobObjectW": ([ctypes.c_void_p, wintypes.LPCWSTR], wintypes.HANDLE),
            "SetInformationJobObject": (
                [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD],
                wintypes.BOOL,
            ),
            "CreateProcessW": (
                [
                    wintypes.LPCWSTR,
                    wintypes.LPWSTR,
                    ctypes.c_void_p,
                    ctypes.c_void_p,
                    wintypes.BOOL,
                    wintypes.DWORD,
                    ctypes.c_void_p,
                    wintypes.LPCWSTR,
                    ctypes.POINTER(_StartupInfo),
                    ctypes.POINTER(_ProcessInformation),
                ],
                wintypes.BOOL,
            ),
            "AssignProcessToJobObject": ([wintypes.HANDLE, wintypes.HANDLE], wintypes.BOOL),
            "ResumeThread": ([wintypes.HANDLE], wintypes.DWORD),
            "TerminateJobObject": ([wintypes.HANDLE, wintypes.UINT], wintypes.BOOL),
            "TerminateProcess": ([wintypes.HANDLE, wintypes.UINT], wintypes.BOOL),
            "QueryInformationJobObject": (
                [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p],
                wintypes.BOOL,
            ),
            "WaitForSingleObject": ([wintypes.HANDLE, wintypes.DWORD], wintypes.DWORD),
            "GetExitCodeProcess": (
                [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)],
                wintypes.BOOL,
            ),
            "CloseHandle": ([wintypes.HANDLE], wintypes.BOOL),
        }
        for name, (args, result) in signatures.items():
            function = getattr(kernel, name)
            function.argtypes = args
            function.restype = result
        self.kernel = kernel

    @staticmethod
    def _check(value: object, action: str) -> None:
        if not value:
            raise JobError(f"{action}: {ctypes.WinError(ctypes.get_last_error())}")

    def create_job(self) -> int:
        job = self.kernel.CreateJobObjectW(None, None)
        self._check(job, "CreateJobObjectW")
        limits = _ExtendedLimitInformation()
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        try:
            self._check(
                self.kernel.SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    ctypes.byref(limits),
                    ctypes.sizeof(limits),
                ),
                "SetInformationJobObject",
            )
        except Exception:
            self.close(job)
            raise
        return job

    def spawn_suspended(
        self, command: list[str], cwd: str | None, env: dict[str, str] | None
    ) -> tuple[int, int, int]:
        startup = _StartupInfo()
        startup.cb = ctypes.sizeof(startup)
        info = _ProcessInformation()
        command_line = ctypes.create_unicode_buffer(subprocess.list2cmdline(command))
        flags = CREATE_SUSPENDED
        environment = None
        if env is not None:
            for key, value in env.items():
                if not key or "=" in key or "\0" in key or "\0" in value:
                    raise JobError("invalid Windows environment entry")
            entries = [
                f"{key}={value}"
                for key, value in sorted(env.items(), key=lambda item: item[0].upper())
            ]
            environment = ctypes.create_unicode_buffer("\0".join(entries) + "\0\0")
            flags |= CREATE_UNICODE_ENVIRONMENT
        self._check(
            self.kernel.CreateProcessW(
                command[0],
                command_line,
                None,
                None,
                False,
                flags,
                ctypes.cast(environment, ctypes.c_void_p) if environment is not None else None,
                cwd,
                ctypes.byref(startup),
                ctypes.byref(info),
            ),
            "CreateProcessW",
        )
        return info.hProcess, info.hThread, info.dwProcessId

    def assign(self, job: int, process: int) -> None:
        self._check(self.kernel.AssignProcessToJobObject(job, process), "AssignProcessToJobObject")

    def resume(self, thread: int) -> None:
        previous_count = self.kernel.ResumeThread(thread)
        if previous_count == 0xFFFFFFFF:
            raise JobError(f"ResumeThread: {ctypes.WinError(ctypes.get_last_error())}")
        if previous_count != 1:
            raise JobError(f"ResumeThread returned unexpected suspend count: {previous_count}")

    def terminate_job(self, job: int) -> None:
        self._check(self.kernel.TerminateJobObject(job, 1), "TerminateJobObject")

    def terminate_process(self, process: int) -> None:
        self._check(self.kernel.TerminateProcess(process, 1), "TerminateProcess")

    def active_processes(self, job: int) -> int:
        accounting = _BasicAccountingInformation()
        self._check(
            self.kernel.QueryInformationJobObject(
                job,
                JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                ctypes.byref(accounting),
                ctypes.sizeof(accounting),
                None,
            ),
            "QueryInformationJobObject",
        )
        return accounting.ActiveProcesses

    def wait(self, handle: int, timeout_ms: int) -> bool:
        result = self.kernel.WaitForSingleObject(handle, timeout_ms)
        if result == WAIT_OBJECT_0:
            return True
        if result == WAIT_TIMEOUT:
            return False
        raise JobError(f"WaitForSingleObject: {ctypes.WinError(ctypes.get_last_error())}")

    def exit_code(self, process: int) -> int:
        code = wintypes.DWORD()
        self._check(
            self.kernel.GetExitCodeProcess(process, ctypes.byref(code)), "GetExitCodeProcess"
        )
        return code.value

    def close(self, handle: int) -> None:
        self._check(self.kernel.CloseHandle(handle), "CloseHandle")


def _close_handles(api: _Backend, handles: list[int]) -> list[str]:
    errors = []
    for handle in handles:
        try:
            api.close(handle)
        except Exception as exc:
            errors.append(f"close: {exc}")
    return errors


class OwnedWindowsProcess:
    """An assigned process tree. Close terminates every surviving Job member."""

    def __init__(self, api: _Backend, job: int, process: int, thread: int, pid: int):
        self._api = api
        self._job = job
        self._process = process
        self._thread = thread
        self.pid = pid
        self._closed = False

    def wait(self, timeout_ms: int = INFINITE) -> int | None:
        if self._closed or timeout_ms < 0 or timeout_ms > INFINITE:
            raise JobError("invalid wait on Windows Job process")
        if not self._api.wait(self._process, timeout_ms):
            return None
        return self._api.exit_code(self._process)

    def close(self, timeout_ms: int = 5000) -> None:
        if self._closed:
            return
        if timeout_ms < 0 or timeout_ms > INFINITE:
            raise JobError("invalid Job cleanup timeout")
        self._closed = True
        errors = []
        try:
            self._api.terminate_job(self._job)
            deadline = time.monotonic() + timeout_ms / 1000.0
            while self._api.active_processes(self._job) != 0:
                if time.monotonic() >= deadline:
                    errors.append("Job members survived cleanup deadline")
                    break
                time.sleep(min(0.01, max(0.0, deadline - time.monotonic())))
        except Exception as exc:
            errors.append(f"Job termination: {exc}")
        errors.extend(_close_handles(self._api, [self._thread, self._process, self._job]))
        if errors:
            raise JobError("; ".join(errors))

    def __enter__(self) -> OwnedWindowsProcess:
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.close()


def launch(
    command: list[str],
    *,
    cwd: str | None = None,
    env: dict[str, str] | None = None,
    _backend: _Backend | None = None,
) -> OwnedWindowsProcess:
    """Start a child suspended, assign it to a kill-on-close Job, then resume.

    Stdout/stderr are inherited. The caller owns evidence capture and must
    close the returned guard even when the root process has exited.
    """
    if (
        not command
        or not isinstance(command[0], str)
        or not command[0]
        or any(not isinstance(arg, str) or "\0" in arg for arg in command)
    ):
        raise JobError("command must contain an executable and NUL-free strings")
    api = _backend if _backend is not None else _Win32Backend()
    job = api.create_job()
    process = thread = None
    assigned = False
    try:
        process, thread, pid = api.spawn_suspended(command, cwd, env)
        api.assign(job, process)
        assigned = True
        api.resume(thread)
        return OwnedWindowsProcess(api, job, process, thread, pid)
    except Exception as exc:
        errors = []
        if process is not None:
            target = ("terminate_job", job) if assigned else ("terminate_process", process)
            try:
                getattr(api, target[0])(target[1])
            except Exception as cleanup_exc:
                errors.append(f"child termination: {cleanup_exc}")
            try:
                if not api.wait(process, 5000):
                    errors.append("suspended child survived cleanup deadline")
            except Exception as cleanup_exc:
                errors.append(f"child wait: {cleanup_exc}")
        errors.extend(_close_handles(api, [h for h in (thread, process, job) if h is not None]))
        detail = f"Windows Job launch failed: {exc}"
        if errors:
            detail += "; cleanup failed: " + "; ".join(errors)
        raise JobError(detail) from exc
