#!/usr/bin/env python3
"""
SmartPack Ubuntu v2
===================
A dependency-free (Python standard library only) lossless archiver designed for Ubuntu.

Key properties
--------------
- No 7-Zip, WinRAR, zstd CLI, pip packages, or network services.
- Uses Python stdlib codecs: Zstandard when Python >= 3.14 provides compression.zstd,
  plus LZMA/XZ, zlib, and bz2 as fallbacks/options.
- Content-aware compression selection.
- Fixed-block deduplication across files.
- Solid packing of small similar files to expose cross-file redundancy.
- Sparse/zero-block optimization.
- Preserves directories, regular files, symlinks, hardlinks, modes and mtimes.
- Per-block SHA-256 integrity verification.
- Safe extraction that rejects path traversal and unsafe symlinks by default.

This is a custom .spk archive format. It is intended to be read by this script.
No lossless compressor can guarantee smaller output than RAR/7z on every dataset.

Recommended runtime:
- Ubuntu 26.04 + Python 3.14: gets stdlib Zstandard + LZMA.
- Older Python: still works, using LZMA/zlib/bz2 only.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import bz2
import hashlib
import json
import lzma
import os
import shutil
import stat
import struct
import sys
import tempfile
import time
import unicodedata
import zlib
from collections import OrderedDict, defaultdict, deque
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import BinaryIO, Iterable

try:
    from compression import zstd as _zstd  # Python 3.14+
except (ImportError, ModuleNotFoundError):
    _zstd = None

VERSION = "2.0.0"
MAGIC = b"SPK2\r\n\x1a\n"
BLOCK_TAG = b"B"
MANIFEST_TAG = b"M"
END_TAG = b"E"

# B + sha256 + method + raw_size + stored_size
BLOCK_META = struct.Struct("<32sBQQ")
# M + raw_manifest_size + stored_manifest_size + sha256(raw_manifest)
MANIFEST_META = struct.Struct("<QQ32s")

METHOD_RAW = 0
METHOD_ZLIB = 1
METHOD_BZ2 = 2
METHOD_LZMA = 3
METHOD_ZSTD = 4
METHOD_ZERO = 5
METHOD_NAMES = {
    METHOD_RAW: "raw",
    METHOD_ZLIB: "zlib",
    METHOD_BZ2: "bz2",
    METHOD_LZMA: "lzma",
    METHOD_ZSTD: "zstd",
    METHOD_ZERO: "zero",
}

# Extensions are hints only. Content sampling still decides whether compression is useful.
ALREADY_COMPRESSED = {
    ".7z", ".rar", ".zip", ".gz", ".bz2", ".xz", ".zst", ".lz4", ".cab",
    ".jpg", ".jpeg", ".webp", ".avif", ".heic", ".heif", ".jxl",
    ".mp3", ".aac", ".m4a", ".ogg", ".opus", ".flac",
    ".mp4", ".m4v", ".mkv", ".mov", ".webm", ".avi",
    ".apk", ".aab", ".ipa", ".jar", ".war", ".whl",
}
TEXT_LIKE = {
    ".txt", ".log", ".csv", ".tsv", ".json", ".jsonl", ".xml", ".yaml", ".yml",
    ".html", ".htm", ".css", ".js", ".ts", ".jsx", ".tsx", ".md", ".rst",
    ".sql", ".ini", ".cfg", ".conf", ".toml", ".py", ".java", ".c", ".cc",
    ".cpp", ".h", ".hpp", ".go", ".rs", ".cs", ".sh", ".bash", ".fish",
}


@dataclass
class BlockInfo:
    offset: int
    method: int
    raw_size: int
    stored_size: int


@dataclass
class Stats:
    input_bytes: int = 0
    unique_raw_bytes: int = 0
    stored_payload_bytes: int = 0
    duplicate_bytes_saved: int = 0
    zero_bytes_saved: int = 0
    blocks: int = 0
    method_raw_bytes: dict[int, int] = field(default_factory=lambda: defaultdict(int))
    method_stored_bytes: dict[int, int] = field(default_factory=lambda: defaultdict(int))


class LRUCache:
    def __init__(self, max_bytes: int):
        self.max_bytes = max_bytes
        self.current = 0
        self.data: OrderedDict[str, bytes] = OrderedDict()

    def get(self, key: str) -> bytes | None:
        value = self.data.get(key)
        if value is not None:
            self.data.move_to_end(key)
        return value

    def put(self, key: str, value: bytes) -> None:
        if len(value) > self.max_bytes:
            return
        old = self.data.pop(key, None)
        if old is not None:
            self.current -= len(old)
        self.data[key] = value
        self.current += len(value)
        self.data.move_to_end(key)
        while self.current > self.max_bytes and self.data:
            _, removed = self.data.popitem(last=False)
            self.current -= len(removed)


def human_size(n: int) -> str:
    value = float(n)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB", "PiB"):
        if value < 1024.0 or unit == "PiB":
            return f"{value:.2f} {unit}"
        value /= 1024.0
    return str(n)


def ratio_string(stored: int, raw: int) -> str:
    if raw == 0:
        return "n/a"
    return f"{stored/raw:.3f} ({100.0 * (1.0 - stored/raw):.1f}% smaller)"


def zstd_available() -> bool:
    return _zstd is not None


def category_for(path: Path) -> str:
    ext = path.suffix.lower()
    if ext in TEXT_LIKE:
        return "text"
    if ext in ALREADY_COMPRESSED:
        return "compressed"
    return "binary"


def compression_probe(data: bytes) -> float:
    """Cheap compressibility estimate. Lower is more compressible."""
    if not data:
        return 0.0
    sample = data if len(data) <= 256 * 1024 else data[:128 * 1024] + data[-128 * 1024:]
    return len(zlib.compress(sample, 1)) / len(sample)


def is_all_zero(data: bytes) -> bool:
    # count() is implemented in C and is much faster than Python byte iteration.
    return bool(data) and data.count(0) == len(data)


def zstd_compress(data: bytes, level: int) -> bytes:
    if _zstd is None:
        raise RuntimeError("stdlib Zstandard is unavailable; requires Python 3.14+ with compression.zstd")
    return _zstd.compress(data, level=level)


def zstd_decompress(data: bytes) -> bytes:
    if _zstd is None:
        raise RuntimeError("archive contains Zstandard blocks but this Python lacks compression.zstd")
    return _zstd.decompress(data)


def compress_block(data: bytes, mode: str, hint: str) -> tuple[int, bytes]:
    """Choose a lossless codec for one unique block."""
    if not data:
        return METHOD_RAW, b""
    if is_all_zero(data):
        return METHOD_ZERO, b""

    probe = compression_probe(data)

    # Already-compressed/high-entropy data: don't waste heroic CPU for negative gain.
    if hint == "compressed" and probe > 0.97:
        if _zstd is not None and mode == "fast":
            c = zstd_compress(data, 1)
            return (METHOD_ZSTD, c) if len(c) + 16 < len(data) else (METHOD_RAW, data)
        return METHOD_RAW, data

    # Very high entropy: storing raw is usually better.
    if probe > 0.992:
        return METHOD_RAW, data

    candidates: list[tuple[int, bytes]] = []

    if mode == "fast":
        if _zstd is not None:
            candidates.append((METHOD_ZSTD, zstd_compress(data, 3)))
        else:
            candidates.append((METHOD_ZLIB, zlib.compress(data, 6)))

    elif mode == "balanced":
        if _zstd is not None:
            # Zstd gives the speed path; LZMA is reserved for highly compressible blocks.
            candidates.append((METHOD_ZSTD, zstd_compress(data, 10)))
            if probe < 0.62:
                candidates.append((METHOD_LZMA, lzma.compress(data, preset=6)))
        else:
            if probe < 0.90:
                candidates.append((METHOD_LZMA, lzma.compress(data, preset=6)))
            else:
                candidates.append((METHOD_ZLIB, zlib.compress(data, 9)))

    elif mode == "max":
        # Python docs warn preset 9 can require very large memory. We use 9|EXTREME
        # deliberately only in explicit max mode.
        candidates.append((METHOD_LZMA, lzma.compress(data, preset=9 | lzma.PRESET_EXTREME)))
        if _zstd is not None:
            # Sometimes Zstd wins on structured binary data despite LZMA's stronger search.
            candidates.append((METHOD_ZSTD, zstd_compress(data, 19)))

    else:  # auto
        if probe < 0.58:
            # Strong redundancy: spend more CPU because the payoff is likely meaningful.
            candidates.append((METHOD_LZMA, lzma.compress(data, preset=6)))
            if _zstd is not None:
                candidates.append((METHOD_ZSTD, zstd_compress(data, 8)))
        elif _zstd is not None:
            candidates.append((METHOD_ZSTD, zstd_compress(data, 6)))
        elif probe < 0.93:
            candidates.append((METHOD_LZMA, lzma.compress(data, preset=5)))
        else:
            candidates.append((METHOD_ZLIB, zlib.compress(data, 6)))

    # Always allow raw storage. Compression must beat it by enough to justify metadata/CPU.
    method, payload = min(candidates, key=lambda x: len(x[1])) if candidates else (METHOD_RAW, data)
    if len(payload) + 16 >= len(data):
        return METHOD_RAW, data
    return method, payload


def decompress_block(method: int, payload: bytes, raw_size: int) -> bytes:
    if method == METHOD_RAW:
        data = payload
    elif method == METHOD_ZLIB:
        data = zlib.decompress(payload)
    elif method == METHOD_BZ2:
        data = bz2.decompress(payload)
    elif method == METHOD_LZMA:
        data = lzma.decompress(payload)
    elif method == METHOD_ZSTD:
        data = zstd_decompress(payload)
    elif method == METHOD_ZERO:
        data = b"\x00" * raw_size
    else:
        raise ValueError(f"unknown block compression method {method}")
    if len(data) != raw_size:
        raise ValueError(f"block size mismatch: expected {raw_size}, got {len(data)}")
    return data


def safe_arc_path(name: str) -> str:
    if not isinstance(name, str) or not name or "\x00" in name or "\\" in name:
        raise ValueError(f"unsafe archive path: {name!r}")
    parts = name.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise ValueError(f"unsafe archive path: {name!r}")
    # Keep archive names portable and prevent drive paths, NTFS alternate streams,
    # device names and names that alias one another on Windows.
    for part in parts:
        if any(ord(char) < 32 or char in '<>:"|?*' for char in part):
            raise ValueError(f"unsafe archive path: {name!r}")
        if part.endswith((".", " ")):
            raise ValueError(f"unsafe archive path: {name!r}")
        device_stem = part.split(".", 1)[0].upper()
        if device_stem in {"CON", "PRN", "AUX", "NUL"} or (
            len(device_stem) == 4
            and device_stem[:3] in {"COM", "LPT"}
            and device_stem[3] in "123456789"
        ):
            raise ValueError(f"unsafe archive path: {name!r}")
    p = PurePosixPath(name)
    if p.is_absolute() or not p.parts:
        raise ValueError(f"unsafe archive path: {name!r}")
    return p.as_posix()


def arcname_for(source: Path, path: Path) -> str:
    base = source.parent
    return safe_arc_path(path.relative_to(base).as_posix())


def walk_entries(source: Path) -> Iterable[Path]:
    """Yield source and descendants without following symlinked directories."""
    yield source
    if source.is_dir() and not source.is_symlink():
        for root, dirs, files in os.walk(source, topdown=True, followlinks=False):
            root_p = Path(root)
            # os.walk lists symlink dirs in dirs; yield them but prevent descent.
            real_dirs = []
            for d in sorted(dirs):
                p = root_p / d
                yield p
                if not p.is_symlink():
                    real_dirs.append(d)
            dirs[:] = real_dirs
            for f in sorted(files):
                yield root_p / f



def sparse_extents(path: Path, size: int) -> list[tuple[str, int, int]]:
    """Return [(kind, offset, length)] using Linux SEEK_DATA/SEEK_HOLE when supported.

    kind is "data" or "hole". Falls back to one data extent on filesystems that
    do not implement sparse extent queries.
    """
    if size <= 0 or not hasattr(os, "SEEK_DATA") or not hasattr(os, "SEEK_HOLE"):
        return [("data", 0, size)] if size else []
    fd = os.open(path, os.O_RDONLY)
    try:
        extents: list[tuple[str, int, int]] = []
        pos = 0
        while pos < size:
            try:
                data_off = os.lseek(fd, pos, os.SEEK_DATA)
            except OSError as e:
                # ENXIO means no more data: the remainder is a hole. EINVAL/ENOTSUP
                # means this filesystem does not support SEEK_DATA/SEEK_HOLE.
                if getattr(e, "errno", None) == 6:  # ENXIO
                    extents.append(("hole", pos, size - pos))
                    break
                return [("data", 0, size)]
            if data_off > pos:
                extents.append(("hole", pos, data_off - pos))
            try:
                hole_off = os.lseek(fd, data_off, os.SEEK_HOLE)
            except OSError:
                return [("data", 0, size)]
            hole_off = min(hole_off, size)
            if hole_off > data_off:
                extents.append(("data", data_off, hole_off - data_off))
            pos = max(hole_off, data_off + 1)
        return extents
    finally:
        os.close(fd)

def metadata_for(path: Path, source: Path) -> dict:
    st = path.lstat()
    entry = {
        "path": arcname_for(source, path),
        "mode": stat.S_IMODE(st.st_mode),
        "mtime_ns": st.st_mtime_ns,
        "uid": st.st_uid,
        "gid": st.st_gid,
    }
    if stat.S_ISLNK(st.st_mode):
        entry["type"] = "symlink"
        entry["target"] = os.readlink(path)
    elif stat.S_ISDIR(st.st_mode):
        entry["type"] = "dir"
    elif stat.S_ISREG(st.st_mode):
        entry["type"] = "file"
        entry["size"] = st.st_size
        entry["segments"] = []
    else:
        entry["type"] = "unsupported"
    return entry


class ArchiveWriter:
    def __init__(self, fp: BinaryIO, mode: str, stats: Stats, jobs: int = 1):
        self.fp = fp
        self.mode = mode
        self.stats = stats
        self.jobs = max(1, jobs)
        self.seen: dict[str, int] = {}  # digest -> raw_size, includes pending blocks
        self.pending = deque()
        self.executor = (
            concurrent.futures.ProcessPoolExecutor(max_workers=self.jobs)
            if self.jobs > 1 else None
        )
        self.max_pending = max(2, self.jobs * 2)
        self.fp.write(MAGIC)
        self.fp.write(struct.pack("<I", 2))

    def _write_result(self, digest: str, raw_size: int, result: tuple[int, bytes]) -> None:
        method, payload = result
        self.fp.write(BLOCK_TAG)
        self.fp.write(BLOCK_META.pack(bytes.fromhex(digest), method, raw_size, len(payload)))
        if payload:
            self.fp.write(payload)
        self.stats.blocks += 1
        self.stats.unique_raw_bytes += raw_size
        self.stats.stored_payload_bytes += len(payload)
        self.stats.method_raw_bytes[method] += raw_size
        self.stats.method_stored_bytes[method] += len(payload)
        if method == METHOD_ZERO:
            self.stats.zero_bytes_saved += raw_size

    def _drain_one(self) -> None:
        if not self.pending:
            return
        digest, raw_size, future = self.pending.popleft()
        self._write_result(digest, raw_size, future.result())

    def add_block(self, data: bytes, hint: str) -> str:
        digest = hashlib.sha256(data).hexdigest()
        if digest in self.seen:
            if self.seen[digest] != len(data):
                raise RuntimeError("SHA-256 collision with different block size")
            self.stats.duplicate_bytes_saved += len(data)
            return digest

        self.seen[digest] = len(data)
        if self.executor is None:
            self._write_result(digest, len(data), compress_block(data, self.mode, hint))
        else:
            future = self.executor.submit(compress_block, data, self.mode, hint)
            self.pending.append((digest, len(data), future))
            if len(self.pending) >= self.max_pending:
                self._drain_one()
        return digest

    def finish(self, manifest: dict) -> None:
        try:
            while self.pending:
                self._drain_one()
            raw = json.dumps(manifest, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
            # Manifest is usually repetitive JSON; modest LZMA is tiny and portable.
            stored = lzma.compress(raw, preset=6)
            digest = hashlib.sha256(raw).digest()
            self.fp.write(MANIFEST_TAG)
            self.fp.write(MANIFEST_META.pack(len(raw), len(stored), digest))
            self.fp.write(stored)
            self.fp.write(END_TAG)
        finally:
            if self.executor is not None:
                self.executor.shutdown(wait=True, cancel_futures=False)
                self.executor = None

    def abort(self) -> None:
        if self.executor is not None:
            self.executor.shutdown(wait=False, cancel_futures=True)
            self.executor = None



def default_jobs(mode: str) -> int:
    cpus = os.cpu_count() or 1
    if mode == "max":
        # LZMA preset 9 can consume hundreds of MiB per compressor, so max mode is
        # intentionally conservative unless the caller explicitly overrides --jobs.
        return 1
    return max(1, min(4, cpus))

def default_sizes(mode: str) -> tuple[int, int]:
    # (large-file chunk size, small-file solid block size)
    if mode == "fast":
        return 4 * 1024**2, 4 * 1024**2
    if mode == "balanced":
        return 16 * 1024**2, 16 * 1024**2
    if mode == "max":
        # Larger blocks give LZMA a broader local search window but require much more RAM.
        return 32 * 1024**2, 32 * 1024**2
    return 8 * 1024**2, 8 * 1024**2


def pack(source: Path, output: Path, mode: str, small_threshold: int, chunk_size: int | None,
         solid_size: int | None, verify_after: bool, jobs: int) -> None:
    source = source.resolve()
    if not source.exists() and not source.is_symlink():
        raise FileNotFoundError(source)
    if output.exists():
        raise FileExistsError(f"output already exists: {output}")

    default_chunk, default_solid = default_sizes(mode)
    chunk_size = chunk_size or default_chunk
    solid_size = solid_size or default_solid

    stats = Stats()
    manifest: dict = {
        "format": "SmartPack",
        "version": 2,
        "created_unix": time.time(),
        "source_name": source.name,
        "python": sys.version.split()[0],
        "zstd": bool(_zstd),
        "entries": [],
    }

    # Small-file solid buffers grouped by broad content class.
    buffers: dict[str, bytearray] = {"text": bytearray(), "binary": bytearray(), "compressed": bytearray()}
    pending_segments: dict[str, list[tuple[dict, int, int]]] = {k: [] for k in buffers}
    small_file_seen: dict[str, dict] = {}
    inode_seen: dict[tuple[int, int], str] = {}

    output.parent.mkdir(parents=True, exist_ok=True)
    temp_output = output.with_name(output.name + f".tmp.{os.getpid()}")

    def flush_group(writer: ArchiveWriter, group: str) -> None:
        buf = buffers[group]
        if not buf:
            return
        data = bytes(buf)
        digest = writer.add_block(data, group)
        for entry, offset, length in pending_segments[group]:
            entry["segments"].append({"block": digest, "offset": offset, "length": length})
        buf.clear()
        pending_segments[group].clear()

    started = time.time()
    writer = None
    try:
        with temp_output.open("wb") as fp:
            writer = ArchiveWriter(fp, mode, stats, jobs=jobs)

            for path in walk_entries(source):
                entry = metadata_for(path, source)
                if entry["type"] == "unsupported":
                    print(f"warning: skipping unsupported filesystem object: {path}", file=sys.stderr)
                    continue
                manifest["entries"].append(entry)

                if entry["type"] != "file":
                    continue

                st = path.lstat()
                stats.input_bytes += st.st_size

                # Preserve hardlinks without reading/storing the same inode twice.
                inode_key = (st.st_dev, st.st_ino)
                if st.st_nlink > 1 and inode_key in inode_seen:
                    entry["type"] = "hardlink"
                    entry["target"] = inode_seen[inode_key]
                    entry.pop("segments", None)
                    stats.duplicate_bytes_saved += st.st_size
                    continue
                if st.st_nlink > 1:
                    inode_seen[inode_key] = entry["path"]

                hint = category_for(path)

                if st.st_size <= small_threshold:
                    data = path.read_bytes()
                    file_digest = hashlib.sha256(data).hexdigest()
                    # Exact small-file duplicate: reuse prior segment references.
                    if file_digest in small_file_seen:
                        # Share the same segment list object. If the original is still
                        # waiting in a not-yet-flushed solid block, both entries are
                        # populated when that block is flushed.
                        entry["segments"] = small_file_seen[file_digest]["segments"]
                        stats.duplicate_bytes_saved += len(data)
                        continue

                    group = hint
                    if len(buffers[group]) + len(data) > solid_size and buffers[group]:
                        flush_group(writer, group)
                    offset = len(buffers[group])
                    buffers[group].extend(data)
                    pending_segments[group].append((entry, offset, len(data)))
                    # Segment reference is only known after the solid block is flushed.
                    # Store a pointer to the entry and capture it after flushing.
                    small_file_seen[file_digest] = entry
                    if len(buffers[group]) >= solid_size:
                        flush_group(writer, group)
                    continue

                # Large file: use Linux sparse extents when available, then fixed-size
                # chunk deduplication for allocated data. This avoids even reading giant holes.
                with path.open("rb") as f:
                    for extent_kind, extent_off, extent_len in sparse_extents(path, st.st_size):
                        if extent_kind == "hole":
                            entry["segments"].append({"zero": extent_len})
                            stats.zero_bytes_saved += extent_len
                            continue
                        f.seek(extent_off)
                        remaining = extent_len
                        while remaining:
                            data = f.read(min(chunk_size, remaining))
                            if not data:
                                raise IOError(f"unexpected EOF while reading {path}")
                            remaining -= len(data)
                            digest = writer.add_block(data, hint)
                            entry["segments"].append({"block": digest, "offset": 0, "length": len(data)})

            for group in buffers:
                flush_group(writer, group)

            # small_file_seen may reference lists populated only after final flush; now all are stable.
            writer.finish(manifest)
            fp.flush()
            os.fsync(fp.fileno())

        os.replace(temp_output, output)
    except Exception:
        if writer is not None:
            writer.abort()
        try:
            temp_output.unlink(missing_ok=True)
        except Exception:
            pass
        raise

    elapsed = time.time() - started
    archive_size = output.stat().st_size
    print(f"Created: {output}")
    print(f"Mode: {mode} | compression workers: {jobs}")
    print(f"Python: {sys.version.split()[0]} | stdlib Zstd: {'yes' if _zstd else 'no'}")
    print(f"Input: {human_size(stats.input_bytes)}")
    print(f"Archive: {human_size(archive_size)}")
    print(f"Ratio: {ratio_string(archive_size, stats.input_bytes)}")
    print(f"Unique blocks: {stats.blocks}")
    print(f"Exact dedup/hardlink bytes avoided: {human_size(stats.duplicate_bytes_saved)}")
    print(f"Zero-block bytes represented sparsely: {human_size(stats.zero_bytes_saved)}")
    if elapsed > 0:
        print(f"Time: {elapsed:.2f}s | input throughput: {human_size(int(stats.input_bytes/elapsed))}/s")
    methods = []
    for method in sorted(stats.method_raw_bytes):
        raw = stats.method_raw_bytes[method]
        stored = stats.method_stored_bytes[method]
        methods.append(f"{METHOD_NAMES[method]} {human_size(raw)}→{human_size(stored)}")
    if methods:
        print("Blocks by codec: " + "; ".join(methods))

    if verify_after:
        verify(output, quiet=False)


def read_index(archive: Path) -> tuple[dict[str, BlockInfo], dict]:
    index: dict[str, BlockInfo] = {}
    with archive.open("rb") as fp:
        if fp.read(len(MAGIC)) != MAGIC:
            raise ValueError("not a SmartPack archive (bad magic)")
        version_data = fp.read(4)
        if len(version_data) != 4:
            raise ValueError("truncated SmartPack header")
        version = struct.unpack("<I", version_data)[0]
        if version != 2:
            raise ValueError(f"unsupported SmartPack format version {version}")

        while True:
            tag = fp.read(1)
            if not tag:
                raise ValueError("archive ended before manifest")
            if tag == BLOCK_TAG:
                meta = fp.read(BLOCK_META.size)
                if len(meta) != BLOCK_META.size:
                    raise ValueError("truncated block header")
                digest_b, method, raw_size, stored_size = BLOCK_META.unpack(meta)
                payload_offset = fp.tell()
                digest = digest_b.hex()
                if digest in index:
                    raise ValueError("duplicate block record in archive")
                index[digest] = BlockInfo(payload_offset, method, raw_size, stored_size)
                fp.seek(stored_size, os.SEEK_CUR)
            elif tag == MANIFEST_TAG:
                meta = fp.read(MANIFEST_META.size)
                if len(meta) != MANIFEST_META.size:
                    raise ValueError("truncated manifest header")
                raw_size, stored_size, expected_digest = MANIFEST_META.unpack(meta)
                stored = fp.read(stored_size)
                if len(stored) != stored_size:
                    raise ValueError("truncated manifest")
                raw = lzma.decompress(stored)
                if len(raw) != raw_size or hashlib.sha256(raw).digest() != expected_digest:
                    raise ValueError("manifest integrity check failed")
                manifest = json.loads(raw.decode("utf-8"))
                end = fp.read(1)
                if end != END_TAG:
                    raise ValueError("missing SmartPack end marker")
                return index, manifest
            else:
                raise ValueError(f"unknown archive record tag: {tag!r}")


def load_block(fp: BinaryIO, info: BlockInfo, digest: str) -> bytes:
    fp.seek(info.offset)
    payload = fp.read(info.stored_size)
    if len(payload) != info.stored_size:
        raise ValueError(f"truncated payload for block {digest}")
    data = decompress_block(info.method, payload, info.raw_size)
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f"SHA-256 mismatch for block {digest}")
    return data


def verify(archive: Path, quiet: bool = False) -> None:
    started = time.time()
    index, manifest = read_index(archive)
    if any(info.method == METHOD_ZSTD for info in index.values()) and _zstd is None:
        raise RuntimeError("archive uses Zstandard; verify with Python 3.14+ containing compression.zstd")

    with archive.open("rb") as fp:
        for digest, info in index.items():
            load_block(fp, info, digest)

    # Verify that all manifest references resolve and segment ranges are legal.
    for entry in manifest.get("entries", []):
        if entry.get("type") == "file":
            total = 0
            for seg in entry.get("segments", []):
                if "zero" in seg:
                    length = int(seg["zero"])
                    if length < 0:
                        raise ValueError(f"invalid zero segment in {entry['path']}")
                    total += length
                    continue
                digest = seg["block"]
                if digest not in index:
                    raise ValueError(f"manifest references missing block {digest}")
                info = index[digest]
                off = int(seg["offset"])
                length = int(seg["length"])
                if off < 0 or length < 0 or off + length > info.raw_size:
                    raise ValueError(f"invalid segment range in {entry['path']}")
                total += length
            if total != int(entry.get("size", -1)):
                raise ValueError(f"file size mismatch in manifest: {entry['path']}")

    if not quiet:
        print(f"Verified OK: {archive} | {len(index)} blocks | {time.time()-started:.2f}s")


def ensure_inside(root: Path, candidate: Path) -> None:
    root_r = root.resolve()
    cand_parent = candidate.parent.resolve()
    if cand_parent != root_r and root_r not in cand_parent.parents:
        raise ValueError(f"path escapes extraction root: {candidate}")


def is_reparse_or_symlink(path: Path) -> bool:
    """Detect links and Windows reparse points without following the final path."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        return False
    if stat.S_ISLNK(info.st_mode):
        return True
    return bool(getattr(info, "st_file_attributes", 0) & 0x400)


