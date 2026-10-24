#!/usr/bin/env python3
"""Normalize historical evidence views and restore their exact original bytes.

This file is both the history-rewrite library and the published, standalone
restoration utility. Recipes contain original identities, never new captures.
"""

from __future__ import annotations

import argparse
import copy
from contextvars import ContextVar
import hashlib
import io
import json
import lzma
import os
from pathlib import Path, PurePosixPath
import re
import struct
import subprocess
import tarfile
import tempfile
import zlib


ORIGINAL_TOKENS = tuple(bytes.fromhex(value) for value in (
    "72656c6172", "52656c6172", "52454c4152",
))
TARGET_TOKENS = (b"imre", b"Imre", b"IMRE")
OLD_PATTERN = re.compile(b"|".join(map(re.escape, ORIGINAL_TOKENS)))
ANY_OLD_PATTERN = re.compile(
    re.escape(ORIGINAL_TOKENS[0]), re.IGNORECASE,
)
RESULTS = "benchmarks/results/"
MANIFEST_PATH = RESULTS + "naming-provenance.json"
SCRIPT_PATH = RESULTS + "restore-evidence.py"
README_PATH = RESULTS + "NAMING_PROVENANCE.md"
PROVENANCE_PATHS = {MANIFEST_PATH, SCRIPT_PATH, README_PATH}
NOTE = (
    b"> Project naming is normalized to Imre; these are historical measurements. "
    b"See [capture provenance and exact restoration](NAMING_PROVENANCE.md).\n\n"
)
SELF_BYTES = Path(__file__).read_bytes()
SELF_SHA256 = hashlib.sha256(SELF_BYTES).hexdigest()
CACHE = Path(os.environ.get("IMRE_EVIDENCE_CACHE", "/tmp/imre-evidence-cache")) / SELF_SHA256
MAX_DEPTH = 32
SHARD_BYTES = 8 * 1024 * 1024
SHARD_THRESHOLD = 64 * 1024
SUPPORT_ROOT = ContextVar("support_root", default=None)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode()


def _encoded_tokens(encoding: str) -> tuple[tuple[bytes, ...], tuple[bytes, ...]]:
    if encoding == "bytes":
        return ORIGINAL_TOKENS, TARGET_TOKENS
    if encoding not in {"utf-16-le", "utf-16-be", "utf-32-le", "utf-32-be"}:
        raise ValueError("Unsupported text encoding")
    return tuple(x.decode().encode(encoding) for x in ORIGINAL_TOKENS), tuple(
        x.decode().encode(encoding) for x in TARGET_TOKENS
    )


def _encoding(data: bytes) -> str:
    for prefix, name in (
        (b"\xff\xfe\x00\x00", "utf-32-le"), (b"\x00\x00\xfe\xff", "utf-32-be"),
        (b"\xff\xfe", "utf-16-le"), (b"\xfe\xff", "utf-16-be"),
    ):
        if data.startswith(prefix):
            return name
    return "bytes"


def _varint(value: int) -> bytes:
    if value < 0:
        raise ValueError("Negative variable-length integer")
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def _read_varints(data: bytes) -> list[int]:
    result = []
    value = shift = 0
    for byte in data:
        value |= (byte & 127) << shift
        if byte & 128:
            shift += 7
            if shift > 63:
                raise ValueError("Oversized variable-length integer")
        else:
            if shift and byte == 0:
                raise ValueError("Noncanonical variable-length integer")
            result.append(value)
            value = shift = 0
    if shift:
        raise ValueError("Truncated variable-length integer")
    return result


def _hex_value(value: str | dict) -> str:
    if isinstance(value, str):
        return value
    root = SUPPORT_ROOT.get()
    if root is None or value.get("encoding") != "hex-shards":
        raise ValueError("Encoded recipe shards require an evidence root")
    pieces = []
    for shard in value["shards"]:
        path = _safe_path(shard["path"])
        if not str(path).startswith(RESULTS + "naming-data/"):
            raise ValueError("Invalid recipe shard path")
        data = (root / path).read_bytes()
        if len(data) != shard["bytes"] or digest(data) != shard["sha256"]:
            raise ValueError("Recipe shard identity mismatch")
        pieces.append(data)
    encoded = b"".join(pieces)
    if len(encoded) != value["hex_bytes"]:
        raise ValueError("Recipe shard total size mismatch")
    return encoded.decode("ascii")


