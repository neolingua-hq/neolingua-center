use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const TMDB_IMG: &str = "https://image.tmdb.org/t/p";
const CACHE_TTL_SECS: i64 = 60 * 60 * 24 * 14;

#[derive(Debug, Deserialize)]
struct SearchTv {
    results: Option<Vec<TvResult>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TvResult {
    id: i32,
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    original_language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchMovie {
    results: Option<Vec<MovieResult>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MovieResult {
    id: i32,
    title: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    release_date: Option<String>,
    original_language: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SeasonPayload {
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    episodes: Option<Vec<EpisodePayload>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct EpisodePayload {
    episode_number: Option<i32>,
    name: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
}

fn poster_url(path: Option<&str>, size: &str) -> Option<String> {
    let path = path.filter(|p| !p.is_empty())?;
    Some(format!("{TMDB_IMG}/{size}{path}"))
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

fn tmdb_get<T: for<'de> Deserialize<'de>>(
    client: &reqwest::blocking::Client,
    api_key: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> Result<T, String> {
    let mut url = reqwest::Url::parse(&format!("{TMDB_BASE}{path}")).map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("api_key", api_key);
        q.append_pair("language", "fr-FR");
        for (k, v) in extra {
            q.append_pair(k, v);
        }
    }
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("TMDB {}", response.status()));
    }
    response.json::<T>().map_err(|e| e.to_string())
}

fn cache_get(conn: &Connection, key: &str) -> Result<Option<CacheHit>, String> {
    let row: Option<String> = conn
        .query_row(
            "SELECT payload FROM tmdb_cache WHERE cache_key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(payload) = row else {
        return Ok(None);
    };
    if payload == CACHE_MISS {
        return Ok(Some(CacheHit::Miss));
    }
    Ok(Some(CacheHit::Value(payload)))
}

const CACHE_MISS: &str = "__miss__";

enum CacheHit {
    Miss,
    Value(String),
}

fn cache_set(conn: &Connection, key: &str, payload: &str) -> Result<(), String> {
    conn.execute(
        r#"
        INSERT INTO tmdb_cache (cache_key, payload, fetched_at)
        VALUES (?1, ?2, datetime('now'))
        ON CONFLICT(cache_key) DO UPDATE SET payload = excluded.payload, fetched_at = excluded.fetched_at
        "#,
        params![key, payload],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn cache_set_miss(conn: &Connection, key: &str) -> Result<(), String> {
    cache_set(conn, key, CACHE_MISS)
}

fn search_tv(
    client: &reqwest::blocking::Client,
    conn: &Connection,
    api_key: &str,
    title: &str,
) -> Result<Option<TvResult>, String> {
    let key = format!("tv-search:{}", title.to_lowercase());
    match cache_get(conn, &key)? {
        Some(CacheHit::Miss) => return Ok(None),
        Some(CacheHit::Value(raw)) => {
            if let Ok(parsed) = serde_json::from_str::<TvResult>(&raw) {
                return Ok(Some(parsed));
            }
        }
        None => {}
    }
    let search: SearchTv = tmdb_get(client, api_key, "/search/tv", &[("query", title)])?;
    let show = search.results.unwrap_or_default().into_iter().next();
    if let Some(ref show) = show {
        if let Ok(json) = serde_json::to_string(show) {
            let _ = cache_set(conn, &key, &json);
        }
    } else {
        let _ = cache_set_miss(conn, &key);
    }
    Ok(show)
}

fn search_movie(
    client: &reqwest::blocking::Client,
    conn: &Connection,
    api_key: &str,
    title: &str,
) -> Result<Option<MovieResult>, String> {
    let key = format!("movie-search:{}", title.to_lowercase());
    match cache_get(conn, &key)? {
        Some(CacheHit::Miss) => return Ok(None),
        Some(CacheHit::Value(raw)) => {
            if let Ok(parsed) = serde_json::from_str::<MovieResult>(&raw) {
                return Ok(Some(parsed));
            }
        }
        None => {}
    }
    let search: SearchMovie = tmdb_get(client, api_key, "/search/movie", &[("query", title)])?;
    let movie = search.results.unwrap_or_default().into_iter().next();
    if let Some(ref movie) = movie {
        if let Ok(json) = serde_json::to_string(movie) {
            let _ = cache_set(conn, &key, &json);
        }
    } else {
        let _ = cache_set_miss(conn, &key);
    }
    Ok(movie)
}

fn load_season(
    client: &reqwest::blocking::Client,
    conn: &Connection,
    api_key: &str,
    tmdb_id: i32,
    season: i32,
) -> Result<Option<SeasonPayload>, String> {
    let key = format!("tv-season:{tmdb_id}:{season}");
    match cache_get(conn, &key)? {
        Some(CacheHit::Miss) => return Ok(None),
        Some(CacheHit::Value(raw)) => {
            if let Ok(parsed) = serde_json::from_str::<SeasonPayload>(&raw) {
                return Ok(Some(parsed));
            }
        }
        None => {}
    }
    let path = format!("/tv/{tmdb_id}/season/{season}");
    match tmdb_get::<SeasonPayload>(client, api_key, &path, &[]) {
        Ok(payload) => {
            if let Ok(json) = serde_json::to_string(&payload) {
                let _ = cache_set(conn, &key, &json);
            }
            Ok(Some(payload))
        }
        Err(_) => {
            let _ = cache_set_miss(conn, &key);
            Ok(None)
        }
    }
}

/// Enrich an already-canonicalized catalog (TMDB seasons / episodes).
/// Skips the API for episodes already marked `tmdb_checked`, and for
/// search / season cache hits (success or absence).
pub fn enrich_catalog(conn: &Connection, api_key: &str) -> Result<(), String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Ok(());
    }
    let client = client()?;

    let mut series_stmt = conn
        .prepare("SELECT id, title, tmdb_id, original_language FROM catalog_series")
        .map_err(|e| e.to_string())?;
    let series: Vec<(String, String, Option<i32>, Option<String>)> = series_stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(series_stmt);

    for (series_id, title, existing_tmdb, existing_lang) in series {
        let tmdb_id = if let Some(id) = existing_tmdb {
            if existing_lang
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            {
                if let Ok(Some(lang)) = fetch_tv_original_language(&client, conn, key, id) {
                    let _ = conn.execute(
                        "UPDATE catalog_series SET original_language = ?1 WHERE id = ?2",
                        params![lang, series_id],
                    );
                }
            }
            id
        } else {
            let Ok(found) = search_tv(&client, conn, key, &title) else {
                continue;
            };
            let Some(show) = found else {
                conn.execute(
                    "UPDATE catalog_episodes SET tmdb_checked = 1 WHERE series_id = ?1",
                    params![series_id],
                )
                .map_err(|e| e.to_string())?;
                continue;
            };
            conn.execute(
                r#"
                UPDATE catalog_series SET
                  display_title = ?1,
                  synopsis = COALESCE(?2, synopsis),
                  poster_url = COALESCE(?3, poster_url),
                  backdrop_url = COALESCE(?4, backdrop_url),
                  tmdb_id = ?5,
                  original_language = COALESCE(?6, original_language)
                WHERE id = ?7
                "#,
                params![
                    show.name.as_deref().unwrap_or(&title),
                    show.overview,
                    poster_url(show.poster_path.as_deref(), "w500"),
                    poster_url(show.backdrop_path.as_deref(), "w780"),
                    show.id,
                    show.original_language,
                    series_id
                ],
            )
            .map_err(|e| e.to_string())?;
            show.id
        };

        let seasons: Vec<i32> = conn
            .prepare(
                r#"
                SELECT DISTINCT season FROM catalog_episodes
                WHERE series_id = ?1 AND tmdb_checked = 0
                "#,
            )
            .map_err(|e| e.to_string())?
            .query_map(params![series_id], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        for season in seasons {
            let Some(payload) = load_season(&client, conn, key, tmdb_id, season)? else {
                conn.execute(
                    "UPDATE catalog_episodes SET tmdb_checked = 1 WHERE series_id = ?1 AND season = ?2",
                    params![series_id, season],
                )
                .map_err(|e| e.to_string())?;
                continue;
            };
            conn.execute(
                r#"
                INSERT INTO catalog_seasons (series_id, number, title, poster_url, synopsis)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(series_id, number) DO UPDATE SET
                  title = excluded.title,
                  poster_url = excluded.poster_url,
                  synopsis = excluded.synopsis
                "#,
                params![
                    series_id,
                    season,
                    payload.name,
                    poster_url(payload.poster_path.as_deref(), "w500"),
                    payload.overview
                ],
            )
            .map_err(|e| e.to_string())?;

            if let Some(episodes) = payload.episodes {
                for ep in episodes {
                    let Some(num) = ep.episode_number else {
                        continue;
                    };
                    conn.execute(
                        r#"
                        UPDATE catalog_episodes
                        SET title = COALESCE(?1, title),
                            poster_url = COALESCE(?2, poster_url),
                            synopsis = COALESCE(?3, synopsis),
                            tmdb_checked = 1
                        WHERE series_id = ?4 AND season = ?5 AND episode = ?6
                        "#,
                        params![
                            ep.name,
                            poster_url(ep.still_path.as_deref(), "w300"),
                            ep.overview,
                            series_id,
                            season,
                            num
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            conn.execute(
                "UPDATE catalog_episodes SET tmdb_checked = 1 WHERE series_id = ?1 AND season = ?2",
                params![series_id, season],
            )
            .map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    let mut movie_stmt = conn
        .prepare("SELECT id, title, tmdb_id, original_language FROM catalog_movies")
        .map_err(|e| e.to_string())?;
    let movies: Vec<(String, String, Option<i32>, Option<String>)> = movie_stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(movie_stmt);

    for (movie_id, title, existing_tmdb, existing_lang) in movies {
        if let Some(id) = existing_tmdb {
            if existing_lang
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            {
                if let Ok(Some(lang)) = fetch_movie_original_language(&client, conn, key, id) {
                    let _ = conn.execute(
                        "UPDATE catalog_movies SET original_language = ?1 WHERE id = ?2",
                        params![lang, movie_id],
                    );
                }
            }
            continue;
        }
        let Ok(found) = search_movie(&client, conn, key, &title) else {
            continue;
        };
        let Some(movie) = found else {
            continue;
        };
        let year = movie
            .release_date
            .as_deref()
            .and_then(|d| d.get(0..4))
            .and_then(|y| y.parse::<i32>().ok());
        conn.execute(
            r#"
            UPDATE catalog_movies SET
              display_title = ?1,
              synopsis = ?2,
              poster_url = ?3,
              backdrop_url = ?4,
              tmdb_id = ?5,
              year = ?6,
              original_language = COALESCE(?7, original_language)
            WHERE id = ?8
            "#,
            params![
                movie.title.as_deref().unwrap_or(&title),
                movie.overview,
                poster_url(movie.poster_path.as_deref(), "w500"),
                poster_url(movie.backdrop_path.as_deref(), "w780"),
                movie.id,
                year,
                movie.original_language,
                movie_id
            ],
        )
        .map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = CACHE_TTL_SECS;
    Ok(())
}

/// Search TMDB and merge local series that share the same `tmdb_id`.
/// Without a key: snapshot unchanged (`local:…` ids).
pub fn canonicalize_snapshot(
    conn: &Connection,
    api_key: &str,
    mut snapshot: crate::scan::CatalogSnapshot,
) -> Result<crate::scan::CatalogSnapshot, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Ok(snapshot);
    }
    let client = client()?;

    use crate::scan::CatalogSeries;
    use std::collections::BTreeMap;

    let mut by_tmdb: BTreeMap<i32, CatalogSeries> = BTreeMap::new();
    let mut unmatched: Vec<CatalogSeries> = Vec::new();

    for mut series in snapshot.series.drain(..) {
        if series.tmdb_id.is_some() {
            if let Some(id) = series.tmdb_id {
                if let Some(existing) = by_tmdb.get_mut(&id) {
                    merge_series(existing, series);
                } else {
                    by_tmdb.insert(id, series);
                }
            }
            continue;
        }
        let query = series
            .display_title
            .clone()
            .unwrap_or_else(|| series.title.clone());
        let Ok(found) = search_tv(&client, conn, key, &query) else {
            unmatched.push(series);
            continue;
        };
        let Some(show) = found else {
            unmatched.push(series);
            continue;
        };
        let new_id = format!("tmdb:{}", show.id);
        series.tmdb_id = Some(show.id);
        series.display_title = show.name.clone().or(Some(series.title.clone()));
        if let Some(name) = &show.name {
            series.title = name.clone();
        }
        series.synopsis = show.overview.clone().or(series.synopsis);
        series.poster_url = poster_url(show.poster_path.as_deref(), "w500").or(series.poster_url);
        series.backdrop_url =
            poster_url(show.backdrop_path.as_deref(), "w780").or(series.backdrop_url);
        if series.original_language.is_none() {
            series.original_language = show.original_language.clone();
        }

        // Re-id episodes
        for season in &mut series.seasons {
            for ep in &mut season.episodes {
                ep.id = format!("{}-s{:02}e{:02}", new_id, ep.season, ep.episode);
            }
        }
        series.id = new_id;

        if let Some(existing) = by_tmdb.get_mut(&show.id) {
            merge_series(existing, series);
        } else {
            by_tmdb.insert(show.id, series);
        }
        std::thread::sleep(Duration::from_millis(40));
    }

    snapshot.series = by_tmdb.into_values().chain(unmatched).collect();
    snapshot.series.sort_by_key(|a| a.title.to_lowercase());

    // Movies
    let mut movies = Vec::new();
    let mut movie_by_tmdb: BTreeMap<i32, usize> = BTreeMap::new();
    for mut movie in snapshot.movies.drain(..) {
        if movie.tmdb_id.is_some() {
            if let Some(id) = movie.tmdb_id {
                if let std::collections::btree_map::Entry::Vacant(slot) = movie_by_tmdb.entry(id) {
                    slot.insert(movies.len());
                    movies.push(movie);
                }
            } else {
                movies.push(movie);
            }
            continue;
        }
        let query = movie
            .display_title
            .clone()
            .unwrap_or_else(|| movie.title.clone());
        if let Ok(Some(hit)) = search_movie(&client, conn, key, &query) {
            movie.tmdb_id = Some(hit.id);
            movie.display_title = hit.title.clone().or(Some(movie.title.clone()));
            if let Some(t) = &hit.title {
                movie.title = t.clone();
            }
            movie.synopsis = hit.overview.clone().or(movie.synopsis);
            movie.poster_url = poster_url(hit.poster_path.as_deref(), "w500").or(movie.poster_url);
            movie.backdrop_url =
                poster_url(hit.backdrop_path.as_deref(), "w780").or(movie.backdrop_url);
            if movie.original_language.is_none() {
                movie.original_language = hit.original_language.clone();
            }
            movie.year = hit
                .release_date
                .as_deref()
                .and_then(|d| d.get(0..4))
                .and_then(|y| y.parse().ok())
                .or(movie.year);
            movie.id = format!("tmdb:{}", hit.id);
            if let Some(idx) = movie_by_tmdb.get(&hit.id) {
                // duplicate: keep the first path
                let _ = idx;
            } else {
                movie_by_tmdb.insert(hit.id, movies.len());
                movies.push(movie);
            }
            std::thread::sleep(Duration::from_millis(40));
        } else {
            movies.push(movie);
        }
    }
    snapshot.movies = movies;
    Ok(snapshot)
}

fn fetch_tv_original_language(
    client: &reqwest::blocking::Client,
    conn: &Connection,
    api_key: &str,
    tmdb_id: i32,
) -> Result<Option<String>, String> {
    let key = format!("tv-detail:{tmdb_id}");
    match cache_get(conn, &key)? {
        Some(CacheHit::Miss) => return Ok(None),
        Some(CacheHit::Value(raw)) => {
            if let Ok(parsed) = serde_json::from_str::<TvDetail>(&raw) {
                return Ok(parsed.original_language);
            }
        }
        None => {}
    }
    let detail: TvDetail = tmdb_get(client, api_key, &format!("/tv/{tmdb_id}"), &[])?;
    if let Ok(json) = serde_json::to_string(&detail) {
        let _ = cache_set(conn, &key, &json);
    }
    Ok(detail.original_language)
}

fn fetch_movie_original_language(
    client: &reqwest::blocking::Client,
    conn: &Connection,
    api_key: &str,
    tmdb_id: i32,
) -> Result<Option<String>, String> {
    let key = format!("movie-detail:{tmdb_id}");
    match cache_get(conn, &key)? {
        Some(CacheHit::Miss) => return Ok(None),
        Some(CacheHit::Value(raw)) => {
            if let Ok(parsed) = serde_json::from_str::<MovieDetail>(&raw) {
                return Ok(parsed.original_language);
            }
        }
        None => {}
    }
    let detail: MovieDetail = tmdb_get(client, api_key, &format!("/movie/{tmdb_id}"), &[])?;
    if let Ok(json) = serde_json::to_string(&detail) {
        let _ = cache_set(conn, &key, &json);
    }
    Ok(detail.original_language)
}

#[derive(Debug, Deserialize, Serialize)]
struct TvDetail {
    original_language: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MovieDetail {
    original_language: Option<String>,
}

fn merge_series(dst: &mut crate::scan::CatalogSeries, mut src: crate::scan::CatalogSeries) {
    if dst.root.is_empty() {
        dst.root = src.root;
    }
    if dst.synopsis.is_none() {
        dst.synopsis = src.synopsis.take();
    }
    if dst.poster_url.is_none() {
        dst.poster_url = src.poster_url.take();
    }
    if dst.backdrop_url.is_none() {
        dst.backdrop_url = src.backdrop_url.take();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_connection, run_migrations};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn poster_url_skips_empty() {
        assert!(poster_url(None, "w342").is_none());
        assert!(poster_url(Some(""), "w342").is_none());
        assert_eq!(
            poster_url(Some("/abc.jpg"), "w342").as_deref(),
            Some("https://image.tmdb.org/t/p/w342/abc.jpg")
        );
    }

    #[test]
    fn canonicalize_empty_key_is_noop() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("neolingua-tmdb-{nanos}.sqlite"));
        let conn = open_connection(&path).unwrap();
        run_migrations(&conn).unwrap();
        let snap = crate::scan::CatalogSnapshot {
            scanned_at: "t".into(),
            series: vec![],
            movies: vec![],
        };
        let out = canonicalize_snapshot(&conn, "  ", snap).unwrap();
        assert!(out.series.is_empty());
        drop(conn);
        let _ = std::fs::remove_file(path);
    }
}