def validate_extraction_plan(root: Path, entries: list[dict], overwrite: bool, unsafe_links: bool) -> None:
    """Validate all names, relationships and existing conflicts before any writes."""
    allowed_types = {"dir", "file", "hardlink", "symlink"}
    types_by_path: dict[str, str] = {}
    entries_by_key: dict[str, dict] = {}
    for entry in entries:
        if not isinstance(entry, dict) or entry.get("type") not in allowed_types:
            raise ValueError("archive contains an unsupported or malformed entry")
        rel = safe_arc_path(entry.get("path"))
        key = unicodedata.normalize("NFC", rel).casefold()
        if key in entries_by_key:
            raise ValueError(f"archive contains colliding paths: {rel!r}")
        entries_by_key[key] = entry
        types_by_path[key] = entry["type"]
        if entry["type"] in {"hardlink", "symlink"}:
            target = entry.get("target")
            if not isinstance(target, str) or not target:
                raise ValueError(f"archive contains an invalid link target: {rel!r}")
            if entry["type"] == "hardlink":
                safe_arc_path(target)
            elif not unsafe_links and not safe_symlink_target(rel, target):
                raise ValueError(f"unsafe symlink target {target!r} in {rel!r}")

    for entry in entries:
        if entry["type"] == "hardlink":
            target_key = unicodedata.normalize("NFC", safe_arc_path(entry["target"])).casefold()
            if types_by_path.get(target_key) != "file":
                raise ValueError(f"hardlink target is not a regular archive file: {entry['path']!r}")

    for rel, kind in ((safe_arc_path(entry["path"]), entry["type"]) for entry in entries):
        parts = PurePosixPath(rel).parts
        parent_key = ""
        for component in parts[:-1]:
            parent_key = f"{parent_key}/{component}" if parent_key else component
            parent_kind = types_by_path.get(unicodedata.normalize("NFC", parent_key).casefold())
            if parent_kind is not None and parent_kind != "dir":
                raise ValueError(f"archive path has a non-directory parent: {rel!r}")
        target = root.joinpath(*parts)
        ensure_inside(root, target)
        parent = root
        for component in parts[:-1]:
            parent = parent / component
            if is_reparse_or_symlink(parent):
                raise ValueError(f"refusing to extract through a link or reparse point: {parent}")
            if parent.exists() and not parent.is_dir():
                raise FileExistsError(f"extraction parent is not a directory: {parent}")
        if is_reparse_or_symlink(target):
            if kind == "dir":
                raise ValueError(f"refusing to use a link as an extraction directory: {target}")
            if not overwrite:
                raise FileExistsError(f"refusing to overwrite: {target}")
        elif target.exists():
            if kind == "dir":
                if not target.is_dir():
                    raise FileExistsError(f"directory conflicts with existing file: {target}")
            elif target.is_dir():
                raise FileExistsError(f"file conflicts with existing directory: {target}")
            elif not overwrite:
                raise FileExistsError(f"refusing to overwrite: {target}")


