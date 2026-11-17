//! Media metadata (ffprobe) and thumbnails (ffmpeg, cached on disk), produced on worker threads.

use aniway_proto::geometry::Size;
use aniway_proto::paths::cache_dir;
use gtk::prelude::*;
use gtk::{gdk, gio};
use serde_json::Value;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Default, Clone)]
pub struct Media {
    pub size: Option<Size>,
    pub codec: Option<String>,
    /// Seconds; `None` for still images.
    pub duration: Option<f64>,
    pub texture: Option<gdk::Texture>,
}

impl Media {
    /// Whether the media moves. ffprobe reports no duration for APNG and animated WebP, so their codec decides.
    pub fn moving(&self) -> bool {
        self.duration.is_some() || matches!(self.codec.as_deref(), Some("apng" | "webp_anim" | "gif"))
    }

    /// Duration as `m:ss` (or `h:mm:ss`); empty for stills and until the probe finished.
    pub fn duration_text(&self) -> String {
        let Some(s) = self.duration.map(|d| d.round() as u64) else { return String::new() };
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    }
}

/// Runs an external tool without a terminal; any failure (including a missing binary) is `None`.
fn run(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn probe(path: &str) -> Media {
    let args = ["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", path];
    let Some(json) = run("ffprobe", &args).and_then(|o| serde_json::from_slice::<Value>(&o).ok()) else {
        return Media::default();
    };
    let video = json["streams"].as_array().and_then(|s| s.iter().find(|s| s["codec_type"] == "video"));
    let dim = |k: &str| video.and_then(|v| v[k].as_u64()).map(|n| n as u32);
    Media {
        size: dim("width").zip(dim("height")).map(|(w, h)| Size { w, h }),
        codec: video.and_then(|v| v["codec_name"].as_str()).map(String::from),
        duration: json["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        texture: None,
    }
}

/// Frame rate of the animated preview.
pub const PREVIEW_FPS: u32 = 12;
/// Seconds of media the preview loops over, and its longest side in pixels.
const PREVIEW_SECS: u32 = 4;
const PREVIEW_SIDE: u32 = 320;

/// Cache key of a file: its path, modification time and size.
fn key(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (path, meta.modified().ok(), meta.len()).hash(&mut hasher);
    Some(format!("{:016x}", hasher.finish()))
}

/// Thumbnail PNG of `path` in the disk cache; built with ffmpeg when missing.
fn thumbnail(path: &str, duration: Option<f64>) -> Option<PathBuf> {
    let dir = cache_dir().join("thumbs");
    let file = dir.join(format!("{}.png", key(path)?));
    if file.exists() {
        return Some(file);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let tmp = file.with_extension("tmp.png");
    let at = duration.map_or(0.0, |d| (d / 2.0).min(1.0)).to_string();
    let args = ["-v", "quiet", "-y", "-ss", &at, "-i", path, "-frames:v", "1", "-vf", "scale=256:-2"];
    let tmp_str = tmp.to_str()?;
    run("ffmpeg", &[&args[..], &[tmp_str]].concat())?;
    std::fs::rename(&tmp, &file).ok()?;
    Some(file)
}

/// Directory of small PNG frames of the first seconds of `path`, cached on disk; built with ffmpeg when missing.
/// The frames are written to a temporary directory that is renamed when complete.
fn preview_dir(path: &str) -> Option<PathBuf> {
    let dir = cache_dir().join("previews").join(key(path)?);
    if dir.is_dir() {
        return Some(dir);
    }
    let tmp = dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).ok()?;
    let fit = format!(
        "fps={PREVIEW_FPS},scale={PREVIEW_SIDE}:{PREVIEW_SIDE}:force_original_aspect_ratio=decrease:force_divisible_by=2"
    );
    let (secs, frames) = (PREVIEW_SECS.to_string(), (PREVIEW_FPS * PREVIEW_SECS).to_string());
    let out = tmp.join("%03d.png");
    let args = ["-v", "quiet", "-t", &secs, "-i", path, "-vf", &fit, "-frames:v", &frames, out.to_str()?];
    run("ffmpeg", &args)?;
    std::fs::rename(&tmp, &dir).ok()?;
    Some(dir)
}

/// Frames of the animated preview of `path`; empty when ffmpeg cannot make them.
pub async fn preview_frames(path: String) -> Vec<gdk::Texture> {
    gio::spawn_blocking(move || {
        let Some(dir) = preview_dir(&path) else { return Vec::new() };
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        files.sort();
        files.iter().filter_map(|f| gdk::Texture::from_filename(f).ok()).collect()
    })
    .await
    .unwrap_or_default()
}

/// Probes `path` and builds its picture: stills load directly, everything else from the cached thumbnail.
pub async fn load(path: String) -> Media {
    let p = path.clone();
    let mut media = gio::spawn_blocking(move || probe(&p)).await.unwrap_or_default();
    let direct = media.duration.is_none().then(|| gdk::Texture::from_filename(&path).ok()).flatten();
    media.texture = match direct {
        Some(t) => {
            media.size = media.size.or(Some(Size { w: t.width() as u32, h: t.height() as u32 }));
            Some(t)
        }
        None => {
            let duration = media.duration;
            let thumb = gio::spawn_blocking(move || thumbnail(&path, duration)).await.ok().flatten();
            thumb.and_then(|t| gdk::Texture::from_filename(t).ok())
        }
    };
    media
}
