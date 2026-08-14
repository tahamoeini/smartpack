#!/usr/bin/env python3
"""SmartPack Python stdlib archive variant.

This is the dependency-free version, keeping to Python's standard library only.
It creates a tar.gz archive and writes a sidecar sha256 manifest for verification.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tarfile
from pathlib import Path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fp:
        for chunk in iter(lambda: fp.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def walk_files(root: Path):
    if root.is_file():
        yield root
        return
    for entry in sorted(root.rglob("*")):
        if entry.is_file():
            yield entry


def build_manifest(root: Path) -> dict:
    base = root.parent
    entries = []
    for file_path in walk_files(root):
        rel = file_path.relative_to(base).as_posix()
        entries.append(
            {
                "path": rel,
                "sha256": sha256_file(file_path),
                "size": file_path.stat().st_size,
            }
        )
    return {
        "format": "smartpack-python-stdlib",
        "root": root.name,
        "files": entries,
    }


def safe_extract(tar: tarfile.TarFile, dest: Path) -> None:
    dest = dest.resolve()
    for member in tar.getmembers():
        target = (dest / member.name).resolve()
        if target != dest and dest not in target.parents:
            raise ValueError(f"unsafe archive member: {member.name!r}")
        if member.isdir():
            target.mkdir(parents=True, exist_ok=True)
        elif member.isfile():
            target.parent.mkdir(parents=True, exist_ok=True)
            extracted = tar.extractfile(member)
            if extracted is None:
                raise ValueError(f"unable to extract {member.name!r}")
            with target.open("wb") as fp:
                fp.write(extracted.read())
        elif member.issym() or member.islnk():
            raise ValueError(f"unsupported link in archive: {member.name!r}")


def pack(source: Path, output: Path) -> None:
    source = source.resolve()
    if not source.exists():
        raise FileNotFoundError(source)
    output = output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(output, mode="w:gz", compresslevel=9) as tar:
        tar.add(source, arcname=source.name, recursive=True)
    manifest = build_manifest(source)
    manifest_path = output.with_name(f"{output.name}.manifest.json")
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    print(f"Packed: {source} -> {output}")


def unpack(archive: Path, output_dir: Path) -> None:
    archive = archive.resolve()
    if not archive.exists():
        raise FileNotFoundError(archive)
    output_dir = output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, mode="r:gz") as tar:
        safe_extract(tar, output_dir)
    print(f"Extracted: {archive} -> {output_dir}")


def verify(archive: Path) -> None:
    archive = archive.resolve()
    manifest_path = archive.with_name(f"{archive.name}.manifest.json")
    if not manifest_path.exists():
        raise FileNotFoundError(f"missing manifest: {manifest_path}")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    with tarfile.open(archive, mode="r:gz") as tar:
        members = {member.name: member for member in tar.getmembers()}
        for item in manifest.get("files", []):
            rel = item["path"]
            if rel not in members:
                raise ValueError(f"missing archive member: {rel}")
            member = members[rel]
            if member.isfile():
                extracted = tar.extractfile(member)
                if extracted is None:
                    raise ValueError(f"unable to read archive member: {rel}")
                digest = hashlib.sha256(extracted.read()).hexdigest()
                if digest != item["sha256"]:
                    raise ValueError(f"sha256 mismatch for {rel}")
    print(f"Verified OK: {archive}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="stdlib-only SmartPack archive tool")
    sub = parser.add_subparsers(dest="command", required=True)

    pack_parser = sub.add_parser("pack", help="create a tar.gz archive and a sha256 manifest")
    pack_parser.add_argument("source", type=Path)
    pack_parser.add_argument("-o", "--output", type=Path, required=True)

    unpack_parser = sub.add_parser("unpack", help="extract a tar.gz archive")
    unpack_parser.add_argument("archive", type=Path)
    unpack_parser.add_argument("-o", "--output", type=Path, default=Path("."))

    verify_parser = sub.add_parser("verify", help="verify archive files against the manifest")
    verify_parser.add_argument("archive", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "pack":
            pack(args.source, args.output)
        elif args.command == "unpack":
            unpack(args.archive, args.output)
        elif args.command == "verify":
            verify(args.archive)
        return 0
    except Exception as exc:  # pragma: no cover - CLI safety
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
