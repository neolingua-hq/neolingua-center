//! Whisper weights live in app data, not the signed bundle.
//! Downloaded once on first transcription that needs them; product updates do not replace the file.

use reqwest::header::{RANGE, USER_AGENT};
use reqwest::StatusCode;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Same sentinel as prep cancel (`cache::CANCELLED_MSG`).
pub const DOWNLOAD_CANCELLED: &str = "__cancelled__";

pub const MODEL_NAME: &str = "ggml-small.bin";
pub const MODEL_EXPECTED_BYTES: u64 = 487_601_967;
pub const PRIMARY_MODEL_URL: &str = "https://download.neolingua.app/whisper/ggml-small.bin";
pub const FALLBACK_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";

const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub name: String,
    pub expected_bytes: u64,
    pub url: String,
    pub fallback_url: String,
}

impl ModelSpec {
    pub fn production() -> Self {
        Self {
            name: MODEL_NAME.into(),
            expected_bytes: MODEL_EXPECTED_BYTES,
            url: PRIMARY_MODEL_URL.into(),
            fallback_url: FALLBACK_MODEL_URL.into(),
        }
    }
}

pub fn model_path(app_data_dir: &Path, spec: &ModelSpec) -> PathBuf {
    app_data_dir.join("whisper").join(&spec.name)
}

pub fn is_complete_model(path: &Path, expected_bytes: u64) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.len() == expected_bytes)
        .unwrap_or(false)
}

fn leftover_dev_model(spec: &ModelSpec) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        let leftover = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("whisper")
            .join(&spec.name);
        if is_complete_model(&leftover, spec.expected_bytes) {
            return Some(leftover);
        }
    }
    let _ = spec;
    None
}

/// Return a usable model path, downloading into app data when needed.
pub fn ensure_model<C, P>(
    app_data_dir: &Path,
    spec: &ModelSpec,
    is_cancelled: C,
    mut on_progress: P,
) -> Result<PathBuf, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64),
{
    let dest = model_path(app_data_dir, spec);
    if is_complete_model(&dest, spec.expected_bytes) {
        return Ok(dest);
    }
    if let Some(leftover) = leftover_dev_model(spec) {
        return Ok(leftover);
    }

    fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("Impossible de préparer le dossier du modèle de sous-titres : {e}"))?;

    let mut last_err = String::new();
    for url in [&spec.url, &spec.fallback_url] {
        if url.is_empty() {
            continue;
        }
        if is_cancelled() {
            return Err(DOWNLOAD_CANCELLED.into());
        }
        match download_file(
            url,
            &dest,
            spec.expected_bytes,
            &is_cancelled,
            &mut on_progress,
        ) {
            Ok(()) => return Ok(dest),
            Err(err) if err == DOWNLOAD_CANCELLED => return Err(err),
            Err(err) => last_err = err,
        }
    }
    Err(if last_err.is_empty() {
        "Impossible de télécharger le modèle de sous-titres. Vérifie la connexion internet.".into()
    } else {
        last_err
    })
}

fn partial_path(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.partial", dest.display()))
}

fn download_file<C, P>(
    url: &str,
    dest: &Path,
    expected_bytes: u64,
    is_cancelled: &C,
    on_progress: &mut P,
) -> Result<(), String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64),
{
    if is_complete_model(dest, expected_bytes) {
        return Ok(());
    }
    if dest.is_file() {
        let _ = fs::remove_file(dest);
    }

    let partial = partial_path(dest);
    let mut have = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if have > expected_bytes {
        let _ = fs::remove_file(&partial);
        have = 0;
    }
    if have == expected_bytes {
        finalize_partial(&partial, dest, expected_bytes)?;
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent(concat!("NeolinguaCenter/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Téléchargement impossible : {e}"))?;

    let mut request = client.get(url).header(
        USER_AGENT,
        concat!("NeolinguaCenter/", env!("CARGO_PKG_VERSION")),
    );
    if have > 0 {
        request = request.header(RANGE, format!("bytes={have}-"));
    }

    let mut response = request.send().map_err(|_| {
        "Impossible de télécharger le modèle de sous-titres. Vérifie la connexion internet."
            .to_string()
    })?;

    let status = response.status();
    if have > 0 && status == StatusCode::OK {
        have = 0;
        let _ = fs::remove_file(&partial);
    }
    if have > 0 && status == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = fs::remove_file(&partial);
        return download_file(url, dest, expected_bytes, is_cancelled, on_progress);
    }
    if !(status.is_success() || status == StatusCode::PARTIAL_CONTENT) {
        return Err(
            "Impossible de télécharger le modèle de sous-titres. Vérifie la connexion internet."
                .into(),
        );
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(have > 0)
        .write(true)
        .truncate(have == 0)
        .open(&partial)
        .map_err(disk_err)?;

    let mut buf = vec![0u8; CHUNK];
    let mut downloaded = have;
    on_progress(downloaded, expected_bytes);

    loop {
        if is_cancelled() {
            return Err(DOWNLOAD_CANCELLED.into());
        }
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("Téléchargement interrompu ({e}). Réessaie la préparation."))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(disk_err)?;
        downloaded += n as u64;
        if downloaded > expected_bytes {
            let _ = fs::remove_file(&partial);
            return Err("Le fichier du modèle de sous-titres est invalide. Réessaie.".into());
        }
        on_progress(downloaded, expected_bytes);
    }
    file.flush().map_err(disk_err)?;
    drop(file);

    finalize_partial(&partial, dest, expected_bytes)
}

