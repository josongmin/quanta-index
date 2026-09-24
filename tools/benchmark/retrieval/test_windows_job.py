"""Contract tests for Windows ownership ordering, runnable on non-Windows hosts."""

from __future__ import annotations

import ctypes
import sys
import time
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval import windows_job


class FakeBackend:
    def __init__(
        self,
        *,
        fail: str | None = None,
        active: list[int] | None = None,
        samples: list[windows_job.JobSample] | None = None,
        root_wait: bool | list[bool] = True,
    ):
        self.fail = fail
        self.active = iter(active if active is not None else [0])
        self.samples = iter(samples) if samples is not None else None
        self.root_wait = iter(root_wait) if isinstance(root_wait, list) else root_wait
        self.calls = []

    def _record(self, name, *args):
        self.calls.append((name, *args))
        if self.fail == name:
            raise windows_job.JobError(name)

    def create_job(self):
        self._record("create_job")
        return 10

    def open_capture(self, stdout_path, stderr_path):
        self._record("open_capture", stdout_path, stderr_path)
        return 50, 51, 52

    def spawn_suspended(self, command, cwd, env, capture):
        self._record("spawn_suspended", command, cwd, env, capture)
        return 20, 30, 40

    def assign(self, job, process):
        self._record("assign", job, process)

    def resume(self, thread):
        self._record("resume", thread)

    def terminate_job(self, job):
        self._record("terminate_job", job)

    def terminate_process(self, process):
        self._record("terminate_process", process)

    def active_processes(self, job):
        self._record("active_processes", job)
        return next(self.active)

    def sample(self, job):
        self._record("sample", job)
        if self.samples is not None:
            return next(self.samples)
        terminated = any(call[0] == "terminate_job" for call in self.calls)
        rows = () if terminated else (windows_job.ProcessWorkingSet(40, 100, 2048),)
        return windows_job.JobSample(
            time.monotonic_ns(),
            4096,
            500,
            700,
            1,
            len(rows),
            sum(row.working_set_bytes for row in rows),
            rows,
        )

    def wait(self, handle, timeout_ms):
        self._record("wait", handle, timeout_ms)
        return next(self.root_wait) if not isinstance(self.root_wait, bool) else self.root_wait

    def exit_code(self, process):
        self._record("exit_code", process)
        return 7

    def close(self, handle):
        self._record("close", handle)


def test_assign_precedes_resume_and_close_owns_descendants():
    api = FakeBackend(active=[2, 1, 0])
    owned = windows_job.launch(["worker.exe", "--run"], _backend=api)
    assert owned.pid == 40
    assert owned.wait(0) == 7
    owned.close()
    owned.close()
    names = [call[0] for call in api.calls]
    assert names[:4] == ["create_job", "spawn_suspended", "assign", "resume"]
    assert names.count("active_processes") == 3
    assert names[-3:] == ["close", "close", "close"]
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]
    with pytest.raises(windows_job.JobError):
        owned.wait(0)


def test_assignment_failure_kills_unowned_suspended_child():
    api = FakeBackend(fail="assign")
    with pytest.raises(windows_job.JobError, match="assign"):
        windows_job.launch(["worker.exe"], _backend=api)
    assert ("terminate_process", 20) in api.calls
    assert ("wait", 20, 5000) in api.calls
    assert not any(call[0] == "resume" for call in api.calls)
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]


def test_resume_failure_terminates_owned_job():
    api = FakeBackend(fail="resume")
    with pytest.raises(windows_job.JobError, match="resume"):
        windows_job.launch(["worker.exe"], _backend=api)
    assert ("terminate_job", 10) in api.calls
    assert ("wait", 20, 5000) in api.calls


def test_spawn_failure_releases_job_without_resuming():
    api = FakeBackend(fail="spawn_suspended")
    with pytest.raises(windows_job.JobError, match="spawn_suspended"):
        windows_job.launch(["worker.exe"], _backend=api)
    assert api.calls[-1] == ("close", 10)
    assert not any(call[0] == "resume" for call in api.calls)


def test_capture_failure_releases_job_without_spawning(tmp_path):
    api = FakeBackend(fail="open_capture")
    with pytest.raises(windows_job.JobError, match="open_capture"):
        windows_job.launch(
            ["worker.exe"],
            stdout_path=tmp_path / "out",
            stderr_path=tmp_path / "err",
            _backend=api,
        )
    assert api.calls[-1] == ("close", 10)
    assert not any(call[0] == "spawn_suspended" for call in api.calls)


def test_parent_capture_close_failure_kills_suspended_child(tmp_path):
    api = FakeBackend(fail="close")
    with pytest.raises(windows_job.JobError, match="parent capture handles"):
        windows_job.launch(
            ["worker.exe"],
            stdout_path=tmp_path / "out",
            stderr_path=tmp_path / "err",
            _backend=api,
        )
    assert ("terminate_process", 20) in api.calls
    assert not any(call[0] == "resume" for call in api.calls)


