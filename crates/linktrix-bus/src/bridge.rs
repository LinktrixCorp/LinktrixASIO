//! Exchange between the engine and one ASIO helper process
//! (`linktrix_asio_host.exe`), which hosts one hardware ASIO driver.
//!
//! Audio goes through shared memory, synchronously inside the driver's
//! callback: the helper publishes block `seq`'s input and signals `Request`;
//! the engine processes it, publishes the output and signals `Done`; the
//! helper hands that output to the driver before returning from the callback.
//! Blocks alternate between two slots (`seq % 2`) so a late engine never
//! reads input the helper is already overwriting. If the output does not
//! arrive within one buffer period the helper plays silence for that block
//! and counts a timeout.
//!
//! Control (load, start, control panel, reset requests) is a line protocol on
//! the helper's stdin/stdout; see docs/ASIO-HELPER-PROTOCOL.md.
//!
//! Samples are 24-bit integers in i32, interleaved with a stride of 64
//! channels: `[frame * 64 + channel]`.
use anyhow::{bail, Result};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use windows_sys::Win32::{
    Foundation::*,
    System::{Memory::*, Performance::*, Threading::*},
};

pub const CHANNELS: usize = 64;
pub const MAX_FRAMES: usize = 2048;
const MAGIC: u32 = 0x4C58_4231; // "LXB1"
const VERSION: u32 = 1;
/// No processor pinning.
pub const NO_PIN: u32 = u32::MAX;
/// Helper executable name, next to the application.
pub const HELPER: &str = "linktrix_asio_host.exe";
/// Device id that makes the helper run a timer-driven stand-in for a driver
/// (2 inputs, 2 outputs) to measure the exchange without hardware.
pub const SIMULATED: &str = "simulated";

#[repr(C, align(64))]
pub struct Header {
    magic: AtomicU32,
    version: AtomicU32,
    /// Frames per block, set by the helper before `ready`.
    pub frames: AtomicU32,
    /// Engine → helper: processor for the driver's callback thread,
    /// `(group << 16) | number`, or `NO_PIN`.
    pub pin: AtomicU32,
    /// Helper → engine: newest block whose input is published.
    pub request: AtomicU64,
    /// Engine → helper: newest block whose output is published.
    pub done: AtomicU64,
    /// Helper: `now_ns()` when the driver's callback for `request` began.
    pub entry_ns: AtomicU64,
    /// Helper: callback start → output received, for the newest block that
    /// received one.
    pub round_trip_ns: AtomicU64,
    /// Helper: blocks played as silence because the output was late.
    pub timeouts: AtomicU64,
}

#[repr(C)]
struct Shared {
    header: Header,
    input: [[i32; MAX_FRAMES * CHANNELS]; 2],
    output: [[i32; MAX_FRAMES * CHANNELS]; 2],
}

pub struct Bridge {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    /// Helper → engine: a block's input is ready (auto-reset).
    pub request: HANDLE,
    /// Engine → helper: a block's output is ready (auto-reset).
    pub done: HANDLE,
}
unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

fn name(key: &str, part: &str) -> Vec<u16> {
    crate::wide(&format!("Local\\Linktrix.AsioBridge.{key}.{part}"))
}

