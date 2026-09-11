//! Guessit-style parse: title / season / episode from the file (+ path as fallback).
//! No hardcoded per-series rules.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const VIDEO_EXT: &[&str] = &["mkv", "mp4", "m4v", "avi", "webm", "mov"];

/// Scene / quality tokens to strip from the title (not show names).
const SCENE_TOKENS: &[&str] = &[
    "1080p", "720p", "480p", "2160p", "4k", "uhd", "hdr", "hdr10", "dv", "dolby",
    "web", "webrip", "web-dl", "webdl", "bluray", "blu-ray", "bdrip", "brrip", "hdtv",
    "dvdrip", "hdrip", "remux", "proper", "repack", "internal", "extended", "unrated",
    "x265", "x264", "h264", "h265", "h.264", "h.265", "hevc", "avc", "av1", "10bit", "8bit",
    "aac", "ac3", "dts", "dts-hd", "truehd", "atmos", "flac", "mp3",
    "multi", "vff", "vfq", "vf2", "vfi", "vo", "vovf", "truefrench", "french", "english",
    "subforced", "subs", "sub", "nl", "amu",
    "amzn", "nf", "dsnp", "hmax", "atvp", "hulu", "cr", "itunes",
];

const GENERIC_STEMS: &[&str] = &[
    "video", "movie", "film", "episode", "ep", "sample", "preview", "trailer",
];

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEpisode {
    pub title: String,
    pub normalized_title: String,
    pub season: i32,
    pub episode: i32,
    pub year: Option<i32>,
    /// 0.0 .. 1.0
    pub confidence: f32,
    pub abs_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMovie {
    pub title: String,
    pub normalized_title: String,
    pub year: Option<i32>,
    pub confidence: f32,
    pub abs_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedMedia {
    Episode(ParsedEpisode),
    Movie(ParsedMovie),
}

fn se_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:^|[\s._\-\[\(])S(\d{1,2})E(\d{1,3})(?:$|[\s._\-\]\)])").expect("se"))
}

fn se_loose_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)S(\d{1,2})E(\d{1,3})").expect("se loose"))
}

fn nxnn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 1x02 pattern - avoid years like 2020 by requiring x
    RE.get_or_init(|| Regex::new(r"(?i)(?:^|[\s._\-])(\d{1,2})x(\d{1,3})(?:$|[\s._\-])").expect("nxnn"))
}

fn season_dir_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:season|saison)[\s._-]*(\d{1,2})$|^s(\d{1,2})$").expect("season dir")
    })
}

fn year_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(?:^|[\s._\-(])((?:19|20)\d{2})(?:$|[\s._\-)])").expect("year"))
}

fn release_group_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-[A-Za-z0-9]+$").expect("group"))
}

fn title_with_season_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Foundation S03" / "Breaking Bad S01"
    RE.get_or_init(|| Regex::new(r"(?i)^(.+?)[\s._-]+S(\d{1,2})$").expect("title+S"))
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| VIDEO_EXT.iter().any(|ok| ext.eq_ignore_ascii_case(ok)))
        .unwrap_or(false)
}

pub fn normalize_title(input: &str) -> String {
    let lower = input.trim().to_lowercase();
    let mut out = String::new();
    let mut prev_space = false;
    for ch in lower.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_space = false;
        } else if !prev_space && !out.is_empty() {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

pub fn display_title(input: &str) -> String {
    let collapsed = input.replace(['.', '_', '-'], " ");
    let joined = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        "Sans titre".into()
    } else {
        joined
    }
}

fn is_scene_token(token: &str) -> bool {
    let t = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-');
    if t.is_empty() {
        return true;
    }
    let lower = t.to_lowercase();
    if SCENE_TOKENS.iter().any(|s| *s == lower) {
        return true;
    }
    // Resolutions like 1920x1080
    if Regex::new(r"^\d{3,4}x\d{3,4}$").ok().is_some_and(|re| re.is_match(&lower)) {
        return true;
    }
    false
}

fn is_generic_stem(stem: &str) -> bool {
    let n = normalize_title(stem);
    GENERIC_STEMS.iter().any(|g| *g == n) || n.is_empty()
}

fn strip_scene_noise(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if let Some(m) = release_group_re().find(&s) {
        // Only strip the group if it looks like a tag (after quality / at the end)
        s = s[..m.start()].to_string();
    }
    let parts: Vec<&str> = s
        .split(['.', '_', ' ', '-'])
        .filter(|p| !p.is_empty())
        .collect();
    let mut kept = Vec::new();
    for part in parts {
        if is_scene_token(part) {
            continue;
        }
        if year_re().is_match(part) && !kept.is_empty() {
            // optional year: often stop before it for the TV title
            break;
        }
        if se_loose_re().is_match(part) || nxnn_re().is_match(part) {
            break;
        }
        kept.push(part);
    }
    display_title(&kept.join(" "))
}