def test_cleanup_timeout_fails_closed_and_closes_handles():
    api = FakeBackend(active=[1])
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="survived cleanup deadline"):
        owned.close(timeout_ms=0)
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]


def test_termination_failure_reports_error_and_closes_handles():
    api = FakeBackend(fail="terminate_job")
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="Job termination"):
        owned.close()
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]


def test_cleanup_rejects_new_active_job_member():
    rows = (windows_job.ProcessWorkingSet(40, 100, 1024),)
    api = FakeBackend(samples=[windows_job.JobSample(1, 4096, 1, 1, 2, 1, 1024, rows)])
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="gained an active process"):
        owned.close()
    assert not owned.cleanup_complete
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]


def test_monitor_reports_final_job_counters_and_capture_paths(tmp_path):
    first = (windows_job.ProcessWorkingSet(40, 100, 1024),)
    second = (windows_job.ProcessWorkingSet(40, 100, 3072),)
    samples = [
        windows_job.JobSample(1, 1024, 200, 300, 2, 1, 1024, first),
        windows_job.JobSample(2, 4096, 500, 600, 2, 1, 3072, second),
        windows_job.JobSample(3, 8192, 900, 1100, 2, 0, 0, ()),
    ]
    api = FakeBackend(samples=samples, root_wait=[False, True])
    stdout = tmp_path / "stdout.log"
    stderr = tmp_path / "stderr.log"
    owned = windows_job.launch(["worker.exe"], stdout_path=stdout, stderr_path=stderr, _backend=api)
    result = owned.monitor(timeout_secs=1, sample_interval_ms=1)
    assert result.root_exit_code == 7
    assert result.samples == 3
    assert result.peak_job_commit_bytes == 8192
    assert result.peak_tree_working_set_bytes == 3072
    assert result.working_set_samples == 2
    assert result.process_peaks == (windows_job.ProcessPeak(40, 100, 3072, 2),)
    assert result.total_user_cpu_ns == 900
    assert result.total_kernel_cpu_ns == 1100
    assert result.total_cpu_ns == 2000
    assert result.sampling_complete and result.cleanup_complete
    assert result.stdout_path == str(stdout)
    assert result.stderr_path == str(stderr)
    assert owned.cleanup_complete and owned.sampling_complete
    assert api.calls[1] == ("open_capture", str(stdout), str(stderr))
    assert api.calls[2][-1] == (50, 51, 52)
    assert api.calls[3:6] == [
        ("close", 50),
        ("close", 51),
        ("close", 52),
    ]
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]


def test_monitor_fails_closed_without_positive_memory():
    empty = windows_job.JobSample(1, 0, 0, 0, 1, 0)
    api = FakeBackend(samples=[empty, empty])
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="no positive working-set"):
        owned.monitor(timeout_secs=1)
    assert owned.cleanup_complete
    assert not owned.sampling_complete


def test_monitor_fails_closed_on_missing_sample_and_still_cleans_up():
    api = FakeBackend(fail="sample")
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="sampling failed.*cleanup failed"):
        owned.monitor(timeout_secs=1)
    assert not owned.cleanup_complete
    assert api.calls[-3:] == [("close", 30), ("close", 20), ("close", 10)]


def test_monitor_rejects_missing_initial_sample_even_with_final_metrics():
    final = windows_job.JobSample(2, 4096, 100, 200, 1, 0)
    api = FakeBackend(samples=[None, final])
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="sampling failed"):
        owned.monitor(timeout_secs=1)
    assert owned.cleanup_complete
    assert not owned.sampling_complete


def test_monitor_rejects_regressed_cpu_counters():
    rows = (windows_job.ProcessWorkingSet(40, 100, 1024),)
    samples = [
        windows_job.JobSample(1, 1024, 300, 300, 1, 1, 1024, rows),
        windows_job.JobSample(2, 1024, 200, 300, 1, 0),
    ]
    api = FakeBackend(samples=samples)
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="final Windows Job accounting regressed"):
        owned.monitor(timeout_secs=1)
    assert owned.cleanup_complete
    assert not owned.sampling_complete


