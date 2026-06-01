use crate::error::{Error, Result};
use crate::pcm::Reader;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

#[link(name = "dl")]
extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
const PA_STREAM_PLAYBACK: c_int = 1;
const PA_SAMPLE_FLOAT32LE: c_int = 5;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

type NewFn = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const SampleSpec,
    *const c_void,
    *const c_void,
    *mut c_int,
) -> *mut c_void;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type DrainFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int;
type FlushFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut c_void);

struct Lib {
    new: NewFn,
    write: WriteFn,
    drain: DrainFn,
    flush: FlushFn,
    free: FreeFn,
}

fn load() -> Result<Lib> {
    unsafe {
        let mut h = std::ptr::null_mut();
        for name in ["libpulse-simple.so.0", "libpulse-simple.so"] {
            let c = CString::new(name).unwrap();
            h = dlopen(c.as_ptr(), RTLD_NOW);
            if !h.is_null() {
                break;
            }
        }
        if h.is_null() {
            return Err(Error::Audio("libpulse-simple not found".into()));
        }
        let sym = |s: &str| -> Result<*mut c_void> {
            let c = CString::new(s).unwrap();
            let p = dlsym(h, c.as_ptr());
            if p.is_null() {
                Err(Error::Audio(format!("missing symbol {s}")))
            } else {
                Ok(p)
            }
        };
        Ok(Lib {
            new: std::mem::transmute::<*mut c_void, NewFn>(sym("pa_simple_new")?),
            write: std::mem::transmute::<*mut c_void, WriteFn>(sym("pa_simple_write")?),
            drain: std::mem::transmute::<*mut c_void, DrainFn>(sym("pa_simple_drain")?),
            flush: std::mem::transmute::<*mut c_void, FlushFn>(sym("pa_simple_flush")?),
            free: std::mem::transmute::<*mut c_void, FreeFn>(sym("pa_simple_free")?),
        })
    }
}

const RATE: u32 = 48000;
const CHANNELS: u8 = 2;
const CHUNK: usize = 1024 * CHANNELS as usize;

struct Handle(*mut c_void);
unsafe impl Send for Handle {}

pub struct PulseSink {
    reader: Option<Reader>,
    stop: Arc<AtomicBool>,
    flush: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    vol: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

impl PulseSink {
    pub fn new(reader: Reader) -> Self {
        PulseSink {
            reader: Some(reader),
            stop: Arc::new(AtomicBool::new(false)),
            flush: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            vol: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            thread: None,
        }
    }

    pub fn set_volume(&self, v: f32) {
        self.vol.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.store(p, Ordering::Relaxed);
    }

    pub fn start(&mut self) -> Result<()> {
        let reader = self
            .reader
            .take()
            .ok_or_else(|| Error::Audio("sink already started".into()))?;
        let lib = load()?;

        let ss = SampleSpec {
            format: PA_SAMPLE_FLOAT32LE,
            rate: RATE,
            channels: CHANNELS,
        };
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: RATE * CHANNELS as u32 * 4 / 12,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: u32::MAX,
        };
        let app = CString::new("mzk").unwrap();
        let stream = CString::new("music").unwrap();
        let mut err: c_int = 0;
        let s = unsafe {
            (lib.new)(
                std::ptr::null(),
                app.as_ptr(),
                PA_STREAM_PLAYBACK,
                std::ptr::null(),
                stream.as_ptr(),
                &ss,
                std::ptr::null(),
                &attr as *const BufferAttr as *const c_void,
                &mut err,
            )
        };
        if s.is_null() {
            return Err(Error::Audio(format!("pa_simple_new failed: {err}")));
        }

        let handle = Handle(s);
        let stop = self.stop.clone();
        let flush = self.flush.clone();
        let paused = self.paused.clone();
        let vol = self.vol.clone();
        self.thread = Some(std::thread::spawn(move || {
            let handle = handle;
            let mut buf = [0f32; CHUNK];
            while !stop.load(Ordering::Relaxed) {
                let mut e: c_int = 0;
                if flush.swap(false, Ordering::Relaxed) {
                    unsafe { (lib.flush)(handle.0, &mut e) };
                }
                if paused.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let got = reader.pop(&mut buf);
                let v = f32::from_bits(vol.load(Ordering::Relaxed));
                for s in buf[..got].iter_mut() {
                    *s = (*s * v).clamp(-1.0, 1.0);
                }
                for s in buf[got..].iter_mut() {
                    *s = 0.0;
                }
                let bytes = CHUNK * 4;
                unsafe {
                    (lib.write)(handle.0, buf.as_ptr() as *const c_void, bytes, &mut e);
                }
            }
            let mut e: c_int = 0;
            unsafe {
                (lib.drain)(handle.0, &mut e);
                (lib.free)(handle.0);
            }
        }));
        Ok(())
    }

    pub fn flush(&self) {
        self.flush.store(true, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PulseSink {
    fn drop(&mut self) {
        self.stop();
    }
}
