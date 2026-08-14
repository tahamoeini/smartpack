use flate2::{write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use tar::Builder;

#[derive(Debug, Serialize, Deserialize)]
struct FileEntry {
    path: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    root: String,
    files: Vec<FileEntry>,
}

fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{:02x}", byte)).collect()
}

fn collect_files(root: &Path, base: &Path, files: &mut Vec<FileEntry>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, base, files)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            let bytes = fs::read(&path)?;
            files.push(FileEntry {
                path: rel,
                sha256: sha256_bytes(&bytes),
                size: bytes.len() as u64,
            });
        }
    }
    Ok(())
}

fn build_manifest(root: &Path) -> io::Result<Manifest> {
    let mut files = Vec::new();
    let base = root.parent().unwrap_or_else(|| Path::new("."));
    collect_files(root, base, &mut files)?;
    Ok(Manifest {
        format: "smartpack-rust-crates".to_string(),
        root: root.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        files,
    })
}

fn pack(source: &Path, output: &Path) -> io::Result<()> {
    if !source.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("source not found: {}", source.display())));
    }
    let file = File::create(output)?;
    let encoder = GzEncoder::new(file, Compression::best());
    let mut tar = Builder::new(encoder);
    tar.append_dir_all(source.file_name().unwrap_or_default(), source)?;
    tar.finish()?;

    let manifest = build_manifest(source)?;
    let manifest_path = output.with_file_name(format!("{}.manifest.json", output.file_name().unwrap().to_string_lossy()));
    let mut out = File::create(&manifest_path)?;
    out.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    println!("Packed: {} -> {}", source.display(), output.display());
    Ok(())
}

fn unpack(archive: &Path, output: &Path) -> io::Result<()> {
    let file = File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive_reader = tar::Archive::new(decoder);
    fs::create_dir_all(output)?;
    archive_reader.unpack(output)?;
    println!("Extracted: {} -> {}", archive.display(), output.display());
    Ok(())
}

fn verify(archive: &Path) -> io::Result<()> {
    let manifest_path = archive.with_file_name(format!("{}.manifest.json", archive.file_name().unwrap().to_string_lossy()));
    let manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    let file = File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive_reader = tar::Archive::new(decoder);
    for entry in archive_reader.entries()? {
        let entry = entry?;
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        let matched = manifest.files.iter().find(|item| item.path == path);
        if let Some(item) = matched {
            let mut bytes = Vec::new();
            let mut stream = entry;
            io::copy(&mut stream, &mut bytes)?;
            let digest = sha256_bytes(&bytes);
            if digest != item.sha256 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("sha256 mismatch for {path}")));
            }
        }
    }
    println!("Verified OK: {}", archive.display());
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let command = match args.next() {
        Some(cmd) => cmd,
        None => {
            eprintln!("usage: smartpack_rust_crates <pack|unpack|verify> <source|archive> [output]");
            std::process::exit(1);
        }
    };

    let result = match command.as_str() {
        "pack" => {
            let source = args.next().expect("missing source path");
            let output = args.next().expect("missing output path");
            pack(Path::new(&source), Path::new(&output))
        }
        "unpack" => {
            let archive = args.next().expect("missing archive path");
            let output = args.next().unwrap_or(".".to_string());
            unpack(Path::new(&archive), Path::new(&output))
        }
        "verify" => {
            let archive = args.next().expect("missing archive path");
            verify(Path::new(&archive))
        }
        _ => {
            eprintln!("usage: smartpack_rust_crates <pack|unpack|verify> <source|archive> [output]");
            std::process::exit(1);
        }
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
