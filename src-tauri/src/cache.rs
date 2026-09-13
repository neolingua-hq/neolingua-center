//! Browser-ready cache: remuxed MP4 + EN/FR WebVTT beside app data.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Hide the child console on Windows (ffmpeg/ffprobe/whisper are console apps).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn command_hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// Returned by prepare steps when the user cancels mid-job.
const CANCELLED_MSG: &str = "__cancelled__";

/// How an EN/FR subtitle track was obtained for the cache.
/// `native` = sidecar or embedded, `generated` = Whisper / translation, `missing` = absent.
pub const SUB_SOURCE_NATIVE: &str = "native";
pub const SUB_SOURCE_GENERATED: &str = "generated";
pub const SUB_SOURCE_MISSING: &str = "missing";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodePrep {
    /// missing | partial | ready | queued | processing | error
    pub status: String,
    pub video: bool,
    pub subs_en: bool,
    pub subs_fr: bool,
    /// native | generated | missing
    pub en_source: String,
    /// native | generated | missing
    pub fr_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Default for EpisodePrep {
    fn default() -> Self {
        Self {
            status: "missing".into(),
            video: false,
            subs_en: false,
            subs_fr: false,
            en_source: SUB_SOURCE_MISSING.into(),
            fr_source: SUB_SOURCE_MISSING.into(),
            progress: None,
            message: None,
        }
    }
}

impl EpisodePrep {
    fn from_tracks(video: bool, en_source: &str, fr_source: &str) -> Self {
        let subs_en = en_source != SUB_SOURCE_MISSING;
        let subs_fr = fr_source != SUB_SOURCE_MISSING;
        let status = if video && subs_en && subs_fr {
            "ready"
        } else if video || subs_en || subs_fr {
            "partial"
        } else {
            "missing"
        };
        let message = if status == "missing" {
            None
        } else if status == "ready" {
            None
        } else {
            Some(constitution_message(en_source, fr_source))
        };
        Self {
            status: status.into(),
            video,
            subs_en,
            subs_fr,
            en_source: en_source.into(),
            fr_source: fr_source.into(),
            progress: if status == "ready" { Some(100) } else { None },
            message,
        }
    }
}

fn source_label_fr(source: &str) -> &'static str {
    match source {
        SUB_SOURCE_NATIVE => "natif",
        SUB_SOURCE_GENERATED => "généré",
        _ => "manquant",
    }
}

fn constitution_message(en_source: &str, fr_source: &str) -> String {
    format!(
        "EN {}, FR {}",
        source_label_fr(en_source),
        source_label_fr(fr_source)
    )
}

pub struct PrepRegistry {
    app_data_dir: PathBuf,
    cache_dir: PathBuf,
    jobs: Mutex<HashMap<String, EpisodePrep>>,
    work: Mutex<()>,
    /// Per-media cancel flags (set by `cancel_prepare`, cleared when a job starts).
    cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Active child PIDs (ffmpeg / whisper) so cancel can kill them promptly.
    active_pids: Mutex<HashMap<String, u32>>,
}