def _unpack_hex(value: str | dict, expected: int | None = None) -> bytes:
    compressed = bytes.fromhex(_hex_value(value))
    decoder = zlib.decompressobj()
    data = decoder.decompress(compressed, expected + 1 if expected is not None else 0)
    if expected is not None and len(data) > expected:
        raise ValueError("Compressed recipe exceeds declared size")
    data += decoder.flush()
    if not decoder.eof or decoder.unused_data or decoder.unconsumed_tail:
        raise ValueError("Invalid compressed recipe")
    if expected is not None and len(data) != expected:
        raise ValueError("Compressed recipe size mismatch")
    return data


def _rename(data: bytes, encoding: str = "bytes") -> tuple[bytes, dict]:
    originals, targets = _encoded_tokens(encoding)
    width = 4 if encoding.startswith("utf-32") else 2 if encoding.startswith("utf-16") else 1
    pattern = re.compile(b"|".join(map(re.escape, originals)))
    insensitive = re.compile(
        re.escape(originals[0]), re.IGNORECASE,
    )
    if any(match.start() % width == 0 and match.group() not in originals for match in insensitive.finditer(data)):
        raise ValueError("Unsupported project-name capitalization")
    pieces = []
    positions = bytearray()
    source_cursor = normalized_cursor = previous = count = 0
    for match in pattern.finditer(data):
        if match.start() % width:
            continue
        prefix = data[source_cursor:match.start()]
        pieces.append(prefix)
        normalized_cursor += len(prefix)
        code = originals.index(match.group())
        positions.extend(_varint(normalized_cursor - previous))
        positions.extend(_varint(code))
        pieces.append(targets[code])
        previous = normalized_cursor
        normalized_cursor += len(targets[code])
        source_cursor = match.end()
        count += 1
    pieces.append(data[source_cursor:])
    return b"".join(pieces), {
        "kind": "text", "encoding": encoding, "replacement_count": count,
        "positions_zlib_hex": zlib.compress(bytes(positions), 9).hex(),
    }


def _undo_text(data: bytes, rule: dict) -> bytes:
    if rule.get("markdown_note"):
        if not data.startswith(NOTE):
            raise ValueError("Missing historical-view note")
        data = data[len(NOTE):]
    originals, targets = _encoded_tokens(rule["encoding"])
    width = 4 if rule["encoding"].startswith("utf-32") else 2 if rule["encoding"].startswith("utf-16") else 1
    values = _read_varints(_unpack_hex(rule["positions_zlib_hex"]))
    if len(values) != 2 * rule["replacement_count"]:
        raise ValueError("Replacement count mismatch")
    pieces = []
    cursor = position = 0
    for delta, code in zip(values[::2], values[1::2]):
        position += delta
        if code >= len(originals) or position < cursor or position % width:
            raise ValueError("Invalid replacement position or token")
        target = targets[code]
        if data[position:position + len(target)] != target:
            raise ValueError("Replacement token mismatch")
        pieces.extend((data[cursor:position], originals[code]))
        cursor = position + len(target)
    pieces.append(data[cursor:])
    return b"".join(pieces)


def renamed_path(path: str) -> str:
    return _rename(path.encode("utf-8", "surrogateescape"))[0].decode("utf-8", "surrogateescape")


def _identity(raw: bytes, normalized: bytes, rule: dict) -> dict:
    return {
        "original_sha256": digest(raw), "original_bytes": len(raw),
        "normalized_sha256": digest(normalized), "normalized_bytes": len(normalized),
        "restoration": rule,
    }


