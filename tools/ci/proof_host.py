"""Observed CI host identity for proof manifests; unknown inputs block publication."""

from __future__ import annotations

import json
import os
import platform
import re
import socket
from collections.abc import Callable, Mapping

REQUIRED_IDENTITY = (
    "RUNNER_NAME",
    "RUNNER_OS",
    "RUNNER_ARCH",
    "RUNNER_ENVIRONMENT",
    "ImageOS",
    "ImageVersion",
    "GITHUB_RUN_ID",
    "GITHUB_RUN_ATTEMPT",
)

CIRCLE_IDENTITY = ("CIRCLE_JOB", "CIRCLE_WORKFLOW_ID", "CIRCLE_BUILD_NUM", "CIRCLE_SHA1")


def circleci_host_environment(
    *,
    environ: Mapping[str, str] | None = None,
    cpu_count: Callable[[], int | None] = os.cpu_count,
    sysconf: Callable[[str], int] = os.sysconf,
    system: Callable[[], str] = platform.system,
    hostname: Callable[[], str] = socket.gethostname,
) -> dict[str, int | str]:
    env = os.environ if environ is None else environ
    if env.get("CIRCLECI") != "true":
        raise ValueError("proof host requires an observed CircleCI job")
    if system() != "Linux":
        raise ValueError("proof host requires a Linux CircleCI Machine executor")
    components = [env.get(key, "").strip() for key in CIRCLE_IDENTITY]
    if any(not value for value in components):
        missing = [key for key, value in zip(CIRCLE_IDENTITY, components) if not value]
        raise ValueError(f"proof host identity is incomplete: {missing}")
    if not components[2].isdecimal() or not re.fullmatch(r"[0-9a-f]{40}", components[3]):
        raise ValueError("proof host build number or source SHA is invalid")
    node = hostname().strip()
    if not node:
        raise ValueError("proof host hostname is unavailable")
    cpus = cpu_count()
    if type(cpus) is not int or cpus < 1:
        raise ValueError("proof host CPU count is unavailable")
    try:
        page_size = sysconf("SC_PAGE_SIZE")
        pages = sysconf("SC_PHYS_PAGES")
    except (AttributeError, OSError, ValueError) as error:
        raise ValueError("proof host memory is unavailable") from error
    if type(page_size) is not int or type(pages) is not int or page_size < 1 or pages < 1:
        raise ValueError("proof host memory is invalid")
    return {
        "profile": "circleci-machine-medium",
        "cpu_count": cpus,
        "memory_bytes": page_size * pages,
        "identity": json.dumps([*components, node], separators=(",", ":")),
    }


def github_host_environment(
    *,
    environ: Mapping[str, str] | None = None,
    cpu_count: Callable[[], int | None] = os.cpu_count,
    sysconf: Callable[[str], int] = os.sysconf,
) -> dict[str, int | str]:
    env = os.environ if environ is None else environ
    if env.get("GITHUB_ACTIONS") != "true":
        raise ValueError("proof host requires an observed GitHub Actions runner")
    if env.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise ValueError("proof host requires a GitHub-hosted runner")
    components = [env.get(key, "").strip() for key in REQUIRED_IDENTITY]
    if any(not value for value in components):
        missing = [key for key, value in zip(REQUIRED_IDENTITY, components) if not value]
        raise ValueError(f"proof host identity is incomplete: {missing}")
    cpus = cpu_count()
    if type(cpus) is not int or cpus < 1:
        raise ValueError("proof host CPU count is unavailable")
    try:
        page_size = sysconf("SC_PAGE_SIZE")
        pages = sysconf("SC_PHYS_PAGES")
    except (AttributeError, OSError, ValueError) as error:
        raise ValueError("proof host memory is unavailable") from error
    if type(page_size) is not int or type(pages) is not int or page_size < 1 or pages < 1:
        raise ValueError("proof host memory is invalid")
    return {
        "profile": "github-hosted-ci",
        "cpu_count": cpus,
        "memory_bytes": page_size * pages,
        "identity": json.dumps(components, separators=(",", ":")),
    }