impl PrepRegistry {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let cache_dir = app_data_dir.join("cache");
        let _ = fs::create_dir_all(cache_dir.join("video"));
        let _ = fs::create_dir_all(cache_dir.join("subs"));
        Self {
            app_data_dir,
            cache_dir,
            jobs: Mutex::new(HashMap::new()),
            work: Mutex::new(()),
            cancel_flags: Mutex::new(HashMap::new()),
            active_pids: Mutex::new(HashMap::new()),
        }
    }

    fn cancel_flag(&self, media_path: &str) -> Arc<AtomicBool> {
        let mut flags = self.cancel_flags.lock().expect("cancel flags lock");
        flags
            .entry(media_path.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    fn reset_cancel_flag(&self, media_path: &str) {
        self.cancel_flag(media_path).store(false, Ordering::SeqCst);
    }

    fn is_cancelled(&self, media_path: &str) -> bool {
        self.cancel_flag(media_path).load(Ordering::SeqCst)
    }

    fn ensure_not_cancelled(&self, media_path: &str) -> Result<(), String> {
        if self.is_cancelled(media_path) {
            Err(CANCELLED_MSG.into())
        } else {
            Ok(())
        }
    }

    fn register_pid(&self, media_path: &str, pid: u32) {
        if let Ok(mut pids) = self.active_pids.lock() {
            pids.insert(media_path.to_string(), pid);
        }
    }

    fn clear_pid(&self, media_path: &str) {
        if let Ok(mut pids) = self.active_pids.lock() {
            pids.remove(media_path);
        }
    }

    fn kill_active_child(&self, media_path: &str) {
        let pid = {
            let Ok(mut pids) = self.active_pids.lock() else {
                return;
            };
            pids.remove(media_path)
        };
        let Some(pid) = pid else {
            return;
        };
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
        #[cfg(windows)]
        {
            let _ = command_hidden("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status();
        }
    }

    /// Request cancel for a queued or in-progress prep. Purges cache when the job stops.
    pub fn cancel_prepare(&self, media_path: &str) -> EpisodePrep {
        self.cancel_flag(media_path).store(true, Ordering::SeqCst);
        self.kill_active_child(media_path);

        let status = {
            let Ok(jobs) = self.jobs.lock() else {
                return inspect_disk(&self.cache_dir, media_path);
            };
            jobs.get(media_path).map(|j| j.status.clone())
        };

        match status.as_deref() {
            Some("queued") => {
                self.clear_job(media_path);
                self.reset_cancel_flag(media_path);
                inspect_disk(&self.cache_dir, media_path)
            }
            Some("processing") => {
                let disk = inspect_disk(&self.cache_dir, media_path);
                EpisodePrep {
                    status: "processing".into(),
                    video: disk.video,
                    subs_en: disk.subs_en,
                    subs_fr: disk.subs_fr,
                    en_source: disk.en_source,
                    fr_source: disk.fr_source,
                    progress: disk.progress,
                    message: Some("Annulation…".into()),
                }
            }
            _ => {
                self.purge_episode(media_path);
                self.reset_cancel_flag(media_path);
                EpisodePrep::default()
            }
        }
    }

    pub fn inspect(&self, media_path: &str) -> EpisodePrep {
        if let Ok(jobs) = self.jobs.lock() {
            if let Some(job) = jobs.get(media_path) {
                if job.status == "processing" || job.status == "queued" || job.status == "error" {
                    return job.clone();
                }
            }
        }
        inspect_disk(&self.cache_dir, media_path)
    }

    pub fn set_job(&self, media_path: &str, state: EpisodePrep) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.insert(media_path.to_string(), state);
        }
    }

    pub fn clear_job(&self, media_path: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.remove(media_path);
        }
    }

    pub fn video_file(&self, media_path: &str) -> Option<PathBuf> {
        let path = video_cache_path(&self.cache_dir, media_path);
        path.is_file().then_some(path)
    }

    pub fn subtitle_file(&self, media_path: &str, lang: &str) -> Option<PathBuf> {
        let lang = if lang == "fr" { "fr" } else { "en" };
        let path = sub_cache_path(&self.cache_dir, media_path, lang);
        path.is_file().then_some(path)
    }

    /// Delete remuxed video + VTT for one media path. Never touches the source file.
    pub fn purge_episode(&self, media_path: &str) {
        let video = video_cache_path(&self.cache_dir, media_path);
        let partial = video.with_extension("partial.mp4");
        let en = sub_cache_path(&self.cache_dir, media_path, "en");
        let fr = sub_cache_path(&self.cache_dir, media_path, "fr");
        let sources = sources_cache_path(&self.cache_dir, media_path);
        let en_whisper_wav = en.with_extension("whisper.wav");
        let en_whisper_vtt = PathBuf::from(format!(
            "{}.vtt",
            en.with_extension("whisper").to_string_lossy()
        ));
        let fr_whisper_wav = fr.with_extension("whisper.wav");
        let fr_whisper_vtt = PathBuf::from(format!(
            "{}.vtt",
            fr.with_extension("whisper").to_string_lossy()
        ));
        for path in [
            &video,
            &partial,
            &en,
            &fr,
            &sources,
            &en_whisper_wav,
            &en_whisper_vtt,
            &fr_whisper_wav,
            &fr_whisper_vtt,
        ] {
            let _ = fs::remove_file(path);
        }
        self.clear_job(media_path);
    }

    /// Bump mtime so recently played items are kept longer under a size ceiling.
    pub fn touch_episode(&self, media_path: &str) {
        let now = std::time::SystemTime::now();
        for path in [
            video_cache_path(&self.cache_dir, media_path),
            sub_cache_path(&self.cache_dir, media_path, "en"),
            sub_cache_path(&self.cache_dir, media_path, "fr"),
        ] {
            if path.is_file() {
                if let Ok(file) = fs::File::open(&path) {
                    let _ = file.set_modified(now);
                }
            }
        }
    }

    pub fn total_bytes(&self) -> u64 {
        dir_total_bytes(&self.cache_dir.join("video"))
            + dir_total_bytes(&self.cache_dir.join("subs"))
    }

    /// Delete oldest cache groups until total size is under `max_bytes`.
    /// `max_bytes == 0` means unlimited. Optionally protect one media path.
    pub fn enforce_limit(&self, max_bytes: u64, protect_media_path: Option<&str>) {
        if max_bytes == 0 {
            return;
        }
        let protect_key = protect_media_path.map(cache_key);
        loop {
            let total = self.total_bytes();
            if total <= max_bytes {
                break;
            }
            let Some(victim) = oldest_cache_key(&self.cache_dir, protect_key.as_deref()) else {
                break;
            };
            remove_cache_key(&self.cache_dir, &victim);
        }
    }

    pub fn enqueue(&self, paths: &[String]) {
        for path in paths {
            let current = self.inspect(path);
            if current.status == "ready"
                || current.status == "processing"
                || current.status == "queued"
            {
                continue;
            }
            self.set_job(
                path,
                EpisodePrep {
                    status: "queued".into(),
                    video: current.video,
                    subs_en: current.subs_en,
                    subs_fr: current.subs_fr,
                    en_source: current.en_source.clone(),
                    fr_source: current.fr_source.clone(),
                    progress: None,
                    message: Some("En file".into()),
                },
            );
        }
    }

    /// Single entry point for media preparation (Center "Préparer" and browser "Ouvrir").
    /// Loads TMDB original language + cache limit from the DB, then runs [`Self::prepare`].
    pub fn prepare_from_db<F>(
        &self,
        db_path: &Path,
        media_path: &str,
        mut on_update: F,
    ) -> EpisodePrep
    where
        F: FnMut(&EpisodePrep),
    {
        let loaded = (|| -> Result<(u64, Option<String>), String> {
            let conn = crate::db::open_connection(&db_path.to_path_buf())?;
            crate::db::run_migrations(&conn)?;
            let max_bytes =
                crate::db::cache_max_bytes(crate::db::load_settings(&conn)?.cache_max_gb);
            let original_language = crate::db::lookup_original_language(&conn, media_path)?;
            Ok((max_bytes, original_language))
        })();

        let (max_bytes, original_language) = match loaded {
            Ok(v) => v,
            Err(message) => {
                let disk = inspect_disk(&self.cache_dir, media_path);
                let failed = EpisodePrep {
                    status: "error".into(),
                    video: disk.video,
                    subs_en: disk.subs_en,
                    subs_fr: disk.subs_fr,
                    en_source: disk.en_source.clone(),
                    fr_source: disk.fr_source.clone(),
                    progress: Some(0),
                    message: Some(message),
                };
                self.set_job(media_path, failed.clone());
                on_update(&failed);
                return failed;
            }
        };

        let result = self.prepare(media_path, original_language.as_deref(), &mut on_update);
        self.enforce_limit(max_bytes, Some(media_path));
        result
    }

    pub fn prepare<F>(
        &self,
        media_path: &str,
        original_language: Option<&str>,
        mut on_update: F,
    ) -> EpisodePrep
    where
        F: FnMut(&EpisodePrep),
    {
        let current = self.inspect(media_path);
        if current.status == "ready" {
            return current;
        }
        if current.status == "processing" {
            return current;
        }

        let work = match self.work.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                let queued = EpisodePrep {
                    status: "queued".into(),
                    video: current.video,
                    subs_en: current.subs_en,
                    subs_fr: current.subs_fr,
                    en_source: current.en_source.clone(),
                    fr_source: current.fr_source.clone(),
                    progress: None,
                    message: Some("En file".into()),
                };
                self.set_job(media_path, queued.clone());
                on_update(&queued);
                self.work.lock().expect("prep work lock")
            }
        };

        let on_disk = inspect_disk(&self.cache_dir, media_path);
        if on_disk.status == "ready" {
            self.clear_job(media_path);
            on_update(&on_disk);
            return on_disk;
        }

        let started = EpisodePrep {
            status: "processing".into(),
            video: on_disk.video,
            subs_en: on_disk.subs_en,
            subs_fr: on_disk.subs_fr,
            en_source: on_disk.en_source.clone(),
            fr_source: on_disk.fr_source.clone(),
            progress: Some(2),
            message: Some("Préparation en cours…".into()),
        };
        self.reset_cancel_flag(media_path);
        self.set_job(media_path, started.clone());
        on_update(&started);

        match prepare_inner(self, media_path, original_language, &mut on_update) {
            Ok(state) => {
                drop(work);
                self.clear_job(media_path);
                self.reset_cancel_flag(media_path);
                on_update(&state);
                state
            }
            Err(message) if message == CANCELLED_MSG => {
                drop(work);
                self.purge_episode(media_path);
                self.clear_pid(media_path);
                self.reset_cancel_flag(media_path);
                let cleared = EpisodePrep::default();
                on_update(&cleared);
                cleared
            }
            Err(message) => {
                drop(work);
                self.clear_pid(media_path);
                let disk = inspect_disk(&self.cache_dir, media_path);
                let message = {
                    let trimmed = message.trim();
                    if trimmed.is_empty() {
                        "Préparation impossible (fichier illisible ou corrompu).".into()
                    } else {
                        trimmed.to_string()
                    }
                };
                let failed = EpisodePrep {
                    status: "error".into(),
                    video: disk.video,
                    subs_en: disk.subs_en,
                    subs_fr: disk.subs_fr,
                    en_source: disk.en_source.clone(),
                    fr_source: disk.fr_source.clone(),
                    progress: Some(0),
                    message: Some(message),
                };
                self.set_job(media_path, failed.clone());
                on_update(&failed);
                failed
            }
        }
    }
}

