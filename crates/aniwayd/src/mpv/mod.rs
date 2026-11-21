mod ffi;

use crate::egl::get_proc_address;
use aniway_proto::geometry::Size;
use aniway_proto::model::{Decode, Overlay};
use aniway_proto::paths;
use anyhow::{bail, Result};
use calloop::channel::Sender;
use ffi::*;
use std::ffi::{c_char, c_int, c_void, CStr, CString};

/// Scaling and post-processing options that cost render time without a visible gain on small overlays. Big
/// hardware-decoded videos keep mpv's defaults, where downscaling quality matters.
const CHEAP_RENDER: [(&str, &str); 9] = [
    ("scale", "bilinear"),
    ("dscale", "bilinear"),
    ("cscale", "bilinear"),
    ("dither", "no"),
    ("deband", "no"),
    ("correct-downscaling", "no"),
    ("linear-downscaling", "no"),
    ("sigmoid-upscaling", "no"),
    ("hdr-compute-peak", "no"),
];

pub enum Event {
    Size(Size),
    Loaded,
    Failed(String),
}

/// Sent from libmpv threads; only forwards the overlay key to the main loop.
struct Wake {
    tx: Sender<u64>,
    key: u64,
}

unsafe extern "C" fn wake(data: *mut c_void) {
    // SAFETY: data is the Box<Wake> owned by the Player, which outlives both callbacks.
    let wake = unsafe { &*(data as *const Wake) };
    let _ = wake.tx.send(wake.key);
}

pub struct Player {
    handle: *mut mpv_handle,
    render: *mut mpv_render_context,
    /// False for still/animated images and alpha-capable containers, which are always decoded in software.
    pub hwdec_eligible: bool,
    /// True once hardware decoding (and its interop contexts) has been enabled.
    pub hwdec_on: bool,
    mirror_y: bool,
    _wake: Box<Wake>,
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

fn error_text(code: c_int) -> String {
    // SAFETY: mpv_error_string returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(mpv_error_string(code)) }.to_string_lossy().into_owned()
}

fn video_rotate(ov: &Overlay) -> u16 {
    (ov.rotation + if ov.flip_h { 180 } else { 0 }) % 360
}

/// What a player loads and what is known about it.
pub struct Source {
    pub path: String,
    /// The media is known to be big, so hardware decoding (and its costly interop contexts) starts enabled.
    pub known_big: bool,
}

impl Player {
    /// The EGL context must be current. `ov` provides the playback options.
    pub fn new(
        ov: &Overlay,
        source: &Source,
        hwdec: &str,
        paused: bool,
        wl_display: *mut c_void,
        tx: Sender<u64>,
        key: u64,
    ) -> Result<Self> {
        // SAFETY: plain version query.
        let version = unsafe { mpv_client_api_version() };
        if version >> 16 != 2 {
            bail!("unsupported libmpv client API {}.{}", version >> 16, version & 0xffff);
        }
        // SAFETY: mpv_create has no preconditions; a null result is handled.
        let handle = unsafe { mpv_create() };
        if handle.is_null() {
            bail!("mpv_create failed");
        }
        let ext = paths::extension(&source.path);
        let image = paths::IMAGE_EXTENSIONS.contains(&ext.as_str());
        // The native VP9 decoder drops the alpha plane of WebM files, so those use libvpx.
        let alpha_container = matches!(ext.as_str(), "webm" | "mkv");
        let video = !image && !alpha_container;
        let raw_copy = ext == "mkv" && std::path::Path::new(&source.path).starts_with(paths::proxies_dir());
        let eligible = video && ov.decode != Decode::Software;
        let mut player = Self {
            handle,
            render: std::ptr::null_mut(),
            hwdec_eligible: eligible,
            hwdec_on: eligible && (ov.decode == Decode::Hardware || source.known_big),
            _wake: Box::new(Wake { tx, key }),
            mirror_y: ov.flip_h != ov.flip_v,
        };
        let audio = if ov.volume == 0 { ("aid", "no".to_string()) } else { ("volume", ov.volume.to_string()) };
        let options = [
            ("config", "no".to_string()),
            ("vo", "libmpv".into()),
            ("hwdec", if player.hwdec_on { hwdec.into() } else { "no".into() }),
            ("background", "none".into()),
            ("panscan", "1".into()),
            // Rendering must never wait for the presentation time: several overlays share one thread.
            ("video-timing-offset", "0".into()),
            ("osc", "no".into()),
            ("input-default-bindings", "no".into()),
            ("input-vo-keyboard", "no".into()),
            ("terminal", "no".into()),
            ("ytdl", "no".into()),
            ("keep-open", "yes".into()),
            ("image-display-duration", "inf".into()),
            ("demuxer-max-bytes", demuxer_bytes(raw_copy).into()),
            ("demuxer-max-back-bytes", "1MiB".into()),
            ("loop-file", loop_value(ov.looping).into()),
            ("video-rotate", video_rotate(ov).to_string()),
            ("speed", ov.speed.to_string()),
            ("pause", yes_no(paused).into()),
            audio,
        ];
        for (name, value) in options {
            player.option(name, &value)?;
        }
        if !player.hwdec_on {
            player.option("gpu-hwdec-interop", "no")?;
        }
        if !player.hwdec_on {
            for (name, value) in CHEAP_RENDER {
                player.option(name, value)?;
            }
        }
        if (video && !player.hwdec_on) || raw_copy {
            // Frame threading multiplies decoder buffers by the core count; small videos and uncompressed copies
            // do not need it.
            player.option("vd-lavc-threads", "1")?;
        }
        if alpha_container {
            let _ = player.option("vd", "libvpx-vp9,libvpx,").map_err(|e| log::warn!("{e:#}"));
        }
        // SAFETY: handle is valid and not yet initialized.
        let rc = unsafe { mpv_initialize(handle) };
        if rc < 0 {
            bail!("mpv_initialize: {}", error_text(rc));
        }
        player.create_render_context(vaapi_display(hwdec, wl_display))?;
        player.load(&source.path)?;
        Ok(player)
    }

