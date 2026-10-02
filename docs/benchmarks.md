# Benchmarks and tool comparison

## Reproducible measurements

`tools/benchmark.py` creates a deterministic corpus with text, incompressible random bytes, many small files, and duplicate files. It benchmarks SmartPack's balanced SPK profile against tar+gzip and 7-Zip's 7z format when those tools are installed. It also detects local RAR/WinRAR and WinZip command-line tools. Every measured archive is extracted and checked against SHA-256 hashes of the input files before the result is accepted.

The report records tool versions, exact commands, options, input and archive sizes, create/extract wall time, repeated samples, median/min/max, and peak resident memory when `psutil` is available. The JSON and CSV reports plus command logs are retained as a GitHub Actions artifact for 90 days. CI uses Ubuntu 24.04, three warm-cache runs, one compression thread for 7-Zip, and synthetic data. Treat these as runner-specific measurements, not as universal rankings.

Run locally from the repository root:

```sh
cargo build --release --locked --manifest-path rust_crates/Cargo.toml
python tools/benchmark.py --smartpack rust_crates/target/release/smartpack --repeat 5
```

On Windows, use `rust_crates/target/release/smartpack.exe`. To benchmark a real directory, add `--corpus path/to/directory`; file names and contents are not copied into the report, but their SHA-256 hashes are. Install `gzip`, `tar`, and `7-Zip` to compare those tools. `--require-tools gzip,7zip` makes a run fail if either comparator is absent. RAR/WinRAR measurements require an installed RAR command-line tool; WinZip measurements require the licensed WZZIP/WZUNZIP add-on. CI does not install commercial archivers.

## Product comparison

| Tool | Main format/use | Strengths and trade-offs | Benchmark coverage |
| --- | --- | --- | --- |
| SmartPack beta | SPK v3 archive manager; also creates ZIP | Cross-platform desktop and CLI, optional authenticated SPK encryption, integrity verification, deduplicated blocks. SPK is a new format and is not yet as widely interoperable as ZIP/7z/RAR. | CI measures balanced SPK v3. |
| gzip + tar | `.tar.gz` stream for files/directories | Simple, open, widely available building block. gzip alone compresses a stream; tar supplies the multi-file container. No archive-level encryption or duplicate-block reuse. | CI measures `tar -czf` (system gzip defaults). |
| 7-Zip | `.7z`, ZIP, and many other formats | Mature general-purpose archiver with configurable compression and broad format support. `.7z` interoperability may require a compatible tool. | CI measures `.7z`, level 5, one thread. ZIP and other levels are not measured. |
| WinRAR / RAR | RAR creation and extraction; WinRAR also handles ZIP | Mature commercial archive tool. Official RAR/UnRAR command-line packages exist for Windows, Linux, and macOS; the Windows WinRAR product is a separate distribution. RAR support is not implemented in SmartPack. | Optional local RAR adapter; no commercial tool is installed in CI. |
| WinZip | ZIP/Zipx and supported formats | Commercial desktop product for Windows and macOS; its optional command-line add-on supports scripted operations and WinZip AES. SmartPack currently creates ordinary Deflate ZIP and does not create WinZip AES ZIP. | Optional local WZZIP/WZUNZIP adapter; not installed in CI. |
| XZ / Zstandard | Compression codecs/streams | Useful codec-level references, but comparing a raw stream against an archive manager omits container, file-walk, and metadata costs. | Not in the initial CI corpus comparison. |

These are feature and format comparisons, not claims that SmartPack is faster or smaller. Official product references: [7-Zip](https://www.7-zip.org/), [WinRAR/RARLAB](https://www.rarlab.com/rar_add.htm), [WinZip format support](https://www.winzip.com/en/learn/file-formats/), [WinZip command-line add-on](https://www.winzip.com/en/product/command-line/), and [Tauri platform distribution](https://v2.tauri.app/distribute/).

The initial synthetic suite does not represent media-heavy collections, sparse files, very large archives, cold-cache runs, or GUI responsiveness. Add workload-specific runs and publish the machine, filesystem, OS, tool versions, exact settings, and cache conditions before drawing conclusions about those cases.
