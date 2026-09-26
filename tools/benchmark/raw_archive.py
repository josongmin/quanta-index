"""Deterministic stored ZIPs over the canonical pinned raw-file owner.

Archive mechanics are shared; pair/corpus owners retain inventory semantics.
ZIP metadata is bounded before ZipFile can materialize the central directory.
Partial destinations are diagnostic work, never admissible publication.
"""

from __future__ import annotations

import stat
import struct
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

if __package__:
    from .evidence import IO_CHUNK_BYTES, EvidenceError, RawFile, RawWriter
else:
    from evidence import IO_CHUNK_BYTES, EvidenceError, RawFile, RawWriter


@dataclass(frozen=True)
class ArchiveLimits:
    max_bytes: int
    max_entries: int = 100_000
    max_directory_bytes: int = 16 * 1024 * 1024

    def __post_init__(self):
        if any(
            type(n) is not int or n <= 0
            for n in (self.max_bytes, self.max_entries, self.max_directory_bytes)
        ):
            raise EvidenceError("archive limits must be positive integers")


def canonical_name(name: str) -> None:
    if (
        not isinstance(name, str)
        or not name
        or "\x00" in name
        or "\\" in name
        or PurePosixPath(name).is_absolute()
        or PurePosixPath(name).as_posix() != name
        or any(part in {"", ".", ".."} for part in name.split("/"))
    ):
        raise EvidenceError("archive has unsafe or noncanonical entry name")
    try:
        if len(name.encode("utf-8")) > 65535:
            raise EvidenceError("archive entry name exceeds ZIP format limit")
    except UnicodeError as error:
        raise EvidenceError("archive entry name is not UTF-8") from error


def _inventory(names: list[str], limits: ArchiveLimits) -> None:
    if not names or len(names) > limits.max_entries:
        raise EvidenceError("archive entry count exceeds limit or is empty")
    if names != sorted(set(names)):
        raise EvidenceError("archive inventory is duplicate or reordered")
    members = set(names)
    for name in names:
        canonical_name(name)
        if any(str(parent) in members for parent in PurePosixPath(name).parents):
            raise EvidenceError("archive file/directory aliases overlap")


class _LimitedSink:
    def __init__(self, raw: RawWriter, limit: int):
        self.raw, self.limit = raw, limit

    def tell(self):
        return self.raw.tell()

    def write(self, block):
        if self.tell() + len(block) > self.limit:
            raise EvidenceError("archive byte limit exceeded")
        return self.raw.write(block)

    def flush(self):
        self.raw.flush()


def pack(files: dict[str, RawFile], target: Path, *, limits: ArchiveLimits) -> RawFile:
    names = sorted(files)
    _inventory(names, limits)
    if any(not isinstance(files[name], RawFile) for name in names):
        raise EvidenceError("archive needs file-backed entries")
    if sum(files[name].size for name in names) > limits.max_bytes:
        raise EvidenceError("archive payload byte limit exceeded")
    # Central header plus worst-case per-entry ZIP64 size/offset fields.
    if sum(46 + len(name.encode("utf-8")) + 32 for name in names) > limits.max_directory_bytes:
        raise EvidenceError("archive central directory limit exceeded")
    try:
        with RawWriter(target) as raw:
            with zipfile.ZipFile(
                _LimitedSink(raw, limits.max_bytes), "w", compression=zipfile.ZIP_STORED
            ) as archive:
                for name in names:
                    entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                    entry.create_system = 3
                    entry.external_attr = (stat.S_IFREG | 0o600) << 16
                    entry.file_size = files[name].size
                    with archive.open(entry, "w") as sink:
                        files[name].copy_into(sink)
            return raw.finish()
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as error:
        raise EvidenceError(f"archive creation failed: {error}") from error


