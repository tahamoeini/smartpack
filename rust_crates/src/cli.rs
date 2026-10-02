use smartpack_engine::{CompressionProfile, Engine, JobProgress};
use std::{error::Error, path::PathBuf};
use zeroize::Zeroize;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("smartpack: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return usage();
    };
    let engine = Engine::new();
    match command.as_str() {
        "--version" | "-V" => {
            println!("smartpack {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        "--help" | "-h" => {
            print_usage();
            return Ok(());
        }
        "create" | "pack" => {
            let source = required(&mut args, "source path")?;
            let destination = required(&mut args, "destination path")?;
            let mut profile = CompressionProfile::Balanced;
            let mut encrypt = false;
            while let Some(argument) = args.next() {
                match argument.as_str() {
                    "--encrypt" => encrypt = true,
                    "--profile" => {
                        profile = match args.next().as_deref() {
                            Some("fast") => CompressionProfile::Fast,
                            Some("balanced") => CompressionProfile::Balanced,
                            Some("smallest") => CompressionProfile::Smallest,
                            Some("store") => CompressionProfile::Store,
                            _ => {
                                return Err(
                                    "profile must be fast, balanced, smallest, or store".into()
                                )
                            }
                        }
                    }
                    _ => return Err(format!("unknown option: {argument}").into()),
                }
            }
            let mut password = if encrypt {
                let mut first = rpassword::prompt_password("Archive password: ")?;
                let mut confirm = rpassword::prompt_password("Confirm password: ")?;
                if first != confirm {
                    first.zeroize();
                    confirm.zeroize();
                    return Err("passwords did not match".into());
                }
                if first.is_empty() {
                    first.zeroize();
                    confirm.zeroize();
                    return Err("password cannot be empty".into());
                }
                confirm.zeroize();
                Some(first)
            } else {
                None
            };
            let result = engine.create(
                &PathBuf::from(source),
                &PathBuf::from(destination),
                profile,
                password.as_deref(),
                report_progress,
            );
            password.zeroize();
            result?;
            println!("Created archive successfully.");
        }
        "list" | "inspect" => {
            let archive = PathBuf::from(required(&mut args, "archive path")?);
            let mut password = optional_password(&archive)?;
            let result = engine.inspect(&archive, password.as_deref());
            password.zeroize();
            let entries = result?;
            println!("TYPE\tSIZE\tPATH");
            for entry in entries {
                println!("{}\t{}\t{}", entry.kind, entry.size, entry.path);
            }
        }
        "verify" => {
            let archive = PathBuf::from(required(&mut args, "archive path")?);
            let mut password = optional_password(&archive)?;
            let result = engine.verify(&archive, password.as_deref(), report_progress);
            password.zeroize();
            result?;
            println!("Verified OK.");
        }
        "extract" | "unpack" => {
            let archive = PathBuf::from(required(&mut args, "archive path")?);
            let destination = PathBuf::from(required(&mut args, "destination directory")?);
            let mut password = optional_password(&archive)?;
            let result =
                engine.extract(&archive, &destination, password.as_deref(), report_progress);
            password.zeroize();
            result?;
            println!("Extracted archive successfully.");
        }
        "recover" => match args.next().as_deref() {
            Some("create") => {
                let archive = PathBuf::from(required(&mut args, "archive path")?);
                let output_dir = PathBuf::from(required(&mut args, "recovery output directory")?);
                let percentage = args
                    .next()
                    .map(|value| value.parse::<u8>())
                    .transpose()?
                    .unwrap_or(10);
                let index = engine.create_recovery(&archive, &output_dir, percentage)?;
                println!("Created recovery set: {}", index.display());
            }
            _ => return usage(),
        },
        "repair" => {
            let index = PathBuf::from(required(&mut args, "PAR2 index file")?);
            let base = PathBuf::from(required(&mut args, "directory containing archive files")?);
            let destination = PathBuf::from(required(&mut args, "repaired output directory")?);
            let count = engine.repair_recovery(&index, &base, &destination)?;
            println!("Recovered {count} file(s) into {}", destination.display());
        }
        _ => return usage(),
    }
    Ok(())
}

fn optional_password(archive: &PathBuf) -> Result<Option<String>> {
    if Engine::archive_requires_password(archive)? {
        Ok(Some(rpassword::prompt_password("Archive password: ")?))
    } else {
        Ok(None)
    }
}

fn required(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next().ok_or_else(|| format!("missing {name}").into())
}

fn report_progress(progress: JobProgress) {
    if let Some(total) = progress.total_bytes {
        if total > 0 {
            let percent = progress.completed_bytes.saturating_mul(100) / total;
            eprint!("\r{}: {percent}%", progress.phase);
        }
    } else if let Some(path) = progress.current_path {
        eprint!("\r{}: {path}", progress.phase);
    }
    if let Some(rate) = progress.throughput_bytes_per_second {
        eprint!(" · {}/s", human_size(rate));
    }
}

fn human_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} TB")
}

fn usage<T>() -> Result<T> {
    print_usage();
    Err("invalid command line".into())
}

fn print_usage() {
    println!("SmartPack — cross-platform archive tools\n\n\
Usage:\n  smartpack create <source> <archive.spk|archive.zip> [--profile fast|balanced|smallest|store] [--encrypt]\n  smartpack list <archive.spk>\n  smartpack verify <archive>\n  smartpack extract <archive> <destination>\n  smartpack recover create <archive> <output-directory> [recovery-percent]\n  smartpack repair <index.par2> <source-directory> <repaired-output-directory>\n\n\
SPK v3 archives support optional password encryption. ZIP filenames remain visible.");
}
