use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn usage() -> String {
    "usage: smartpack_rust_native <pack|unpack|verify> <source|archive> [output]".to_string()
}

fn command_status(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("failed to run {:?}: {e}", cmd))?;
    if !status.success() {
        return Err(format!("command failed: {:?}", cmd));
    }
    Ok(())
}

fn walk_files(root: &Path, base: &Path, entries: &mut Vec<String>) -> Result<(), String> {
    if root.is_file() {
        let rel = root.strip_prefix(base).map_err(|e| e.to_string())?;
        entries.push(rel.to_string_lossy().replace('\\', "/"));
        return Ok(());
    }

    for entry in fs::read_dir(root).map_err(|e| format!("read_dir failed for {root:?}: {e}"))? {
        let entry = entry.map_err(|e| format!("read_dir entry failed: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, base, entries)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(base).map_err(|e| e.to_string())?;
            entries.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum").arg(path).output().map_err(|e| format!("sha256sum failed: {e}"))?;
    if !output.status.success() {
        return Err(format!("sha256sum failed for {:?}", path));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = stdout.split_whitespace().next().unwrap_or_default();
    Ok(hash.to_string())
}

fn pack(source: &Path, output: &Path) -> Result<(), String> {
    if !source.exists() {
        return Err(format!("source does not exist: {}", source.display()));
    }
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let archive_name = output.file_name().ok_or("output requires a filename")?;
    let mut tar = Command::new("tar");
    tar.arg("-C").arg(parent).arg("-czf").arg(output).arg(source.file_name().unwrap_or_default());
    command_status(&mut tar)?;

    let base = source.parent().unwrap_or_else(|| Path::new("."));
    let mut lines: Vec<String> = Vec::new();
    walk_files(source, base, &mut lines)?;
    let manifest_path = output.with_file_name(format!("{}.sha256", archive_name.to_string_lossy()));
    let mut text = String::new();
    text.push_str("# smartpack-rust-native\n");
    for rel in lines {
        let path = base.join(&rel);
        let digest = sha256_file(&path)?;
        let display = path.canonicalize().unwrap_or(path.clone());
        text.push_str(&format!("{digest}  {}\n", display.display()));
    }
    fs::write(&manifest_path, text).map_err(|e| format!("unable to write manifest: {e}"))?;
    println!("Packed: {} -> {}", source.display(), output.display());
    Ok(())
}

fn unpack(archive: &Path, output_dir: &Path) -> Result<(), String> {
    if !archive.exists() {
        return Err(format!("archive does not exist: {}", archive.display()));
    }
    fs::create_dir_all(output_dir).map_err(|e| format!("create_dir_all failed: {e}"))?;
    let mut tar = Command::new("tar");
    tar.arg("-xzf").arg(archive).arg("-C").arg(output_dir);
    command_status(&mut tar)?;
    println!("Extracted: {} -> {}", archive.display(), output_dir.display());
    Ok(())
}

fn verify(archive: &Path) -> Result<(), String> {
    let manifest_path = archive.with_file_name(format!("{}.sha256", archive.file_name().unwrap().to_string_lossy()));
    if !manifest_path.exists() {
        return Err(format!("missing manifest: {}", manifest_path.display()));
    }
    let output = Command::new("sha256sum")
        .arg("-c")
        .arg(&manifest_path)
        .output()
        .map_err(|e| format!("sha256sum verification failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("verification failed for {}: {}", archive.display(), stderr));
    }
    println!("Verified OK: {}", archive.display());
    Ok(())
}

fn main() {
    let mut args = env::args().skip(1);
    let command = match args.next() {
        Some(value) => value,
        None => {
            eprintln!("{}", usage());
            std::process::exit(1);
        }
    };

    let result = match command.as_str() {
        "pack" => {
            let source = args.next().unwrap_or_else(|| {
                eprintln!("{}", usage());
                std::process::exit(1);
            });
            let output = args.next().unwrap_or_else(|| {
                eprintln!("{}", usage());
                std::process::exit(1);
            });
            pack(Path::new(&source), Path::new(&output))
        }
        "unpack" => {
            let archive = args.next().unwrap_or_else(|| {
                eprintln!("{}", usage());
                std::process::exit(1);
            });
            let output_dir = args.next().unwrap_or_else(|| ".".to_string());
            unpack(Path::new(&archive), Path::new(&output_dir))
        }
        "verify" => {
            let archive = args.next().unwrap_or_else(|| {
                eprintln!("{}", usage());
                std::process::exit(1);
            });
            verify(Path::new(&archive))
        }
        _ => {
            eprintln!("{}", usage());
            std::process::exit(1);
        }
    };

    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}
