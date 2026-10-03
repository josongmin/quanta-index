"""P00 CI host identity must be observed rather than synthesized."""

from __future__ import annotations

import json

import pytest

from tools.ci.proof_host import circleci_host_environment, github_host_environment

ENV = {
    "GITHUB_ACTIONS": "true",
    "RUNNER_NAME": "runner-1",
    "RUNNER_OS": "Linux",
    "RUNNER_ARCH": "X64",
    "RUNNER_ENVIRONMENT": "github-hosted",
    "ImageOS": "ubuntu24",
    "ImageVersion": "20260925.1",
    "GITHUB_RUN_ID": "123",
    "GITHUB_RUN_ATTEMPT": "2",
}

CIRCLE_ENV = {
    "CIRCLECI": "true",
    "CIRCLE_JOB": "verify-python",
    "CIRCLE_WORKFLOW_ID": "59afd36b-3c98-4684-a4ae-2cbd52bba0e7",
    "CIRCLE_BUILD_NUM": "849",
    "CIRCLE_SHA1": "a" * 40,
}


def circle_observed():
    return circleci_host_environment(
        environ=CIRCLE_ENV,
        cpu_count=lambda: 2,
        sysconf=lambda key: {"SC_PAGE_SIZE": 4096, "SC_PHYS_PAGES": 1024}[key],
        system=lambda: "Linux",
        hostname=lambda: "circle-node",
    )


def test_circleci_machine_identity_is_source_bound():
    observed = circle_observed()
    assert observed["profile"] == "circleci-machine-medium"
    assert observed["cpu_count"] == 2
    assert observed["memory_bytes"] == 4_194_304
    assert json.loads(observed["identity"]) == [
        "verify-python",
        "59afd36b-3c98-4684-a4ae-2cbd52bba0e7",
        "849",
        "a" * 40,
        "circle-node",
    ]


@pytest.mark.parametrize(
    "missing", ["CIRCLE_JOB", "CIRCLE_WORKFLOW_ID", "CIRCLE_BUILD_NUM", "CIRCLE_SHA1"]
)
def test_circleci_missing_identity_is_refused(missing):
    with pytest.raises(ValueError, match="incomplete"):
        circleci_host_environment(
            environ={key: value for key, value in CIRCLE_ENV.items() if key != missing},
            system=lambda: "Linux",
        )


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ({"CIRCLECI": "false"}, "CircleCI"),
        ({"CIRCLE_BUILD_NUM": "not-a-number"}, "invalid"),
        ({"CIRCLE_SHA1": "short"}, "invalid"),
    ],
)
def test_circleci_invalid_identity_is_refused(change, message):
    with pytest.raises(ValueError, match=message):
        circleci_host_environment(environ={**CIRCLE_ENV, **change}, system=lambda: "Linux")


def test_circleci_non_linux_host_is_refused():
    with pytest.raises(ValueError, match="Linux"):
        circleci_host_environment(environ=CIRCLE_ENV, system=lambda: "Darwin")


def observed(**kwargs):
    return github_host_environment(
        environ=ENV,
        cpu_count=lambda: 4,
        sysconf=lambda key: {"SC_PAGE_SIZE": 4096, "SC_PHYS_PAGES": 1024}[key],
        **kwargs,
    )


def test_complete_observed_host_is_bound():
    assert observed() == {
        "profile": "github-hosted-ci",
        "cpu_count": 4,
        "memory_bytes": 4_194_304,
        "identity": '["runner-1","Linux","X64","github-hosted","ubuntu24","20260925.1","123","2"]',
    }


@pytest.mark.parametrize(
    "missing",
    [
        "RUNNER_NAME",
        "RUNNER_OS",
        "RUNNER_ARCH",
        "ImageOS",
        "ImageVersion",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
    ],
)
def test_missing_identity_is_refused(missing):
    with pytest.raises(ValueError, match="incomplete"):
        github_host_environment(
            environ={key: value for key, value in ENV.items() if key != missing},
            cpu_count=lambda: 4,
            sysconf=lambda _: 4096,
        )


@pytest.mark.parametrize("cpus", [None, 0, -1, True])
def test_unknown_cpu_is_refused(cpus):
    with pytest.raises(ValueError, match="CPU count"):
        github_host_environment(environ=ENV, cpu_count=lambda: cpus, sysconf=lambda _: 4096)


def test_unknown_memory_is_refused():
    with pytest.raises(ValueError, match="memory"):
        github_host_environment(environ=ENV, cpu_count=lambda: 4, sysconf=lambda _: 0)


def test_memory_probe_failure_is_refused():
    def unavailable(_key):
        raise OSError("sysconf unavailable")

    with pytest.raises(ValueError, match="memory is unavailable"):
        github_host_environment(environ=ENV, cpu_count=lambda: 4, sysconf=unavailable)


def test_non_ci_host_is_refused():
    with pytest.raises(ValueError, match="GitHub Actions"):
        github_host_environment(
            environ={**ENV, "GITHUB_ACTIONS": "false"},
            cpu_count=lambda: 4,
            sysconf=lambda _: 4096,
        )


def test_self_hosted_runner_is_refused():
    with pytest.raises(ValueError, match="GitHub-hosted"):
        github_host_environment(
            environ={**ENV, "RUNNER_ENVIRONMENT": "self-hosted"},
            cpu_count=lambda: 4,
            sysconf=lambda _: 4096,
        )
