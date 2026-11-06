"""Fail-closed, offline Mach-O companion matching for an existing macOS sample.

This is a diagnostic address lookup, not a CPU profile or benchmark verdict.
The companion is never executed. Only thin arm64 Mach-O executables are accepted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from tools.benchmark.evidence import _consume_regular_file

MAX_BINARY_BYTES = 512 * 1024 * 1024
MAX_SAMPLE_BYTES = 16 * 1024 * 1024
MAX_ADDRESSES = 20_000
MAX_COMMAND_BYTES = 4 * 1024 * 1024
MACH_MAGIC_64 = 0xFEEDFACF
CPU_TYPE_ARM64 = 0x0100000C
MH_EXECUTE = 2
LC_SEGMENT_64 = 0x19
LC_UUID = 0x1B
INSTRUCTION_FLAGS = 0x80000000 | 0x00000400
U64_MAX = (1 << 64) - 1
HEADER = struct.Struct("<IiiIIIII")
SEGMENT = struct.Struct("<II16sQQQQiiII")
SECTION = struct.Struct("<16s16sQQIIIIIIII")
UUID = struct.Struct("<II16s")


class SymbolMatchError(ValueError):
    """The original, sample and companion are not provably address-compatible."""


@dataclass(frozen=True)
class TextSection:
    name: str
    address: int
    size: int
    offset: int
    flags: int
    contents: bytes

    @property
    def executable(self) -> bool:
        return bool(self.flags & INSTRUCTION_FLAGS)


@dataclass(frozen=True)
class MachO:
    preferred_base: int
    text_size: int
    uuid: str
    sections: tuple[TextSection, ...]

    def executable_address(self, address: int) -> bool:
        return any(
            section.executable
            and section.address <= address < section.address + section.size
            for section in self.sections
        )


def _bounded_regular(path: Path, limit: int) -> bytes:
    def consume(handle) -> bytes:
        size = handle.seek(0, 2)
        if size > limit:
            raise SymbolMatchError(f"input exceeds {limit} bytes: {path}")
        handle.seek(0)
        data = handle.read(limit + 1)
        if len(data) > limit or len(data) != size:
            raise SymbolMatchError(f"input changed or exceeds bound: {path}")
        return data

    return _consume_regular_file(path, consume)


def _checked_end(start: int, size: int, limit: int, label: str) -> int:
    end = start + size
    if start < 0 or size < 0 or end > limit or end > U64_MAX:
        raise SymbolMatchError(f"{label} exceeds its bound")
    return end


def _name(raw: bytes) -> str:
    try:
        return raw.split(b"\0", 1)[0].decode("ascii")
    except UnicodeDecodeError as error:
        raise SymbolMatchError("non-ASCII Mach-O section name") from error


def parse_macho(data: bytes) -> MachO:
    """Read SDK loader.h's mach_header_64/segment_command_64/section_64."""
    if len(data) < HEADER.size:
        raise SymbolMatchError("truncated Mach-O header")
    magic, cpu, _subtype, filetype, count, commands_size, _flags, _reserved = HEADER.unpack_from(
        data
    )
    if magic != MACH_MAGIC_64 or cpu != CPU_TYPE_ARM64 or filetype != MH_EXECUTE:
        raise SymbolMatchError("requires a thin little-endian arm64 Mach-O executable")
    if count == 0 or count > 4096 or commands_size > MAX_COMMAND_BYTES:
        raise SymbolMatchError("invalid Mach-O command bound")
    commands_end = _checked_end(HEADER.size, commands_size, len(data), "Mach-O commands")
    cursor = HEADER.size
    text = None
    image_uuid = None
    for _ in range(count):
        if cursor + 8 > commands_end:
            raise SymbolMatchError("truncated Mach-O load command")
        command, command_size = struct.unpack_from("<II", data, cursor)
        end = _checked_end(cursor, command_size, commands_end, "Mach-O load command")
        if command_size < 8:
            raise SymbolMatchError("invalid Mach-O load command size")
        if command == LC_UUID:
            if command_size != UUID.size or image_uuid is not None:
                raise SymbolMatchError("invalid or duplicate Mach-O UUID")
            image_uuid = UUID.unpack_from(data, cursor)[2].hex().upper()
        if command == LC_SEGMENT_64:
            if command_size < SEGMENT.size:
                raise SymbolMatchError("truncated Mach-O segment")
            (
                _cmd,
                _size,
                raw_segment,
                vmaddr,
                vmsize,
                fileoff,
                filesize,
                _maxprot,
                _initprot,
                nsects,
                _segflags,
            ) = SEGMENT.unpack_from(data, cursor)
            if nsects > 4096 or SEGMENT.size + nsects * SECTION.size > command_size:
                raise SymbolMatchError("truncated or oversized Mach-O sections")
            if _name(raw_segment) == "__TEXT":
                if text is not None:
                    raise SymbolMatchError("duplicate __TEXT segment")
                segment_end = _checked_end(fileoff, filesize, len(data), "__TEXT segment")
                _checked_end(vmaddr, vmsize, U64_MAX, "__TEXT VM region")
                sections = []
                seen = set()
                for index in range(nsects):
                    fields = SECTION.unpack_from(data, cursor + SEGMENT.size + index * SECTION.size)
                    section_name, section_segment, addr, size, offset = fields[:5]
                    flags = fields[8]
                    name = _name(section_name)
                    if _name(section_segment) != "__TEXT" or name in seen:
                        raise SymbolMatchError("invalid or duplicate __TEXT section")
                    seen.add(name)
                    _checked_end(addr, size, vmaddr + vmsize, "__TEXT section VM range")
                    section_end = _checked_end(offset, size, segment_end, "__TEXT section bytes")
                    if addr < vmaddr or offset < fileoff or addr - vmaddr != offset - fileoff:
                        raise SymbolMatchError("__TEXT section address/file mapping differs")
                    sections.append(TextSection(name, addr, size, offset, flags, data[offset:section_end]))
                text = MachO(vmaddr, vmsize, "", tuple(sections))
        cursor = end
    if cursor != commands_end or text is None or image_uuid is None:
        raise SymbolMatchError("incomplete Mach-O commands, __TEXT or UUID")
    code = [section for section in text.sections if section.name == "__text"]
    if len(code) != 1 or not code[0].executable or code[0].size == 0:
        raise SymbolMatchError("missing executable __TEXT,__text")
    return MachO(text.preferred_base, text.text_size, image_uuid, text.sections)


