//! SmartPack's shared archive engine.
//!
//! SPK v3 is a streaming, content-defined, deduplicating format. The reader
//! also accepts SmartPack v2 archives produced by the original Python tool.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use flate2::read::ZlibDecoder;
use hmac::{Hmac, Mac};
use lzma_rust2::{XzReader, XzWriter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Instant;
use std::{
    collections::HashMap,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::UNIX_EPOCH,
};
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;
use zeroize::Zeroizing;

const SPK2_MAGIC: &[u8; 8] = b"SPK2\r\n\x1a\n";
const SPK3_MAGIC: &[u8; 8] = b"SPK3\r\n\x1a\n";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BLOCK_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LZMA_MEMORY_KIB: u32 = 256 * 1024;
const MAX_BLOCKS: usize = 2_000_000;
const MAX_MANIFEST_ENTRIES: usize = 1_000_000;
const MAX_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_PAR2_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_KDF_MEMORY_KIB: u32 = 64 * 1024;
const DEFAULT_KDF_ITERATIONS: u32 = 3;
const DEFAULT_KDF_LANES: u32 = 1;
const CDC_MIN: usize = 64 * 1024;
const CDC_MAX: usize = 1024 * 1024;
const CDC_MASK: u64 = (1 << 18) - 1;
const SMALL_FILE_LIMIT: u64 = 4096;
const SMALL_SOLID_LIMIT: usize = 256 * 1024;
type EngineResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompressionProfile {
    Fast,
    #[default]
    Balanced,
    Smallest,
    Store,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobProgress {
    pub phase: String,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub current_path: Option<String>,
    pub throughput_bytes_per_second: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    #[serde(rename = "type", alias = "kind")]
    pub kind: String,
    pub size: u64,
    #[serde(default, rename = "mtime_ns", alias = "modified_unix")]
    pub modified_ns: Option<u64>,
    pub mode: Option<u32>,
    #[serde(default)]
    segments: Vec<Segment>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Segment {
    Block {
        block: String,
        offset: u64,
        length: u64,
    },
    Zero {
        zero: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    entries: Vec<ArchiveEntry>,
}

#[derive(Debug, Clone, Copy)]
struct BlockInfo {
    offset: u64,
    method: u8,
    raw_size: u64,
    stored_size: u64,
}

#[derive(Debug)]
struct ArchiveIndex {
    version: u32,
    key: Option<Zeroizing<[u8; 32]>>,
    blocks: HashMap<String, BlockInfo>,
    manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct Engine {
    cancel: Arc<AtomicBool>,
    par_cancel: Arc<std::sync::Mutex<Option<parmesan::create::CreateCancellation>>>,
    max_output_bytes: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            par_cancel: Arc::new(std::sync::Mutex::new(None)),
            max_output_bytes: MAX_OUTPUT_BYTES,
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(guard) = self.par_cancel.lock() {
            if let Some(cancellation) = guard.as_ref() {
                cancellation.cancel();
            }
        }
    }

    pub fn reset_cancel(&self) {
        self.cancel.store(false, Ordering::Relaxed);
    }

    #[must_use]
    pub fn with_max_output_bytes(mut self, maximum: u64) -> Self {
        self.max_output_bytes = maximum;
        self
    }

    pub fn archive_requires_password(archive: &Path) -> EngineResult<bool> {
        let mut file = File::open(archive)?;
        let mut magic = [0_u8; 8];
        match file.read_exact(&mut magic) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        if &magic != SPK3_MAGIC {
            return Ok(false);
        }
        if read_u32(&mut file)? != 3 {
            return Ok(false);
        }
        let mut flags = [0_u8; 1];
        file.read_exact(&mut flags)?;
        Ok(flags[0] & 1 == 1)
    }

    pub fn create(
        &self,
        source: &Path,
        destination: &Path,
        profile: CompressionProfile,
        password: Option<&str>,
        mut progress: impl FnMut(JobProgress),
    ) -> EngineResult<()> {
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination already exists; choose another path",
            )
            .into());
        }
        if password == Some("") {
            return Err(invalid("archive password cannot be empty"));
        }
        if destination
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            if password.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "encrypted ZIP creation is not available in this build",
                )
                .into());
            }
            return create_zip(source, destination, &self.cancel, &mut progress);
        }
        create_spk(
            source,
            destination,
            profile,
            password,
            &self.cancel,
            &mut progress,
        )
    }

    pub fn inspect(
        &self,
        archive: &Path,
        password: Option<&str>,
    ) -> EngineResult<Vec<ArchiveEntry>> {
        if !is_smartpack(archive)? {
            return inspect_external(archive);
        }
        let index = read_spk_index(archive, password)?;
        validate_manifest(
            &index.manifest,
            &index.blocks,
            index.version,
            self.max_output_bytes,
        )?;
        Ok(index.manifest.entries)
    }

    pub fn verify(
        &self,
        archive: &Path,
        password: Option<&str>,
        mut progress: impl FnMut(JobProgress),
    ) -> EngineResult<()> {
        if !is_smartpack(archive)? {
            return verify_external(archive, &self.cancel, &mut progress);
        }
        let index = read_spk_index(archive, password)?;
        let total = index
            .blocks
            .values()
            .map(|block| block.raw_size)
            .sum::<u64>();
        let mut input = File::open(archive)?;
        let started = Instant::now();
        let mut completed = 0_u64;
        for (id, block) in &index.blocks {
            check_cancel(&self.cancel)?;
            let data = load_block(
                &mut input,
                *block,
                id,
                index.version,
                key_ref(index.key.as_ref()),
            )?;
            if data.len() as u64 != block.raw_size {
                return Err(invalid("block size mismatch"));
            }
            completed = completed.saturating_add(block.raw_size);
            progress(JobProgress {
                phase: "verify".into(),
                completed_bytes: completed,
                total_bytes: Some(total),
                current_path: None,
                throughput_bytes_per_second: Some(throughput(completed, started)),
            });
        }
        validate_manifest(
            &index.manifest,
            &index.blocks,
            index.version,
            self.max_output_bytes,
        )?;
        Ok(())
    }

    pub fn extract(
        &self,
        archive: &Path,
        destination: &Path,
        password: Option<&str>,
        mut progress: impl FnMut(JobProgress),
    ) -> EngineResult<()> {
        if !is_smartpack(archive)? {
            return extract_external(
                archive,
                destination,
                self.max_output_bytes,
                &self.cancel,
                &mut progress,
            );
        }
        if !destination.exists() {
            fs::create_dir_all(destination)?;
        }
        if !destination.is_dir() {
            return Err(invalid("extraction destination is not a directory"));
        }
        let index = read_spk_index(archive, password)?;
        validate_manifest(
            &index.manifest,
            &index.blocks,
            index.version,
            self.max_output_bytes,
        )?;
        extract_spk(archive, destination, &index, &self.cancel, &mut progress)
    }

    pub fn create_recovery(
        &self,
        archive: &Path,
        output_dir: &Path,
        percentage: u8,
    ) -> EngineResult<PathBuf> {
        check_cancel(&self.cancel)?;
        if !(1..=100).contains(&percentage) {
            return Err(invalid("recovery percentage must be between 1 and 100"));
        }
        let base = archive
            .file_name()
            .ok_or_else(|| invalid("archive filename is missing"))?
            .to_string_lossy()
            .into_owned();
        let request = parmesan::create::CreateRequest::from_paths([archive])
            .output_dir(output_dir)
            .base_name(base)
            .recovery(parmesan::create::Recovery::Percentage(percentage))
            .memory_limit(512 * 1024 * 1024);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let cancellation = parmesan::create::CreateCancellation::new();
        *self
            .par_cancel
            .lock()
            .map_err(|_| invalid("PAR2 job state is unavailable"))? = Some(cancellation.clone());
        if self.cancel.load(Ordering::Relaxed) {
            cancellation.cancel();
        }
        let result = runtime.block_on(parmesan::create::create_cancellable(request, &cancellation));
        if let Ok(mut guard) = self.par_cancel.lock() {
            *guard = None;
        }
        let report = result?;
        report
            .index_path
            .ok_or_else(|| invalid("PAR2 creator did not return an index path"))
    }

    pub fn repair_recovery(
        &self,
        index_file: &Path,
        base_dir: &Path,
        repaired_dir: &Path,
    ) -> EngineResult<usize> {
        fs::create_dir_all(repaired_dir)?;
        let repaired_root = repaired_dir.canonicalize()?;
        let mut set = parmesan::recovery_set::RecoverySet::load_metadata(index_file)?;
        let report = parmesan::verify::verify(&set, base_dir)?;
        if report.is_ok() {
            return Ok(0);
        }
        if !report.is_repairable() {
            return Err(invalid(
                "PAR2 recovery set has insufficient recovery blocks",
            ));
        }
        preflight_par2_outputs(&repaired_root, &set, &report)?;
        let needed_blocks = u64::try_from(report.total_bad_slices())?;
        let recovery_memory = needed_blocks
            .checked_mul(set.slice_size)
            .ok_or_else(|| invalid("PAR2 repair memory estimate overflow"))?;
        if recovery_memory > MAX_PAR2_MEMORY_BYTES {
            return Err(invalid("PAR2 repair exceeds the configured memory budget"));
        }
        set.load_recovery_blocks(Some(report.total_bad_slices()))?;
        let outcome = parmesan::repair::repair(
            &set,
            &report,
            base_dir,
            &parmesan::repair::RepairOptions {
                out_dir: Some(repaired_dir.to_path_buf()),
                dry_run: false,
            },
        )?;
        Ok(outcome.repaired_files.len())
    }
}

