//! Minimal libpulse ABI. All PulseAudio objects stay on one dedicated thread.
//! The server performs sample conversion, avoiding a resampler or recorder subprocess.
use crate::transcribe::AudioFrame;
use anyhow::{Context, Result, ensure};
use libc::{c_char, c_int, c_void};
use std::{
    ffi::CString,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

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
#[link(name = "libpulse.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn pa_mainloop_new() -> *mut c_void;
    fn pa_mainloop_free(m: *mut c_void);
    fn pa_mainloop_get_api(m: *mut c_void) -> *mut c_void;
    fn pa_mainloop_iterate(m: *mut c_void, block: c_int, retval: *mut c_int) -> c_int;
    fn pa_context_new(api: *mut c_void, name: *const c_char) -> *mut c_void;
    fn pa_context_connect(
        c: *mut c_void,
        server: *const c_char,
        flags: c_int,
        api: *const c_void,
    ) -> c_int;
    fn pa_context_get_state(c: *const c_void) -> c_int;
    fn pa_context_disconnect(c: *mut c_void);
    fn pa_context_unref(c: *mut c_void);
    fn pa_stream_new(
        c: *mut c_void,
        name: *const c_char,
        spec: *const SampleSpec,
        map: *const c_void,
    ) -> *mut c_void;
    fn pa_stream_connect_record(
        s: *mut c_void,
        device: *const c_char,
        attr: *const BufferAttr,
        flags: c_int,
    ) -> c_int;
    fn pa_stream_get_state(s: *const c_void) -> c_int;
    fn pa_stream_readable_size(s: *const c_void) -> usize;
    fn pa_stream_peek(s: *mut c_void, data: *mut *const c_void, bytes: *mut usize) -> c_int;
    fn pa_stream_drop(s: *mut c_void) -> c_int;
    fn pa_stream_disconnect(s: *mut c_void) -> c_int;
    fn pa_stream_unref(s: *mut c_void);
}
struct Pulse {
    mainloop: *mut c_void,
    context: *mut c_void,
    stream: *mut c_void,
}
impl Pulse {
    fn connect() -> Result<Self> {
        // SAFETY: opaque handles are checked, used on this thread, and freed in Drop.
        unsafe {
            let mainloop = pa_mainloop_new();
            ensure!(!mainloop.is_null(), "Cannot allocate PulseAudio mainloop");
            let mut p = Self {
                mainloop,
                context: ptr::null_mut(),
                stream: ptr::null_mut(),
            };
            p.context = pa_context_new(pa_mainloop_get_api(mainloop), c"Likh".as_ptr());
            ensure!(!p.context.is_null(), "Cannot create PulseAudio context");
            ensure!(
                pa_context_connect(p.context, ptr::null(), 0, ptr::null()) >= 0,
                "Cannot connect to PulseAudio/PipeWire"
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                p.iterate()?;
                match pa_context_get_state(p.context) {
                    4 => return Ok(p),
                    5 | 6 => anyhow::bail!("Audio server unavailable"),
                    _ => (),
                }
                ensure!(
                    Instant::now() < deadline,
                    "Audio server connection timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    fn iterate(&mut self) -> Result<()> {
        unsafe {
            ensure!(
                pa_mainloop_iterate(self.mainloop, 0, ptr::null_mut()) >= 0
                    && !matches!(pa_context_get_state(self.context), 5 | 6),
                "Audio server disconnected"
            );
        }
        Ok(())
    }
    fn start(&mut self, device: Option<&str>) -> Result<()> {
        self.stop();
        let device = device.map(CString::new).transpose()?;
        unsafe {
            self.stream = pa_stream_new(
                self.context,
                c"Voice typing".as_ptr(),
                &SampleSpec {
                    format: 3,
                    rate: 16000,
                    channels: 1,
                },
                ptr::null(),
            );
            ensure!(!self.stream.is_null(), "Cannot create microphone stream");
            let attr = BufferAttr {
                maxlength: 32000,
                tlength: u32::MAX,
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: 640,
            };
            ensure!(
                pa_stream_connect_record(
                    self.stream,
                    device.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
                    &attr,
                    0x2000
                ) >= 0,
                "Cannot open microphone"
            );
        }
        Ok(())
    }
    fn ready(&self) -> Result<bool> {
        let state = unsafe { pa_stream_get_state(self.stream) };
        ensure!(
            !matches!(state, 3 | 4),
            "Microphone disconnected or unavailable"
        );
        Ok(state == 2)
    }
    fn read(&mut self) -> Result<Option<Vec<u8>>> {
        unsafe {
            let available = pa_stream_readable_size(self.stream);
            ensure!(available != usize::MAX, "Cannot read microphone");
            if available == 0 {
                return Ok(None);
            }
            let mut data = ptr::null();
            let mut size = 0;
            ensure!(
                pa_stream_peek(self.stream, &mut data, &mut size) >= 0,
                "Microphone read failed"
            );
            ensure!(size <= 32000, "Audio server returned an oversized fragment");
            if size == 0 {
                return Ok(None);
            }
            let bytes = if data.is_null() {
                vec![0; size]
            } else {
                std::slice::from_raw_parts(data as *const u8, size).to_vec()
            };
            ensure!(
                pa_stream_drop(self.stream) >= 0,
                "Cannot release audio buffer"
            );
            Ok(Some(bytes))
        }
    }
    fn stop(&mut self) {
        if !self.stream.is_null() {
            unsafe {
                pa_stream_disconnect(self.stream);
                pa_stream_unref(self.stream);
            }
            self.stream = ptr::null_mut();
        }
    }
}
impl Drop for Pulse {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            if !self.context.is_null() {
                pa_context_disconnect(self.context);
                pa_context_unref(self.context);
            }
            pa_mainloop_free(self.mainloop);
        }
    }
}

pub enum Event {
    Ready(u64),
    Error(u64, String),
}
struct Request {
    id: u64,
    active: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    frames: tokio::sync::mpsc::Sender<AudioFrame>,
    device: Option<String>,
}
pub struct Audio {
    tx: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
}
pub struct Capture {
    active: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
}
impl Capture {
    /// Consume the loudest fragment since the previous UI update, without queuing events.
    pub fn take_level(&self) -> f32 {
        self.level.swap(0, Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Audio {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Request>();
        let (ev, events) = mpsc::channel();
        let (initialized, init) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("likh-audio".into())
            .spawn(move || {
                let mut pulse = match Pulse::connect() {
                    Ok(p) => {
                        let _ = initialized.send(Ok(()));
                        p
                    }
                    Err(e) => {
                        let _ = initialized.send(Err(e.to_string()));
                        return;
                    }
                };
                loop {
                    let request = match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(r) => r,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = pulse.iterate();
                            continue;
                        }
                        Err(_) => break,
                    };
                    if !request.active.load(Ordering::Acquire) {
                        continue;
                    }
                    let result = (|| -> Result<()> {
                        if pulse.iterate().is_err() {
                            pulse = Pulse::connect()?;
                        }
                        pulse.start(request.device.as_deref())?;
                        let started = Instant::now();
                        let mut ready = false;
                        while request.active.load(Ordering::Acquire) {
                            pulse.iterate()?;
                            if !pulse.ready()? {
                                ensure!(
                                    started.elapsed() < Duration::from_secs(3),
                                    "Microphone startup timed out"
                                );
                            } else {
                                if !ready {
                                    ready = true;
                                    let _ = ev.send(Event::Ready(request.id));
                                }
                                while request.active.load(Ordering::Acquire) {
                                    let Some(frame) = pulse.read()? else {
                                        break;
                                    };
                                    if !request.active.load(Ordering::Acquire) {
                                        break;
                                    }
                                    request.level.fetch_max(
                                        (meter_level(&frame) * 1000.0) as u32,
                                        Ordering::Relaxed,
                                    );
                                    if !enqueue(&request.frames, &frame)? {
                                        return Ok(());
                                    }
                                }
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Ok(())
                    })();
                    pulse.stop();
                    if let Err(e) = result {
                        let _ = request.frames.try_send(Err(e.to_string()));
                        let _ = ev.send(Event::Error(request.id, e.to_string()));
                    }
                    // Dropping the sender flushes queued frames and finalizes the socket.
                }
            })?;
        init.recv()
            .context("Audio initialization failed")?
            .map_err(anyhow::Error::msg)?;
        Ok(Self { tx, events })
    }
    pub fn start(
        &self,
        id: u64,
        device: Option<String>,
    ) -> Result<(Capture, tokio::sync::mpsc::Receiver<AudioFrame>)> {
        let active = Arc::new(AtomicBool::new(true));
        let level = Arc::new(AtomicU32::new(0));
        // At 20ms fragments this covers the full 20s handshake without unbounded growth.
        let (frames, rx) = tokio::sync::mpsc::channel(1500);
        self.tx.send(Request {
            id,
            active: active.clone(),
            level: level.clone(),
            frames,
            device,
        })?;
        Ok((Capture { active, level }, rx))
    }
    pub fn probe() -> Result<()> {
        Pulse::connect().map(|_| ())
    }
}

// Mono signed 16-bit little-endian PCM. A small noise floor keeps quiet rooms
// calm; the square-root response makes ordinary speech visible without clipping.
fn meter_level(frame: &[u8]) -> f32 {
    let count = frame.len() / 2;
    if count == 0 {
        return 0.0;
    }
    let energy: f32 = frame
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| {
            let sample = i16::from_le_bytes([b[0], b[1]]) as f32;
            sample * sample
        })
        .sum();
    let rms = (energy / count as f32).sqrt() / 32768.0;
    ((rms - 0.004).max(0.0) * 6.0).sqrt().min(1.0)
}

fn enqueue(tx: &tokio::sync::mpsc::Sender<AudioFrame>, frame: &[u8]) -> Result<bool> {
    ensure!(
        frame.len().is_multiple_of(2),
        "Invalid PCM sample alignment"
    );
    // Fixed 20ms chunks bound the queue to 960KB, independent of server fragments.
    for chunk in frame.chunks(640) {
        match tx.try_send(Ok(chunk.to_vec())) {
            Ok(()) => (),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return Ok(false),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                anyhow::bail!("Audio upload could not keep up; recording stopped")
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_and_drop_stop_capture() {
        let active = Arc::new(AtomicBool::new(true));
        let capture = Capture {
            active: active.clone(),
            level: Arc::new(AtomicU32::new(500)),
        };
        assert_eq!(capture.take_level(), 0.5);
        assert_eq!(capture.take_level(), 0.0);
        capture.stop();
        assert!(!active.load(Ordering::Acquire));
        active.store(true, Ordering::Release);
        drop(capture);
        assert!(!active.load(Ordering::Acquire));
    }
    #[test]
    fn microphone_meter_handles_silence_speech_and_clipping() {
        let frame = |value: i16| value.to_le_bytes().repeat(320);
        assert_eq!(meter_level(&[]), 0.0);
        assert_eq!(meter_level(&frame(0)), 0.0);
        assert_eq!(meter_level(&frame(64)), 0.0);
        let quiet = meter_level(&frame(512));
        let speech = meter_level(&frame(4096));
        assert!(quiet > 0.0 && quiet < speech && speech < 1.0);
        assert_eq!(meter_level(&frame(-4096)), speech);
        assert_eq!(meter_level(&frame(i16::MAX)), 1.0);
        assert_eq!(meter_level(&frame(i16::MIN)), 1.0);
    }
    #[test]
    fn queue_overflow_and_closed_receiver() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        assert!(enqueue(&tx, &[1; 1280]).unwrap());
        assert!(enqueue(&tx, &[2; 640]).is_err());
        assert_eq!(rx.try_recv().unwrap().unwrap(), vec![1; 640]);
        assert_eq!(rx.try_recv().unwrap().unwrap(), vec![1; 640]);
        drop(rx);
        assert!(!enqueue(&tx, &[0; 640]).unwrap());
        assert!(enqueue(&tx, &[0]).is_err());
    }
}
