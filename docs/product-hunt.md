# Product Hunt positioning

This file is the public-positioning source of truth for the current SmartPack implementation. Keep Product Hunt and other launch copy aligned with the current Rust/Tauri product rather than the historical Python-only prototype.

## Recommended fields

**Name**  
SmartPack

**Tagline**  
Private, cross-platform archiving with a shared Rust core.

**Product URL**  
https://github.com/tahamoeini/smartpack

**Description**  
SmartPack is a local-first archive manager for Windows, macOS, and Linux, powered by a shared Rust engine. It creates SPK and ZIP archives, reads common formats such as ZIP, 7z, TAR, gzip, xz, Zstandard, and lz4, and supports adaptive compression, duplicate-block reuse, integrity verification, safe extraction, optional SPK encryption, and PAR2 recovery workflows. Processing stays on-device. The project is free and open source; signed installers are not available yet.

**GitHub URL**  
https://github.com/tahamoeini/smartpack

**Pricing**  
Free

## Category guidance

Keep **File storage and sharing apps** and **Command line tools** if these are the available Product Hunt taxonomy choices. SmartPack now has a desktop application as well as a CLI, so a desktop-utility category is a better third category if Product Hunt offers one at edit time. Do not remove the CLI category while the CLI remains a supported first-class interface.

## Claims to remove from the current listing

Do not describe the current product as:

- built entirely with Python's standard library;
- zero third-party dependencies;
- Ubuntu-only;
- proven to compress a 64 MB corpus to 8 MB versus 37 MB with xz;
- approximately three times faster than xz.

Those statements describe the earlier Python implementation and/or an early benchmark. The current product uses a Rust engine, Tauri desktop stack, and external crates. The current benchmark protocol explicitly requires reproducible measurements against the current engine before comparative performance claims are published.

## Release-status wording

Until signed installers are published and platform smoke tests are complete, avoid copy that implies a polished downloadable desktop release. The repository can truthfully say that Windows, macOS, and Linux engine/desktop-host builds pass CI while signed installers and broader release validation are still pending.
