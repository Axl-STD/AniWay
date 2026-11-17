#![allow(non_camel_case_types)]
use std::ffi::{c_char, c_double, c_int, c_ulong, c_void};

pub enum mpv_handle {}
pub enum mpv_render_context {}

pub const FORMAT_STRING: c_int = 1;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_VIDEO_RECONFIG: c_int = 17;
pub const END_FILE_ERROR: c_int = 4;

pub const RP_API_TYPE: c_int = 1;
pub const RP_OPENGL_INIT_PARAMS: c_int = 2;
pub const RP_OPENGL_FBO: c_int = 3;
pub const RP_FLIP_Y: c_int = 4;
pub const RP_WL_DISPLAY: c_int = 9;
pub const RP_BLOCK_FOR_TARGET_TIME: c_int = 12;
pub const RENDER_UPDATE_FRAME: u64 = 1;

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
}

#[repr(C)]
pub struct mpv_render_param {
    pub kind: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

pub type Callback = unsafe extern "C" fn(*mut c_void);

pub const M_ARENA_MAX: c_int = -8;

extern "C" {
    /// glibc: returns freed heap memory to the system.
    pub fn malloc_trim(pad: usize) -> c_int;
    /// glibc: sets an allocator tunable.
    pub fn mallopt(param: c_int, value: c_int) -> c_int;
}

#[link(name = "mpv")]
extern "C" {
    pub fn mpv_client_api_version() -> c_ulong;
    pub fn mpv_create() -> *mut mpv_handle;
    pub fn mpv_initialize(h: *mut mpv_handle) -> c_int;
    pub fn mpv_terminate_destroy(h: *mut mpv_handle);
    pub fn mpv_set_option_string(h: *mut mpv_handle, name: *const c_char, value: *const c_char) -> c_int;
    pub fn mpv_set_property_string(h: *mut mpv_handle, name: *const c_char, value: *const c_char) -> c_int;
    pub fn mpv_get_property(h: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    pub fn mpv_free(data: *mut c_void);
    pub fn mpv_command(h: *mut mpv_handle, args: *mut *const c_char) -> c_int;
    pub fn mpv_wait_event(h: *mut mpv_handle, timeout: c_double) -> *mut mpv_event;
    pub fn mpv_set_wakeup_callback(h: *mut mpv_handle, cb: Option<Callback>, data: *mut c_void);
    pub fn mpv_error_string(error: c_int) -> *const c_char;
    pub fn mpv_render_context_create(
        res: *mut *mut mpv_render_context,
        h: *mut mpv_handle,
        params: *mut mpv_render_param,
    ) -> c_int;
    pub fn mpv_render_context_set_update_callback(
        ctx: *mut mpv_render_context,
        cb: Option<Callback>,
        data: *mut c_void,
    );
    pub fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64;
    pub fn mpv_render_context_render(ctx: *mut mpv_render_context, params: *mut mpv_render_param) -> c_int;
    pub fn mpv_render_context_report_swap(ctx: *mut mpv_render_context);
    pub fn mpv_render_context_free(ctx: *mut mpv_render_context);
}
