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
    for source, reason in [
        ("struct Wire { pub code:\n String }", "free-form code field"),
        (
            'fn f() { let _ = Error { code:\n format!("LEX_REGEX_{}", kind) }; }',
            "dynamic code producer",
        ),
        ('fn f() { if code /* wire */\n == "NOT_READY" {} }', "code string comparison"),
        ('fn f() { if r#"NOT_READY"#\n == code {} }', "code string comparison"),
        (
            'fn f() { if error_code\n.as_wire_str ()\n.contains("PARSE") {} }',
            "code substring classification",
        ),
        ("fn typed(code:\n &str) -> CoreError { todo!() }", "unowned code &str pass-through"),
    ]:
        assert module.scan_source(path, source) == [f"{path}:1: {reason}"], source


def test_comments_and_string_contents_do_not_produce_findings() -> None:
    module = _load_module()
    path = "crates/quanta-index-search-plane/src/example.rs"
    source = (
        "// pub code: String\n"
        '/* code == "NOT_READY" */\n'
        'const EXAMPLE: &str = r#"code: format!(...)"#;\n'
    )
    assert not module.scan_source(path, source)


def test_lower_domain_decoder_is_not_misclassified_as_passthrough() -> None:
    module = _load_module()
    path = "crates/quanta-index-core/src/domains/auxiliary.rs"
    decoder = "pub fn from_code_str(\n code: &str,\n) -> Option<Self> { None }\n"
    assert not module.scan_source(path, decoder)
    source = decoder + "pub fn forward(code: &str) -> CoreError { todo!() }"
    assert module.scan_source(path, source) == [f"{path}:4: unowned code &str pass-through"]


def test_parser_recovery_preserves_checks_in_unrelated_items() -> None:
    module = _load_module()
    path = "crates/quanta-index-catalog/src/auxiliary.rs"
    source = (
        "fn track_from_code(code: &str) -> usize { code.len() }\n"
        "fn read(raw: Row) { let _ = inspect(&raw.domain); }\n"
        "// pub code: String\n"
        "struct Wire { code: String }\n"
    )
    assert module.scan_source(path, source) == [f"{path}:4: free-form code field"]


def test_message_text_assertion_is_not_error_code_substring_classification() -> None:
    module = _load_module()
    source = 'fn f() { assert!(code == SearchPlaneErrorCodeV2::NotReady && message.contains("warming")); }'
    assert not module.scan_source("crates/quanta-index-core/src/error.rs", source)


def test_only_test_configuration_can_hide_wire_assertions() -> None:
    module = _load_module()
    path = "crates/quanta-index-search-plane/src/example.rs"
    source = (
        "#[cfg(test)]\n"
        "// Keep this attribute attached across comments.\n"
        'mod checks { fn wire() { if code == "WIRE" {} } }\n'
        'fn produce() { if code == "PRODUCTION" {} }\n'
    )
    assert module.scan_source(path, source) == [f"{path}:4: code string comparison"]

    mixed = source.replace("cfg(test)", 'cfg(any(test, feature = "route"))')
    assert module.scan_source(path, mixed) == [
        f"{path}:3: code string comparison",
        f"{path}:4: code string comparison",
    ]


def test_cutover_record_exception_is_limited_to_the_exact_type_and_macro() -> None:
    module = _load_module()
    path = "crates/quanta-index-contract/src/native_cutover_evidence_v1.rs"
    source = (
        "enum NativeCutoverControlTerminalV1 { Failed { code: String } }\n"
        "crate::native_serde_enum!(NativeCutoverControlTerminalV1 { code: String });\n"
        "enum Another { Failed { code: String } }\n"
    )
    assert module.scan_source(path, source) == [f"{path}:3: free-form code field"]
    assert len(module.scan_source("crates/quanta-index-contract/src/ipc/example.rs", source)) == 3


def test_protocol_neutral_accounting_does_not_exempt_its_consumers() -> None:
    module = _load_module()
    path = "crates/quanta-index-searchd-harness/src/open_loop_accounting.rs"
    source = (
        'enum Outcome { Failed { code: String } }\nfn classify() { if code == "PRODUCTION" {} }\n'
    )
    assert module.scan_source(path, source) == [f"{path}:2: code string comparison"]
