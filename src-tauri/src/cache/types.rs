//! Data types for browser-ready cache: remuxed MP4 + EN/FR WebVTT.

use serde::{Deserialize, Serialize};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Cache preparation lifecycle for an episode (JSON: camelCase / lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrepStatus {
    Missing,
    Partial,
    Ready,
    Queued,
    Processing,
    Error,
}

/// How an EN/FR subtitle track was obtained for the cache.
/// `native` = sidecar or embedded, `generated` = Whisper / translation, `missing` = absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubTrackSource {
    Native,
    Generated,
    Missing,
}

impl SubTrackSource {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Generated => "generated",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodePrep {
    pub status: PrepStatus,
    pub video: bool,
    pub subs_en: bool,
    pub subs_fr: bool,
    pub en_source: SubTrackSource,
    pub fr_source: SubTrackSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Default for EpisodePrep {
    fn default() -> Self {
        Self {
            status: PrepStatus::Missing,
            video: false,
            subs_en: false,
            subs_fr: false,
            en_source: SubTrackSource::Missing,
            fr_source: SubTrackSource::Missing,
            progress: None,
            message: None,
        }
    }
}

impl EpisodePrep {
    pub(super) fn from_tracks(
        video: bool,
        en_source: SubTrackSource,
        fr_source: SubTrackSource,
    ) -> Self {
        let subs_en = en_source != SubTrackSource::Missing;
        let subs_fr = fr_source != SubTrackSource::Missing;
        let status = if video && subs_en && subs_fr {
            PrepStatus::Ready
        } else if video || subs_en || subs_fr {
            PrepStatus::Partial
        } else {
            PrepStatus::Missing
        };
        let message = match status {
            PrepStatus::Missing | PrepStatus::Ready => None,
            _ => Some(constitution_message(en_source, fr_source)),
        };
        Self {
            status,
            video,
            subs_en,
            subs_fr,
            en_source,
            fr_source,
            progress: if status == PrepStatus::Ready {
                Some(100)
            } else {
                None
            },
            message,
        }
    }
}

fn source_label_fr(source: SubTrackSource) -> &'static str {
    match source {
        SubTrackSource::Native => "natif",
        SubTrackSource::Generated => "généré",
        SubTrackSource::Missing => "manquant",
    }
}

fn constitution_message(en_source: SubTrackSource, fr_source: SubTrackSource) -> String {
    format!(
        "EN {}, FR {}",
        source_label_fr(en_source),
        source_label_fr(fr_source)
    )
}

pub(super) fn cache_key(media_path: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    media_path.hash(&mut hasher);
    format!("p{:016x}", hasher.finish())
}

pub(super) fn video_cache_path(cache_dir: &Path, media_path: &str) -> PathBuf {
    cache_dir
        .join("video")
        .join(format!("{}.mp4", cache_key(media_path)))
}

pub(super) fn sub_cache_path(cache_dir: &Path, media_path: &str, lang: &str) -> PathBuf {
    cache_dir
        .join("subs")
        .join(format!("{}.{}.vtt", cache_key(media_path), lang))
}

pub(super) fn sources_cache_path(cache_dir: &Path, media_path: &str) -> PathBuf {
    cache_dir
        .join("subs")
        .join(format!("{}.sources.json", cache_key(media_path)))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(super) struct SubSourcesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fr: Option<String>,
}

pub(super) fn load_sources_file(cache_dir: &Path, media_path: &str) -> SubSourcesFile {
    let path = sources_cache_path(cache_dir, media_path);
    let Ok(raw) = fs::read_to_string(&path) else {
        return SubSourcesFile::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub(super) fn write_sub_source(
    cache_dir: &Path,
    media_path: &str,
    lang: &str,
    source: SubTrackSource,
) {
    let mut file = load_sources_file(cache_dir, media_path);
    match lang {
        "fr" => file.fr = Some(source.as_str().into()),
        _ => file.en = Some(source.as_str().into()),
    }
    let path = sources_cache_path(cache_dir, media_path);
    if let Ok(raw) = serde_json::to_string_pretty(&file) {
        let _ = fs::write(path, raw);
    }
}

pub(super) fn resolve_track_source(has_file: bool, recorded: Option<&str>) -> SubTrackSource {
    if !has_file {
        return SubTrackSource::Missing;
    }
    match recorded {
        Some("generated") => SubTrackSource::Generated,
        Some("native") | Some(_) | None => SubTrackSource::Native,
    }
}

pub(super) fn inspect_disk(cache_dir: &Path, media_path: &str) -> EpisodePrep {
    let video = video_cache_path(cache_dir, media_path).is_file();
    let subs_en = sub_cache_path(cache_dir, media_path, "en").is_file();
    let subs_fr = sub_cache_path(cache_dir, media_path, "fr").is_file();
    let recorded = load_sources_file(cache_dir, media_path);
    let en_source = resolve_track_source(subs_en, recorded.en.as_deref());
    let fr_source = resolve_track_source(subs_fr, recorded.fr.as_deref());
    EpisodePrep::from_tracks(video, en_source, fr_source)
}

pub(super) fn dir_total_bytes(dir: &Path) -> u64 {
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

pub(super) fn cache_key_from_filename(name: &str) -> Option<String> {
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

pub(super) fn oldest_cache_key(cache_dir: &Path, protect_key: Option<&str>) -> Option<String> {
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

pub(super) fn remove_cache_key(cache_dir: &Path, key: &str) {
    let video = cache_dir.join("video").join(format!("{key}.mp4"));
    let en = cache_dir.join("subs").join(format!("{key}.en.vtt"));
    let fr = cache_dir.join("subs").join(format!("{key}.fr.vtt"));
    let sources = cache_dir.join("subs").join(format!("{key}.sources.json"));
    let _ = fs::remove_file(video);
    let _ = fs::remove_file(en);
    let _ = fs::remove_file(fr);
    let _ = fs::remove_file(sources);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tracks_status_matrix() {
        let missing =
            EpisodePrep::from_tracks(false, SubTrackSource::Missing, SubTrackSource::Missing);
        assert_eq!(missing.status, PrepStatus::Missing);
        assert!(missing.message.is_none());

        let partial =
            EpisodePrep::from_tracks(true, SubTrackSource::Native, SubTrackSource::Missing);
        assert_eq!(partial.status, PrepStatus::Partial);
        assert_eq!(partial.message.as_deref(), Some("EN natif, FR manquant"));

        let ready =
            EpisodePrep::from_tracks(true, SubTrackSource::Native, SubTrackSource::Generated);
        assert_eq!(ready.status, PrepStatus::Ready);
        assert_eq!(ready.progress, Some(100));
        assert!(ready.message.is_none());
    }

    #[test]
    fn resolve_track_source_rules() {
        assert_eq!(
            resolve_track_source(false, Some("native")),
            SubTrackSource::Missing
        );
        assert_eq!(
            resolve_track_source(true, Some("generated")),
            SubTrackSource::Generated
        );
        assert_eq!(resolve_track_source(true, None), SubTrackSource::Native);
        assert_eq!(
            resolve_track_source(true, Some("weird")),
            SubTrackSource::Native
        );
    }

    #[test]
    fn prep_enums_serialize_lowercase() {
        assert_eq!(
            serde_json::to_string(&PrepStatus::Missing).unwrap(),
            "\"missing\""
        );
        assert_eq!(
            serde_json::to_string(&PrepStatus::Partial).unwrap(),
            "\"partial\""
        );
        assert_eq!(
            serde_json::to_string(&PrepStatus::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&PrepStatus::Queued).unwrap(),
            "\"queued\""
        );
        assert_eq!(
            serde_json::to_string(&PrepStatus::Processing).unwrap(),
            "\"processing\""
        );
        assert_eq!(
            serde_json::to_string(&PrepStatus::Error).unwrap(),
            "\"error\""
        );
        assert_eq!(
            serde_json::to_string(&SubTrackSource::Native).unwrap(),
            "\"native\""
        );
        assert_eq!(
            serde_json::to_string(&SubTrackSource::Generated).unwrap(),
            "\"generated\""
        );
        assert_eq!(
            serde_json::to_string(&SubTrackSource::Missing).unwrap(),
            "\"missing\""
        );
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
}