fn cache_key(media_path: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    media_path.hash(&mut hasher);
    format!("p{:016x}", hasher.finish())
}

fn dir_total_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

fn cache_key_from_filename(name: &str) -> Option<String> {
    // pHASH.mp4, pHASH.en.vtt / pHASH.fr.vtt, or pHASH.sources.json
    let stem = name
        .strip_suffix(".mp4")
        .or_else(|| name.strip_suffix(".vtt"))
        .or_else(|| name.strip_suffix(".sources.json"))?;
    let key = stem.split('.').next()?;
    if key.starts_with('p') && key.len() == 17 {
        Some(key.to_string())
    } else {
        None
    }
}

fn oldest_cache_key(cache_dir: &Path, protect_key: Option<&str>) -> Option<String> {
    use std::collections::HashMap;
    use std::time::SystemTime;

    let mut groups: HashMap<String, (u64, SystemTime)> = HashMap::new();
    for sub in ["video", "subs"] {
        let dir = cache_dir.join(sub);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(key) = cache_key_from_filename(name) else {
                continue;
            };
            if protect_key == Some(key.as_str()) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let len = meta.len();
            groups
                .entry(key)
                .and_modify(|(bytes, oldest)| {
                    *bytes += len;
                    if mtime < *oldest {
                        *oldest = mtime;
                    }
                })
                .or_insert((len, mtime));
        }
    }

    groups
        .into_iter()
        .min_by_key(|(_, (_, mtime))| *mtime)
        .map(|(key, _)| key)
}

fn remove_cache_key(cache_dir: &Path, key: &str) {
    let video = cache_dir.join("video").join(format!("{key}.mp4"));
    let en = cache_dir.join("subs").join(format!("{key}.en.vtt"));
    let fr = cache_dir.join("subs").join(format!("{key}.fr.vtt"));
    let sources = cache_dir.join("subs").join(format!("{key}.sources.json"));
    let _ = fs::remove_file(video);
    let _ = fs::remove_file(en);
    let _ = fs::remove_file(fr);
    let _ = fs::remove_file(sources);
}

fn host_target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    {
        "unknown"
    }
}