fn preflight_par2_outputs(
    root: &Path,
    set: &parmesan::recovery_set::RecoverySet,
    report: &parmesan::verify::VerifyReport,
) -> EngineResult<()> {
    if set.files.len() != report.files.len() {
        return Err(invalid(
            "PAR2 recovery set and verification report disagree",
        ));
    }
    let mut destinations = std::collections::HashSet::new();
    for (file, status) in set.files.iter().zip(&report.files) {
        if status.status == parmesan::verify::FileStatus::Ok {
            continue;
        }
        if !safe_archive_path(&file.name) || !destinations.insert(normalized_path_key(&file.name)) {
            return Err(invalid("PAR2 output has an unsafe or colliding file name"));
        }
        let destination = parmesan::recovery_set::contained_path(root, &file.name)
            .map_err(|_| invalid("PAR2 output path escapes the selected repair directory"))?;
        let relative = destination
            .strip_prefix(root)
            .map_err(|_| invalid("PAR2 output path escapes the selected repair directory"))?;
        let components = relative.components().collect::<Vec<_>>();
        let mut current = root.to_path_buf();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(invalid("PAR2 output path is not relative"));
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(_) if index + 1 == components.len() => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("PAR2 repair output already exists: {}", current.display()),
                    )
                    .into());
                }
                Ok(metadata) if is_reparse_point(&metadata) || !metadata.is_dir() => {
                    return Err(invalid("PAR2 repair path crosses a link or non-directory"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn invalid(message: &str) -> Box<dyn Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

fn check_cancel(cancel: &AtomicBool) -> EngineResult<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "operation cancelled").into())
    } else {
        Ok(())
    }
}

fn throughput(bytes: u64, started: Instant) -> u64 {
    let seconds = started.elapsed().as_secs_f64().max(0.001);
    (bytes as f64 / seconds).min(u64::MAX as f64) as u64
}

fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn derive_key(
    password: &str,
    salt: &[u8; 16],
    memory: u32,
    iterations: u32,
    lanes: u32,
) -> EngineResult<Zeroizing<[u8; 32]>> {
    if !(8 * 1024..=256 * 1024).contains(&memory)
        || !(1..=10).contains(&iterations)
        || !(1..=8).contains(&lanes)
    {
        return Err(invalid("archive KDF parameters exceed safe bounds"));
    }
    let params = Params::new(memory, iterations, lanes, Some(32))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; 32]);
    argon.hash_password_into(password.as_bytes(), salt, &mut *key)?;
    Ok(key)
}

fn key_ref(key: Option<&Zeroizing<[u8; 32]>>) -> Option<&[u8; 32]> {
    key.map(|secret| &**secret)
}

fn seal(key: &[u8; 32], plain: &[u8], associated_data: &[u8]) -> EngineResult<Vec<u8>> {
    let mut nonce = [0_u8; 24];
    getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
    let cipher = XChaCha20Poly1305::new_from_slice(key)?;
    let encrypted = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: associated_data,
            },
        )
        .map_err(|_| invalid("failed to encrypt archive data"))?;
    let mut output = Vec::with_capacity(nonce.len() + encrypted.len());
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&encrypted);
    Ok(output)
}

fn unseal(key: &[u8; 32], stored: &[u8], associated_data: &[u8]) -> EngineResult<Vec<u8>> {
    if stored.len() < 24 {
        return Err(invalid("truncated encrypted archive record"));
    }
    let cipher = XChaCha20Poly1305::new_from_slice(key)?;
    cipher
        .decrypt(
            XNonce::from_slice(&stored[..24]),
            Payload {
                msg: &stored[24..],
                aad: associated_data,
            },
        )
        .map_err(|_| invalid("wrong password or damaged archive"))
}

fn keyed_id(key: &[u8; 32], sha: &[u8; 32]) -> EngineResult<[u8; 32]> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key)?;
    mac.update(b"SmartPack v3 block id\0");
    mac.update(sha);
    Ok(mac.finalize().into_bytes().into())
}

fn file_kind(path: &Path) -> String {
    if path.is_dir() {
        "directory".into()
    } else {
        "file".into()
    }
}