def safe_symlink_target(entry_path: str, target: str) -> bool:
    if "\\" in target or "\x00" in target or ":" in target or any(ord(char) < 32 for char in target):
        return False
    t = PurePosixPath(target)
    if t.is_absolute():
        return False
    combined = PurePosixPath(entry_path).parent / t
    depth = 0
    for part in combined.parts:
        if part == "..":
            depth -= 1
            if depth < 0:
                return False
        elif part not in ("", "."):
            depth += 1
    return True


def unpack(archive: Path, output_dir: Path, overwrite: bool, unsafe_links: bool, cache_mib: int) -> None:
    index, manifest = read_index(archive)
    if any(info.method == METHOD_ZSTD for info in index.values()) and _zstd is None:
        raise RuntimeError("archive uses Zstandard; unpack with Python 3.14+ containing compression.zstd")

    # Authenticate and validate archive content before making any filesystem changes.
    verify(archive, quiet=True)
    entries = manifest.get("entries", [])
    if not isinstance(entries, list):
        raise ValueError("archive manifest entries are malformed")
    output_dir.mkdir(parents=True, exist_ok=True)
    output_dir = output_dir.resolve()
    validate_extraction_plan(output_dir, entries, overwrite, unsafe_links)
    cache = LRUCache(cache_mib * 1024**2)

    # Directories first, then files/hardlinks, symlinks last to reduce traversal hazards.
    dirs = [e for e in entries if e.get("type") == "dir"]
    files = [e for e in entries if e.get("type") == "file"]
    hardlinks = [e for e in entries if e.get("type") == "hardlink"]
    symlinks = [e for e in entries if e.get("type") == "symlink"]

    def target_for(entry: dict) -> Path:
        rel = safe_arc_path(entry["path"])
        target = output_dir.joinpath(*PurePosixPath(rel).parts)
        ensure_inside(output_dir, target)
        return target

    for entry in dirs:
        target = target_for(entry)
        if is_reparse_or_symlink(target):
            raise ValueError(f"refusing to use a link as an extraction directory: {target}")
        target.mkdir(parents=True, exist_ok=True)

    with archive.open("rb") as fp:
        for entry in files:
            target = target_for(entry)
            target.parent.mkdir(parents=True, exist_ok=True)
            if (target.exists() or is_reparse_or_symlink(target)) and not overwrite:
                raise FileExistsError(f"refusing to overwrite: {target}")
            fd, temp_name = tempfile.mkstemp(prefix=".smartpack-", dir=target.parent)
            temp = Path(temp_name)
            try:
                with os.fdopen(fd, "wb") as out:
                    for seg in entry.get("segments", []):
                        if "zero" in seg:
                            length = int(seg["zero"])
                            out.seek(length, os.SEEK_CUR)
                            continue

                        digest = seg["block"]
                        info = index[digest]
                        off = int(seg["offset"])
                        length = int(seg["length"])

                        # Entire zero block segment: create a sparse hole instead of physically writing zeros.
                        if info.method == METHOD_ZERO and off == 0 and length == info.raw_size:
                            out.seek(length, os.SEEK_CUR)
                            continue

                        data = cache.get(digest)
                        if data is None:
                            data = load_block(fp, info, digest)
                            cache.put(digest, data)
                        out.write(data[off:off+length])
                    out.truncate(int(entry["size"]))
                    out.flush()
                    os.fsync(out.fileno())
                if overwrite:
                    # replace() replaces the destination symlink itself; it never opens its target.
                    os.replace(temp, target)
                else:
                    # Hard-linking the staged file commits atomically and refuses a racing conflict.
                    os.link(temp, target)
                    temp.unlink()
            finally:
                try:
                    temp.unlink()
                except FileNotFoundError:
                    pass
            apply_metadata(target, entry, follow_symlinks=True)

    # Hardlinks after regular files exist.
    for entry in hardlinks:
        target = target_for(entry)
        source_rel = safe_arc_path(entry["target"])
        source_target = output_dir.joinpath(*PurePosixPath(source_rel).parts)
        ensure_inside(output_dir, source_target)
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() or target.is_symlink():
            if not overwrite:
                raise FileExistsError(f"refusing to overwrite: {target}")
            target.unlink()
        os.link(source_target, target)

    # Symlinks last; reject absolute/out-of-tree targets unless explicitly allowed.
    for entry in symlinks:
        target = target_for(entry)
        link_target = entry["target"]
        if not unsafe_links and not safe_symlink_target(entry["path"], link_target):
            raise ValueError(f"unsafe symlink target {link_target!r} in {entry['path']!r}")
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() or target.is_symlink():
            if not overwrite:
                raise FileExistsError(f"refusing to overwrite: {target}")
            target.unlink()
        os.symlink(link_target, target)

    # Apply directory metadata last so file creation doesn't keep changing directory mtimes.
    for entry in reversed(dirs):
        target = target_for(entry)
        apply_metadata(target, entry, follow_symlinks=True)

    print(f"Extracted: {archive} -> {output_dir}")