def test_monitor_rejects_incomplete_or_wrong_working_set_rows():
    for bad in (
        windows_job.JobSample(
            1, 4096, 1, 1, 2, 2, 1024, (windows_job.ProcessWorkingSet(40, 100, 1024),)
        ),
        windows_job.JobSample(
            1, 4096, 1, 1, 1, 1, 2048, (windows_job.ProcessWorkingSet(40, 100, 1024),)
        ),
        windows_job.JobSample(
            1,
            4096,
            1,
            1,
            2,
            2,
            2048,
            (
                windows_job.ProcessWorkingSet(40, 100, 1024),
                windows_job.ProcessWorkingSet(40, 200, 1024),
            ),
        ),
    ):
        final = windows_job.JobSample(2, 4096, 2, 2, 2, 0)
        api = FakeBackend(samples=[bad, final])
        owned = windows_job.launch(["worker.exe"], _backend=api)
        with pytest.raises(windows_job.JobError, match="sampling failed"):
            owned.monitor(timeout_secs=1)
        assert owned.cleanup_complete
        assert not owned.sampling_complete


def test_pid_reuse_between_samples_keeps_separate_process_identities():
    first = (windows_job.ProcessWorkingSet(40, 100, 1024),)
    second = (windows_job.ProcessWorkingSet(40, 200, 2048),)
    api = FakeBackend(
        samples=[
            windows_job.JobSample(1, 4096, 100, 100, 1, 1, 1024, first),
            windows_job.JobSample(2, 4096, 200, 200, 2, 1, 2048, second),
            windows_job.JobSample(3, 4096, 300, 300, 2, 0),
        ],
        root_wait=[False, True],
    )
    result = windows_job.launch(["worker.exe"], _backend=api).monitor(
        timeout_secs=1, sample_interval_ms=1
    )
    assert result.peak_tree_working_set_bytes == 2048
    assert result.process_peaks == (
        windows_job.ProcessPeak(40, 100, 1024, 1),
        windows_job.ProcessPeak(40, 200, 2048, 1),
    )


class FakeProcessListKernel:
    def __init__(self, assigned, pids):
        self.assigned = assigned
        self.pids = pids

    def QueryInformationJobObject(self, _job, _kind, buffer, _size, _unused):
        header = ctypes.cast(buffer, ctypes.POINTER(windows_job._ProcessIdList)).contents
        header.NumberOfAssignedProcesses = self.assigned
        header.NumberOfProcessIdsInList = len(self.pids)
        offset = windows_job._ProcessIdList.ProcessIdList.offset
        values = (ctypes.c_size_t * len(self.pids)).from_buffer(buffer, offset)
        values[:] = self.pids
        return True


def test_native_pid_list_parser_rejects_partial_or_duplicate_lists():
    native = object.__new__(windows_job._Win32Backend)
    native.kernel = FakeProcessListKernel(2, [40])
    with pytest.raises(windows_job.JobError, match="incomplete"):
        native._process_ids(10, 2)
    native.kernel = FakeProcessListKernel(2, [40, 40])
    with pytest.raises(windows_job.JobError, match="duplicate"):
        native._process_ids(10, 2)


def test_native_sampler_rejects_population_race_and_releases_handles():
    native = object.__new__(windows_job._Win32Backend)
    native.kernel = SimpleNamespace(OpenProcess=lambda *_args: 20)
    native._basic_accounting = lambda _job: SimpleNamespace(ActiveProcesses=1, TotalProcesses=1)
    pid_lists = iter([(40,), (41,)])
    native._process_ids = lambda _job, _expected: next(pid_lists)
    native._working_set = lambda _job, _handle, pid: windows_job.ProcessWorkingSet(pid, 100, 1024)
    native.wait = lambda _handle, _timeout: False
    closed = []
    native.close = closed.append
    with pytest.raises(windows_job.JobError, match="PID list changed"):
        native.sample(10)
    assert closed == [20]


def test_native_sampler_reads_working_set_and_cpu_without_commit_aliasing():
    native = object.__new__(windows_job._Win32Backend)

    def open_process(access, inheritable, pid):
        assert access == windows_job.PROCESS_QUERY_LIMITED_INFORMATION | windows_job.SYNCHRONIZE
        assert not inheritable and pid == 40
        return 20

    def query(_job, kind, buffer, _size, _unused):
        assert kind == windows_job.JOB_OBJECT_EXTENDED_LIMIT_INFORMATION
        limits = ctypes.cast(buffer, ctypes.POINTER(windows_job._ExtendedLimitInformation)).contents
        limits.PeakJobMemoryUsed = 8192
        return True

    native.kernel = SimpleNamespace(
        OpenProcess=open_process,
        QueryInformationJobObject=query,
    )
    accounting = SimpleNamespace(
        ActiveProcesses=1, TotalProcesses=1, TotalUserTime=7, TotalKernelTime=11
    )
    native._basic_accounting = lambda _job: accounting
    native._process_ids = lambda _job, _expected: (40,)
    native._working_set = lambda _job, _handle, pid: windows_job.ProcessWorkingSet(pid, 100, 3072)
    native.wait = lambda _handle, _timeout: False
    closed = []
    native.close = closed.append
    sample = native.sample(10)
    assert sample.peak_job_commit_bytes == 8192
    assert sample.tree_working_set_bytes == 3072
    assert sample.total_cpu_ns == 1800
    assert sample.processes == (windows_job.ProcessWorkingSet(40, 100, 3072),)
    assert closed == [20]


