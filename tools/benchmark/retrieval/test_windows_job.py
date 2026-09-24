"""Contract tests for Windows ownership ordering, runnable on non-Windows hosts."""

from __future__ import annotations

import sys

import pytest

from tools.benchmark.retrieval import windows_job


class FakeBackend:
    def __init__(self, *, fail: str | None = None, active: list[int] | None = None):
        self.fail = fail
        self.active = iter(active if active is not None else [0])
        self.calls = []

    def _record(self, name, *args):
        self.calls.append((name, *args))
        if self.fail == name:
            raise windows_job.JobError(name)

    def create_job(self):
        self._record("create_job")
        return 10

    def spawn_suspended(self, command, cwd, env):
        self._record("spawn_suspended", command, cwd, env)
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

    def wait(self, handle, timeout_ms):
        self._record("wait", handle, timeout_ms)
        return True

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


def test_invalid_command_has_no_process_side_effects():
    api = FakeBackend()
    with pytest.raises(windows_job.JobError, match="command"):
        windows_job.launch([], _backend=api)
    assert api.calls == []


@pytest.mark.skipif(sys.platform == "win32", reason="requires non-Windows host")
def test_native_backend_rejects_non_windows_host():
    with pytest.raises(windows_job.JobError, match="native Windows"):
        windows_job.launch(["worker.exe"])
