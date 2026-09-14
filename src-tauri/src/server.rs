use crate::cache::{self, EpisodePrep, PrepRegistry, PrepStatus, SubTrackSource};
use crate::db;
use crate::jam::{self, JamRegistry, SessionPublic, SessionView};
use crate::presence::{PresenceHeartbeat, PresenceRegistry, PresenceSnapshot};
use axum::body::Body;
use axum::extract::{FromRef, Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running: bool,
    pub port: u16,
    pub error: Option<String>,
}

#[derive(Clone)]
struct HttpState {
    port: u16,
    db_path: PathBuf,
    prep: Arc<PrepRegistry>,
    presence: Arc<PresenceRegistry>,
    jam: Arc<JamRegistry>,
    viewer_dir: PathBuf,
}

impl FromRef<HttpState> for Arc<JamRegistry> {
    fn from_ref(state: &HttpState) -> Self {
        Arc::clone(&state.jam)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthBody {
    ok: bool,
    service: &'static str,
    port: u16,
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: String,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PrepareBody {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobStateBody {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackBody {
    status: String,
    source: SubTrackSource,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubtitlesBody {
    status: String,
    message: String,
    en: TrackBody,
    fr: TrackBody,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusBody {
    id: String,
    video: JobStateBody,
    subtitles: SubtitlesBody,
}

struct Inner {
    port: u16,
    running: bool,
    error: Option<String>,
    stop: Option<oneshot::Sender<()>>,
}

pub struct ServerController {
    db_path: PathBuf,
    prep: Arc<PrepRegistry>,
    presence: Arc<PresenceRegistry>,
    jam: Arc<JamRegistry>,
    viewer_dir: PathBuf,
    inner: Mutex<Inner>,
}

/// Resolve the LAN viewer static files: bundled Resources in release, crate
/// `viewer/` in development.
fn resolve_viewer_dir() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // macOS app: Contents/MacOS → Contents/Resources/viewer
            candidates.push(dir.join("../Resources/viewer"));
            candidates.push(dir.join("resources/viewer"));
            candidates.push(dir.join("viewer"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("viewer"));

    for candidate in &candidates {
        if candidate.join("index.html").is_file() {
            return candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("viewer")
}

impl ServerController {
    pub fn new(db_path: PathBuf, prep: Arc<PrepRegistry>) -> Self {
        let viewer_dir = resolve_viewer_dir();
        Self {
            db_path,
            prep,
            presence: Arc::new(PresenceRegistry::new()),
            jam: Arc::new(JamRegistry::new()),
            viewer_dir,
            inner: Mutex::new(Inner {
                port: 8787,
                running: false,
                error: None,
                stop: None,
            }),
        }
    }

    pub fn status(&self) -> ServerStatus {
        let inner = self.inner.lock().expect("server lock");
        ServerStatus {
            running: inner.running,
            port: inner.port,
            error: inner.error.clone(),
        }
    }

    pub fn presence_snapshot(&self) -> crate::presence::PresenceSnapshot {
        self.presence.snapshot()
    }

    /// Stop the LAN HTTP server without starting a new one (used before app relaunch).
    pub fn stop(&self) {
        let mut inner = self.inner.lock().expect("server lock");
        if let Some(stop) = inner.stop.take() {
            let _ = stop.send(());
        }
        inner.running = false;
        inner.error = None;
    }

    fn apply_bind_err(&self, port: u16, message: String) {
        let mut inner = self.inner.lock().expect("server lock");
        if inner.port != port {
            return;
        }
        inner.running = false;
        inner.error = Some(message);
        inner.stop = None;
    }

    fn apply_stopped(&self, port: u16) {
        let mut inner = self.inner.lock().expect("server lock");
        if inner.port != port {
            return;
        }
        inner.running = false;
        inner.stop = None;
    }

    /// Prefer `preferred`, then try the next ports if that one is already taken.
    fn adopt_listening_port(&self, preferred: u16, actual: u16) {
        let mut inner = self.inner.lock().expect("server lock");
        if inner.port != preferred {
            return;
        }
        inner.port = actual;
        inner.running = true;
        inner.error = None;
    }

    pub fn ensure_running(this: &std::sync::Arc<Self>, port: u16) {
        {
            let mut inner = this.inner.lock().expect("server lock");
            if inner.running && inner.port == port && inner.error.is_none() {
                return;
            }
            if let Some(stop) = inner.stop.take() {
                let _ = stop.send(());
            }
            inner.port = port;
            inner.running = false;
            inner.error = None;
        }

        let (stop_tx, stop_rx) = oneshot::channel();
        {
            let mut inner = this.inner.lock().expect("server lock");
            inner.stop = Some(stop_tx);
        }

        let controller = std::sync::Arc::clone(this);
        tauri::async_runtime::spawn(async move {
            run_http_server(controller, port, stop_rx).await;
        });
    }
}

fn persist_server_port(db_path: &PathBuf, port: u16) {
    let result = (|| -> Result<(), String> {
        let conn = db::open_migrated(db_path)?;
        let mut settings = db::load_settings(&conn)?;
        if settings.server_port == u32::from(port) {
            return Ok(());
        }
        settings.server_port = u32::from(port);
        db::save_settings(&conn, &settings)
    })();
    if let Err(err) = result {
        eprintln!("neolingua-center: failed to persist server port {port}: {err}");
    }
}

async fn bind_lan_listener(preferred: u16) -> Result<(TcpListener, u16), String> {
    let last = preferred.saturating_add(19);
    let mut last_err = String::new();
    for candidate in preferred..=last {
        let addr = SocketAddr::from(([0, 0, 0, 0], candidate));
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok((listener, candidate)),
            Err(err) => {
                last_err = err.to_string();
            }
        }
    }
    Err(format!(
        "Impossible d'écouter sur le port {preferred} (ni les suivants jusqu'à {last}) : {last_err}"
    ))
}

async fn run_http_server(
    controller: std::sync::Arc<ServerController>,
    preferred_port: u16,
    stop_rx: oneshot::Receiver<()>,
) {
    let (listener, port) = match bind_lan_listener(preferred_port).await {
        Ok(bound) => bound,
        Err(message) => {
            controller.apply_bind_err(preferred_port, message);
            return;
        }
    };

    if port != preferred_port {
        persist_server_port(&controller.db_path, port);
    }
    controller.adopt_listening_port(preferred_port, port);

    let state = HttpState {
        port,
        db_path: controller.db_path.clone(),
        prep: Arc::clone(&controller.prep),
        presence: Arc::clone(&controller.presence),
        jam: Arc::clone(&controller.jam),
        viewer_dir: controller.viewer_dir.clone(),
    };

    let index = state.viewer_dir.join("index.html");
    if !index.is_file() {
        let message = format!(
            "Viewer LAN introuvable (attendu : {}). Réinstallez Neolingua Center.",
            index.display()
        );
        controller.apply_bind_err(port, message);
        return;
    }
    let static_files = ServeDir::new(&state.viewer_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index));

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/settings", get(public_settings))
        .route("/api/library", get(library))
        .route("/api/status", get(status))
        .route("/api/presence", get(presence_get).post(presence_post))
        .route("/api/jam/info", get(jam_info))
        .route("/api/jam/sessions", post(jam_create_session))
        .route("/api/jam/sessions/{id}", get(jam_get_session))
        .route("/ws/jam", get(jam::ws_handler))
        .route("/api/prepare-video", post(prepare_video))
        .route("/api/purge-cache", post(purge_cache))
        .route("/api/video", get(video))
        .route("/api/subtitles.vtt", get(subtitles_vtt))
        .fallback_service(static_files)
        // Family LAN host: any device on the local network may open the viewer
        // (TV, phone, laptop). Origins are not a fixed allowlist. The trust
        // boundary is the LAN itself (no Internet exposure assumed).
        .layer(CorsLayer::permissive())
        .with_state(state);

    let serve = axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = stop_rx.await;
    });

    if let Err(err) = serve.await {
        controller.apply_bind_err(port, format!("Le serveur s'est arrêté ({err})"));
        return;
    }
    controller.apply_stopped(port);
}

async fn health(State(state): State<HttpState>) -> Json<HealthBody> {
    Json(HealthBody {
        ok: true,
        service: "neolingua-center",
        port: state.port,
    })
}

async fn presence_get(State(state): State<HttpState>) -> Json<PresenceSnapshot> {
    Json(state.presence.snapshot())
}

async fn presence_post(
    State(state): State<HttpState>,
    Json(body): Json<PresenceHeartbeat>,
) -> Json<PresenceSnapshot> {
    state.presence.heartbeat(body);
    Json(state.presence.snapshot())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JamCreateBody {
    #[serde(default)]
    quiz_mode: Option<bool>,
    #[serde(default)]
    quiz_interval_seconds: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JamInfoBody {
    port: u16,
    addresses: Vec<String>,
}

async fn jam_info(State(state): State<HttpState>) -> Json<JamInfoBody> {
    Json(JamInfoBody {
        port: state.port,
        addresses: crate::network::list_lan_addresses(),
    })
}

async fn jam_create_session(
    State(state): State<HttpState>,
    Json(body): Json<JamCreateBody>,
) -> (StatusCode, Json<SessionPublic>) {
    let quiz_mode = body.quiz_mode.unwrap_or(false);
    let quiz_interval_seconds = body
        .quiz_interval_seconds
        .filter(|v| v.is_finite())
        .unwrap_or(60.0);
    let created = state.jam.create_session(quiz_mode, quiz_interval_seconds).await;
    (StatusCode::CREATED, Json(created))
}

async fn jam_get_session(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SessionView>, (StatusCode, Json<serde_json::Value>)> {
    state.jam.get_session(&id).await.map(Json).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Session introuvable" })),
        )
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicSettingsBody {
    skip_intro: bool,
    purge_cache_after_watch: bool,
    jam_quiz_mode: bool,
    jam_quiz_interval_seconds: u32,
    jam_display_sub_en: bool,
    jam_display_sub_fr: bool,
}

async fn public_settings(
    State(state): State<HttpState>,
) -> Result<Json<PublicSettingsBody>, (StatusCode, String)> {
    let path = state.db_path.clone();
    let settings = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open_migrated(&path)?;
        db::load_settings(&conn)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(PublicSettingsBody {
        skip_intro: settings.skip_intro,
        purge_cache_after_watch: settings.purge_cache_after_watch,
        jam_quiz_mode: settings.jam_quiz_mode,
        jam_quiz_interval_seconds: settings.jam_quiz_interval_seconds,
        jam_display_sub_en: settings.jam_display_sub_en,
        jam_display_sub_fr: settings.jam_display_sub_fr,
    }))
}

async fn library(
    State(state): State<HttpState>,
) -> Result<Json<crate::scan::CatalogSnapshot>, (StatusCode, String)> {
    let path = state.db_path.clone();
    let prep = Arc::clone(&state.prep);
    let mut snapshot = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open_migrated(&path)?;
        db::load_catalog(&conn)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    cache::enrich_catalog_prep(&mut snapshot, &prep);
    Ok(Json(snapshot))
}

async fn status(
    State(state): State<HttpState>,
    Query(query): Query<PathQuery>,
) -> Result<Json<StatusBody>, (StatusCode, String)> {
    let path = normalize_path(&query.path)?;
    ensure_known_path(&state.db_path, &path).await?;
    let prep = state.prep.inspect(&path);
    Ok(Json(status_from_prep(&path, &prep)))
}

async fn prepare_video(
    State(state): State<HttpState>,
    Json(body): Json<PrepareBody>,
) -> Result<Json<StatusBody>, (StatusCode, String)> {
    let path = normalize_path(&body.path)?;
    ensure_known_path(&state.db_path, &path).await?;
    let current = state.prep.inspect(&path);
    if current.status != PrepStatus::Ready
        && current.status != PrepStatus::Processing
        && current.status != PrepStatus::Queued
    {
        state.prep.enqueue(std::slice::from_ref(&path));
        let prep_reg = Arc::clone(&state.prep);
        let media_path = path.clone();
        let db_path = state.db_path.clone();
        // Same pipeline as Center "Préparer" (prepare_from_db).
        tauri::async_runtime::spawn_blocking(move || {
            let _ = prep_reg.prepare_from_db(&db_path, &media_path, |_| {});
        });
    }
    let prep = state.prep.inspect(&path);
    Ok(Json(status_from_prep(&path, &prep)))
}

async fn purge_cache(
    State(state): State<HttpState>,
    Json(body): Json<PrepareBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = normalize_path(&body.path)?;
    ensure_known_path(&state.db_path, &path).await?;
    let db_path = state.db_path.clone();
    let allowed = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open_migrated(&db_path)?;
        Ok::<bool, String>(db::load_settings(&conn)?.purge_cache_after_watch)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            "Purge après visionnage désactivée".into(),
        ));
    }
    state.prep.purge_episode(&path);
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn video(
    State(state): State<HttpState>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let path = normalize_path(&query.path)?;
    ensure_known_path(&state.db_path, &path).await?;
    let Some(file_path) = state.prep.video_file(&path) else {
        return Err((
            StatusCode::CONFLICT,
            "Vidéo pas encore prête. Lancez la préparation.".into(),
        ));
    };
    state.prep.touch_episode(&path);
    stream_file(file_path, "video/mp4", &headers).await
}

async fn subtitles_vtt(
    State(state): State<HttpState>,
    Query(query): Query<PathQuery>,
) -> Result<Response, (StatusCode, String)> {
    let path = normalize_path(&query.path)?;
    ensure_known_path(&state.db_path, &path).await?;
    let lang = query.lang.as_deref().unwrap_or("en");
    let Some(file_path) = state.prep.subtitle_file(&path, lang) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!(
                "Sous-titres {} indisponibles",
                if lang == "fr" { "français" } else { "anglais" }
            ),
        ));
    };
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/vtt; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

fn status_from_prep(path: &str, prep: &EpisodePrep) -> StatusBody {
    // Prefer job phase over on-disk video: remux may finish while Whisper is still running.
    let video_status = if matches!(
        prep.status,
        PrepStatus::Processing | PrepStatus::Queued
    ) {
        "processing"
    } else if prep.status == PrepStatus::Error {
        "error"
    } else if prep.video {
        "ready"
    } else {
        "idle"
    };
    let en_ready = prep.subs_en;
    let fr_ready = prep.subs_fr;
    let (subs_status, default_message) = if en_ready && fr_ready {
        ("ready", "EN + FR prêts")
    } else if en_ready || fr_ready {
        ("partial", "Préparation incomplète")
    } else {
        ("missing", "Sous-titres absents")
    };
    let subs_message = prep
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| {
            !m.is_empty()
                && !matches!(
                    prep.status,
                    PrepStatus::Processing | PrepStatus::Queued
                )
        })
        .unwrap_or(default_message);
    StatusBody {
        id: path.to_string(),
        video: JobStateBody {
            status: video_status.into(),
            message: {
                let raw = prep.message.clone().unwrap_or_default();
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    if prep.status == PrepStatus::Error {
                        Some("Préparation impossible (fichier illisible ou corrompu).".into())
                    } else {
                        None
                    }
                } else {
                    Some(trimmed.to_string())
                }
            },
            progress: prep.progress,
        },
        subtitles: SubtitlesBody {
            status: subs_status.into(),
            message: subs_message.into(),
            en: TrackBody {
                status: if en_ready { "ready" } else { "idle" }.into(),
                source: prep.en_source,
            },
            fr: TrackBody {
                status: if fr_ready { "ready" } else { "idle" }.into(),
                source: prep.fr_source,
            },
        },
    }
}

fn normalize_path(raw: &str) -> Result<String, (StatusCode, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Paramètre path manquant".into()));
    }
    Ok(trimmed.replace('\\', "/"))
}