fn safe_archive_path(path: &str) -> bool {
    let path = Path::new(path);
    let raw = path.to_string_lossy();
    let without_trailing_slash = raw.strip_suffix('/').unwrap_or(&raw);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !raw.contains('\\')
        && !raw.contains('\0')
        && !without_trailing_slash.is_empty()
        && without_trailing_slash.split('/').all(|part| {
            if part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with('.')
                || part.ends_with(' ')
                || part
                    .chars()
                    .any(|character| character.is_control() || "<>:\"|?*".contains(character))
            {
                return false;
            }
            let device = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
            !matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !((device.starts_with("COM") || device.starts_with("LPT"))
                    && device.len() == 4
                    && matches!(device.as_bytes()[3], b'1'..=b'9'))
        })
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn normalized_path_key(path: &str) -> String {
    path.trim_end_matches('/')
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

fn create_spk(
    source: &Path,
    destination: &Path,
    profile: CompressionProfile,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(JobProgress),
) -> EngineResult<()> {
    if !source.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "source does not exist").into());
    }
    let base = if source.is_dir() {
        source.parent().unwrap_or(source)
    } else {
        source.parent().unwrap_or(Path::new("."))
    };
    let walk_root = source;
    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;
    let temp_path = temp_sibling(destination);
    for item in WalkDir::new(walk_root)
        .follow_links(false)
        .sort_by_file_name()
    {
        let item = item?;
        if item.path() == temp_path {
            continue;
        }
        if item.file_type().is_symlink() {
            continue;
        }
        if item.file_type().is_file() {
            total_bytes = total_bytes.saturating_add(item.metadata()?.len());
        }
    }
    let parent = parent_directory(destination);
    fs::create_dir_all(parent)?;
    let mut owns_temp = false;
    let result = (|| -> EngineResult<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        owns_temp = true;
        output.write_all(SPK3_MAGIC)?;
        output.write_all(&3_u32.to_le_bytes())?;
        let encrypted = password.is_some();
        output.write_all(&[u8::from(encrypted)])?;
        let mut salt = [0_u8; 16];
        let key = if let Some(password) = password {
            getrandom::fill(&mut salt).map_err(|error| io::Error::other(error.to_string()))?;
            output.write_all(&salt)?;
            output.write_all(&DEFAULT_KDF_MEMORY_KIB.to_le_bytes())?;
            output.write_all(&DEFAULT_KDF_ITERATIONS.to_le_bytes())?;
            output.write_all(&DEFAULT_KDF_LANES.to_le_bytes())?;
            Some(derive_key(
                password,
                &salt,
                DEFAULT_KDF_MEMORY_KIB,
                DEFAULT_KDF_ITERATIONS,
                DEFAULT_KDF_LANES,
            )?)
        } else {
            output.write_all(&[0_u8; 28])?;
            None
        };
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut solid_buffers: HashMap<&'static str, Vec<u8>> = HashMap::new();
        let mut solid_members: HashMap<&'static str, Vec<(usize, usize, usize)>> = HashMap::new();
        solid_buffers.insert("text", Vec::with_capacity(SMALL_SOLID_LIMIT));
        solid_buffers.insert("binary", Vec::with_capacity(SMALL_SOLID_LIMIT));
        solid_members.insert("text", Vec::new());
        solid_members.insert("binary", Vec::new());
        let mut source_paths = std::collections::HashSet::new();
        let mut processed = 0_u64;
        let started = Instant::now();
        for item in WalkDir::new(walk_root)
            .follow_links(false)
            .sort_by_file_name()
        {
            check_cancel(cancel)?;
            let item = item?;
            if item.path() == temp_path {
                continue;
            }
            if item.file_type().is_symlink() {
                continue;
            }
            let absolute = item.path();
            let relative = absolute
                .strip_prefix(base)?
                .to_string_lossy()
                .replace('\\', "/");
            if !safe_archive_path(&relative) {
                return Err(invalid("source contains a non-portable path"));
            }
            if entries.len() >= MAX_MANIFEST_ENTRIES {
                return Err(invalid("archive has too many filesystem entries"));
            }
            if !source_paths.insert(normalized_path_key(&relative)) {
                return Err(invalid("source has duplicate or case-colliding paths"));
            }
            let metadata = item.metadata()?;
            let modified_unix = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            let mut entry = ArchiveEntry {
                path: relative,
                kind: file_kind(absolute),
                size: if item.file_type().is_file() {
                    metadata.len()
                } else {
                    0
                },
                modified_ns: modified_unix.map(|seconds| seconds.saturating_mul(1_000_000_000)),
                mode: file_mode(&metadata),
                segments: Vec::new(),
                target: None,
            };
            if item.file_type().is_file() && metadata.len() <= SMALL_FILE_LIMIT {
                let mut input = File::open(absolute)?;
                let mut bytes = Vec::new();
                Read::by_ref(&mut input)
                    .take(SMALL_FILE_LIMIT + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 <= SMALL_FILE_LIMIT {
                    if bytes.len() as u64 != entry.size {
                        return Err(invalid("source file changed while it was being archived"));
                    }
                    processed = processed.saturating_add(bytes.len() as u64);
                    if bytes.is_empty() {
                        entries.push(entry);
                        continue;
                    }
                    let group = if looks_like_text(&bytes) {
                        "text"
                    } else {
                        "binary"
                    };
                    let buffer = solid_buffers
                        .get_mut(group)
                        .expect("solid group initialized");
                    if buffer.len().saturating_add(bytes.len()) > SMALL_SOLID_LIMIT
                        && !buffer.is_empty()
                    {
                        flush_solid_group(
                            group,
                            &mut output,
                            profile,
                            key_ref(key.as_ref()),
                            &mut seen,
                            &mut solid_buffers,
                            &mut solid_members,
                            &mut entries,
                        )?;
                    }
                    let buffer = solid_buffers
                        .get_mut(group)
                        .expect("solid group initialized");
                    let offset = buffer.len();
                    buffer.extend_from_slice(&bytes);
                    let entry_index = entries.len();
                    entries.push(entry);
                    solid_members
                        .get_mut(group)
                        .expect("solid members initialized")
                        .push((entry_index, offset, bytes.len()));
                    if buffer.len() >= SMALL_SOLID_LIMIT {
                        flush_solid_group(
                            group,
                            &mut output,
                            profile,
                            key_ref(key.as_ref()),
                            &mut seen,
                            &mut solid_buffers,
                            &mut solid_members,
                            &mut entries,
                        )?;
                    }
                    progress(JobProgress {
                        phase: "compress".into(),
                        completed_bytes: processed,
                        total_bytes: Some(total_bytes),
                        current_path: Some(absolute.display().to_string()),
                        throughput_bytes_per_second: Some(throughput(processed, started)),
                    });
                    continue;
                }
                return Err(invalid("source file changed while it was being archived"));
            }
            if item.file_type().is_file() {
                let mut input = File::open(absolute)?;
                let mut buf = [0_u8; 64 * 1024];
                let mut chunk = Vec::with_capacity(CDC_MAX);
                let mut fingerprint = 0_u64;
                let mut entry_read = 0_u64;
                loop {
                    check_cancel(cancel)?;
                    let n = input.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    entry_read = entry_read.saturating_add(n as u64);
                    processed = processed.saturating_add(n as u64);
                    for &byte in &buf[..n] {
                        chunk.push(byte);
                        fingerprint = fingerprint.rotate_left(1).wrapping_add(gear(byte));
                        if chunk.len() >= CDC_MIN
                            && ((fingerprint & CDC_MASK) == 0 || chunk.len() >= CDC_MAX)
                        {
                            let segment = write_block(
                                &mut output,
                                &chunk,
                                profile,
                                key_ref(key.as_ref()),
                                &mut seen,
                            )?;
                            entry.segments.push(segment);
                            chunk.clear();
                            fingerprint = 0;
                        }
                    }
                    progress(JobProgress {
                        phase: "compress".into(),
                        completed_bytes: processed,
                        total_bytes: Some(total_bytes),
                        current_path: Some(entry.path.clone()),
                        throughput_bytes_per_second: Some(throughput(processed, started)),
                    });
                }
                if !chunk.is_empty() {
                    let segment = write_block(
                        &mut output,
                        &chunk,
                        profile,
                        key_ref(key.as_ref()),
                        &mut seen,
                    )?;
                    entry.segments.push(segment);
                }
                if entry_read != entry.size {
                    return Err(invalid("source file changed while it was being archived"));
                }
            }
            entries.push(entry);
        }
        for group in ["text", "binary"] {
            flush_solid_group(
                group,
                &mut output,
                profile,
                key_ref(key.as_ref()),
                &mut seen,
                &mut solid_buffers,
                &mut solid_members,
                &mut entries,
            )?;
        }
        let manifest = Manifest {
            format: "SmartPack".into(),
            version: 3,
            entries,
        };
        let plain = serde_json::to_vec(&manifest)?;
        if plain.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(invalid(
                "archive manifest exceeds the configured size limit",
            ));
        }
        let compressed = zstd::bulk::compress(&plain, 3)?;
        let stored = match key_ref(key.as_ref()) {
            Some(key) => seal(key, &compressed, b"SPK3 manifest")?,
            None => compressed,
        };
        output.write_all(b"M")?;
        output.write_all(&(plain.len() as u64).to_le_bytes())?;
        output.write_all(&(stored.len() as u64).to_le_bytes())?;
        output.write_all(&if encrypted {
            [0_u8; 32]
        } else {
            digest(&plain)
        })?;
        output.write_all(&stored)?;
        output.write_all(b"E")?;
        output.sync_all()?;
        Ok(())
    })();
    match result {
        Ok(()) => match check_cancel(cancel) {
            Ok(()) => publish_file(&temp_path, destination),
            Err(error) => {
                if owns_temp {
                    let _ = fs::remove_file(&temp_path);
                }
                Err(error)
            }
        },
        Err(error) => {
            if owns_temp {
                let _ = fs::remove_file(&temp_path);
            }
            Err(error)
        }
    }
}

