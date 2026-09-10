<div align="center">
  <img src="assets/smartpack-logo.svg" alt="SmartPack logo" width="420" />

  <h3>Lossless archiving. Zero dependencies.</h3>

  <p>
    An adaptive, integrity-checked lossless archiver for Ubuntu,<br/>
    built entirely on Python's standard library.
  </p>
</div>

---

# SmartPack

SmartPack is a dependency-free lossless archiver designed for Ubuntu. The primary implementation, `smartpack_ubuntu.py`, creates its own `.spk` archive format and decides how to store data block by block instead of applying one compression strategy to everything.

It is built around a simple constraint: **no 7-Zip, no WinRAR, no zstd CLI, no pip packages, and no network service.**

SmartPack combines content-aware compression, cross-file deduplication, solid packing of small files, sparse-data handling, integrity verification, and defensive extraction in one standalone Python script.

## Why SmartPack?

| Capability | What it does |
| --- | --- |
| **Zero external dependencies** | The main implementation uses Python's standard library only. |
| **Adaptive compression** | Samples data and chooses an appropriate storage/compression path instead of forcing one codec on every block. |
| **Cross-file deduplication** | Identical fixed-size blocks are stored once and referenced wherever they reappear. |
| **Solid packing** | Small, similar files can be grouped to expose redundancy across file boundaries. |
| **Sparse / zero-block optimization** | Zero-filled regions can be represented without storing all those zero bytes. |
| **Integrity verification** | Every stored block is identified and checked with SHA-256. |
| **Safer extraction** | Rejects path traversal and unsafe symlink targets by default. |
| **Filesystem-aware** | Preserves directories, regular files, symlinks, hardlinks, modes, and modification times. |

## Requirements

- Ubuntu or another Linux environment
- **Python 3.10+** for the primary implementation
- **Python 3.14+ recommended** if you want Python's standard-library Zstandard support via `compression.zstd`

On older supported Python versions, SmartPack still works using its standard-library LZMA/zlib paths.

## Quick start

Clone the repository:

```bash
git clone https://github.com/tahamoeini/smartpack.git
cd smartpack
```

Check the version:

```bash
python3 smartpack_ubuntu.py --version
```

### 1. Analyze before packing

SmartPack can inspect a directory and estimate how compressible its contents look before creating an archive:

```bash
python3 smartpack_ubuntu.py analyze ./my-folder
```

### 2. Create an archive

```bash
python3 smartpack_ubuntu.py pack ./my-folder -o backup.spk
```

If `-o` is omitted, SmartPack creates `<source>.spk`.

### 3. List archive contents

```bash
python3 smartpack_ubuntu.py list backup.spk
```

### 4. Verify the archive

```bash
python3 smartpack_ubuntu.py verify backup.spk
```

Packing performs a full verification pass by default. Use `--no-verify` only when you explicitly want to skip that post-write check.

### 5. Extract

```bash
python3 smartpack_ubuntu.py unpack backup.spk -o ./restored
```

Existing files are not overwritten unless you pass `--overwrite`.

## Compression modes

```bash
python3 smartpack_ubuntu.py pack ./data -o data.spk --mode auto
```

| Mode | Intended use |
| --- | --- |
| `auto` | Default. Chooses compression effort from the observed data. |
| `fast` | Favors faster compression and lower CPU cost. |
| `balanced` | Trades more CPU for potentially better compression. |
| `max` | Uses the most aggressive available settings; expect substantially higher CPU and memory use. |

You can also tune block sizing, solid-pack sizing, and worker count:

```bash
python3 smartpack_ubuntu.py pack ./data -o data.spk \
  --mode balanced \
  --small-kib 256 \
  --chunk-mib 8 \
  --solid-mib 4 \
  -j 4
```

## How it works

At a high level, SmartPack:

1. Walks the input tree and records filesystem metadata.
2. Groups eligible small files so cross-file redundancy can be exposed.
3. Splits larger data into blocks.
4. Detects zero-filled blocks and avoids storing their full payload.
5. Hashes blocks with SHA-256 and reuses already-seen blocks instead of storing duplicates.
6. Probes compressibility and chooses between raw storage and available standard-library codecs.
7. Writes a compressed manifest describing the archive.
8. Verifies stored blocks and manifest references before reporting success, unless verification is explicitly disabled.

The result is a custom `.spk` format designed to be read by SmartPack itself.

## Extraction safety

Archive extraction is deliberately defensive:

- archive paths are checked so they cannot escape the selected output directory;
- absolute or out-of-tree symlink targets are rejected by default;
- symlinks are created after regular files and hardlinks;
- overwriting existing files requires explicit opt-in.

There is an `--unsafe-links` option for cases where you intentionally need absolute or out-of-tree symlinks, but it should be treated as an escape hatch rather than the default workflow.

## Zstandard compatibility

When running on Python 3.14+ with `compression.zstd` available, SmartPack may create Zstandard-compressed blocks.

An archive containing those blocks must also be verified or extracted using a Python runtime that provides `compression.zstd`. Archives created without Zstandard support use the other available standard-library compression paths.

## What SmartPack is not

SmartPack does **not** claim to beat 7z, RAR, or every other compressor on every dataset. No lossless compressor can guarantee that.

The project is exploring a different trade-off: how far can a self-contained archiver go when it combines adaptive compression, deduplication, solid packing, integrity checking, and safe extraction without external runtime dependencies?

Also note that `.spk` is a custom format. Common archive tools do not natively open it.

## Repository variants

The repository contains several implementations and experiments:

| Path | Purpose |
| --- | --- |
| `smartpack_ubuntu.py` | **Primary implementation.** Custom `.spk` format, adaptive compression, deduplication, solid packing, verification, and safe extraction. |
| `smartpack_python_native.py` | Simpler Python stdlib variant using `tar.gz` plus a JSON SHA-256 manifest. |
| `smartpack_python_lite.py` | Python variant that can optionally use `orjson` and `zstandard`, with clean stdlib fallbacks. |
| `smartpack_rust_native.rs` | Rust experiment built around system `tar` and `sha256sum`. |
| `rust_crates/` | Crate-based Rust experiment. |

If you just want to try SmartPack, start with **`smartpack_ubuntu.py`**.

## Useful commands

```text
smartpack_ubuntu.py analyze <source>
smartpack_ubuntu.py pack <source> [-o archive.spk] [--mode auto|fast|balanced|max]
smartpack_ubuntu.py list <archive.spk>
smartpack_ubuntu.py verify <archive.spk>
smartpack_ubuntu.py unpack <archive.spk> [-o output_dir]
```

Run command-specific help for the complete option set:

```bash
python3 smartpack_ubuntu.py pack --help
python3 smartpack_ubuntu.py unpack --help
```

---

<div align="center">
  <strong>SmartPack</strong><br/>
  Pack smarter. Verify everything. Depend on less.
</div>
