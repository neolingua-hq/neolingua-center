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
                match db::open_migrated(&db_path)
                    .and_then(|conn| db::load_settings(&conn).map_err(crate::error::AppError::from))
                {
                    Ok(settings) => settings.library_check_minutes,
                    Err(err) => {
                        eprintln!("neolingua-center: watcher settings read failed: {err}");
                        60
                    }
                }
            };

            if minutes == 0 {
                thread::sleep(Duration::from_secs(60));
                continue;
            }

            match library::sync_catalog_at(&db_path, false) {
                Ok((_, true)) => {
                    let _ = app.emit("library-updated", ());
                }
                Ok((_, false)) => {}
                Err(err) => {
                    eprintln!("neolingua-center: library sync failed: {err}");
                }
            }

            let secs = u64::from(minutes.max(1)) * 60;
            thread::sleep(Duration::from_secs(secs));
        }
    });
}