fn file_mode(metadata: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

fn gear(byte: u8) -> u64 {
    let mut value = u64::from(byte).wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn looks_like_text(data: &[u8]) -> bool {
    std::str::from_utf8(data).is_ok()
        && !data.iter().any(|byte| *byte == 0)
        && data
            .iter()
            .filter(|byte| byte.is_ascii_control() && !matches!(**byte, b'\n' | b'\r' | b'\t'))
            .count()
            * 20
            < data.len().max(1)
}

fn flush_solid_group(
    group: &'static str,
    output: &mut File,
    profile: CompressionProfile,
    key: Option<&[u8; 32]>,
    seen: &mut HashMap<String, ()>,
    buffers: &mut HashMap<&'static str, Vec<u8>>,
    members: &mut HashMap<&'static str, Vec<(usize, usize, usize)>>,
    entries: &mut [ArchiveEntry],
) -> EngineResult<()> {
    let buffer = buffers
        .get_mut(group)
        .ok_or_else(|| invalid("unknown solid group"))?;
    if buffer.is_empty() {
        return Ok(());
    }
    let content = std::mem::take(buffer);
    let locations = std::mem::take(
        members
            .get_mut(group)
            .ok_or_else(|| invalid("solid references are missing"))?,
    );
    let segment = write_block(output, &content, profile, key, seen)?;
    for (entry_index, offset, length) in locations {
        match &segment {
            Segment::Block { block, .. } => entries
                .get_mut(entry_index)
                .ok_or_else(|| invalid("solid file index is invalid"))?
                .segments
                .push(Segment::Block {
                    block: block.clone(),
                    offset: offset as u64,
                    length: length as u64,
                }),
            Segment::Zero { .. } => entries
                .get_mut(entry_index)
                .ok_or_else(|| invalid("solid file index is invalid"))?
                .segments
                .push(Segment::Zero {
                    zero: length as u64,
                }),
        }
    }
    Ok(())
}

fn is_likely_compressed(data: &[u8]) -> bool {
    [
        b"PK\x03\x04".as_slice(),
        b"\x1f\x8b",
        b"BZh",
        b"\xfd7zXZ\0",
        b"\x28\xb5\x2f\xfd",
        b"\x89PNG\r\n\x1a\n",
        b"\xff\xd8\xff",
        b"%PDF",
        b"OggS",
        b"RIFF",
    ]
    .iter()
    .any(|signature| data.starts_with(signature))
}

fn write_block(
    output: &mut File,
    data: &[u8],
    profile: CompressionProfile,
    key: Option<&[u8; 32]>,
    seen: &mut HashMap<String, ()>,
) -> EngineResult<Segment> {
    if data.iter().all(|byte| *byte == 0) {
        return Ok(Segment::Zero {
            zero: data.len() as u64,
        });
    }
    let plain_hash = digest(data);
    let block_id = match key {
        Some(key) => keyed_id(key, &plain_hash)?,
        None => plain_hash,
    };
    let id = hex::encode(block_id);
    if seen.contains_key(&id) {
        return Ok(Segment::Block {
            block: id,
            offset: 0,
            length: data.len() as u64,
        });
    }
    if seen.len() >= MAX_BLOCKS {
        return Err(invalid("archive exceeds maximum unique block count"));
    }
    let encryption_overhead = if key.is_some() { 24 + 16 } else { 0 };
    let saves_space =
        |compressed: &[u8]| compressed.len().saturating_add(encryption_overhead) < data.len();
    let (method, compressed) = match profile {
        CompressionProfile::Store => (0_u8, data.to_vec()),
        CompressionProfile::Smallest if !is_likely_compressed(data) => {
            let mut encoder = XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(6))?;
            encoder.write_all(data)?;
            let encoded = encoder.finish()?;
            if saves_space(&encoded) {
                (6, encoded)
            } else {
                (0, data.to_vec())
            }
        }
        profile if !is_likely_compressed(data) => {
            let level = if profile == CompressionProfile::Fast {
                1
            } else {
                3
            };
            let encoded = zstd::bulk::compress(data, level)?;
            if saves_space(&encoded) {
                (4, encoded)
            } else {
                (0, data.to_vec())
            }
        }
        _ => (0, data.to_vec()),
    };
    let associated = block_aad(&block_id, data.len() as u64, method);
    let payload = match key {
        Some(key) => seal(key, &compressed, &associated)?,
        None => compressed.to_vec(),
    };
    output.write_all(b"B")?;
    output.write_all(&block_id)?;
    output.write_all(&[method])?;
    output.write_all(&(data.len() as u64).to_le_bytes())?;
    output.write_all(&(payload.len() as u64).to_le_bytes())?;
    output.write_all(&payload)?;
    seen.insert(id.clone(), ());
    Ok(Segment::Block {
        block: id,
        offset: 0,
        length: data.len() as u64,
    })
}

fn block_aad(id: &[u8; 32], raw_size: u64, method: u8) -> Vec<u8> {
    let mut aad = b"SPK3 block".to_vec();
    aad.extend_from_slice(id);
    aad.extend_from_slice(&raw_size.to_le_bytes());
    aad.push(method);
    aad
}

fn read_spk_index(path: &Path, password: Option<&str>) -> EngineResult<ArchiveIndex> {
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)?;
    let version = read_u32(&mut file)?;
    if (&magic != SPK2_MAGIC || version != 2) && (&magic != SPK3_MAGIC || version != 3) {
        return Err(invalid("not a supported SmartPack archive"));
    }
    let mut key = None;
    if version == 3 {
        let mut flags = [0_u8; 1];
        file.read_exact(&mut flags)?;
        if flags[0] & !1 != 0 {
            return Err(invalid("unsupported SPK v3 feature flags"));
        }
        if flags[0] & 1 == 1 {
            let mut salt = [0_u8; 16];
            file.read_exact(&mut salt)?;
            let memory = read_u32(&mut file)?;
            let iterations = read_u32(&mut file)?;
            let lanes = read_u32(&mut file)?;
            let password = password.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "this archive requires a password",
                )
            })?;
            key = Some(derive_key(password, &salt, memory, iterations, lanes)?);
        } else {
            let mut unused = [0_u8; 28];
            file.read_exact(&mut unused)?;
            if unused.iter().any(|byte| *byte != 0) {
                return Err(invalid("invalid unencrypted SPK v3 header"));
            }
        }
    }
    let mut blocks = HashMap::new();
    loop {
        let mut tag = [0_u8; 1];
        if file.read(&mut tag)? != 1 {
            return Err(invalid("archive ended before manifest"));
        }
        match tag[0] {
            b'B' => {
                let mut id = [0_u8; 32];
                file.read_exact(&mut id)?;
                let mut method = [0_u8; 1];
                file.read_exact(&mut method)?;
                let raw_size = read_u64(&mut file)?;
                let stored_size = read_u64(&mut file)?;
                if raw_size > MAX_BLOCK_BYTES || stored_size > MAX_BLOCK_BYTES + 64 {
                    return Err(invalid("archive block exceeds configured size limit"));
                }
                let offset = file.stream_position()?;
                let end = offset
                    .checked_add(stored_size)
                    .ok_or_else(|| invalid("archive record offset overflow"))?;
                if end > file.metadata()?.len() {
                    return Err(invalid("truncated archive block"));
                }
                if blocks
                    .insert(
                        hex::encode(id),
                        BlockInfo {
                            offset,
                            method: method[0],
                            raw_size,
                            stored_size,
                        },
                    )
                    .is_some()
                {
                    return Err(invalid("duplicate block record"));
                }
                if blocks.len() > MAX_BLOCKS {
                    return Err(invalid("archive exceeds maximum block count"));
                }
                file.seek(SeekFrom::Start(end))?;
            }
            b'M' => {
                let raw_size = read_u64(&mut file)?;
                let stored_size = read_u64(&mut file)?;
                let mut expected = [0_u8; 32];
                file.read_exact(&mut expected)?;
                if raw_size > MAX_MANIFEST_BYTES || stored_size > MAX_MANIFEST_BYTES + 1024 {
                    return Err(invalid("archive manifest exceeds configured size limit"));
                }
                let mut stored = vec![0_u8; stored_size as usize];
                file.read_exact(&mut stored)?;
                let compressed = match key_ref(key.as_ref()) {
                    Some(key) => unseal(key, &stored, b"SPK3 manifest")?,
                    None => stored,
                };
                let plain = if version == 3 {
                    zstd::bulk::decompress(&compressed, raw_size as usize)?
                } else {
                    let decoder =
                        XzReader::new_mem_limit(compressed.as_slice(), false, MAX_LZMA_MEMORY_KIB);
                    let mut bytes = Vec::with_capacity(raw_size as usize);
                    decoder
                        .take(raw_size.saturating_add(1))
                        .read_to_end(&mut bytes)?;
                    bytes
                };
                if plain.len() as u64 != raw_size {
                    return Err(invalid("manifest size mismatch"));
                }
                if expected != [0_u8; 32] && digest(&plain) != expected {
                    return Err(invalid("manifest integrity check failed"));
                }
                let manifest: Manifest = serde_json::from_slice(&plain)?;
                let mut end = [0_u8; 1];
                file.read_exact(&mut end)?;
                if end[0] != b'E' {
                    return Err(invalid("missing SmartPack end marker"));
                }
                let mut trailing = [0_u8; 1];
                if file.read(&mut trailing)? != 0 {
                    return Err(invalid("unexpected data after SmartPack end marker"));
                }
                return Ok(ArchiveIndex {
                    version,
                    key,
                    blocks,
                    manifest,
                });
            }
            _ => return Err(invalid("unknown SmartPack record tag")),
        }
    }
}