def matching_text(original: MachO, companion: MachO) -> None:
    if (
        original.preferred_base != companion.preferred_base
        or original.text_size != companion.text_size
    ):
        raise SymbolMatchError("__TEXT segment VM layout differs")
    if len(original.sections) != len(companion.sections):
        raise SymbolMatchError("__TEXT section count differs")
    for left, right in zip(original.sections, companion.sections):
        if (
            left.name,
            left.address,
            left.size,
            left.offset,
            left.flags,
            left.contents,
        ) != (
            right.name,
            right.address,
            right.size,
            right.offset,
            right.flags,
            right.contents,
        ):
            raise SymbolMatchError(f"__TEXT section differs: {left.name}")


def _sample_addresses(sample: str, module: str, uuid: str, image: MachO) -> tuple[int, dict[int, int]]:
    header = re.findall(r"(?m)^Load Address:\s+(0x[0-9a-fA-F]+)\s*$", sample)
    if len(header) != 1:
        raise SymbolMatchError("sample has no unique Load Address")
    load = int(header[0], 16)
    if load > U64_MAX:
        raise SymbolMatchError("sample load address overflow")
    if len(re.findall(r"(?m)^Code Type:[ \t]+ARM64[ \t]*$", sample)) != 1:
        raise SymbolMatchError("sample is not uniquely ARM64")
    images = re.findall(
        rf"(?m)^\s*(0x[0-9a-fA-F]+)\s+-\s+(0x[0-9a-fA-F]+)\s+\+?{re.escape(module)}\s+\([^\n]*?\)\s+<([0-9a-fA-F-]{{36}})>\s+[^\n]+$",
        sample,
    )
    if len(images) != 1:
        raise SymbolMatchError("sample has no unique selected Binary Images module")
    image_start, image_end = (int(images[0][index], 16) for index in (0, 1))
    if image_start != load or image_end < load or images[0][2].replace("-", "").upper() != uuid:
        raise SymbolMatchError("sample load/module/UUID differs from original")
    pattern = re.compile(
        rf"(?m)^[ \t+!|:]{{0,256}}\d{{1,10}}[ \t]+\?\?\?[ \t]+\(in {re.escape(module)}\)[ \t]+"
        r"load address (0x[0-9a-fA-F]+) \+ (0x[0-9a-fA-F]+)[ \t]+\[(0x[0-9a-fA-F]+)\][ \t]*$"
    )
    selected_frames = re.findall(
        rf"(?m)^[ \t+!|:]{{0,256}}\d{{1,10}}[ \t]+\?\?\?[ \t]+\(in {re.escape(module)}\)[^\n]*$",
        sample,
    )
    matches = list(pattern.finditer(sample))
    if len(selected_frames) != len(matches):
        raise SymbolMatchError("selected unresolved sample frame is malformed")
    offsets: dict[int, int] = {}
    for match in matches:
        frame_load, offset, address = (int(value, 16) for value in match.groups())
        if (
            frame_load != load
            or offset > U64_MAX - load
            or load + offset != address
            or address > image_end
            or offset > U64_MAX - image.preferred_base
        ):
            raise SymbolMatchError("sample frame address/module relation differs")
        preferred_address = image.preferred_base + offset
        if not image.executable_address(preferred_address):
            raise SymbolMatchError("sample frame falls outside matched executable __TEXT")
        offsets[offset] = offsets.get(offset, 0) + 1
        if len(offsets) > MAX_ADDRESSES:
            raise SymbolMatchError("sample distinct address bound exceeded")
    if not offsets:
        raise SymbolMatchError("sample has no selected unresolved frames")
    return load, offsets