def apply_metadata(path: Path, entry: dict, follow_symlinks: bool) -> None:
    try:
        os.chmod(path, int(entry.get("mode", 0o755)), follow_symlinks=follow_symlinks)
    except (PermissionError, NotImplementedError, OSError):
        pass
    mtime_ns = entry.get("mtime_ns")
    if mtime_ns is not None:
        try:
            os.utime(path, ns=(int(mtime_ns), int(mtime_ns)), follow_symlinks=follow_symlinks)
        except (PermissionError, NotImplementedError, OSError):
            pass
    # chown is attempted only when permitted (normally root); harmlessly ignored otherwise.
    try:
        os.chown(path, int(entry.get("uid", -1)), int(entry.get("gid", -1)), follow_symlinks=follow_symlinks)
    except (PermissionError, NotImplementedError, OSError, AttributeError):
        pass


def list_archive(archive: Path) -> None:
    index, manifest = read_index(archive)
    print(f"Archive: {archive}")
    print(f"Format: SmartPack v{manifest.get('version')}")
    print(f"Created with Python: {manifest.get('python')}; Zstd available then: {manifest.get('zstd')}")
    print(f"Blocks: {len(index)}")
    for entry in manifest.get("entries", []):
        t = entry.get("type", "?")
        size = entry.get("size", "")
        print(f"{t:8} {str(size):>12}  {entry.get('path','')}")


