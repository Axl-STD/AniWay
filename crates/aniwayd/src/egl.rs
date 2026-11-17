use anyhow::{anyhow, Result};
use khronos_egl as egl;
use std::ffi::{c_char, c_void, CStr};
use wayland_egl::WlEglSurface;

const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

/// One desktop-GL 3.3 core context shared by every overlay surface. When no overlay surface is current, a 1x1
/// pbuffer is bound instead: libmpv inspects the default framebuffer when its render context is created.
pub struct Egl {
    api: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    idle: egl::Surface,
}

impl Egl {
    pub fn new(wl_display: *mut c_void) -> Result<Self> {
        let api = egl::Instance::new(egl::Static);
        // SAFETY: wl_display is the live wl_display owned by the Wayland connection.
        let display = unsafe { api.get_platform_display(PLATFORM_WAYLAND_KHR, wl_display, &[egl::ATTRIB_NONE]) }?;
        api.initialize(display)?;
        api.bind_api(egl::OPENGL_API)?;
        let config = api
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::WINDOW_BIT | egl::PBUFFER_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::NONE,
                ],
            )?
            .ok_or_else(|| anyhow!("no RGBA8888 desktop-GL EGL config"))?;
        let context = api.create_context(
            display,
            config,
            None,
            &[
                egl::CONTEXT_MAJOR_VERSION,
                3,
                egl::CONTEXT_MINOR_VERSION,
                3,
                egl::CONTEXT_OPENGL_PROFILE_MASK,
                egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
                egl::NONE,
            ],
        )?;
        let idle = api.create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])?;
        let egl = Self { api, display, config, context, idle };
        egl.make_current(None)?;
        Ok(egl)
    }

    /// Creates a window surface with vsync disabled so several surfaces never block on one thread.
    pub fn create_surface(&self, window: &WlEglSurface) -> Result<egl::Surface> {
        // SAFETY: window.ptr() is a live wl_egl_window that outlives the returned surface.
        let surface = unsafe {
            self.api.create_platform_window_surface(
                self.display,
                self.config,
                window.ptr() as *mut c_void,
                &[egl::ATTRIB_NONE],
            )
        }?;
        self.make_current(Some(surface))?;
        self.api.swap_interval(self.display, 0)?;
        Ok(surface)
    }

    pub fn make_current(&self, surface: Option<egl::Surface>) -> Result<(), egl::Error> {
        let surface = surface.unwrap_or(self.idle);
        self.api.make_current(self.display, Some(surface), Some(surface), Some(self.context))
    }

    pub fn swap(&self, surface: egl::Surface) -> Result<(), egl::Error> {
        self.api.swap_buffers(self.display, surface)
    }

    /// Destroys a surface, leaving the context bound to the idle pbuffer.
    pub fn destroy_surface(&self, surface: egl::Surface) {
        let _ = self.make_current(None);
        let _ = self.api.destroy_surface(self.display, surface);
    }
}

impl Drop for Egl {
    fn drop(&mut self) {
        let _ = self.api.make_current(self.display, None, None, None);
        let _ = self.api.destroy_surface(self.display, self.idle);
        let _ = self.api.destroy_context(self.display, self.context);
        let _ = self.api.terminate(self.display);
    }
}

/// `get_proc_address` callback handed to libmpv.
pub unsafe extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: libmpv passes a valid NUL-terminated symbol name.
    let name = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or_default();
    egl::Instance::new(egl::Static).get_proc_address(name).map_or(std::ptr::null_mut(), |f| f as *mut c_void)
}
