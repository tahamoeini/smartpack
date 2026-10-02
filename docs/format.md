# SmartPack archive compatibility

## SPK v3

New archives use magic `SPK3\r\n\x1a\n`, followed by little-endian version `3`, feature flags, and (for encrypted archives) a 16-byte salt plus bounded Argon2id parameters. Records are sequential and end with the `E` marker:

- `B`: 32-byte block identifier, method byte, uncompressed length, stored length, payload.
- `M`: uncompressed manifest length, stored length, manifest digest (all zero when encrypted), payload.
- `E`: end marker.

Data is split with rolling content-defined boundaries (64 KiB minimum, about 256 KiB average, 1 MiB maximum). SHA-256 identifiers deduplicate unencrypted blocks; encrypted v3 uses a keyed HMAC identifier to avoid exposing public plaintext hashes. Zstandard is the balanced/fast codec; LZMA2 in an XZ stream is used by Smallest; payloads are stored raw if compression is not beneficial or a recognized file signature indicates already-compressed content. Zero chunks are represented as manifest extents.

SPK readers reject blocks above 32 MiB, manifests above 64 MiB, more than two million blocks, more than one million entries, paths above 64 KiB, and expanded output above the default 1 TiB limit. XZ decoding is capped at 256 MiB of decoder memory. The 32 MiB block cap accommodates the original Python writer's largest built-in profile while bounding decompression allocations. The bundled external archive reader uses its own 4 GiB safe decoded-output budget.

Encryption is applied after compression to every stored data block and to the manifest. The v3 format uses Argon2id v1.3 (64 MiB, 3 iterations, one lane) and XChaCha20-Poly1305 with a unique nonce per record. KDF values are stored in the archive and rejected outside 8–256 MiB, 1–10 iterations, and 1–8 lanes before key derivation. Filenames and metadata live in the encrypted manifest. SPK v3 encrypted block identifiers are keyed.

## SPK v2

The engine reads and verifies the Python writer's `SPK2\r\n\x1a\n` version 2 records. It supports raw, zlib, bzip2, XZ/LZMA, Zstandard, and zero methods, along with the v2 compressed JSON manifest. New output is v3; no v3 read support is promised in older SmartPack releases.

## Interoperability and current limits

ZIP creation uses ZIP's Deflate method and supports ZIP64 through the bundled streaming archive implementation. The safe external reader currently uses the bundled archive library for ZIP, 7z, TAR and its enabled gzip, xz, zstd and lz4 filters. PAR2 creation and repair entry points are wired, but need damaged-file round-trip fixtures and cross-tool validation. RAR, standalone bzip2-filter extraction, ZIP WinZip AES creation, split volumes, selected-entry extraction, and full metadata restoration are not yet implemented. Unsupported archive features must be surfaced as errors rather than silently treated as verified. This explicit list is the current implementation boundary; CI and release work must expand it before claiming the full product scope.
