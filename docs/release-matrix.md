# Release validation matrix

| Area | Implemented in this branch | Release work remaining |
| --- | --- | --- |
| Engine and CLI | Shared Rust library, create/list/verify/extract commands, cancellation handle, JSON-based internal records; CI builds and tests on Windows, macOS, and Linux | Stable public request/result/event API; CLI cancellation and batch jobs |
| SPK | v3 write, v2 read, Zstandard/LZMA2/raw selection, CDC boundaries, duplicate and zero chunks, optional authenticated encryption | Volumes; robust cross-version fixture corpus; metadata restoration; configurable codec memory; content-size privacy review |
| ZIP and other formats | ZIP creation; bundled streaming reader/extractor for supported ZIP/7z/TAR and enabled gzip/xz/Zstandard/lz4 filters | RAR support; standalone bzip2 filter; WinZip AES creation; format-specific unsupported-feature reporting and broad fixtures |
| Extraction safety | Capability-rooted file creation, no-follow final-path checks, staged per-file output, no-overwrite publication, traversal/collision and decompression-size checks | Adversarial Windows reparse-point, Unicode normalization, hardlink, and parser-fuzz runs on all target operating systems |
| PAR2 | Optional PAR2 creation and repair APIs are wired into the engine, CLI and GUI; creation cancellation is connected | End-to-end damage/recovery fixtures, repair cancellation, cross-tool compatibility testing |
| Desktop | Tauri 2 + React interface; CI native host checks on Windows, macOS, and Linux; tag builds produce Windows x64 setup EXE, Ubuntu x64 DEB/AppImage, and Intel/Apple-silicon macOS DMG | Per-OS installer smoke tests, default-app/context-menu validation, accessibility checks, signing and notarization |
| Release | Apache-2.0 project license; lockfile-derived third-party notices bundled in the app; beta release workflow; repeatable SmartPack/gzip/7-Zip benchmark artifact | Stable-release gates below, signing and notarization, full adversarial platform smoke testing, PAR2 cross-tool validation |

GitHub Actions is the authoritative native build environment for the three desktop operating systems. The beta installers are unsigned and are not notarized; users may see operating-system security prompts. The first beta is suitable for evaluation with copies of important data, not as the only backup.

## Release gates

- Round-trip bytes match exactly for v3 and supported v2 fixtures; supported ZIP and external-format fixtures verify and extract correctly.
- Adversarial archive paths cannot escape the output capability; no final destination is overwritten without explicit policy; cancellation removes temporary outputs.
- KDF parameters are checked before key derivation; wrong passwords and ciphertext tampering fail closed.
- Peak working memory is measured against the selected block, KDF, parser, and worker budgets.
- CI passes for Windows, macOS, and Linux, and the tag-triggered native bundles complete for all listed architectures.
- Before a stable release, smoke-test signed installers, file associations, drag/drop, extraction, and cancellation on each OS; complete PAR2 cross-tool fixtures and adversarial platform tests.
- Comparative results identify the dataset, machine, settings, and tool version; do not generalize the synthetic CI corpus to all workloads.
