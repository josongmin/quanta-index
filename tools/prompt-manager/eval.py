#!/usr/bin/env python3
"""Source-bound decision probes; this does not qualify agent tool execution."""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import subprocess
from pathlib import Path

import jinja2
import pm
import yaml

CASES = pm.PM_DIR / "evals" / "cases.json"


class EvalError(ValueError):
    pass


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise EvalError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def parse_json(raw):
    try:
        return json.loads(raw, object_pairs_hook=unique_object)
    except (ValueError, TypeError) as error:
        raise EvalError(str(error)) from error


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def load_cases():
    document = parse_json(CASES.read_bytes())
    if not isinstance(document, dict) or document.get("version") != 1:
        raise EvalError("unsupported case corpus")
    cases = document.get("cases")
    if not isinstance(cases, list) or not cases:
        raise EvalError("empty or malformed case corpus")
    seen = set()
    for case in cases:
        if not isinstance(case, dict) or set(case) != {"id", "input", "expected"}:
            raise EvalError("malformed case")
        if not isinstance(case["id"], str) or not case["id"] or case["id"] in seen:
            raise EvalError("empty or duplicate case ID")
        seen.add(case["id"])
        if not isinstance(case["input"], str) or not case["input"]:
            raise EvalError("missing case input")
        expected = case["expected"]
        if not isinstance(expected, dict) or set(expected) != {"status", "next_action"}:
            raise EvalError("malformed case oracle")
        if (
            expected["status"] not in {"VERIFIED", "FAILED", "BLOCKED", "NOT_RUN", "NOT_APPLICABLE"}
            or not isinstance(expected["next_action"], str)
            or not expected["next_action"]
        ):
            raise EvalError("invalid case oracle")
    return cases


def request_document():
    cases = load_cases()
    targets = pm.load_targets()
    rendered = {}
    for target in targets:
        content = pm.render(target)
        if not target.output.is_file() or target.output.read_text(encoding="utf-8") != content:
            raise EvalError(f"prompt drift: {target.name}; run pm.py sync")
        rendered[target.name] = content
    files = {
        path
        for path in pm.PM_DIR.rglob("*")
        if path.is_file() and path.suffix in {".py", ".md", ".j2", ".yaml", ".json"}
    } | {target.output for target in targets}
    source_files = {
        path.relative_to(pm.ROOT).as_posix(): digest(path.read_bytes()) for path in sorted(files)
    }
    source_digest = digest(json.dumps(source_files, sort_keys=True).encode())
    prompt = (
        "Apply the repository guidance to each hypothetical scenario independently. "
        "Status means the requested verification scope's execution/evidence state, "
        "not the boolean truth of an assertion. "
        'Return only JSON: {"results":[{"id":"case ID",'
        '"status":"VERIFIED|FAILED|BLOCKED|NOT_RUN|NOT_APPLICABLE",'
        '"next_action":"action"}]}. Return every case once, in the supplied order. '
        "Actions: run_required_rail, rerun_on_current_source, request_required_input, "
        "report_execution_failure, edit_prompt_sources, preserve_unrelated_changes, "
        "report_covered_scope. Do not execute tools; these are decision probes.\n\n"
        + "\n\n".join(rendered[name] for name in ["agents", "agent-core", "agent-playbook"])
        + "\n\n<scenario_data>\n"
        + json.dumps([{"id": case["id"], "input": case["input"]} for case in cases])
        + "\n</scenario_data>\nClassify each requested claim and select its next action.\n"
    )
    git_args = ["git", "-C", str(pm.ROOT)]
    return {
        "version": 1,
        "git_head": subprocess.check_output(git_args + ["rev-parse", "HEAD"], text=True).strip(),
        "prompt_dirty_state": subprocess.check_output(
            git_args
            + [
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                "tools/prompt-manager",
                *[str(t.output.relative_to(pm.ROOT)) for t in targets],
            ],
            text=True,
        ),
        "runtime": {
            "python": platform.python_version(),
            "platform": platform.platform(),
            "jinja2": jinja2.__version__,
            "pyyaml": yaml.__version__,
        },
        "source_files": source_files,
        "source_sha256": source_digest,
        "cases_sha256": digest(CASES.read_bytes()),
        "prompt_sha256": digest(prompt.encode()),
        "prompt": prompt,
    }


def grade(request, raw_response):
    if request != request_document():
        raise EvalError("request is stale, tampered, or from another source/environment")
    response = parse_json(raw_response)
    model_usage = None
    if isinstance(response, dict) and response.get("type") == "result":
        if response.get("is_error") is not False:
            raise EvalError("model invocation failed")
        model_usage = response.get("modelUsage")
        response = (
            response["structured_output"]
            if "structured_output" in response
            else parse_json(response.get("result"))
        )
    if not isinstance(response, dict) or set(response) != {"results"}:
        raise EvalError("response must contain only results")
    results = response["results"]
    cases = load_cases()
    if not isinstance(results, list) or len(results) != len(cases):
        raise EvalError("missing or extra case results")
    failures = []
    for case, result in zip(cases, results):
        if not isinstance(result, dict) or set(result) != {"id", "status", "next_action"}:
            raise EvalError("malformed result")
        if result["id"] != case["id"]:
            raise EvalError("wrong, duplicate, or reordered case ID")
        actual = {key: result[key] for key in ["status", "next_action"]}
        if actual != case["expected"]:
            failures.append({"id": case["id"], "actual": actual, "expected": case["expected"]})
    return {
        "version": 1,
        "status": "passed" if not failures else "failed",
        "source_sha256": request["source_sha256"],
        "cases_sha256": request["cases_sha256"],
        "response_sha256": digest(raw_response),
        "model_usage_reported_by_cli": model_usage,
        "selected": len(cases),
        "executed": len(results),
        "passed": len(cases) - len(failures),
        "failed": len(failures),
        "failures": failures,
        "scope": "single-response decision probes; no tool execution or installed loading proof",
        "qualification": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--request", type=Path, required=True)
    prepare.add_argument("--prompt", type=Path, required=True)
    evaluate = commands.add_parser("grade")
    evaluate.add_argument("--request", type=Path, required=True)
    evaluate.add_argument("--responses", type=Path, required=True)
    evaluate.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            request = request_document()
            args.request.write_text(json.dumps(request, indent=2) + "\n", encoding="utf-8")
            args.prompt.write_text(request["prompt"], encoding="utf-8")
        else:
            result = grade(parse_json(args.request.read_bytes()), args.responses.read_bytes())
            args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            print(json.dumps(result))
            return 0 if result["status"] == "passed" else 1
    except (EvalError, OSError, subprocess.CalledProcessError, pm.PromptManagerError) as error:
        parser.exit(2, f"evaluation refused: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
