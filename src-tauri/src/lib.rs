mod cache;
mod db;
mod jam;
mod library;
mod network;
mod parse;
mod presence;
mod scan;
mod server;
mod tmdb;
mod watcher;

use cache::{EpisodePrep, PrepRegistry};
use db::{with_connection, AppSettings, DbState, MediaRoot};
use network::NetworkInfo;
use presence::PresenceSnapshot;
use scan::CatalogSnapshot;
use server::{ServerController, ServerStatus};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
fn get_app_settings(state: State<'_, Mutex<DbState>>) -> Result<AppSettings, String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    with_connection(&db, db::load_settings)
}

#[tauri::command]
fn save_app_settings(
    settings: AppSettings,
    db_state: State<'_, Mutex<DbState>>,
    server: State<'_, Arc<ServerController>>,
    prep: State<'_, Arc<PrepRegistry>>,
) -> Result<(), String> {
    let db = db_state.lock().map_err(|e| e.to_string())?;
    with_connection(&db, |conn| db::save_settings(conn, &settings))?;
    drop(db);
    prep.enforce_limit(db::cache_max_bytes(settings.cache_max_gb), None);
    let port = clamp_port(settings.server_port);
    ServerController::ensure_running(&server, port);
    Ok(())
}

#[tauri::command]
fn list_media_roots(state: State<'_, Mutex<DbState>>) -> Result<Vec<MediaRoot>, String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    with_connection(&db, db::list_media_roots)
}

#[tauri::command]
fn add_media_root(path: String, state: State<'_, Mutex<DbState>>) -> Result<MediaRoot, String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    with_connection(&db, |conn| db::add_media_root(conn, &path))
}

#[tauri::command]
fn remove_media_root(id: i64, state: State<'_, Mutex<DbState>>) -> Result<(), String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    with_connection(&db, |conn| db::remove_media_root(conn, id))
}

#[tauri::command]
fn get_network_info() -> NetworkInfo {
    network::get_network_info()
}

#[tauri::command]
fn get_presence(server: State<'_, Arc<ServerController>>) -> PresenceSnapshot {
    server.presence_snapshot()
}

#[tauri::command]
fn get_server_status(server: State<'_, Arc<ServerController>>) -> ServerStatus {
    server.status()
}

#[tauri::command]
fn stop_lan_server(server: State<'_, Arc<ServerController>>) {
    server.stop();
}

#[tauri::command]
fn get_catalog(
    state: State<'_, Mutex<DbState>>,
    prep: State<'_, Arc<PrepRegistry>>,
) -> Result<CatalogSnapshot, String> {
    let db = state.lock().map_err(|e| e.to_string())?;
    let mut snap = with_connection(&db, db::load_catalog)?;
    enrich_prep(&mut snap, &prep);
    Ok(snap)
}

#[tauri::command]
async fn scan_catalog(
    app: AppHandle,
    state: State<'_, Mutex<DbState>>,
    prep: State<'_, Arc<PrepRegistry>>,
) -> Result<CatalogSnapshot, String> {
    let path = {
        let db = state.lock().map_err(|e| e.to_string())?;
        db.path.clone()
    };
    let mut snap = tauri::async_runtime::spawn_blocking(move || {
        library::sync_catalog_at(&path, true).map(|(snap, _)| snap)
    })
    .await
    .map_err(|e| e.to_string())??;
    enrich_prep(&mut snap, &prep);
    let _ = app.emit("library-updated", ());
    Ok(snap)
}

#[tauri::command]
async fn check_library(
    app: AppHandle,
    state: State<'_, Mutex<DbState>>,
    prep: State<'_, Arc<PrepRegistry>>,
) -> Result<CatalogSnapshot, String> {
    let path = {
        let db = state.lock().map_err(|e| e.to_string())?;
        db.path.clone()
    };
    let (mut snap, changed) = tauri::async_runtime::spawn_blocking(move || {
        library::sync_catalog_at(&path, false)
    })
    .await
    .map_err(|e| e.to_string())??;
    enrich_prep(&mut snap, &prep);
    if changed {
        let _ = app.emit("library-updated", ());
    }
    Ok(snap)
}