impl Bridge {
    /// Engine side: create the exchange for one device. `key` must be unique
    /// per device and process (for example `"<pid>.<slot>"`).
    pub fn create(key: &str) -> Result<Self> {
        let this = Self::map(key, true)?;
        let h = this.header();
        h.version.store(VERSION, Ordering::Relaxed);
        h.pin.store(NO_PIN, Ordering::Relaxed);
        h.magic.store(MAGIC, Ordering::Release);
        Ok(this)
    }
    /// Helper side: open the exchange the engine created.
    pub fn open(key: &str) -> Result<Self> {
        let this = Self::map(key, false)?;
        let h = this.header();
        if h.magic.load(Ordering::Acquire) != MAGIC || h.version.load(Ordering::Relaxed) != VERSION {
            bail!("ASIO bridge version mismatch");
        }
        Ok(this)
    }
    fn map(key: &str, create: bool) -> Result<Self> {
        unsafe {
            let size = std::mem::size_of::<Shared>();
            let mapping = if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    (size as u64 >> 32) as u32,
                    size as u32,
                    name(key, "Memory").as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name(key, "Memory").as_ptr())
            };
            if mapping.is_null() {
                bail!("ASIO bridge memory: {}", std::io::Error::last_os_error());
            }
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, size);
            if view.Value.is_null() {
                CloseHandle(mapping);
                bail!("Map ASIO bridge memory: {}", std::io::Error::last_os_error());
            }
            let event = |part| {
                if create {
                    CreateEventW(std::ptr::null(), 0, 0, name(key, part).as_ptr())
                } else {
                    OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE, 0, name(key, part).as_ptr())
                }
            };
            let request = event("Request");
            let done = event("Done");
            let this = Self { mapping, view, request, done };
            if request.is_null() || done.is_null() {
                bail!("ASIO bridge events: {}", std::io::Error::last_os_error());
            }
            Ok(this)
        }
    }
    fn shared(&self) -> *mut Shared {
        self.view.Value.cast()
    }
    pub fn header(&self) -> &Header {
        unsafe { &(*self.shared()).header }
    }
    /// The input of block `seq`, interleaved with a stride of 64.
    ///
    /// # Safety
    /// Only the helper writes it, before publishing `seq` in `request`; only
    /// the engine reads it, after observing `seq` there.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn input(&self, seq: u64, frames: usize) -> &mut [i32] {
        let slot = &mut (*self.shared()).input[(seq % 2) as usize];
        &mut slot[..frames.min(MAX_FRAMES) * CHANNELS]
    }
    /// The output of block `seq`, interleaved with a stride of 64.
    ///
    /// # Safety
    /// Only the engine writes it, before publishing `seq` in `done`; only the
    /// helper reads it, after observing `seq` there.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn output(&self, seq: u64, frames: usize) -> &mut [i32] {
        let slot = &mut (*self.shared()).output[(seq % 2) as usize];
        &mut slot[..frames.min(MAX_FRAMES) * CHANNELS]
    }
    /// Helper: publish block `seq` (input already written) and wait until the
    /// engine publishes its output or `deadline_ns` passes. True when the
    /// output arrived in time.
    pub fn exchange(&self, seq: u64, entry_ns: u64, deadline_ns: u64) -> bool {
        let h = self.header();
        h.entry_ns.store(entry_ns, Ordering::Relaxed);
        h.request.store(seq, Ordering::Release);
        unsafe { SetEvent(self.request) };
        loop {
            if h.done.load(Ordering::Acquire) >= seq {
                h.round_trip_ns.store(now_ns().saturating_sub(entry_ns), Ordering::Relaxed);
                return true;
            }
            let now = now_ns();
            if now >= deadline_ns {
                h.timeouts.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            // Millisecond waits: round up so a short buffer still gets its
            // whole period. A stale signal (an earlier, late block) loops.
            let wait = (deadline_ns - now).div_ceil(1_000_000) as u32;
            unsafe { WaitForSingleObject(self.done, wait) };
        }
    }
    /// Engine: wait up to `timeout_ms` for a block newer than `last`.
    /// Returns its sequence number.
    pub fn next_request(&self, last: u64, timeout_ms: u32) -> Option<u64> {
        let h = self.header();
        let seq = h.request.load(Ordering::Acquire);
        if seq > last {
            return Some(seq);
        }
        unsafe { WaitForSingleObject(self.request, timeout_ms) };
        let seq = h.request.load(Ordering::Acquire);
        (seq > last).then_some(seq)
    }
    /// Engine: block `seq`'s output is written.
    pub fn complete(&self, seq: u64) {
        self.header().done.store(seq, Ordering::Release);
        unsafe { SetEvent(self.done) };
    }
    /// Wake a thread blocked in `next_request` (engine shutdown).
    pub fn wake(&self) {
        unsafe { SetEvent(self.request) };
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        unsafe {
            if !self.request.is_null() {
                CloseHandle(self.request);
            }
            if !self.done.is_null() {
                CloseHandle(self.done);
            }
            UnmapViewOfFile(self.view);
            CloseHandle(self.mapping);
        }
    }
}

/// Monotonic nanoseconds, comparable across processes (QueryPerformanceCounter).
pub fn now_ns() -> u64 {
    static FREQUENCY: AtomicU64 = AtomicU64::new(0);
    let mut frequency = FREQUENCY.load(Ordering::Relaxed);
    if frequency == 0 {
        let mut f = 0i64;
        unsafe { QueryPerformanceFrequency(&mut f) };
        frequency = f.max(1) as u64;
        FREQUENCY.store(frequency, Ordering::Relaxed);
    }
    let mut counter = 0i64;
    unsafe { QueryPerformanceCounter(&mut counter) };
    (counter as u128 * 1_000_000_000 / frequency as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_and_helper_exchange_blocks_and_late_output_times_out() {
        let key = format!("test.{}", std::process::id());
        let engine = Bridge::create(&key).unwrap();
        let helper = Bridge::open(&key).unwrap();
        let worker = std::thread::spawn(move || {
            let mut last = 0;
            while last < 3 {
                if let Some(seq) = engine.next_request(last, 1000) {
                    unsafe {
                        let input = engine.input(seq, 4).to_vec();
                        for (o, i) in engine.output(seq, 4).iter_mut().zip(&input) {
                            *o = -i;
                        }
                    }
                    // Block 3 answers too late.
                    if seq == 3 {
                        std::thread::sleep(std::time::Duration::from_millis(30));
                    }
                    engine.complete(seq);
                    last = seq;
                }
            }
        });
        for seq in 1..=3u64 {
            unsafe {
                for (i, x) in helper.input(seq, 4).iter_mut().enumerate() {
                    *x = (seq as i32) * 1000 + i as i32;
                }
            }
            let entry = now_ns();
            let ok = helper.exchange(seq, entry, entry + 10_000_000);
            if seq < 3 {
                assert!(ok);
                let output = unsafe { helper.output(seq, 4) };
                assert_eq!(output[5], -((seq as i32) * 1000 + 5));
            } else {
                assert!(!ok);
            }
        }
        worker.join().unwrap();
        assert_eq!(helper.header().timeouts.load(Ordering::Relaxed), 1);
        assert!(helper.header().round_trip_ns.load(Ordering::Relaxed) > 0);
    }
}
