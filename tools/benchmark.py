#!/usr/bin/env python3
"""Run repeatable archive benchmarks and write JSON/CSV results."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import platform
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

try:
    import psutil
except ImportError:  # Memory readings are optional outside the CI benchmark image.
    psutil = None


def find_tool(*names: str) -> str | None:
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return None


def run_command(command: list[str], cwd: Path, log_path: Path) -> dict[str, Any]:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    peak_rss = 0
    start = time.perf_counter()
    with log_path.open("wb") as log:
        process = subprocess.Popen(command, cwd=cwd, stdout=log, stderr=subprocess.STDOUT)
        if psutil is not None:
            try:
                root = psutil.Process(process.pid)
            except psutil.Error:
                root = None
            if root is not None:
                try:
                    peak_rss = sum(
                        proc.memory_info().rss for proc in [root, *root.children(recursive=True)]
                    )
                except psutil.Error:
                    pass
            while process.poll() is None:
                if root is not None:
                    try:
                        children = root.children(recursive=True)
                        peak_rss = max(
                            peak_rss,
                            sum(proc.memory_info().rss for proc in [root, *children]),
                        )
                    except psutil.Error:
                        pass
                time.sleep(0.025)
        return_code = process.wait()
    elapsed = time.perf_counter() - start
    if return_code:
        tail = log_path.read_text(encoding="utf-8", errors="replace")[-6000:]
        raise RuntimeError(
            f"Command failed ({return_code}): {command!r}\n"
            f"Log: {log_path}\n{tail}"
        )
    return {
        "command": command,
        "wall_seconds": elapsed,
        "peak_rss_bytes": peak_rss or None,
        "log": str(log_path),
    }


def file_hashes(root: Path) -> dict[str, str]:
    hashes: dict[str, str] = {}
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
        hashes[path.relative_to(root).as_posix()] = digest.hexdigest()
    return hashes


def make_suite(root: Path) -> dict[str, Path]:
    suite = root / "corpus"
    suite.mkdir(parents=True)
    sentence = (
        b"SmartPack deterministic benchmark corpus. Repeated text tests compression; "
        b"the other cases exercise incompressible data, small files, and duplicates.\n"
    )
    text = (sentence * ((512 * 1024 // len(sentence)) + 1))[: 512 * 1024]
    cases: dict[str, Path] = {}

    def write_case(name: str, files: dict[str, bytes]) -> None:
        payload = suite / name / "payload"
        payload.mkdir(parents=True)
        for relative, content in files.items():
            destination = payload / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
        cases[name] = payload

    write_case("text", {"report.txt": text})
    generator = random.Random(0x5A17)
    write_case("incompressible-random", {"random.bin": generator.randbytes(256 * 1024)})
    write_case(
        "many-small-files",
        {
            f"batch-{number // 32:02d}/item-{number:03d}.txt": (
                (f"record={number:03d}; ".encode("ascii") + text[:4050])[:4096]
            )
            for number in range(128)
        },
    )
    duplicate = (text + text[: 256 * 1024])[: 256 * 1024]
    write_case("duplicate-files", {f"copy-{number:02d}.bin": duplicate for number in range(8)})
    return cases


def tool_specs(smartpack: str) -> tuple[list[dict[str, Any]], dict[str, str]]:
    tools: list[dict[str, Any]] = [
        {
            "id": "smartpack",
            "label": "SmartPack balanced (SPK v3)",
            "extension": ".spk",
            "create": lambda source, archive: [
                smartpack,
                "create",
                str(source),
                str(archive),
                "--profile",
                "balanced",
            ],
            "extract": lambda archive, destination: [
                smartpack,
                "extract",
                str(archive),
                str(destination),
            ],
            "version": [smartpack, "--version"],
        }
    ]
    available: dict[str, str] = {"smartpack": smartpack}

    tar = find_tool("tar")
    gzip = find_tool("gzip")
    if tar and gzip:
        tools.append(
            {
                "id": "gzip-tar",
                "label": "tar + gzip (-6)",
                "extension": ".tar.gz",
                "create": lambda source, archive: [
                    tar,
                    "-czf",
                    str(archive),
                    "-C",
                    str(source.parent),
                    source.name,
                ],
                "extract": lambda archive, destination: [
                    tar,
                    "-xzf",
                    str(archive),
                    "-C",
                    str(destination),
                ],
                "versions": {"gzip": [gzip, "--version"], "tar": [tar, "--version"]},
                "options": "tar -czf (gzip default level; report the installed GNU gzip version)",
            }
        )
        available["gzip"] = gzip
    else:
        tools.append({"id": "gzip-tar", "label": "tar + gzip", "unavailable": "tar or gzip not found"})

    seven_zip = find_tool("7zz", "7z", "7za")
    if seven_zip:
        tools.append(
            {
                "id": "7zip",
                "label": "7-Zip 7z (-mx=5, one thread)",
                "extension": ".7z",
                "create": lambda source, archive: [
                    seven_zip,
                    "a",
                    "-t7z",
                    "-mx=5",
                    "-mmt=1",
                    "-y",
                    str(archive),
                    source.name,
                ],
                "extract": lambda archive, destination: [
                    seven_zip,
                    "x",
                    str(archive),
                    f"-o{destination}",
                    "-y",
                ],
                "version": [seven_zip],
                "options": "7z format, compression level 5, one thread",
            }
        )
        available["7zip"] = seven_zip
    else:
        tools.append({"id": "7zip", "label": "7-Zip", "unavailable": "7z/7zz executable not found"})

    rar = find_tool("rar", "Rar.exe", "WinRAR.exe")
    if rar:
        tools.append(
            {
                "id": "winrar",
                "label": "RAR (-m3, one thread)",
                "extension": ".rar",
                "create": lambda source, archive: [
                    rar,
                    "a",
                    "-m3",
                    "-mt1",
                    "-y",
                    str(archive),
                    source.name,
                ],
                "extract": lambda archive, destination: [
                    rar,
                    "x",
                    "-y",
                    str(archive),
                    str(destination) + os.sep,
                ],
                "version": [rar],
                "options": "RAR format, method 3, one thread",
            }
        )
        available["winrar"] = rar
    else:
        tools.append({"id": "winrar", "label": "WinRAR / RAR", "unavailable": "RAR executable not found"})

    wzzip = find_tool("wzzip", "wzzip.exe")
    wzunzip = find_tool("wzunzip", "wzunzip.exe")
    if wzzip and wzunzip:
        tools.append(
            {
                "id": "winzip",
                "label": "WinZip ZIP",
                "extension": ".zip",
                "create": lambda source, archive: [
                    wzzip,
                    "-ex",
                    "-rP",
                    str(archive),
                    source.name,
                ],
                "extract": lambda archive, destination: [
                    wzunzip,
                    str(archive),
                    str(destination),
                ],
                "version": [wzzip],
                "options": "WinZip command-line add-on defaults with recursive paths",
            }
        )
        available["winzip"] = wzzip
    else:
        tools.append({"id": "winzip", "label": "WinZip", "unavailable": "WZZIP and WZUNZIP not found"})

    return tools, available


def version_string(command: list[str], cwd: Path) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    text = (result.stdout + result.stderr).strip().splitlines()
    return text[0][:240] if text else "version unavailable"


def median_summary(values: list[float]) -> dict[str, float]:
    return {
        "median": statistics.median(values),
        "min": min(values),
        "max": max(values),
    }


def memory_total_bytes() -> int | None:
    if psutil is None:
        return None
    return int(psutil.virtual_memory().total)


def benchmark_case(
    tool: dict[str, Any],
    case: str,
    source: Path,
    expected_hashes: dict[str, str],
    repeat: int,
    root: Path,
    log_root: Path,
) -> dict[str, Any]:
    samples = []
    archive_sizes = []
    for number in range(1, repeat + 1):
        run_root = root / f"run-{number}"
        run_root.mkdir(parents=True)
        archive = run_root / f"archive{tool['extension']}"
        destination = run_root / "extracted"
        destination.mkdir()
        create_command = tool["create"](source, archive)
        create = run_command(
            create_command,
            source.parent,
            log_root / tool["id"] / case / f"{number}-create.log",
        )
        archive_size = archive.stat().st_size
        extract_command = tool["extract"](archive, destination)
        extract = run_command(
            extract_command,
            source.parent,
            log_root / tool["id"] / case / f"{number}-extract.log",
        )
        extracted_root = destination / source.name
        actual_hashes = file_hashes(extracted_root) if extracted_root.is_dir() else {}
        if actual_hashes != expected_hashes:
            raise RuntimeError(
                f"Round-trip content check failed for {tool['id']} on {case}, run {number}"
            )
        archive_sizes.append(archive_size)
        samples.append(
            {
                "run": number,
                "archive_bytes": archive_size,
                "create_seconds": create["wall_seconds"],
                "extract_seconds": extract["wall_seconds"],
                "create_peak_rss_bytes": create["peak_rss_bytes"],
                "extract_peak_rss_bytes": extract["peak_rss_bytes"],
                "create_command": create_command,
                "extract_command": extract_command,
            }
        )
    create_times = [sample["create_seconds"] for sample in samples]
    extract_times = [sample["extract_seconds"] for sample in samples]
    rss_values = [
        value
        for sample in samples
        for value in (sample["create_peak_rss_bytes"], sample["extract_peak_rss_bytes"])
        if value is not None
    ]
    return {
        "tool_id": tool["id"],
        "tool": tool["label"],
        "format": tool["extension"].lstrip("."),
        "options": tool.get("options", "SmartPack balanced profile"),
        "case": case,
        "input_bytes": sum((source / path).stat().st_size for path in expected_hashes),
        "input_files": len(expected_hashes),
        "archive_bytes_median": int(statistics.median(archive_sizes)),
        "archive_to_input_ratio": statistics.median(archive_sizes)
        / sum((source / path).stat().st_size for path in expected_hashes),
        "create_seconds": median_summary(create_times),
        "extract_seconds": median_summary(extract_times),
        "peak_rss_bytes": max(rss_values) if rss_values else None,
        "samples": samples,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--smartpack", required=True, help="Path to the SmartPack CLI binary")
    parser.add_argument("--repeat", type=int, default=3, help="Measured runs per tool and dataset (default: 3)")
    parser.add_argument("--output-dir", type=Path, default=Path("benchmark-results"))
    parser.add_argument("--corpus", type=Path, help="Use this directory as a single custom dataset")
    parser.add_argument(
        "--require-tools",
        default="",
        help="Comma-separated tool IDs which must be installed (for example: gzip,7zip)",
    )
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be at least 1")
    smartpack = str(Path(args.smartpack).resolve())
    if not Path(smartpack).is_file():
        parser.error(f"SmartPack executable does not exist: {smartpack}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    log_root = args.output_dir / "logs"
    with tempfile.TemporaryDirectory(prefix="smartpack-bench-") as temporary:
        root = Path(temporary)
        if args.corpus:
            corpus = args.corpus.resolve()
            if not corpus.is_dir():
                parser.error("--corpus must point to an existing directory")
            cases = {"custom": corpus}
        else:
            cases = make_suite(root)

        corpus_records = {}
        for name, source in cases.items():
            hashes = file_hashes(source)
            if not hashes:
                parser.error(f"dataset is empty: {source}")
            corpus_records[name] = {
                "input_files": len(hashes),
                "input_bytes": sum((source / item).stat().st_size for item in hashes),
                "sha256": hashlib.sha256(
                    "\n".join(f"{name}:{digest}" for name, digest in sorted(hashes.items())).encode()
                ).hexdigest(),
                "source_path_recorded": False,
            }

        tools, available = tool_specs(smartpack)
        missing = sorted(
            item.strip()
            for item in args.require_tools.split(",")
            if item.strip() and item.strip() not in available
        )
        if missing:
            raise RuntimeError(f"Required benchmark tools are unavailable: {', '.join(missing)}")

        results = []
        skipped = []
        versions = {}
        for tool in tools:
            if tool.get("unavailable"):
                skipped.append({"tool_id": tool["id"], "reason": tool["unavailable"]})
                continue
            version_commands = tool.get("versions", {tool["id"]: tool["version"]})
            versions[tool["id"]] = {
                name: version_string(command, Path.cwd())
                for name, command in version_commands.items()
            }
            for case, source in cases.items():
                hashes = file_hashes(source)
                result = benchmark_case(
                    tool,
                    case,
                    source,
                    hashes,
                    args.repeat,
                    root / f"work-{tool['id']}-{case}",
                    log_root,
                )
                results.append(result)

        try:
            commit = subprocess.run(
                ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True
            ).stdout.strip()
        except (OSError, subprocess.CalledProcessError):
            commit = None
        report = {
            "schema_version": 1,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "git_commit": commit,
            "system": {
                "os": platform.platform(),
                "architecture": platform.machine(),
                "processor": platform.processor() or None,
                "logical_cpu_count": os.cpu_count(),
                "memory_total_bytes": memory_total_bytes(),
                "python": platform.python_version(),
                "psutil": getattr(psutil, "__version__", None),
                "tool_versions": versions,
                "cache_policy": "warm cache; sequential runs; archive output on local runner filesystem",
            },
            "corpus": corpus_records,
            "results": results,
            "skipped_tools": skipped,
        }
        json_path = args.output_dir / "results.json"
        json_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        csv_path = args.output_dir / "summary.csv"
        with csv_path.open("w", newline="", encoding="utf-8") as output:
            fields = [
                "case",
                "tool_id",
                "tool",
                "format",
                "options",
                "input_bytes",
                "archive_bytes_median",
                "archive_to_input_ratio",
                "size_saving_percent",
                "create_seconds_median",
                "create_seconds_min",
                "create_seconds_max",
                "extract_seconds_median",
                "extract_seconds_min",
                "extract_seconds_max",
                "peak_rss_bytes",
            ]
            writer = csv.DictWriter(output, fieldnames=fields)
            writer.writeheader()
            for result in results:
                writer.writerow(
                    {
                        **{field: result[field] for field in fields if field in result},
                        "size_saving_percent": (
                            100 * (1 - result["archive_to_input_ratio"])
                        ),
                        "create_seconds_median": result["create_seconds"]["median"],
                        "create_seconds_min": result["create_seconds"]["min"],
                        "create_seconds_max": result["create_seconds"]["max"],
                        "extract_seconds_median": result["extract_seconds"]["median"],
                        "extract_seconds_min": result["extract_seconds"]["min"],
                        "extract_seconds_max": result["extract_seconds"]["max"],
                    }
                )
        print(f"Wrote {json_path} and {csv_path}")
        print(f"Measured {len(results)} tool/dataset combinations; skipped {len(skipped)} unavailable tools.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"benchmark: {error}", file=sys.stderr)
        raise SystemExit(1)