def _directory_size(handle, size: int, limits: ArchiveLimits) -> tuple[int, int]:
    """Bound ZIP metadata before constructing ZipFile, including ZIP64 counts."""
    tail_size = min(size, 65535 + 22)
    handle.seek(size - tail_size)
    tail = handle.read(tail_size)
    at = tail.rfind(b"PK\x05\x06")
    if at < 0 or at + 22 > len(tail):
        raise EvidenceError("archive end record is missing or truncated")
    _, disk, directory_disk, on_disk, count, length, offset, comment = struct.unpack(
        "<4s4H2IH", tail[at : at + 22]
    )
    if at + 22 + comment != len(tail) or disk or directory_disk or on_disk != count:
        raise EvidenceError("archive end record is inconsistent or multi-disk")
    end = size - tail_size + at
    # ZIP64 can be present even when the classic counts did not overflow.
    if end >= 20:
        handle.seek(end - 20)
        locator = handle.read(20)
    else:
        locator = b""
    if locator[:4] == b"PK\x06\x07":
        _, zip_disk, zip_offset, disks = struct.unpack("<4sIQI", locator)
        if zip_disk or disks != 1 or zip_offset + 56 != end - 20:
            raise EvidenceError("archive ZIP64 locator is malformed")
        handle.seek(zip_offset)
        header = handle.read(56)
        if len(header) != 56:
            raise EvidenceError("archive ZIP64 record is truncated")
        (
            signature,
            record_size,
            _made,
            _needed,
            disk,
            directory_disk,
            on_disk,
            count,
            length,
            offset,
        ) = struct.unpack("<4sQ2H2I4Q", header)
        if (
            signature != b"PK\x06\x06"
            or record_size != 44
            or disk
            or directory_disk
            or on_disk != count
        ):
            raise EvidenceError("archive ZIP64 record is malformed or multi-disk")
        end = zip_offset
    elif length == 0xFFFFFFFF or offset == 0xFFFFFFFF:
        raise EvidenceError("archive ZIP64 record is missing")
    if (
        not 0 < count <= limits.max_entries
        or length > limits.max_directory_bytes
        or length < count * 46
        or offset + length != end
    ):
        raise EvidenceError("archive central directory exceeds limits or has invalid bounds")
    # Do not trust the declared count: ZipFile otherwise materializes every
    # central row before we could check a forged small count against its list.
    cursor, actual = offset, 0
    while cursor < end:
        handle.seek(cursor)
        header = handle.read(46)
        if len(header) != 46 or header[:4] != b"PK\x01\x02":
            raise EvidenceError("archive central directory row is malformed")
        name_bytes, extra_bytes, comment_bytes = struct.unpack_from("<3H", header, 28)
        cursor += 46 + name_bytes + extra_bytes + comment_bytes
        actual += 1
        if cursor > end or actual > count:
            raise EvidenceError("archive central directory count or bounds differ")
    if actual != count:
        raise EvidenceError("archive central directory is incomplete")
    return count, length


def unpack(raw: RawFile, destination: Path, *, limits: ArchiveLimits, admit_names=None) -> None:
    if not isinstance(raw, RawFile) or not 0 < raw.size <= limits.max_bytes:
        raise EvidenceError("archive is empty or exceeds byte limit")

    def consume(handle):
        expected_count, _ = _directory_size(handle, raw.size, limits)
        with zipfile.ZipFile(handle) as archive:
            entries = archive.infolist()
            names = [entry.filename for entry in entries]
            _inventory(names, limits)
            if len(entries) != expected_count:
                raise EvidenceError("archive entry count differs from end record")
            if admit_names is not None:
                admit_names(names)
            total = 0
            for entry in entries:
                total += entry.file_size
                if (
                    entry.is_dir()
                    or entry.orig_filename != entry.filename
                    or entry.volume != 0
                    or entry.compress_type != zipfile.ZIP_STORED
                    or entry.flag_bits & 1
                    or entry.file_size != entry.compress_size
                    or entry.file_size > raw.size
                    or total > limits.max_bytes
                    or (entry.external_attr >> 16) & 0o170000 not in {0, stat.S_IFREG}
                ):
                    raise EvidenceError("archive has unsafe, linked, compressed or oversized entry")
            # Validate the complete declared inventory before creating output.
            for entry in entries:
                with archive.open(entry) as source, RawWriter(destination / entry.filename) as sink:
                    count = 0
                    while block := source.read(IO_CHUNK_BYTES):
                        count += len(block)
                        if count > entry.file_size:
                            raise EvidenceError("archive entry exceeds declared size")
                        sink.write(block)
                    if count != entry.file_size:
                        raise EvidenceError("archive entry is incomplete")
                    sink.finish()

    try:
        raw.consume_seekable(consume)
    except (
        OSError,
        ValueError,
        RuntimeError,
        NotImplementedError,
        zipfile.BadZipFile,
        zipfile.LargeZipFile,
        EOFError,
    ) as error:
        raise EvidenceError(f"archive is malformed, unsafe or incomplete: {error}") from error