fn sidecar_file_name(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.exe")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

fn sidecar_triple_name(name: &str) -> String {
    let base = format!("{name}-{}", host_target_triple());
    #[cfg(windows)]
    {
        format!("{base}.exe")
    }
    #[cfg(not(windows))]
    {
        base
    }
}

/// Resolve ffmpeg/ffprobe from the app bundle (Tauri externalBin), then the
/// crate `binaries/` folder used in development.
fn find_media_tool(name: &str) -> Result<PathBuf, String> {
    let plain = sidecar_file_name(name);
    let triple = sidecar_triple_name(name);
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(&plain));
            candidates.push(dir.join(&triple));
        }
    }

    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&triple),
    );

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }

    // Dev-only: allow a system install when iterating without fetched sidecars.
    #[cfg(debug_assertions)]
    {
        if let Ok(path_env) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_env) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(format!(
        "Outil manquant dans l'application ({name}). Réinstallez Neolingua Center."
    ))
}

fn ffmpeg_bin() -> Result<&'static Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match PATH.get_or_init(|| find_media_tool("ffmpeg")) {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

fn ffprobe_bin() -> Result<&'static Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match PATH.get_or_init(|| find_media_tool("ffprobe")) {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

fn video_cache_path(cache_dir: &Path, media_path: &str) -> PathBuf {
    cache_dir
        .join("video")
        .join(format!("{}.mp4", cache_key(media_path)))
}

fn sub_cache_path(cache_dir: &Path, media_path: &str, lang: &str) -> PathBuf {
    cache_dir
        .join("subs")
        .join(format!("{}.{}.vtt", cache_key(media_path), lang))
}

fn sources_cache_path(cache_dir: &Path, media_path: &str) -> PathBuf {
    cache_dir
        .join("subs")
        .join(format!("{}.sources.json", cache_key(media_path)))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SubSourcesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    en: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fr: Option<String>,
}

