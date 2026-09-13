//! Multi-layout scan: each video is a candidate, grouped by normalized title
//! (then TMDB-canonicalized if a key is present). The parent folder is never the series identity.

use crate::parse::{self, ParsedMedia};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEpisode {
    pub id: String,
    pub season: i32,
    pub episode: i32,
    pub label: String,
    pub path: String,
    pub title: Option<String>,
    pub poster_url: Option<String>,
    pub synopsis: Option<String>,
    pub confidence: f32,
    pub tmdb_checked: bool,
    pub prep: crate::cache::EpisodePrep,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSeason {
    pub number: i32,
    pub title: Option<String>,
    pub poster_url: Option<String>,
    pub synopsis: Option<String>,
    pub episodes: Vec<CatalogEpisode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSeries {
    pub id: String,
    pub title: String,
    pub root: String,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub synopsis: Option<String>,
    pub tmdb_id: Option<i32>,
    /// TMDB ISO 639-1 (e.g. en, fr). Used to decide Whisper language.
    pub original_language: Option<String>,
    pub display_title: Option<String>,
    pub seasons: Vec<CatalogSeason>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMovie {
    pub id: String,
    pub title: String,
    pub path: String,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub synopsis: Option<String>,
    pub tmdb_id: Option<i32>,
    /// TMDB ISO 639-1 (e.g. en, fr). Used to decide Whisper language.
    pub original_language: Option<String>,
    pub year: Option<i32>,
    pub display_title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub scanned_at: String,
    pub series: Vec<CatalogSeries>,
    pub movies: Vec<CatalogMovie>,
}

#[derive(Debug, Clone)]
pub struct PathMapping {
    pub path: String,
    pub kind: String,
    pub series_id: Option<String>,
    pub episode_id: Option<String>,
    pub movie_id: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub parsed_title: String,
    pub confidence: f32,
}

struct SeriesAccum {
    title: String,
    roots: BTreeMap<String, usize>,
    episodes: Vec<(i32, i32, String, f32)>,
}

fn collect_videos(root: &Path, videos: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            collect_videos(&path, videos)?;
        } else if file_type.is_file() && parse::is_video(&path) {
            videos.push(path);
        }
    }
    Ok(())
}

fn local_series_id(normalized: &str) -> String {
    let slug = normalized.replace(' ', "-");
    if slug.is_empty() {
        "local:unknown".into()
    } else {
        format!("local:{slug}")
    }
}

fn episode_id(series_id: &str, season: i32, episode: i32) -> String {
    format!("{series_id}-s{season:02}e{episode:02}")
}

/// Scan roots and group by normalized title (independent of the folder tree).
pub fn scan_roots(roots: &[String]) -> Result<(CatalogSnapshot, Vec<PathMapping>), String> {
    let mut series_map: BTreeMap<String, SeriesAccum> = BTreeMap::new();
    let mut movies: Vec<CatalogMovie> = Vec::new();
    let mut movie_by_norm: BTreeMap<String, usize> = BTreeMap::new();
    let mut mappings: Vec<PathMapping> = Vec::new();

    for root in roots {
        let abs_root = PathBuf::from(root)
            .canonicalize()
            .map_err(|e| format!("{root}: {e}"))?;
        if !abs_root.is_dir() {
            return Err(format!("Pas un dossier: {root}"));
        }

        let mut videos = Vec::new();
        collect_videos(&abs_root, &mut videos)?;

        for abs in videos {
            let abs_str = abs.to_string_lossy().replace('\\', "/");
            match parse::parse_video(&abs) {
                ParsedMedia::Episode(ep) => {
                    if ep.normalized_title.is_empty() {
                        continue;
                    }
                    let key = ep.normalized_title.clone();
                    let series_id = local_series_id(&key);
                    let ep_id = episode_id(&series_id, ep.season, ep.episode);
                    let parent = abs
                        .parent()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();

                    let entry = series_map.entry(key).or_insert_with(|| SeriesAccum {
                        title: ep.title.clone(),
                        roots: BTreeMap::new(),
                        episodes: Vec::new(),
                    });
                    *entry.roots.entry(parent).or_insert(0) += 1;
                    // SxxExx dedup: keep the path with higher confidence
                    if let Some(existing) = entry
                        .episodes
                        .iter_mut()
                        .find(|(s, e, _, _)| *s == ep.season && *e == ep.episode)
                    {
                        if ep.confidence > existing.3 {
                            existing.2 = abs_str.clone();
                            existing.3 = ep.confidence;
                        }
                    } else {
                        entry.episodes.push((
                            ep.season,
                            ep.episode,
                            abs_str.clone(),
                            ep.confidence,
                        ));
                    }

                    mappings.push(PathMapping {
                        path: abs_str,
                        kind: "episode".into(),
                        series_id: Some(series_id),
                        episode_id: Some(ep_id),
                        movie_id: None,
                        season: Some(ep.season),
                        episode: Some(ep.episode),
                        parsed_title: ep.title,
                        confidence: ep.confidence,
                    });
                }
                ParsedMedia::Movie(movie) => {
                    let mut id =
                        local_series_id(&movie.normalized_title).replacen("local:", "movie:", 1);
                    if let Some(idx) = movie_by_norm.get(&movie.normalized_title) {
                        // Same title: keep the first, or create a path-based id if paths differ
                        let existing = &movies[*idx];
                        if existing.path != abs_str {
                            id = format!(
                                "movie:{}",
                                parse::normalize_title(&abs_str).replace(' ', "-")
                            );
                            movies.push(CatalogMovie {
                                id: id.clone(),
                                title: movie.title.clone(),
                                path: abs_str.clone(),
                                poster_url: None,
                                backdrop_url: None,
                                synopsis: None,
                                tmdb_id: None,
                                original_language: None,
                                year: movie.year,
                                display_title: None,
                            });
                        }
                    } else {
                        movie_by_norm.insert(movie.normalized_title.clone(), movies.len());
                        movies.push(CatalogMovie {
                            id: id.clone(),
                            title: movie.title.clone(),
                            path: abs_str.clone(),
                            poster_url: None,
                            backdrop_url: None,
                            synopsis: None,
                            tmdb_id: None,
                            original_language: None,
                            year: movie.year,
                            display_title: None,
                        });
                    }
                    mappings.push(PathMapping {
                        path: abs_str,
                        kind: "movie".into(),
                        series_id: None,
                        episode_id: None,
                        movie_id: Some(id),
                        season: None,
                        episode: None,
                        parsed_title: movie.title,
                        confidence: movie.confidence,
                    });
                }
            }
        }
    }

    let mut series: Vec<CatalogSeries> = series_map
        .into_iter()
        .map(|(norm, accum)| {
            let id = local_series_id(&norm);
            let root = accum
                .roots
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(path, _)| path)
                .unwrap_or_default();

            let mut by_season: BTreeMap<i32, Vec<CatalogEpisode>> = BTreeMap::new();
            for (season, episode, path, confidence) in accum.episodes {
                by_season.entry(season).or_default().push(CatalogEpisode {
                    id: episode_id(&id, season, episode),
                    season,
                    episode,
                    label: format!("S{season:02}E{episode:02}"),
                    path,
                    title: None,
                    poster_url: None,
                    synopsis: None,
                    confidence,
                    tmdb_checked: false,
                    prep: crate::cache::EpisodePrep::default(),
                });
            }
            for eps in by_season.values_mut() {
                eps.sort_by_key(|e| e.episode);
            }

            CatalogSeries {
                id,
                title: accum.title,
                root,
                poster_url: None,
                backdrop_url: None,
                synopsis: None,
                tmdb_id: None,
                original_language: None,
                display_title: None,
                seasons: by_season
                    .into_iter()
                    .map(|(number, episodes)| CatalogSeason {
                        number,
                        title: None,
                        poster_url: None,
                        synopsis: None,
                        episodes,
                    })
                    .collect(),
            }
        })
        .collect();

    series.sort_by_key(|a| a.title.to_lowercase());
    movies.sort_by_key(|a| a.title.to_lowercase());

    Ok((
        CatalogSnapshot {
            scanned_at: String::new(),
            series,
            movies,
        },
        mappings,
    ))
}

