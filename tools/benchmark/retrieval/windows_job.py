"""Windows Job ownership, capture, and accounting for benchmark sidecars.

The child cannot run before Job assignment succeeds. Job peak memory is
committed bytes. The tree working-set peak is a sampled sum of process working
sets, analogous to summed RSS; shared pages count once per process and peaks
between samples are not observable. Keep these metrics distinct in evidence.
"""

from __future__ import annotations

import ctypes
import os
import subprocess
import sys
import time
from ctypes import wintypes
from dataclasses import dataclass
from typing import Protocol

CREATE_SUSPENDED = 0x00000004
CREATE_UNICODE_ENVIRONMENT = 0x00000400
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JOB_OBJECT_EXTENDED_LIMIT_INFORMATION = 9
JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION = 1
JOB_OBJECT_BASIC_PROCESS_ID_LIST = 3
MAX_JOB_PROCESSES = 4096
PROCESS_QUERY_LIMITED_INFORMATION = 0x00001000
SYNCHRONIZE = 0x00100000
EXTENDED_STARTUPINFO_PRESENT = 0x00080000
STARTF_USESTDHANDLES = 0x00000100
PROC_THREAD_ATTRIBUTE_HANDLE_LIST = 0x00020002
DUPLICATE_SAME_ACCESS = 0x00000002
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
CREATE_NEW = 1
OPEN_EXISTING = 3
FILE_ATTRIBUTE_NORMAL = 0x00000080
ERROR_INSUFFICIENT_BUFFER = 122
WAIT_OBJECT_0 = 0
WAIT_TIMEOUT = 0x00000102
INFINITE = 0xFFFFFFFF


class JobError(RuntimeError):
    """Ownership or cleanup could not be established."""


@dataclass(frozen=True)
class ProcessWorkingSet:
    pid: int
    creation_time_100ns: int
    working_set_bytes: int


@dataclass(frozen=True)
class ProcessPeak:
    pid: int
    creation_time_100ns: int
    peak_working_set_bytes: int
    samples: int


@dataclass(frozen=True)
class JobSample:
    observed_monotonic_ns: int
    peak_job_commit_bytes: int
    total_user_cpu_ns: int
    total_kernel_cpu_ns: int
    total_processes: int
    active_processes: int
    tree_working_set_bytes: int = 0
    processes: tuple[ProcessWorkingSet, ...] = ()

    @property
    def total_cpu_ns(self) -> int:
        return self.total_user_cpu_ns + self.total_kernel_cpu_ns


@dataclass(frozen=True)
class JobRunResult:
    """Successful snapshot series; sampling_complete does not mean continuous RSS coverage."""

    root_pid: int
    root_exit_code: int | None
    timed_out: bool
    elapsed_ms: float
    sample_interval_ms: int
    samples: int
    peak_job_commit_bytes: int
    peak_tree_working_set_bytes: int
    working_set_samples: int
    process_peaks: tuple[ProcessPeak, ...]
    total_user_cpu_ns: int
    total_kernel_cpu_ns: int
    sampling_complete: bool
    cleanup_complete: bool
    stdout_path: str | None
    stderr_path: str | None

    @property
    def total_cpu_ns(self) -> int:
        return self.total_user_cpu_ns + self.total_kernel_cpu_ns


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


class _ProcessIdList(ctypes.Structure):
    _fields_ = [
        ("NumberOfAssignedProcesses", wintypes.DWORD),
        ("NumberOfProcessIdsInList", wintypes.DWORD),
        ("ProcessIdList", ctypes.c_size_t * 1),
    ]


class _FileTime(ctypes.Structure):
    _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]

    @property
    def ticks(self) -> int:
        return (self.dwHighDateTime << 32) | self.dwLowDateTime


