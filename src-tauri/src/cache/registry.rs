//! PrepRegistry: job management, cancellation, and preparation orchestration.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::ffmpeg::{
    notify_progress, reject_unreadable_source, remux_video, PrepTrackFlags, CANCELLED_MSG,
};
use super::subs::ensure_subtitle;
use super::types::{
    cache_key, dir_total_bytes, inspect_disk, oldest_cache_key, remove_cache_key,
    sources_cache_path, sub_cache_path, video_cache_path, EpisodePrep, PrepStatus, SubTrackSource,
};

#[cfg(windows)]
use super::ffmpeg::command_hidden;

pub struct PrepRegistry {
    pub(super) app_data_dir: PathBuf,
    pub(super) cache_dir: PathBuf,
    jobs: Mutex<HashMap<String, EpisodePrep>>,
    work: Mutex<()>,
    cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
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

    pub(super) fn reset_cancel_flag(&self, media_path: &str) {
        self.cancel_flag(media_path).store(false, Ordering::SeqCst);
    }

    pub(super) fn is_cancelled(&self, media_path: &str) -> bool {
        self.cancel_flag(media_path).load(Ordering::SeqCst)
    }

    pub(super) fn ensure_not_cancelled(&self, media_path: &str) -> Result<(), String> {
        if self.is_cancelled(media_path) {
            Err(CANCELLED_MSG.into())
        } else {
            Ok(())
        }
    }

    pub(super) fn register_pid(&self, media_path: &str, pid: u32) {
        if let Ok(mut pids) = self.active_pids.lock() {
            pids.insert(media_path.to_string(), pid);
        }
    }

    pub(super) fn clear_pid(&self, media_path: &str) {
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
            jobs.get(media_path).map(|j| j.status)
        };

        match status {
            Some(PrepStatus::Queued) => {
                self.clear_job(media_path);
                self.reset_cancel_flag(media_path);
                inspect_disk(&self.cache_dir, media_path)
            }
            Some(PrepStatus::Processing) => {
                let disk = inspect_disk(&self.cache_dir, media_path);
                EpisodePrep {
                    status: PrepStatus::Processing,
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
                if matches!(
                    job.status,
                    PrepStatus::Processing | PrepStatus::Queued | PrepStatus::Error
                ) {
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
            if matches!(
                current.status,
                PrepStatus::Ready | PrepStatus::Processing | PrepStatus::Queued
            ) {
                continue;
            }
            self.set_job(
                path,
                EpisodePrep {
                    status: PrepStatus::Queued,
                    video: current.video,
                    subs_en: current.subs_en,
                    subs_fr: current.subs_fr,
                    en_source: current.en_source,
                    fr_source: current.fr_source,
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
            let conn = crate::db::open_migrated(&db_path.to_path_buf())?;
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
                    status: PrepStatus::Error,
                    video: disk.video,
                    subs_en: disk.subs_en,
                    subs_fr: disk.subs_fr,
                    en_source: disk.en_source,
                    fr_source: disk.fr_source,
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
        if current.status == PrepStatus::Ready {
            return current;
        }
        if current.status == PrepStatus::Processing {
            return current;
        }

        let work = match self.work.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                let queued = EpisodePrep {
                    status: PrepStatus::Queued,
                    video: current.video,
                    subs_en: current.subs_en,
                    subs_fr: current.subs_fr,
                    en_source: current.en_source,
                    fr_source: current.fr_source,
                    progress: None,
                    message: Some("En file".into()),
                };
                self.set_job(media_path, queued.clone());
                on_update(&queued);
                self.work.lock().expect("prep work lock")
            }
        };

        let on_disk = inspect_disk(&self.cache_dir, media_path);
        if on_disk.status == PrepStatus::Ready {
            self.clear_job(media_path);
            on_update(&on_disk);
            return on_disk;
        }

        let started = EpisodePrep {
            status: PrepStatus::Processing,
            video: on_disk.video,
            subs_en: on_disk.subs_en,
            subs_fr: on_disk.subs_fr,
            en_source: on_disk.en_source,
            fr_source: on_disk.fr_source,
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
                    status: PrepStatus::Error,
                    video: disk.video,
                    subs_en: disk.subs_en,
                    subs_fr: disk.subs_fr,
                    en_source: disk.en_source,
                    fr_source: disk.fr_source,
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
                en_source: SubTrackSource::Missing,
                fr_source: SubTrackSource::Missing,
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
                    en_source: SubTrackSource::Missing,
                    fr_source: SubTrackSource::Missing,
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
            en_source: SubTrackSource::Missing,
            fr_source: SubTrackSource::Missing,
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
            en_source: after_en.en_source,
            fr_source: after_en.fr_source,
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
            en_source: after_fr.en_source,
            fr_source: after_fr.fr_source,
        },
        on_update,
    );

    let state = inspect_disk(&registry.cache_dir, media_path);
    if state.status == PrepStatus::Ready {
        return Ok(state);
    }
    if !state.video {
        return Err("Vidéo manquante après préparation.".into());
    }
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

/// Attach live prep status onto every episode in a catalog snapshot.
pub fn enrich_catalog_prep(snap: &mut crate::scan::CatalogSnapshot, prep: &PrepRegistry) {
    for series in &mut snap.series {
        for season in &mut series.seasons {
            for episode in &mut season.episodes {
                episode.prep = prep.inspect(&episode.path);
            }
        }
    }
}
