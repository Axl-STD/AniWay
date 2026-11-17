//! Optimized copies ("proxies") of media drawn much smaller than their size or faster than allowed: ffmpeg
//! builds them in the background, the original keeps playing meanwhile and the player is swapped when ready.
use crate::live::output_size;
use crate::{App, Debounce};
use aniway_proto::geometry::{place, Size};
use aniway_proto::ipc::ProxyState;
use aniway_proto::model::{Overlay, ProxyFormat};
use aniway_proto::paths;
use anyhow::{bail, Context, Result};
use calloop::channel;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

/// The drawn size must stay stable this long before a copy is built, so interactive resizing builds nothing.
const SETTLE: Duration = Duration::from_secs(2);
/// Copies are built at the buffer size rounded up to this many pixels, so small scale changes reuse them.
const BUCKET: u32 = 32;
/// A copy is worth it when the media is drawn below this fraction of its size.
const WORTH: f64 = 0.75;
const MB: u64 = 1024 * 1024;

/// What a copy has to look like.
#[derive(Clone, PartialEq)]
pub(crate) struct Want {
    path: String,
    media: Size,
    width: u32,
    fps: Option<u32>,
    alpha: bool,
    /// Stored uncompressed (only with alpha).
    raw: bool,
}

/// An optimized copy of an overlay's media, being built or built.
pub(crate) struct Job {
    want: Want,
    gen: u64,
    pub state: ProxyState,
    file: Option<String>,
    pub error: Option<String>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Task {
    id: String,
    gen: u64,
    want: Want,
    cancel: Arc<AtomicBool>,
    limit_mb: u32,
    /// Copies being played, never evicted.
    keep: Vec<PathBuf>,
}

pub(crate) struct Done {
    id: String,
    gen: u64,
    result: Result<String, String>,
}

/// Runs the builds one at a time on a background thread.
pub(crate) struct Worker(mpsc::Sender<Task>);

impl Worker {
    pub(crate) fn start(done: channel::Sender<Done>) -> Self {
        let (tx, rx) = mpsc::channel::<Task>();
        std::thread::spawn(move || {
            for task in rx.into_iter().filter(|t| !t.cancel.load(Ordering::Relaxed)) {
                let started = Instant::now();
                let result = build(&task).map(|p| p.to_string_lossy().into_owned()).map_err(|e| format!("{e:#}"));
                log::info!("proxy for '{}' finished in {:.1?}: {:?}", task.id, started.elapsed(), result);
                let _ = done.send(Done { id: task.id, gen: task.gen, result });
            }
        });
        Self(tx)
    }
}

impl App {
    /// Copies that should exist now, per overlay.
    /// Uncompressed copies are granted in scene order while they fit the per-file and total limits. Nothing is
    /// decided before the player reports the frames (alpha and duration), even when it already plays a copy.
    fn proxy_wants(&self) -> HashMap<String, Want> {
        let mut wants = HashMap::new();
        let mut raw_left = u64::from(self.config.proxy_cache_mb) * MB;
        let raw_max = u64::from(self.config.raw_proxy_max_mb) * MB;
        for ov in self.scene.overlays.iter() {
            let Some(live) = self.live.get(&ov.id) else { continue };
            let scale = live.surface.as_ref().map(|s| s.scale120());
            let Some(mut want) = (live.media.zip(live.fps).zip(live.alpha))
                .and_then(|((m, f), alpha)| self.want_for(ov, m, f, alpha, scale))
            else {
                continue;
            };
            let raw = want.alpha && self.config.proxy_format == ProxyFormat::Auto;
            let bytes = live.duration.zip(live.fps).map(|(d, f)| raw_bytes(&want, d, f));
            if let Some(bytes) = bytes.filter(|&b| raw && b <= raw_max && b <= raw_left) {
                want.raw = true;
                raw_left -= bytes;
            }
            wants.insert(ov.id.clone(), want);
        }
        wants
    }