fn load_sources_file(cache_dir: &Path, media_path: &str) -> SubSourcesFile {
    let path = sources_cache_path(cache_dir, media_path);
    let Ok(raw) = fs::read_to_string(&path) else {
        return SubSourcesFile::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_sub_source(cache_dir: &Path, media_path: &str, lang: &str, source: &str) {
    let mut file = load_sources_file(cache_dir, media_path);
    match lang {
        "fr" => file.fr = Some(source.into()),
        _ => file.en = Some(source.into()),
    }
    let path = sources_cache_path(cache_dir, media_path);
    if let Ok(raw) = serde_json::to_string_pretty(&file) {
        let _ = fs::write(path, raw);
    }
}

fn resolve_track_source(has_file: bool, recorded: Option<&str>) -> String {
    if !has_file {
        return SUB_SOURCE_MISSING.into();
    }
    match recorded {
        Some(SUB_SOURCE_GENERATED) => SUB_SOURCE_GENERATED.into(),
        Some(SUB_SOURCE_NATIVE) | Some(_) | None => SUB_SOURCE_NATIVE.into(),
    }
}

fn inspect_disk(cache_dir: &Path, media_path: &str) -> EpisodePrep {
    let video = video_cache_path(cache_dir, media_path).is_file();
    let subs_en = sub_cache_path(cache_dir, media_path, "en").is_file();
    let subs_fr = sub_cache_path(cache_dir, media_path, "fr").is_file();
    let recorded = load_sources_file(cache_dir, media_path);
    let en_source = resolve_track_source(subs_en, recorded.en.as_deref());
    let fr_source = resolve_track_source(subs_fr, recorded.fr.as_deref());
    EpisodePrep::from_tracks(video, &en_source, &fr_source)
}

fn prepare_inner<F>(
    registry: &PrepRegistry,
    media_path: &str,
    original_language: Option<&str>,
    on_update: &mut F,
) -> Result<EpisodePrep, String>
where
    F: FnMut(&EpisodePrep),
{
    let source = PathBuf::from(media_path);
    if !source.is_file() {
        return Err("Fichier source introuvable".into());
    }
    reject_unreadable_source(&source)?;
    registry.ensure_not_cancelled(media_path)?;

    let original_track = crate::db::normalize_track_language(original_language);

    let video_out = video_cache_path(&registry.cache_dir, media_path);
    if !video_out.is_file() {
        notify_progress(
            registry,
            media_path,
            5,
            "Remux vidéo…",
            PrepTrackFlags {
                video: false,
                en_source: SUB_SOURCE_MISSING.into(),
                fr_source: SUB_SOURCE_MISSING.into(),
            },
            on_update,
        );
        remux_video(registry, media_path, &source, &video_out, |remux_pct| {
            let overall = 5 + (u16::from(remux_pct) * 70 / 100) as u8;
            notify_progress(
                registry,
                media_path,
                overall,
                "Remux vidéo…",
                PrepTrackFlags {
                    video: false,
                    en_source: SUB_SOURCE_MISSING.into(),
                    fr_source: SUB_SOURCE_MISSING.into(),
                },
                on_update,
            );
        })?;
    }

    registry.ensure_not_cancelled(media_path)?;
    notify_progress(
        registry,
        media_path,
        78,
        "Sous-titres…",
        PrepTrackFlags {
            video: true,
            en_source: SUB_SOURCE_MISSING.into(),
            fr_source: SUB_SOURCE_MISSING.into(),
        },
        on_update,
    );
    ensure_subtitle(
        registry,
        media_path,
        &source,
        "en",
        original_track,
        original_language,
        on_update,
    )?;
    registry.ensure_not_cancelled(media_path)?;
    let after_en = inspect_disk(&registry.cache_dir, media_path);
    notify_progress(
        registry,
        media_path,
        88,
        "Sous-titres…",
        PrepTrackFlags {
            video: true,
            en_source: after_en.en_source.clone(),
            fr_source: after_en.fr_source.clone(),
        },
        on_update,
    );
    ensure_subtitle(
        registry,
        media_path,
        &source,
        "fr",
        original_track,
        original_language,
        on_update,
    )?;
    registry.ensure_not_cancelled(media_path)?;
    let after_fr = inspect_disk(&registry.cache_dir, media_path);
    notify_progress(
        registry,
        media_path,
        96,
        "Finalisation…",
        PrepTrackFlags {
            video: true,
            en_source: after_fr.en_source.clone(),
            fr_source: after_fr.fr_source.clone(),
        },
        on_update,
    );

    let state = inspect_disk(&registry.cache_dir, media_path);
    if state.status == "ready" {
        return Ok(state);
    }
    if !state.video {
        return Err("Vidéo manquante après préparation.".into());
    }
    // Secondary language still missing after extract + optional Whisper: keep
    // partial, with an explicit reason (shown under the episode title).
    if let Some(orig) = original_track {
        let secondary_missing = (orig == "en" && state.subs_en && !state.subs_fr)
            || (orig == "fr" && state.subs_fr && !state.subs_en);
        if secondary_missing {
            let mut partial = state;
            let missing = if orig == "en" { "français" } else { "anglais" };
            let ready = if orig == "en" { "anglais" } else { "français" };
            partial.message = Some(format!(
                "Pas de sous-titres {missing} dans le fichier ({ready} prêt, pas de piste audio {missing} pour Whisper)."
            ));
            return Ok(partial);
        }
    }
    let detail = match (
        state.subs_en,
        state.subs_fr,
        original_track,
        original_language,
    ) {
        (false, true, Some("en"), _) => {
            "Pas de sous-titres anglais (Whisper a échoué ou est indisponible)."
        }
        (true, false, Some("fr"), _) => {
            "Pas de sous-titres français (Whisper a échoué ou est indisponible)."
        }
        (false, true, _, _) => "Pas de sous-titres anglais dans le fichier (français trouvé).",
        (true, false, _, _) => "Pas de sous-titres français dans le fichier (anglais trouvé).",
        (false, false, None, None) => {
            "Langue originale inconnue (TMDB). Impossible de générer les sous-titres."
        }
        (false, false, None, Some(_)) => {
            "Langue originale hors EN/FR : transcription non prise en charge."
        }
        (false, false, _, _) => "Pas de sous-titres EN/FR dans le fichier source.",
        _ => "Préparation incomplète : pistes manquantes dans le fichier source.",
    };
    Err(detail.into())
}

fn reject_unreadable_source(source: &Path) -> Result<(), String> {
    use std::io::Read;
    let meta = fs::metadata(source).map_err(|e| format!("Impossible de lire le fichier : {e}"))?;
    if meta.len() == 0 {
        return Err("Fichier source vide.".into());
    }
    let mut file =
        fs::File::open(source).map_err(|e| format!("Impossible d’ouvrir le fichier : {e}"))?;
    let mut buf = [0u8; 64];
    let n = file
        .read(&mut buf)
        .map_err(|e| format!("Impossible de lire le fichier : {e}"))?;
    if n == 0 || buf[..n].iter().all(|&b| b == 0) {
        return Err(
            "Fichier source corrompu ou incomplet (données vides). Remplacez le fichier média."
                .into(),
        );
    }
    Ok(())
}

struct PrepTrackFlags {
    video: bool,
    en_source: String,
    fr_source: String,
}

fn notify_progress<F>(
    registry: &PrepRegistry,
    media_path: &str,
    progress: u8,
    message: &str,
    flags: PrepTrackFlags,
    on_update: &mut F,
) where
    F: FnMut(&EpisodePrep),
{
    let state = EpisodePrep {
        status: "processing".into(),
        video: flags.video,
        subs_en: flags.en_source != SUB_SOURCE_MISSING,
        subs_fr: flags.fr_source != SUB_SOURCE_MISSING,
        en_source: flags.en_source,
        fr_source: flags.fr_source,
        progress: Some(progress.min(99)),
        message: Some(message.into()),
    };
    registry.set_job(media_path, state.clone());
    on_update(&state);
}

fn ensure_subtitle<F>(
    registry: &PrepRegistry,
    media_path: &str,
    source: &Path,
    lang: &str,
    original_track: Option<&str>,
    original_language_raw: Option<&str>,
    on_update: &mut F,
) -> Result<(), String>
where
    F: FnMut(&EpisodePrep),
{
    let out = sub_cache_path(&registry.cache_dir, media_path, lang);
    if out.is_file() {
        // Legacy cache without sources.json counts as native.
        let recorded = load_sources_file(&registry.cache_dir, media_path);
        let existing = if lang == "fr" {
            recorded.fr.as_deref()
        } else {
            recorded.en.as_deref()
        };
        if existing.is_none() {
            write_sub_source(&registry.cache_dir, media_path, lang, SUB_SOURCE_NATIVE);
        }
        return Ok(());
    }

    if let Some(sidecar) = find_sidecar_sub(source, lang) {
        convert_sub_to_vtt(&sidecar, &out)?;
        write_sub_source(&registry.cache_dir, media_path, lang, SUB_SOURCE_NATIVE);
        return Ok(());
    }

    if let Some(index) = probe_subtitle_index(source, lang)? {
        extract_embedded_sub(source, &out, index)?;
        write_sub_source(&registry.cache_dir, media_path, lang, SUB_SOURCE_NATIVE);
        return Ok(());
    }

    // Whisper for the original language, or for the secondary when a matching
    // audio track exists (e.g. MULTi with a French dub but no FR subtitles).
    let whisper_secondary =
        original_track != Some(lang) && probe_audio_index_matching_lang(source, lang)?.is_some();
    if original_track == Some(lang) || whisper_secondary {
        let disk = inspect_disk(&registry.cache_dir, media_path);
        notify_progress(
            registry,
            media_path,
            82,
            if whisper_secondary {
                "Transcription (piste audio)…"
            } else {
                "Transcription des sous-titres…"
            },
            PrepTrackFlags {
                video: true,
                en_source: disk.en_source,
                fr_source: disk.fr_source,
            },
            on_update,
        );
        registry.ensure_not_cancelled(media_path)?;
        transcribe_with_whisper(registry, media_path, source, &out, lang, on_update)?;
        write_sub_source(&registry.cache_dir, media_path, lang, SUB_SOURCE_GENERATED);
        return Ok(());
    }

    let _ = original_language_raw;
    Ok(())
}

fn whisper_cli_bin() -> Result<&'static Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match PATH.get_or_init(|| find_media_tool("whisper-cli")) {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

fn ensure_whisper_model<F>(
    registry: &PrepRegistry,
    media_path: &str,
    on_update: &mut F,
) -> Result<PathBuf, String>
where
    F: FnMut(&EpisodePrep),
{
    let spec = crate::whisper_model::ModelSpec::production();
    let dest = crate::whisper_model::model_path(&registry.app_data_dir, &spec);
    if crate::whisper_model::is_complete_model(&dest, spec.expected_bytes) {
        return Ok(dest);
    }

    let disk = inspect_disk(&registry.cache_dir, media_path);
    let mut last_pct: i16 = -1;
    crate::whisper_model::ensure_model(
        &registry.app_data_dir,
        &spec,
        || registry.is_cancelled(media_path),
        |downloaded, total| {
            let pct = if total > 0 {
                ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 99.0) as u8
            } else {
                0
            };
            if i16::from(pct) < last_pct + 2 && downloaded < total {
                return;
            }
            last_pct = i16::from(pct);
            let overall = 78 + (u16::from(pct) * 8 / 100) as u8;
            let downloaded_mb = downloaded / 1_000_000;
            let total_mb = total / 1_000_000;
            let message = if total > 0 {
                format!("Téléchargement du modèle de sous-titres… {downloaded_mb} / {total_mb} Mo")
            } else {
                "Téléchargement du modèle de sous-titres…".into()
            };
            notify_progress(
                registry,
                media_path,
                overall,
                &message,
                PrepTrackFlags {
                    video: true,
                    en_source: disk.en_source.clone(),
                    fr_source: disk.fr_source.clone(),
                },
                on_update,
            );
        },
    )
}

fn audio_lang_prefixes(lang: &str) -> &'static [&'static str] {
    if lang == "fr" {
        &["fre", "fra", "fr"]
    } else {
        &["eng", "en"]
    }
}

