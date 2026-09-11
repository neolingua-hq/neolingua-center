use crate::scan::{CatalogMovie, CatalogSeason, CatalogSeries, CatalogSnapshot};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Target schema version after migrations have run (single baseline).
pub const SCHEMA_VERSION: i32 = 1;

/// Allowed Jam quiz intervals (seconds).
pub const QUIZ_INTERVAL_SECONDS: &[u32] = &[30, 60, 120, 300, 900];

type SeasonMetaRow = (i32, Option<String>, Option<String>, Option<String>);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub wizard_step: String,
    pub skip_intro: bool,
    pub server_port: u32,
    pub setup_complete: bool,
    pub tmdb_api_key: String,
    /// 0 = disabled. Otherwise minutes between background folder checks.
    pub library_check_minutes: u32,
    /// 0 = unlimited. Otherwise remux/subtitle cache ceiling in GiB.
    pub cache_max_gb: u32,
    /// Delete a media item's cache after it has been watched to the end.
    pub purge_cache_after_watch: bool,
    /// Default: start Jam sessions with CEFR quiz mode on.
    pub jam_quiz_mode: bool,
    /// Default seconds between Jam quiz rounds (host default, overridable per Jam).
    pub jam_quiz_interval_seconds: u32,
    /// Default: show English subtitles on the Jam video display.
    pub jam_display_sub_en: bool,
    /// Default: show French subtitles on the Jam video display.
    pub jam_display_sub_fr: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRoot {
    pub id: i64,
    pub path: String,
    pub sort_order: i32,
    /// Path exists and is readable (mounted volume, permissions, etc.).
    pub accessible: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            wizard_step: "welcome".into(),
            skip_intro: false,
            server_port: 8787,
            setup_complete: false,
            tmdb_api_key: String::new(),
            library_check_minutes: 60,
            cache_max_gb: 20,
            purge_cache_after_watch: false,
            jam_quiz_mode: false,
            jam_quiz_interval_seconds: 60,
            jam_display_sub_en: false,
            jam_display_sub_fr: false,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogOverride {
    pub id: i64,
    pub kind: String,
    pub source_key: String,
    pub target_key: Option<String>,
    pub display_title: Option<String>,
}

pub struct DbState {
    pub path: PathBuf,
}

pub fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("neolingua.sqlite"))
}

pub fn open_connection(path: &PathBuf) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn schema_version(conn: &Connection) -> Result<i32, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