    /// The copy an overlay needs, if any, given the media size, rate and transparency and the buffer scale in 1/120
    /// units (the output's own scale when the overlay has no surface yet).
    fn want_for(&self, ov: &Overlay, media: Size, fps: f64, alpha: bool, scale120: Option<u32>) -> Option<Want> {
        if !(self.config.optimize && ov.optimize && ov.volume == 0) || paths::is_url(&ov.path) {
            return None;
        }
        let (_, info) = self.wl.find_output(ov.output.as_deref())?;
        let rect = place(output_size(&info), media, ov);
        // Without a surface the scale is estimated like `state` does: physical over logical size, in 1/120 units.
        let (logical, physical) = (output_size(&info), info.modes.iter().find(|m| m.current).map(|m| m.dimensions));
        let guess = physical.map_or(info.scale_factor.max(1) as u32 * 120, |(w, h)| {
            (f64::from(w.max(h)) / f64::from(logical.w.max(logical.h)) * 120.0).round() as u32
        });
        let scale = scale120.unwrap_or(guess);
        let ratio = |drawn: u32, full: u32| f64::from(drawn) * f64::from(scale) / 120.0 / f64::from(full);
        let ratio = ratio(rect.w, media.w).max(ratio(rect.h, media.h));
        let capped = ov.max_fps.filter(|&n| f64::from(n) + 0.5 < fps);
        if ratio >= WORTH && capped.is_none() {
            return None;
        }
        let width = if ratio < WORTH {
            ((f64::from(media.w) * ratio).ceil() as u32).div_ceil(BUCKET).saturating_mul(BUCKET).min(media.w)
        } else {
            media.w
        };
        Some(Want { path: ov.path.clone(), media, width, fps: capped, alpha, raw: false })
    }

    /// An already built copy of the overlay's media, found before any player exists so it never plays the original
    /// first. Returns the file with the media size and rate that were probed.
    pub(crate) fn proxy_cached(&self, ov: &Overlay) -> Option<(String, Size, f64)> {
        if !(self.config.optimize && ov.optimize && ov.volume == 0) || paths::is_url(&ov.path) {
            return None;
        }
        let src = paths::expand_tilde(&ov.path);
        let probe = probe(&src).ok()?;
        let fps = probe.fps?;
        let want = self.want_for(ov, probe.size, fps, probe.alpha, None)?;
        let raw = Want { raw: true, ..want.clone() };
        let file = [raw, want].iter().filter_map(|w| cache_file(w, &src).ok()).find(|f| f.exists())?;
        log::debug!("overlay '{}' found its copy {}", ov.id, file.display());
        touch(&file);
        Some((file.to_string_lossy().into_owned(), probe.size, fps))
    }

    /// Schedules `proxy_apply` when the wanted copies differ from the existing ones.
    pub(crate) fn proxy_review(&mut self) {
        let wants = self.proxy_wants();
        let same = wants.len() == self.proxies.len()
            && wants.iter().all(|(id, w)| self.proxies.get(id).is_some_and(|j| j.want == *w));
        if !same {
            self.debounce(Debounce::Proxy, SETTLE);
        }
    }

    /// Cancels obsolete builds, starts the missing ones and switches players to whatever is ready.
    pub(crate) fn proxy_apply(&mut self) {
        let wants = self.proxy_wants();
        self.proxies.retain(|id, job| wants.get(id).is_some_and(|w| *w == job.want));
        let keep: Vec<PathBuf> = self.live.values().filter_map(|l| l.source.as_ref().map(PathBuf::from)).collect();
        for (id, want) in wants {
            if self.proxies.contains_key(&id) {
                continue;
            }
            log::debug!(
                "overlay '{id}' wants a copy {}px wide (media {}x{}, cap {:?})",
                want.width,
                want.media.w,
                want.media.h,
                want.fps
            );
            self.proxy_gen += 1;
            let cancel = Arc::new(AtomicBool::new(false));
            let task = Task {
                id: id.clone(),
                gen: self.proxy_gen,
                want: want.clone(),
                cancel: cancel.clone(),
                limit_mb: self.config.proxy_cache_mb,
                keep: keep.clone(),
            };
            let _ = self.worker.0.send(task);
            let job = Job { want, gen: self.proxy_gen, state: ProxyState::Building, file: None, error: None, cancel };
            self.proxies.insert(id, job);
        }
        for id in self.live.keys().cloned().collect::<Vec<_>>() {
            self.proxy_switch(&id);
        }
        self.notify_state();
    }

    pub(crate) fn proxy_done(&mut self, done: Done) {
        let Some(job) = self.proxies.get_mut(&done.id).filter(|j| j.gen == done.gen) else { return };
        match done.result {
            Ok(file) => (job.state, job.file) = (ProxyState::Ready, Some(file)),
            Err(e) => {
                log::warn!("overlay '{}': optimized copy failed: {e}", done.id);
                (job.state, job.error) = (ProxyState::Failed, Some(e));
            }
        }
        self.proxy_switch(&done.id);
        self.notify_state();
    }

    /// File the overlay should play instead of its original, when its copy is ready.
    pub(crate) fn proxy_source(&self, ov: &Overlay) -> Option<String> {
        let job = self.proxies.get(&ov.id).filter(|j| j.state == ProxyState::Ready)?;
        let fits = job.want.path == ov.path && job.want.fps.is_none_or(|n| ov.max_fps == Some(n));
        job.file.clone().filter(|_| fits && self.config.optimize && ov.optimize)
    }

