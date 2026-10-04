"""Host custody of an owned Zoekt translator export."""

import hashlib
import json

import pytest

from tools.benchmark.retrieval import live_lexical_external as live


@pytest.mark.parametrize(
    ("case", "refusal"),
    [
        ("success", None),
        ("stale_ticks", "proc executable export failed"),
        ("wrong_sha", "export identity differs"),
        ("wrong_host_sha", "executable bytes differ"),
        ("over_cap", "export identity differs"),
        ("cp_failure", "executable bytes differ"),
        ("cleanup_failure", "export cleanup is unverified"),
        ("indeterminate_export", "export disconnected"),
    ],
)
def test_owned_translator_export_custody_and_cleanup(tmp_path, monkeypatch, case, refusal):
    scope = live.sourcegraph_index_scope
    container = "c" * 64
    token = "a" * 32
    raw = b"bound Rosetta translator executable bytes"
    digest = hashlib.sha256(raw).hexdigest()
    remote = "/tmp/qi-sg-owned-translator-" + token
    process = {"pid": 89, "start_ticks": 456, "exe_sha256": digest}
    calls = []

    def fake_process(argv, _timeout):
        calls.append(argv)
        if argv[1] == "cp":
            assert argv[2] == f"{container}:{remote}"
            assert "/proc/" not in argv[2]
            if case == "cp_failure":
                return 1, b"", b"Docker cp refused", 0.01
            (tmp_path / "native-translator").write_bytes(
                raw[::-1] if case == "wrong_host_sha" else raw
            )
            return 0, b"", b"", 0.01
        assert argv[:5] == ["docker", "exec", container, "python3", "-c"]
        if len(argv) == 11:
            assert argv[6:10] == ["89", "456", token, digest]
            if case == "indeterminate_export":
                raise TimeoutError("export disconnected")
            if case == "stale_ticks":
                return 3, b"", b"", 0.01
            result = {
                "path": remote,
                "sha256": "0" * 64 if case == "wrong_sha" else digest,
                "bytes": scope.MAX_TRANSLATOR_BYTES + 1 if case == "over_cap" else len(raw),
                "device": 64,
                "inode": 1234,
                "pid": 89,
                "start_ticks": 456,
            }
            return 0, json.dumps(result).encode(), b"", 0.01
        assert len(argv) == 9
        assert argv[6] == token
        assert argv[8] == str(scope.MAX_TRANSLATOR_BYTES)
        if case == "cleanup_failure":
            return 6, b"", b"cleanup unverified", 0.01
        return (
            0,
            json.dumps(
                {
                    "tombstone_created": True,
                    "removed": True,
                    "owned_worker_stopped": True,
                    "path": remote,
                }
            ).encode(),
            b"",
            0.01,
        )

    monkeypatch.setattr(live, "_process", fake_process)
    if refusal is None:
        scope._capture_translator_bytes(tmp_path, container, process, token)
        assert (tmp_path / "native-translator").read_bytes() == raw
    else:
        with pytest.raises((ValueError, TimeoutError), match=refusal):
            scope._capture_translator_bytes(tmp_path, container, process, token)
    assert len([argv for argv in calls if argv[1] == "exec" and len(argv) == 9]) == 1
