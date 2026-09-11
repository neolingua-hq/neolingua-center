//! Periodic media-folder checks (new files).

use crate::db;
use crate::library;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub fn spawn(app: AppHandle, db_path: PathBuf) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(20));
        loop {
            let minutes = {
                match db::open_connection(&db_path).and_then(|conn| {
                    db::run_migrations(&conn)?;
                    db::load_settings(&conn)
                }) {
                    Ok(settings) => settings.library_check_minutes,
                    Err(_) => 60,
                }
            };

            if minutes == 0 {
                thread::sleep(Duration::from_secs(60));
                continue;
            }

            let changed = library::sync_catalog_at(&db_path, false)
                .map(|(_, changed)| changed)
                .unwrap_or(false);

            if changed {
                let _ = app.emit("library-updated", ());
            }

            let secs = u64::from(minutes.max(1)) * 60;
            thread::sleep(Duration::from_secs(secs));
        }
    });
}