    /// Replaces the player of an overlay that plays a different file than it should: a ready copy replaces the
    /// original or an older copy, and no copy at all goes back to the original. While a new copy builds (or failed),
    /// whatever plays now keeps playing.
    fn proxy_switch(&mut self, id: &str) {
        let Some(ov) = self.scene.find(id).cloned() else { return };
        let target = match self.proxies.get(id) {
            Some(job) if job.state == ProxyState::Ready => self.proxy_source(&ov),
            Some(_) => return,
            None => None,
        };
        if self.live.get(id).is_some_and(|l| l.source != target) {
            self.respawn(id);
        }
    }
}

/// Scale and frame-rate filters; alpha is premultiplied around the scaling so edges get no dark halo.
fn filter(want: &Want) -> String {
    let scale = (want.width < want.media.w).then(|| format!("scale={}:-2:flags=lanczos", want.width));
    let mut chain: Vec<String> = want.fps.map(|n| format!("fps={n}")).into_iter().collect();
    match scale {
        Some(s) if want.alpha => {
            chain.extend(["format=gbrap,premultiply=inplace=1".into(), s, "unpremultiply=inplace=1".into()])
        }
        Some(s) => chain.push(s),
        None => {}
    }
    if chain.is_empty() {
        chain.push("null".into());
    }
    chain.join(",")
}

/// Bytes of an uncompressed RGBA copy of `duration` seconds.
fn raw_bytes(want: &Want, duration: f64, fps: f64) -> u64 {
    let height = (f64::from(want.width) * f64::from(want.media.h) / f64::from(want.media.w)).ceil() as u64;
    let frames = (duration * want.fps.map_or(fps, f64::from)).ceil().max(1.0) as u64;
    frames * u64::from(want.width) * height * 4
}

/// Extension, muxer and encoder options. Uncompressed RGBA in Matroska costs no decoding and keeps alpha exact
/// (NUT adds a pause at every loop). Otherwise alpha uses VP9 (the only widely available codec with it): lossy, but
/// 10-20 times smaller and cheaper to decode than lossless APNG; opaque media uses H.264.
fn encoder(alpha: bool, raw: bool) -> (&'static str, &'static str, &'static [&'static str]) {
    if alpha && raw {
        ("mkv", "matroska", &["-c:v", "rawvideo", "-pix_fmt", "rgba"])
    } else if alpha {
        (
            "webm",
            "webm",
            &[
                "-c:v",
                "libvpx-vp9",
                "-pix_fmt",
                "yuva420p",
                "-crf",
                "30",
                "-b:v",
                "0",
                "-row-mt",
                "1",
                "-threads",
                "2",
                "-auto-alt-ref",
                "0",
            ],
        )
    } else {
        (
            "mp4",
            "mp4",
            &["-c:v", "libx264", "-crf", "18", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-movflags", "+faststart"],
        )
    }
}

/// What `ffprobe` says about the first video stream.
struct Probe {
    size: Size,
    fps: Option<f64>,
    alpha: bool,
}

fn probe(src: &Path) -> Result<Probe> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-of", "default=nw=1"])
        .args(["-show_entries", "stream=width,height,pix_fmt,avg_frame_rate:stream_tags=alpha_mode"])
        .arg(src)
        .output()
        .context("running ffprobe (is ffmpeg installed?)")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix('='));
    let num = |name: &str| field(name).and_then(|v| v.parse::<u32>().ok());
    let (Some(w), Some(h), Some(fmt)) = (num("width"), num("height"), field("pix_fmt")) else {
        bail!("ffprobe found no video stream");
    };
    let fps = field("avg_frame_rate").and_then(|v| {
        let (n, d) = v.split_once('/')?;
        let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
        (d > 0.0 && n > 0.0).then_some(n / d)
    });
    // ffprobe decodes VP9 without its alpha plane; WebM marks it with a stream tag instead.
    let alpha = alpha_format(fmt) || text.lines().any(|l| l.eq_ignore_ascii_case("TAG:alpha_mode=1"));
    Ok(Probe { size: Size { w, h }, fps, alpha })
}

/// Pixel formats that may carry transparency (`pal8` is how GIF and APNG palettes with a transparent index decode).
fn alpha_format(fmt: &str) -> bool {
    ["rgba", "bgra", "argb", "abgr", "ya8", "ya16", "yuva", "gbrap", "pal8"].iter().any(|n| fmt.starts_with(n))
}