fn extract_se(text: &str) -> Option<(i32, i32, usize)> {
    if let Some(caps) = se_re().captures(text) {
        let season: i32 = caps.get(1)?.as_str().parse().ok()?;
        let episode: i32 = caps.get(2)?.as_str().parse().ok()?;
        return Some((season, episode, caps.get(0)?.start()));
    }
    if let Some(caps) = se_loose_re().captures(text) {
        let season: i32 = caps.get(1)?.as_str().parse().ok()?;
        let episode: i32 = caps.get(2)?.as_str().parse().ok()?;
        return Some((season, episode, caps.get(0)?.start()));
    }
    if let Some(caps) = nxnn_re().captures(text) {
        let season: i32 = caps.get(1)?.as_str().parse().ok()?;
        let episode: i32 = caps.get(2)?.as_str().parse().ok()?;
        return Some((season, episode, caps.get(0)?.start()));
    }
    None
}

fn extract_year(text: &str) -> Option<i32> {
    let caps = year_re().captures(text)?;
    caps.get(1)?.as_str().parse().ok()
}

fn parse_season_dir(name: &str) -> Option<i32> {
    let caps = season_dir_re().captures(name)?;
    caps.get(1)
        .or_else(|| caps.get(2))
        .and_then(|m| m.as_str().parse().ok())
}

fn title_and_season_from_folder(name: &str) -> Option<(String, Option<i32>)> {
    if let Some(season) = parse_season_dir(name) {
        return Some((String::new(), Some(season)));
    }
    if let Some(caps) = title_with_season_re().captures(name) {
        let title = strip_scene_noise(caps.get(1)?.as_str());
        let season: i32 = caps.get(2)?.as_str().parse().ok()?;
        if !title.is_empty() {
            return Some((title, Some(season)));
        }
    }
    let cleaned = strip_scene_noise(name);
    if cleaned.is_empty() || is_generic_stem(&cleaned) {
        return None;
    }
    // If the folder itself is an SxxExx release
    if let Some((season, _ep, at)) = extract_se(name) {
        let before = &name[..at];
        let title = strip_scene_noise(before);
        if !title.is_empty() {
            return Some((title, Some(season)));
        }
    }
    Some((cleaned, None))
}

/// Name source to parse: file stem, or parent folder if the stem is generic.
fn naming_candidates(abs: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let stem = abs
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    if !is_generic_stem(&stem) {
        out.push(stem.clone());
    }
    if let Some(parent) = abs.parent() {
        if let Some(name) = parent.file_name().and_then(|n| n.to_str()) {
            out.push(name.to_string());
        }
        if let Some(grand) = parent.parent() {
            if let Some(name) = grand.file_name().and_then(|n| n.to_str()) {
                out.push(name.to_string());
            }
        }
    }
    if is_generic_stem(&stem) {
        out.push(stem);
    }
    out
}

fn path_season_and_show(abs: &Path) -> (Option<String>, Option<i32>) {
    let mut show: Option<String> = None;
    let mut season: Option<i32> = None;
    for ancestor in abs.ancestors().skip(1).take(5) {
        let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(s) = parse_season_dir(name) {
            season = Some(s);
            continue;
        }
        if let Some((title, folder_season)) = title_and_season_from_folder(name) {
            if let Some(s) = folder_season {
                season = season.or(Some(s));
            }
            if !title.is_empty() {
                show = Some(title);
                break;
            }
        }
    }
    (show, season)
}

/// Parse a video file into an episode or movie.
pub fn parse_video(abs: &Path) -> ParsedMedia {
    let abs_path = abs.to_path_buf();
    let (path_show, path_season) = path_season_and_show(abs);

    for candidate in naming_candidates(abs) {
        if let Some((season, episode, at)) = extract_se(&candidate) {
            let before = &candidate[..at];
            let mut title = strip_scene_noise(before);
            if title.is_empty() {
                title = path_show.clone().unwrap_or_default();
            }
            if title.is_empty() {
                continue;
            }
            let year = extract_year(&candidate);
            let confidence = if !before.trim().is_empty() { 0.95 } else { 0.75 };
            return ParsedMedia::Episode(ParsedEpisode {
                normalized_title: normalize_title(&title),
                title: display_title(&title),
                season: path_season.unwrap_or(season),
                episode,
                year,
                confidence,
                abs_path,
            });
        }
    }

    // No SxxExx in the name: folder season + episode from an alternate pattern
    if let (Some(title), Some(season)) = (path_show.clone(), path_season) {
        let stem = abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        // E01 / ep01 / 01 patterns in the stem
        if let Some(ep) = parse_episode_only(stem) {
            return ParsedMedia::Episode(ParsedEpisode {
                normalized_title: normalize_title(&title),
                title: display_title(&title),
                season,
                episode: ep,
                year: None,
                confidence: 0.55,
                abs_path,
            });
        }
    }

    // Movie
    let stem = abs
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("film");
    let mut title_src = if is_generic_stem(stem) {
        abs.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(stem)
            .to_string()
    } else {
        stem.to_string()
    };
    let year = extract_year(&title_src);
    title_src = strip_scene_noise(&title_src);
    if title_src.is_empty() {
        title_src = display_title(stem);
    }
    ParsedMedia::Movie(ParsedMovie {
        normalized_title: normalize_title(&title_src),
        title: display_title(&title_src),
        year,
        confidence: 0.6,
        abs_path,
    })
}

