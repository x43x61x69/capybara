#!/usr/bin/env python3
"""Package a privately exported Sonix ROM for the H717 flash partition.

Pure Python; no firmware, key or game asset is bundled with this tool.
The matching Rust reader lives in ../core/src/flash.rs.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import struct
import sys
import zlib

MAGIC = b"H717ROM1"
VERSION = 1
PAGE_SIZE = 4096
PAGE_COUNT = 4096
ROM_SIZE = PAGE_SIZE * PAGE_COUNT
PRAM_SIZE = 64 * 1024
MAILBOX_SIZE = 4 * 1024
HEADER_SIZE = 64
ENTRY_SIZE = 12
MAX_PACKAGE_SIZE = 12 * 1024 * 1024
PRAM_OFFSET = HEADER_SIZE
MAILBOX_OFFSET = PRAM_OFFSET + PRAM_SIZE
INDEX_OFFSET = MAILBOX_OFFSET + MAILBOX_SIZE
DATA_OFFSET = INDEX_OFFSET + PAGE_COUNT * ENTRY_SIZE


class PackageError(ValueError):
    """Input is malformed or cannot fit the board's partition."""


def crc32(data: bytes | bytearray) -> int:
    return zlib.crc32(data) & 0xFFFFFFFF


def _extended_length(output: bytearray, length: int) -> None:
    while length >= 255:
        output.append(255)
        length -= 255
    output.append(length)


def lz4_compress(data: bytes) -> bytes:
    """Encode one raw LZ4 block, retaining standard last-literal constraints."""
    output = bytearray()
    previous: dict[bytes, int] = {}
    literal_start = cursor = 0
    # LZ4's last match starts at least 12 bytes before the end; leave 5 literals.
    while cursor <= len(data) - 12:
        key = data[cursor : cursor + 4]
        candidate = previous.get(key)
        previous[key] = cursor
        if candidate is None or cursor - candidate > 65535:
            cursor += 1
            continue
        match_size = 4
        while (
            cursor + match_size < len(data) - 5
            and data[candidate + match_size] == data[cursor + match_size]
        ):
            match_size += 1
        literals = cursor - literal_start
        encoded_match = match_size - 4
        output.append((min(literals, 15) << 4) | min(encoded_match, 15))
        if literals >= 15:
            _extended_length(output, literals - 15)
        output.extend(data[literal_start:cursor])
        output.extend(struct.pack("<H", cursor - candidate))
        if encoded_match >= 15:
            _extended_length(output, encoded_match - 15)
        end = cursor + match_size
        # Seed positions inside the match to find subsequent adjacent runs.
        for position in range(cursor + 1, end):
            previous[data[position : position + 4]] = position
        cursor = literal_start = end
    literals = len(data) - literal_start
    output.append(min(literals, 15) << 4)
    if literals >= 15:
        _extended_length(output, literals - 15)
    output.extend(data[literal_start:])
    return bytes(output)


def lz4_decompress(data: bytes, expected_size: int = PAGE_SIZE) -> bytes:
    """Decode with explicit input/output limits, including overlapping matches."""
    output = bytearray()
    cursor = 0

    def length_with_extension(length: int) -> int:
        nonlocal cursor
        if length == 15:
            while True:
                if cursor >= len(data):
                    raise PackageError("truncated LZ4 length")
                extra = data[cursor]
                cursor += 1
                length += extra
                if length > expected_size:
                    raise PackageError("LZ4 output exceeds page size")
                if extra != 255:
                    break
        return length

    while cursor < len(data):
        token = data[cursor]
        cursor += 1
        literal_size = length_with_extension(token >> 4)
        if cursor + literal_size > len(data):
            raise PackageError("truncated LZ4 literals")
        if len(output) + literal_size > expected_size:
            raise PackageError("LZ4 output exceeds page size")
        output.extend(data[cursor : cursor + literal_size])
        cursor += literal_size
        if cursor == len(data):
            break
        if cursor + 2 > len(data):
            raise PackageError("truncated LZ4 distance")
        distance = struct.unpack_from("<H", data, cursor)[0]
        cursor += 2
        if distance == 0 or distance > len(output):
            raise PackageError("invalid LZ4 distance")
        match_size = length_with_extension(token & 15) + 4
        if len(output) + match_size > expected_size:
            raise PackageError("LZ4 output exceeds page size")
        for _ in range(match_size):
            output.append(output[-distance])
    if len(output) != expected_size:
        raise PackageError("LZ4 page has incorrect decoded size")
    return bytes(output)


def _u32(value: object, name: str) -> int:
    if type(value) is not int or not 0 <= value <= 0xFFFFFFFF:
        raise PackageError(f"metadata {name} must be an unsigned 32-bit integer")
    return value


