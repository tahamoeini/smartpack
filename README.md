<div align="center">
  <img src="assets/smartpack-logo.svg" alt="SmartPack logo" width="420" />

  <h3>Private, cross-platform archiving with a shared Rust core.</h3>

  <p>
    A local-first archive manager for Windows, macOS, and Linux,<br/>
    with a desktop app, CLI, adaptive compression, encryption, verification, and recovery workflows.
  </p>

  <a href="https://www.producthunt.com/products/smartpack?embed=true&amp;utm_source=badge-featured&amp;utm_medium=badge&amp;utm_campaign=badge-smartpack" target="_blank" rel="noopener noreferrer">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1247021&amp;theme=dark&amp;t=1789071163896">
      <source media="(prefers-color-scheme: light)" srcset="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1247021&amp;theme=light&amp;t=1789071152289">
      <img alt="SmartPack on Product Hunt" width="250" height="54" src="https://api.producthunt.com/widgets/embed-image/v1/featured.svg?post_id=1247021&amp;theme=neutral&amp;t=1789071158579">
    </picture>
  </a>
</div>

# SmartPack

SmartPack is a local-first archive manager built around a shared Rust engine, a command-line interface, and a Tauri desktop application. It is designed to make everyday archive creation and extraction straightforward while keeping advanced compression, integrity, privacy, and recovery controls available when needed.

> **Current status:** SmartPack is at **v0.3** and should be treated as an active pre-release project. The engine and desktop host build successfully in CI on Windows, macOS, and Linux, but signed installers and broad platform smoke testing are not complete yet. There are currently no published GitHub Releases.

SmartPack does not claim to beat every archive tool on every dataset. Compression results depend on input, codec, CPU, filesystem, and settings. Comparative claims belong in reproducible, workload-specific benchmarks using the current engine.

## What SmartPack does today

- Creates **SPK v3** archives with content-defined chunking, SHA-256 integrity checks, duplicate-block reuse, zero-chunk representation, streaming input, and bounded per-chunk buffers.
- Reads and verifies legacy **SPK v2** archives created by the earlier Python implementation.
- Provides Fast and Balanced profiles using Zstandard, Smallest using LZMA2/XZ, and Store for uncompressed storage. Already-compressed signatures and unhelpful compression results fall back to raw storage.
- Supports optional **SPK encryption** using Argon2id and XChaCha20-Poly1305. Names and metadata are stored in the encrypted manifest, and passwords are not written to logs or configuration.
- Creates ZIP archives and safely reads supported external formats through a bundled pure-Rust archive implementation, including ZIP, 7z, TAR, and enabled gzip, xz, Zstandard, and lz4 filters.
- Uses capability-rooted safe extraction. Traversal and absolute paths are rejected, links and special files are disabled by default, and existing destination entries are not overwritten.
- Includes a Tauri 2 desktop interface for Windows, macOS, and Linux with create, open, extract, verify, recent jobs, drag-and-drop, progress, cancellation, and compression profiles.
- Exposes the same archive engine through a CLI:

  ```text
  smartpack create <source> <archive.spk|archive.zip> [--profile fast|balanced|smallest|store] [--encrypt]
  smartpack list <archive.spk>
  smartpack verify <archive>
  smartpack extract <archive> <destination>
  smartpack recover create <archive> <output-directory> [recovery-percent]
  smartpack repair <index.par2> <source-directory> <repaired-output-directory>
  ```

## What it does not claim yet

SmartPack is not yet a drop-in replacement for every mature archive suite. PAR2 creation and repair have CLI, GUI, and shared-engine entry points, but still need end-to-end damaged-file fixtures and cross-tool validation. RAR reading, standalone bzip2-filter extraction, WinZip AES creation, SPK volume splitting, full timestamp/permissions/link/sparse restoration, signed installers, and broad platform smoke testing remain to be completed.

See the [release validation matrix](docs/release-matrix.md) and [benchmark protocol](docs/benchmarks.md) for the remaining release gates.

## Build the engine and CLI

Rust 1.89 or newer is required by the selected codec and archive crates.

```powershell
cargo test --manifest-path rust_crates/Cargo.toml --locked
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

For a local production bundle, use `npm run tauri build`. Tauri can package platform-native installers, but each platform’s required system libraries, signing, file associations, and installer smoke checks must be verified before distribution.

## Product Hunt copy

The current public positioning and the exact Product Hunt fields that match the v0.3 implementation are maintained in [docs/product-hunt.md](docs/product-hunt.md). Historical Python-only claims and early benchmark numbers should not be reused for the current Rust-based product unless they are reproduced against the current engine.

## Repository layout

- `rust_crates/`: shared archive engine, CLI, and engine tests.
- `desktop/`: React/TypeScript UI and Tauri 2 host.
- `docs/`: SPK format notes, release validation, benchmark protocol, and public product copy.
- `smartpack_ubuntu.py`, `smartpack_python_lite.py`, `smartpack_python_native.py`, `smartpack_rust_native.rs`: historical prototypes retained for compatibility/reference; new development should use the Rust engine.

## License

SmartPack is distributed under the Apache License 2.0; see [LICENSE](LICENSE). Third-party dependency notices must be included with binary releases.