async fn ensure_known_path(db_path: &Path, path: &str) -> Result<(), (StatusCode, String)> {
    let db_path = db_path.to_path_buf();
    let media_path = path.to_string();
    let known = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open_migrated(&db_path)?;
        db::is_known_media_path(&conn, &media_path)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !known {
        return Err((StatusCode::FORBIDDEN, "Média hors catalogue".into()));
    }
    Ok(())
}

async fn stream_file(
    path: PathBuf,
    content_type: &'static str,
    headers: &HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    let file_size = meta.len();
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some((start, end)) = parse_byte_range(range_header, file_size) {
        if start >= file_size || end >= file_size || start > end {
            return Err((
                StatusCode::RANGE_NOT_SATISFIABLE,
                format!("bytes */{file_size}"),
            ));
        }
        let mut file = File::open(&path)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let take = end - start + 1;
        let reader = file.take(take);
        let stream = ReaderStream::new(reader);
        let body = Body::from_stream(stream);
        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        let headers_mut = response.headers_mut();
        headers_mut.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        headers_mut.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers_mut.insert(
            header::CONTENT_LENGTH,
            header_value_u64(take)?,
        );
        headers_mut.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{file_size}")).map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "En-tête Content-Range invalide".into(),
                )
            })?,
        );
        return Ok(response);
    }

    let file = File::open(&path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    let headers_mut = response.headers_mut();
    headers_mut.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers_mut.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers_mut.insert(header::CONTENT_LENGTH, header_value_u64(file_size)?);
    Ok(response)
}

