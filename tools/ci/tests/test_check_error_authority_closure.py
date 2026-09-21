from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools/ci/check-error-authority-closure.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_error_authority_closure", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_free_form_producer_and_control_flow_tripwires() -> None:
    module = _load_module()
    path = "crates/quanta-index-search-plane/src/example.rs"
    for line in [
        "pub code: String,",
        'code: format!("LEX_REGEX_{}", kind),',
        'if code == "NOT_READY" {',
        'if error_code.as_wire_str().contains("PARSE") {',
        'fn typed(code: &str) -> CoreError {',
    ]:
        assert module.scan_line(path, 7, line), line


def test_lower_domain_decoder_is_not_misclassified_as_passthrough() -> None:
    module = _load_module()
    path = "crates/quanta-index-core/src/domains/auxiliary.rs"
    assert not module.scan_line(path, 77, "pub fn from_code_str(code: &str) -> Option<Self> {")
    assert module.scan_line(path, 77, "pub fn forward(code: &str) -> CoreError {")


def test_message_text_assertion_is_not_error_code_substring_classification() -> None:
    module = _load_module()
    line = 'assert!(code == SearchPlaneErrorCodeV2::NotReady && message.contains("warming"));'
    assert not module.scan_line("crates/quanta-index-core/src/error.rs", 20, line)