fn load_block(
    file: &mut File,
    block: BlockInfo,
    id: &str,
    version: u32,
    key: Option<&[u8; 32]>,
) -> EngineResult<Vec<u8>> {
    file.seek(SeekFrom::Start(block.offset))?;
    let mut stored = vec![0_u8; block.stored_size as usize];
    file.read_exact(&mut stored)?;
    let compressed = if version == 3 {
        match key {
            Some(key) => unseal(
                key,
                &stored,
                &block_aad(
                    &hex::decode(id)?
                        .try_into()
                        .map_err(|_| invalid("invalid block identifier"))?,
                    block.raw_size,
                    block.method,
                ),
            )?,
            None => stored,
        }
    } else {
        stored
    };
    let output = match block.method {
        0 => compressed,
        1 => {
            let mut decoder = ZlibDecoder::new(compressed.as_slice());
            bounded_read(&mut decoder, block.raw_size)?
        }
        2 => {
            let mut decoder = bzip2::read::BzDecoder::new(compressed.as_slice());
            bounded_read(&mut decoder, block.raw_size)?
        }
        3 => {
            let mut decoder =
                XzReader::new_mem_limit(compressed.as_slice(), false, MAX_LZMA_MEMORY_KIB);
            bounded_read(&mut decoder, block.raw_size)?
        }
        4 => zstd::bulk::decompress(&compressed, block.raw_size as usize)?,
        5 => vec![0_u8; block.raw_size as usize],
        6 if version == 3 => {
            let mut decoder =
                XzReader::new_mem_limit(compressed.as_slice(), false, MAX_LZMA_MEMORY_KIB);
            bounded_read(&mut decoder, block.raw_size)?
        }
        _ => return Err(invalid("unsupported SmartPack block compression method")),
    };
    if output.len() as u64 != block.raw_size {
        return Err(invalid("decompressed block size mismatch"));
    }
    let sha = digest(&output);
    let expected = if version == 3 {
        match key {
            Some(key) => keyed_id(key, &sha)?,
            None => sha,
        }
    } else {
        hex::decode(id)?
            .try_into()
            .map_err(|_| invalid("invalid block digest"))?
    };
    if sha != expected && key.is_none() {
        return Err(invalid("block SHA-256 mismatch"));
    }
    if key.is_some() && hex::encode(keyed_id(key.expect("checked key"), &sha)?) != id {
        return Err(invalid("encrypted block integrity mismatch"));
    }
    Ok(output)
}

fn bounded_read(reader: &mut impl Read, limit: u64) -> EngineResult<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(MAX_BLOCK_BYTES) as usize);
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut output)?;
    if output.len() as u64 > limit {
        return Err(invalid("decompressed data exceeds declared size"));
    }
    Ok(output)
}

fn validate_manifest(
    manifest: &Manifest,
    blocks: &HashMap<String, BlockInfo>,
    version: u32,
    maximum_output: u64,
) -> EngineResult<()> {
    if manifest.version != version {
        return Err(invalid("manifest and archive versions do not match"));
    }
    if manifest.entries.len() > MAX_MANIFEST_ENTRIES {
        return Err(invalid("archive has too many manifest entries"));
    }
    let mut paths = std::collections::HashSet::new();
    let mut path_types = HashMap::new();
    let mut total_output = 0_u64;
    for entry in &manifest.entries {
        if entry.path.len() > 64 * 1024 {
            return Err(invalid("archive path exceeds the configured length limit"));
        }
        if !safe_archive_path(&entry.path) {
            return Err(invalid("unsafe path in SmartPack manifest"));
        }
        let normalized_path = normalized_path_key(&entry.path);
        if !paths.insert(normalized_path.clone()) {
            return Err(invalid("duplicate or case-colliding manifest path"));
        }
        path_types.insert(normalized_path, entry.kind.as_str());
        if entry.kind == "file" {
            total_output = total_output
                .checked_add(entry.size)
                .ok_or_else(|| invalid("archive expanded-size overflow"))?;
            if total_output > maximum_output {
                return Err(invalid("archive exceeds configured expanded-size limit"));
            }
            if entry.segments.len() > MAX_BLOCKS {
                return Err(invalid("archive file has too many data segments"));
            }
            let mut total = 0_u64;
            for segment in &entry.segments {
                match segment {
                    Segment::Zero { zero } => {
                        total = total
                            .checked_add(*zero)
                            .ok_or_else(|| invalid("file size overflow"))?
                    }
                    Segment::Block {
                        block,
                        offset,
                        length,
                    } => {
                        let info = blocks
                            .get(block)
                            .ok_or_else(|| invalid("manifest references a missing block"))?;
                        if offset
                            .checked_add(*length)
                            .is_none_or(|end| end > info.raw_size)
                        {
                            return Err(invalid("manifest block range is invalid"));
                        }
                        total = total
                            .checked_add(*length)
                            .ok_or_else(|| invalid("file size overflow"))?;
                    }
                }
            }
            if total != entry.size {
                return Err(invalid("manifest file size does not match its segments"));
            }
        } else if !matches!(
            entry.kind.as_str(),
            "directory" | "dir" | "symlink" | "hardlink"
        ) {
            return Err(invalid("unsupported SmartPack entry type"));
        }
    }
    let mut ordered_paths: Vec<_> = path_types.keys().collect();
    ordered_paths.sort_unstable();
    for window in ordered_paths.windows(2) {
        let current = window[0];
        let next = window[1];
        if path_types.get(current) == Some(&"file") && next.starts_with(&format!("{current}/")) {
            return Err(invalid("file entry conflicts with a child path"));
        }
    }
    Ok(())
}