    pub fn set_transform(&mut self, ov: &Overlay) {
        self.property("video-rotate", &video_rotate(ov).to_string());
        self.mirror_y = ov.flip_h != ov.flip_v;
    }

    fn create_render_context(&mut self, wl_display: *mut c_void) -> Result<()> {
        let mut init = mpv_opengl_init_params {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: std::ptr::null_mut(),
        };
        let api = cstr("opengl");
        let mut params = [
            mpv_render_param { kind: RP_API_TYPE, data: api.as_ptr() as *mut c_void },
            mpv_render_param { kind: RP_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
            // A null display ends the list early: only VAAPI needs it and it costs memory per player.
            mpv_render_param { kind: if wl_display.is_null() { 0 } else { RP_WL_DISPLAY }, data: wl_display },
            mpv_render_param { kind: 0, data: std::ptr::null_mut() },
        ];

        let data = &*self._wake as *const Wake as *mut c_void;
        // SAFETY: params is INVALID-terminated and every pointer in it outlives the call.
        let rc = unsafe { mpv_render_context_create(&mut self.render, self.handle, params.as_mut_ptr()) };
        if rc < 0 {
            self.render = std::ptr::null_mut();
            bail!("mpv_render_context_create: {}", error_text(rc));
        }
        // SAFETY: data points to the Box<Wake> owned by self, freed only after both callbacks are cleared.
        unsafe {
            mpv_set_wakeup_callback(self.handle, Some(wake), data);
            mpv_render_context_set_update_callback(self.render, Some(wake), data);
        }
        Ok(())
    }

    fn option(&self, name: &str, value: &str) -> Result<()> {
        // SAFETY: handle is valid and both strings are NUL-terminated.
        let rc = unsafe { mpv_set_option_string(self.handle, cstr(name).as_ptr(), cstr(value).as_ptr()) };
        if rc < 0 {
            bail!("mpv option {name}={value}: {}", error_text(rc));
        }
        Ok(())
    }

    fn property(&self, name: &str, value: &str) {
        // SAFETY: handle is valid and both strings are NUL-terminated.
        let rc = unsafe { mpv_set_property_string(self.handle, cstr(name).as_ptr(), cstr(value).as_ptr()) };
        if rc < 0 {
            log::warn!("mpv property {name}={value}: {}", error_text(rc));
        }
    }

    fn load(&self, path: &str) -> Result<()> {
        let path =
            if paths::is_url(path) { path.into() } else { paths::expand_tilde(path).to_string_lossy().into_owned() };
        let (cmd, path, mode) = (cstr("loadfile"), cstr(&path), cstr("replace"));
        let mut args = [cmd.as_ptr(), path.as_ptr(), mode.as_ptr(), std::ptr::null()];
        // SAFETY: args is NULL-terminated and the strings outlive the call.
        let rc = unsafe { mpv_command(self.handle, args.as_mut_ptr()) };
        if rc < 0 {
            bail!("loadfile: {}", error_text(rc));
        }
        Ok(())
    }

    pub fn set_paused(&self, paused: bool) {
        self.property("pause", yes_no(paused));
    }

    pub fn set_speed(&self, speed: f64) {
        self.property("speed", &speed.to_string());
    }

    pub fn set_loop(&self, looping: bool) {
        self.property("loop-file", loop_value(looping));
    }

    pub fn set_volume(&self, volume: u32) {
        self.property("aid", if volume == 0 { "no" } else { "auto" });
        self.property("volume", &volume.to_string());
    }

    /// Changes the hardware decoding method. Ignored for players created without hwdec.
    pub fn set_hwdec(&self, hwdec: &str) {
        if self.hwdec_on {
            self.property("hwdec", hwdec);
        }
    }

    fn get<T: Default>(&self, name: &str, format: c_int) -> Option<T> {
        let mut out = T::default();
        // SAFETY: out is a valid T matching the requested mpv format.
        let rc =
            unsafe { mpv_get_property(self.handle, cstr(name).as_ptr(), format, &mut out as *mut T as *mut c_void) };
        (rc >= 0).then_some(out)
    }

    fn string(&self, name: &str) -> Option<String> {
        let ptr: *mut c_char = self.get(name, FORMAT_STRING).filter(|p: &*mut c_char| !p.is_null())?;
        // SAFETY: mpv returned a NUL-terminated string that we free right after copying it.
        let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        // SAFETY: ptr was allocated by mpv_get_property.
        unsafe { mpv_free(ptr as *mut c_void) };
        Some(text)
    }

    pub fn hwdec_current(&self) -> Option<String> {
        self.string("hwdec-current")
    }

    /// The decoded frames carry transparency (the property is unavailable without an alpha channel).
    pub fn alpha(&self) -> bool {
        matches!(self.string("video-params/alpha").as_deref(), Some("straight" | "premul"))
    }

    /// Frames lost so far: dropped by the decoder/VO plus frames the VO delivered late.
    pub fn dropped(&self) -> u64 {
        let count = |name| self.get::<i64>(name, FORMAT_INT64).unwrap_or(0).max(0) as u64;
        count("frame-drop-count") + count("vo-delayed-frame-count")
    }

    /// Frame rate of the loaded media, when known.
    pub fn fps(&self) -> Option<f64> {
        self.get("container-fps", FORMAT_DOUBLE).filter(|f: &f64| *f > 0.0)
    }

    pub fn duration(&self) -> Option<f64> {
        self.get("duration", FORMAT_DOUBLE).filter(|d: &f64| *d > 0.0)
    }

    /// Caps the frame rate by dropping frames with the `fps` filter (used when there is no optimized copy).
    pub fn cap_fps(&self, fps: u32) {
        self.property("vf", &format!("fps={fps}"));
    }

    fn display_size(&self) -> Option<Size> {
        let w: i64 = self.get("dwidth", FORMAT_INT64)?;
        let h: i64 = self.get("dheight", FORMAT_INT64)?;
        (w > 0 && h > 0).then_some(Size { w: w as u32, h: h as u32 })
    }

    /// Drains pending mpv events.
    pub fn events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            // SAFETY: handle is valid; the returned event is read before the next mpv_wait_event call.
            let ev = unsafe { &*mpv_wait_event(self.handle, 0.0) };
            match ev.event_id {
                EVENT_NONE => return out,
                EVENT_FILE_LOADED => out.push(Event::Loaded),
                EVENT_VIDEO_RECONFIG => out.extend(self.display_size().map(Event::Size)),
                EVENT_END_FILE if !ev.data.is_null() => {
                    // SAFETY: END_FILE events carry an mpv_event_end_file.
                    let end = unsafe { &*(ev.data as *const mpv_event_end_file) };
                    if end.reason == END_FILE_ERROR {
                        out.push(Event::Failed(error_text(end.error)));
                    }
                }
                _ => {}
            }
        }
    }

