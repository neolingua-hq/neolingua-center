//! Library sync: disk scan, preserve TMDB metadata, targeted enrichment.

use crate::db;
use crate::scan::{self, CatalogSeries, CatalogSnapshot};
use crate::tmdb;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
struct PreservedEpisode {
    series_id: Option<String>,
    title: Option<String>,
    poster_url: Option<String>,
    synopsis: Option<String>,
    tmdb_checked: bool,
    confidence: f32,
}

#[derive(Debug, Clone, Default)]
struct PreservedSeries {
    id: String,
    title: String,
    display_title: Option<String>,
    synopsis: Option<String>,
    poster_url: Option<String>,
    backdrop_url: Option<String>,
    tmdb_id: Option<i32>,
}

#[derive(Debug, Clone, Default)]
struct PreservedMovie {
    id: String,
    title: String,
    display_title: Option<String>,
    synopsis: Option<String>,
    poster_url: Option<String>,
    backdrop_url: Option<String>,
    tmdb_id: Option<i32>,
    year: Option<i32>,
}

#[derive(Debug, Clone, Default)]
struct PreservedSeason {
    title: Option<String>,
    poster_url: Option<String>,
    synopsis: Option<String>,
}

struct Preserved {
    episodes: HashMap<String, PreservedEpisode>,
    series_by_id: HashMap<String, PreservedSeries>,
    seasons: HashMap<(String, i32), PreservedSeason>,
    movies: HashMap<String, PreservedMovie>,
    paths: HashSet<String>,
}