class _ProcessMemoryCountersEx(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
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


class _SecurityAttributes(ctypes.Structure):
    _fields_ = [
        ("nLength", wintypes.DWORD),
        ("lpSecurityDescriptor", ctypes.c_void_p),
        ("bInheritHandle", wintypes.BOOL),
    ]


class _StartupInfoEx(ctypes.Structure):
    _fields_ = [
        ("StartupInfo", _StartupInfo),
        ("lpAttributeList", ctypes.c_void_p),
    ]


class _Backend(Protocol):
    def create_job(self) -> int: ...
    def open_capture(self, stdout_path: str, stderr_path: str) -> tuple[int, int, int]: ...
    def spawn_suspended(
        self,
        command: list[str],
        cwd: str | None,
        env: dict[str, str] | None,
        capture: tuple[int, int, int] | None,
    ) -> tuple[int, int, int]: ...
    def assign(self, job: int, process: int) -> None: ...
    def resume(self, thread: int) -> None: ...
    def terminate_job(self, job: int) -> None: ...
    def terminate_process(self, process: int) -> None: ...
    def active_processes(self, job: int) -> int: ...
    def sample(self, job: int) -> JobSample: ...
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
                    ctypes.c_void_p,
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
            "OpenProcess": ([wintypes.DWORD, wintypes.BOOL, wintypes.DWORD], wintypes.HANDLE),
            "GetCurrentProcess": ([], wintypes.HANDLE),
            "DuplicateHandle": (
                [
                    wintypes.HANDLE,
                    wintypes.HANDLE,
                    wintypes.HANDLE,
                    ctypes.POINTER(wintypes.HANDLE),
                    wintypes.DWORD,
                    wintypes.BOOL,
                    wintypes.DWORD,
                ],
                wintypes.BOOL,
            ),
            "GetProcessId": ([wintypes.HANDLE], wintypes.DWORD),
            "IsProcessInJob": (
                [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)],
                wintypes.BOOL,
            ),
            "GetProcessTimes": (
                [
                    wintypes.HANDLE,
                    ctypes.POINTER(_FileTime),
                    ctypes.POINTER(_FileTime),
                    ctypes.POINTER(_FileTime),
                    ctypes.POINTER(_FileTime),
                ],
                wintypes.BOOL,
            ),
            "K32GetProcessMemoryInfo": (
                [wintypes.HANDLE, ctypes.POINTER(_ProcessMemoryCountersEx), wintypes.DWORD],
                wintypes.BOOL,
            ),
            "CloseHandle": ([wintypes.HANDLE], wintypes.BOOL),
            "CreateFileW": (
                [
                    wintypes.LPCWSTR,
                    wintypes.DWORD,
                    wintypes.DWORD,
                    ctypes.POINTER(_SecurityAttributes),
                    wintypes.DWORD,
                    wintypes.DWORD,
                    wintypes.HANDLE,
                ],
                wintypes.HANDLE,
            ),
            "InitializeProcThreadAttributeList": (
                [ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, ctypes.POINTER(ctypes.c_size_t)],
                wintypes.BOOL,
            ),
            "UpdateProcThreadAttribute": (
                [
                    ctypes.c_void_p,
                    wintypes.DWORD,
                    ctypes.c_size_t,
                    ctypes.c_void_p,
                    ctypes.c_size_t,
                    ctypes.c_void_p,
                    ctypes.c_void_p,
                ],
                wintypes.BOOL,
            ),
            "DeleteProcThreadAttributeList": ([ctypes.c_void_p], None),
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

    def open_capture(self, stdout_path: str, stderr_path: str) -> tuple[int, int, int]:
        attributes = _SecurityAttributes()
        attributes.nLength = ctypes.sizeof(attributes)
        attributes.bInheritHandle = False
        handles = []
        try:
            for path, access, share, creation in (
                ("NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING),
                (stdout_path, GENERIC_WRITE, FILE_SHARE_READ, CREATE_NEW),
                (stderr_path, GENERIC_WRITE, FILE_SHARE_READ, CREATE_NEW),
            ):
                handle = self.kernel.CreateFileW(
                    path,
                    access,
                    share,
                    ctypes.byref(attributes),
                    creation,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
                if handle == ctypes.c_void_p(-1).value or handle is None:
                    raise JobError(
                        f"CreateFileW {path}: {ctypes.WinError(ctypes.get_last_error())}"
                    )
                handles.append(handle)
        except Exception as exc:
            errors = _close_handles(self, handles)
            if errors:
                raise JobError(f"capture setup failed: {exc}; cleanup failed: {errors}") from exc
            raise
        return tuple(handles)

    def _capture_startup(self, capture: tuple[int, int, int]):
        size = ctypes.c_size_t()
        self.kernel.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(size))
        if ctypes.get_last_error() != ERROR_INSUFFICIENT_BUFFER or size.value == 0:
            raise JobError("cannot size Windows handle inheritance list")
        buffer = ctypes.create_string_buffer(size.value)
        self._check(
            self.kernel.InitializeProcThreadAttributeList(buffer, 1, 0, ctypes.byref(size)),
            "InitializeProcThreadAttributeList",
        )
        try:
            inherited = (wintypes.HANDLE * len(capture))(*capture)
            self._check(
                self.kernel.UpdateProcThreadAttribute(
                    buffer,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                    ctypes.cast(inherited, ctypes.c_void_p),
                    ctypes.sizeof(inherited),
                    None,
                    None,
                ),
                "UpdateProcThreadAttribute",
            )
            startup = _StartupInfoEx()
            startup.StartupInfo.cb = ctypes.sizeof(startup)
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES
            startup.StartupInfo.hStdInput = capture[0]
            startup.StartupInfo.hStdOutput = capture[1]
            startup.StartupInfo.hStdError = capture[2]
            startup.lpAttributeList = ctypes.cast(buffer, ctypes.c_void_p)
            return startup, buffer, inherited
        except Exception:
            self.kernel.DeleteProcThreadAttributeList(buffer)
            raise

    def spawn_suspended(
        self,
        command: list[str],
        cwd: str | None,
        env: dict[str, str] | None,
        capture: tuple[int, int, int] | None,
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
        attribute_list = None
        inheritable = []
        created = False
        failure = None
        try:
            if capture is not None:
                parent = self.kernel.GetCurrentProcess()
                for handle in capture:
                    duplicate = wintypes.HANDLE()
                    self._check(
                        self.kernel.DuplicateHandle(
                            parent,
                            handle,
                            parent,
                            ctypes.byref(duplicate),
                            0,
                            True,
                            DUPLICATE_SAME_ACCESS,
                        ),
                        "DuplicateHandle capture",
                    )
                    inheritable.append(duplicate.value)
                startup, attribute_list, _inherited = self._capture_startup(tuple(inheritable))
                flags |= EXTENDED_STARTUPINFO_PRESENT
            self._check(
                self.kernel.CreateProcessW(
                    command[0],
                    command_line,
                    None,
                    None,
                    capture is not None,
                    flags,
                    ctypes.cast(environment, ctypes.c_void_p) if environment is not None else None,
                    cwd,
                    ctypes.byref(startup),
                    ctypes.byref(info),
                ),
                "CreateProcessW",
            )
            created = True
        except Exception as exc:
            failure = exc
        finally:
            if attribute_list is not None:
                self.kernel.DeleteProcThreadAttributeList(attribute_list)
            close_errors = _close_handles(self, inheritable)
            if close_errors:
                if created:
                    child_errors = []
                    try:
                        self.terminate_process(info.hProcess)
                        if not self.wait(info.hProcess, 5000):
                            child_errors.append("suspended child survived")
                    except Exception as exc:
                        child_errors.append(str(exc))
                    child_errors.extend(_close_handles(self, [info.hThread, info.hProcess]))
                    created = False
                else:
                    child_errors = []
                failure = JobError(
                    f"capture handle cleanup failed: {close_errors}; child cleanup: {child_errors}"
                )
        if failure is not None:
            raise failure
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
        return self._basic_accounting(job).ActiveProcesses

    def _basic_accounting(self, job: int) -> _BasicAccountingInformation:
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
        return accounting

    def _process_ids(self, job: int, expected_active: int) -> tuple[int, ...]:
        if expected_active < 0 or expected_active > MAX_JOB_PROCESSES:
            raise JobError("Windows Job process count exceeds sampler bound")
        capacity = max(expected_active, 1)
        offset = _ProcessIdList.ProcessIdList.offset
        size = offset + capacity * ctypes.sizeof(ctypes.c_size_t)
        buffer = ctypes.create_string_buffer(size)
        self._check(
            self.kernel.QueryInformationJobObject(
                job,
                JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                buffer,
                size,
                None,
            ),
            "QueryInformationJobObject process IDs",
        )
        header = ctypes.cast(buffer, ctypes.POINTER(_ProcessIdList)).contents
        assigned = header.NumberOfAssignedProcesses
        returned = header.NumberOfProcessIdsInList
        if assigned != expected_active or returned != assigned or returned > capacity:
            raise JobError("Windows Job PID list is incomplete or changed during sampling")
        ids = tuple(sorted((ctypes.c_size_t * returned).from_buffer(buffer, offset)))
        if len(set(ids)) != len(ids) or any(pid <= 0 or pid > 0xFFFFFFFF for pid in ids):
            raise JobError("Windows Job PID list contains invalid or duplicate identities")
        return ids

    def _working_set(self, job: int, process: int, pid: int) -> ProcessWorkingSet:
        if self.wait(process, 0):
            raise JobError(f"Windows Job process {pid} exited during sampling")
        observed_pid = self.kernel.GetProcessId(process)
        if observed_pid != pid:
            raise JobError(f"Windows Job PID identity changed during sampling: {pid}")
        in_job = wintypes.BOOL()
        self._check(
            self.kernel.IsProcessInJob(process, job, ctypes.byref(in_job)), "IsProcessInJob"
        )
        if not in_job.value:
            raise JobError(f"Windows Job PID {pid} no longer belongs to this Job")
        created = _FileTime()
        exited = _FileTime()
        kernel = _FileTime()
        user = _FileTime()
        self._check(
            self.kernel.GetProcessTimes(
                process,
                ctypes.byref(created),
                ctypes.byref(exited),
                ctypes.byref(kernel),
                ctypes.byref(user),
            ),
            "GetProcessTimes",
        )
        memory = _ProcessMemoryCountersEx()
        memory.cb = ctypes.sizeof(memory)
        self._check(
            self.kernel.K32GetProcessMemoryInfo(process, ctypes.byref(memory), memory.cb),
            "K32GetProcessMemoryInfo",
        )
        if self.wait(process, 0):
            raise JobError(f"Windows Job process {pid} exited during sampling")
        if created.ticks == 0:
            raise JobError(f"Windows Job process {pid} has no creation identity")
        return ProcessWorkingSet(pid, created.ticks, memory.WorkingSetSize)

    def sample(self, job: int) -> JobSample:
        before = self._basic_accounting(job)
        pids = self._process_ids(job, before.ActiveProcesses)
        handles = []
        processes = []
        failure = None
        after = None
        try:
            for pid in pids:
                process = self.kernel.OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, False, pid
                )
                self._check(process, f"OpenProcess {pid}")
                handles.append(process)
                processes.append(self._working_set(job, process, pid))
            if self._process_ids(job, len(pids)) != pids:
                raise JobError("Windows Job PID list changed during sampling")
            after = self._basic_accounting(job)
            if after.TotalProcesses != before.TotalProcesses or after.ActiveProcesses != len(pids):
                raise JobError("Windows Job process population changed during sampling")
            for process in handles:
                if self.wait(process, 0):
                    raise JobError("Windows Job process exited before sample completion")
        except Exception as exc:
            failure = exc
        close_errors = _close_handles(self, handles)
        if failure is not None:
            if close_errors:
                raise JobError(
                    f"{failure}; process handle cleanup failed: {close_errors}"
                ) from failure
            raise failure
        if close_errors:
            raise JobError(f"process handle cleanup failed: {close_errors}")
        limits = _ExtendedLimitInformation()
        self._check(
            self.kernel.QueryInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                ctypes.byref(limits),
                ctypes.sizeof(limits),
                None,
            ),
            "QueryInformationJobObject extended limits",
        )
        accounting = self._basic_accounting(job)
        if (
            accounting.TotalProcesses != after.TotalProcesses
            or accounting.ActiveProcesses != after.ActiveProcesses
        ):
            raise JobError("Windows Job changed after working-set sampling")
        return JobSample(
            observed_monotonic_ns=time.monotonic_ns(),
            peak_job_commit_bytes=limits.PeakJobMemoryUsed,
            total_user_cpu_ns=accounting.TotalUserTime * 100,
            total_kernel_cpu_ns=accounting.TotalKernelTime * 100,
            total_processes=accounting.TotalProcesses,
            active_processes=accounting.ActiveProcesses,
            tree_working_set_bytes=sum(row.working_set_bytes for row in processes),
            processes=tuple(processes),
        )

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
    """An assigned process tree with independent Job accounting."""

    def __init__(
        self,
        api: _Backend,
        job: int,
        process: int,
        thread: int,
        pid: int,
        stdout_path: str | None = None,
        stderr_path: str | None = None,
    ):
        self._api = api
        self._job = job
        self._process = process
        self._thread = thread
        self.stdout_path = stdout_path
        self.stderr_path = stderr_path
        self.pid = pid
        self._closed = False
        self.cleanup_complete = False
        self.sampling_complete = False
        self._final_sample: JobSample | None = None

    def _sample_unchecked(self) -> JobSample:
        sample = self._api.sample(self._job)
        if not isinstance(sample, JobSample) or any(
            type(value) is not int or value < 0
            for value in (
                sample.observed_monotonic_ns,
                sample.peak_job_commit_bytes,
                sample.total_user_cpu_ns,
                sample.total_kernel_cpu_ns,
                sample.total_processes,
                sample.active_processes,
                sample.tree_working_set_bytes,
            )
        ):
            raise JobError("missing or malformed Windows Job resource sample")
        if sample.total_processes == 0 or sample.active_processes > sample.total_processes:
            raise JobError("inconsistent Windows Job process accounting")
        if type(sample.processes) is not tuple or len(sample.processes) != sample.active_processes:
            raise JobError("Windows Job working-set process list is incomplete")
        pids = set()
        total_working_set = 0
        for row in sample.processes:
            if (
                not isinstance(row, ProcessWorkingSet)
                or type(row.pid) is not int
                or row.pid <= 0
                or type(row.creation_time_100ns) is not int
                or row.creation_time_100ns <= 0
                or type(row.working_set_bytes) is not int
                or row.working_set_bytes < 0
                or row.pid in pids
            ):
                raise JobError("Windows Job working-set identity is missing or duplicated")
            pids.add(row.pid)
            total_working_set += row.working_set_bytes
        if total_working_set != sample.tree_working_set_bytes:
            raise JobError("Windows Job working-set sum differs from process rows")
        return sample

    def sample(self) -> JobSample:
        """Read Job commit peak and cumulative CPU, independent of Unix ps."""
        if self._closed:
            raise JobError("cannot sample a closed Windows Job")
        return self._sample_unchecked()

    def wait(self, timeout_ms: int = INFINITE) -> int | None:
        if self._closed or timeout_ms < 0 or timeout_ms > INFINITE:
            raise JobError("invalid wait on Windows Job process")
        if not self._api.wait(self._process, timeout_ms):
            return None
        return self._api.exit_code(self._process)

    def close(self, timeout_ms: int = 5000) -> JobSample:
        if self._closed:
            if self.cleanup_complete and self._final_sample is not None:
                return self._final_sample
            raise JobError("Windows Job cleanup previously failed")
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
            if not errors:
                self._final_sample = self._sample_unchecked()
                if self._final_sample.active_processes != 0:
                    raise JobError("Windows Job gained an active process during cleanup")
        except Exception as exc:
            errors.append(f"Job termination: {exc}")
        errors.extend(_close_handles(self._api, [self._thread, self._process, self._job]))
        if errors:
            raise JobError("; ".join(errors))
        self.cleanup_complete = True
        return self._final_sample

    def monitor(self, *, timeout_secs: float, sample_interval_ms: int = 50) -> JobRunResult:
        """Sample through root exit or timeout, then terminate and verify the Job.

        A result exists only when every sample and cleanup check succeeds and
        a positive tree working-set sample was observed. A timed-out result is
        explicit and must be rejected by callers for scoring.
        """
        if self._closed:
            raise JobError("cannot monitor a closed Windows Job")
        if (
            type(timeout_secs) not in (int, float)
            or not 0 < timeout_secs <= sys.float_info.max
            or type(sample_interval_ms) is not int
            or sample_interval_ms <= 0
        ):
            try:
                self.close()
            except Exception as exc:
                raise JobError(f"invalid monitor parameters; cleanup failed: {exc}") from exc
            raise JobError("monitor timeout and sample interval must be positive")
        started = time.monotonic()
        observations: list[JobSample] = []
        peak_tree_working_set = 0
        working_set_samples = 0
        process_peaks: dict[tuple[int, int], tuple[int, int]] = {}
        exit_code = None
        timed_out = False
        sampling_error = None
        try:
            while True:
                current = self.sample()
                if observations and any(
                    getattr(current, field) < getattr(observations[-1], field)
                    for field in (
                        "observed_monotonic_ns",
                        "peak_job_commit_bytes",
                        "total_user_cpu_ns",
                        "total_kernel_cpu_ns",
                        "total_processes",
                    )
                ):
                    raise JobError("Windows Job accounting regressed between samples")
                observations.append(current)
                peak_tree_working_set = max(peak_tree_working_set, current.tree_working_set_bytes)
                if current.active_processes:
                    working_set_samples += 1
                for row in current.processes:
                    key = (row.pid, row.creation_time_100ns)
                    prior_peak, prior_count = process_peaks.get(key, (0, 0))
                    process_peaks[key] = (max(prior_peak, row.working_set_bytes), prior_count + 1)
                exit_code = self.wait(0)
                if exit_code is not None:
                    break
                remaining = timeout_secs - (time.monotonic() - started)
                if remaining <= 0:
                    timed_out = True
                    break
                time.sleep(min(sample_interval_ms / 1000.0, remaining))
        except Exception as exc:
            sampling_error = exc
        try:
            final = self.close()
        except Exception as exc:
            if sampling_error is not None:
                raise JobError(
                    f"sampling failed: {sampling_error}; cleanup failed: {exc}"
                ) from sampling_error
            raise
        if sampling_error is not None:
            raise JobError(f"Windows Job sampling failed: {sampling_error}") from sampling_error
        if observations and any(
            getattr(final, field) < getattr(observations[-1], field)
            for field in (
                "observed_monotonic_ns",
                "peak_job_commit_bytes",
                "total_user_cpu_ns",
                "total_kernel_cpu_ns",
                "total_processes",
            )
        ):
            raise JobError("final Windows Job accounting regressed")
        observations.append(final)
        if (
            len(observations) < 2
            or final.peak_job_commit_bytes <= 0
            or peak_tree_working_set <= 0
            or working_set_samples == 0
        ):
            raise JobError("Windows Job resource evidence has no positive working-set sample")
        self.sampling_complete = True
        return JobRunResult(
            root_pid=self.pid,
            root_exit_code=exit_code,
            timed_out=timed_out,
            elapsed_ms=(time.monotonic() - started) * 1000.0,
            sample_interval_ms=sample_interval_ms,
            samples=len(observations),
            peak_job_commit_bytes=final.peak_job_commit_bytes,
            peak_tree_working_set_bytes=peak_tree_working_set,
            working_set_samples=working_set_samples,
            process_peaks=tuple(
                ProcessPeak(pid, created, peak, count)
                for (pid, created), (peak, count) in sorted(process_peaks.items())
            ),
            total_user_cpu_ns=final.total_user_cpu_ns,
            total_kernel_cpu_ns=final.total_kernel_cpu_ns,
            sampling_complete=True,
            cleanup_complete=True,
            stdout_path=self.stdout_path,
            stderr_path=self.stderr_path,
        )

    def __enter__(self) -> OwnedWindowsProcess:
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.close()