fn finalize_partial(partial: &Path, dest: &Path, expected_bytes: u64) -> Result<(), String> {
    if !is_complete_model(partial, expected_bytes) {
        let got = fs::metadata(partial).map(|m| m.len()).unwrap_or(0);
        let _ = fs::remove_file(partial);
        return Err(format!(
            "Le fichier du modèle est incomplet ({got} octets). Réessaie la préparation."
        ));
    }
    if dest.is_file() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(partial, dest).map_err(disk_err)?;
    Ok(())
}

fn disk_err(err: std::io::Error) -> String {
    match err.raw_os_error() {
        Some(28) | Some(112) => {
            "Pas assez d'espace disque pour le modèle de sous-titres (environ 500 Mo).".into()
        }
        _ => format!("Impossible d'écrire le modèle de sous-titres : {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nl-whisper-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spawn_file_server(body: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                let start = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .and_then(|rest| {
                                rest.trim()
                                    .split('-')
                                    .next()
                                    .and_then(|s| s.parse::<usize>().ok())
                            })
                    })
                    .unwrap_or(0)
                    .min(body.len());
                let slice = &body[start..];
                let status = if start > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\n",
                        body.len().saturating_sub(1),
                        body.len()
                    )
                } else {
                    "HTTP/1.1 200 OK\r\n".into()
                };
                let header = format!(
                    "{status}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    slice.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(slice);
            }
        });
        (format!("http://{addr}/ggml-small.bin"), handle)
    }

    #[test]
    fn complete_local_file_is_reused() {
        let dir = temp_dir();
        let spec = ModelSpec {
            name: "tiny.bin".into(),
            expected_bytes: 8,
            url: "http://127.0.0.1:1/missing".into(),
            fallback_url: String::new(),
        };
        let dest = model_path(&dir, &spec);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"12345678").unwrap();
        let got = ensure_model(&dir, &spec, || false, |_, _| {}).unwrap();
        assert_eq!(got, dest);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloads_when_missing() {
        let payload = vec![7u8; 50_000];
        let (url, server) = spawn_file_server(payload.clone());
        let dir = temp_dir();
        let spec = ModelSpec {
            name: "tiny.bin".into(),
            expected_bytes: payload.len() as u64,
            url,
            fallback_url: String::new(),
        };
        let mut last = (0u64, 0u64);
        let got = ensure_model(&dir, &spec, || false, |a, b| last = (a, b)).unwrap();
        assert_eq!(fs::read(&got).unwrap(), payload);
        assert_eq!(last.1, payload.len() as u64);
        let _ = server.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resumes_partial_download() {
        let payload: Vec<u8> = (0..20_000).map(|i| (i % 251) as u8).collect();
        let dir = temp_dir();
        let spec = ModelSpec {
            name: "tiny.bin".into(),
            expected_bytes: payload.len() as u64,
            url: String::new(),
            fallback_url: String::new(),
        };
        let dest = model_path(&dir, &spec);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let partial = PathBuf::from(format!("{}.partial", dest.display()));
        fs::write(&partial, &payload[..4_000]).unwrap();

        let (url, server) = spawn_file_server(payload.clone());
        let spec = ModelSpec { url, ..spec };
        let got = ensure_model(&dir, &spec, || false, |_, _| {}).unwrap();
        assert_eq!(fs::read(&got).unwrap(), payload);
        let _ = server.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_download() {
        let payload = vec![1u8; 80_000];
        let (url, server) = spawn_file_server(payload.clone());
        let dir = temp_dir();
        let spec = ModelSpec {
            name: "tiny.bin".into(),
            expected_bytes: payload.len() as u64,
            url,
            fallback_url: String::new(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let result = ensure_model(
            &dir,
            &spec,
            || stop.load(Ordering::SeqCst),
            |downloaded, _| {
                if downloaded > 8_000 {
                    flag.store(true, Ordering::SeqCst);
                }
            },
        );
        assert_eq!(result.unwrap_err(), DOWNLOAD_CANCELLED);
        let _ = server.join();
        let _ = fs::remove_dir_all(&dir);
    }
}