def _check_identity(data: bytes, wrapper: dict, which: str) -> None:
    if len(data) != wrapper[which + "_bytes"] or digest(data) != wrapper[which + "_sha256"]:
        raise ValueError(which.capitalize() + " content identity mismatch")


def _header_valid(block: bytes) -> bool:
    if len(block) != 512 or block == bytes(512):
        return False
    try:
        wanted = tarfile.nti(block[148:156])
    except (ValueError, tarfile.InvalidHeaderError):
        return False
    unsigned, signed = tarfile.calc_chksums(block)
    return wanted in (unsigned, signed)


def _kind(path: str, data: bytes) -> str:
    suffix = path.lower()
    if data.startswith(b"\x1f\x8b") or suffix.endswith((".gz", ".tgz")):
        return "gzip"
    if data.startswith(b"\xfd7zXZ\x00") or suffix.endswith(".xz"):
        return "xz"
    if suffix.endswith(".tar") or data[257:262] == b"ustar" or _header_valid(data[:512]):
        return "tar"
    if data.startswith((b"PK\x03\x04", b"PK\x05\x06", b"BZh", b"\x28\xb5\x2f\xfd")) or suffix.endswith((".zip", ".bz2", ".zst")):
        raise ValueError("Unsupported archive encoding: " + path)
    if data.startswith((b"# v2 git bundle\n", b"# v3 git bundle\n", b"\x7fELF", b"MZ", b"!<arch>\n", b"BC\xc0\xde", b"\xca\xfe\xba\xbe", b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf")):
        return "opaque"
    return "text"


def _tar_members(data: bytes) -> tuple[tarfile.TarFile, list[tarfile.TarInfo]]:
    archive = tarfile.open(fileobj=io.BytesIO(data), mode="r:", encoding="utf-8", errors="surrogateescape")
    members = archive.getmembers()
    previous_end = 0
    supported = {tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.CONTTYPE, tarfile.DIRTYPE,
                 tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.CHRTYPE, tarfile.BLKTYPE, tarfile.FIFOTYPE}
    for member in members:
        if member.type not in supported or member.issparse() or any(
            key.startswith("GNU.sparse.") for key in member.pax_headers
        ):
            raise ValueError("Unsupported or sparse tar member")
        if member.size < 0 or member.offset_data < 512 or member.offset_data + member.size > len(data):
            raise ValueError("Tar payload lies outside archive")
        if member.offset_data - 512 < previous_end or not _header_valid(data[member.offset_data - 512:member.offset_data]):
            raise ValueError("Overlapping or unsupported tar member intervals")
        if not member.isreg() and member.size:
            raise ValueError("Nonregular tar member has a payload")
        previous_end = member.offset_data + member.size
    return archive, members


def _normalize_tar(path: str, data: bytes, depth: int) -> tuple[bytes, dict]:
    source, members = _tar_members(data)
    output = io.BytesIO()
    records = []
    metadata = bytearray()
    layout = []
    cursor = 0
    with source, tarfile.open(fileobj=output, mode="w", format=tarfile.PAX_FORMAT,
                             encoding="utf-8", errors="surrogateescape") as target:
        for ordinal, member in enumerate(members):
            replacement = copy.copy(member)
            replacement.name = renamed_path(member.name)
            replacement.linkname = renamed_path(member.linkname)
            replacement.uname = renamed_path(member.uname)
            replacement.gname = renamed_path(member.gname)
            replacement.pax_headers = {}
            for key, value in member.pax_headers.items():
                new_key = renamed_path(key)
                if new_key in replacement.pax_headers:
                    raise ValueError("Renamed PAX keys collide")
                replacement.pax_headers[new_key] = renamed_path(value)
            payload_record = None
            normalized_body = None
            if member.isreg():
                start, end = member.offset_data, member.offset_data + member.size
                body = data[start:end]
                extracted = source.extractfile(member)
                if extracted is None or extracted.read() != body:
                    raise ValueError("Tar logical payload differs from its physical interval")
                normalized_body, payload_record = _normalize_node(member.name, body, depth + 1)
                replacement.size = len(normalized_body)
                gap = data[cursor:start]
                if gap:
                    layout.append([0, len(gap)])
                    metadata.extend(gap)
                layout.append([1, ordinal])
                cursor = end
            for key, value in (("path", replacement.name), ("linkpath", replacement.linkname), ("size", str(replacement.size))):
                if key in replacement.pax_headers:
                    replacement.pax_headers[key] = value
            target.addfile(replacement, io.BytesIO(normalized_body) if normalized_body is not None else None)
            records.append({"ordinal": ordinal, "type_hex": member.type.hex(),
                            "normalized_name": replacement.name, "payload": payload_record})
    tail = data[cursor:]
    if tail:
        layout.append([0, len(tail)])
        metadata.extend(tail)
    normalized = output.getvalue()
    check, actual = _tar_members(normalized)
    with check:
        if len(actual) != len(records):
            raise ValueError("Tar rebuild changed logical member count")
        names = {}
        for record, member, original in zip(records, actual, members):
            if member.type.hex() != record["type_hex"]:
                raise ValueError("Tar rebuild changed logical member type")
            previous = names.setdefault(member.name, original.name)
            if previous != original.name:
                raise ValueError("Distinct tar member names collide after normalization")
            record["normalized_name"] = member.name
    return normalized, {
        "kind": "tar", "members": records, "layout": layout,
        "metadata_bytes": len(metadata), "metadata_zlib_hex": zlib.compress(bytes(metadata), 9).hex(),
    }


def _restore_tar(data: bytes, rule: dict, depth: int) -> bytes:
    archive, members = _tar_members(data)
    records = rule["members"]
    if len(members) != len(records):
        raise ValueError("Tar member count mismatch")
    restored = {}
    with archive:
        for ordinal, (member, record) in enumerate(zip(members, records)):
            if record["ordinal"] != ordinal or member.type.hex() != record["type_hex"] or member.name != record["normalized_name"]:
                raise ValueError("Tar member identity mismatch")
            if member.isreg():
                if record["payload"] is None:
                    raise ValueError("Missing tar payload recipe")
                restored[ordinal] = _restore_node(data[member.offset_data:member.offset_data + member.size], record["payload"], depth + 1)
            elif record["payload"] is not None:
                raise ValueError("Unexpected tar payload recipe")
    metadata = _unpack_hex(rule["metadata_zlib_hex"], rule["metadata_bytes"])
    cursor = 0
    used = set()
    output = []
    for entry in rule["layout"]:
        if len(entry) != 2 or not all(type(x) is int and x >= 0 for x in entry):
            raise ValueError("Invalid tar layout entry")
        kind, value = entry
        if kind == 0:
            end = cursor + value
            if end > len(metadata):
                raise ValueError("Tar layout consumes excess metadata")
            output.append(metadata[cursor:end])
            cursor = end
        elif kind == 1 and value in restored and value not in used:
            output.append(restored[value])
            used.add(value)
        else:
            raise ValueError("Invalid or duplicate tar payload reference")
    if cursor != len(metadata) or used != restored.keys():
        raise ValueError("Tar layout leaves unconsumed bytes or payloads")
    return b"".join(output)


def _gzip_parts(data: bytes) -> tuple[bytes, bytes, bytes, list[tuple[int, int]]]:
    if len(data) < 18 or data[:3] != b"\x1f\x8b\x08" or data[3] & 0xE0:
        raise ValueError("Invalid gzip header")
    flags, cursor = data[3], 10
    strings = []
    if flags & 4:
        if cursor + 2 > len(data):
            raise ValueError("Truncated gzip extra length")
        length = int.from_bytes(data[cursor:cursor + 2], "little")
        cursor += 2 + length
        if cursor > len(data):
            raise ValueError("Truncated gzip extra data")
        if ANY_OLD_PATTERN.search(data[12:cursor]):
            raise ValueError("Project token in structured gzip extra data requires an explicit rule")
    for flag in (8, 16):
        if flags & flag:
            end = data.find(b"\x00", cursor)
            if end < 0:
                raise ValueError("Unterminated gzip header string")
            strings.append((cursor, end))
            cursor = end + 1
    if flags & 2:
        if cursor + 2 > len(data) or int.from_bytes(data[cursor:cursor + 2], "little") != zlib.crc32(data[:cursor]) & 0xFFFF:
            raise ValueError("Gzip header CRC mismatch")
        cursor += 2
    decoder = zlib.decompressobj(-15)
    payload = decoder.decompress(data[cursor:]) + decoder.flush()
    trailer = decoder.unused_data
    if not decoder.eof or len(trailer) != 8 or decoder.unconsumed_tail:
        raise ValueError("Unsupported concatenated or truncated gzip stream")
    checksum, size = struct.unpack("<II", trailer)
    if checksum != zlib.crc32(payload) or size != len(payload) & 0xFFFFFFFF:
        raise ValueError("Gzip payload checksum or size mismatch")
    return data[:cursor], payload, trailer, strings


def _deflate(data: bytes) -> bytes:
    encoder = zlib.compressobj(9, zlib.DEFLATED, -15, 8, zlib.Z_DEFAULT_STRATEGY)
    return encoder.compress(data) + encoder.flush()


def _normalize_gzip(path: str, data: bytes, depth: int) -> tuple[bytes, dict]:
    header, body, trailer, strings = _gzip_parts(data)
    cursor = 0
    parts = []
    for start, end in strings:
        parts.extend((header[cursor:start], _rename(header[start:end])[0]))
        cursor = end
    parts.append(header[cursor:-2] if header[3] & 2 else header[cursor:])
    new_header = b"".join(parts)
    if header[3] & 2:
        new_header += struct.pack("<H", zlib.crc32(new_header) & 0xFFFF)
    child_path = path[:-4] + ".tar" if path.lower().endswith(".tgz") else path[:-3] if path.lower().endswith(".gz") else path + ".payload"
    normalized_body, payload_record = _normalize_node(child_path, body, depth + 1)
    normalized = new_header + _deflate(normalized_body) + struct.pack("<II", zlib.crc32(normalized_body), len(normalized_body) & 0xFFFFFFFF)
    rule = {"payload": payload_record, "encoder_zlib_version": zlib.ZLIB_VERSION}
    if header + _deflate(body) + trailer == data:
        rule.update({"kind": "gzip-deflate", "header_hex": header.hex(), "trailer_hex": trailer.hex(),
                     "deflate": {"level": 9, "window_bits": -15, "memory_level": 8, "strategy": 0}})
    else:
        rule.update({"kind": "gzip-original", "original_zlib_hex": zlib.compress(data, 9).hex(),
                     "reason": "Original compressed stream differs from the specified encoder"})
    return normalized, rule


def _restore_gzip(data: bytes, rule: dict, depth: int) -> bytes:
    _, body, _, _ = _gzip_parts(data)
    original_body = _restore_node(body, rule["payload"], depth + 1)
    if rule["kind"] == "gzip-deflate":
        if rule["deflate"] != {"level": 9, "window_bits": -15, "memory_level": 8, "strategy": 0}:
            raise ValueError("Unknown gzip encoder recipe")
        original = bytes.fromhex(rule["header_hex"]) + _deflate(original_body) + bytes.fromhex(rule["trailer_hex"])
    else:
        original = _unpack_hex(rule["original_zlib_hex"])
    if _gzip_parts(original)[1] != original_body:
        raise ValueError("Restored gzip stream disagrees with recursive payload")
    return original


def _xz_compress(data: bytes, threaded: bool) -> bytes:
    if threaded:
        # The multiblock captures were produced by XZ Utils with its default block size.
        return subprocess.run(
            ["xz", "--threads=4", "-6", "--stdout"], input=data, stdout=subprocess.PIPE, check=True,
        ).stdout
    return lzma.compress(data, format=lzma.FORMAT_XZ, preset=6)


def _normalize_xz(path: str, data: bytes, depth: int) -> tuple[bytes, dict]:
    body = lzma.decompress(data, format=lzma.FORMAT_XZ)
    normalized, payload = _normalize_node(path.removesuffix(".xz"), body, depth + 1)
    threaded = bool(data[13] & 0xC0)
    rule = {"kind": "xz", "threaded": threaded, "payload": payload}
    if _xz_compress(body, threaded) != data:
        rule["original_zlib_hex"] = zlib.compress(data, 9).hex()
    return _xz_compress(normalized, threaded), rule


def _normalize_node(path: str, raw: bytes, depth: int = 0, markdown: bool = False) -> tuple[bytes, dict]:
    if depth > MAX_DEPTH:
        raise ValueError("Archive nesting limit exceeded")
    kind = _kind(path, raw)
    if kind == "opaque":
        normalized, rule = raw, {"kind": "opaque"}
    elif kind == "xz":
        normalized, rule = _normalize_xz(path, raw, depth)
    elif kind == "tar":
        normalized, rule = _normalize_tar(path, raw, depth)
    elif kind == "gzip":
        normalized, rule = _normalize_gzip(path, raw, depth)
    else:
        encoding = _encoding(raw)
        normalized, rule = _rename(raw, encoding)
        if markdown and encoding == "bytes" and not raw.startswith(b"\xef\xbb\xbf") and not normalized.startswith(NOTE):
            normalized = NOTE + normalized
            rule["markdown_note"] = True
    return normalized, _identity(raw, normalized, rule)


def _restore_node(normalized: bytes, wrapper: dict, depth: int = 0) -> bytes:
    if depth > MAX_DEPTH:
        raise ValueError("Archive nesting limit exceeded")
    _check_identity(normalized, wrapper, "normalized")
    rule = wrapper["restoration"]
    if rule["kind"] == "opaque":
        restored = normalized
    elif rule["kind"] == "xz":
        body = _restore_node(lzma.decompress(normalized), rule["payload"], depth + 1)
        if "original_zlib_hex" in rule:
            restored = _unpack_hex(rule["original_zlib_hex"], wrapper["original_bytes"])
            if lzma.decompress(restored) != body:
                raise ValueError("Restored XZ stream disagrees with recursive payload")
        else:
            restored = _xz_compress(body, rule["threaded"])
    elif rule["kind"] == "text":
        restored = _undo_text(normalized, rule)
    elif rule["kind"] == "tar":
        restored = _restore_tar(normalized, rule, depth)
    elif rule["kind"] in {"gzip-deflate", "gzip-original"}:
        restored = _restore_gzip(normalized, rule, depth)
    else:
        raise ValueError("Unknown restoration rule")
    _check_identity(restored, wrapper, "original")
    return restored


def restore_blob(normalized: bytes, record: dict, support_root: Path | None = None) -> bytes:
    token = SUPPORT_ROOT.set(support_root)
    try:
        return _restore_node(normalized, record)
    finally:
        SUPPORT_ROOT.reset(token)


def _atomic_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(dir=path.parent, prefix=".pending-")
    try:
        with os.fdopen(handle, "wb") as stream:
            stream.write(data)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def normalize_blob(path: str, raw: bytes) -> tuple[bytes, dict]:
    """Return a current-name view and a checked, one-layer exact inverse recipe."""
    markdown = path.lower().endswith(".md")
    mode = _kind(path, raw) + ("-markdown" if markdown else "")
    cache_key = digest(raw) + "-" + mode
    data_path, recipe_path = CACHE / (cache_key + ".blob"), CACHE / (cache_key + ".json")
    if data_path.exists() and recipe_path.exists():
        normalized, record = data_path.read_bytes(), json.loads(recipe_path.read_bytes())
    else:
        normalized, record = _normalize_node(path, raw, markdown=markdown)
        if restore_blob(normalized, record) != raw:
            raise ValueError("Exact inverse verification failed")
        _atomic_write(data_path, normalized)
        _atomic_write(recipe_path, json_bytes(record))
    if restore_blob(normalized, record) != raw:
        raise ValueError("Cached inverse verification failed")
    record = copy.deepcopy(record)
    if renamed_path(path) != path:
        record["original_path_utf8_hex"] = path.encode("utf-8", "surrogateescape").hex()
    return normalized, record


def _safe_path(value: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts) or "\\" in value:
        raise ValueError("Unsafe evidence path")
    return path


def provenance_files(records: dict[str, dict], prior_support: dict[str, bytes] | None = None) -> dict[str, bytes]:
    """Publish current view identities and preserve the previous utility layer."""
    if ANY_OLD_PATTERN.search(SELF_BYTES):
        raise ValueError("Restoration utility contains a previous visible project name")
    for path in records:
        _safe_path(path)
        if not path.startswith(RESULTS) or path in PROVENANCE_PATHS or ANY_OLD_PATTERN.search(path.encode()):
            raise ValueError("Invalid normalized evidence path")
    prior = []
    if prior_support:
        if set(prior_support) != PROVENANCE_PATHS:
            raise ValueError("Prior support must contain exactly the three provenance files")
        old_manifest = json.loads(prior_support[MANIFEST_PATH])
        if old_manifest.get("restoration_script_sha256", old_manifest.get("restoration_utility_sha256")) != digest(prior_support[SCRIPT_PATH]):
            raise ValueError("Prior utility identity mismatch")
        for old_path, old_record in old_manifest["files"].items():
            current = records.get(renamed_path(old_path))
            if current is None or current["original_sha256"] != old_record["normalized_sha256"] or current["original_bytes"] != old_record["normalized_bytes"]:
                raise ValueError("Prior view does not match the next layer's original identity")
        for path, data in sorted(prior_support.items()):
            prior.append({"path_utf8_hex": path.encode().hex(), "sha256": digest(data),
                          "bytes": len(data), "data_zlib_hex": zlib.compress(data, 9).hex()})
    shards = {}

    def externalize(value: object, key: str = "") -> object:
        if isinstance(value, dict):
            return {child_key: externalize(child, child_key) for child_key, child in value.items()}
        if isinstance(value, list):
            return [externalize(child) for child in value]
        if isinstance(value, str) and key.endswith("_zlib_hex") and len(value) > SHARD_THRESHOLD:
            references = []
            for start in range(0, len(value), SHARD_BYTES):
                data = value[start:start + SHARD_BYTES].encode("ascii")
                sha = digest(data)
                path = RESULTS + "naming-data/" + sha + ".hex"
                shards[path] = data
                references.append({"path": path, "sha256": sha, "bytes": len(data)})
            return {"encoding": "hex-shards", "hex_bytes": len(value), "shards": references}
        return value

    manifest = {
        "schema": 2, "project": "Imre", "kind": "historical-evidence-view",
        "policy": "Names are view aliases. Capture hashes, source revisions, timestamps, outputs, and measurements retain their original meaning. Restore one layer to recover exact original files and previous provenance support.",
        "restoration_utility": SCRIPT_PATH, "restoration_utility_sha256": SELF_SHA256,
        "token_code_original_hex": [value.hex() for value in ORIGINAL_TOKENS],
        "token_code_target": [value.decode() for value in TARGET_TOKENS],
        "text_positions": "zlib-compressed unsigned LEB128 pairs: normalized byte offset delta, original token code",
        "gzip_encoder": "Exact deflate recipes require a compatible zlib encoder; other streams retain an encoded original backup.",
        "files": externalize(records), "prior_support": externalize(prior),
        "support_blobs": {path: {"sha256": digest(data), "bytes": len(data)} for path, data in sorted(shards.items())},
    }
    readme = b"""# Historical evidence naming\n\nThese files present historical evidence using the current Imre name. They do not claim new measurements or new captures. Recorded source revisions, hashes, commands, timestamps, outputs, and performance values keep their historical meaning. The manifest separately identifies the current view and the exact original bytes.\n\nVerify every current view and its inverse:\n\n```sh\npython3 benchmarks/results/restore-evidence.py verify\n```\n\nRestore one naming layer into a new directory:\n\n```sh\npython3 benchmarks/results/restore-evidence.py restore /tmp/imre-original-evidence\n```\n\nRestoration also reinstates any preceding provenance manifest, documentation, and utility. Run that restored utility in the restored tree to verify or recover an earlier layer separately. Run capture-specific replay scripts and checksum audits from the restored directory. Their hashes describe the files as they were captured. Original paths and prior support files are encoded in the current manifest.\n\nText recipes record only replaced occurrences, preserving preexisting current-name text. Tar recipes retain original metadata, headers, and padding plus recursively restored logical payloads. Gzip recipes preserve exact original headers and either reproducible compressed payloads or an encoded original stream. XZ streams use the original compression layout or a stored original stream. Multiblock XZ restoration requires the xz command from XZ Utils. Embedded Git bundles retain their object identities. Every layer checks both byte lengths and SHA-256 identities.\n"""
    output = {MANIFEST_PATH: json_bytes(manifest), SCRIPT_PATH: SELF_BYTES, README_PATH: readme, **shards}
    if any(len(data) >= 90 * 1024 * 1024 for data in output.values()):
        raise ValueError("Generated provenance blob exceeds the publication size guard")
    if any(ANY_OLD_PATTERN.search(data) for data in output.values()):
        raise ValueError("Generated provenance contains a previous visible project name")
    return output


def verify_tree(root: Path, destination: Path | None = None) -> dict:
    manifest = json.loads((root / MANIFEST_PATH).read_bytes())
    if manifest.get("schema") != 2 or digest((root / SCRIPT_PATH).read_bytes()) != manifest["restoration_utility_sha256"]:
        raise ValueError("Unsupported manifest or utility identity mismatch")
    for path, identity in manifest["support_blobs"].items():
        _safe_path(path)
        data = (root / path).read_bytes()
        if len(data) != identity["bytes"] or digest(data) != identity["sha256"]:
            raise ValueError("Support blob identity mismatch")
    pending = {}
    for path, record in manifest["files"].items():
        _safe_path(path)
        original_path = bytes.fromhex(record["original_path_utf8_hex"]).decode("utf-8", "surrogateescape") if "original_path_utf8_hex" in record else path
        _safe_path(original_path)
        if original_path in pending:
            raise ValueError("Restored evidence paths collide")
        pending[original_path] = restore_blob((root / path).read_bytes(), record, root)
    for entry in manifest.get("prior_support", []):
        path = bytes.fromhex(entry["path_utf8_hex"]).decode("utf-8", "surrogateescape")
        _safe_path(path)
        if path in pending:
            raise ValueError("Prior support collides with restored evidence")
        token = SUPPORT_ROOT.set(root)
        try:
            data = _unpack_hex(entry["data_zlib_hex"], entry["bytes"])
        finally:
            SUPPORT_ROOT.reset(token)
        if digest(data) != entry["sha256"]:
            raise ValueError("Prior support identity mismatch")
        pending[path] = data
    if destination is not None:
        if destination.exists():
            raise ValueError("Restoration destination must not exist")
        destination.mkdir(parents=True)
        for path, data in pending.items():
            target = destination / path
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open("xb") as stream:
                stream.write(data)
    return {"status": "passed", "files": len(manifest["files"]),
            "prior_support": len(manifest.get("prior_support", [])),
            "original_bytes": sum(len(value) for value in pending.values())}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent.parent)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("verify")
    restore = subparsers.add_parser("restore")
    restore.add_argument("destination", type=Path)
    arguments = parser.parse_args()
    print(json.dumps(verify_tree(arguments.root, arguments.destination if arguments.command == "restore" else None), sort_keys=True))


if __name__ == "__main__":
    main()
