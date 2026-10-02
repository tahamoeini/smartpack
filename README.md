<div align="center">
  <img src="assets/smartpack-logo.svg" alt="SmartPack logo" width="420" />

  <h3>Fast, safe archiving for every desktop.</h3>

  <p>
    A local-first archive manager for Windows, macOS, and Linux,<br/>
    powered by a shared Rust engine.
  </p>

  <a href="https://www.producthunt.com/products/smartpack?embed=true&amp;utm_source=badge-featured&amp;utm_medium=badge&amp;utm_campaign=badge-smartpack" target="_blank" rel="noopener noreferrer">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1247021&amp;theme=dark&amp;t=1789071163896">
      <source media="(prefers-color-scheme: light)" srcset="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1247021&amp;theme=light&amp;t=1789071152289">
      <img alt="SmartPack - Lossless archiving. Zero dependencies. | Product Hunt" width="250" height="54" src="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1247021&amp;theme=neutral&amp;t=1789071158579">
    </picture>
  </a>
</div>

# SmartPack

SmartPack is a local-first archive manager with a shared Rust engine, command-line tools, and a cross-platform Tauri desktop interface. It aims for a straightforward default workflow while exposing compression profiles and security controls when needed.

> SmartPack does not claim to beat every archive tool on every dataset. Compression results depend on input, codec, CPU, and settings. Comparative claims belong in reproducible, workload-specific benchmarks.

## Current implementation

- SPK v3 writer with content-defined chunking, SHA-256 integrity, duplicate-block reuse, zero-chunk representation, streaming input, and bounded per-chunk buffers.
- SPK v2 reader for the original Python format and SPK v2 verification/extraction.
- Balanced and Fast profiles using Zstandard, Smallest using LZMA2/XZ, and Store. Already-compressed signatures and unhelpful compression results use raw storage.
- Optional SPK encryption with Argon2id and XChaCha20-Poly1305. Names and metadata are kept in the encrypted manifest. Passwords are not written to logs or configuration.
- ZIP creation and safe external extraction through a bundled pure-Rust archive implementation. Supported external readers include ZIP, 7z, TAR, and the enabled gzip, xz, Zstandard, and lz4 filters.
- Capability-rooted safe extraction. Traversal and absolute paths are rejected, links and special files are disabled by default, and existing destination entries are not overwritten.
- Tauri 2 desktop interface for Windows, macOS, and Linux, with create, open, extract, verify, recent jobs, drag-and-drop, progress, cancellation, and compression profiles.
- Shared CLI:
  ```text
  smartpack create <source> <archive.spk|archive.zip> [--profile fast|balanced|smallest|store] [--encrypt]
  smartpack list <archive.spk>
  smartpack verify <archive>
  smartpack extract <archive> <destination>
  smartpack recover create <archive> <output-directory> [recovery-percent]
  smartpack repair <index.par2> <source-directory> <repaired-output-directory>
  ```

## Compatibility and current boundaries

SPK v3 is the write format. SmartPack continues to read and verify SPK v2; older SmartPack releases are not expected to read v3. The v3 record layout and encryption parameters are documented in [docs/format.md](docs/format.md).

The desktop and engine groundwork is in place, but this is not yet the full release suite in the approved platform plan. PAR2 creation and repair have CLI, GUI, and shared-engine entry points, but still need end-to-end damaged-file fixtures and cross-tool validation. RAR reading, standalone bzip2-filter extraction, WinZip AES creation, SPK volume splitting, full timestamp/permissions/link/sparse restoration, signed installers, and broad platform smoke testing remain to be completed. Unsupported features should return clear errors. See the [release matrix](docs/release-matrix.md) and [benchmark protocol](docs/benchmarks.md) for the remaining gates.

## Build the engine and CLI

Rust 1.89 or newer is required by the selected codec and archive crates.

```powershell
cargo test --manifest-path rust_crates/Cargo.toml
cargo run --manifest-path rust_crates/Cargo.toml -- create .\input .\backup.spk --profile balanced
cargo run --manifest-path rust_crates/Cargo.toml -- verify .\backup.spk
cargo run --manifest-path rust_crates/Cargo.toml -- extract .\backup.spk .\restored
```

To create an encrypted SPK, add `--encrypt`; the CLI prompts without echoing the password. ZIP names remain visible by design.

## Build the desktop app

Install the platform prerequisites for Tauri 2 and Node.js 22, then:

```sh
cd desktop
npm ci
npm run tauri dev
```

For a local production bundle, use `npm run tauri build`. Tauri can package platform-native installers, but each platform’s required system libraries, signing, file associations, and installer smoke checks must be verified on that platform before release.

## Repository layout

- `rust_crates/`: shared archive engine, CLI, and engine tests.
- `desktop/`: React/TypeScript UI and Tauri 2 host.
- `docs/`: SPK compatibility notes, release validation, and benchmark protocol.
- `smartpack_ubuntu.py`, `smartpack_python_lite.py`, `smartpack_python_native.py`, `smartpack_rust_native.rs`: historical prototypes retained for compatibility/reference; new development should use the Rust engine.

## License

SmartPack is distributed under the Apache License 2.0; see [LICENSE](LICENSE). Third-party dependency notices must be included with binary releases.