fn header_value_u64(value: u64) -> Result<HeaderValue, (StatusCode, String)> {
    HeaderValue::from_str(&value.to_string()).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "En-tête HTTP numérique invalide".into(),
        )
    })
}

/// Parse a single RFC 7233 bytes range (`bytes=start-end`, `bytes=start-`, or suffix `bytes=-N`).
fn parse_byte_range(header: &str, file_size: u64) -> Option<(u64, u64)> {
    if file_size == 0 {
        return None;
    }
    let header = header.trim();
    if !header.starts_with("bytes=") {
        return None;
    }
    let spec = &header["bytes=".len()..];
    // Only one range is supported (video players send a single range).
    if spec.contains(',') {
        return None;
    }
    let (start_raw, end_raw) = spec.split_once('-')?;
    let last = file_size - 1;

    if start_raw.is_empty() {
        // Suffix form: last N bytes (`bytes=-500`).
        let n: u64 = end_raw.parse().ok()?;
        if n == 0 {
            return None;
        }
        let start = file_size.saturating_sub(n);
        return Some((start, last));
    }

    let start: u64 = start_raw.parse().ok()?;
    if start > last {
        return None;
    }
    let end = if end_raw.is_empty() {
        last
    } else {
        end_raw.parse::<u64>().ok()?.min(last)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_byte_range_suffix_and_open_end() {
        assert_eq!(parse_byte_range("bytes=0-", 1000), Some((0, 999)));
        assert_eq!(parse_byte_range("bytes=100-199", 1000), Some((100, 199)));
        assert_eq!(parse_byte_range("bytes=-500", 1000), Some((500, 999)));
        assert_eq!(parse_byte_range("bytes=-1000", 1000), Some((0, 999)));
        assert_eq!(parse_byte_range("bytes=-1", 1000), Some((999, 999)));
        assert_eq!(parse_byte_range("bytes=-0", 1000), None);
        assert_eq!(parse_byte_range("bytes=1000-", 1000), None);
        assert_eq!(parse_byte_range("bytes=", 1000), None);
        assert_eq!(parse_byte_range("unit=0-1", 1000), None);
        assert_eq!(parse_byte_range("", 1000), None);
        assert_eq!(parse_byte_range("bytes=0-0", 0), None);
    }

    #[test]
    fn normalize_path_trims_and_rejects_empty() {
        assert_eq!(normalize_path("  a\\b  ").unwrap(), "a/b");
        assert!(normalize_path("   ").is_err());
    }

    #[test]
    fn status_from_prep_prefers_job_phase() {
        let mut prep = EpisodePrep::default();
        prep.status = PrepStatus::Processing;
        prep.video = true;
        prep.subs_en = true;
        prep.subs_fr = false;
        prep.en_source = SubTrackSource::Native;
        prep.fr_source = SubTrackSource::Missing;
        prep.message = Some("Whisper…".into());
        let body = status_from_prep("/m.mkv", &prep);
        assert_eq!(body.video.status, "processing");
        assert_eq!(body.subtitles.status, "partial");
        assert_eq!(body.subtitles.en.status, "ready");
        assert_eq!(body.subtitles.fr.status, "idle");

        prep.status = PrepStatus::Error;
        prep.video = false;
        prep.message = None;
        let err = status_from_prep("/m.mkv", &prep);
        assert_eq!(err.video.status, "error");
        assert!(err
            .video
            .message
            .as_deref()
            .unwrap_or("")
            .contains("illisible"));
    }
}