fn extract_spk(
    archive: &Path,
    destination: &Path,
    index: &ArchiveIndex,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(JobProgress),
) -> EngineResult<()> {
    use cap_std::{ambient_authority, fs::Dir};
    let root = Dir::open_ambient_dir(destination, ambient_authority())?;
    let mut file = File::open(archive)?;
    if index
        .manifest
        .entries
        .iter()
        .any(|entry| entry.kind == "symlink" || entry.kind == "hardlink")
    {
        return Err(invalid("safe extraction does not create archive links"));
    }
    for (id, block) in &index.blocks {
        check_cancel(cancel)?;
        let _ = load_block(
            &mut file,
            *block,
            id,
            index.version,
            key_ref(index.key.as_ref()),
        )?;
    }
    for entry in &index.manifest.entries {
        preflight_destination(
            &root,
            &entry.path,
            entry.kind == "dir" || entry.kind == "directory",
        )?;
    }
    let total = index
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == "file")
        .try_fold(0_u64, |sum, entry| sum.checked_add(entry.size))
        .ok_or_else(|| invalid("archive expanded-size overflow"))?;
    let mut completed = 0_u64;
    let started = Instant::now();
    for entry in &index.manifest.entries {
        check_cancel(cancel)?;
        if entry.kind == "directory" || entry.kind == "dir" {
            ensure_cap_dir(&root, &entry.path)?;
            continue;
        }
        let (parent, name) = ensure_cap_parent(&root, &entry.path)?;
        if parent.symlink_metadata(&name).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("output already exists: {}", entry.path),
            )
            .into());
        }
        let temp = format!(
            ".smartpack-{}-{}.partial",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut output = parent.open_with(&temp, &options)?;
        let result = (|| -> EngineResult<()> {
            for segment in &entry.segments {
                check_cancel(cancel)?;
                match segment {
                    Segment::Zero { zero } => write_zeros(&mut output, *zero, cancel)?,
                    Segment::Block {
                        block,
                        offset,
                        length,
                    } => {
                        let info = *index
                            .blocks
                            .get(block)
                            .ok_or_else(|| invalid("manifest references missing block"))?;
                        let bytes = load_block(
                            &mut file,
                            info,
                            block,
                            index.version,
                            key_ref(index.key.as_ref()),
                        )?;
                        let start = usize::try_from(*offset)?;
                        let end = usize::try_from(offset + length)?;
                        output.write_all(&bytes[start..end])?;
                    }
                }
                completed = completed.saturating_add(match segment {
                    Segment::Zero { zero } => *zero,
                    Segment::Block { length, .. } => *length,
                });
                progress(JobProgress {
                    phase: "extract".into(),
                    completed_bytes: completed,
                    total_bytes: Some(total),
                    current_path: Some(entry.path.clone()),
                    throughput_bytes_per_second: Some(throughput(completed, started)),
                });
            }
            output.flush()?;
            output.sync_all()?;
            Ok(())
        })();
        drop(output);
        if let Err(error) = result {
            let _ = parent.remove_file(&temp);
            return Err(error);
        }
        if let Err(error) = parent.hard_link(&temp, &parent, &name) {
            let _ = parent.remove_file(&temp);
            return Err(error.into());
        }
        parent.remove_file(&temp)?;
    }
    Ok(())
}

static TEMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn ensure_cap_dir(root: &cap_std::fs::Dir, path: &str) -> EngineResult<()> {
    let mut current = root.try_clone()?;
    for component in Path::new(path).components() {
        let Component::Normal(name) = component else {
            return Err(invalid("unsafe output path"));
        };
        match current.open_dir(name) {
            Ok(next) => current = next,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                current.create_dir(name)?;
                current = current.open_dir(name)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ensure_cap_parent<'a>(
    root: &'a cap_std::fs::Dir,
    path: &str,
) -> EngineResult<(cap_std::fs::Dir, String)> {
    let mut components = Path::new(path).components().peekable();
    let mut current = root.try_clone()?;
    let mut name = None;
    while let Some(component) = components.next() {
        let Component::Normal(part) = component else {
            return Err(invalid("unsafe output path"));
        };
        if components.peek().is_none() {
            name = Some(part.to_string_lossy().into_owned());
            break;
        }
        let part = part.to_os_string();
        match current.open_dir(&part) {
            Ok(next) => current = next,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                current.create_dir(&part)?;
                current = current.open_dir(&part)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok((current, name.ok_or_else(|| invalid("empty output path"))?))
}

fn preflight_destination(
    root: &cap_std::fs::Dir,
    path: &str,
    is_directory: bool,
) -> EngineResult<()> {
    let mut components = Path::new(path).components().peekable();
    let mut current = root.try_clone()?;
    while let Some(component) = components.next() {
        let Component::Normal(part) = component else {
            return Err(invalid("unsafe output path"));
        };
        if components.peek().is_some() {
            match current.open_dir(part) {
                Ok(next) => current = next,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
            continue;
        }
        match current.symlink_metadata(part) {
            Ok(metadata)
                if is_directory && metadata.is_dir() && !metadata.file_type().is_symlink() =>
            {
                return Ok(())
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("output conflict: {path}"),
                )
                .into())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid("empty output path"))
}

fn write_zeros(output: &mut impl Write, mut count: u64, cancel: &AtomicBool) -> EngineResult<()> {
    let zeroes = [0_u8; 64 * 1024];
    while count > 0 {
        check_cancel(cancel)?;
        let n = count.min(zeroes.len() as u64) as usize;
        output.write_all(&zeroes[..n])?;
        count -= n as u64;
    }
    Ok(())
}

fn temp_sibling(destination: &Path) -> PathBuf {
    destination.with_file_name(format!(
        ".smartpack-{}-{}.partial",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn publish_file(temporary: &Path, destination: &Path) -> EngineResult<()> {
    fs::hard_link(temporary, destination)?;
    fs::remove_file(temporary)?;
    Ok(())
}

fn is_smartpack(path: &Path) -> EngineResult<bool> {
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 8];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == SPK2_MAGIC || &magic == SPK3_MAGIC),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn read_u32(reader: &mut impl Read) -> EngineResult<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn read_u64(reader: &mut impl Read) -> EngineResult<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn create_zip(
    source: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(JobProgress),
) -> EngineResult<()> {
    use libarchive_oxide::libarchive_oxide_core::{ArchivePath, EntryKind, EntryMetadata, Limits};
    use libarchive_oxide::{ArchiveWriter, ZipMethod};
    let base = source.parent().unwrap_or(Path::new("."));
    let root = source;
    fs::create_dir_all(parent_directory(destination))?;
    let temp = temp_sibling(destination);
    let mut owns_temp = false;
    let started = Instant::now();
    let result = (|| -> EngineResult<()> {
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        owns_temp = true;
        let mut writer =
            ArchiveWriter::with_zip_method(output, ZipMethod::Deflate, Limits::default());
        let mut completed = 0_u64;
        for item in WalkDir::new(root).follow_links(false).sort_by_file_name() {
            check_cancel(cancel)?;
            let item = item?;
            if item.path() == temp {
                continue;
            }
            if item.file_type().is_symlink() {
                continue;
            }
            let rel = item
                .path()
                .strip_prefix(base)?
                .to_string_lossy()
                .replace('\\', "/");
            if !safe_archive_path(&rel) {
                return Err(invalid("source contains a non-portable path"));
            }
            let metadata = item.metadata()?;
            let kind = if item.file_type().is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            let name = if kind == EntryKind::Dir {
                format!("{rel}/")
            } else {
                rel.clone()
            };
            let header = EntryMetadata::builder(kind, ArchivePath::from_bytes(name.into_bytes()))
                .size(if kind == EntryKind::File {
                    Some(metadata.len())
                } else {
                    Some(0)
                })
                .mode(file_mode(&metadata))
                .build();
            writer.start_entry(&header)?;
            if kind == EntryKind::File {
                let mut input = File::open(item.path())?;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let n = input.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    writer.write_data(&buffer[..n])?;
                    completed = completed.saturating_add(n as u64);
                }
            }
            writer.end_entry()?;
            progress(JobProgress {
                phase: "compress".into(),
                completed_bytes: completed,
                total_bytes: None,
                current_path: Some(rel),
                throughput_bytes_per_second: Some(throughput(completed, started)),
            });
        }
        writer.finish()?;
        Ok(())
    })();
    match result {
        Ok(()) => match check_cancel(cancel) {
            Ok(()) => publish_file(&temp, destination),
            Err(error) => {
                if owns_temp {
                    let _ = fs::remove_file(&temp);
                }
                Err(error)
            }
        },
        Err(error) => {
            if owns_temp {
                let _ = fs::remove_file(temp);
            }
            Err(error)
        }
    }
}

fn extract_external(
    archive: &Path,
    destination: &Path,
    maximum_output: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(JobProgress),
) -> EngineResult<()> {
    use cap_std::{ambient_authority, fs::Dir};
    use libarchive_oxide::{ArchiveReader, Extractor};
    let entries = inspect_external(archive)?;
    validate_entry_paths(&entries)?;
    let mut total = 0_u64;
    for entry in &entries {
        if !matches!(entry.kind.as_str(), "file" | "directory") {
            return Err(invalid(
                "safe extraction does not create archive links or special files",
            ));
        }
        if entry.kind == "file" {
            total = total
                .checked_add(entry.size)
                .ok_or_else(|| invalid("archive expanded-size overflow"))?;
            if total > maximum_output {
                return Err(invalid("archive exceeds configured expanded-size limit"));
            }
        }
    }
    fs::create_dir_all(destination)?;
    let root = Dir::open_ambient_dir(destination, ambient_authority())?;
    for entry in &entries {
        preflight_destination(&root, &entry.path, entry.kind == "directory")?;
    }
    check_cancel(cancel)?;
    let mut extractor = Extractor::new(root);
    let report = match libarchive_oxide::SeekArchiveReader::new(File::open(archive)?) {
        Ok(mut reader) => extractor.extract_seek_matching(&mut reader, |_| true)?,
        Err(_) => {
            let mut reader = ArchiveReader::new(File::open(archive)?);
            extractor.extract(&mut reader)?
        }
    };
    if report.has_rejections() {
        return Err(invalid(
            "archive contained entries rejected by the safe extraction policy",
        ));
    }
    progress(JobProgress {
        phase: "extract".into(),
        completed_bytes: 1,
        total_bytes: Some(1),
        current_path: None,
        throughput_bytes_per_second: None,
    });
    check_cancel(cancel)?;
    Ok(())
}

fn validate_entry_paths(entries: &[ArchiveEntry]) -> EngineResult<()> {
    let mut kinds = HashMap::new();
    for entry in entries {
        if !safe_archive_path(&entry.path) {
            return Err(invalid("unsafe path in archive"));
        }
        let key = normalized_path_key(&entry.path);
        if kinds.insert(key, entry.kind.as_str()).is_some() {
            return Err(invalid(
                "archive contains duplicate or case-colliding paths",
            ));
        }
    }
    let mut paths: Vec<_> = kinds.keys().collect();
    paths.sort_unstable();
    for window in paths.windows(2) {
        if kinds.get(window[0]) == Some(&"file")
            && window[1].starts_with(&format!("{}/", window[0]))
        {
            return Err(invalid("archive file conflicts with a child path"));
        }
    }
    Ok(())
}

fn inspect_external(archive: &Path) -> EngineResult<Vec<ArchiveEntry>> {
    use libarchive_oxide::{ArchiveReader, ReaderEvent, SeekArchiveReader};
    if let Ok(mut reader) = SeekArchiveReader::new(File::open(archive)?) {
        let mut entries = Vec::new();
        loop {
            match reader.next_event()? {
                ReaderEvent::Entry(metadata) => {
                    if entries.len() >= MAX_MANIFEST_ENTRIES {
                        return Err(invalid("archive has too many entries"));
                    }
                    entries.push(entry_from_external_metadata(&metadata));
                    reader.skip_entry()?;
                }
                ReaderEvent::Done => return Ok(entries),
                ReaderEvent::Data(_) | ReaderEvent::ArchiveMetadata(_) | ReaderEvent::EndEntry => {}
                _ => return Err(invalid("unsupported archive reader event")),
            }
        }
    }
    let mut reader = ArchiveReader::new(File::open(archive)?);
    let mut entries = Vec::new();
    loop {
        match reader.next_event()? {
            ReaderEvent::Entry(metadata) => {
                if entries.len() >= MAX_MANIFEST_ENTRIES {
                    return Err(invalid("archive has too many entries"));
                }
                entries.push(entry_from_external_metadata(&metadata));
            }
            ReaderEvent::Done => return Ok(entries),
            ReaderEvent::Data(_) | ReaderEvent::ArchiveMetadata(_) | ReaderEvent::EndEntry => {}
            _ => return Err(invalid("unsupported archive reader event")),
        }
    }
}

fn entry_from_external_metadata(
    metadata: &libarchive_oxide::libarchive_oxide_core::EntryMetadata,
) -> ArchiveEntry {
    use libarchive_oxide::libarchive_oxide_core::EntryKind;
    let kind = match metadata.kind() {
        EntryKind::File => "file",
        EntryKind::Dir => "directory",
        EntryKind::Symlink => "symlink",
        EntryKind::Hardlink => "hardlink",
        _ => "special",
    };
    let modified_ns = metadata.times().modified.map(|time| {
        (time.secs.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::from(time.nanos))
    });
    ArchiveEntry {
        path: metadata.path().display_lossy(),
        kind: kind.into(),
        size: metadata.size().unwrap_or(0),
        modified_ns,
        mode: metadata.mode(),
        segments: Vec::new(),
        target: metadata.link_target().map(|path| path.display_lossy()),
    }
}

fn verify_external(
    archive: &Path,
    cancel: &AtomicBool,
    _progress: &mut impl FnMut(JobProgress),
) -> EngineResult<()> {
    use libarchive_oxide::{ArchiveReader, ReaderEvent, SeekArchiveReader};
    if let Ok(mut reader) = SeekArchiveReader::new(File::open(archive)?) {
        loop {
            check_cancel(cancel)?;
            match reader.next_event()? {
                ReaderEvent::Done => return Ok(()),
                ReaderEvent::Data(_)
                | ReaderEvent::Entry(_)
                | ReaderEvent::ArchiveMetadata(_)
                | ReaderEvent::EndEntry => {}
                _ => return Err(invalid("unsupported archive reader event")),
            }
        }
    }
    let mut reader = ArchiveReader::new(File::open(archive)?);
    loop {
        check_cancel(cancel)?;
        match reader.next_event()? {
            ReaderEvent::Done => return Ok(()),
            ReaderEvent::Data(_)
            | ReaderEvent::Entry(_)
            | ReaderEvent::ArchiveMetadata(_)
            | ReaderEvent::EndEntry => {}
            _ => return Err(invalid("unsupported archive reader event")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn spk_v3_round_trip_deduplicates_and_handles_zeroes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let body = [b"repeat me".repeat(20_000), vec![0_u8; 150_000]].concat();
        fs::write(source.join("one.bin"), &body).unwrap();
        fs::write(source.join("two.bin"), &body).unwrap();
        let archive = temp.path().join("backup.spk");
        let restored = temp.path().join("restored");
        let engine = Engine::new();
        engine
            .create(&source, &archive, CompressionProfile::Fast, None, |_| {})
            .unwrap();
        engine.verify(&archive, None, |_| {}).unwrap();
        engine.extract(&archive, &restored, None, |_| {}).unwrap();
        assert_eq!(fs::read(restored.join("source/one.bin")).unwrap(), body);
        assert_eq!(fs::read(restored.join("source/two.bin")).unwrap(), body);
    }

    #[test]
    fn encrypted_spk_rejects_wrong_password() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("data.txt");
        fs::write(&source, b"secret contents").unwrap();
        let archive = temp.path().join("secret.spk");
        let engine = Engine::new();
        engine
            .create(
                &source,
                &archive,
                CompressionProfile::Balanced,
                Some("correct horse"),
                |_| {},
            )
            .unwrap();
        assert!(engine
            .verify(&archive, Some("wrong password"), |_| {})
            .is_err());
        engine
            .verify(&archive, Some("correct horse"), |_| {})
            .unwrap();
    }

    #[test]
    fn encrypted_spk_hides_filenames_and_plaintext() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("private-name.txt");
        fs::write(&source, b"classified content marker").unwrap();
        let archive = temp.path().join("private.spk");
        Engine::new()
            .create(
                &source,
                &archive,
                CompressionProfile::Fast,
                Some("secret password"),
                |_| {},
            )
            .unwrap();
        let bytes = fs::read(archive).unwrap();
        assert!(!bytes
            .windows(b"private-name.txt".len())
            .any(|window| window == b"private-name.txt"));
        assert!(!bytes
            .windows(b"classified content marker".len())
            .any(|window| window == b"classified content marker"));
    }

    #[test]
    fn unsafe_manifest_paths_are_rejected() {
        for path in [
            "../escape",
            "/absolute",
            "C:/drive",
            "a/../../escape",
            "a/./b",
            "a\\..\\escape",
            "NUL.txt",
            "folder/trailing.",
        ] {
            assert!(!safe_archive_path(path));
        }
        assert!(safe_archive_path("folder/file.txt"));
    }

    #[test]
    fn unicode_normalization_and_case_collisions_are_rejected() {
        let entries = ["folder/é.txt", "FOLDER/e\u{301}.TXT"]
            .into_iter()
            .map(|path| ArchiveEntry {
                path: path.into(),
                kind: "file".into(),
                size: 0,
                modified_ns: None,
                mode: None,
                segments: Vec::new(),
                target: None,
            })
            .collect::<Vec<_>>();
        assert!(validate_entry_paths(&entries).is_err());
    }

    #[test]
    fn v2_reader_compatibility_fixture_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("legacy.spk");
        let restored = temp.path().join("restored");
        let bytes = b"created with the v2 record layout";
        let block_hash = digest(bytes);
        let mut block_encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        block_encoder.write_all(bytes).unwrap();
        let block_payload = block_encoder.finish().unwrap();
        let manifest = Manifest {
            format: "SmartPack".into(),
            version: 2,
            entries: vec![ArchiveEntry {
                path: "legacy/file.txt".into(),
                kind: "file".into(),
                size: bytes.len() as u64,
                modified_ns: None,
                mode: Some(0o644),
                segments: vec![Segment::Block {
                    block: hex::encode(block_hash),
                    offset: 0,
                    length: bytes.len() as u64,
                }],
                target: None,
            }],
        };
        let raw_manifest = serde_json::to_vec(&manifest).unwrap();
        let mut manifest_encoder =
            XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(3)).unwrap();
        manifest_encoder.write_all(&raw_manifest).unwrap();
        let stored_manifest = manifest_encoder.finish().unwrap();
        let mut output = File::create(&archive).unwrap();
        output.write_all(SPK2_MAGIC).unwrap();
        output.write_all(&2_u32.to_le_bytes()).unwrap();
        output.write_all(b"B").unwrap();
        output.write_all(&block_hash).unwrap();
        output.write_all(&[1]).unwrap();
        output
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .unwrap();
        output
            .write_all(&(block_payload.len() as u64).to_le_bytes())
            .unwrap();
        output.write_all(&block_payload).unwrap();
        output.write_all(b"M").unwrap();
        output
            .write_all(&(raw_manifest.len() as u64).to_le_bytes())
            .unwrap();
        output
            .write_all(&(stored_manifest.len() as u64).to_le_bytes())
            .unwrap();
        output.write_all(&digest(&raw_manifest)).unwrap();
        output.write_all(&stored_manifest).unwrap();
        output.write_all(b"E").unwrap();
        let engine = Engine::new();
        engine.verify(&archive, None, |_| {}).unwrap();
        engine.extract(&archive, &restored, None, |_| {}).unwrap();
        assert_eq!(fs::read(restored.join("legacy/file.txt")).unwrap(), bytes);
    }

    #[test]
    fn malformed_paths_and_expansion_claims_fail_before_extraction() {
        let temp = tempfile::tempdir().unwrap();
        for (name, path, size, segment) in [
            ("traversal", "../escape", 0, Segment::Zero { zero: 0 }),
            (
                "oversized",
                "large.bin",
                MAX_OUTPUT_BYTES + 1,
                Segment::Zero {
                    zero: MAX_OUTPUT_BYTES + 1,
                },
            ),
        ] {
            let archive = temp.path().join(format!("{name}.spk"));
            let manifest = Manifest {
                format: "SmartPack".into(),
                version: 3,
                entries: vec![ArchiveEntry {
                    path: path.into(),
                    kind: "file".into(),
                    size,
                    modified_ns: None,
                    mode: None,
                    segments: vec![segment],
                    target: None,
                }],
            };
            write_v3_manifest_archive(&archive, &manifest);
            assert!(Engine::new().verify(&archive, None, |_| {}).is_err());
            let destination = temp.path().join(format!("{name}-out"));
            assert!(Engine::new()
                .extract(&archive, &destination, None, |_| {})
                .is_err());
            assert!(!temp.path().join("escape").exists());
        }
    }

    #[test]
    fn zip_create_verify_and_safe_extract_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("zip-source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("sample.txt"), b"zip round trip").unwrap();
        let archive = temp.path().join("sample.zip");
        let restored = temp.path().join("zip-restored");
        let engine = Engine::new();
        engine
            .create(
                &source,
                &archive,
                CompressionProfile::Balanced,
                None,
                |_| {},
            )
            .unwrap();
        engine.verify(&archive, None, |_| {}).unwrap();
        engine.extract(&archive, &restored, None, |_| {}).unwrap();
        assert_eq!(
            fs::read(restored.join("zip-source/sample.txt")).unwrap(),
            b"zip round trip"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_final_symlink_is_not_followed_or_replaced() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("protected.txt"), b"archive data").unwrap();
        let archive = temp.path().join("protected.spk");
        Engine::new()
            .create(&source, &archive, CompressionProfile::Store, None, |_| {})
            .unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir_all(destination.join("source")).unwrap();
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, b"do not modify").unwrap();
        symlink(&outside, destination.join("source/protected.txt")).unwrap();
        assert!(Engine::new()
            .extract(&archive, &destination, None, |_| {})
            .is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"do not modify");
    }

    #[test]
    fn temp_output_cleanup_on_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("input");
        fs::create_dir(&source).unwrap();
        let mut file = File::create(source.join("large.bin")).unwrap();
        file.write_all(&vec![7_u8; 4 * 1024 * 1024]).unwrap();
        let archive = temp.path().join("cancel.spk");
        let engine = Engine::new();
        let canceller = engine.clone();
        assert!(engine
            .create(&source, &archive, CompressionProfile::Fast, None, |_| {
                canceller.cancel();
            })
            .is_err());
        assert!(!archive.exists());
        assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")));
    }

    fn write_v3_manifest_archive(path: &Path, manifest: &Manifest) {
        let plain = serde_json::to_vec(manifest).unwrap();
        let compressed = zstd::bulk::compress(&plain, 1).unwrap();
        let mut output = File::create(path).unwrap();
        output.write_all(SPK3_MAGIC).unwrap();
        output.write_all(&3_u32.to_le_bytes()).unwrap();
        output.write_all(&[0]).unwrap();
        output.write_all(&[0_u8; 28]).unwrap();
        output.write_all(b"M").unwrap();
        output
            .write_all(&(plain.len() as u64).to_le_bytes())
            .unwrap();
        output
            .write_all(&(compressed.len() as u64).to_le_bytes())
            .unwrap();
        output.write_all(&digest(&plain)).unwrap();
        output.write_all(&compressed).unwrap();
        output.write_all(b"E").unwrap();
    }
}
