import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Archive, ArrowDownToLine, Check, ChevronRight, CircleHelp, Clock3, FileArchive, FolderOpen, Gauge, LockKeyhole, Plus, Search, Settings2, ShieldCheck, X } from "lucide-react";

type Page = "home" | "jobs" | "settings";
type Profile = "fast" | "balanced" | "smallest" | "store";
type ArchiveFormat = "spk" | "zip";
type Entry = { path: string; type: string; size: number };
type Progress = { phase: string; completed_bytes: number; total_bytes?: number | null; current_path?: string | null; throughput_bytes_per_second?: number | null };
type Job = { id: string; title: string; operation: string; phase: string; completed: number; total?: number | null; speed?: number | null; status: "running" | "done" | "error"; message?: string };

const prettySize = (n: number) => {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
  return `${(n / 1024 ** i).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
};

export default function App() {
  const [page, setPage] = useState<Page>("home");
  const [profile, setProfile] = useState<Profile>("balanced");
  const [archiveFormat, setArchiveFormat] = useState<ArchiveFormat>("spk");
  const [encrypt, setEncrypt] = useState(false);
  const [password, setPassword] = useState("");
  const [source, setSource] = useState<string | null>(null);
  const [archive, setArchive] = useState<string | null>(null);
  const [archivePassword, setArchivePassword] = useState("");
  const [requiresPassword, setRequiresPassword] = useState(false);
  const [recoveryPercent, setRecoveryPercent] = useState(10);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [filter, setFilter] = useState("");
  const [jobs, setJobs] = useState<Job[]>([]);
  const [notice, setNotice] = useState("");
  const [dragging, setDragging] = useState(false);

  const visibleEntries = useMemo(() => entries.filter((entry) => entry.path.toLowerCase().includes(filter.toLowerCase())), [entries, filter]);

  useEffect(() => {
    let active = true;
    const progressListener = listen<[string, Progress]>("job-progress", ({ payload }) => {
      const [id, event] = payload;
      if (!active) return;
      setJobs((current) => current.map((job) => job.id === id ? { ...job, phase: event.phase, completed: event.completed_bytes, total: event.total_bytes, speed: event.throughput_bytes_per_second, title: event.current_path || job.title } : job));
    });
    const finishListener = listen<{ id: string; operation: string; result: { Ok?: string; Err?: string } }>("job-finished", ({ payload }) => {
      if (!active) return;
      const error = payload.result.Err;
      setJobs((current) => current.map((job) => job.id === payload.id ? { ...job, status: error ? "error" : "done", message: error || "Completed" } : job));
      setNotice(error || `${payload.operation[0].toUpperCase()}${payload.operation.slice(1)} completed.`);
    });
    const dropListener = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "enter" || event.payload.type === "over") setDragging(true);
      if (event.payload.type === "leave") setDragging(false);
      if (event.payload.type === "drop") {
        setDragging(false);
        const paths = event.payload.paths;
        if (paths.length) openArchive(paths[0]);
      }
    });
    void invoke<string | null>("take_startup_archive").then((path) => { if (path) void openArchive(path); });
    return () => { active = false; void progressListener.then((off) => off()); void finishListener.then((off) => off()); void dropListener.then((off) => off()); };
  }, []);

  async function openArchive(path?: string) {
    const selected = path || await open({ multiple: false, filters: [{ name: "Archives", extensions: ["spk", "zip", "7z", "tar", "gz", "xz", "zst", "bz2"] }] });
    if (!selected || Array.isArray(selected)) return;
    const extension = selected.split(/[\\/]/).at(-1)?.split(".").at(-1)?.toLowerCase();
    if (!extension || !["spk", "zip", "7z", "tar", "gz", "xz", "zst", "bz2"].includes(extension)) {
      setSource(selected); setArchive(null); setEntries([]); setPage("home"); return;
    }
    setArchive(selected);
    setEntries([]);
    setRequiresPassword(false);
    setArchivePassword("");
    try {
      const encrypted = await invoke<boolean>("archive_requires_password", { archive: selected });
      setRequiresPassword(encrypted);
      if (encrypted) { setNotice("Enter the archive password to browse its file list."); return; }
      const result = await invoke<Entry[]>("list_archive", { archive: selected, password: null });
      setEntries(result);
    } catch (error) { setNotice(String(error)); }
  }

  async function beginCreate() {
    if (!source) return setNotice("Choose a file or folder first.");
    if (encrypt && !password) return setNotice("Enter a password to encrypt this SPK archive.");
    const suggested = source.split(/[\\/]/).filter(Boolean).at(-1) || "archive";
    const extension = archiveFormat;
    const destination = await save({ defaultPath: `${suggested}.${extension}`, filters: [{ name: extension.toUpperCase(), extensions: [extension] }] });
    if (!destination) return;
    try {
      const id = await invoke<string>("create_archive", { source, destination, compressionProfile: profile, password: encrypt ? password : null });
      setPassword("");
      setJobs((current) => [{ id, title: suggested, operation: "Create archive", phase: "Starting", completed: 0, status: "running" }, ...current]);
      setNotice("Archive job started."); setPage("jobs");
    } catch (error) { setNotice(String(error)); }
  }

  async function beginVerify() {
    if (!archive) return setNotice("Open an archive first.");
    if (requiresPassword && !archivePassword) return setNotice("Enter the archive password above.");
    try {
      const id = await invoke<string>("verify_archive", { archive, password: requiresPassword ? archivePassword : null });
      setArchivePassword("");
      setJobs((current) => [{ id, title: archive.split(/[\\/]/).at(-1) || archive, operation: "Verify", phase: "Starting", completed: 0, status: "running" }, ...current]); setPage("jobs");
    } catch (error) { setNotice(String(error)); }
  }

  async function beginExtract() {
    if (!archive) return setNotice("Open an archive first.");
    const destination = await open({ directory: true, multiple: false, title: "Choose where to extract" });
    if (!destination || Array.isArray(destination)) return;
    if (requiresPassword && !archivePassword) return setNotice("Enter the archive password above.");
    try {
      const id = await invoke<string>("extract_archive", { archive, destination, password: requiresPassword ? archivePassword : null });
      setArchivePassword("");
      setJobs((current) => [{ id, title: archive.split(/[\\/]/).at(-1) || archive, operation: "Extract", phase: "Starting", completed: 0, status: "running" }, ...current]); setPage("jobs");
    } catch (error) { setNotice(String(error)); }
  }

  async function beginRecovery() {
    if (!archive) return setNotice("Open an archive first.");
    try {
      const id = await invoke<string>("create_recovery", { archive, percentage: recoveryPercent });
      setJobs((current) => [{ id, title: archive.split(/[\\/]/).at(-1) || archive, operation: "Create recovery data", phase: "Starting", completed: 0, status: "running" }, ...current]); setPage("jobs");
    } catch (error) { setNotice(String(error)); }
  }

  async function beginRepair() {
    if (!archive) return setNotice("Open the damaged archive first.");
    const repaired = await open({ directory: true, multiple: false, title: "Choose where repaired files will be written" });
    if (!repaired || Array.isArray(repaired)) return;
    const separator = Math.max(archive.lastIndexOf("/"), archive.lastIndexOf("\\"));
    const base = separator >= 0 ? archive.slice(0, separator) : ".";
    try {
      const id = await invoke<string>("repair_recovery", { indexFile: `${archive}.par2`, baseDir: base, repairedDir: repaired });
      setJobs((current) => [{ id, title: archive.split(/[\\/]/).at(-1) || archive, operation: "Repair from PAR2", phase: "Starting", completed: 0, status: "running" }, ...current]); setNotice("Repair job started. It cannot be canceled after it begins."); setPage("jobs");
    } catch (error) { setNotice(String(error)); }
  }

  async function chooseSource(directory = false) {
    const selected = await open({ multiple: false, directory, title: directory ? "Choose a folder to archive" : "Choose a file to archive" });
    if (selected && !Array.isArray(selected)) setSource(selected);
  }

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-icon"><Archive size={19}/></span><span>smartpack<span className="brand-dot">.</span><small>ARCHIVE MANAGER</small></span></div>
      <div className="nav-label">WORKSPACE</div>
      <button className={`nav-item ${page === "home" ? "active" : ""}`} onClick={() => setPage("home")}><Archive size={17}/>Home</button>
      <button className={`nav-item ${page === "jobs" ? "active" : ""}`} onClick={() => setPage("jobs")}><Clock3 size={17}/>Activity <span className="nav-count">{jobs.length}</span></button>
      <div className="nav-label nav-spaced">PREFERENCES</div>
      <button className={`nav-item ${page === "settings" ? "active" : ""}`} onClick={() => setPage("settings")}><Settings2 size={17}/>Settings</button>
      <div className="sidebar-bottom"><span className="status-dot"/> LOCAL &amp; PRIVATE <span className="build-tag">v0.3</span></div>
    </aside>

    <main className="main-view">
      <header className="topbar"><div className="breadcrumbs">SMARTPACK <ChevronRight size={13}/> <strong>{page === "home" ? "HOME" : page === "jobs" ? "ACTIVITY" : "SETTINGS"}</strong></div><button className="help-button" onClick={() => setNotice("SmartPack works locally. SPK archives support encryption; ZIP names remain visible.")}><CircleHelp size={16}/> Help</button></header>
      {notice && <button className="notice" onClick={() => setNotice("")}><span>{notice}</span><X size={15}/></button>}

      {page === "home" && <div className="page-content">
        <div className="welcome-row"><div><div className="eyebrow">YOUR FILES, PACKED SMARTER</div><h1>What would you like to do?</h1><p>Fast, dependable archives that stay on your device.</p></div><div className="hero-mark"><Archive size={28}/></div></div>
        <div className={`drop-zone ${dragging ? "dragging" : ""}`} onClick={() => void openArchive()}>
          <div className="drop-icon"><FolderOpen size={22}/></div><strong>Drop an archive here to open it</strong><span>SPK, ZIP, 7z, TAR and more</span><button className="text-button" onClick={(event) => { event.stopPropagation(); void openArchive(); }}>Browse archives <ChevronRight size={14}/></button>
          {dragging && <div className="drop-overlay">Drop to open archive</div>}
        </div>
        <section className="section-heading"><div><h2>Quick actions</h2><p>Start with one click, adjust options when you need them.</p></div></section>
        <div className="action-grid">
          <button className="action-card primary-card" onClick={() => void chooseSource()}><span className="action-icon violet"><Plus size={19}/></span><strong>Create archive</strong><small>Pack a file or folder into SPK or ZIP</small><span className="card-arrow"><ChevronRight size={17}/></span></button>
          <button className="action-card" onClick={() => void openArchive()}><span className="action-icon blue"><FolderOpen size={19}/></span><strong>Extract files</strong><small>Open and unpack an existing archive</small><span className="card-arrow"><ChevronRight size={17}/></span></button>
          <button className="action-card" onClick={() => void beginVerify()}><span className="action-icon green"><ShieldCheck size={19}/></span><strong>Verify archive</strong><small>Check archive integrity before use</small><span className="card-arrow"><ChevronRight size={17}/></span></button>
        </div>
        <button className="folder-source-link" onClick={() => void chooseSource(true)}>Or choose a folder to archive <ChevronRight size={13}/></button>
        {source && <div className="create-panel"><div className="panel-top"><div><div className="eyebrow">READY TO PACK</div><strong>{source.split(/[\\/]/).at(-1)}</strong></div><button className="icon-button" onClick={() => setSource(null)} aria-label="Clear source"><X size={16}/></button></div><div className="options-row"><label>Format<select value={archiveFormat} onChange={(event) => { const value = event.target.value as ArchiveFormat; setArchiveFormat(value); if (value === "zip") { setEncrypt(false); setPassword(""); } }}><option value="spk">SmartPack (.spk)</option><option value="zip">ZIP (.zip)</option></select></label><label>Compression profile<select value={profile} disabled={archiveFormat === "zip"} onChange={(event) => setProfile(event.target.value as Profile)}><option value="fast">Fast</option><option value="balanced">Balanced</option><option value="smallest">Smallest</option><option value="store">Store</option></select></label><label className="check-label"><input type="checkbox" checked={encrypt} disabled={archiveFormat === "zip"} onChange={(event) => setEncrypt(event.target.checked)}/><LockKeyhole size={14}/> Encrypt with password</label>{encrypt && <input className="password-field" type="password" value={password} onChange={(event) => setPassword(event.target.value)} placeholder="Archive password"/>}<button className="button button-primary" onClick={() => void beginCreate()}>Create archive <ChevronRight size={15}/></button></div><small style={{color:"#8992a2",fontSize:10,marginTop:8,display:"block"}}>{archiveFormat === "zip" ? "ZIP uses Deflate. Compression profiles and SPK encryption apply only to SPK." : "SPK uses the selected profile and supports password encryption."}</small></div>}
        {archive && <section className="archive-panel"><div className="section-heading archive-title"><div><div className="eyebrow">OPEN ARCHIVE</div><h2>{archive.split(/[\\/]/).at(-1)}</h2></div><button className="icon-button" onClick={() => { setArchive(null); setEntries([]); setRequiresPassword(false); setArchivePassword(""); }} aria-label="Close archive"><X size={16}/></button></div>{requiresPassword && <div className="archive-password"><LockKeyhole size={15}/><input type="password" autoComplete="current-password" value={archivePassword} onChange={(event) => setArchivePassword(event.target.value)} placeholder="Archive password"/><button className="button button-secondary" onClick={() => void invoke<Entry[]>("list_archive", { archive, password: archivePassword }).then(setEntries).catch((error) => setNotice(String(error)))}>Unlock file list</button></div>}<div className="archive-actions"><button className="button button-primary" onClick={() => void beginExtract()}><ArrowDownToLine size={15}/> Extract</button><button className="button button-secondary" onClick={() => void beginVerify()}><ShieldCheck size={15}/> Verify</button><label className="recovery-select">Recovery %<input type="number" min="1" max="100" value={recoveryPercent} onChange={(event) => setRecoveryPercent(Math.min(100, Math.max(1, Number(event.target.value))))}/></label><button className="button button-secondary" onClick={() => void beginRecovery()}>Add PAR2</button><button className="button button-secondary" onClick={() => void beginRepair()}>Repair</button></div>{entries.length > 0 && <><label className="search-box"><Search size={15}/><input value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Search files in archive"/></label><div className="entry-list">{visibleEntries.slice(0, 300).map((entry) => <div className="entry-row" key={entry.path}><FileArchive size={15}/><span>{entry.path}</span><small>{prettySize(entry.size)}</small></div>)}{visibleEntries.length > 300 && <div className="list-cap">Showing the first 300 matches.</div>}</div></>}</section>}
      </div>}

      {page === "jobs" && <div className="page-content"><div className="eyebrow">RECENT WORK</div><h1>Activity</h1><p className="page-subtitle">Track progress and outcomes from archive jobs.</p>{jobs.length === 0 ? <div className="empty-state"><span className="empty-icon"><Gauge size={22}/></span><strong>No recent jobs</strong><span>Created, extracted and verified archives will show here.</span></div> : <div className="jobs-list">{jobs.map((job) => { const percent = job.total ? Math.min(100, Math.round(job.completed * 100 / job.total)) : 0; return <article className="job-card" key={job.id}><div className={`job-symbol ${job.status}`} >{job.status === "done" ? <Check size={17}/> : job.status === "error" ? <X size={17}/> : <Archive size={17}/>}</div><div className="job-info"><div className="job-title"><strong>{job.operation}</strong><span>{job.status === "running" ? job.phase : job.status === "done" ? "Completed" : "Failed"}</span></div><div className="job-detail">{job.title}{job.speed ? ` · ${prettySize(job.speed)}/s` : ""}{job.message && job.status === "error" ? ` · ${job.message}` : ""}</div>{job.status === "running" && <div className="progress-track"><span style={{ width: `${percent}%` }}/></div>}</div>{job.status === "running" && job.operation !== "Repair from PAR2" && <button className="icon-button" onClick={() => void invoke("cancel_job", { id: job.id }).catch((error) => setNotice(String(error)))} title="Cancel job"><X size={16}/></button>}</article>; })}</div>}</div>}

      {page === "settings" && <div className="page-content"><div className="eyebrow">PREFERENCES</div><h1>Settings</h1><p className="page-subtitle">Choose how SmartPack balances speed, size and privacy.</p><section className="settings-card"><div className="settings-heading"><span className="action-icon violet"><Gauge size={18}/></span><div><strong>Default compression profile</strong><small>Applied when creating a new archive</small></div></div><div className="profile-options">{([["fast", "Fast", "Lower CPU use, quicker results"], ["balanced", "Balanced", "A practical speed and size balance"], ["smallest", "Smallest", "More CPU time to reduce archive size"], ["store", "Store", "Skip compression for maximum speed"]] as const).map(([value, title, detail]) => <button className={`profile-option ${profile === value ? "selected" : ""}`} key={value} onClick={() => setProfile(value)}><span className="radio-dot"/><span><strong>{title}</strong><small>{detail}</small></span></button>)}</div></section><section className="settings-card"><div className="settings-heading"><span className="action-icon green"><ShieldCheck size={18}/></span><div><strong>Privacy &amp; security</strong><small>Archive processing stays on this device</small></div></div><div className="privacy-note"><LockKeyhole size={16}/><span>SmartPack runs offline. Passwords are used only for the current operation and are not saved.</span></div></section></div>}
    </main>
  </div>;
}
