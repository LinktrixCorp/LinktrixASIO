//! Hosts one hardware ASIO driver for Linktrix Tools, in its own process.
//!
//! Copyright (c) 2026 Linktrix. Licensed under the GNU General Public License,
//! version 3 only (see COPYING). It talks to the application only through the
//! documented protocol in docs/ASIO-HELPER-PROTOCOL.md: the shared-memory
//! block exchange of the MIT `linktrix-bus` crate, and text lines on
//! stdin/stdout. Any program speaking that protocol can use it.
//!
//! Usage: `linktrix_asio_host <key> <driver CLSID | simulated> <frames> <primary|secondary>`
//! A secondary device may fall back to the driver's preferred buffer size.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("linktrix_asio_host runs on Windows only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(e) = host::run() {
        println!("error {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod host {
    use linktrix_asio::*;
    use linktrix_bus::{
        bridge::{now_ns, Bridge, NO_PIN, SIMULATED},
        pcm,
    };
    use std::{
        ffi::c_void,
        io::{BufRead, Write},
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering},
            mpsc,
        },
        time::Duration,
    };
    use windows_sys::{
        core::GUID,
        Win32::{
            Foundation::*,
            System::{Com::*, Kernel::PROCESSOR_NUMBER, SystemInformation::GROUP_AFFINITY, Threading::*},
            UI::WindowsAndMessaging::*,
        },
    };

    type Result<T> = std::result::Result<T, String>;

    /// The one device this process hosts, read by the driver's callbacks.
    struct Device {
        bridge: Bridge,
        frames: usize,
        period_ns: u64,
        /// The driver's buffers (both halves) and each one's sample container.
        buffers: Vec<BufferInfo>,
        kinds: Vec<i32>,
        driver: *mut Asio,
        /// The driver's buffers exist (dispose them before release).
        created: bool,
        seq: AtomicU64,
        pinned: AtomicU32,
    }
    static DEVICE: AtomicPtr<Device> = AtomicPtr::new(null_mut());
    static BUSY: AtomicBool = AtomicBool::new(false);
    /// The driver asked to be reset (or changed rate); reported on stdout.
    static RESET: AtomicBool = AtomicBool::new(false);

    impl Device {
        /// One driver callback: hand the input to the engine, wait for its
        /// output (at most one buffer period) and give it to the driver.
        unsafe fn switch(&self, index: usize) {
            let entry = now_ns();
            self.apply_pin();
            let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
            let frames = self.frames;
            let input = self.bridge.input(seq, frames);
            for (b, kind) in self.buffers.iter().zip(&self.kinds).filter(|(b, _)| b.is_input != 0) {
                let width = pcm::sample_bytes(*kind).unwrap();
                let src = std::slice::from_raw_parts(b.buffers[index].cast::<u8>(), frames * width);
                for f in 0..frames {
                    input[f * 64 + b.channel as usize] = pcm::decode(*kind, &src[f * width..]);
                }
            }
            let ok = self.bridge.exchange(seq, entry, entry + self.period_ns);
            let output = self.bridge.output(seq, frames);
            for (b, kind) in self.buffers.iter().zip(&self.kinds).filter(|(b, _)| b.is_input == 0) {
                let width = pcm::sample_bytes(*kind).unwrap();
                let dst = std::slice::from_raw_parts_mut(b.buffers[index].cast::<u8>(), frames * width);
                for f in 0..frames {
                    let x = if ok { output[f * 64 + b.channel as usize] } else { 0 };
                    pcm::encode(*kind, x, &mut dst[f * width..]);
                }
            }
            if !self.driver.is_null() {
                ((*(*self.driver).vtable).ready)(self.driver);
            }
        }
        /// Follow the engine's processor choice for the driver's thread.
        unsafe fn apply_pin(&self) {
            let wanted = self.bridge.header().pin.load(Ordering::Relaxed);
            if wanted == self.pinned.swap(wanted, Ordering::Relaxed) {
                return;
            }
            let thread = GetCurrentThread();
            if wanted == NO_PIN {
                let (mut process, mut system) = (0usize, 0usize);
                if GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) != 0 && process != 0 {
                    SetThreadAffinityMask(thread, process);
                }
            } else {
                let (group, number) = ((wanted >> 16) as u16, (wanted & 0xFF) as u8);
                let affinity = GROUP_AFFINITY { Mask: 1usize << number, Group: group, Reserved: [0; 3] };
                SetThreadGroupAffinity(thread, &affinity, null_mut());
                let ideal = PROCESSOR_NUMBER { Group: group, Number: number, Reserved: 0 };
                SetThreadIdealProcessorEx(thread, &ideal, null_mut());
            }
        }
    }

    thread_local! {
        /// This thread already runs in the "Pro Audio" MMCSS class.
        static PRO_AUDIO: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    /// Raise the calling thread into the "Pro Audio" MMCSS class (once). Many
    /// drivers call back on a plain time-critical thread (priority 15), which
    /// other programs' time-critical threads preempt during a load spike; the
    /// engine's own thread is already MMCSS and waits on this one.
    unsafe fn ensure_pro_audio() {
        if PRO_AUDIO.with(|p| p.replace(true)) {
            return;
        }
        let mut task = 0;
        let mm = AvSetMmThreadCharacteristicsW(linktrix_bus::wide("Pro Audio").as_ptr(), &mut task);
        if !mm.is_null() {
            AvSetMmThreadPriority(mm, AVRT_PRIORITY_CRITICAL);
        }
    }
    unsafe extern "C" fn buffer_switch(index: i32, _: i32) {
        if !(0..=1).contains(&index) || BUSY.swap(true, Ordering::Acquire) {
            return;
        }
        ensure_pro_audio();
        let device = DEVICE.load(Ordering::Acquire);
        if !device.is_null() {
            (*device).switch(index as usize);
        }
        BUSY.store(false, Ordering::Release);
    }
    unsafe extern "C" fn rate_changed(rate: f64) {
        if rate != 48000.0 {
            RESET.store(true, Ordering::Release);
        }
    }
    unsafe extern "C" fn message(selector: i32, value: i32, _: *mut c_void, _: *mut f64) -> i32 {
        match selector {
            // Supported selectors: reset, resync and latency-change requests.
            1 => matches!(value, 2 | 3 | 5 | 6) as i32,
            // Host ASIO version.
            2 => 2,
            3 | 5 | 6 => {
                RESET.store(true, Ordering::Release);
                1
            }
            _ => 0,
        }
    }
    fn check(code: i32, context: &str) -> Result<()> {
        if code != OK {
            return Err(format!("{context}: ASIO error {code}"));
        }
        Ok(())
    }

    /// What `ready` reports.
    struct Loaded {
        inputs: usize,
        outputs: usize,
        input_latency: i32,
        output_latency: i32,
        callbacks: Box<Callbacks>,
    }

    /// Load and prepare the driver: 48 kHz, buffers for up to 64 inputs and
    /// 64 outputs. Same rules the engine used when it hosted drivers itself.
    unsafe fn load(id: &str, requested: i32, secondary: bool, device: &mut Device) -> Result<Loaded> {
        if id.eq_ignore_ascii_case(CLSID_TEXT) {
            return Err("The Linktrix Audio input driver cannot be used as hardware".into());
        }
        let mut clsid: GUID = std::mem::zeroed();
        if CLSIDFromString(linktrix_bus::wide(id).as_ptr(), &mut clsid) < 0 {
            return Err(format!("Invalid ASIO class ID: {id}"));
        }
        let mut raw = null_mut();
        let hr = CoCreateInstance(&clsid, null_mut(), CLSCTX_INPROC_SERVER, &clsid, &mut raw);
        if hr < 0 {
            return Err(format!("Load ASIO {id}: 0x{:08X}", hr as u32));
        }
        let driver = raw.cast::<Asio>();
        device.driver = driver;
        let v = &*(*driver).vtable;
        if (v.init)(driver, null_mut()) == 0 {
            let mut err = [0i8; 124];
            (v.error)(driver, err.as_mut_ptr());
            err[123] = 0;
            return Err(format!(
                "Initialize ASIO {id}: {}",
                std::ffi::CStr::from_ptr(err.as_ptr()).to_string_lossy()
            ));
        }
        check((v.can_rate)(driver, 48000.0), "48 kHz support")?;
        check((v.set_rate)(driver, 48000.0), "Set 48 kHz")?;
        let mut rate = 0.0;
        check((v.get_rate)(driver, &mut rate), "Read rate")?;
        if rate != 48000.0 {
            return Err("ASIO device did not switch to 48 kHz".into());
        }
        let (mut input, mut output) = (0, 0);
        check((v.channels)(driver, &mut input, &mut output), "Read channels")?;
        if !(0..=4096).contains(&input) || !(0..=4096).contains(&output) || input + output == 0 {
            return Err("Invalid channel count".into());
        }
        let (mut min, mut max, mut preferred, mut gran) = (0, 0, 0, 0);
        check((v.buffer_size)(driver, &mut min, &mut max, &mut preferred, &mut gran), "Read buffer sizes")?;
        let frames = if buffer_allowed(requested, min, max, gran) {
            requested
        } else if secondary && buffer_allowed(preferred, min, max, gran) {
            preferred
        } else {
            return Err(format!(
                "ASIO {id} buffer {requested} unsupported; min {min}, max {max}, preferred {preferred}, granularity {gran}"
            ));
        };
        device.frames = frames as usize;
        for is_input in [1, 0] {
            for channel in 0..(if is_input == 1 { input } else { output }).min(64) {
                let mut info: ChannelInfo = std::mem::zeroed();
                info.channel = channel;
                info.is_input = is_input;
                check((v.channel_info)(driver, &mut info), "Read ASIO format")?;
                if pcm::sample_bytes(info.kind).is_none() {
                    return Err(format!("Unsupported 24-bit-capable ASIO container {}", info.kind));
                }
                device.kinds.push(info.kind);
                device.buffers.push(BufferInfo { is_input, channel, buffers: [null_mut(); 2] });
            }
        }
        let mut callbacks = Box::new(Callbacks {
            switch: Some(buffer_switch),
            rate: Some(rate_changed),
            message: Some(message),
            time: None,
        });
        check(
            (v.create)(driver, device.buffers.as_mut_ptr(), device.buffers.len() as i32, frames, &mut *callbacks),
            "Create ASIO buffers",
        )?;
        device.created = true;
        if device.buffers.iter().any(|b| b.buffers.iter().any(|p| p.is_null())) {
            return Err("ASIO returned a null audio buffer".into());
        }
        let (mut input_latency, mut output_latency) = (-1, -1);
        if (v.latencies)(driver, &mut input_latency, &mut output_latency) != OK {
            (input_latency, output_latency) = (-1, -1);
        }
        Ok(Loaded {
            inputs: input.min(64) as usize,
            outputs: output.min(64) as usize,
            input_latency,
            output_latency,
            callbacks,
        })
    }

    /// Stand-in driver: a high-resolution timer calls the same callback path
    /// once per period, with 2 inputs and 2 outputs in memory.
    struct Simulated {
        stop: std::sync::Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    fn simulate(device: &mut Device, requested: i32) -> Result<(Loaded, Vec<Box<[u8]>>)> {
        if !buffer_allowed(requested, 16, 2048, -1) {
            return Err(format!("Simulated buffer {requested} unsupported"));
        }
        device.frames = requested as usize;
        let mut memory = vec![];
        for (is_input, channel) in [(1, 0), (1, 1), (0, 0), (0, 1)] {
            let mut data = vec![0u8; device.frames * 3 * 2].into_boxed_slice();
            let base = data.as_mut_ptr();
            device.kinds.push(17);
            device.buffers.push(BufferInfo {
                is_input,
                channel,
                buffers: unsafe { [base.cast(), base.add(device.frames * 3).cast()] },
            });
            memory.push(data);
        }
        let loaded = Loaded {
            inputs: 2,
            outputs: 2,
            input_latency: requested,
            output_latency: requested,
            callbacks: Box::new(Callbacks { switch: None, rate: None, message: None, time: None }),
        };
        Ok((loaded, memory))
    }
    fn start_simulated(period: Duration) -> Simulated {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("simulated ASIO driver".into())
            .spawn(move || unsafe {
                let timer = CreateWaitableTimerExW(null(), null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS);
                let mut task = 0;
                let mm = AvSetMmThreadCharacteristicsW(linktrix_bus::wide("Pro Audio").as_ptr(), &mut task);
                if !mm.is_null() {
                    AvSetMmThreadPriority(mm, AVRT_PRIORITY_CRITICAL);
                }
                PRO_AUDIO.with(|p| p.set(true));
                let mut next = std::time::Instant::now() + period;
                let mut index = 0;
                while !flag.load(Ordering::Acquire) {
                    let wait = next.saturating_duration_since(std::time::Instant::now());
                    let due = -((wait.as_nanos() / 100).max(1) as i64);
                    SetWaitableTimerEx(timer, &due, 0, None, null(), null(), 0);
                    WaitForSingleObject(timer, 100);
                    buffer_switch(index, 1);
                    index ^= 1;
                    next += period;
                    if std::time::Instant::now() > next + period * 4 {
                        next = std::time::Instant::now() + period;
                    }
                }
                if !mm.is_null() {
                    AvRevertMmThreadCharacteristics(mm);
                }
                CloseHandle(timer);
            })
            .ok();
        Simulated { stop, thread }
    }

    fn say(line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    pub fn run() -> Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let [key, id, frames, role] = args.as_slice() else {
            return Err("usage: linktrix_asio_host <key> <CLSID|simulated> <frames> <primary|secondary>".into());
        };
        let requested: i32 = frames.parse().map_err(|_| format!("bad buffer size {frames}"))?;
        unsafe {
            // This process has no window, so Windows 11 would otherwise count
            // it as background work (EcoQoS: efficiency cores, low clocks).
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: 0,
            };
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
            // The driver's own helper threads (not MMCSS) run above normal work.
            SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
            let hr = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
            if hr < 0 {
                return Err(format!("COM initialization: 0x{:08X}", hr as u32));
            }
        }
        let bridge = Bridge::open(key).map_err(|e| e.to_string())?;
        let mut device = Box::new(Device {
            bridge,
            frames: 0,
            period_ns: 0,
            buffers: vec![],
            kinds: vec![],
            driver: null_mut(),
            created: false,
            seq: AtomicU64::new(0),
            pinned: AtomicU32::new(NO_PIN),
        });
        let simulated = id == SIMULATED;
        let mut memory = vec![];
        let loaded = if simulated {
            let (loaded, m) = simulate(&mut device, requested)?;
            memory = m;
            loaded
        } else {
            let result = unsafe { load(id, requested, role == "secondary", &mut device) };
            if result.is_err() && !device.driver.is_null() {
                unsafe { dispose(&device) };
            }
            result?
        };
        device.period_ns = device.frames as u64 * 1_000_000_000 / 48_000;
        device.bridge.header().frames.store(device.frames as u32, Ordering::Release);
        DEVICE.store(&mut *device, Ordering::Release);
        say(&format!(
            "ready {} {} {} {} {}",
            loaded.inputs, loaded.outputs, device.frames, loaded.input_latency, loaded.output_latency
        ));

        // Commands arrive on stdin; the end of stdin (the application exited)
        // means quit. This thread keeps the driver's window messages flowing.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    return;
                }
            }
            let _ = tx.send("quit".into());
        });
        let mut started = false;
        let mut sim: Option<Simulated> = None;
        loop {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if RESET.swap(false, Ordering::AcqRel) {
                say("reset");
            }
            let command = match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => "quit".into(),
            };
            match command.trim() {
                "start" if !started => {
                    let result = if simulated {
                        sim = Some(start_simulated(Duration::from_nanos(device.period_ns)));
                        Ok(())
                    } else {
                        unsafe { check(((*(*device.driver).vtable).start)(device.driver), "Start ASIO hardware") }
                    };
                    match result {
                        Ok(()) => {
                            started = true;
                            say("started");
                        }
                        Err(e) => say(&format!("error {e}")),
                    }
                }
                "panel" => {
                    let code = if simulated {
                        NOT_PRESENT
                    } else {
                        unsafe { ((*(*device.driver).vtable).panel)(device.driver) }
                    };
                    say(&if code == OK { "panel ok".into() } else { format!("panel error ASIO control panel: ASIO error {code}") });
                }
                "quit" => break,
                _ => {}
            }
        }
        // Stop the callbacks, wait out one still running, then free the driver.
        if let Some(mut s) = sim.take() {
            s.stop.store(true, Ordering::Release);
            if let Some(t) = s.thread.take() {
                let _ = t.join();
            }
        }
        if started && !simulated {
            unsafe { ((*(*device.driver).vtable).stop)(device.driver) };
        }
        DEVICE.store(null_mut(), Ordering::Release);
        while BUSY.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        if !simulated {
            unsafe { dispose(&device) };
        }
        drop(loaded.callbacks);
        drop(memory);
        unsafe { CoUninitialize() };
        Ok(())
    }

    /// Dispose the driver's buffers (if created) and release the driver.
    unsafe fn dispose(device: &Device) {
        let v = &*(*device.driver).vtable;
        if device.created {
            (v.dispose)(device.driver);
        }
        (v.release)(device.driver);
    }
}