def analyze(source: Path) -> None:
    source = source.resolve()
    files = 0
    dirs = 0
    total = 0
    sampled = 0
    sample_stored = 0
    by_cat = defaultdict(int)
    max_sample = 32 * 1024**2

    for path in walk_entries(source):
        try:
            st = path.lstat()
        except OSError:
            continue
        if stat.S_ISDIR(st.st_mode):
            dirs += 1
        elif stat.S_ISREG(st.st_mode):
            files += 1
            total += st.st_size
            by_cat[category_for(path)] += st.st_size
            if sampled < max_sample and st.st_size:
                take = min(512 * 1024, max_sample - sampled, st.st_size)
                with path.open("rb") as f:
                    data = f.read(take)
                sampled += len(data)
                sample_stored += len(zlib.compress(data, 1))

    probe = sample_stored / sampled if sampled else 1.0
    print(f"Source: {source}")
    print(f"Files: {files} | dirs: {dirs} | size: {human_size(total)}")
    print(f"Quick compressibility ratio: {probe:.3f} (lower is better)")
    for cat in ("text", "binary", "compressed"):
        n = by_cat[cat]
        share = 100*n/total if total else 0
        print(f"{cat:10}: {human_size(n):>12} ({share:5.1f}%)")
    if _zstd is not None:
        print("Runtime: stdlib Zstandard available (Python 3.14+ path enabled)")
    else:
        print("Runtime: stdlib Zstandard unavailable; SmartPack will use LZMA/zlib only")
    if probe < 0.55:
        print("Suggested mode: auto or balanced; max may reduce size further if RAM/time are acceptable")
    elif probe < 0.90:
        print("Suggested mode: balanced")
    else:
        print("Suggested mode: fast/auto; most data appears already compressed or high entropy")