def _atos(companion: Path, base: int, addresses: list[int]) -> list[str]:
    result = subprocess.run(
        ["/usr/bin/atos", "-arch", "arm64", "-o", str(companion), "-l", hex(base), *(hex(a) for a in addresses)],
        capture_output=True,
        check=False,
        timeout=20,
    )
    if result.returncode or len(result.stdout) > 1024 * 1024 or result.stderr:
        raise SymbolMatchError("atos failed or exceeded its bounded output")
    try:
        lines = result.stdout.decode("utf-8", "replace").splitlines()
    except UnicodeError as error:
        raise SymbolMatchError("atos output is not text") from error
    if len(lines) != len(addresses):
        raise SymbolMatchError("atos returned a partial address set")
    return lines


def analyze(
    original: Path,
    expected_sha256: str,
    companion: Path,
    sample: Path,
    module: str,
    *,
    atos: Callable[[Path, int, list[int]], list[str]] = _atos,
) -> dict:
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise SymbolMatchError("expected original SHA-256 is invalid")
    if module != original.name or not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", module):
        raise SymbolMatchError("sample module differs from original basename")
    old_bytes = _bounded_regular(original, MAX_BINARY_BYTES)
    observed_sha = hashlib.sha256(old_bytes).hexdigest()
    if observed_sha != expected_sha256:
        raise SymbolMatchError("original binary SHA-256 differs")
    new_bytes = _bounded_regular(companion, MAX_BINARY_BYTES)
    sample_bytes = _bounded_regular(sample, MAX_SAMPLE_BYTES)
    old = parse_macho(old_bytes)
    new = parse_macho(new_bytes)
    matching_text(old, new)
    try:
        sample_text = sample_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise SymbolMatchError("sample is not UTF-8") from error
    load, offsets = _sample_addresses(sample_text, module, old.uuid, old)
    addresses = [old.preferred_base + offset for offset in sorted(offsets)]
    symbols = []
    if _bounded_regular(companion, MAX_BINARY_BYTES) != new_bytes:
        raise SymbolMatchError("companion changed before atos")
    for start in range(0, len(addresses), 128):
        symbols.extend(atos(companion, old.preferred_base, addresses[start : start + 128]))
    if _bounded_regular(companion, MAX_BINARY_BYTES) != new_bytes:
        raise SymbolMatchError("companion changed during atos")
    if len(symbols) != len(addresses):
        raise SymbolMatchError("symbol resolver returned a partial result")
    rows = []
    for address, symbol in zip(addresses, symbols):
        unresolved = not symbol.strip() or bool(
            re.match(r"^(?:0x[0-9a-fA-F]+(?:\s|$)|\?\?\?(?:\s|$))", symbol.strip())
        )
        rows.append(
            {
                "sample_offset": hex(address - old.preferred_base),
                "sample_address": hex(load + address - old.preferred_base),
                "preferred_address": hex(address),
                "sample_frame_lines": offsets[address - old.preferred_base],
                "resolution": "unresolved" if unresolved else "atos_candidate",
                "atos": symbol if not unresolved else None,
            }
        )
    return {
        "status": "LAYOUT_MATCHED_DIAGNOSTIC",
        "symbol_name_provenance": "UNVERIFIED",
        "original_sha256": observed_sha,
        "companion_sha256": hashlib.sha256(new_bytes).hexdigest(),
        "original_uuid": old.uuid,
        "companion_uuid": new.uuid,
        "sample_load_address": hex(load),
        "preferred_base": hex(old.preferred_base),
        "module": module,
        "distinct_offsets": len(rows),
        "atos_candidate_offsets": sum(row["resolution"] == "atos_candidate" for row in rows),
        "unresolved_offsets": sum(row["resolution"] == "unresolved" for row in rows),
        "note": "Code-address layout is matched; atos names are unverified candidates. Frame-line counts are not CPU samples or request percentages.",
        "addresses": rows,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--original", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--companion", type=Path, required=True)
    parser.add_argument("--sample", type=Path, required=True)
    parser.add_argument("--module", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        result = analyze(
            args.original, args.expected_sha256, args.companion, args.sample, args.module
        )
        if args.out.exists() or args.out.is_symlink():
            raise SymbolMatchError("output already exists")
        args.out.write_text(json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n")
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"native stack symbols refused: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
