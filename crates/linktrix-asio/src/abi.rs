//! Windows x64 IASIO ABI, translated from Steinberg ASIO SDK 2.3 (GPLv3
//! option). Original SDK notices remain in third_party/asio.
//! ASIO's C++ virtual methods are the platform ABI on x64. x86 is not supported.
use std::ffi::{c_char, c_void};
use windows_sys::core::GUID;
pub const CLSID: GUID = GUID::from_u128(0xa874e189_2361_4c9e_ae64_56a474986402);
pub const CLSID_TEXT: &str = "{A874E189-2361-4C9E-AE64-56A474986402}";
pub const OK: i32 = 0;
pub const NOT_PRESENT: i32 = -1000;
pub const INVALID: i32 = -998;
pub const BAD_MODE: i32 = -997;
pub const NO_CLOCK: i32 = -995;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Samples {
    pub hi: u32,
    pub lo: u32,
}
impl From<u64> for Samples {
    fn from(v: u64) -> Self {
        Self {
            hi: (v >> 32) as u32,
            lo: v as u32,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ChannelInfo {
    pub channel: i32,
    pub is_input: i32,
    pub active: i32,
    pub group: i32,
    pub kind: i32,
    pub name: [c_char; 32],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferInfo {
    pub is_input: i32,
    pub channel: i32,
    pub buffers: [*mut c_void; 2],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClockSource {
    pub index: i32,
    pub channel: i32,
    pub group: i32,
    pub current: i32,
    pub name: [c_char; 32],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Callbacks {
    pub switch: Option<unsafe extern "C" fn(i32, i32)>,
    pub rate: Option<unsafe extern "C" fn(f64)>,
    pub message: Option<unsafe extern "C" fn(i32, i32, *mut c_void, *mut f64) -> i32>,
    pub time: Option<unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_void>,
}
#[repr(C)]
pub struct Asio {
    pub vtable: *const Vtable,
}
#[repr(C)]
pub struct Vtable {
    pub query: unsafe extern "system" fn(*mut Asio, *const GUID, *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "system" fn(*mut Asio) -> u32,
    pub release: unsafe extern "system" fn(*mut Asio) -> u32,
    pub init: unsafe extern "system" fn(*mut Asio, *mut c_void) -> i32,
    pub name: unsafe extern "system" fn(*mut Asio, *mut c_char),
    pub version: unsafe extern "system" fn(*mut Asio) -> i32,
    pub error: unsafe extern "system" fn(*mut Asio, *mut c_char),
    pub start: unsafe extern "system" fn(*mut Asio) -> i32,
    pub stop: unsafe extern "system" fn(*mut Asio) -> i32,
    pub channels: unsafe extern "system" fn(*mut Asio, *mut i32, *mut i32) -> i32,
    pub latencies: unsafe extern "system" fn(*mut Asio, *mut i32, *mut i32) -> i32,
    pub buffer_size:
        unsafe extern "system" fn(*mut Asio, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    pub can_rate: unsafe extern "system" fn(*mut Asio, f64) -> i32,
    pub get_rate: unsafe extern "system" fn(*mut Asio, *mut f64) -> i32,
    pub set_rate: unsafe extern "system" fn(*mut Asio, f64) -> i32,
    pub clocks: unsafe extern "system" fn(*mut Asio, *mut ClockSource, *mut i32) -> i32,
    pub set_clock: unsafe extern "system" fn(*mut Asio, i32) -> i32,
    pub position: unsafe extern "system" fn(*mut Asio, *mut Samples, *mut Samples) -> i32,
    pub channel_info: unsafe extern "system" fn(*mut Asio, *mut ChannelInfo) -> i32,
    pub create:
        unsafe extern "system" fn(*mut Asio, *mut BufferInfo, i32, i32, *mut Callbacks) -> i32,
    pub dispose: unsafe extern "system" fn(*mut Asio) -> i32,
    pub panel: unsafe extern "system" fn(*mut Asio) -> i32,
    pub future: unsafe extern "system" fn(*mut Asio, i32, *mut c_void) -> i32,
    pub ready: unsafe extern "system" fn(*mut Asio) -> i32,
}
pub fn text(dst: &mut [c_char], s: &str) {
    dst.fill(0);
    let n = s.len().min(dst.len().saturating_sub(1));
    for (a, b) in dst.iter_mut().take(n).zip(s.bytes()) {
        *a = b as c_char;
    }
}