/// Audio stream index tagged for `lang`, or `None` if no matching track.
fn probe_audio_index_matching_lang(source: &Path, lang: &str) -> Result<Option<usize>, String> {
    let prefixes = audio_lang_prefixes(lang);
    let streams = probe_streams(source)?;
    for stream in streams {
        if stream.get("codec_type").and_then(|c| c.as_str()) != Some("audio") {
            continue;
        }
        let tag = stream_lang(&stream);
        if prefixes.iter().any(|p| tag == *p || tag.starts_with(p)) {
            if let Some(index) = stream.get("index").and_then(|i| i.as_u64()) {
                return Ok(Some(index as usize));
            }
        }
    }
    Ok(None)
}

fn probe_audio_index_for_lang(source: &Path, lang: &str) -> Result<usize, String> {
    if let Some(index) = probe_audio_index_matching_lang(source, lang)? {
        return Ok(index);
    }
    let streams = probe_streams(source)?;
    let audio: Vec<&Value> = streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"))
        .collect();
    let chosen = audio.first().ok_or_else(|| {
        "Aucune piste audio (fichier illisible, corrompu, ou sans son).".to_string()
    })?;
    chosen
        .get("index")
        .and_then(|i| i.as_u64())
        .map(|i| i as usize)
        .ok_or_else(|| "Index audio invalide".into())
}