fn migrate_baseline(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS app_settings (
          id INTEGER PRIMARY KEY CHECK (id = 1),
          wizard_step TEXT NOT NULL DEFAULT 'welcome',
          skip_intro INTEGER NOT NULL DEFAULT 0,
          server_port INTEGER NOT NULL DEFAULT 8787,
          setup_complete INTEGER NOT NULL DEFAULT 0,
          tmdb_api_key TEXT NOT NULL DEFAULT '',
          library_check_minutes INTEGER NOT NULL DEFAULT 60,
          cache_max_gb INTEGER NOT NULL DEFAULT 20,
          purge_cache_after_watch INTEGER NOT NULL DEFAULT 0,
          jam_quiz_mode INTEGER NOT NULL DEFAULT 0,
          jam_quiz_interval_seconds INTEGER NOT NULL DEFAULT 60,
          jam_display_sub_en INTEGER NOT NULL DEFAULT 0,
          jam_display_sub_fr INTEGER NOT NULL DEFAULT 0,
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO app_settings (id, wizard_step, updated_at)
          VALUES (1, 'welcome', datetime('now'));

        CREATE TABLE IF NOT EXISTS media_roots (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          path TEXT NOT NULL UNIQUE,
          sort_order INTEGER NOT NULL DEFAULT 0,
          added_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS catalog_meta (
          id INTEGER PRIMARY KEY CHECK (id = 1),
          scanned_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS catalog_series (
          id TEXT PRIMARY KEY,
          title TEXT NOT NULL,
          root TEXT NOT NULL,
          display_title TEXT,
          synopsis TEXT,
          poster_url TEXT,
          backdrop_url TEXT,
          tmdb_id INTEGER,
          original_language TEXT
        );
        CREATE TABLE IF NOT EXISTS catalog_seasons (
          series_id TEXT NOT NULL,
          number INTEGER NOT NULL,
          title TEXT,
          poster_url TEXT,
          synopsis TEXT,
          PRIMARY KEY (series_id, number),
          FOREIGN KEY (series_id) REFERENCES catalog_series(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS catalog_episodes (
          id TEXT PRIMARY KEY,
          series_id TEXT NOT NULL,
          season INTEGER NOT NULL,
          episode INTEGER NOT NULL,
          label TEXT NOT NULL,
          path TEXT NOT NULL,
          title TEXT,
          poster_url TEXT,
          synopsis TEXT,
          confidence REAL NOT NULL DEFAULT 1.0,
          tmdb_checked INTEGER NOT NULL DEFAULT 0,
          FOREIGN KEY (series_id) REFERENCES catalog_series(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS catalog_movies (
          id TEXT PRIMARY KEY,
          title TEXT NOT NULL,
          path TEXT NOT NULL,
          display_title TEXT,
          synopsis TEXT,
          poster_url TEXT,
          backdrop_url TEXT,
          tmdb_id INTEGER,
          year INTEGER,
          original_language TEXT
        );
        CREATE TABLE IF NOT EXISTS tmdb_cache (
          cache_key TEXT PRIMARY KEY,
          payload TEXT NOT NULL,
          fetched_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS media_path_map (
          path TEXT PRIMARY KEY,
          kind TEXT NOT NULL,
          series_id TEXT,
          episode_id TEXT,
          movie_id TEXT,
          season INTEGER,
          episode INTEGER,
          parsed_title TEXT NOT NULL DEFAULT '',
          confidence REAL NOT NULL DEFAULT 0,
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS catalog_overrides (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          kind TEXT NOT NULL,
          source_key TEXT NOT NULL,
          target_key TEXT,
          display_title TEXT,
          created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_overrides_source ON catalog_overrides(source_key);
        "#,
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO schema_migrations (version) VALUES (1)",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run_migrations(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
          version INTEGER PRIMARY KEY NOT NULL
        );
        "#,
    )
    .map_err(|e| e.to_string())?;

    let current = schema_version(conn)?;
    if current == 0 {
        migrate_baseline(conn)?;
    }

    let current = schema_version(conn)?;
    if current != SCHEMA_VERSION {
        return Err(format!(
            "incomplete migrations: schema {current}, expected {SCHEMA_VERSION}"
        ));
    }

    Ok(())
}

fn remove_db_files(path: &PathBuf) -> Result<(), String> {
    for suffix in ["", "-wal", "-shm"] {
        let candidate = if suffix.is_empty() {
            path.clone()
        } else {
            PathBuf::from(format!("{}{suffix}", path.display()))
        };
        match std::fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(())
}

/// Open the app database on the single baseline schema.
/// Pre-release: any other schema stamp is treated as obsolete local state and
/// the SQLite file is deleted then recreated (no migration chain).
pub fn prepare_database(path: &PathBuf) -> Result<Connection, String> {
    let conn = open_connection(path)?;
    match run_migrations(&conn) {
        Ok(()) => Ok(conn),
        Err(err) if err.starts_with("incomplete migrations:") => {
            drop(conn);
            remove_db_files(path)?;
            let conn = open_connection(path)?;
            run_migrations(&conn)?;
            Ok(conn)
        }
        Err(err) => Err(err),
    }
}

pub fn load_settings(conn: &Connection) -> Result<AppSettings, String> {
    conn.query_row(
        r#"
        SELECT wizard_step, skip_intro, server_port, setup_complete, tmdb_api_key,
               library_check_minutes, cache_max_gb, purge_cache_after_watch,
               jam_quiz_mode, jam_quiz_interval_seconds,
               jam_display_sub_en, jam_display_sub_fr
        FROM app_settings WHERE id = 1
        "#,
        [],
        |row| {
            Ok(AppSettings {
                wizard_step: row.get(0)?,
                skip_intro: row.get::<_, i32>(1)? != 0,
                server_port: row.get::<_, i32>(2)? as u32,
                setup_complete: row.get::<_, i32>(3)? != 0,
                tmdb_api_key: row.get::<_, String>(4).unwrap_or_default(),
                library_check_minutes: row.get::<_, i32>(5).unwrap_or(60).max(0) as u32,
                cache_max_gb: row.get::<_, i32>(6).unwrap_or(20).max(0) as u32,
                purge_cache_after_watch: row.get::<_, i32>(7).unwrap_or(0) != 0,
                jam_quiz_mode: row.get::<_, i32>(8).unwrap_or(0) != 0,
                jam_quiz_interval_seconds: normalize_quiz_interval(
                    row.get::<_, i32>(9).unwrap_or(60) as u32,
                ),
                jam_display_sub_en: row.get::<_, i32>(10).unwrap_or(0) != 0,
                jam_display_sub_fr: row.get::<_, i32>(11).unwrap_or(0) != 0,
            })
        },
    )
    .map_err(|e| e.to_string())
}

/// Allowed Jam quiz intervals (seconds). Unknown values fall back to 60.
pub fn normalize_quiz_interval(raw: u32) -> u32 {
    if QUIZ_INTERVAL_SECONDS.contains(&raw) {
        raw
    } else {
        60
    }
}

pub fn save_settings(conn: &Connection, settings: &AppSettings) -> Result<(), String> {
    conn.execute(
        r#"
        UPDATE app_settings SET
          wizard_step = ?1,
          skip_intro = ?2,
          server_port = ?3,
          setup_complete = ?4,
          tmdb_api_key = ?5,
          library_check_minutes = ?6,
          cache_max_gb = ?7,
          purge_cache_after_watch = ?8,
          jam_quiz_mode = ?9,
          jam_quiz_interval_seconds = ?10,
          jam_display_sub_en = ?11,
          jam_display_sub_fr = ?12,
          updated_at = datetime('now')
        WHERE id = 1
        "#,
        params![
            settings.wizard_step,
            if settings.skip_intro { 1 } else { 0 },
            settings.server_port as i32,
            if settings.setup_complete { 1 } else { 0 },
            settings.tmdb_api_key.trim(),
            settings.library_check_minutes as i32,
            settings.cache_max_gb as i32,
            if settings.purge_cache_after_watch { 1 } else { 0 },
            if settings.jam_quiz_mode { 1 } else { 0 },
            normalize_quiz_interval(settings.jam_quiz_interval_seconds) as i32,
            if settings.jam_display_sub_en { 1 } else { 0 },
            if settings.jam_display_sub_fr { 1 } else { 0 },
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Convert a cache ceiling in GiB to bytes. 0 means unlimited.
pub fn cache_max_bytes(cache_max_gb: u32) -> u64 {
    if cache_max_gb == 0 {
        0
    } else {
        u64::from(cache_max_gb) * 1024 * 1024 * 1024
    }
}

pub fn list_media_roots(conn: &Connection) -> Result<Vec<MediaRoot>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, sort_order FROM media_roots ORDER BY sort_order ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let path: String = row.get(1)?;
            let accessible = std::path::Path::new(&path).is_dir();
            Ok(MediaRoot {
                id: row.get(0)?,
                path,
                sort_order: row.get(2)?,
                accessible,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn add_media_root(conn: &Connection, path: &str) -> Result<MediaRoot, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Chemin vide".into());
    }
    let next_order: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM media_roots",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO media_roots (path, sort_order) VALUES (?1, ?2)",
        params![trimmed, next_order],
    )
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            "Ce dossier est déjà dans la liste".into()
        } else {
            e.to_string()
        }
    })?;
    let id = conn.last_insert_rowid();
    let accessible = std::path::Path::new(trimmed).is_dir();
    Ok(MediaRoot {
        id,
        path: trimmed.to_string(),
        sort_order: next_order,
        accessible,
    })
}

pub fn remove_media_root(conn: &Connection, id: i64) -> Result<(), String> {
    let changed = conn
        .execute("DELETE FROM media_roots WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("Dossier introuvable".into());
    }
    Ok(())
}

pub fn replace_catalog(conn: &Connection, snapshot: &CatalogSnapshot) -> Result<(), String> {
    // Atomic replace: readers must never see a half-written catalog (e.g. 9 series, 0 movies).
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    tx.execute_batch(
        r#"
        DELETE FROM catalog_episodes;
        DELETE FROM catalog_seasons;
        DELETE FROM catalog_series;
        DELETE FROM catalog_movies;
        DELETE FROM catalog_meta;
        "#,
    )
    .map_err(|e| e.to_string())?;

    for series in &snapshot.series {
        tx.execute(
            r#"
            INSERT INTO catalog_series (id, title, root, display_title, synopsis, poster_url, backdrop_url, tmdb_id, original_language)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                series.id,
                series.title,
                series.root,
                series.display_title,
                series.synopsis,
                series.poster_url,
                series.backdrop_url,
                series.tmdb_id,
                series.original_language
            ],
        )
        .map_err(|e| e.to_string())?;
        for season in &series.seasons {
            tx.execute(
                "INSERT INTO catalog_seasons (series_id, number, title, poster_url, synopsis) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![series.id, season.number, season.title, season.poster_url, season.synopsis],
            )
            .map_err(|e| e.to_string())?;
            for episode in &season.episodes {
                tx.execute(
                    r#"
                    INSERT INTO catalog_episodes (id, series_id, season, episode, label, path, title, poster_url, synopsis, confidence, tmdb_checked)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                    "#,
                    params![
                        episode.id,
                        series.id,
                        episode.season,
                        episode.episode,
                        episode.label,
                        episode.path,
                        episode.title,
                        episode.poster_url,
                        episode.synopsis,
                        episode.confidence as f64,
                        if episode.tmdb_checked { 1 } else { 0 }
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    for movie in &snapshot.movies {
        tx.execute(
            r#"
            INSERT INTO catalog_movies (id, title, path, display_title, synopsis, poster_url, backdrop_url, tmdb_id, year, original_language)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                movie.id,
                movie.title,
                movie.path,
                movie.display_title,
                movie.synopsis,
                movie.poster_url,
                movie.backdrop_url,
                movie.tmdb_id,
                movie.year,
                movie.original_language
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    tx.execute(
        "INSERT INTO catalog_meta (id, scanned_at) VALUES (1, datetime('now'))",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn replace_media_path_map(
    conn: &Connection,
    mappings: &[crate::scan::PathMapping],
) -> Result<(), String> {
    conn.execute("DELETE FROM media_path_map", [])
        .map_err(|e| e.to_string())?;
    for m in mappings {
        conn.execute(
            r#"
            INSERT INTO media_path_map (
              path, kind, series_id, episode_id, movie_id, season, episode, parsed_title, confidence, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))
            "#,
            params![
                m.path,
                m.kind,
                m.series_id,
                m.episode_id,
                m.movie_id,
                m.season,
                m.episode,
                m.parsed_title,
                m.confidence as f64
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn list_catalog_overrides(conn: &Connection) -> Result<Vec<CatalogOverride>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, source_key, target_key, display_title FROM catalog_overrides ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CatalogOverride {
                id: row.get(0)?,
                kind: row.get(1)?,
                source_key: row.get(2)?,
                target_key: row.get(3)?,
                display_title: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn load_catalog(conn: &Connection) -> Result<CatalogSnapshot, String> {
    let scanned_at: Option<String> = conn
        .query_row("SELECT scanned_at FROM catalog_meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;

    let mut series_stmt = conn
        .prepare(
            r#"
            SELECT id, title, root, display_title, synopsis, poster_url, backdrop_url, tmdb_id, original_language
            FROM catalog_series
            ORDER BY COALESCE(display_title, title) COLLATE NOCASE ASC
            "#,
        )
        .map_err(|e| e.to_string())?;
    let series_rows = series_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i32>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut series = Vec::new();
    for row in series_rows {
        let (
            id,
            title,
            root,
            display_title,
            synopsis,
            poster_url,
            backdrop_url,
            tmdb_id,
            original_language,
        ) = row.map_err(|e| e.to_string())?;
        let mut ep_stmt = conn
            .prepare(
                r#"
                SELECT id, season, episode, label, path, title, poster_url, confidence, synopsis, tmdb_checked
                FROM catalog_episodes
                WHERE series_id = ?1
                ORDER BY season ASC, episode ASC
                "#,
            )
            .map_err(|e| e.to_string())?;
        let episodes = ep_stmt
            .query_map(params![id], |row| {
                Ok(crate::scan::CatalogEpisode {
                    id: row.get(0)?,
                    season: row.get(1)?,
                    episode: row.get(2)?,
                    label: row.get(3)?,
                    path: row.get(4)?,
                    title: row.get(5)?,
                    poster_url: row.get(6)?,
                    confidence: row.get::<_, f64>(7).unwrap_or(1.0) as f32,
                    synopsis: row.get(8)?,
                    tmdb_checked: row.get::<_, i32>(9).unwrap_or(0) != 0,
                    prep: crate::cache::EpisodePrep::default(),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let mut season_meta = conn
            .prepare(
                "SELECT number, title, poster_url, synopsis FROM catalog_seasons WHERE series_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let metas: Vec<SeasonMetaRow> = season_meta
            .query_map(params![id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let mut seasons: Vec<CatalogSeason> = Vec::new();
        for episode in episodes {
            if let Some(last) = seasons.last_mut() {
                if last.number == episode.season {
                    last.episodes.push(episode);
                    continue;
                }
            }
            let meta = metas.iter().find(|m| m.0 == episode.season);
            seasons.push(CatalogSeason {
                number: episode.season,
                title: meta.and_then(|m| m.1.clone()),
                poster_url: meta.and_then(|m| m.2.clone()),
                synopsis: meta.and_then(|m| m.3.clone()),
                episodes: vec![episode],
            });
        }

        series.push(CatalogSeries {
            id,
            title,
            root,
            poster_url,
            backdrop_url,
            synopsis,
            tmdb_id,
            original_language,
            display_title,
            seasons,
        });
    }

    let mut movie_stmt = conn
        .prepare(
            r#"
            SELECT id, title, path, display_title, synopsis, poster_url, backdrop_url, tmdb_id, year, original_language
            FROM catalog_movies
            ORDER BY COALESCE(display_title, title) COLLATE NOCASE ASC
            "#,
        )
        .map_err(|e| e.to_string())?;
    let movies = movie_stmt
        .query_map([], |row| {
            Ok(CatalogMovie {
                id: row.get(0)?,
                title: row.get(1)?,
                path: row.get(2)?,
                display_title: row.get(3)?,
                synopsis: row.get(4)?,
                poster_url: row.get(5)?,
                backdrop_url: row.get(6)?,
                tmdb_id: row.get(7)?,
                year: row.get(8)?,
                original_language: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(CatalogSnapshot {
        scanned_at: scanned_at.unwrap_or_default(),
        series,
        movies,
    })
}

pub fn is_known_media_path(conn: &Connection, path: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            r#"
            SELECT
              (SELECT COUNT(*) FROM catalog_episodes WHERE path = ?1)
              + (SELECT COUNT(*) FROM catalog_movies WHERE path = ?1)
            "#,
            params![path],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

/// Normalize TMDB ISO language to NeoLingua track codes (`en` / `fr`), or None.
pub fn normalize_track_language(raw: Option<&str>) -> Option<&'static str> {
    let lang = raw?.trim().to_ascii_lowercase();
    if lang.is_empty() {
        return None;
    }
    if lang == "en" || lang.starts_with("eng") {
        return Some("en");
    }
    if lang == "fr" || lang.starts_with("fre") || lang.starts_with("fra") {
        return Some("fr");
    }
    None
}

/// Original language for a media path (series or movie), already normalized to en/fr when possible.
/// Returns the raw TMDB code when outside en/fr so callers can explain why Whisper is skipped.
pub fn lookup_original_language(conn: &Connection, media_path: &str) -> Result<Option<String>, String> {
    let from_series: Option<String> = conn
        .query_row(
            r#"
            SELECT s.original_language
            FROM catalog_episodes e
            JOIN catalog_series s ON s.id = e.series_id
            WHERE e.path = ?1
            LIMIT 1
            "#,
            params![media_path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if from_series.is_some() {
        return Ok(from_series);
    }
    let from_movie: Option<String> = conn
        .query_row(
            "SELECT original_language FROM catalog_movies WHERE path = ?1 LIMIT 1",
            params![media_path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(from_movie)
}

pub fn with_connection<F, T>(state: &DbState, f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String>,
{
    let conn = open_connection(&state.path)?;
    run_migrations(&conn)?;
    f(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db() -> (PathBuf, Connection) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tid = std::thread::current().id();
        let path = std::env::temp_dir().join(format!("neolingua-db-test-{nanos}-{tid:?}.sqlite"));
        let conn = open_connection(&path).expect("open");
        run_migrations(&conn).expect("migrate");
        (path, conn)
    }

    #[test]
    fn cache_max_bytes_unlimited_and_gib() {
        assert_eq!(cache_max_bytes(0), 0);
        assert_eq!(cache_max_bytes(1), 1024 * 1024 * 1024);
        assert_eq!(cache_max_bytes(20), 20 * 1024 * 1024 * 1024);
    }

    #[test]
    fn normalize_track_language_en_fr() {
        assert_eq!(normalize_track_language(Some("en")), Some("en"));
        assert_eq!(normalize_track_language(Some("eng")), Some("en"));
        assert_eq!(normalize_track_language(Some("FR")), Some("fr"));
        assert_eq!(normalize_track_language(Some("fra")), Some("fr"));
        assert_eq!(normalize_track_language(Some("de")), None);
        assert_eq!(normalize_track_language(None), None);
        assert_eq!(normalize_track_language(Some("  ")), None);
    }

    #[test]
    fn settings_roundtrip_and_media_roots() {
        let (path, conn) = temp_db();
        let mut settings = load_settings(&conn).expect("load");
        settings.wizard_step = "options".into();
        settings.skip_intro = true;
        settings.server_port = 9000;
        settings.setup_complete = true;
        settings.tmdb_api_key = "  abc  ".into();
        settings.cache_max_gb = 5;
        settings.purge_cache_after_watch = true;
        settings.jam_quiz_mode = true;
        settings.jam_quiz_interval_seconds = 120;
        settings.jam_display_sub_en = true;
        settings.jam_display_sub_fr = false;
        save_settings(&conn, &settings).expect("save");

        let loaded = load_settings(&conn).expect("reload");
        assert_eq!(loaded.wizard_step, "options");
        assert!(loaded.skip_intro);
        assert_eq!(loaded.server_port, 9000);
        assert!(loaded.setup_complete);
        assert_eq!(loaded.tmdb_api_key, "abc");
        assert_eq!(loaded.cache_max_gb, 5);
        assert!(loaded.purge_cache_after_watch);
        assert!(loaded.jam_quiz_mode);
        assert_eq!(loaded.jam_quiz_interval_seconds, 120);
        assert!(loaded.jam_display_sub_en);
        assert!(!loaded.jam_display_sub_fr);

        settings.jam_quiz_interval_seconds = 7;
        save_settings(&conn, &settings).expect("save bad interval");
        let clamped = load_settings(&conn).expect("reload clamped");
        assert_eq!(clamped.jam_quiz_interval_seconds, 60);

        let tmp_dir = std::env::temp_dir();
        let root = add_media_root(&conn, tmp_dir.to_str().unwrap()).expect("add root");
        let listed = list_media_roots(&conn).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, root.id);
        remove_media_root(&conn, root.id).expect("remove");
        assert!(list_media_roots(&conn).expect("list empty").is_empty());

        drop(conn);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reject_empty_media_root() {
        let (path, conn) = temp_db();
        assert!(add_media_root(&conn, "  ").is_err());
        drop(conn);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn catalog_roundtrip_and_known_path() {
        let (path, conn) = temp_db();
        let snap = CatalogSnapshot {
            scanned_at: "2020-01-01T00:00:00Z".into(),
            series: vec![crate::scan::CatalogSeries {
                id: "local:show".into(),
                title: "Show".into(),
                root: "/media".into(),
                poster_url: None,
                backdrop_url: None,
                synopsis: None,
                tmdb_id: None,
                original_language: Some("en".into()),
                display_title: None,
                seasons: vec![crate::scan::CatalogSeason {
                    number: 1,
                    title: None,
                    poster_url: None,
                    synopsis: None,
                    episodes: vec![crate::scan::CatalogEpisode {
                        id: "local:show-s01e01".into(),
                        season: 1,
                        episode: 1,
                        label: "S01E01".into(),
                        path: "/media/s01e01.mkv".into(),
                        title: None,
                        poster_url: None,
                        synopsis: None,
                        confidence: 0.9,
                        tmdb_checked: false,
                        prep: crate::cache::EpisodePrep::default(),
                    }],
                }],
            }],
            movies: vec![crate::scan::CatalogMovie {
                id: "local:film".into(),
                title: "Film".into(),
                path: "/media/film.mkv".into(),
                poster_url: None,
                backdrop_url: None,
                synopsis: None,
                tmdb_id: None,
                original_language: None,
                year: Some(2001),
                display_title: None,
            }],
        };
        replace_catalog(&conn, &snap).expect("replace");
        let loaded = load_catalog(&conn).expect("load");
        assert_eq!(loaded.series.len(), 1);
        assert_eq!(loaded.movies.len(), 1);
        assert!(is_known_media_path(&conn, "/media/s01e01.mkv").unwrap());
        assert!(is_known_media_path(&conn, "/media/film.mkv").unwrap());
        assert!(!is_known_media_path(&conn, "/media/other.mkv").unwrap());
        drop(conn);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn migrations_are_idempotent() {
        let (path, conn) = temp_db();
        run_migrations(&conn).expect("second migrate");
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        drop(conn);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn normalize_quiz_interval_whitelist() {
        assert_eq!(normalize_quiz_interval(30), 30);
        assert_eq!(normalize_quiz_interval(60), 60);
        assert_eq!(normalize_quiz_interval(120), 120);
        assert_eq!(normalize_quiz_interval(300), 300);
        assert_eq!(normalize_quiz_interval(900), 900);
        assert_eq!(normalize_quiz_interval(5), 60);
        assert_eq!(normalize_quiz_interval(90), 60);
        assert_eq!(normalize_quiz_interval(0), 60);
    }

    #[test]
    fn prepare_database_resets_obsolete_schema_stamp() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("neolingua-db-obsolete-{nanos}.sqlite"));
        let conn = open_connection(&path).expect("open");
        conn.execute_batch(
            r#"
            CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY NOT NULL);
            INSERT INTO schema_migrations (version) VALUES (11);
            "#,
        )
        .expect("seed obsolete stamp");
        drop(conn);

        let conn = prepare_database(&path).expect("reset");
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        let settings = load_settings(&conn).expect("settings");
        assert_eq!(settings.server_port, 8787);
        drop(conn);
        let _ = std::fs::remove_file(path);
    }
}
