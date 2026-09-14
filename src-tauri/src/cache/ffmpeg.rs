//! ffmpeg/ffprobe binary resolution and media processing utilities.

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use super::registry::PrepRegistry;
use super::types::{EpisodePrep, PrepStatus, SubTrackSource};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(super) fn command_hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

pub(super) const CANCELLED_MSG: &str = "__cancelled__";

fn host_target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    {
        "unknown"
    }
}

fn sidecar_file_name(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.exe")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

fn sidecar_triple_name(name: &str) -> String {
    let base = format!("{name}-{}", host_target_triple());
    #[cfg(windows)]
    {
        format!("{base}.exe")
    }
    #[cfg(not(windows))]
    {
        base
    }
}

/// Resolve ffmpeg/ffprobe from the app bundle (Tauri externalBin), then the
/// crate `binaries/` folder used in development.
pub(super) fn find_media_tool(name: &str) -> Result<PathBuf, String> {
    let plain = sidecar_file_name(name);
    let triple = sidecar_triple_name(name);
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(&plain));
            candidates.push(dir.join(&triple));
        }
    }

    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&triple),
    );

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }

    #[cfg(debug_assertions)]
    {
        if let Ok(path_env) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_env) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err(format!(
        "Outil manquant dans l'application ({name}). Réinstallez Neolingua Center."
    ))
}

pub(super) fn ffmpeg_bin() -> Result<&'static Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match PATH.get_or_init(|| find_media_tool("ffmpeg")) {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

pub(super) fn ffprobe_bin() -> Result<&'static Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match PATH.get_or_init(|| find_media_tool("ffprobe")) {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

pub(super) fn run_ffmpeg(args: &[&str]) -> Result<(), String> {
    let output = command_hidden(ffmpeg_bin()?)
        .args(args)
        .output()
        .map_err(|e| format!("Impossible de lancer la conversion : {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tip = stderr
        .lines()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Err(if tip.is_empty() {
        format!("Conversion impossible (code {})", output.status)
    } else {
        tip
    })
}

pub(super) fn probe_streams(source: &Path) -> Result<Vec<Value>, String> {
    let output = command_hidden(ffprobe_bin()?)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            &source.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("Impossible d'analyser le fichier : {e}"))?;
    if !output.status.success() {
        let tip = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if tip.is_empty() {
            "Fichier illisible ou corrompu.".into()
        } else {
            tip
        });
    }
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Analyse média invalide : {e}"))?;
    Ok(parsed
        .get("streams")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default())
}

pub(super) fn stream_lang(stream: &Value) -> String {
    stream
        .get("tags")
        .and_then(|t| t.get("language"))
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(super) fn probe_english_audio_index(source: &Path) -> Result<usize, String> {
    let streams = probe_streams(source)?;
    let audio: Vec<&Value> = streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"))
        .collect();
    let eng = audio.iter().find(|s| {
        let lang = stream_lang(s);
        lang.starts_with("eng") || lang == "en"
    });
    let chosen = eng.or(audio.first()).ok_or_else(|| {
        "Aucune piste audio (fichier illisible, corrompu, ou sans son).".to_string()
    })?;
    chosen
        .get("index")
        .and_then(|i| i.as_u64())
        .map(|i| i as usize)
        .ok_or_else(|| "Index audio invalide".into())
}

pub(super) fn probe_duration_seconds(source: &Path) -> Option<f64> {
    let output = command_hidden(ffprobe_bin().ok()?)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_entries",
            "format=duration",
            &source.to_string_lossy(),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: Value = serde_json::from_slice(&output.stdout).ok()?;
    let duration = parsed
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())?;
    if duration.is_finite() && duration > 0.0 {
        Some(duration)
    } else {
        None
    }
}

pub(super) fn probe_subtitle_index(source: &Path, lang: &str) -> Result<Option<usize>, String> {
    let prefixes: &[&str] = if lang == "fr" {
        &["fre", "fra", "fr"]
    } else {
        &["eng", "en"]
    };
    let streams = probe_streams(source)?;
    for stream in streams {
        if stream.get("codec_type").and_then(|c| c.as_str()) != Some("subtitle") {
            continue;
        }
        let tag = stream_lang(&stream);
        if prefixes.iter().any(|p| tag == *p || tag.starts_with(p)) {
            if let Some(index) = stream.get("index").and_then(|i| i.as_u64()) {
                return Ok(Some(index as usize));
            }
        }
    }
    Ok(None)
}

pub(super) fn audio_lang_prefixes(lang: &str) -> &'static [&'static str] {
    if lang == "fr" {
        &["fre", "fra", "fr"]
    } else {
        &["eng", "en"]
    }
}

pub(super) fn probe_audio_index_matching_lang(source: &Path, lang: &str) -> Result<Option<usize>, String> {
    let prefixes = audio_lang_prefixes(lang);
    let streams = probe_streams(source)?;
    for stream in streams {
        if stream.get("codec_type").and_then(|c| c.as_str()) != Some("audio") {
            continue;
        }
        let tag = stream_lang(&stream);
        if prefixes.iter().any(|p| tag == *p || tag.starts_with(p)) {
            if let Some(index) = stream.get("index").and_then(|i| i.as_u64()) {
                return Ok(Some(index as usize));
            }
        }
    }
    Ok(None)
}

