# Release validation matrix

| Area | Implemented in this branch | Release work remaining |
| --- | --- | --- |
| Engine and CLI | Shared Rust library, create/list/verify/extract commands, cancellation handle, JSON-based internal records | Type-check/build on a machine with native toolchains; publish stable request/result/event API; CLI cancellation and batch jobs |
| SPK | v3 write, v2 read, Zstandard/LZMA2/raw selection, CDC boundaries, duplicate and zero chunks, optional authenticated encryption | Volumes; robust cross-version fixture corpus; metadata restoration; configurable codec memory; content-size privacy review |
| ZIP and other formats | ZIP creation; bundled streaming reader/extractor for supported ZIP/7z/TAR and enabled gzip/xz/Zstandard/lz4 filters | RAR support; standalone bzip2 filter; WinZip AES creation; format-specific unsupported-feature reporting and broad fixtures |
| Extraction safety | Capability-rooted file creation, no-follow final-path checks, staged per-file output, no-overwrite publication, traversal/collision and decompression-size checks | Adversarial Windows reparse-point, Unicode normalization, hardlink, and parser-fuzz runs on all target operating systems |
| PAR2 | Optional PAR2 creation and repair APIs are wired into the engine, CLI and GUI; creation cancellation is connected | End-to-end damage/recovery fixtures, repair cancellation, cross-tool compatibility testing |
| Desktop | Tauri 2 + React interface, dialogs, drop handling, job progress and cancellation, compression profiles, SPK file association declaration | Native Tauri build and smoke tests on Windows/macOS/Linux; OS default-app and context-menu validation; accessibility and installer checks |
| Release | Apache-2.0 project license, CI workflow, dependency lockfiles, product/format documentation | Complete third-party notices, signed installers, supported architecture matrix, release automation, comparative benchmark publication |

The Windows development checkout did not have the MSVC linker or Windows SDK libraries at implementation time. Rust engine compilation and runtime tests therefore have to pass in CI or a properly provisioned native environment before this branch is considered release-ready. The frontend TypeScript/Vite production build is verified separately.

## Release gates

- Round-trip bytes match exactly for v3 and supported v2 fixtures; supported ZIP and external-format fixtures verify and extract correctly.
- Adversarial archive paths cannot escape the output capability; no final destination is overwritten without explicit policy; cancellation removes temporary outputs.
- KDF parameters are checked before key derivation; wrong passwords and ciphertext tampering fail closed.
- Peak working memory is measured against the selected block, KDF, parser, and worker budgets.
- CI passes for Windows, macOS, and Linux. Installer signing, file associations, drag/drop, extraction, and cancellation are smoke-tested per platform.
- No unresolved critical/high security issues; comparative results identify dataset, machine, settings, and tool version.