/// Rebuild the path → episode / movie mapping after canonicalization (stable ids).
pub fn mappings_from_snapshot(snapshot: &CatalogSnapshot) -> Vec<PathMapping> {
    let mut out = Vec::new();
    for series in &snapshot.series {
        for season in &series.seasons {
            for ep in &season.episodes {
                out.push(PathMapping {
                    path: ep.path.clone(),
                    kind: "episode".into(),
                    series_id: Some(series.id.clone()),
                    episode_id: Some(ep.id.clone()),
                    movie_id: None,
                    season: Some(ep.season),
                    episode: Some(ep.episode),
                    parsed_title: series
                        .display_title
                        .clone()
                        .unwrap_or_else(|| series.title.clone()),
                    confidence: ep.confidence,
                });
            }
        }
    }
    for movie in &snapshot.movies {
        out.push(PathMapping {
            path: movie.path.clone(),
            kind: "movie".into(),
            series_id: None,
            episode_id: None,
            movie_id: Some(movie.id.clone()),
            season: None,
            episode: None,
            parsed_title: movie
                .display_title
                .clone()
                .unwrap_or_else(|| movie.title.clone()),
            confidence: 0.6,
        });
    }
    out
}
pub fn apply_overrides(snapshot: &mut CatalogSnapshot, overrides: &[crate::db::CatalogOverride]) {
    for ov in overrides {
        match ov.kind.as_str() {
            "rename" => {
                if let Some(s) = snapshot.series.iter_mut().find(|s| s.id == ov.source_key) {
                    if let Some(title) = &ov.display_title {
                        s.title = title.clone();
                        s.display_title = Some(title.clone());
                    }
                }
            }
            "merge" => {
                let Some(target) = ov.target_key.as_ref() else {
                    continue;
                };
                let Some(src_idx) = snapshot.series.iter().position(|s| s.id == ov.source_key)
                else {
                    continue;
                };
                let Some(dst_idx) = snapshot.series.iter().position(|s| &s.id == target) else {
                    continue;
                };
                if src_idx == dst_idx {
                    continue;
                }
                let mut src = snapshot.series.remove(src_idx);
                let dst_idx = if dst_idx > src_idx {
                    dst_idx - 1
                } else {
                    dst_idx
                };
                let dst = &mut snapshot.series[dst_idx];
                for season in src.seasons.drain(..) {
                    if let Some(existing) =
                        dst.seasons.iter_mut().find(|s| s.number == season.number)
                    {
                        for ep in season.episodes {
                            if !existing.episodes.iter().any(|e| e.episode == ep.episode) {
                                existing.episodes.push(ep);
                            }
                        }
                        existing.episodes.sort_by_key(|e| e.episode);
                    } else {
                        dst.seasons.push(season);
                    }
                }
                dst.seasons.sort_by_key(|s| s.number);
            }
            "detach" => {
                // Planned schema: the UI will create a new id; for now scan is a no-op.
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_root() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("neolingua-scan-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn groups_futurama_release_folders() {
        let root = tmp_root();
        touch(&root.join("Futurama.S08E01.MULTi.1080p.WEB.x265-G/Futurama.S08E01.mkv"));
        touch(&root.join("Futurama.S08E02.MULTi.1080p.WEB.x265-G/video.mkv"));
        touch(&root.join("Futurama.S08E03.MULTi.1080p.mkv"));

        let (snap, _) = scan_roots(&[root.to_string_lossy().into()]).unwrap();
        assert_eq!(snap.series.len(), 1);
        assert_eq!(parse::normalize_title(&snap.series[0].title), "futurama");
        let eps: usize = snap.series[0]
            .seasons
            .iter()
            .map(|s| s.episodes.len())
            .sum();
        assert_eq!(eps, 3);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn groups_breaking_bad_seasons() {
        let root = tmp_root();
        for season in 1..=5 {
            for ep in 1..=2 {
                touch(&root.join(format!(
                    "Breaking Bad/season.{season:02}/Breaking.Bad.S{season:02}E{ep:02}.mkv"
                )));
            }
        }
        let (snap, _) = scan_roots(&[root.to_string_lossy().into()]).unwrap();
        assert_eq!(snap.series.len(), 1);
        assert_eq!(snap.series[0].seasons.len(), 5);
        assert_eq!(
            parse::normalize_title(&snap.series[0].title),
            "breaking bad"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn merges_foundation_folder_variants_locally() {
        let root = tmp_root();
        touch(&root.join("foundation/season.01/Foundation.S01E01.mkv"));
        touch(&root.join("Foundation S03/Foundation.S03E01.1080p.mkv"));
        let (snap, _) = scan_roots(&[root.to_string_lossy().into()]).unwrap();
        assert_eq!(snap.series.len(), 1);
        assert_eq!(parse::normalize_title(&snap.series[0].title), "foundation");
        assert_eq!(snap.series[0].seasons.len(), 2);
        fs::remove_dir_all(&root).ok();
    }

    fn sample_series(
        id: &str,
        title: &str,
        season: i32,
        episode: i32,
        path: &str,
    ) -> CatalogSeries {
        CatalogSeries {
            id: id.into(),
            title: title.into(),
            root: "/media".into(),
            poster_url: None,
            backdrop_url: None,
            synopsis: None,
            tmdb_id: None,
            original_language: None,
            display_title: Some(title.into()),
            seasons: vec![CatalogSeason {
                number: season,
                title: None,
                poster_url: None,
                synopsis: None,
                episodes: vec![CatalogEpisode {
                    id: format!("{id}-s{season:02}e{episode:02}"),
                    season,
                    episode,
                    label: format!("S{season:02}E{episode:02}"),
                    path: path.into(),
                    title: None,
                    poster_url: None,
                    synopsis: None,
                    confidence: 0.9,
                    tmdb_checked: false,
                    prep: crate::cache::EpisodePrep::default(),
                }],
            }],
        }
    }

    #[test]
    fn apply_overrides_rename_and_merge() {
        let mut snap = CatalogSnapshot {
            scanned_at: "now".into(),
            series: vec![
                sample_series("local:a", "Show A", 1, 1, "/a/s01e01.mkv"),
                sample_series("local:b", "Show B", 1, 2, "/b/s01e02.mkv"),
            ],
            movies: vec![],
        };
        apply_overrides(
            &mut snap,
            &[crate::db::CatalogOverride {
                id: 1,
                kind: "rename".into(),
                source_key: "local:a".into(),
                target_key: None,
                display_title: Some("Renamed".into()),
            }],
        );
        assert_eq!(snap.series[0].title, "Renamed");
        assert_eq!(snap.series[0].display_title.as_deref(), Some("Renamed"));

        apply_overrides(
            &mut snap,
            &[crate::db::CatalogOverride {
                id: 2,
                kind: "merge".into(),
                source_key: "local:b".into(),
                target_key: Some("local:a".into()),
                display_title: None,
            }],
        );
        assert_eq!(snap.series.len(), 1);
        assert_eq!(snap.series[0].id, "local:a");
        let eps: Vec<_> = snap.series[0].seasons[0]
            .episodes
            .iter()
            .map(|e| e.episode)
            .collect();
        assert_eq!(eps, vec![1, 2]);
    }

    #[test]
    fn local_ids_and_mappings() {
        assert_eq!(local_series_id("breaking bad"), "local:breaking-bad");
        assert_eq!(local_series_id(""), "local:unknown");
        assert_eq!(episode_id("local:x", 2, 3), "local:x-s02e03");

        let snap = CatalogSnapshot {
            scanned_at: "now".into(),
            series: vec![sample_series("local:a", "A", 1, 1, "/ep.mkv")],
            movies: vec![CatalogMovie {
                id: "local:m".into(),
                title: "M".into(),
                path: "/m.mkv".into(),
                poster_url: None,
                backdrop_url: None,
                synopsis: None,
                tmdb_id: None,
                original_language: None,
                year: Some(1999),
                display_title: None,
            }],
        };
        let maps = mappings_from_snapshot(&snap);
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].kind, "episode");
        assert_eq!(maps[1].kind, "movie");
    }
}