/// FNV-1a: stable across runs, unlike the std hasher.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Path of the copy for `want`, named after everything that changes its content.
fn cache_file(want: &Want, src: &Path) -> Result<PathBuf> {
    let meta = std::fs::metadata(src).with_context(|| format!("reading {}", src.display()))?;
    let mtime = meta.modified()?.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let (ext, _, codec) = encoder(want.alpha, want.raw);
    let key = fnv(&format!(
        "{}|{mtime}|{}|{}x{}|{:?}|{}",
        want.path,
        meta.len(),
        want.width,
        want.media.h,
        want.fps,
        codec.join(" ")
    ));
    Ok(paths::proxies_dir().join(format!("{key:016x}.{ext}")))
}

/// Marks a file as recently used for the LRU eviction.
fn touch(file: &Path) {
    let _ = std::fs::File::open(file).and_then(|f| f.set_modified(SystemTime::now()));
}

fn build(task: &Task) -> Result<PathBuf> {
    let (want, dir) = (&task.want, paths::proxies_dir());
    let src = paths::expand_tilde(&want.path);
    let (_, muxer, codec) = encoder(want.alpha, want.raw);
    let out = cache_file(want, &src)?;
    if out.exists() {
        touch(&out);
        return Ok(out);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let part = out.with_extension("part");
    let mut child = Command::new("nice")
        .args(["-n", "19", "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-threads", "2", "-i"])
        .arg(&src)
        .args(["-an", "-vf", &filter(want)])
        .args(codec)
        .args(["-f", muxer])
        .arg(&part)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting ffmpeg (is it installed?)")?;
    let status = loop {
        if task.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&part);
            bail!("cancelled");
        }
        match child.try_wait()? {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    if !status.success() {
        let mut log = String::new();
        let _ = child.stderr.take().map(|mut e| e.read_to_string(&mut log));
        let _ = std::fs::remove_file(&part);
        bail!("ffmpeg failed: {}", log.lines().last().unwrap_or("unknown error"));
    }
    std::fs::rename(&part, &out)?;
    let keep: Vec<PathBuf> = task.keep.iter().cloned().chain([out.clone()]).collect();
    evict(&dir, task.limit_mb, &keep);
    Ok(out)
}

/// Deletes the least recently used files of the cache directory (never those in `keep`) until it fits the limit.
fn evict(dir: &Path, limit_mb: u32, keep: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| Some((e.path(), e.metadata().ok().filter(|m| m.is_file())?)))
        .map(|(p, m)| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), p))
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files.into_iter().filter(|f| !keep.contains(&f.2)) {
        if total <= u64::from(limit_mb) * MB {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(width: u32, fps: Option<u32>) -> Want {
        Want { path: "a.apng".into(), media: Size { w: 1300, h: 1300 }, width, fps, alpha: true, raw: false }
    }

    #[test]
    fn filter_scales_and_caps() {
        assert_eq!(filter(&Want { alpha: false, ..want(416, None) }), "scale=416:-2:flags=lanczos");
        assert_eq!(
            filter(&want(416, Some(30))),
            "fps=30,format=gbrap,premultiply=inplace=1,scale=416:-2:flags=lanczos,unpremultiply=inplace=1"
        );
        assert_eq!(filter(&want(1300, Some(24))), "fps=24");
    }

    #[test]
    fn keys_and_encoders() {
        assert!(["bgra", "rgba64le", "yuva420p", "pal8", "gbrapf32le"].into_iter().all(alpha_format));
        assert!(!["yuv420p", "rgb24", "gray", "yuvj420p"].into_iter().any(alpha_format));
        assert_eq!(fnv("a"), fnv("a"));
        assert_ne!(fnv("a"), fnv("b"));
        assert_eq!(encoder(true, false).0, "webm");
        assert_eq!(encoder(false, false).0, "mp4");
        assert_eq!(encoder(true, true).1, "matroska");
        assert_eq!(encoder(false, true).0, "mp4");
        // 434 frames at 60 fps, 416x416 RGBA.
        assert_eq!(raw_bytes(&want(416, None), 434.0 / 60.0, 60.0), 434 * 416 * 416 * 4);
        assert_eq!(raw_bytes(&want(416, Some(30)), 434.0 / 60.0, 60.0), 217 * 416 * 416 * 4);
    }

    #[test]
    fn eviction_deletes_least_recently_used_first_and_keeps_the_new_file() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("ow-evict-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let now = SystemTime::now();
        for (name, age) in [("old", 300), ("mid", 200), ("new", 100)] {
            let file = std::fs::File::create(dir.join(name))?;
            file.set_len(600 * 1024)?;
            file.set_modified(now - Duration::from_secs(age))?;
        }
        evict(&dir, 1, &[dir.join("new")]);
        let left: Vec<_> = ["old", "mid", "new"].into_iter().filter(|n| dir.join(n).exists()).collect();
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(left, ["new"]);
        Ok(())
    }
}