pub(super) fn probe_audio_index_for_lang(source: &Path, lang: &str) -> Result<usize, String> {
    if let Some(index) = probe_audio_index_matching_lang(source, lang)? {
        return Ok(index);
    }
    let streams = probe_streams(source)?;
    let audio: Vec<&Value> = streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"))
        .collect();
    let chosen = audio.first().ok_or_else(|| {
        "Aucune piste audio (fichier illisible, corrompu, ou sans son).".to_string()
    })?;
    chosen
        .get("index")
        .and_then(|i| i.as_u64())
        .map(|i| i as usize)
        .ok_or_else(|| "Index audio invalide".into())
}

pub(super) struct PrepTrackFlags {
    pub video: bool,
    pub en_source: SubTrackSource,
    pub fr_source: SubTrackSource,
}

pub(super) fn notify_progress<F>(
    registry: &PrepRegistry,
    media_path: &str,
    progress: u8,
    message: &str,
    flags: PrepTrackFlags,
    on_update: &mut F,
) where
    F: FnMut(&EpisodePrep),
{
    let state = EpisodePrep {
        status: PrepStatus::Processing,
        video: flags.video,
        subs_en: flags.en_source != SubTrackSource::Missing,
        subs_fr: flags.fr_source != SubTrackSource::Missing,
        en_source: flags.en_source,
        fr_source: flags.fr_source,
        progress: Some(progress.min(99)),
        message: Some(message.into()),
    };
    registry.set_job(media_path, state.clone());
    on_update(&state);
}

pub(super) fn remux_video<F>(
    registry: &PrepRegistry,
    media_path: &str,
    source: &Path,
    out: &Path,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u8),
{
    fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let audio_index = probe_english_audio_index(source)?;
    let duration_sec = probe_duration_seconds(source);
    let tmp = out.with_extension("partial.mp4");
    let _ = fs::remove_file(&tmp);
    let tmp_str = tmp.to_string_lossy().into_owned();
    let source_str = source.to_string_lossy().into_owned();
    let audio_map = format!("0:{audio_index}");

    let mut child = command_hidden(ffmpeg_bin()?)
        .args([
            "-y",
            "-i",
            &source_str,
            "-map",
            "0:v:0",
            "-map",
            &audio_map,
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-b:a",
            "160k",
            "-movflags",
            "+faststart",
            "-progress",
            "pipe:1",
            "-nostats",
            &tmp_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Impossible de lancer la conversion : {e}"))?;

    registry.register_pid(media_path, child.id());

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg stdout manquant".to_string())?;
    let mut last_pct: i16 = -1;
    let mut cancelled = false;
    for line in BufReader::new(stdout).lines() {
        if registry.is_cancelled(media_path) {
            cancelled = true;
            let _ = child.kill();
            break;
        }
        let line = match line {
            Ok(line) => line,
            Err(_) if registry.is_cancelled(media_path) => {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            Err(e) => {
                registry.clear_pid(media_path);
                return Err(e.to_string());
            }
        };
        if let Some(raw) = line.strip_prefix("out_time_ms=") {
            let Ok(out_time_ms) = raw.parse::<f64>() else {
                continue;
            };
            let pct = if let Some(duration) = duration_sec.filter(|d| *d > 0.0) {
                ((out_time_ms / 1_000_000.0 / duration) * 100.0)
                    .clamp(0.0, 99.0)
                    .round() as u8
            } else if last_pct < 5 {
                5
            } else {
                continue;
            };
            if i16::from(pct) >= last_pct + 2 {
                last_pct = i16::from(pct);
                on_progress(pct);
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    registry.clear_pid(media_path);
    if cancelled || registry.is_cancelled(media_path) {
        let _ = fs::remove_file(&tmp);
        return Err(CANCELLED_MSG.into());
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut stderr);
    }
    if !status.success() {
        let tip = stderr
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fs::remove_file(&tmp);
        return Err(if tip.is_empty() {
            format!("Conversion vidéo impossible (code {status})")
        } else {
            format!("Conversion vidéo impossible : {tip}")
        });
    }
    on_progress(100);
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn reject_unreadable_source(source: &Path) -> Result<(), String> {
    use std::io::Read;
    let meta = fs::metadata(source).map_err(|e| format!("Impossible de lire le fichier : {e}"))?;
    if meta.len() == 0 {
        return Err("Fichier source vide.".into());
    }
    let mut file =
        fs::File::open(source).map_err(|e| format!("Impossible d'ouvrir le fichier : {e}"))?;
    let mut buf = [0u8; 64];
    let n = file
        .read(&mut buf)
        .map_err(|e| format!("Impossible de lire le fichier : {e}"))?;
    if n == 0 || buf[..n].iter().all(|&b| b == 0) {
        return Err(
            "Fichier source corrompu ou incomplet (données vides). Remplacez le fichier média."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_lang_reads_tags() {
        let stream = serde_json::json!({
            "tags": { "language": "eng" }
        });
        assert_eq!(stream_lang(&stream), "eng");
        assert_eq!(stream_lang(&serde_json::json!({})), "");
    }
}