    /// True when a new video frame is ready to be rendered.
    pub fn frame_ready(&self) -> bool {
        // SAFETY: render is a valid render context.
        unsafe { mpv_render_context_update(self.render) & RENDER_UPDATE_FRAME != 0 }
    }

    /// Renders the current frame into the bound framebuffer 0.
    pub fn render(&self, w: u32, h: u32) {
        let mut fbo = mpv_opengl_fbo { fbo: 0, w: w as c_int, h: h as c_int, internal_format: 0 };
        let mut flip_y: c_int = if self.mirror_y { 0 } else { 1 };
        let mut block: c_int = 0;
        let mut params = [
            mpv_render_param { kind: RP_BLOCK_FOR_TARGET_TIME, data: &mut block as *mut _ as *mut c_void },
            mpv_render_param { kind: RP_OPENGL_FBO, data: &mut fbo as *mut _ as *mut c_void },
            mpv_render_param { kind: RP_FLIP_Y, data: &mut flip_y as *mut _ as *mut c_void },
            mpv_render_param { kind: 0, data: std::ptr::null_mut() },
        ];
        // SAFETY: params is INVALID-terminated and the EGL context is current.
        unsafe { mpv_render_context_render(self.render, params.as_mut_ptr()) };
    }

    pub fn report_swap(&self) {
        // SAFETY: render is a valid render context.
        unsafe { mpv_render_context_report_swap(self.render) };
    }
}