def make_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        epilog="Created by Taha Moeini · https://taha.one",
        prog="smartpack_ubuntu.py",
        description="Dependency-free adaptive lossless archiver for Ubuntu (Python stdlib only).",
    )
    p.add_argument(
        "--version",
        action="version",
        version=f"SmartPack {VERSION}\nCreated by Taha Moeini · https://taha.one",
    )
    sub = p.add_subparsers(dest="command", required=True)

    a = sub.add_parser("analyze", help="inspect an input tree without creating an archive")
    a.add_argument("source", type=Path)

    c = sub.add_parser("pack", help="create a .spk archive")
    c.add_argument("source", type=Path)
    c.add_argument("-o", "--output", type=Path)
    c.add_argument("--mode", choices=("auto", "fast", "balanced", "max"), default="auto")
    c.add_argument("--small-kib", type=int, default=256,
                   help="files at/below this size are solid-grouped by type (default: 256 KiB)")
    c.add_argument("--chunk-mib", type=int,
                   help="large-file block size; default depends on mode")
    c.add_argument("--solid-mib", type=int,
                   help="small-file solid block target; default depends on mode")
    c.add_argument("-j", "--jobs", type=int, default=0,
                   help="parallel compression workers; 0=auto (max mode defaults to 1)")
    c.add_argument("--no-verify", action="store_true", help="skip full post-write block verification")

    u = sub.add_parser("unpack", help="extract a .spk archive")
    u.add_argument("archive", type=Path)
    u.add_argument("-o", "--output", type=Path, default=Path("."))
    u.add_argument("--overwrite", action="store_true")
    u.add_argument("--unsafe-links", action="store_true",
                   help="allow absolute/out-of-tree symlink targets (not recommended)")
    u.add_argument("--cache-mib", type=int, default=128,
                   help="decompressed block cache used for dedup/solid blocks")

    v = sub.add_parser("verify", help="fully verify manifest and every data block")
    v.add_argument("archive", type=Path)

    l = sub.add_parser("list", help="list archive contents")
    l.add_argument("archive", type=Path)
    return p


def main() -> int:
    args = make_parser().parse_args()
    try:
        if args.command == "analyze":
            analyze(args.source)
        elif args.command == "pack":
            output = args.output
            if output is None:
                output = Path(args.source.name + ".spk")
            chunk_size = args.chunk_mib * 1024**2 if args.chunk_mib else None
            solid_size = args.solid_mib * 1024**2 if args.solid_mib else None
            jobs = args.jobs if args.jobs > 0 else default_jobs(args.mode)
            if args.mode == "max" and jobs > 2:
                print("warning: max mode with >2 workers can require very large RAM due to LZMA preset 9", file=sys.stderr)
            pack(
                args.source,
                output,
                args.mode,
                args.small_kib * 1024,
                chunk_size,
                solid_size,
                not args.no_verify,
                jobs,
            )
        elif args.command == "unpack":
            unpack(args.archive, args.output, args.overwrite, args.unsafe_links, args.cache_mib)
        elif args.command == "verify":
            verify(args.archive)
        elif args.command == "list":
            list_archive(args.archive)
        return 0
    except KeyboardInterrupt:
        print("Interrupted.", file=sys.stderr)
        return 130
    except Exception as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
