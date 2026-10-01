# Benchmark protocol

SmartPack has no defensible comparative results yet. Do not publish an overall claim that it beats every compressor.

Record OS, CPU model, memory, filesystem, archive tool versions, command lines, profile, thread count, and warm/cold-cache conditions with every run. Use the same machine and input bytes for SmartPack, 7-Zip LZMA2, 7-Zip ZIP, Zstandard, and WinRAR when available.

The corpus must include source/text, media, already-compressed inputs, random data, many small files, duplicate trees, sparse files, and large files. Report archive bytes, create and extract wall time, CPU time, peak resident memory, and UI responsiveness. Include repeated runs and median plus spread. Keep the raw command output and hashes of each corpus fixture with the report.