fn load_preserved(conn: &Connection) -> Result<Preserved, String> {
    let mut episodes = HashMap::new();
    let mut paths = HashSet::new();

    let mut ep_stmt = conn
        .prepare(
            r#"
            SELECT path, series_id, title, poster_url, synopsis, tmdb_checked, confidence
            FROM catalog_episodes
            "#,
        )
        .map_err(|e| e.to_string())?;
    let ep_rows = ep_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i32>(5).unwrap_or(0) != 0,
                row.get::<_, f64>(6).unwrap_or(1.0) as f32,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in ep_rows {
        let (path, series_id, title, poster_url, synopsis, tmdb_checked, confidence) =
            row.map_err(|e| e.to_string())?;
        paths.insert(path.clone());
        episodes.insert(
            path,
            PreservedEpisode {
                series_id: Some(series_id),
                title,
                poster_url,
                synopsis,
                tmdb_checked,
                confidence,
            },
        );
    }

    // Movie paths + map rows (in case catalog and map diverge)
    let mut map_stmt = conn
        .prepare("SELECT path FROM media_path_map")
        .map_err(|e| e.to_string())?;
    let map_rows = map_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for row in map_rows {
        paths.insert(row.map_err(|e| e.to_string())?);
    }

    let mut series_by_id = HashMap::new();
    let mut series_stmt = conn
        .prepare(
            r#"
            SELECT id, title, display_title, synopsis, poster_url, backdrop_url, tmdb_id
            FROM catalog_series
            "#,
        )
        .map_err(|e| e.to_string())?;
    let series_rows = series_stmt
        .query_map([], |row| {
            Ok(PreservedSeries {
                id: row.get(0)?,
                title: row.get(1)?,
                display_title: row.get(2)?,
                synopsis: row.get(3)?,
                poster_url: row.get(4)?,
                backdrop_url: row.get(5)?,
                tmdb_id: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    for row in series_rows {
        let s = row.map_err(|e| e.to_string())?;
        series_by_id.insert(s.id.clone(), s);
    }

    let mut seasons = HashMap::new();
    let mut season_stmt = conn
        .prepare(
            "SELECT series_id, number, title, poster_url, synopsis FROM catalog_seasons",
        )
        .map_err(|e| e.to_string())?;
    let season_rows = season_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                PreservedSeason {
                    title: row.get(2)?,
                    poster_url: row.get(3)?,
                    synopsis: row.get(4)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in season_rows {
        let (series_id, number, season) = row.map_err(|e| e.to_string())?;
        seasons.insert((series_id, number), season);
    }

    let mut movies = HashMap::new();
    let mut movie_stmt = conn
        .prepare(
            r#"
            SELECT path, id, title, display_title, synopsis, poster_url, backdrop_url, tmdb_id, year
            FROM catalog_movies
            "#,
        )
        .map_err(|e| e.to_string())?;
    let movie_rows = movie_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                PreservedMovie {
                    id: row.get(1)?,
                    title: row.get(2)?,
                    display_title: row.get(3)?,
                    synopsis: row.get(4)?,
                    poster_url: row.get(5)?,
                    backdrop_url: row.get(6)?,
                    tmdb_id: row.get(7)?,
                    year: row.get(8)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in movie_rows {
        let (path, movie) = row.map_err(|e| e.to_string())?;
        paths.insert(path.clone());
        movies.insert(path, movie);
    }

    Ok(Preserved {
        episodes,
        series_by_id,
        seasons,
        movies,
        paths,
    })
}

fn snapshot_paths(snapshot: &CatalogSnapshot) -> HashSet<String> {
    let mut paths = HashSet::new();
    for series in &snapshot.series {
        for season in &series.seasons {
            for ep in &season.episodes {
                paths.insert(ep.path.clone());
            }
        }
    }
    for movie in &snapshot.movies {
        paths.insert(movie.path.clone());
    }
    paths
}

/// Re-apply known ids / metadata so we do not re-query TMDB unnecessarily.
fn apply_preserved(snapshot: &mut CatalogSnapshot, preserved: &Preserved) {
    // Series: if an episode already points at a known series_id, reuse that id + meta.
    let mut remapped: Vec<CatalogSeries> = Vec::new();
    let mut by_final_id: HashMap<String, usize> = HashMap::new();

    for mut series in snapshot.series.drain(..) {
        let mut target_id: Option<String> = None;
        for season in &series.seasons {
            for ep in &season.episodes {
                if let Some(prev) = preserved.episodes.get(&ep.path) {
                    if let Some(sid) = &prev.series_id {
                        target_id = Some(sid.clone());
                        break;
                    }
                }
            }
            if target_id.is_some() {
                break;
            }
        }

        if let Some(id) = target_id {
            if let Some(meta) = preserved.series_by_id.get(&id) {
                series.id = id.clone();
                series.title = meta.title.clone();
                series.display_title = meta.display_title.clone();
                series.synopsis = meta.synopsis.clone();
                series.poster_url = meta.poster_url.clone();
                series.backdrop_url = meta.backdrop_url.clone();
                series.tmdb_id = meta.tmdb_id;
            } else {
                series.id = id;
            }
            for season in &mut series.seasons {
                if let Some(prev_season) = preserved.seasons.get(&(series.id.clone(), season.number))
                {
                    if season.title.is_none() {
                        season.title = prev_season.title.clone();
                    }
                    if season.poster_url.is_none() {
                        season.poster_url = prev_season.poster_url.clone();
                    }
                    if season.synopsis.is_none() {
                        season.synopsis = prev_season.synopsis.clone();
                    }
                }
                for ep in &mut season.episodes {
                    ep.id = format!("{}-s{:02}e{:02}", series.id, ep.season, ep.episode);
                    if let Some(prev) = preserved.episodes.get(&ep.path) {
                        ep.title = prev.title.clone().or(ep.title.take());
                        ep.poster_url = prev.poster_url.clone().or(ep.poster_url.take());
                        ep.synopsis = prev.synopsis.clone().or(ep.synopsis.take());
                        ep.tmdb_checked = prev.tmdb_checked;
                        if prev.confidence > 0.0 {
                            ep.confidence = prev.confidence;
                        }
                    }
                }
            }

            if let Some(idx) = by_final_id.get(&series.id).copied() {
                merge_series_into(&mut remapped[idx], series);
            } else {
                by_final_id.insert(series.id.clone(), remapped.len());
                remapped.push(series);
            }
        } else {
            for season in &mut series.seasons {
                for ep in &mut season.episodes {
                    if let Some(prev) = preserved.episodes.get(&ep.path) {
                        ep.title = prev.title.clone().or(ep.title.take());
                        ep.poster_url = prev.poster_url.clone().or(ep.poster_url.take());
                        ep.synopsis = prev.synopsis.clone().or(ep.synopsis.take());
                        ep.tmdb_checked = prev.tmdb_checked;
                    }
                }
            }
            remapped.push(series);
        }
    }
    snapshot.series = remapped;

    for movie in &mut snapshot.movies {
        if let Some(prev) = preserved.movies.get(&movie.path) {
            movie.id = prev.id.clone();
            movie.title = prev.title.clone();
            movie.display_title = prev.display_title.clone();
            movie.synopsis = prev.synopsis.clone();
            movie.poster_url = prev.poster_url.clone();
            movie.backdrop_url = prev.backdrop_url.clone();
            movie.tmdb_id = prev.tmdb_id;
            movie.year = prev.year.or(movie.year);
        }
    }
}

fn merge_series_into(dst: &mut CatalogSeries, mut src: CatalogSeries) {
    if dst.root.is_empty() {
        dst.root = std::mem::take(&mut src.root);
    }
    if dst.original_language.is_none() {
        dst.original_language = src.original_language.take();
    }
    for season in src.seasons.drain(..) {
        if let Some(existing) = dst.seasons.iter_mut().find(|s| s.number == season.number) {
            for ep in season.episodes {
                if !existing.episodes.iter().any(|e| e.episode == ep.episode) {
                    existing.episodes.push(ep);
                }
            }
            existing.episodes.sort_by_key(|e| e.episode);
            if existing.title.is_none() {
                existing.title = season.title;
            }
            if existing.poster_url.is_none() {
                existing.poster_url = season.poster_url;
            }
            if existing.synopsis.is_none() {
                existing.synopsis = season.synopsis;
            }
        } else {
            dst.seasons.push(season);
        }
    }
    dst.seasons.sort_by_key(|s| s.number);
}

/// Sync from disk. `force` skips the path-set equality short-circuit.
/// Returns `(catalog, changed)`.
pub fn sync_catalog(conn: &Connection, force: bool) -> Result<(CatalogSnapshot, bool), String> {
    let roots = db::list_media_roots(conn)?;
    if roots.is_empty() {
        return Err("Ajoutez d'abord un dossier de vidéos.".into());
    }
    let paths: Vec<String> = roots.into_iter().map(|r| r.path).collect();
    let preserved = load_preserved(conn)?;
    let (mut snapshot, _) = scan::scan_roots(&paths)?;
    let new_paths = snapshot_paths(&snapshot);

    if !force && !preserved.paths.is_empty() && new_paths == preserved.paths {
        return Ok((db::load_catalog(conn)?, false));
    }

    let overrides = db::list_catalog_overrides(conn)?;
    scan::apply_overrides(&mut snapshot, &overrides);
    apply_preserved(&mut snapshot, &preserved);

    let key = db::load_settings(conn)?.tmdb_api_key;
    // Canonicalize only series still missing tmdb_id (known ones are preserved).
    snapshot = tmdb::canonicalize_snapshot(conn, &key, snapshot)?;
    // Re-apply episode meta after a possible tmdb:… id change.
    apply_preserved(&mut snapshot, &preserved);

    let mappings = scan::mappings_from_snapshot(&snapshot);
    db::replace_catalog(conn, &snapshot)?;
    db::replace_media_path_map(conn, &mappings)?;
    tmdb::enrich_catalog(conn, &key)?;
    Ok((db::load_catalog(conn)?, true))
}

pub fn sync_catalog_at(db_path: &PathBuf, force: bool) -> Result<(CatalogSnapshot, bool), String> {
    let conn = db::open_connection(db_path)?;
    db::run_migrations(&conn)?;
    sync_catalog(&conn, force)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::EpisodePrep;
    use crate::scan::{CatalogEpisode, CatalogSeason, CatalogSeries, CatalogSnapshot};

    fn ep(season: i32, episode: i32, path: &str) -> CatalogEpisode {
        CatalogEpisode {
            id: format!("id-s{season:02}e{episode:02}"),
            season,
            episode,
            label: format!("S{season:02}E{episode:02}"),
            path: path.into(),
            title: None,
            poster_url: None,
            synopsis: None,
            confidence: 0.9,
            tmdb_checked: false,
            prep: EpisodePrep::default(),
        }
    }

    fn series(id: &str, seasons: Vec<CatalogSeason>) -> CatalogSeries {
        CatalogSeries {
            id: id.into(),
            title: id.into(),
            root: "/r".into(),
            poster_url: None,
            backdrop_url: None,
            synopsis: None,
            tmdb_id: None,
            original_language: Some("en".into()),
            display_title: None,
            seasons,
        }
    }

    #[test]
    fn snapshot_paths_collects_episodes_and_movies() {
        let snap = CatalogSnapshot {
            scanned_at: "t".into(),
            series: vec![series(
                "s",
                vec![CatalogSeason {
                    number: 1,
                    title: None,
                    poster_url: None,
                    synopsis: None,
                    episodes: vec![ep(1, 1, "/a.mkv"), ep(1, 2, "/b.mkv")],
                }],
            )],
            movies: vec![crate::scan::CatalogMovie {
                id: "m".into(),
                title: "M".into(),
                path: "/m.mkv".into(),
                poster_url: None,
                backdrop_url: None,
                synopsis: None,
                tmdb_id: None,
                original_language: None,
                year: None,
                display_title: None,
            }],
        };
        let paths = snapshot_paths(&snap);
        assert_eq!(paths.len(), 3);
        assert!(paths.contains("/a.mkv"));
        assert!(paths.contains("/m.mkv"));
    }

    #[test]
    fn merge_series_into_dedups_episodes() {
        let mut dst = series(
            "dst",
            vec![CatalogSeason {
                number: 1,
                title: Some("S1".into()),
                poster_url: None,
                synopsis: None,
                episodes: vec![ep(1, 1, "/a.mkv")],
            }],
        );
        let src = series(
            "src",
            vec![CatalogSeason {
                number: 1,
                title: None,
                poster_url: None,
                synopsis: None,
                episodes: vec![ep(1, 1, "/dup.mkv"), ep(1, 2, "/b.mkv")],
            }],
        );
        merge_series_into(&mut dst, src);
        assert_eq!(dst.seasons[0].episodes.len(), 2);
        assert_eq!(dst.seasons[0].episodes[0].path, "/a.mkv");
        assert_eq!(dst.seasons[0].episodes[1].episode, 2);
        assert_eq!(dst.seasons[0].title.as_deref(), Some("S1"));
    }
}