fn transcribe_with_whisper<F>(
    registry: &PrepRegistry,
    media_path: &str,
    source: &Path,
    out: &Path,
    lang: &str,
    on_update: &mut F,
) -> Result<(), String>
where
    F: FnMut(&EpisodePrep),
{
    registry.ensure_not_cancelled(media_path)?;
    let cli = whisper_cli_bin()?;
    let model = ensure_whisper_model(registry, media_path, on_update)?;
    registry.ensure_not_cancelled(media_path)?;
    let disk = inspect_disk(&registry.cache_dir, media_path);
    notify_progress(
        registry,
        media_path,
        88,
        "Transcription des sous-titres…",
        PrepTrackFlags {
            video: true,
            en_source: disk.en_source,
            fr_source: disk.fr_source,
        },
        on_update,
    );
    fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;

    let audio_index = probe_audio_index_for_lang(source, lang)?;
    let wav = out.with_extension("whisper.wav");
    let vtt_stem = out.with_extension("whisper");
    let vtt_out = PathBuf::from(format!("{}.vtt", vtt_stem.to_string_lossy()));
    let _ = fs::remove_file(&wav);
    let _ = fs::remove_file(&vtt_out);

    run_ffmpeg(&[
        "-y",
        "-i",
        &source.to_string_lossy(),
        "-map",
        &format!("0:{audio_index}"),
        "-ac",
        "1",
        "-ar",
        "16000",
        &wav.to_string_lossy(),
    ])?;
    registry.ensure_not_cancelled(media_path)?;

    let mut child = command_hidden(cli)
        .args([
            "-m",
            &model.to_string_lossy(),
            "-l",
            lang,
            "-f",
            &wav.to_string_lossy(),
            "-ovtt",
            "-of",
            &vtt_stem.to_string_lossy(),
            "-np",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Impossible de lancer Whisper : {e}"))?;
    registry.register_pid(media_path, child.id());

    let status = loop {
        if registry.is_cancelled(media_path) {
            let _ = child.kill();
            let _ = child.wait();
            registry.clear_pid(media_path);
            let _ = fs::remove_file(&wav);
            let _ = fs::remove_file(&vtt_out);
            return Err(CANCELLED_MSG.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => {
                registry.clear_pid(media_path);
                let _ = fs::remove_file(&wav);
                return Err(e.to_string());
            }
        }
    };
    registry.clear_pid(media_path);

    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut stderr);
    }
    let _ = fs::remove_file(&wav);
    if !status.success() {
        let tip = stderr
            .trim()
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fs::remove_file(&vtt_out);
        return Err(if tip.is_empty() {
            format!("Whisper a échoué (code {:?}).", status.code())
        } else {
            tip
        });
    }
    if !vtt_out.is_file() {
        return Err("Whisper n'a pas produit de fichier VTT.".into());
    }
    validate_vtt(&vtt_out)?;
    fs::rename(&vtt_out, out).map_err(|e| e.to_string())?;
    Ok(())
}

fn find_sidecar_sub(source: &Path, lang: &str) -> Option<PathBuf> {
    let parent = source.parent()?;
    let file_stem = source.file_stem()?.to_string_lossy();
    let candidates = [
        format!("{}.{}.vtt", file_stem, lang),
        format!("{}.{}.srt", file_stem, lang),
        format!("{}.vtt", file_stem),
        format!("{}.srt", file_stem),
    ];
    for name in candidates {
        let path = parent.join(&name);
        if path.is_file() {
            if lang == "fr"
                && (name.ends_with(".vtt") || name.ends_with(".srt"))
                && name.contains(".en.")
            {
                continue;
            }
            if lang == "en" && name.contains(".fr.") {
                continue;
            }
            // Bare .srt/.vtt only counts for English by convention
            if (name.ends_with(".srt") || name.ends_with(".vtt"))
                && !name.contains(".en.")
                && !name.contains(".fr.")
                && lang != "en"
            {
                continue;
            }
            return Some(path);
        }
    }
    None
}

fn convert_sub_to_vtt(source: &Path, out: &Path) -> Result<(), String> {
    if source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("vtt"))
        .unwrap_or(false)
    {
        fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
        fs::copy(source, out).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let tmp = out.with_extension("partial.vtt");
    run_ffmpeg(&[
        "-y",
        "-i",
        &source.to_string_lossy(),
        "-f",
        "webvtt",
        &tmp.to_string_lossy(),
    ])?;
    validate_vtt(&tmp)?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

fn extract_embedded_sub(source: &Path, out: &Path, stream_index: usize) -> Result<(), String> {
    let tmp = out.with_extension("partial.vtt");
    run_ffmpeg(&[
        "-y",
        "-i",
        &source.to_string_lossy(),
        "-map",
        &format!("0:{stream_index}"),
        "-f",
        "webvtt",
        &tmp.to_string_lossy(),
    ])?;
    validate_vtt(&tmp)?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

fn validate_vtt(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if !content.contains("-->") {
        let _ = fs::remove_file(path);
        return Err("Piste de sous-titres vide".into());
    }
    Ok(())
}

fn remux_video<F>(
    registry: &PrepRegistry,
    media_path: &str,
    source: &Path,
    out: &Path,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u8),
{
    fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let audio_index = probe_english_audio_index(source)?;
    let duration_sec = probe_duration_seconds(source);
    let tmp = out.with_extension("partial.mp4");
    let _ = fs::remove_file(&tmp);
    let tmp_str = tmp.to_string_lossy().into_owned();
    let source_str = source.to_string_lossy().into_owned();
    let audio_map = format!("0:{audio_index}");

    let mut child = command_hidden(ffmpeg_bin()?)
        .args([
            "-y",
            "-i",
            &source_str,
            "-map",
            "0:v:0",
            "-map",
            &audio_map,
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-b:a",
            "160k",
            "-movflags",
            "+faststart",
            "-progress",
            "pipe:1",
            "-nostats",
            &tmp_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Impossible de lancer la conversion : {e}"))?;

    registry.register_pid(media_path, child.id());

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg stdout manquant".to_string())?;
    let mut last_pct: i16 = -1;
    let mut cancelled = false;
    for line in BufReader::new(stdout).lines() {
        if registry.is_cancelled(media_path) {
            cancelled = true;
            let _ = child.kill();
            break;
        }
        let line = match line {
            Ok(line) => line,
            Err(_) if registry.is_cancelled(media_path) => {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            Err(e) => {
                registry.clear_pid(media_path);
                return Err(e.to_string());
            }
        };
        if let Some(raw) = line.strip_prefix("out_time_ms=") {
            let Ok(out_time_ms) = raw.parse::<f64>() else {
                continue;
            };
            let pct = if let Some(duration) = duration_sec.filter(|d| *d > 0.0) {
                ((out_time_ms / 1_000_000.0 / duration) * 100.0)
                    .clamp(0.0, 99.0)
                    .round() as u8
            } else if last_pct < 5 {
                5
            } else {
                continue;
            };
            if i16::from(pct) >= last_pct + 2 {
                last_pct = i16::from(pct);
                on_progress(pct);
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    registry.clear_pid(media_path);
    if cancelled || registry.is_cancelled(media_path) {
        let _ = fs::remove_file(&tmp);
        return Err(CANCELLED_MSG.into());
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut stderr);
    }
    if !status.success() {
        let tip = stderr
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fs::remove_file(&tmp);
        return Err(if tip.is_empty() {
            format!("Conversion vidéo impossible (code {status})")
        } else {
            format!("Conversion vidéo impossible : {tip}")
        });
    }
    on_progress(100);
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

fn probe_duration_seconds(source: &Path) -> Option<f64> {
    let output = command_hidden(ffprobe_bin().ok()?)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_entries",
            "format=duration",
            &source.to_string_lossy(),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: Value = serde_json::from_slice(&output.stdout).ok()?;
    let duration = parsed
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())?;
    if duration.is_finite() && duration > 0.0 {
        Some(duration)
    } else {
        None
    }
}

fn probe_streams(source: &Path) -> Result<Vec<Value>, String> {
    let output = command_hidden(ffprobe_bin()?)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            &source.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("Impossible d’analyser le fichier : {e}"))?;
    if !output.status.success() {
        let tip = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if tip.is_empty() {
            "Fichier illisible ou corrompu.".into()
        } else {
            tip
        });
    }
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Analyse média invalide : {e}"))?;
    Ok(parsed
        .get("streams")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default())
}

fn stream_lang(stream: &Value) -> String {
    stream
        .get("tags")
        .and_then(|t| t.get("language"))
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn probe_english_audio_index(source: &Path) -> Result<usize, String> {
    let streams = probe_streams(source)?;
    let audio: Vec<&Value> = streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"))
        .collect();
    let eng = audio.iter().find(|s| {
        let lang = stream_lang(s);
        lang.starts_with("eng") || lang == "en"
    });
    let chosen = eng.or(audio.first()).ok_or_else(|| {
        "Aucune piste audio (fichier illisible, corrompu, ou sans son).".to_string()
    })?;
    chosen
        .get("index")
        .and_then(|i| i.as_u64())
        .map(|i| i as usize)
        .ok_or_else(|| "Index audio invalide".into())
}

fn probe_subtitle_index(source: &Path, lang: &str) -> Result<Option<usize>, String> {
    let prefixes: &[&str] = if lang == "fr" {
        &["fre", "fra", "fr"]
    } else {
        &["eng", "en"]
    };
    let streams = probe_streams(source)?;
    for stream in streams {
        if stream.get("codec_type").and_then(|c| c.as_str()) != Some("subtitle") {
            continue;
        }
        let tag = stream_lang(&stream);
        if prefixes.iter().any(|p| tag == *p || tag.starts_with(p)) {
            if let Some(index) = stream.get("index").and_then(|i| i.as_u64()) {
                return Ok(Some(index as usize));
            }
        }
    }
    Ok(None)
}

fn run_ffmpeg(args: &[&str]) -> Result<(), String> {
    let output = command_hidden(ffmpeg_bin()?)
        .args(args)
        .output()
        .map_err(|e| format!("Impossible de lancer la conversion : {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tip = stderr
        .lines()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Err(if tip.is_empty() {
        format!("Conversion impossible (code {})", output.status)
    } else {
        tip
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tracks_status_matrix() {
        let missing = EpisodePrep::from_tracks(false, SUB_SOURCE_MISSING, SUB_SOURCE_MISSING);
        assert_eq!(missing.status, "missing");
        assert!(missing.message.is_none());

        let partial = EpisodePrep::from_tracks(true, SUB_SOURCE_NATIVE, SUB_SOURCE_MISSING);
        assert_eq!(partial.status, "partial");
        assert_eq!(partial.message.as_deref(), Some("EN natif, FR manquant"));

        let ready = EpisodePrep::from_tracks(true, SUB_SOURCE_NATIVE, SUB_SOURCE_GENERATED);
        assert_eq!(ready.status, "ready");
        assert_eq!(ready.progress, Some(100));
        assert!(ready.message.is_none());
    }

    #[test]
    fn resolve_track_source_rules() {
        assert_eq!(
            resolve_track_source(false, Some(SUB_SOURCE_NATIVE)),
            SUB_SOURCE_MISSING
        );
        assert_eq!(
            resolve_track_source(true, Some(SUB_SOURCE_GENERATED)),
            SUB_SOURCE_GENERATED
        );
        assert_eq!(resolve_track_source(true, None), SUB_SOURCE_NATIVE);
        assert_eq!(resolve_track_source(true, Some("weird")), SUB_SOURCE_NATIVE);
    }

    #[test]
    fn cache_key_stable_and_filename_parse() {
        let a = cache_key("/media/show.mkv");
        let b = cache_key("/media/show.mkv");
        assert_eq!(a, b);
        assert!(a.starts_with('p'));
        assert_eq!(a.len(), 17);

        assert_eq!(
            cache_key_from_filename(&format!("{a}.mp4")),
            Some(a.clone())
        );
        assert_eq!(
            cache_key_from_filename(&format!("{a}.en.vtt")),
            Some(a.clone())
        );
        assert_eq!(
            cache_key_from_filename(&format!("{a}.sources.json")),
            Some(a)
        );
        assert_eq!(cache_key_from_filename("junk.mp4"), None);
    }

    #[test]
    fn stream_lang_reads_tags() {
        let stream = serde_json::json!({
            "tags": { "language": "eng" }
        });
        assert_eq!(stream_lang(&stream), "eng");
        assert_eq!(stream_lang(&serde_json::json!({})), "");
    }
}
