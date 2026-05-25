"""Tests for tools/ci/timing/rust_profile_history.py."""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "timing" / "rust_profile_history.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("rust_profile_history", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["rust_profile_history"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def test_handle_append_profile_writes_jsonl(tmp_path, monkeypatch):
    state_root = tmp_path / "state"
    monkeypatch.setenv("QUANTA_INDEX_STATE_ROOT", str(state_root))

    args = argparse.Namespace(
        profile="dev-fast",
        delegated_recipe="rust-check-fast",
        exit_code=0,
        duration_ms=512,
    )

    assert MODULE.handle_append_profile(args) == 0
    events = MODULE.load_events(state_root / "build-profile" / "history.jsonl")
    assert len(events) == 1
    assert events[0]["k"] == "profile"
    assert events[0]["v"] == 2
    assert events[0]["profile"] == "dev-fast"
    assert events[0]["recipe"] == "rust-check-fast"
    assert events[0]["rc"] == 0
    assert "cwd" not in events[0]
    assert "repo_root" not in events[0]


def test_handle_append_cargo_writes_compact_payload(tmp_path, monkeypatch):
    state_root = tmp_path / "state"
    monkeypatch.setenv("QUANTA_INDEX_STATE_ROOT", str(state_root))

    args = argparse.Namespace(
        lane="fast-lane",
        lane_source="explicit",
        exit_code=101,
        duration_ms=2048,
        cargo_subcommand="check",
    )

    assert MODULE.handle_append_cargo(args) == 0
    events = MODULE.load_events(state_root / "build-profile" / "history.jsonl")
    assert len(events) == 1
    assert events[0]["k"] == "cargo"
    assert events[0]["lane"] == "fast-lane"
    assert events[0]["src"] == "explicit"
    assert events[0]["cmd"] == "check"
    assert events[0]["rc"] == 101
    assert "argv" not in events[0]


def test_summary_json_aggregates_profiles_and_failures(tmp_path, monkeypatch, capsys):
    state_root = tmp_path / "state"
    monkeypatch.setenv("QUANTA_INDEX_STATE_ROOT", str(state_root))

    log_path = state_root / "build-profile" / "history.jsonl"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text(
        "\n".join(
            [
                json.dumps(
                    {
                        "v": 2,
                        "ts": "2026-05-25T12:00:00.000+00:00",
                        "k": "profile",
                        "profile": "dev-fast",
                        "recipe": "rust-check-fast",
                        "ms": 400,
                        "rc": 0,
                    }
                ),
                json.dumps(
                    {
                        "schema_version": 1,
                        "recorded_at_utc": "2026-05-25T12:10:00.000+00:00",
                        "event_kind": "profile",
                        "profile": "dev-fast",
                        "delegated_recipe": "rust-check-fast",
                        "duration_ms": 800,
                        "exit_code": 101,
                    }
                ),
                json.dumps(
                    {
                        "v": 2,
                        "ts": "2026-05-25T12:20:00.000+00:00",
                        "k": "cargo",
                        "lane": "fast-lane",
                        "src": "auto",
                        "cmd": "check",
                        "ms": 1200,
                        "rc": 0,
                    }
                ),
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    args = argparse.Namespace(command="summary", json=True, failure_limit=5)
    assert MODULE.handle_summary(args) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["total_events"] == 3
    assert payload["profiles"][0]["key"] == "dev-fast"
    assert payload["profiles"][0]["failures"] == 1
    assert payload["cargo_lanes"][0]["key"] == "fast-lane"
    assert payload["latest_failures"][0]["profile"] == "dev-fast"
    assert payload["latest_failures"][0]["delegated_recipe"] == "rust-check-fast"


def test_summary_json_reads_legacy_v1_events(tmp_path, monkeypatch, capsys):
    state_root = tmp_path / "state"
    monkeypatch.setenv("QUANTA_INDEX_STATE_ROOT", str(state_root))

    log_path = state_root / "build-profile" / "history.jsonl"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "recorded_at_utc": "2026-05-25T12:20:00.000+00:00",
                "event_kind": "cargo",
                "lane": "fast-lane",
                "lane_source": "auto",
                "duration_ms": 1200,
                "exit_code": 101,
                "argv": ["check", "--workspace"],
            }
        )
        + "\n",
        encoding="utf-8",
    )

    args = argparse.Namespace(command="summary", json=True, failure_limit=5)
    assert MODULE.handle_summary(args) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["cargo_lanes"][0]["key"] == "fast-lane"
    assert payload["latest_failures"][0]["lane"] == "fast-lane"
    assert payload["latest_failures"][0]["cargo_subcommand"] == "check"