#[tauri::command]
async fn prepare_episode(
    path: String,
    prep: State<'_, Arc<PrepRegistry>>,
    db_state: State<'_, Mutex<DbState>>,
    app: AppHandle,
) -> Result<EpisodePrep, String> {
    let db_path = {
        let db = db_state.lock().map_err(|e| e.to_string())?;
        db.path.clone()
    };
    let registry = Arc::clone(&prep);
    let media_path = path.clone();
    let app_progress = app.clone();
    let path_progress = path.clone();
    let state = tauri::async_runtime::spawn_blocking(move || {
        registry.prepare_from_db(&db_path, &media_path, |prep_state| {
            let _ = app_progress.emit(
                "episode-prep-updated",
                serde_json::json!({ "path": path_progress, "prep": prep_state }),
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(state)
}

#[tauri::command]
async fn prepare_episodes(
    paths: Vec<String>,
    prep: State<'_, Arc<PrepRegistry>>,
    db_state: State<'_, Mutex<DbState>>,
    app: AppHandle,
) -> Result<(), String> {
    let db_path = {
        let db = db_state.lock().map_err(|e| e.to_string())?;
        db.path.clone()
    };
    prep.enqueue(&paths);
    for path in paths {
        let registry = Arc::clone(&prep);
        let media_path = path.clone();
        let db_path = db_path.clone();
        let app_progress = app.clone();
        let path_progress = path.clone();
        let _state = tauri::async_runtime::spawn_blocking(move || {
            registry.prepare_from_db(&db_path, &media_path, |prep_state| {
                let _ = app_progress.emit(
                    "episode-prep-updated",
                    serde_json::json!({ "path": path_progress, "prep": prep_state }),
                );
            })
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn cancel_prepare(
    path: String,
    prep: State<'_, Arc<PrepRegistry>>,
    app: AppHandle,
) -> Result<EpisodePrep, String> {
    let state = prep.cancel_prepare(&path);
    let _ = app.emit(
        "episode-prep-updated",
        serde_json::json!({ "path": path, "prep": state }),
    );
    Ok(state)
}

#[tauri::command]
async fn pick_media_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app
        .dialog()
        .file()
        .set_title("Dossier des vidéos")
        .blocking_pick_folder();

    Ok(picked.map(|p| p.to_string()))
}

/// Write a small HTML launcher and open it so the default browser loads the watch URL.
#[tauri::command]
async fn open_watch_launcher(app: AppHandle, url: String) -> Result<(), String> {
    use std::hash::{Hash, Hasher};

    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("URL de lecture invalide.".into());
    }

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Dossier app introuvable : {e}"))?;
    let dir = app_data.join("cache").join("launchers");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Impossible de créer le lanceur : {e}"))?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    trimmed.hash(&mut hasher);
    let file_name = format!("watch-{:016x}.html", hasher.finish());
    let path = dir.join(file_name);

    let escaped = trimmed
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta http-equiv="refresh" content="0;url={escaped}">
<title>Neolingua</title>
<script>location.replace({json});</script>
</head>
<body>
<p><a href="{escaped}">Ouvrir la lecture Neolingua</a></p>
</body>
</html>
"#,
        escaped = escaped,
        json = serde_json::to_string(trimmed).unwrap_or_else(|_| "\"\"".into()),
    );
    std::fs::write(&path, html).map_err(|e| format!("Impossible d’écrire le lanceur : {e}"))?;

    // Open the launcher file with the OS default handler (browser).
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg(&path)
            .status()
            .map_err(|e| format!("Impossible d’ouvrir le lanceur : {e}"))?;
        if !status.success() {
            return Err("Impossible d’ouvrir le lanceur.".into());
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| format!("Impossible d’ouvrir le lanceur : {e}"))?;
    }
    Ok(())
}

fn enrich_prep(snap: &mut CatalogSnapshot, prep: &PrepRegistry) {
    for series in &mut snap.series {
        for season in &mut series.seasons {
            for episode in &mut season.episodes {
                episode.prep = prep.inspect(&episode.path);
            }
        }
    }
}

fn clamp_port(port: u32) -> u16 {
    u16::try_from(port).unwrap_or(8787).max(1024)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            let path = db::db_path(app.handle())?;
            let conn = db::prepare_database(&path)?;
            let settings = db::load_settings(&conn)?;
            drop(conn);
            app.manage(Mutex::new(DbState { path: path.clone() }));

            let app_data = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone());
            let prep = Arc::new(PrepRegistry::new(app_data));
            app.manage(Arc::clone(&prep));

            let server = Arc::new(ServerController::new(path.clone(), prep));
            ServerController::ensure_running(&server, clamp_port(settings.server_port));
            app.manage(server);

            watcher::spawn(app.handle().clone(), path);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_settings,
            save_app_settings,
            list_media_roots,
            add_media_root,
            remove_media_root,
            pick_media_directory,
            open_watch_launcher,
            get_network_info,
            get_presence,
            get_server_status,
            stop_lan_server,
            get_catalog,
            scan_catalog,
            check_library,
            prepare_episode,
            prepare_episodes,
            cancel_prepare,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(test)]
mod tests {
    use super::clamp_port;

    #[test]
    fn clamp_port_floor_and_overflow() {
        assert_eq!(clamp_port(8787), 8787);
        assert_eq!(clamp_port(80), 1024);
        assert_eq!(clamp_port(1024), 1024);
        assert_eq!(clamp_port(u32::MAX), 8787);
    }
}
