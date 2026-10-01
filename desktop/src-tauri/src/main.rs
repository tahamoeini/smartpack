#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use smartpack_engine::{ArchiveEntry, CompressionProfile, Engine, JobProgress};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Emitter, Manager, State};
use zeroize::Zeroize;

#[derive(Clone, Default)]
struct JobManager(Arc<Mutex<HashMap<String, Engine>>>, Arc<AtomicUsize>);

struct StartupArchive(Mutex<Option<String>>);

#[derive(Clone, Serialize)]
struct JobFinished {
    id: String,
    operation: String,
    result: std::result::Result<String, String>,
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

fn new_job(manager: &JobManager) -> Result<(String, Engine), String> {
    if manager
        .1
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < 2).then_some(active + 1)
        })
        .is_err()
    {
        return Err(
            "SmartPack is already running two archive jobs. Wait for a job to finish.".into(),
        );
    }
    let id = format!("job-{}", NEXT_JOB.fetch_add(1, Ordering::Relaxed));
    let engine = Engine::new();
    manager
        .0
        .lock()
        .expect("job state poisoned")
        .insert(id.clone(), engine.clone());
    Ok((id, engine))
}

fn profile(name: &str) -> Result<CompressionProfile, String> {
    match name {
        "fast" => Ok(CompressionProfile::Fast),
        "balanced" => Ok(CompressionProfile::Balanced),
        "smallest" => Ok(CompressionProfile::Smallest),
        "store" => Ok(CompressionProfile::Store),
        _ => Err("Unknown compression profile".into()),
    }
}

fn emit_progress(app: &tauri::AppHandle, id: &str, progress: JobProgress) {
    let _ = app.emit("job-progress", (id.to_owned(), progress));
}

fn finish_job(
    app: &tauri::AppHandle,
    manager: &JobManager,
    id: &str,
    operation: &str,
    result: Result<(), String>,
) {
    manager.0.lock().expect("job state poisoned").remove(id);
    manager.1.fetch_sub(1, Ordering::AcqRel);
    let status = result.map(|()| "Completed".to_owned());
    let _ = app.emit(
        "job-finished",
        JobFinished {
            id: id.to_owned(),
            operation: operation.to_owned(),
            result: status,
        },
    );
}

#[tauri::command]
async fn create_archive(
    app: tauri::AppHandle,
    manager: State<'_, JobManager>,
    source: String,
    destination: String,
    compression_profile: String,
    password: Option<String>,
) -> Result<String, String> {
    let selected_profile = profile(&compression_profile)?;
    let (id, engine) = new_job(&manager)?;
    let job_id = id.clone();
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut password = password;
        let result = engine
            .create(
                PathBuf::from(source).as_path(),
                PathBuf::from(destination).as_path(),
                selected_profile,
                password.as_deref(),
                |event| emit_progress(&app, &job_id, event),
            )
            .map_err(|error| error.to_string());
        password.zeroize();
        finish_job(&app, &manager, &job_id, "create", result);
    });
    Ok(id)
}

#[tauri::command]
async fn verify_archive(
    app: tauri::AppHandle,
    manager: State<'_, JobManager>,
    archive: String,
    password: Option<String>,
) -> Result<String, String> {
    let (id, engine) = new_job(&manager)?;
    let job_id = id.clone();
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut password = password;
        let result = engine
            .verify(
                PathBuf::from(archive).as_path(),
                password.as_deref(),
                |event| emit_progress(&app, &job_id, event),
            )
            .map_err(|error| error.to_string());
        password.zeroize();
        finish_job(&app, &manager, &job_id, "verify", result);
    });
    Ok(id)
}

#[tauri::command]
async fn extract_archive(
    app: tauri::AppHandle,
    manager: State<'_, JobManager>,
    archive: String,
    destination: String,
    password: Option<String>,
) -> Result<String, String> {
    let (id, engine) = new_job(&manager)?;
    let job_id = id.clone();
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut password = password;
        let result = engine
            .extract(
                PathBuf::from(archive).as_path(),
                PathBuf::from(destination).as_path(),
                password.as_deref(),
                |event| emit_progress(&app, &job_id, event),
            )
            .map_err(|error| error.to_string());
        password.zeroize();
        finish_job(&app, &manager, &job_id, "extract", result);
    });
    Ok(id)
}

#[tauri::command]
async fn create_recovery(
    app: tauri::AppHandle,
    manager: State<'_, JobManager>,
    archive: String,
    percentage: u8,
) -> Result<String, String> {
    let archive_path = PathBuf::from(archive);
    let output_dir = archive_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let (id, engine) = new_job(&manager)?;
    let job_id = id.clone();
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = engine
            .create_recovery(&archive_path, &output_dir, percentage)
            .map(|_| ())
            .map_err(|error| error.to_string());
        finish_job(&app, &manager, &job_id, "recovery data", result);
    });
    Ok(id)
}

#[tauri::command]
async fn repair_recovery(
    app: tauri::AppHandle,
    manager: State<'_, JobManager>,
    index_file: String,
    base_dir: String,
    repaired_dir: String,
) -> Result<String, String> {
    let (id, engine) = new_job(&manager)?;
    let job_id = id.clone();
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = engine
            .repair_recovery(
                PathBuf::from(index_file).as_path(),
                PathBuf::from(base_dir).as_path(),
                PathBuf::from(repaired_dir).as_path(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string());
        finish_job(&app, &manager, &job_id, "repair", result);
    });
    Ok(id)
}

#[tauri::command]
async fn list_archive(
    archive: String,
    password: Option<String>,
) -> Result<Vec<ArchiveEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut password = password;
        let result = Engine::new()
            .inspect(PathBuf::from(archive).as_path(), password.as_deref())
            .map_err(|error| error.to_string());
        password.zeroize();
        result
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn archive_requires_password(archive: String) -> Result<bool, String> {
    Engine::archive_requires_password(PathBuf::from(archive).as_path())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_job(manager: State<'_, JobManager>, id: String) -> Result<(), String> {
    let jobs = manager.0.lock().map_err(|_| "job state is unavailable")?;
    let engine = jobs
        .get(&id)
        .ok_or_else(|| "job was not found".to_owned())?;
    engine.cancel();
    Ok(())
}

#[tauri::command]
fn take_startup_archive(startup: State<'_, StartupArchive>) -> Option<String> {
    startup.0.lock().ok()?.take()
}

fn main() {
    let startup_archive = std::env::args()
        .skip(1)
        .find(|argument| argument.to_ascii_lowercase().ends_with(".spk"));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(JobManager::default())
        .manage(StartupArchive(Mutex::new(startup_archive)))
        .invoke_handler(tauri::generate_handler![
            create_archive,
            verify_archive,
            extract_archive,
            create_recovery,
            repair_recovery,
            list_archive,
            archive_requires_password,
            cancel_job,
            take_startup_archive
        ])
        .run(tauri::generate_context!())
        .expect("failed to run SmartPack desktop application");
}
