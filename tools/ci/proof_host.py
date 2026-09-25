"""Observed CI host identity for proof manifests; unknown inputs block publication."""

from __future__ import annotations

import json
import os
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
