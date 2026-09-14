//! Subtitle handling: extraction, conversion, and Whisper transcription.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use super::ffmpeg::{
    command_hidden, find_media_tool, notify_progress, probe_audio_index_for_lang,
    probe_audio_index_matching_lang, probe_subtitle_index, run_ffmpeg, PrepTrackFlags,
    CANCELLED_MSG,
};
use super::registry::PrepRegistry;
use super::types::{
    inspect_disk, load_sources_file, sub_cache_path, write_sub_source, EpisodePrep,
    SubTrackSource,
};

fn whisper_cli_bin() -> Result<&'static Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match PATH.get_or_init(|| find_media_tool("whisper-cli")) {
        Ok(path) => Ok(path.as_path()),
        Err(err) => Err(err.clone()),
    }
}

pub(super) fn find_sidecar_sub(source: &Path, lang: &str) -> Option<PathBuf> {
    let parent = source.parent()?;
    let file_stem = source.file_stem()?.to_string_lossy();
    let candidates = [
        format!("{}.{}.vtt", file_stem, lang),
        format!("{}.{}.srt", file_stem, lang),
        format!("{}.vtt", file_stem),
        format!("{}.srt", file_stem),
    ];
    for name in candidates {
        let path = parent.join(&name);
        if path.is_file() {
            if lang == "fr"
                && (name.ends_with(".vtt") || name.ends_with(".srt"))
                && name.contains(".en.")
            {
                continue;
            }
            if lang == "en" && name.contains(".fr.") {
                continue;
            }
            if (name.ends_with(".srt") || name.ends_with(".vtt"))
                && !name.contains(".en.")
                && !name.contains(".fr.")
                && lang != "en"
            {
                continue;
            }
            return Some(path);
        }
    }
    None
}

