use anyhow::{bail, Result};
use std::{
    ptr::null,
    sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    System::{Memory::*, Threading::*},
};
const CAP: usize = 8192;
#[repr(C, align(64))]
struct Frame {
    stamp: AtomicU64,
    samples: [AtomicI32; 64],
}
#[repr(C)]
struct Shared {
    magic: AtomicU32,
    running: AtomicU32,
    written: AtomicU64,
    generation: AtomicU64,
    stream_start: AtomicU64,
    frames: [Frame; CAP],
}
pub struct Bus {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    pub tick: HANDLE,
}
unsafe impl Send for Bus {}
unsafe impl Sync for Bus {}
impl Bus {
    pub fn open() -> Result<Self> {
        Self::open_named("Capture")
    }
    /// DAW outputs -> routing engine; independent from the engine -> DAW bus.
    pub fn open_output() -> Result<Self> {
        Self::open_named("Playback")
    }
    fn open_named(direction: &str) -> Result<Self> {
        unsafe {
            let security = BusSecurity::new()?;
            let mapping = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &security.attributes,
                PAGE_READWRITE,
                0,
                std::mem::size_of::<Shared>() as u32,
                crate::wide(&format!("Local\\Linktrix.WAT.{direction}.Bus.v2")).as_ptr(),
            );
            if mapping.is_null() {
                bail!("Shared bus: {}", std::io::Error::last_os_error());
            }
            let view = MapViewOfFile(
                mapping,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<Shared>(),
            );
            if view.Value.is_null() {
                CloseHandle(mapping);
                bail!("Map audio bus failed");
            }
            let tick = CreateEventW(
                &security.attributes,
                0,
                0,
                crate::wide(&format!("Local\\Linktrix.WAT.{direction}.Tick.v2")).as_ptr(),
            );
            if tick.is_null() {
                UnmapViewOfFile(view);
                CloseHandle(mapping);
                bail!("Create bus event failed");
            }
            let this = Self {
                mapping,
                view,
                tick,
            };
            this.shared()
                .magic
                .compare_exchange(0, 0x57415431, Ordering::SeqCst, Ordering::SeqCst)
                .ok();
            if this.shared().magic.load(Ordering::Acquire) != 0x57415431 {
                bail!("Audio bus version mismatch");
            }
            Ok(this)
        }
    }
    fn shared(&self) -> &Shared {
        unsafe { &*self.view.Value.cast::<Shared>() }
    }
    pub fn running(&self) -> bool {
        self.shared().running.load(Ordering::Acquire) != 0
    }
    pub fn set_running(&self, v: bool) {
        if v {
            self.shared()
                .stream_start
                .store(self.written(), Ordering::Release);
            self.shared().generation.fetch_add(1, Ordering::AcqRel);
        }
        self.shared().running.store(v as u32, Ordering::Release);
        unsafe {
            SetEvent(self.tick);
        }
    }
    pub fn written(&self) -> u64 {
        self.shared().written.load(Ordering::Acquire)
    }
    // Single producer, enforced by the engine's named mutex. All shared fields are atomic.
    pub fn write(&self, data: &[i32]) {
        let s = self.shared();
        let start = s.written.load(Ordering::Relaxed);
        for (i, src) in data.as_chunks::<64>().0.iter().enumerate() {
            let n = start + i as u64;
            let f = &s.frames[n as usize % CAP];
            f.stamp.store(n * 2 + 1, Ordering::SeqCst);
            for (a, b) in f.samples.iter().zip(src) {
                a.store(*b, Ordering::SeqCst);
            }
            f.stamp.store(n * 2 + 2, Ordering::SeqCst);
        }
        s.written
            .store(start + (data.len() / 64) as u64, Ordering::Release);
        unsafe {
            SetEvent(self.tick);
        }
    }
    pub fn read_frame(&self, n: u64, dst: &mut [i32; 64]) -> bool {
        let f = &self.shared().frames[n as usize % CAP];
        let expected = n * 2 + 2;
        if f.stamp.load(Ordering::SeqCst) != expected {
            dst.fill(0);
            return false;
        }
        for (a, b) in dst.iter_mut().zip(&f.samples) {
            *a = b.load(Ordering::SeqCst);
        }
        if f.stamp.load(Ordering::SeqCst) != expected {
            dst.fill(0);
            return false;
        }
        true
    }
    pub fn capacity() -> u64 {
        CAP as u64
    }
}
/// Reblocks one clock-synchronous stream without replaying old samples after stop/start.
pub struct BusReader {
    bus: Bus,
    cursor: u64,
    generation: u64,
}
impl BusReader {
    pub fn new(bus: Bus) -> Self {
        Self {
            cursor: bus.written(),
            generation: bus.shared().generation.load(Ordering::Acquire),
            bus,
        }
    }
    pub fn read(&mut self, out: &mut [i32]) -> usize {
        out.fill(0);
        if !self.bus.running() {
            return 0;
        }
        let generation = self.bus.shared().generation.load(Ordering::Acquire);
        if generation != self.generation {
            self.cursor = self.bus.shared().stream_start.load(Ordering::Acquire);
            self.generation = generation;
        }
        let written = self.bus.written();
        if written.saturating_sub(self.cursor) > Bus::capacity() {
            self.cursor = written.saturating_sub((out.len() / 64) as u64);
        }
        let count = (written.saturating_sub(self.cursor) as usize).min(out.len() / 64);
        for (f, frame) in out
            .as_chunks_mut::<64>()
            .0
            .iter_mut()
            .take(count)
            .enumerate()
        {
            self.bus.read_frame(self.cursor + f as u64, frame);
        }
        self.cursor += count as u64;
        if !self.bus.running() || self.bus.shared().generation.load(Ordering::Acquire) != generation
        {
            out.fill(0);
            return 0;
        }
        count
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.tick);
            UnmapViewOfFile(self.view);
            CloseHandle(self.mapping);
        }
    }
}
// Explicit current-user ACL + medium integrity lets an elevated WDM manager
// communicate with that same user's unelevated DAW, without granting Everyone.
struct BusSecurity {
    attributes: SECURITY_ATTRIBUTES,
}
impl BusSecurity {
    fn new() -> Result<Self> {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                bail!("Read audio session identity failed");
            }
            let mut required = 0;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required);
            let mut storage = vec![0u64; (required as usize).div_ceil(8)];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                storage.as_mut_ptr().cast(),
                required,
                &mut required,
            );
            CloseHandle(token);
            if ok == 0 {
                bail!("Read audio session SID failed");
            }
            let user = &*storage.as_ptr().cast::<TOKEN_USER>();
            let mut sid = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
                bail!("Format audio session SID failed");
            }
            let mut len = 0;
            while *sid.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, len));
            LocalFree(sid.cast());
            let sddl = crate::wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{text})S:(ML;;NW;;;ME)"));
            let mut descriptor = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                bail!("Create audio bus security descriptor failed");
            }
            Ok(Self {
                attributes: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                },
            })
        }
    }
}
impl Drop for BusSecurity {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.attributes.lpSecurityDescriptor);
        }
    }
}
pub struct Exclusive(HANDLE);
impl Exclusive {
    pub fn acquire(name: &str) -> Result<Self> {
        unsafe {
            let h = CreateMutexW(null(), 0, crate::wide(name).as_ptr());
            if h.is_null() {
                bail!("Create ownership mutex failed");
            }
            let status = WaitForSingleObject(h, 0);
            if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
                CloseHandle(h);
                bail!("Another process already owns {name}");
            }
            Ok(Self(h))
        }
    }
}
// Must be created and dropped on its owning thread.
impl Drop for Exclusive {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