def build_package(flash: bytes, pram: bytes, mailbox: bytes, metadata: dict) -> bytes:
    if len(flash) != ROM_SIZE:
        raise PackageError(f"flash.bin must contain exactly {ROM_SIZE} bytes")
    if len(pram) != PRAM_SIZE:
        raise PackageError(f"pram.bin must contain exactly {PRAM_SIZE} bytes")
    if len(mailbox) != MAILBOX_SIZE:
        raise PackageError(f"mailbox.bin must contain exactly {MAILBOX_SIZE} bytes")
    raw_key = metadata.get("device_key")
    key = _u32(0 if raw_key is None else raw_key, "device_key")
    xip_base = _u32(metadata.get("xip_base"), "xip_base")
    index = bytearray()
    payload = bytearray()
    previous: dict[bytes, tuple[int, int]] = {}
    for page_number in range(PAGE_COUNT):
        page = flash[page_number * PAGE_SIZE : (page_number + 1) * PAGE_SIZE]
        entry = previous.get(page)
        if entry is None:
            encoded = lz4_compress(page)
            if len(encoded) >= PAGE_SIZE:
                encoded = page
            entry = (DATA_OFFSET + len(payload), len(encoded))
            payload.extend(encoded)
            previous[page] = entry
        index.extend(struct.pack("<III", *entry, crc32(page)))
    total = DATA_OFFSET + len(payload)
    if total > MAX_PACKAGE_SIZE:
        raise PackageError(
            f"ROM package needs {total:,} bytes; the H717 partition holds "
            f"{MAX_PACKAGE_SIZE:,}. No ROM data was truncated."
        )
    header = struct.pack(
        "<8s11I",
        MAGIC,
        VERSION,
        total,
        crc32(flash),
        key,
        xip_base,
        PAGE_SIZE,
        PAGE_COUNT,
        PRAM_OFFSET,
        MAILBOX_OFFSET,
        INDEX_OFFSET,
        DATA_OFFSET,
    )
    header += struct.pack("<III", crc32(header), crc32(pram), crc32(mailbox))
    return header + pram + mailbox + index + payload


def verify_package(package: bytes) -> dict[str, int]:
    if len(package) < DATA_OFFSET or len(package) > MAX_PACKAGE_SIZE:
        raise PackageError("invalid package size")
    fields = struct.unpack_from("<8s14I", package)
    magic, version, total, rom_crc, key, xip_base, page_size, count, pram, mailbox, index, data, header_crc, pram_crc, mailbox_crc = fields
    if magic != MAGIC or version != VERSION:
        raise PackageError("invalid package magic or version")
    if total != len(package) or header_crc != crc32(package[:52]):
        raise PackageError("invalid package size or header checksum")
    if (page_size, count, pram, mailbox, index, data) != (
        PAGE_SIZE, PAGE_COUNT, PRAM_OFFSET, MAILBOX_OFFSET, INDEX_OFFSET, DATA_OFFSET
    ):
        raise PackageError("invalid package geometry")
    if crc32(package[pram:mailbox]) != pram_crc:
        raise PackageError("PRAM checksum mismatch")
    if crc32(package[mailbox:index]) != mailbox_crc:
        raise PackageError("mailbox checksum mismatch")
    running_crc = 0
    cache: dict[tuple[int, int], bytes] = {}
    for number in range(count):
        offset, length, page_crc = struct.unpack_from("<III", package, index + number * ENTRY_SIZE)
        if not 0 < length <= PAGE_SIZE or offset < data or offset + length > total:
            raise PackageError(f"page {number}: invalid payload range")
        page = cache.get((offset, length))
        if page is None:
            encoded = package[offset : offset + length]
            page = encoded if length == PAGE_SIZE else lz4_decompress(encoded)
            cache[(offset, length)] = page
        if crc32(page) != page_crc:
            raise PackageError(f"page {number}: checksum mismatch")
        running_crc = zlib.crc32(page, running_crc)
    if running_crc & 0xFFFFFFFF != rom_crc:
        raise PackageError("ROM checksum mismatch")
    return {"bytes": total, "rom_crc32": rom_crc, "unique_pages": len(cache), "xip_base": xip_base}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("prepared", type=Path, help="private directory produced by h717_export")
    parser.add_argument("output", type=Path, help="private H717ROM1 output image")
    parser.add_argument("--verify", action="store_true", help="decode all pages and verify CRCs after packing")
    args = parser.parse_args(argv)
    try:
        metadata = json.loads((args.prepared / "metadata.json").read_text())
        if not isinstance(metadata, dict):
            raise PackageError("metadata.json must be an object")
        package = build_package(
            (args.prepared / "flash.bin").read_bytes(),
            (args.prepared / "pram.bin").read_bytes(),
            (args.prepared / "mailbox.bin").read_bytes(),
            metadata,
        )
        if args.verify:
            verify_package(package)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        # Write only after validation succeeds; retain any existing file on error.
        temporary = args.output.with_suffix(args.output.suffix + ".tmp")
        temporary.write_bytes(package)
        temporary.replace(args.output)
        print(f"Packed {ROM_SIZE:,} ROM bytes into {len(package):,} bytes ({len(package) / ROM_SIZE:.1%}).")
        print(f"Partition budget: {MAX_PACKAGE_SIZE:,} bytes; free: {MAX_PACKAGE_SIZE - len(package):,} bytes.")
        if args.verify:
            print("Verified every decoded page and the complete ROM checksum.")
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"pack_rom: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