fn parse_episode_only(stem: &str) -> Option<i32> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[\s._\-])(?:e|ep|episode)?[\s._\-]*(\d{1,3})(?:$|[\s._\-])").expect("ep")
    });
    let caps = re.captures(stem)?;
    let n: i32 = caps.get(1)?.as_str().parse().ok()?;
    if (1..=300).contains(&n) {
        Some(n)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_sxxexx() {
        let p = PathBuf::from("/media/Futurama.S08E01.MULTi.1080p.WEB.x265-GROUP.mkv");
        match parse_video(&p) {
            ParsedMedia::Episode(ep) => {
                assert_eq!(ep.normalized_title, "futurama");
                assert_eq!(ep.season, 8);
                assert_eq!(ep.episode, 1);
                assert!(ep.confidence > 0.8);
            }
            _ => panic!("expected episode"),
        }
    }

    #[test]
    fn per_release_folder() {
        let p = PathBuf::from(
            "/series/Futurama.S08E02.MULTi.1080p.WEB.x265-GROUP/Futurama.S08E02.mkv",
        );
        match parse_video(&p) {
            ParsedMedia::Episode(ep) => {
                assert_eq!(ep.normalized_title, "futurama");
                assert_eq!((ep.season, ep.episode), (8, 2));
            }
            _ => panic!("expected episode"),
        }
    }

    #[test]
    fn show_season_dir() {
        let p = PathBuf::from("/library/Breaking Bad/season.01/Breaking.Bad.S01E01.mkv");
        match parse_video(&p) {
            ParsedMedia::Episode(ep) => {
                assert_eq!(ep.normalized_title, "breaking bad");
                assert_eq!((ep.season, ep.episode), (1, 1));
            }
            _ => panic!("expected episode"),
        }
    }

    #[test]
    fn show_s01_dir() {
        let p = PathBuf::from("/library/Breaking Bad/S02/E04.mkv");
        match parse_video(&p) {
            ParsedMedia::Episode(ep) => {
                assert_eq!(ep.normalized_title, "breaking bad");
                assert_eq!((ep.season, ep.episode), (2, 4));
            }
            _ => panic!("expected episode"),
        }
    }

    #[test]
    fn show_space_s01_folder() {
        let p = PathBuf::from("/library/Foundation S03/Foundation.S03E01.1080p.mkv");
        match parse_video(&p) {
            ParsedMedia::Episode(ep) => {
                assert_eq!(ep.normalized_title, "foundation");
                assert_eq!((ep.season, ep.episode), (3, 1));
            }
            _ => panic!("expected episode"),
        }
    }

    #[test]
    fn foundation_folder_vs_flat() {
        let a = PathBuf::from("/m/foundation/season.01/Foundation.S01E05.mkv");
        let b = PathBuf::from("/m/Foundation S03/Foundation.S03E02.mkv");
        let ParsedMedia::Episode(ea) = parse_video(&a) else {
            panic!();
        };
        let ParsedMedia::Episode(eb) = parse_video(&b) else {
            panic!();
        };
        assert_eq!(ea.normalized_title, "foundation");
        assert_eq!(eb.normalized_title, "foundation");
        assert_eq!(ea.season, 1);
        assert_eq!(eb.season, 3);
    }

    #[test]
    fn movie_not_episode() {
        let p = PathBuf::from("/movies/The.Matrix.1999.1080p.BluRay.x264.mkv");
        match parse_video(&p) {
            ParsedMedia::Movie(m) => {
                assert_eq!(m.normalized_title, "the matrix");
                assert_eq!(m.year, Some(1999));
            }
            _ => panic!("expected movie"),
        }
    }

    #[test]
    fn normalize_casefold() {
        assert_eq!(normalize_title("Foundation"), normalize_title("foundation"));
        assert_eq!(normalize_title("Breaking.Bad"), "breaking bad");
    }

    #[test]
    fn is_video_and_display_title() {
        assert!(is_video(Path::new("a.MKV")));
        assert!(is_video(Path::new("a.mp4")));
        assert!(!is_video(Path::new("a.txt")));
        assert_eq!(display_title("The.Matrix_1999"), "The Matrix 1999");
        assert_eq!(display_title("..."), "Sans titre");
    }

    #[test]
    fn nxnn_and_season_dir_helpers() {
        assert_eq!(parse_season_dir("Season 1"), Some(1));
        assert_eq!(parse_season_dir("S02"), Some(2));
        assert_eq!(parse_season_dir("misc"), None);

        let p = PathBuf::from("/library/Show/Show.1x02.mkv");
        match parse_video(&p) {
            ParsedMedia::Episode(ep) => {
                assert_eq!(ep.season, 1);
                assert_eq!(ep.episode, 2);
            }
            _ => panic!("expected 1x02 episode"),
        }
    }
}