pub(super) fn convert_sub_to_vtt(source: &Path, out: &Path) -> Result<(), String> {
    if source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("vtt"))
        .unwrap_or(false)
    {
        fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
        fs::copy(source, out).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let tmp = out.with_extension("partial.vtt");
    run_ffmpeg(&[
        "-y",
        "-i",
        &source.to_string_lossy(),
        "-f",
        "webvtt",
        &tmp.to_string_lossy(),
    ])?;
    validate_vtt(&tmp)?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn extract_embedded_sub(source: &Path, out: &Path, stream_index: usize) -> Result<(), String> {
    let tmp = out.with_extension("partial.vtt");
    run_ffmpeg(&[
        "-y",
        "-i",
        &source.to_string_lossy(),
        "-map",
        &format!("0:{stream_index}"),
        "-f",
        "webvtt",
        &tmp.to_string_lossy(),
    ])?;
    validate_vtt(&tmp)?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn validate_vtt(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if !content.contains("-->") {
        let _ = fs::remove_file(path);
        return Err("Piste de sous-titres vide".into());
    }
    Ok(())
}

fn ensure_whisper_model<F>(
    registry: &PrepRegistry,
    media_path: &str,
    on_update: &mut F,
) -> Result<PathBuf, String>
where
    F: FnMut(&EpisodePrep),
{
    let spec = crate::whisper_model::ModelSpec::production();
    let dest = crate::whisper_model::model_path(&registry.app_data_dir, &spec);
    if crate::whisper_model::is_complete_model(&dest, spec.expected_bytes) {
        return Ok(dest);
    }

    let disk = inspect_disk(&registry.cache_dir, media_path);
    let mut last_pct: i16 = -1;
    crate::whisper_model::ensure_model(
        &registry.app_data_dir,
        &spec,
        || registry.is_cancelled(media_path),
        |downloaded, total| {
            let pct = if total > 0 {
                ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 99.0) as u8
            } else {
                0
            };
            if i16::from(pct) < last_pct + 2 && downloaded < total {
                return;
            }
            last_pct = i16::from(pct);
            let overall = 78 + (u16::from(pct) * 8 / 100) as u8;
            let downloaded_mb = downloaded / 1_000_000;
            let total_mb = total / 1_000_000;
            let message = if total > 0 {
                format!("Téléchargement du modèle de sous-titres… {downloaded_mb} / {total_mb} Mo")
            } else {
                "Téléchargement du modèle de sous-titres…".into()
            };
            notify_progress(
                registry,
                media_path,
                overall,
                &message,
                PrepTrackFlags {
                    video: true,
                    en_source: disk.en_source,
                    fr_source: disk.fr_source,
                },
                on_update,
            );
        },
    )
}

pub(super) fn transcribe_with_whisper<F>(
    registry: &PrepRegistry,
    media_path: &str,
    source: &Path,
    out: &Path,
    lang: &str,
    on_update: &mut F,
) -> Result<(), String>
where
    F: FnMut(&EpisodePrep),
{
    registry.ensure_not_cancelled(media_path)?;
    let cli = whisper_cli_bin()?;
    let model = ensure_whisper_model(registry, media_path, on_update)?;
    registry.ensure_not_cancelled(media_path)?;
    let disk = inspect_disk(&registry.cache_dir, media_path);
    notify_progress(
        registry,
        media_path,
        88,
        "Transcription des sous-titres…",
        PrepTrackFlags {
            video: true,
            en_source: disk.en_source,
            fr_source: disk.fr_source,
        },
        on_update,
    );
    fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;

    let audio_index = probe_audio_index_for_lang(source, lang)?;
    let wav = out.with_extension("whisper.wav");
    let vtt_stem = out.with_extension("whisper");
    let vtt_out = PathBuf::from(format!("{}.vtt", vtt_stem.to_string_lossy()));
    let _ = fs::remove_file(&wav);
    let _ = fs::remove_file(&vtt_out);

    run_ffmpeg(&[
        "-y",
        "-i",
        &source.to_string_lossy(),
        "-map",
        &format!("0:{audio_index}"),
        "-ac",
        "1",
        "-ar",
        "16000",
        &wav.to_string_lossy(),
    ])?;
    registry.ensure_not_cancelled(media_path)?;

    let mut child = command_hidden(cli)
        .args([
            "-m",
            &model.to_string_lossy(),
            "-l",
            lang,
            "-f",
            &wav.to_string_lossy(),
            "-ovtt",
            "-of",
            &vtt_stem.to_string_lossy(),
            "-np",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Impossible de lancer Whisper : {e}"))?;
    registry.register_pid(media_path, child.id());

    let status = loop {
        if registry.is_cancelled(media_path) {
            let _ = child.kill();
            let _ = child.wait();
            registry.clear_pid(media_path);
            let _ = fs::remove_file(&wav);
            let _ = fs::remove_file(&vtt_out);
            return Err(CANCELLED_MSG.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => {
                registry.clear_pid(media_path);
                let _ = fs::remove_file(&wav);
                return Err(e.to_string());
            }
        }
    };
    registry.clear_pid(media_path);

    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut stderr);
    }
    let _ = fs::remove_file(&wav);
    if !status.success() {
        let tip = stderr
            .trim()
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fs::remove_file(&vtt_out);
        return Err(if tip.is_empty() {
            format!("Whisper a échoué (code {:?}).", status.code())
        } else {
            tip
        });
    }
    if !vtt_out.is_file() {
        return Err("Whisper n'a pas produit de fichier VTT.".into());
    }
    validate_vtt(&vtt_out)?;
    fs::rename(&vtt_out, out).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn ensure_subtitle<F>(
    registry: &PrepRegistry,
    media_path: &str,
    source: &Path,
    lang: &str,
    original_track: Option<&str>,
    original_language_raw: Option<&str>,
    on_update: &mut F,
) -> Result<(), String>
where
    F: FnMut(&EpisodePrep),
{
    let out = sub_cache_path(&registry.cache_dir, media_path, lang);
    if out.is_file() {
        let recorded = load_sources_file(&registry.cache_dir, media_path);
        let existing = if lang == "fr" {
            recorded.fr.as_deref()
        } else {
            recorded.en.as_deref()
        };
        if existing.is_none() {
            write_sub_source(&registry.cache_dir, media_path, lang, SubTrackSource::Native);
        }
        return Ok(());
    }

    if let Some(sidecar) = find_sidecar_sub(source, lang) {
        convert_sub_to_vtt(&sidecar, &out)?;
        write_sub_source(&registry.cache_dir, media_path, lang, SubTrackSource::Native);
        return Ok(());
    }

    if let Some(index) = probe_subtitle_index(source, lang)? {
        extract_embedded_sub(source, &out, index)?;
        write_sub_source(&registry.cache_dir, media_path, lang, SubTrackSource::Native);
        return Ok(());
    }

    let whisper_secondary =
        original_track != Some(lang) && probe_audio_index_matching_lang(source, lang)?.is_some();
    if original_track == Some(lang) || whisper_secondary {
        let disk = inspect_disk(&registry.cache_dir, media_path);
        notify_progress(
            registry,
            media_path,
            82,
            if whisper_secondary {
                "Transcription (piste audio)…"
            } else {
                "Transcription des sous-titres…"
            },
            PrepTrackFlags {
                video: true,
                en_source: disk.en_source,
                fr_source: disk.fr_source,
            },
            on_update,
        );
        registry.ensure_not_cancelled(media_path)?;
        transcribe_with_whisper(registry, media_path, source, &out, lang, on_update)?;
        write_sub_source(&registry.cache_dir, media_path, lang, SubTrackSource::Generated);
        return Ok(());
    }

    let _ = original_language_raw;
    Ok(())
}
