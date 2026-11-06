"""Fixed Mach-O/sample gold for offline stack companion address matching."""

from __future__ import annotations

import hashlib
import struct
from pathlib import Path

import pytest

from tools.benchmark.retrieval.native_stack_symbols import (
    SymbolMatchError,
    analyze,
    parse_macho,
)

BASE = 0x100000000
LOAD = 0x100100000
UUID = bytes.fromhex("00112233445566778899aabbccddeeff")


def macho(
    *,
    code: bytes = b"\x1f\x20\x03\xd5\xc0\x03\x5f\xd6",
    section_address: int = BASE + 0x200,
    uuid: bytes = UUID,
    header_flags: int = 0,
    arch: int = 0x0100000C,
) -> bytes:
    sections = [
        struct.pack(
            "<16s16sQQIIIIIIII",
            b"__text",
            b"__TEXT",
            section_address,
            len(code),
            0x200,
            2,
            0,
            0,
            0x80000000,
            0,
            0,
            0,
        ),
        struct.pack(
            "<16s16sQQIIIIIIII",
            b"__cstring",
            b"__TEXT",
            BASE + 0x210,
            3,
            0x210,
            0,
            0,
            0,
            2,
            0,
            0,
            0,
        ),
    ]
    segment = struct.pack(
        "<II16sQQQQiiII",
        0x19,
        72 + 80 * len(sections),
        b"__TEXT",
        BASE,
        0x1000,
        0,
        0x1000,
        7,
        5,
        len(sections),
        0,
    ) + b"".join(sections)
    uuid_command = struct.pack("<II16s", 0x1B, 24, uuid)
    header = struct.pack(
        "<IiiIIIII",
        0xFEEDFACF,
        arch,
        0,
        2,
        2,
        len(segment) + len(uuid_command),
        header_flags,
        0,
    )
    payload = bytearray(0x1000)
    payload[: len(header + segment + uuid_command)] = header + segment + uuid_command
    payload[0x200 : 0x200 + len(code)] = code
    payload[0x210:0x213] = b"ok\0"
    return bytes(payload)


def sample(
    *, module: str = "quanta-index-searchd", load: int = LOAD, uuid: bytes = UUID
) -> str:
    hex_uuid = uuid.hex().upper()
    dashed_uuid = (
        f"{hex_uuid[:8]}-{hex_uuid[8:12]}-{hex_uuid[12:16]}-"
        f"{hex_uuid[16:20]}-{hex_uuid[20:]}"
    )
    return (
        f"Load Address:    {load:#x}\n"
        "Code Type:       ARM64\n"
        "Call graph:\n"
        f"    +   ! 3 ???  (in {module})  load address {load:#x} + 0x200  [{load + 0x200:#x}]\n"
        f"    + 2 ???  (in {module})  load address {load:#x} + 0x204  [{load + 0x204:#x}]\n"
        "Binary Images:\n"
        f"       {load:#x} - {load + 0xfff:#x} +{module} (???) "
        f"<{dashed_uuid}> /private/tmp/{module}\n"
    )


def inputs(tmp_path: Path, *, original: bytes | None = None, companion: bytes | None = None, report: str | None = None):
    old = tmp_path / "quanta-index-searchd"
    new = tmp_path / "companion"
    text = tmp_path / "sample.txt"
    old.write_bytes(original if original is not None else macho())
    new.write_bytes(companion if companion is not None else macho(uuid=b"\xaa" * 16, header_flags=17))
    text.write_text(report if report is not None else sample())
    return old, new, text, hashlib.sha256(old.read_bytes()).hexdigest()


