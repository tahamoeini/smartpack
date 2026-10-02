# Product Hunt positioning

This file is the public-positioning source of truth for the current SmartPack implementation. Keep Product Hunt and other launch copy aligned with the current Rust/Tauri product rather than the historical Python-only prototype.

## Recommended fields

**Name**  
SmartPack

**Tagline**  
Private, cross-platform archiving powered by Rust.

**Product URL**  
https://github.com/tahamoeini/smartpack

**Description**  
SmartPack is a local-first archive manager for Windows, macOS, and Linux. A shared Rust engine powers the desktop app and CLI. Create SPK or ZIP archives, open common formats, verify integrity, extract defensively, encrypt SPK archives, and add PAR2 recovery data. SmartPack keeps file processing on your device and uses compression and deduplication strategies to avoid wasted work. Free and open source; signed installers are not available yet.

**GitHub URL**  
https://github.com/tahamoeini/smartpack

**Pricing**  
Free

## Category guidance

Keep **File storage and sharing apps** and **Command line tools** if these are the available Product Hunt taxonomy choices. SmartPack now has a desktop application as well as a CLI, so add a desktop utility / file utility category if Product Hunt offers one at edit time. Do not remove the CLI category while the CLI remains a supported first-class interface.

## Maker comment

Replace the original benchmark-heavy launch comment with this current-state update:

> SmartPack started as a Python standard-library experiment: how far can archiving go without third-party dependencies? That experiment taught me a lot, but the product outgrew the constraint.
>
> SmartPack v0.3 is now a local-first archive manager built around a shared Rust engine and a Tauri desktop app for Windows, macOS, and Linux.
>
> What it does today:
> - creates SPK and ZIP archives;
> - reads common formats including ZIP, 7z, TAR, gzip, xz, Zstandard, and lz4;
> - verifies integrity and extracts defensively;
> - encrypts SPK archives with Argon2id and XChaCha20-Poly1305;
> - reuses duplicate blocks and represents zero chunks efficiently;
> - supports PAR2 recovery workflows.
>
> The benchmark numbers from the original launch were produced by the earlier Python prototype, so I am retiring those claims until I rerun a reproducible benchmark against the current Rust engine.
>
> The principle is still the same: avoid wasted work, keep archive processing local, and make integrity and recovery first-class concerns.
>
> It is free and open source and still pre-release. Signed installers are not available yet. I would especially value real-world archives and edge cases that stress extraction, compatibility, recovery, and large-file behavior.

## Gallery plan

The current Product Hunt page exposes only one gallery asset. Replace or expand it so the visual story matches the desktop product:

1. **Desktop home / create flow** — show the current Tauri UI and the SPK/ZIP creation options.
2. **Open / inspect / extract flow** — show archive browsing, verification, extraction, and progress.
3. **Privacy and safety** — communicate local processing, optional SPK encryption, integrity verification, and safe extraction without overclaiming security certification.
4. **How SmartPack works** — a simple architecture/feature visual showing the shared Rust engine, desktop app, CLI, supported read formats, and SPK-specific capabilities.

Avoid using old Python-terminal screenshots as the lead visual unless they are explicitly labeled as project history.

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

## GitHub metadata

The GitHub repository description should also be changed from the historical Python/Ubuntu wording to:

> Local-first archive manager for Windows, macOS, and Linux with a shared Rust engine, desktop app, CLI, encryption, verification, and recovery.
