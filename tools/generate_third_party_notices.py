#!/usr/bin/env python3
"""Build a complete, lockfile-derived third-party license notice bundle."""

from __future__ import annotations

import argparse
import json
import platform
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def license_files(package_dir: Path, declared_file: str | None = None) -> list[Path]:
    found: set[Path] = set()
    if declared_file:
        specified = Path(declared_file)
        if not specified.is_absolute():
            specified = package_dir / specified
        if specified.is_file():
            found.add(specified)
    if not package_dir.is_dir():
        return []
    for item in package_dir.iterdir():
        if item.is_file() and item.name.upper().startswith(
            ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE")
        ):
            found.add(item)
        elif item.is_dir() and item.name.casefold() in {"licenses", "licences"}:
            found.update(path for path in item.rglob("*") if path.is_file())
    return sorted(found, key=lambda path: path.relative_to(package_dir).as_posix().casefold())


def npm_license_owner(lock_name: str) -> str | None:
    if lock_name.startswith("node_modules/@esbuild/"):
        return "node_modules/esbuild"
    if lock_name.startswith("node_modules/@rollup/rollup-"):
        return "node_modules/rollup"
    if lock_name.startswith("node_modules/@tauri-apps/cli-"):
        return "node_modules/@tauri-apps/cli"
    if lock_name.startswith("node_modules/@napi-rs/lzma-"):
        return "node_modules/@napi-rs/lzma"
    return None


def cargo_packages(manifest: Path, target: str) -> list[dict[str, Any]]:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            target,
            "--manifest-path",
            str(manifest),
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    metadata = json.loads(result.stdout)
    nodes = {node["id"]: node for node in metadata.get("resolve", {}).get("nodes", [])}
    root = metadata.get("resolve", {}).get("root")
    reachable = set()
    pending = [root] if root else []
    while pending:
        package_id = pending.pop()
        if package_id in reachable:
            continue
        reachable.add(package_id)
        for dependency in nodes.get(package_id, {}).get("deps", []):
            kinds = dependency.get("dep_kinds", [])
            if kinds and all(kind.get("kind") == "dev" for kind in kinds):
                continue
            pending.append(dependency["pkg"])
    return [
        package
        for package in metadata["packages"]
        if package.get("source") and package["id"] in reachable
    ]


def collect_packages(target: str) -> list[dict[str, Any]]:
    packages: dict[tuple[str, str], dict[str, Any]] = {}

    for manifest_rel in ("rust_crates/Cargo.toml", "desktop/src-tauri/Cargo.toml"):
        for package in cargo_packages(ROOT / manifest_rel, target):
            key = (package["name"], package["version"])
            record = packages.setdefault(
                key,
                {
                    "name": package["name"],
                    "version": package["version"],
                    "license": package.get("license") or "license metadata missing",
                    "repository": package.get("repository"),
                    "files": [],
                },
            )
            package_dir = Path(package["manifest_path"]).parent
            record["files"].extend(
                license_files(package_dir, package.get("license_file"))
            )

    lock_path = ROOT / "desktop/package-lock.json"
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    desktop = lock_path.parent
    for lock_name, package in lock.get("packages", {}).items():
        if not lock_name.startswith("node_modules/") or not package.get("version"):
            continue
        package_dir = desktop / lock_name
        # npm lockfiles list optional native packages for every OS; only installed
        # packages are part of the bundle produced on this runner.
        if not package_dir.is_dir():
            continue
        package_json = package_dir / "package.json"
        metadata = {}
        if package_json.is_file():
            metadata = json.loads(package_json.read_text(encoding="utf-8"))
        key = (metadata.get("name") or lock_name.removeprefix("node_modules/"), package["version"])
        record = packages.setdefault(
            key,
            {
                "name": key[0],
                "version": key[1],
                "license": package.get("license") or metadata.get("license") or "license metadata missing",
                "repository": metadata.get("repository"),
                "files": [],
            },
        )
        declared_file = None
        declared = record["license"]
        if isinstance(declared, str) and declared.upper().startswith("SEE LICENSE IN "):
            declared_file = declared[15:].strip()
        record["files"].extend(license_files(package_dir, declared_file))
        if not record["files"]:
            owner = npm_license_owner(lock_name)
            if owner:
                record["files"].extend(license_files(desktop / owner))

    for record in packages.values():
        record["files"] = sorted(set(record["files"]))
    return sorted(packages.values(), key=lambda record: (record["name"].casefold(), record["version"]))


def build_notice(target: str) -> tuple[str, list[str]]:
    packages = collect_packages(target)
    missing = []
    sections = [
        "# Third-party software notices",
        "",
        f"This file lists and reproduces license and notice files for the Rust dependency graph filtered to `{target}` and the JavaScript packages installed for this operating system. SmartPack-authored code is licensed separately under Apache-2.0 in `LICENSE`.",
        "",
        "## Dependency inventory",
        "",
        "| Package | Version | Declared license |",
        "| --- | --- | --- |",
    ]
    for package in packages:
        license = str(package["license"]).replace("|", "\\|").replace("\n", " ")
        sections.append(f"| `{package['name']}` | `{package['version']}` | {license} |")

    sections.extend(["", "## License and notice texts", ""])
    for package in packages:
        if package["license"] == "license metadata missing":
            missing.append(f"{package['name']} {package['version']} (license metadata missing)")
        if not package["files"]:
            missing.append(f"{package['name']} {package['version']} ({package['license']})")
            continue
        for path in package["files"]:
            try:
                contents = path.read_text(encoding="utf-8", errors="replace").strip()
            except OSError as error:
                missing.append(f"{package['name']} {package['version']} ({error})")
                continue
            if not contents:
                missing.append(f"{package['name']} {package['version']} (empty {path.name})")
                continue
            label = f"{package['name']} {package['version']} — {path.name}"
            sections.extend([f"### {label}", "", "    " + contents.replace("\n", "\n    "), ""])
    return "\n".join(sections).rstrip() + "\n", missing


def native_target() -> str:
    result = subprocess.run(
        ["rustc", "-vV"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    for line in result.stdout.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ").strip()
    raise RuntimeError("rustc did not report its host target")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "desktop/src-tauri/resources/THIRD_PARTY_NOTICES.md",
    )
    parser.add_argument("--target", help="Rust target triple (defaults to the installed rustc host)")
    parser.add_argument("--check", action="store_true", help="Fail if the output notice file is stale")
    args = parser.parse_args()
    try:
        target = args.target or native_target()
        notice, missing = build_notice(target)
    except (OSError, RuntimeError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"notice generation failed: {error}", file=sys.stderr)
        return 1
    if missing:
        print("Dependency packages without license/notice text files:", file=sys.stderr)
        for package in missing:
            print(f"- {package}", file=sys.stderr)
        return 1
    if args.check:
        if not args.output.is_file() or args.output.read_text(encoding="utf-8") != notice:
            print(f"{args.output} is missing or stale; regenerate it with this script", file=sys.stderr)
            return 1
        print(f"Verified complete notices: {args.output}")
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(notice, encoding="utf-8", newline="\n")
    print(f"Wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