def test_same_code_allows_distinct_header_uuid_and_retains_unresolved(tmp_path: Path) -> None:
    old, new, text, digest = inputs(tmp_path)
    called = []

    def resolver(path: Path, base: int, addresses: list[int]) -> list[str]:
        called.append((path, base, addresses))
        return ["quanta::source::apply", f"{BASE + 0x204:#x}"]

    result = analyze(old, digest, new, text, old.name, atos=resolver)
    assert result["status"] == "LAYOUT_MATCHED_DIAGNOSTIC"
    assert result["symbol_name_provenance"] == "UNVERIFIED"
    assert result["original_uuid"] != result["companion_uuid"]
    assert called == [(new, BASE, [BASE + 0x200, BASE + 0x204])]
    assert (
        result["distinct_offsets"],
        result["atos_candidate_offsets"],
        result["unresolved_offsets"],
    ) == (2, 1, 1)
    assert [row["sample_frame_lines"] for row in result["addresses"]] == [1, 1]
    assert result["addresses"][0]["resolution"] == "atos_candidate"
    assert result["addresses"][1]["atos"] is None
    assert result["addresses"][1]["resolution"] == "unresolved"


@pytest.mark.parametrize(
    "candidate",
    [
        macho(code=b"\x00" * 8),
        macho(section_address=BASE + 0x204),
        macho(arch=0x01000007),
        b"\xca\xfe\xba\xbe" + b"\0" * 128,
        macho()[:90],
    ],
)
def test_code_address_arch_fat_and_truncated_candidates_refuse(tmp_path: Path, candidate: bytes) -> None:
    old, new, text, digest = inputs(tmp_path, companion=candidate)
    with pytest.raises(SymbolMatchError):
        analyze(old, digest, new, text, old.name, atos=lambda *_args: pytest.fail("atos called"))


def test_overflow_and_out_of_file_sections_refuse() -> None:
    with pytest.raises(SymbolMatchError):
        parse_macho(macho(section_address=(1 << 64) - 3))
    bad = bytearray(macho())
    # section_64.offset is at segment command + 72 + 48.
    struct.pack_into("<I", bad, 32 + 72 + 48, 0xFFFF_FFF0)
    with pytest.raises(SymbolMatchError):
        parse_macho(bytes(bad))


@pytest.mark.parametrize(
    "change",
    [
        "sha",
        "wrong_module",
        "wrong_load",
        "wrong_uuid",
        "wrong_frame_address",
        "wrong_frame_section",
    ],
)
def test_wrong_original_or_sample_binding_refuses(tmp_path: Path, change: str) -> None:
    report = sample()
    if change == "wrong_module":
        report = sample(module="other-daemon")
    elif change == "wrong_load":
        report = report.replace(f"Load Address:    {LOAD:#x}", f"Load Address:    {LOAD + 4:#x}")
    elif change == "wrong_uuid":
        report = sample(uuid=b"\xff" * 16)
    elif change == "wrong_frame_address":
        report = report.replace(f"[{LOAD + 0x200:#x}]", f"[{LOAD + 0x201:#x}]")
    elif change == "wrong_frame_section":
        report = report.replace("+ 0x200", "+ 0x210").replace(
            f"[{LOAD + 0x200:#x}]", f"[{LOAD + 0x210:#x}]"
        )
    old, new, text, digest = inputs(tmp_path, report=report)
    if change == "sha":
        digest = "0" * 64
    with pytest.raises(SymbolMatchError):
        analyze(old, digest, new, text, old.name, atos=lambda *_args: pytest.fail("atos called"))


def test_partial_resolver_refuses(tmp_path: Path) -> None:
    old, new, text, digest = inputs(tmp_path)
    with pytest.raises(SymbolMatchError, match="partial"):
        analyze(old, digest, new, text, old.name, atos=lambda *_args: ["one"])


def test_malformed_selected_frame_refuses(tmp_path: Path) -> None:
    report = sample().replace("load address", "wrong address", 1)
    old, new, text, digest = inputs(tmp_path, report=report)
    with pytest.raises(SymbolMatchError, match="malformed"):
        analyze(old, digest, new, text, old.name, atos=lambda *_args: pytest.fail("atos called"))


def test_companion_mutation_during_lookup_refuses(tmp_path: Path) -> None:
    old, new, text, digest = inputs(tmp_path)

    def mutating_resolver(_path: Path, _base: int, addresses: list[int]) -> list[str]:
        new.write_bytes(macho(code=b"\x00" * 8))
        return ["symbol"] * len(addresses)

    with pytest.raises(SymbolMatchError, match="changed during atos"):
        analyze(old, digest, new, text, old.name, atos=mutating_resolver)