def launch(
    command: list[str],
    *,
    cwd: str | None = None,
    env: dict[str, str] | None = None,
    stdout_path: str | os.PathLike[str] | None = None,
    stderr_path: str | os.PathLike[str] | None = None,
    _backend: _Backend | None = None,
) -> OwnedWindowsProcess:
    """Start a child suspended, assign it to a kill-on-close Job, then resume.

    Supply both output paths to create exclusive stdout/stderr captures with
    an explicit Windows inherited-handle list. Without paths, no output capture
    is provided. Use monitor() for resource evidence. Temporary inheritable
    duplicates exist only around CreateProcess, but a dedicated launcher is
    still needed if other threads may concurrently create inheriting children.
    """
    if (
        not command
        or not isinstance(command[0], str)
        or not command[0]
        or any(not isinstance(arg, str) or "\0" in arg for arg in command)
    ):
        raise JobError("command must contain an executable and NUL-free strings")
    if (stdout_path is None) != (stderr_path is None):
        raise JobError("stdout and stderr capture paths must be supplied together")
    if stdout_path is not None:
        stdout_path = os.path.abspath(os.fspath(stdout_path))
        stderr_path = os.path.abspath(os.fspath(stderr_path))
        if "\0" in stdout_path or "\0" in stderr_path:
            raise JobError("capture paths must be NUL-free")
        if os.path.normcase(stdout_path) == os.path.normcase(stderr_path):
            raise JobError("stdout and stderr capture paths must differ")
    api = _backend if _backend is not None else _Win32Backend()
    job = api.create_job()
    process = thread = None
    capture = None
    assigned = False
    try:
        if stdout_path is not None:
            capture = api.open_capture(stdout_path, stderr_path)
        process, thread, pid = api.spawn_suspended(command, cwd, env, capture)
        if capture is not None:
            failed_handles = []
            capture_errors = []
            for handle in capture:
                try:
                    api.close(handle)
                except Exception as close_exc:
                    failed_handles.append(handle)
                    capture_errors.append(str(close_exc))
            capture = tuple(failed_handles) if failed_handles else None
            if capture_errors:
                raise JobError("cannot close parent capture handles: " + "; ".join(capture_errors))
        api.assign(job, process)
        assigned = True
        api.resume(thread)
        return OwnedWindowsProcess(api, job, process, thread, pid, stdout_path, stderr_path)
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
        handles = [h for h in (thread, process, job) if h is not None]
        if capture is not None:
            handles.extend(capture)
        errors.extend(_close_handles(api, handles))
        detail = f"Windows Job launch failed: {exc}"
        if errors:
            detail += "; cleanup failed: " + "; ".join(errors)
        raise JobError(detail) from exc
