# Product Hunt listing update

The existing Product Hunt profile describes the original Ubuntu-only Python prototype and calls SmartPack "zero dependencies." The current product is a cross-platform Rust desktop app and CLI with third-party dependencies. Replace the stale profile copy with the text below.

## Tagline

Create, verify, and safely extract archives on desktop

## Short description

Local-first archive manager for Windows, macOS, and Linux, powered by a shared Rust engine.

## Product description

SmartPack is a local-first archive manager for creating, inspecting, verifying, and extracting archives on Windows, macOS, and Linux. Its desktop app and CLI share the same Rust engine, so the same core archive behavior is available in both workflows.

Create SmartPack (SPK) archives with Balanced, Fast, Smallest, or Store profiles, or create ZIP archives. SPK supports optional password-based encryption and integrity checks. SmartPack can also read and safely extract supported ZIP, 7z, and TAR archives, with enabled gzip, xz, Zstandard, and lz4 filters.

Extraction rejects unsafe paths and avoids overwriting existing destination entries. SmartPack is open source under Apache-2.0 and runs locally; it does not require an account or cloud service.

SmartPack is in active development. Some planned format support, metadata restoration, signed installers, and full cross-platform release checks are still in progress. Check the release notes for current build availability and known limitations.

## Suggested first comment

Hi Product Hunt 👋 SmartPack is a local-first archive manager for Windows, macOS, and Linux. The desktop app and CLI share a Rust engine, so you can create and verify archives with the same core tools in either workflow. It creates SPK and ZIP archives, supports optional encryption for SPK, and safely extracts supported formats without overwriting existing files. It’s open source and still in active development. I’d especially value feedback on the everyday workflow, compatibility with real-world archives, and what you expect from a cross-platform archive tool.

## Update checklist

- Replace the Ubuntu/Python-only description; it describes the prototype, not the current implementation.
- Remove "zero dependencies"; the Rust engine and desktop app use third-party dependencies.
- Avoid unqualified "lossless" claims as a differentiator; archive extraction correctness should be demonstrated through compatibility and round-trip evidence.
- Add current screenshots of the desktop app and a short demo showing create, verify, and extract.
- Link to the current release/download page and clearly label beta status and platform availability.
- Keep format support precise: RAR, standalone bzip2-filter extraction, WinZip AES creation, volume splitting, and full metadata restoration are not currently complete.
- Avoid comparative speed or size claims until reproducible benchmark results are published with workload, settings, hardware, and tool versions.