def test_native_working_set_probe_checks_identity_and_liveness():
    native = object.__new__(windows_job._Win32Backend)

    def in_job(_process, _job, output):
        ctypes.cast(output, ctypes.POINTER(windows_job.wintypes.BOOL))[0] = True
        return True

    def times(_process, created, _exited, _kernel, _user):
        ctypes.cast(created, ctypes.POINTER(windows_job._FileTime)).contents.dwLowDateTime = 1234
        return True

    def memory(_process, output, _size):
        ctypes.cast(
            output, ctypes.POINTER(windows_job._ProcessMemoryCountersEx)
        ).contents.WorkingSetSize = 2048
        return True

    native.kernel = SimpleNamespace(
        GetProcessId=lambda _process: 40,
        IsProcessInJob=in_job,
        GetProcessTimes=times,
        K32GetProcessMemoryInfo=memory,
    )
    native.wait = lambda _process, _timeout: False
    assert native._working_set(10, 20, 40) == windows_job.ProcessWorkingSet(40, 1234, 2048)
    native.kernel.GetProcessId = lambda _process: 41
    with pytest.raises(windows_job.JobError, match="PID identity changed"):
        native._working_set(10, 20, 40)
    native.wait = lambda _process, _timeout: True
    with pytest.raises(windows_job.JobError, match="exited during sampling"):
        native._working_set(10, 20, 40)


def test_native_capture_uses_short_lived_inheritable_duplicates():
    native = object.__new__(windows_job._Win32Backend)
    duplicated = []
    inherited = []
    closed = []
    creation_flags = []

    def duplicate(_source, handle, _target, output, _access, inheritable, _options):
        assert inheritable
        value = handle + 100
        ctypes.cast(output, ctypes.POINTER(windows_job.wintypes.HANDLE))[0] = value
        duplicated.append(value)
        return True

    def create(
        _application,
        _command,
        _process_attrs,
        _thread_attrs,
        inherit,
        flags,
        _environment,
        _cwd,
        _startup,
        info,
    ):
        assert inherit
        creation_flags.append(flags)
        target = ctypes.cast(info, ctypes.POINTER(windows_job._ProcessInformation)).contents
        target.hProcess = 20
        target.hThread = 30
        target.dwProcessId = 40
        return True

    native.kernel = SimpleNamespace(
        GetCurrentProcess=lambda: 1,
        DuplicateHandle=duplicate,
        CreateProcessW=create,
        DeleteProcThreadAttributeList=lambda _list: None,
    )
    native._capture_startup = lambda handles: (
        inherited.append(handles) or windows_job._StartupInfoEx(),
        ctypes.c_void_p(1),
        None,
    )
    native.close = closed.append
    assert native.spawn_suspended(["worker.exe"], None, None, (50, 51, 52)) == (20, 30, 40)
    assert duplicated == [150, 151, 152]
    assert inherited == [(150, 151, 152)]
    assert closed == duplicated
    assert creation_flags == [
        windows_job.CREATE_SUSPENDED | windows_job.EXTENDED_STARTUPINFO_PRESENT
    ]


def test_monitor_marks_timeout_without_scoring_it():
    api = FakeBackend(root_wait=False)
    owned = windows_job.launch(["worker.exe"], _backend=api)
    result = owned.monitor(timeout_secs=0.000001)
    assert result.timed_out
    assert result.root_exit_code is None
    assert result.cleanup_complete and result.sampling_complete


def test_invalid_monitor_parameters_still_cleanup():
    api = FakeBackend()
    owned = windows_job.launch(["worker.exe"], _backend=api)
    with pytest.raises(windows_job.JobError, match="monitor timeout"):
        owned.monitor(timeout_secs=0)
    assert owned.cleanup_complete


def test_capture_requires_two_distinct_paths():
    api = FakeBackend()
    with pytest.raises(windows_job.JobError, match="supplied together"):
        windows_job.launch(["worker.exe"], stdout_path="out.log", _backend=api)
    with pytest.raises(windows_job.JobError, match="must differ"):
        windows_job.launch(
            ["worker.exe"], stdout_path="out.log", stderr_path="out.log", _backend=api
        )
    assert api.calls == []


def test_invalid_command_has_no_process_side_effects():
    api = FakeBackend()
    with pytest.raises(windows_job.JobError, match="command"):
        windows_job.launch([], _backend=api)
    assert api.calls == []


@pytest.mark.skipif(sys.platform == "win32", reason="requires non-Windows host")
def test_native_backend_rejects_non_windows_host():
    with pytest.raises(windows_job.JobError, match="native Windows"):
        windows_job.launch(["worker.exe"])
