#![allow(clippy::missing_safety_doc)]
//! Linktrix Audio 64-input / 64-output ASIO COM server (x64). Windows only:
//! the crate is empty on other platforms.
//!
//! Copyright (c) 2026 Linktrix. Licensed under the GNU General Public License,
//! version 3 only (see COPYING). It exchanges audio with Linktrix Tools only
//! through the shared-memory bus of the MIT-licensed `linktrix-bus` crate.
#![cfg(windows)]
use std::{
    ffi::{c_char, c_void},
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};
use linktrix_asio::*;
use linktrix_bus::{
    bus::{Bus, Exclusive},
    pcm,
};
use windows_sys::{
    core::GUID,
    Win32::System::{Performance::*, Threading::*},
};
const E_NOINTERFACE: i32 = 0x80004002u32 as i32;
const E_POINTER: i32 = 0x80004003u32 as i32;
const E_CLASS: i32 = 0x80040111u32 as i32;
const IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
static OBJECTS: AtomicU32 = AtomicU32::new(0);
static LOCKS: AtomicU32 = AtomicU32::new(0);
fn eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

struct Buffer {
    channel: usize,
    input: bool,
    data: std::cell::UnsafeCell<Box<[u8]>>,
}
// Driver writes inputs; host writes outputs. Each half is reused at the next
// bufferSwitch deadline. Buffers stay owned until the worker has stopped.
unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}
struct State {
    frames: usize,
    buffers: Arc<Vec<Buffer>>,
    callbacks: Option<Callbacks>,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            frames: 128,
            buffers: Arc::new(vec![]),
            callbacks: None,
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}
#[repr(C)]
struct Driver {
    vtable: *const Vtable,
    refs: AtomicU32,
    state: Mutex<State>,
    position: Arc<AtomicU64>,
    stamp: Arc<AtomicU64>,
}
unsafe fn obj<'a>(p: *mut Asio) -> &'a Driver {
    &*p.cast::<Driver>()
}
fn make_driver() -> *mut Asio {
    OBJECTS.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(Driver {
        vtable: &VTABLE,
        refs: AtomicU32::new(1),
        state: Mutex::new(State::default()),
        position: Arc::new(AtomicU64::new(0)),
        stamp: Arc::new(AtomicU64::new(0)),
    }))
    .cast()
}
unsafe extern "system" fn query(p: *mut Asio, id: *const GUID, out: *mut *mut c_void) -> i32 {
    if out.is_null() || id.is_null() {
        return E_POINTER;
    }
    *out = null_mut();
    if eq(&*id, &IUNKNOWN) || eq(&*id, &CLSID) {
        *out = p.cast();
        add_ref(p);
        0
    } else {
        E_NOINTERFACE
    }
}
unsafe extern "system" fn add_ref(p: *mut Asio) -> u32 {
    obj(p).refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn release(p: *mut Asio) -> u32 {
    let n = obj(p).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        let d = Box::from_raw(p.cast::<Driver>());
        let mut s = d.state.lock().unwrap();
        s.stop.store(true, Ordering::Release);
        let worker = s.worker.take();
        drop(s);
        if let Some(t) = worker {
            if t.thread().id() != thread::current().id() {
                let _ = t.join();
            }
        }
        drop(d);
        OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
    n
}
unsafe extern "system" fn init(_: *mut Asio, _: *mut c_void) -> i32 {
    1
}
unsafe fn copy_text(p: *mut c_char, max: usize, s: &str) {
    if !p.is_null() {
        text(std::slice::from_raw_parts_mut(p, max), s);
    }
}
unsafe extern "system" fn name(_: *mut Asio, p: *mut c_char) {
    copy_text(p, 32, "Linktrix Audio 64 IO");
}
unsafe extern "system" fn version(_: *mut Asio) -> i32 {
    3
}
unsafe extern "system" fn error(_: *mut Asio, p: *mut c_char) {
    copy_text(
        p,
        124,
        "Start Linktrix Tools at 48 kHz. One ASIO host per Windows session is supported.",
    );
}
unsafe extern "system" fn channels(_: *mut Asio, i: *mut i32, o: *mut i32) -> i32 {
    if i.is_null() || o.is_null() {
        return INVALID;
    }
    *i = 64;
    *o = 64;
    OK
}
unsafe extern "system" fn latencies(p: *mut Asio, i: *mut i32, o: *mut i32) -> i32 {
    if i.is_null() || o.is_null() {
        return INVALID;
    }
    *i = obj(p).state.lock().unwrap().frames as i32;
    *o = *i;
    OK
}
unsafe extern "system" fn buffer_size(
    _: *mut Asio,
    min: *mut i32,
    max: *mut i32,
    pref: *mut i32,
    gran: *mut i32,
) -> i32 {
    if min.is_null() || max.is_null() || pref.is_null() || gran.is_null() {
        return INVALID;
    }
    *min = 32;
    *max = 2048;
    *pref = 128;
    *gran = -1;
    OK
}
unsafe extern "system" fn can_rate(_: *mut Asio, rate: f64) -> i32 {
    if rate == 48000.0 {
        OK
    } else {
        NO_CLOCK
    }
}
unsafe extern "system" fn get_rate(_: *mut Asio, rate: *mut f64) -> i32 {
    if rate.is_null() {
        return INVALID;
    }
    *rate = 48000.0;
    OK
}
unsafe extern "system" fn clocks(_: *mut Asio, c: *mut ClockSource, n: *mut i32) -> i32 {
    if c.is_null() || n.is_null() || *n < 1 {
        return INVALID;
    }
    *n = 1;
    *c = std::mem::zeroed();
    (*c).channel = -1;
    (*c).group = -1;
    (*c).current = 1;
    text(&mut (*c).name, "Linktrix Audio engine clock");
    OK
}
unsafe extern "system" fn set_clock(_: *mut Asio, n: i32) -> i32 {
    if n == 0 {
        OK
    } else {
        INVALID
    }
}
unsafe extern "system" fn position(p: *mut Asio, s: *mut Samples, t: *mut Samples) -> i32 {
    if s.is_null() || t.is_null() {
        return INVALID;
    }
    *s = obj(p).position.load(Ordering::Acquire).into();
    *t = obj(p).stamp.load(Ordering::Acquire).into();
    OK
}
unsafe extern "system" fn channel_info(p: *mut Asio, c: *mut ChannelInfo) -> i32 {
    if c.is_null() || !(0..=1).contains(&(*c).is_input) || !(0..64).contains(&(*c).channel) {
        return INVALID;
    }
    let s = obj(p).state.lock().unwrap();
    (*c).active = s
        .buffers
        .iter()
        .any(|b| b.channel == (*c).channel as usize && b.input == ((*c).is_input == 1))
        as i32;
    (*c).group = 0;
    (*c).kind = 17;
    text(
        &mut (*c).name,
        &format!(
            "Linktrix Audio {} {:02}",
            if (*c).is_input == 1 {
                "Input"
            } else {
                "Output"
            },
            (*c).channel + 1
        ),
    );
    OK
}
unsafe extern "system" fn create(
    p: *mut Asio,
    infos: *mut BufferInfo,
    n: i32,
    frames: i32,
    callbacks: *mut Callbacks,
) -> i32 {
    if infos.is_null()
        || callbacks.is_null()
        || !(1..=128).contains(&n)
        || !(32..=2048).contains(&frames)
        || !(frames as u32).is_power_of_two()
        || (*callbacks).switch.is_none()
    {
        return INVALID;
    }
    let mut s = obj(p).state.lock().unwrap();
    if s.worker.is_some() || !s.buffers.is_empty() {
        return BAD_MODE;
    }
    let infos = std::slice::from_raw_parts_mut(infos, n as usize);
    let mut seen = [0u64; 2];
    for b in infos.iter() {
        if !(0..=1).contains(&b.is_input) || !(0..64).contains(&b.channel) {
            return INVALID;
        }
        if seen[b.is_input as usize] & (1 << b.channel) != 0 {
            return INVALID;
        }
        seen[b.is_input as usize] |= 1 << b.channel;
    }
    let mut buffers = Vec::with_capacity(n as usize);
    for b in infos {
        let mut data = vec![0u8; frames as usize * 3 * 2].into_boxed_slice();
        b.buffers = [
            data.as_mut_ptr().cast(),
            data.as_mut_ptr().add(frames as usize * 3).cast(),
        ];
        buffers.push(Buffer {
            channel: b.channel as usize,
            input: b.is_input == 1,
            data: std::cell::UnsafeCell::new(data),
        });
    }
    s.frames = frames as usize;
    s.buffers = Arc::new(buffers);
    s.callbacks = Some(*callbacks);
    OK
}
unsafe extern "system" fn start(p: *mut Asio) -> i32 {
    let d = obj(p);
    let mut s = d.state.lock().unwrap();
    if s.worker.is_some() || s.buffers.is_empty() {
        return BAD_MODE;
    }
    let bus = match Bus::open() {
        Ok(b) if b.running() => b,
        _ => return NO_CLOCK,
    };
    let output_bus = match Bus::open_output() {
        Ok(b) => b,
        Err(_) => return -994,
    };
    let stop = s.stop.clone();
    stop.store(false, Ordering::Release);
    let buffers = s.buffers.clone();
    let frames = s.frames;
    let callbacks = s.callbacks.unwrap();
    let pos = d.position.clone();
    let stamp = d.stamp.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // Prevent unloading the DLL while its worker still executes, including a
    // host's unusual final Release call from inside bufferSwitch.
    OBJECTS.fetch_add(1, Ordering::Relaxed);
    let worker = match thread::Builder::new()
        .name("Linktrix Audio ASIO duplex".into())
        .spawn(move || {
            let _claim = match Exclusive::acquire("Local\\Linktrix.WAT.InputHost.v1") {
                Ok(c) => c,
                Err(_) => {
                    let _ = tx.send(false);
                    OBJECTS.fetch_sub(1, Ordering::Release);
                    return;
                }
            };
            let mut task = 0;
            let mm = AvSetMmThreadCharacteristicsW(linktrix_bus::wide("Pro Audio").as_ptr(), &mut task);
            if !mm.is_null() {
                AvSetMmThreadPriority(mm, AVRT_PRIORITY_CRITICAL);
            }
            let mut cursor = bus.written();
            let mut output = vec![0i32; frames * 64];
            output_bus.set_running(true);
            let _ = tx.send(true);
            let mut index = 0usize;
            let mut total = 0u64;
            let mut samples = [0i32; 64];
            let mut last_progress = std::time::Instant::now();
            let mut frequency = 0;
            QueryPerformanceFrequency(&mut frequency);
            while !stop.load(Ordering::Acquire) {
                WaitForSingleObject(bus.tick, 100);
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if !bus.running() {
                    if let Some(msg) = callbacks.message {
                        msg(3, 0, null_mut(), null_mut());
                    }
                    break;
                }
                let written = bus.written();
                if written > cursor {
                    last_progress = std::time::Instant::now();
                }
                if last_progress.elapsed() > std::time::Duration::from_secs(2) {
                    if let Some(msg) = callbacks.message {
                        msg(3, 0, null_mut(), null_mut());
                    }
                    break;
                }
                if written.saturating_sub(cursor) > Bus::capacity() {
                    if let Some(msg) = callbacks.message {
                        msg(5, 0, null_mut(), null_mut());
                    }
                    break;
                }
                while written.saturating_sub(cursor) >= frames as u64
                    && !stop.load(Ordering::Acquire)
                {
                    // The opposite half was given to the host at the previous
                    // callback. Its processing deadline is this buffer switch.
                    output.fill(0);
                    for b in buffers.iter().filter(|b| !b.input) {
                        let data = &mut *b.data.get();
                        let half =
                            &mut data[((index ^ 1) * frames * 3)..((index ^ 1) + 1) * frames * 3];
                        for f in 0..frames {
                            output[f * 64 + b.channel] = pcm::unpack24(&half[f * 3..]);
                        }
                        half.fill(0);
                    }
                    output_bus.write(&output);
                    for f in 0..frames {
                        bus.read_frame(cursor + f as u64, &mut samples);
                        for b in buffers.iter().filter(|b| b.input) {
                            let data = &mut *b.data.get();
                            pcm::pack24(samples[b.channel], &mut data[(index * frames + f) * 3..]);
                        }
                    }
                    let mut counter = 0;
                    QueryPerformanceCounter(&mut counter);
                    stamp.store(
                        ((counter as u128 * 1_000_000_000) / frequency as u128) as u64,
                        Ordering::Release,
                    );
                    pos.store(total, Ordering::Release);
                    if let Some(switch) = callbacks.switch {
                        switch(index as i32, 1);
                    }
                    cursor += frames as u64;
                    total += frames as u64;
                    index ^= 1;
                }
            }
            output_bus.set_running(false);
            if !mm.is_null() {
                AvRevertMmThreadCharacteristics(mm);
            }
            OBJECTS.fetch_sub(1, Ordering::Release);
        }) {
        Ok(t) => t,
        Err(_) => {
            OBJECTS.fetch_sub(1, Ordering::Release);
            return -994;
        }
    };
    match rx.recv() {
        Ok(true) => {
            s.worker = Some(worker);
            OK
        }
        _ => {
            let _ = worker.join();
            BAD_MODE
        }
    }
}
unsafe extern "system" fn stop(p: *mut Asio) -> i32 {
    let mut s = obj(p).state.lock().unwrap();
    if s.worker
        .as_ref()
        .is_some_and(|t| t.thread().id() == thread::current().id())
    {
        return BAD_MODE;
    }
    s.stop.store(true, Ordering::Release);
    let worker = s.worker.take();
    drop(s);
    if let Some(t) = worker {
        let _ = t.join();
    }
    // The host may prefill output halves before the next ASIOStart. Clear here,
    // after callbacks have stopped, rather than discarding that prefill at start.
    for b in obj(p).state.lock().unwrap().buffers.iter() {
        (&mut *b.data.get()).fill(0);
    }
    OK
}
unsafe extern "system" fn dispose(p: *mut Asio) -> i32 {
    let e = stop(p);
    if e != OK {
        return e;
    }
    let mut s = obj(p).state.lock().unwrap();
    s.buffers = Arc::new(vec![]);
    s.callbacks = None;
    OK
}
unsafe extern "system" fn panel(_: *mut Asio) -> i32 {
    NOT_PRESENT
}
unsafe extern "system" fn future(_: *mut Asio, _: i32, _: *mut c_void) -> i32 {
    NOT_PRESENT
}
unsafe extern "system" fn ready(_: *mut Asio) -> i32 {
    NOT_PRESENT
}
static VTABLE: Vtable = Vtable {
    query,
    add_ref,
    release,
    init,
    name,
    version,
    error,
    start,
    stop,
    channels,
    latencies,
    buffer_size,
    can_rate,
    get_rate,
    set_rate: can_rate,
    clocks,
    set_clock,
    position,
    channel_info,
    create,
    dispose,
    panel,
    future,
    ready,
};

#[repr(C)]
struct Factory {
    table: *const FactoryVtable,
    refs: AtomicU32,
}
#[repr(C)]
struct FactoryVtable {
    query: unsafe extern "system" fn(*mut Factory, *const GUID, *mut *mut c_void) -> i32,
    add: unsafe extern "system" fn(*mut Factory) -> u32,
    release: unsafe extern "system" fn(*mut Factory) -> u32,
    create:
        unsafe extern "system" fn(*mut Factory, *mut c_void, *const GUID, *mut *mut c_void) -> i32,
    lock: unsafe extern "system" fn(*mut Factory, i32) -> i32,
}
unsafe extern "system" fn factory_query(
    f: *mut Factory,
    i: *const GUID,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() || i.is_null() {
        return E_POINTER;
    }
    *out = null_mut();
    if eq(&*i, &IUNKNOWN) || eq(&*i, &IFACTORY) {
        *out = f.cast();
        factory_add(f);
        0
    } else {
        E_NOINTERFACE
    }
}
unsafe extern "system" fn factory_add(f: *mut Factory) -> u32 {
    (*f).refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn factory_release(f: *mut Factory) -> u32 {
    let n = (*f).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        drop(Box::from_raw(f));
        OBJECTS.fetch_sub(1, Ordering::Release);
    }
    n
}
unsafe extern "system" fn factory_create(
    _: *mut Factory,
    outer: *mut c_void,
    id: *const GUID,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() || id.is_null() {
        return E_POINTER;
    }
    *out = null_mut();
    if !outer.is_null() {
        return 0x80040110u32 as i32;
    }
    let d = make_driver();
    let hr = query(d, id, out);
    release(d);
    hr
}
unsafe extern "system" fn factory_lock(_: *mut Factory, lock: i32) -> i32 {
    if lock != 0 {
        LOCKS.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = LOCKS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(1));
    }
    0
}
static FACTORY: FactoryVtable = FactoryVtable {
    query: factory_query,
    add: factory_add,
    release: factory_release,
    create: factory_create,
    lock: factory_lock,
};
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() || clsid.is_null() || iid.is_null() {
        return E_POINTER;
    }
    *out = null_mut();
    if !eq(&*clsid, &CLSID) {
        return E_CLASS;
    }
    OBJECTS.fetch_add(1, Ordering::Relaxed);
    let f = Box::into_raw(Box::new(Factory {
        table: &FACTORY,
        refs: AtomicU32::new(1),
    }));
    let hr = factory_query(f, iid, out);
    factory_release(f);
    hr
}
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> i32 {
    if OBJECTS.load(Ordering::Acquire) == 0 && LOCKS.load(Ordering::Acquire) == 0 {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_BUFFERS: std::sync::atomic::AtomicPtr<BufferInfo> =
        std::sync::atomic::AtomicPtr::new(null_mut());
    static TEST_SWITCHES: AtomicU32 = AtomicU32::new(0);
    static TEST_ERRORS: AtomicU32 = AtomicU32::new(0);
    unsafe extern "C" fn test_switch(index: i32, _: i32) {
        let infos = std::slice::from_raw_parts(TEST_BUFFERS.load(Ordering::Acquire), 128);
        for (c, b) in infos.iter().enumerate() {
            let channel = c % 64;
            let data =
                std::slice::from_raw_parts_mut(b.buffers[index as usize].cast::<u8>(), 128 * 3);
            let expected = if channel % 2 == 0 {
                -7_000_000 + channel as i32
            } else {
                7_000_000 - channel as i32
            };
            for f in 0..128 {
                if b.is_input == 0 {
                    pcm::pack24(-expected + f as i32, &mut data[f * 3..]);
                } else if pcm::unpack24(&data[f * 3..]) != expected {
                    TEST_ERRORS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        TEST_SWITCHES.fetch_add(1, Ordering::Release);
    }
    #[test]
    fn shared_bus_duplex_64_channels_restart_and_wraparound() {
        let _producer = Exclusive::acquire("Local\\Linktrix.WAT.Engine.v1")
            .expect("Stop the WAT audio engine before tests");
        let bus = Bus::open().unwrap();
        let reader = Bus::open().unwrap();
        let output_bus = Bus::open_output().unwrap();
        let mut reblock = linktrix_bus::bus::BusReader::new(Bus::open_output().unwrap());
        let samples: Vec<i32> = (0usize..128 * 64)
            .map(|i| {
                let c = i % 64;
                if c % 2 == 0 {
                    -7_000_000 + c as i32
                } else {
                    7_000_000 - c as i32
                }
            })
            .collect();
        unsafe {
            bus.set_running(true);
            let p = make_driver();
            let mut infos: Vec<BufferInfo> = (0..128)
                .map(|c| BufferInfo {
                    is_input: (c < 64) as i32,
                    channel: c % 64,
                    buffers: [null_mut(); 2],
                })
                .collect();
            let mut callbacks = Callbacks {
                switch: Some(test_switch),
                rate: None,
                message: None,
                time: None,
            };
            assert_eq!(create(p, infos.as_mut_ptr(), 128, 128, &mut callbacks), OK);
            TEST_BUFFERS.store(infos.as_mut_ptr(), Ordering::Release);
            TEST_ERRORS.store(0, Ordering::Release);
            TEST_SWITCHES.store(0, Ordering::Release);
            assert_eq!(start(p), OK);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while TEST_SWITCHES.load(Ordering::Acquire) < 8 && std::time::Instant::now() < deadline
            {
                bus.write(&samples);
                thread::sleep(std::time::Duration::from_millis(4));
            }
            assert_eq!(stop(p), OK);
            let mut frame = [0; 64];
            assert!(output_bus.read_frame(output_bus.written() - 1, &mut frame));
            for c in 0..64 {
                assert_eq!(frame[c], -samples[c] + 127);
            }
            let mut queued = vec![1; 64 * 32];
            assert_eq!(reblock.read(&mut queued), 0);
            assert!(queued.iter().all(|x| *x == 0));
            // First output half after restart is silent, not a stale DAW block.
            TEST_SWITCHES.store(0, Ordering::Release);
            assert_eq!(start(p), OK);
            bus.write(&samples);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while TEST_SWITCHES.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline
            {
                thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(reblock.read(&mut queued), 32);
            assert!(queued.iter().all(|x| *x == 0));
            assert_eq!(stop(p), OK);
            assert_eq!(dispose(p), OK);
            release(p);
            bus.set_running(false);
            assert!(TEST_SWITCHES.load(Ordering::Acquire) >= 1);
            assert_eq!(TEST_ERRORS.load(Ordering::Acquire), 0);
        }
        let stale = bus.written();
        bus.write(&samples);
        for _ in 0..65 {
            bus.write(&samples);
        }
        let mut frame = [1; 64];
        assert!(!reader.read_frame(stale, &mut frame));
        assert_eq!(frame, [0; 64]);
        assert!(reader.read_frame(bus.written() - 1, &mut frame));
        assert_eq!(&frame[..], &samples[..64]);
    }
    #[test]
    fn duplex_driver_contract_and_sparse_output_buffers() {
        unsafe {
            let p = make_driver();
            let (mut i, mut o) = (0, 0);
            assert_eq!(channels(p, &mut i, &mut o), 0);
            assert_eq!((i, o), (64, 64));
            assert_eq!(can_rate(p, 44100.0), NO_CLOCK);
            assert_eq!(can_rate(p, 48000.0), OK);
            let mut c: ChannelInfo = std::mem::zeroed();
            c.channel = 63;
            c.is_input = 1;
            assert_eq!(channel_info(p, &mut c), OK);
            assert_eq!(c.kind, 17);
            c.is_input = 0;
            assert_eq!(channel_info(p, &mut c), OK);
            let mut callbacks = Callbacks {
                switch: Some(test_switch),
                rate: None,
                message: None,
                time: None,
            };
            let mut buffers = [BufferInfo {
                is_input: 0,
                channel: 63,
                buffers: [null_mut(); 2],
            }; 2];
            assert_eq!(
                create(p, buffers.as_mut_ptr(), 2, 128, &mut callbacks),
                INVALID
            );
            assert_eq!(create(p, buffers.as_mut_ptr(), 1, 128, &mut callbacks), OK);
            assert_eq!(channel_info(p, &mut c), OK);
            assert_eq!(c.active, 1);
            c.is_input = 1;
            assert_eq!(channel_info(p, &mut c), OK);
            assert_eq!(c.active, 0);
            assert_eq!(dispose(p), OK);
            c.channel = 64;
            assert_eq!(channel_info(p, &mut c), INVALID);
            release(p);
        }
    }
}