impl Drop for Player {
    /// The EGL context must be current so mpv can release its GL objects.
    fn drop(&mut self) {
        // SAFETY: the render context is freed before its mpv handle, as libmpv requires.
        unsafe {
            if !self.render.is_null() {
                mpv_render_context_set_update_callback(self.render, None, std::ptr::null_mut());
                mpv_render_context_free(self.render);
            }
            mpv_set_wakeup_callback(self.handle, None, std::ptr::null_mut());
            mpv_terminate_destroy(self.handle);
            // Decoded frames and cached packets are freed in scattered small blocks that glibc keeps otherwise.
            malloc_trim(0);
        }
    }
}

/// Caps glibc at two malloc arenas, before any thread exists. By default every mpv thread can get its own arena (8
/// per core) and each one keeps freed decoder buffers: ~110 MB more after a few dozen players came and went.
pub fn limit_arenas() {
    // SAFETY: mallopt only changes an allocator tunable.
    unsafe { mallopt(M_ARENA_MAX, 2) };
}

/// The Wayland display is only needed for VAAPI interop and costs memory per player.
fn vaapi_display(hwdec: &str, wl_display: *mut c_void) -> *mut c_void {
    if hwdec.contains("vaapi") {
        wl_display
    } else {
        std::ptr::null_mut()
    }
}

/// Read-ahead limit. Uncompressed copies are read from the page cache with no decoding, so a few frames ahead are
/// enough, and a 16 MiB queue of ~700 KB frames per overlay would only hold memory.
fn demuxer_bytes(raw_copy: bool) -> &'static str {
    if raw_copy {
        "2MiB"
    } else {
        "16MiB"
    }
}

fn yes_no(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

fn loop_value(looping: bool) -> &'static str {
    if looping {
        "inf"
    } else {
        "no"
    }
}
