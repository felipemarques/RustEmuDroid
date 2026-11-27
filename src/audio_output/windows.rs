//! Windows Core Audio (MMDevice API and WASAPI), called through their
//! documented COM interfaces. Only the methods used here are declared.

mod route;

use super::{Device, Inventory};
pub(crate) use route::{Fault, Notice, Status};
use route::{Router, Step, Target};
use std::{
    collections::VecDeque,
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    },
    core::GUID,
};

const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IMM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const IID_IAUDIO_CLIENT: GUID = GUID::from_u128(0x1cb9ad4c_dbfa_4c32_b178_c2f568a703b2);
const IID_IAUDIO_RENDER_CLIENT: GUID = GUID::from_u128(0xf294acfc_3146_4483_a7bf_addca7c260e2);
/// PKEY_Device_FriendlyName.
const FRIENDLY_NAME: PropertyKey = PropertyKey {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};
const E_RENDER: i32 = 0;
const E_MULTIMEDIA: i32 = 1;
const DEVICE_STATE_ACTIVE: u32 = 1;
const STGM_READ: u32 = 0;
const VT_LPWSTR: u16 = 31;
const AUDCLNT_SHAREMODE_SHARED: i32 = 0;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const WAVE_FORMAT_PCM: u16 = 1;

/// The format requested from the engine's `streamAudio` and given to WASAPI.
pub(crate) const RATE: u32 = 48_000;
pub(crate) const CHANNELS: u16 = 2;

#[repr(C)]
struct PropertyKey {
    fmtid: GUID,
    pid: u32,
}

/// PROPVARIANT: the value union starts at byte 8 and is 16 bytes on x64.
#[repr(C)]
struct PropVariant {
    vt: u16,
    reserved: [u16; 3],
    data: [usize; 2],
}

#[repr(C, packed(1))]
struct WaveFormatEx {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    size: u16,
}

/// An owned COM interface pointer.
struct Com(*mut c_void);

impl Com {
    /// The function at `index` in the interface's vtable.
    unsafe fn method<F: Copy>(&self, index: usize) -> F {
        unsafe {
            let table = *(self.0 as *const *const usize);
            std::mem::transmute_copy(&*table.add(index))
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let release: unsafe extern "system" fn(*mut c_void) -> u32 = self.method(2);
                release(self.0);
            }
        }
    }
}

type Out = *mut *mut c_void;

/// A failed Core Audio call: its name and HRESULT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    what: &'static str,
    hr: i32,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: 0x{:08X}", self.what, self.hr as u32)
    }
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.to_string()
    }
}

impl From<Failure> for Fault {
    fn from(failure: Failure) -> Self {
        Fault::from_call(failure.what, failure.hr)
    }
}

fn check(hr: i32, what: &'static str) -> Result<(), Failure> {
    if hr < 0 {
        Err(Failure { what, hr })
    } else {
        Ok(())
    }
}

/// COM for the calling thread, released with the guard.
struct Apartment(bool);

impl Apartment {
    fn enter() -> Self {
        // S_OK and S_FALSE both need a matching CoUninitialize. A thread already
        // in another apartment (RPC_E_CHANGED_MODE) can still make these calls.
        Self(unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as _) } >= 0)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

unsafe fn take_wide(text: *mut u16) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        CoTaskMemFree(text as *const c_void);
        value
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn enumerator() -> Result<Com, Failure> {
    let mut raw = std::ptr::null_mut();
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_MM_DEVICE_ENUMERATOR,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMM_DEVICE_ENUMERATOR,
            &mut raw,
        )
    };
    check(hr, "MMDeviceEnumerator")?;
    Ok(Com(raw))
}

unsafe fn device_id(device: &Com) -> Result<String, Failure> {
    unsafe {
        let get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32 = device.method(5);
        let mut id = std::ptr::null_mut();
        check(get_id(device.0, &mut id), "IMMDevice::GetId")?;
        Ok(take_wide(id))
    }
}

unsafe fn friendly_name(device: &Com) -> Option<String> {
    unsafe {
        let open: unsafe extern "system" fn(*mut c_void, u32, Out) -> i32 = device.method(4);
        let mut raw = std::ptr::null_mut();
        if open(device.0, STGM_READ, &mut raw) < 0 {
            return None;
        }
        let store = Com(raw);
        let get_value: unsafe extern "system" fn(
            *mut c_void,
            *const PropertyKey,
            *mut PropVariant,
        ) -> i32 = store.method(5);
        let mut value = PropVariant {
            vt: 0,
            reserved: [0; 3],
            data: [0; 2],
        };
        if get_value(store.0, &FRIENDLY_NAME, &mut value) < 0 {
            return None;
        }
        // The friendly name is always VT_LPWSTR; other types own no memory here.
        (value.vt == VT_LPWSTR).then(|| take_wide(value.data[0] as *mut u16))
    }
}

unsafe fn default_endpoint(enumerator: &Com) -> Result<Com, Failure> {
    unsafe {
        let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, Out) -> i32 =
            enumerator.method(4);
        let mut raw = std::ptr::null_mut();
        // The multimedia role, the one the engine's wave mapper output follows.
        check(
            get_default(enumerator.0, E_RENDER, E_MULTIMEDIA, &mut raw),
            "GetDefaultAudioEndpoint",
        )?;
        Ok(Com(raw))
    }
}

pub(crate) fn inventory() -> Result<Inventory, String> {
    let _com = Apartment::enter();
    let enumerator = enumerator()?;
    unsafe {
        let endpoints: unsafe extern "system" fn(*mut c_void, i32, u32, Out) -> i32 =
            enumerator.method(3);
        let mut raw = std::ptr::null_mut();
        check(
            endpoints(enumerator.0, E_RENDER, DEVICE_STATE_ACTIVE, &mut raw),
            "EnumAudioEndpoints",
        )?;
        let collection = Com(raw);
        let count_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
            collection.method(3);
        let item: unsafe extern "system" fn(*mut c_void, u32, Out) -> i32 = collection.method(4);
        let mut count = 0;
        check(
            count_fn(collection.0, &mut count),
            "IMMDeviceCollection::GetCount",
        )?;
        let mut devices = Vec::new();
        for index in 0..count {
            let mut raw = std::ptr::null_mut();
            if item(collection.0, index, &mut raw) < 0 {
                continue;
            }
            let device = Com(raw);
            let Ok(id) = device_id(&device) else {
                continue;
            };
            let name = friendly_name(&device).unwrap_or_else(|| id.clone());
            devices.push(Device { id, name });
        }
        devices.sort_by_key(|device| device.name.to_lowercase());
        let default = default_endpoint(&enumerator)
            .ok()
            .and_then(|device| device_id(&device).ok());
        Ok(Inventory { devices, default })
    }
}

/// Interleaved 16-bit stereo samples from the engine, waiting to be played.
#[derive(Default)]
pub(crate) struct Queue {
    samples: Mutex<VecDeque<i16>>,
}

impl Queue {
    /// At most one second waits; older samples are dropped first.
    pub(crate) fn push(&self, bytes: &[u8]) {
        let limit = (RATE * CHANNELS as u32) as usize;
        let mut samples = self.samples.lock().unwrap();
        samples.extend(
            bytes
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
        );
        let excess = samples.len().saturating_sub(limit);
        samples.drain(..excess);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.samples.lock().unwrap().len()
    }
}

/// Frames buffered before playback starts or resumes after running dry.
const PRIME_FRAMES: usize = (RATE / 1000 * 40) as usize;
/// Queue plus device buffer above this means the engine ran ahead of the
/// device clock; drop the oldest samples back to `PRIME_FRAMES`.
const MAX_FRAMES: usize = (RATE / 1000 * 160) as usize;

/// The engine's clock and the device's clock drift apart slowly (about
/// 0.6 ms per second measured on the test PC, engine slower). Below this
/// level one frame is repeated per write, above `HIGH_FRAMES` one is skipped:
/// at most a few tenths of a percent of speed, so the queue stays near its
/// target instead of running dry and re-priming with a gap.
const LOW_FRAMES: usize = (RATE / 1000 * 30) as usize;
const HIGH_FRAMES: usize = (RATE / 1000 * 70) as usize;

/// +1 repeats a frame, -1 skips one, 0 leaves the queue as it is.
pub(crate) fn nudge(buffered: usize) -> i32 {
    if buffered < LOW_FRAMES {
        1
    } else if buffered > HIGH_FRAMES {
        -1
    } else {
        0
    }
}

/// How many queued frames to discard before the next write.
pub(crate) fn excess_frames(queued: usize, padding: usize) -> usize {
    if queued + padding > MAX_FRAMES {
        queued.saturating_sub(PRIME_FRAMES.saturating_sub(padding.min(PRIME_FRAMES)))
    } else {
        0
    }
}

/// Without an audible sample for this long, the device stream is stopped: a
/// running stream keeps the computer from sleeping and a wireless device
/// awake. The engine's stream is no guide here: it pauses when Android's
/// output goes idle, but an app that keeps a track open delivers zeros.
const QUIET_TIME: Duration = Duration::from_secs(5);

/// What one pass of the player does with the device.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Feed {
    /// Frames to move from the front of the queue to the device.
    pub frames: usize,
    /// Start the device stream after that write.
    pub start: bool,
    /// Stop the device stream: it has played everything it was given.
    pub stop: bool,
    /// Frames waiting in the queue and the device while playing (QA stats).
    pub buffered: Option<usize>,
}

/// The queue side of the player for one open device: priming, drift and
/// silence, decided without calling the device.
#[derive(Debug, Default)]
pub(crate) struct Feeder {
    primed: bool,
    /// Silence is discarded instead of played, up to the next audible frame.
    resting: bool,
    /// The device stream is started.
    running: bool,
    /// When an audible frame last went to the device.
    heard: Option<Instant>,
}

impl Feeder {
    /// A new device starts on the first audible frame.
    pub(crate) fn new() -> Self {
        Self {
            resting: true,
            ..Self::default()
        }
    }

    fn quiet(&self, now: Instant) -> bool {
        self.heard
            .is_none_or(|heard| now.duration_since(heard) >= QUIET_TIME)
    }

    /// Trims, primes and nudges `samples` for a device holding `padding` of
    /// its `capacity` frames, and says what to do with the device.
    pub(crate) fn feed(
        &mut self,
        samples: &mut VecDeque<i16>,
        padding: usize,
        capacity: usize,
        now: Instant,
    ) -> Feed {
        let (frames, buffered) = self.take(samples, padding, capacity, now);
        let mut feed = Feed {
            frames,
            buffered,
            ..Feed::default()
        };
        if frames > 0 {
            feed.start = !self.running;
            self.running = true;
        } else if self.running && padding == 0 && self.quiet(now) {
            self.running = false;
            feed.stop = true;
        }
        feed
    }

    /// How many frames at the front of `samples` go to the device now, and
    /// the frames buffered while playing.
    fn take(
        &mut self,
        samples: &mut VecDeque<i16>,
        padding: usize,
        capacity: usize,
        now: Instant,
    ) -> (usize, Option<usize>) {
        let channels = CHANNELS as usize;
        if self.resting {
            let frames = samples.len() / channels;
            let silent = samples
                .iter()
                .position(|&sample| sample != 0)
                .map_or(frames, |index| index / channels);
            samples.drain(..silent * channels);
            if silent == frames {
                return (0, None);
            }
            // The sound starts with its first audible frame, primed as usual.
            self.resting = false;
            self.primed = false;
        }
        let queued = samples.len() / channels;
        let drop = excess_frames(queued, padding);
        samples.drain(..drop * channels);
        let queued = queued - drop;
        if !self.primed {
            if queued < PRIME_FRAMES {
                return (0, None);
            }
            self.primed = true;
        } else if queued == 0 {
            self.primed = padding > 0;
            return (0, Some(padding));
        }
        let buffered = queued + padding;
        let space = capacity.saturating_sub(padding);
        let next = queued.min(space) * channels;
        if samples.iter().take(next).any(|&sample| sample != 0) {
            self.heard = Some(now);
        } else if next > 0 && self.quiet(now) {
            // Nothing but silence for a while: stop playing it.
            self.resting = true;
            return (0, Some(buffered));
        }
        let mut queued = queued;
        match nudge(buffered) {
            1 => {
                let first: Vec<i16> = samples.iter().take(channels).copied().collect();
                for sample in first.into_iter().rev() {
                    samples.push_front(sample);
                }
                queued += 1;
            }
            -1 if queued > 1 => {
                samples.drain(..channels);
                queued -= 1;
            }
            _ => {}
        }
        (queued.min(space), Some(buffered))
    }
}

struct Output {
    client: Com,
    render: Com,
    frames: u32,
    /// The endpoint that was opened; for the system default, the one it was then.
    endpoint: String,
    feeder: Feeder,
}

/// Opens `endpoint`, or the system default, without starting its stream.
unsafe fn open_output(enumerator: &Com, endpoint: Option<&str>) -> Result<Output, Fault> {
    unsafe {
        let device = match endpoint {
            Some(id) => {
                let get_device: unsafe extern "system" fn(*mut c_void, *const u16, Out) -> i32 =
                    enumerator.method(5);
                let id = wide(id);
                let mut raw = std::ptr::null_mut();
                check(get_device(enumerator.0, id.as_ptr(), &mut raw), "GetDevice")?;
                let device = Com(raw);
                let get_state: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                    device.method(6);
                let mut state = 0;
                check(get_state(device.0, &mut state), "IMMDevice::GetState")?;
                if state != DEVICE_STATE_ACTIVE {
                    return Err(Fault::Gone);
                }
                device
            }
            None => default_endpoint(enumerator)?,
        };
        let endpoint = match endpoint {
            Some(id) => id.to_owned(),
            None => device_id(&device)?,
        };
        let activate: unsafe extern "system" fn(
            *mut c_void,
            *const GUID,
            u32,
            *mut c_void,
            Out,
        ) -> i32 = device.method(3);
        let mut raw = std::ptr::null_mut();
        check(
            activate(
                device.0,
                &IID_IAUDIO_CLIENT,
                CLSCTX_ALL,
                std::ptr::null_mut(),
                &mut raw,
            ),
            "IMMDevice::Activate",
        )?;
        let client = Com(raw);
        let format = WaveFormatEx {
            format_tag: WAVE_FORMAT_PCM,
            channels: CHANNELS,
            samples_per_sec: RATE,
            avg_bytes_per_sec: RATE * CHANNELS as u32 * 2,
            block_align: CHANNELS * 2,
            bits_per_sample: 16,
            size: 0,
        };
        let initialize: unsafe extern "system" fn(
            *mut c_void,
            i32,
            u32,
            i64,
            i64,
            *const WaveFormatEx,
            *const GUID,
        ) -> i32 = client.method(3);
        // 100 ms buffer in 100 ns units; the shared engine converts the format.
        check(
            initialize(
                client.0,
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                1_000_000,
                0,
                &format,
                std::ptr::null(),
            ),
            "IAudioClient::Initialize",
        )?;
        let buffer_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 = client.method(4);
        let mut frames = 0;
        check(buffer_size(client.0, &mut frames), "GetBufferSize")?;
        let service: unsafe extern "system" fn(*mut c_void, *const GUID, Out) -> i32 =
            client.method(14);
        let mut raw = std::ptr::null_mut();
        check(
            service(client.0, &IID_IAUDIO_RENDER_CLIENT, &mut raw),
            "GetService",
        )?;
        let render = Com(raw);
        // The stream starts with the first audible write (`Feeder`).
        Ok(Output {
            client,
            render,
            frames,
            endpoint,
            feeder: Feeder::new(),
        })
    }
}

impl Output {
    /// Writes what the queue holds, starts or stops the stream as the
    /// `Feeder` decides, and returns the frames waiting in the queue and the
    /// device buffer while playing. An error means the device must be reopened.
    unsafe fn feed(&mut self, queue: &Queue, now: Instant) -> Result<Option<usize>, Fault> {
        unsafe {
            let padding_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                self.client.method(6);
            let mut padding = 0;
            check(
                padding_fn(self.client.0, &mut padding),
                "IAudioClient::GetCurrentPadding",
            )?;
            let feed = {
                let mut samples = queue.samples.lock().unwrap();
                let feed =
                    self.feeder
                        .feed(&mut samples, padding as usize, self.frames as usize, now);
                if feed.frames > 0 {
                    let channels = CHANNELS as usize;
                    let get_buffer: unsafe extern "system" fn(
                        *mut c_void,
                        u32,
                        *mut *mut u8,
                    ) -> i32 = self.render.method(3);
                    let release: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32 =
                        self.render.method(4);
                    let mut data = std::ptr::null_mut();
                    check(
                        get_buffer(self.render.0, feed.frames as u32, &mut data),
                        "IAudioRenderClient::GetBuffer",
                    )?;
                    let target =
                        std::slice::from_raw_parts_mut(data as *mut i16, feed.frames * channels);
                    for (slot, sample) in target
                        .iter_mut()
                        .zip(samples.drain(..feed.frames * channels))
                    {
                        *slot = sample;
                    }
                    check(
                        release(self.render.0, feed.frames as u32, 0),
                        "IAudioRenderClient::ReleaseBuffer",
                    )?;
                }
                feed
            };
            // Data written to a stopped stream waits there until it starts.
            if feed.start {
                let start: unsafe extern "system" fn(*mut c_void) -> i32 = self.client.method(10);
                check(start(self.client.0), "IAudioClient::Start")?;
            }
            if feed.stop {
                let stop: unsafe extern "system" fn(*mut c_void) -> i32 = self.client.method(11);
                check(stop(self.client.0), "IAudioClient::Stop")?;
            }
            Ok(feed.buffered)
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            let stop: unsafe extern "system" fn(*mut c_void) -> i32 = self.client.method(11);
            stop(self.client.0);
        }
    }
}

/// Buffered audio while playing, printed once a second to stderr.
#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    min: usize,
    max: usize,
    sum: usize,
    count: usize,
}

impl Stats {
    fn add(&mut self, frames: usize) {
        let since = *self.since.get_or_insert_with(Instant::now);
        self.min = if self.count == 0 {
            frames
        } else {
            self.min.min(frames)
        };
        self.max = self.max.max(frames);
        self.sum += frames;
        self.count += 1;
        if since.elapsed() >= Duration::from_secs(1) {
            let ms = |frames: usize| frames as f64 * 1000. / RATE as f64;
            eprintln!(
                "RustEmuDroid audio stats: client buffer ms min {:.1} avg {:.1} max {:.1}",
                ms(self.min),
                ms(self.sum / self.count),
                ms(self.max)
            );
            *self = Self::default();
        }
    }
}

/// Plays `queue` on `endpoint` until `stop` is set. While that device is
/// unavailable the sound plays on the system default, following its changes,
/// and returns to the device when it works again (`Router`). `report` gets
/// every change of where the sound is, with the notice for the user if the
/// change calls for one.
pub(crate) fn play(
    endpoint: String,
    queue: Arc<Queue>,
    stop: Arc<AtomicBool>,
    mut report: impl FnMut(Status, Option<Notice>),
) {
    let _com = Apartment::enter();
    let mut router = Router::new(Instant::now());
    let mut status = router.status();
    let mut enumerator: Option<Com> = None;
    let mut output: Option<Output> = None;
    // QA only: once a second, how much audio waits in the client while playing.
    let mut stats = std::env::var_os("RUSTEMUDROID_AUDIO_STATS").map(|_| Stats::default());
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        match router.step(now) {
            Step::Wait => {}
            Step::Open(target) => {
                if enumerator.is_none() {
                    match self::enumerator() {
                        Ok(new) => enumerator = Some(new),
                        Err(error) => router.failed(target, error.into(), now),
                    }
                }
                if let Some(devices) = &enumerator {
                    let wanted = (target == Target::Chosen).then_some(endpoint.as_str());
                    match unsafe { open_output(devices, wanted) } {
                        // Replaces the device that played until now, if any.
                        Ok(new) => {
                            output = Some(new);
                            router.opened(target, now);
                        }
                        Err(fault) => {
                            router.failed(target, fault, now);
                            if target == Target::Default {
                                // Ask Windows from scratch on the next attempt.
                                enumerator = None;
                            }
                        }
                    }
                }
            }
            Step::CheckDefault => {
                router.checked(now);
                // Another device became the default: move there. If it cannot
                // be opened, the old one keeps playing until the next check.
                if let (Some(devices), Some(playing)) = (&enumerator, &output)
                    && let Ok(current) =
                        unsafe { default_endpoint(devices).and_then(|device| device_id(&device)) }
                    && current != playing.endpoint
                    && let Ok(new) = unsafe { open_output(devices, None) }
                {
                    output = Some(new);
                    router.opened(Target::Default, now);
                }
            }
        }
        if let Some(playing) = &mut output {
            match unsafe { playing.feed(&queue, now) } {
                Ok(buffered) => {
                    if let (Some(stats), Some(frames)) = (stats.as_mut(), buffered) {
                        stats.add(frames);
                    }
                }
                Err(fault) => {
                    // Device removed or its format changed: reopen or fall back.
                    output = None;
                    router.lost(fault, now);
                }
            }
        }
        let notice = router.notice(now);
        if notice.is_some() || router.status() != status {
            status = router.status();
            report(status.clone(), notice);
        }
        let pause = if output.is_some() { 5 } else { 20 };
        std::thread::sleep(Duration::from_millis(pause));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_keeps_little_endian_samples_and_at_most_one_second() {
        let queue = Queue::default();
        queue.push(&[0x01, 0x00, 0xff, 0xff, 0x7f]);
        assert_eq!(
            queue
                .samples
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [1, -1]
        );
        queue.push(&vec![0; (RATE as usize) * 2 * 2 + 64]);
        assert_eq!(queue.len(), RATE as usize * 2);
    }

    #[test]
    fn the_drift_dead_band_lies_between_30_and_70_ms() {
        assert_eq!(nudge(LOW_FRAMES - 1), 1);
        assert_eq!(nudge(PRIME_FRAMES), 0);
        assert_eq!(nudge(HIGH_FRAMES), 0);
        assert_eq!(nudge(HIGH_FRAMES + 1), -1);
        assert!(
            LOW_FRAMES < PRIME_FRAMES && PRIME_FRAMES < HIGH_FRAMES && HIGH_FRAMES < MAX_FRAMES
        );
    }

    /// The device buffer of the tests: 100 ms, as `open_output` asks for.
    const CAPACITY: usize = RATE as usize / 10;
    const MS: usize = RATE as usize / 1000;

    /// `frames` stereo frames whose left samples count up from `first`.
    fn ramp(first: i16, frames: usize) -> VecDeque<i16> {
        (0..frames as i16)
            .flat_map(|frame| [first + frame, -(first + frame)])
            .collect()
    }

    /// A feeder in the middle of a sound that was audible at `now`.
    fn playing(now: Instant) -> Feeder {
        Feeder {
            primed: true,
            resting: false,
            running: true,
            heard: Some(now),
        }
    }

    #[test]
    fn slow_drift_is_nudged_one_frame_at_a_time() {
        let now = Instant::now();
        // 20 ms buffered, below the band: the first frame goes out twice.
        let mut samples = ramp(100, 10 * MS);
        let feed = playing(now).feed(&mut samples, 10 * MS, CAPACITY, now);
        assert_eq!((feed.frames, feed.buffered), (10 * MS + 1, Some(20 * MS)));
        assert_eq!(samples.len(), (10 * MS + 1) * 2);
        assert_eq!(
            samples.iter().take(6).copied().collect::<Vec<_>>(),
            [100, -100, 100, -100, 101, -101]
        );

        // 80 ms buffered, above the band: the first frame is skipped.
        let mut samples = ramp(100, 10 * MS);
        let feed = playing(now).feed(&mut samples, 70 * MS, CAPACITY, now);
        assert_eq!((feed.frames, feed.buffered), (10 * MS - 1, Some(80 * MS)));
        assert_eq!(
            samples.iter().take(4).copied().collect::<Vec<_>>(),
            [101, -101, 102, -102]
        );
        // The only queued frame is never skipped.
        let mut samples = ramp(100, 1);
        let feed = playing(now).feed(&mut samples, 75 * MS, CAPACITY, now);
        assert_eq!((feed.frames, samples.len()), (1, 2));

        // 50 ms buffered, inside the band: the queue goes out as it is.
        let mut samples = ramp(100, 10 * MS);
        let untouched = samples.clone();
        let feed = playing(now).feed(&mut samples, 40 * MS, CAPACITY, now);
        assert_eq!((feed.frames, feed.buffered), (10 * MS, Some(50 * MS)));
        assert_eq!(samples, untouched);
        assert!(!feed.start && !feed.stop, "the stream keeps running");

        // A write never exceeds the room left in the device.
        let mut samples = ramp(100, 30 * MS);
        let feed = playing(now).feed(&mut samples, CAPACITY - 5 * MS, CAPACITY, now);
        assert_eq!(feed.frames, 5 * MS);
    }

    #[test]
    fn playback_primes_40_ms_and_primes_again_after_running_dry() {
        let now = Instant::now();
        let mut feeder = Feeder::new();
        let mut samples = ramp(1, 39 * MS);
        assert_eq!(feeder.feed(&mut samples, 0, CAPACITY, now), Feed::default());
        samples.extend(ramp(5000, MS));
        let feed = feeder.feed(&mut samples, 0, CAPACITY, now);
        assert_eq!(
            feed,
            Feed {
                frames: 40 * MS,
                start: true,
                stop: false,
                buffered: Some(40 * MS),
            }
        );
        samples.clear();
        // Dry with audio still in the device: new samples continue at once.
        let feed = feeder.feed(&mut samples, 20 * MS, CAPACITY, now);
        assert_eq!((feed.frames, feed.buffered), (0, Some(20 * MS)));
        let mut more = ramp(1, 10 * MS);
        assert_eq!(
            feeder.feed(&mut more, 20 * MS, CAPACITY, now).frames,
            10 * MS
        );
        // Dry with an empty device: wait for 40 ms again.
        more.clear();
        feeder.feed(&mut more, 0, CAPACITY, now);
        let mut late = ramp(1, 10 * MS);
        assert_eq!(feeder.feed(&mut late, 0, CAPACITY, now).frames, 0);
        assert_eq!(late.len(), 10 * MS * 2, "kept for the next write");
    }

    #[test]
    fn the_stream_stops_after_silence_and_starts_on_the_first_audible_frame() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let silence = |frames: usize| VecDeque::from(vec![0i16; frames * 2]);
        let mut feeder = Feeder::new();

        // Android plays silence (an open track): it is dropped and the device stays stopped.
        let mut samples = silence(60 * MS);
        assert_eq!(
            feeder.feed(&mut samples, 0, CAPACITY, at(0)),
            Feed::default()
        );
        assert!(samples.is_empty());

        // A sound begins inside a packet: nothing of it is lost, and the
        // stream starts once 40 ms of it are queued.
        samples.extend(silence(3 * MS));
        samples.extend(ramp(7, 20 * MS));
        assert_eq!(
            feeder.feed(&mut samples, 0, CAPACITY, at(10)),
            Feed::default()
        );
        assert_eq!(
            samples.front(),
            Some(&7),
            "starts at the first audible sample"
        );
        assert_eq!(samples.len(), 20 * MS * 2);
        samples.extend(ramp(7, 20 * MS));
        let feed = feeder.feed(&mut samples, 0, CAPACITY, at(30));
        assert_eq!((feed.frames, feed.start, feed.stop), (40 * MS, true, false));
        assert_eq!(samples.front(), Some(&7));
        samples.clear();

        // Silence inside the sound is played: a pause keeps its length.
        let mut samples = silence(10 * MS);
        let feed = feeder.feed(&mut samples, 30 * MS, CAPACITY, at(2_000));
        assert_eq!(
            (feed.frames, feed.start, feed.stop),
            (10 * MS, false, false)
        );
        let mut samples = silence(10 * MS);
        let feed = feeder.feed(&mut samples, 30 * MS, CAPACITY, at(5_029));
        assert_eq!(feed.frames, 10 * MS, "not yet five seconds");

        // Five seconds without an audible frame: the silence is no longer
        // played, and the stream stops once the device has played its rest.
        let mut samples = silence(10 * MS);
        let feed = feeder.feed(&mut samples, 30 * MS, CAPACITY, at(5_030));
        assert_eq!((feed.frames, feed.stop), (0, false));
        let feed = feeder.feed(&mut samples, 20 * MS, CAPACITY, at(5_035));
        assert_eq!((feed.frames, feed.stop), (0, false));
        assert!(samples.is_empty(), "silence is dropped while resting");
        let feed = feeder.feed(&mut samples, 0, CAPACITY, at(5_130));
        assert_eq!((feed.frames, feed.start, feed.stop), (0, false, true));
        assert_eq!(
            feeder.feed(&mut samples, 0, CAPACITY, at(5_135)),
            Feed::default(),
            "stopped once"
        );

        // More silence does not start it; the next sound does, from its first frame.
        let mut samples = silence(500 * MS);
        assert_eq!(
            feeder.feed(&mut samples, 0, CAPACITY, at(60_000)),
            Feed::default()
        );
        samples.extend(ramp(9, 45 * MS));
        let feed = feeder.feed(&mut samples, 0, CAPACITY, at(60_005));
        assert_eq!((feed.frames, feed.start, feed.stop), (45 * MS, true, false));
        assert_eq!(samples.front(), Some(&9));
    }

    #[test]
    fn the_stream_also_stops_when_the_engine_sends_nothing() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut feeder = Feeder::new();
        let mut samples = ramp(1, 40 * MS);
        assert!(feeder.feed(&mut samples, 0, CAPACITY, at(0)).start);
        samples.clear();
        // Android's output went idle: no packets at all.
        for (ms, padding) in [(20, 20 * MS), (100, 0), (4_999, 0)] {
            let feed = feeder.feed(&mut samples, padding, CAPACITY, at(ms));
            assert_eq!((feed.frames, feed.stop), (0, false), "at {ms} ms");
        }
        assert!(feeder.feed(&mut samples, 0, CAPACITY, at(5_000)).stop);
        assert!(!feeder.feed(&mut samples, 0, CAPACITY, at(5_005)).stop);
        // The next sound starts the stream again.
        let mut samples = ramp(1, 40 * MS);
        let feed = feeder.feed(&mut samples, 0, CAPACITY, at(9_000));
        assert_eq!((feed.frames, feed.start), (40 * MS, true));
    }

    #[test]
    fn a_backlog_is_trimmed_before_it_is_played() {
        let now = Instant::now();
        // One second waited while no device was open: play the newest 40 ms.
        let mut samples = ramp(1, RATE as usize / 2);
        samples.extend(ramp(1, RATE as usize / 2));
        let feed = Feeder::new().feed(&mut samples, 0, CAPACITY, now);
        assert_eq!((feed.frames, feed.start), (PRIME_FRAMES, true));
        assert_eq!(samples.len(), PRIME_FRAMES * 2);
    }

    #[test]
    fn drift_is_trimmed_back_to_the_priming_level() {
        assert_eq!(excess_frames(1_000, 1_000), 0);
        assert_eq!(excess_frames(MAX_FRAMES, 0), 0);
        // 200 ms queued with an empty device: keep 40 ms.
        assert_eq!(excess_frames(9_600, 0), 9_600 - PRIME_FRAMES);
        // With 30 ms already in the device, keep 10 ms in the queue.
        assert_eq!(excess_frames(9_600, 1_440), 9_600 - 480);
        assert_eq!(excess_frames(10, 9_000), 10);
    }

    /// Records `seconds` from the capture endpoint whose name contains `name`.
    fn record(name: &str, seconds: f32) -> Vec<i16> {
        const E_CAPTURE: i32 = 1;
        const IID_IAUDIO_CAPTURE_CLIENT: GUID =
            GUID::from_u128(0xc8adbd64_e71e_48a0_a4de_185c395cd317);
        let _com = Apartment::enter();
        let enumerator = enumerator().unwrap();
        unsafe {
            let endpoints: unsafe extern "system" fn(*mut c_void, i32, u32, Out) -> i32 =
                enumerator.method(3);
            let mut raw = std::ptr::null_mut();
            check(
                endpoints(enumerator.0, E_CAPTURE, DEVICE_STATE_ACTIVE, &mut raw),
                "EnumAudioEndpoints",
            )
            .unwrap();
            let collection = Com(raw);
            let count_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                collection.method(3);
            let item: unsafe extern "system" fn(*mut c_void, u32, Out) -> i32 =
                collection.method(4);
            let mut count = 0;
            count_fn(collection.0, &mut count);
            let device = (0..count)
                .find_map(|index| {
                    let mut raw = std::ptr::null_mut();
                    item(collection.0, index, &mut raw);
                    let device = Com(raw);
                    friendly_name(&device)
                        .is_some_and(|n| n.contains(name))
                        .then_some(device)
                })
                .expect("capture endpoint");
            let activate: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                u32,
                *mut c_void,
                Out,
            ) -> i32 = device.method(3);
            let mut raw = std::ptr::null_mut();
            check(
                activate(
                    device.0,
                    &IID_IAUDIO_CLIENT,
                    CLSCTX_ALL,
                    std::ptr::null_mut(),
                    &mut raw,
                ),
                "Activate",
            )
            .unwrap();
            let client = Com(raw);
            let format = WaveFormatEx {
                format_tag: WAVE_FORMAT_PCM,
                channels: CHANNELS,
                samples_per_sec: RATE,
                avg_bytes_per_sec: RATE * CHANNELS as u32 * 2,
                block_align: CHANNELS * 2,
                bits_per_sample: 16,
                size: 0,
            };
            let initialize: unsafe extern "system" fn(
                *mut c_void,
                i32,
                u32,
                i64,
                i64,
                *const WaveFormatEx,
                *const GUID,
            ) -> i32 = client.method(3);
            check(
                initialize(
                    client.0,
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    2_000_000,
                    0,
                    &format,
                    std::ptr::null(),
                ),
                "Initialize capture",
            )
            .unwrap();
            let service: unsafe extern "system" fn(*mut c_void, *const GUID, Out) -> i32 =
                client.method(14);
            let mut raw = std::ptr::null_mut();
            check(
                service(client.0, &IID_IAUDIO_CAPTURE_CLIENT, &mut raw),
                "GetService",
            )
            .unwrap();
            let capture = Com(raw);
            let start: unsafe extern "system" fn(*mut c_void) -> i32 = client.method(10);
            check(start(client.0), "Start").unwrap();
            let get_buffer: unsafe extern "system" fn(
                *mut c_void,
                *mut *mut u8,
                *mut u32,
                *mut u32,
                *mut u64,
                *mut u64,
            ) -> i32 = capture.method(3);
            let release: unsafe extern "system" fn(*mut c_void, u32) -> i32 = capture.method(4);
            let wanted = (RATE as f32 * seconds) as usize * CHANNELS as usize;
            let mut samples = Vec::new();
            let deadline = Instant::now() + Duration::from_secs_f32(seconds + 3.);
            while samples.len() < wanted && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
                loop {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                    let hr = get_buffer(
                        capture.0,
                        &mut data,
                        &mut frames,
                        &mut flags,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                    if hr != 0 || frames == 0 {
                        if hr == 0 {
                            release(capture.0, 0);
                        }
                        break;
                    }
                    let n = frames as usize * CHANNELS as usize;
                    if flags & 2 != 0 {
                        samples.extend(std::iter::repeat_n(0, n));
                    } else {
                        samples
                            .extend_from_slice(std::slice::from_raw_parts(data as *const i16, n));
                    }
                    release(capture.0, frames);
                }
            }
            let stop: unsafe extern "system" fn(*mut c_void) -> i32 = client.method(11);
            stop(client.0);
            samples
        }
    }

    /// Plays a 1 kHz tone through `play` on VB-Audio's "CABLE Input" and
    /// records it back from "CABLE Output". Inaudible: the cable is virtual.
    /// Run with --ignored --nocapture on a PC with VB-Audio Virtual Cable.
    #[test]
    #[ignore]
    fn plays_on_the_chosen_endpoint_through_a_virtual_cable() {
        let inventory = inventory().unwrap();
        let cable = inventory
            .devices
            .iter()
            .find(|d| d.name.starts_with("CABLE Input"))
            .expect("VB-Audio CABLE Input")
            .clone();
        assert_ne!(
            inventory.default.as_deref(),
            Some(cable.id.as_str()),
            "not the default"
        );
        let queue = Arc::new(Queue::default());
        let stop = Arc::new(AtomicBool::new(false));
        let player = {
            let (queue, stop) = (queue.clone(), stop.clone());
            std::thread::spawn(move || {
                play(cable.id, queue, stop, |status, notice| {
                    assert_eq!((status, notice), (Status::Chosen, None));
                })
            })
        };
        let feeder = {
            let (queue, stop) = (queue.clone(), stop.clone());
            std::thread::spawn(move || {
                // 30 ms packets at real time, like the engine's streamAudio.
                let mut phase = 0u64;
                let start = Instant::now();
                let mut sent = 0u64;
                while !stop.load(Ordering::Acquire) {
                    let due = start.elapsed().as_millis() as u64 * RATE as u64 / 1000;
                    let mut bytes = Vec::new();
                    while sent < due {
                        let value =
                            ((phase as f64 * 2. * std::f64::consts::PI * 1000. / RATE as f64).sin()
                                * 8000.) as i16;
                        phase += 1;
                        sent += 1;
                        for _ in 0..CHANNELS {
                            bytes.extend_from_slice(&value.to_le_bytes());
                        }
                    }
                    queue.push(&bytes);
                    std::thread::sleep(Duration::from_millis(30));
                }
            })
        };
        std::thread::sleep(Duration::from_millis(500));
        let recorded = record("CABLE Output", 2.0);
        stop.store(true, Ordering::Release);
        player.join().unwrap();
        feeder.join().unwrap();
        let left: Vec<f64> = recorded.iter().step_by(2).map(|&s| s as f64).collect();
        let rms = (left.iter().map(|s| s * s).sum::<f64>() / left.len() as f64).sqrt();
        let crossings = left
            .windows(2)
            .filter(|w| (w[0] < 0.) != (w[1] < 0.))
            .count();
        let hz = crossings as f64 / 2. / (left.len() as f64 / RATE as f64);
        let silent = left.iter().filter(|s| s.abs() < 1.).count();
        println!(
            "recorded {} frames, rms {rms:.0}, {hz:.1} Hz by zero crossings, {silent} near-zero frames",
            left.len()
        );
        assert!(rms > 2000., "tone reached the cable");
        assert!((hz - 1000.).abs() < 30., "1 kHz");
    }

    /// Records VB-Audio's "CABLE Output" for RUSTEMUDROID_TEST_RECORD_SECONDS
    /// (default 10) and prints the level per second. Used while a live VM plays
    /// to "CABLE Input" through the window.
    #[test]
    #[ignore]
    fn record_the_virtual_cable() {
        let seconds: f32 = std::env::var("RUSTEMUDROID_TEST_RECORD_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10.);
        let recorded = record("CABLE Output", seconds);
        let per_second = RATE as usize * CHANNELS as usize;
        for (index, second) in recorded.chunks(per_second).enumerate() {
            let peak = second.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
            let rms = (second.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
                / second.len().max(1) as f64)
                .sqrt();
            println!("second {index}: peak {peak}, rms {rms:.0}");
        }
    }

    /// For RUSTEMUDROID_TEST_SESSION_SECONDS (default 10), samples every 100 ms
    /// the audio sessions of every active output and prints, per output and
    /// process, the highest session peak level seen (0.0–1.0) and how many
    /// samples found the session active.
    #[test]
    #[ignore]
    fn print_output_sessions() {
        const IID_SESSION_MANAGER2: GUID = GUID::from_u128(0x77aa99a0_1bd6_484f_8bc7_2c654c9a9b6f);
        const IID_SESSION_CONTROL2: GUID = GUID::from_u128(0xbfb7ff88_7239_4fc9_8fa2_07c950be9c6d);
        const IID_METER: GUID = GUID::from_u128(0xc02216f6_8c67_4b5b_9d00_d008e73e0064);
        let seconds: f32 = std::env::var("RUSTEMUDROID_TEST_SESSION_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10.);
        let inventory = inventory().unwrap();
        let _com = Apartment::enter();
        let enumerator = enumerator().unwrap();
        let mut peaks: std::collections::BTreeMap<(String, u32), (f32, u32)> = Default::default();
        let deadline = Instant::now() + Duration::from_secs_f32(seconds);
        unsafe {
            let get_device: unsafe extern "system" fn(*mut c_void, *const u16, Out) -> i32 =
                enumerator.method(5);
            let mut managers = Vec::new();
            for device in &inventory.devices {
                let id = wide(&device.id);
                let mut raw = std::ptr::null_mut();
                check(get_device(enumerator.0, id.as_ptr(), &mut raw), "GetDevice").unwrap();
                let endpoint = Com(raw);
                let activate: unsafe extern "system" fn(
                    *mut c_void,
                    *const GUID,
                    u32,
                    *mut c_void,
                    Out,
                ) -> i32 = endpoint.method(3);
                let mut raw = std::ptr::null_mut();
                check(
                    activate(
                        endpoint.0,
                        &IID_SESSION_MANAGER2,
                        CLSCTX_ALL,
                        std::ptr::null_mut(),
                        &mut raw,
                    ),
                    "Activate session manager",
                )
                .unwrap();
                managers.push((device.name.clone(), Com(raw)));
            }
            while Instant::now() < deadline {
                for (name, manager) in &managers {
                    let sessions: unsafe extern "system" fn(*mut c_void, Out) -> i32 =
                        manager.method(5);
                    let mut raw = std::ptr::null_mut();
                    if sessions(manager.0, &mut raw) < 0 {
                        continue;
                    }
                    let list = Com(raw);
                    let count_fn: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
                        list.method(3);
                    let item: unsafe extern "system" fn(*mut c_void, i32, Out) -> i32 =
                        list.method(4);
                    let mut count = 0;
                    count_fn(list.0, &mut count);
                    for index in 0..count {
                        let mut raw = std::ptr::null_mut();
                        if item(list.0, index, &mut raw) < 0 {
                            continue;
                        }
                        let control = Com(raw);
                        let query: unsafe extern "system" fn(*mut c_void, *const GUID, Out) -> i32 =
                            control.method(0);
                        let mut raw = std::ptr::null_mut();
                        if query(control.0, &IID_SESSION_CONTROL2, &mut raw) < 0 {
                            continue;
                        }
                        let control2 = Com(raw);
                        let state_fn: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 =
                            control2.method(3);
                        let pid_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                            control2.method(14);
                        let (mut pid, mut state) = (0, 0);
                        pid_fn(control2.0, &mut pid);
                        state_fn(control2.0, &mut state);
                        let mut peak = 0f32;
                        let mut raw = std::ptr::null_mut();
                        if query(control.0, &IID_METER, &mut raw) >= 0 {
                            let meter = Com(raw);
                            let get_peak: unsafe extern "system" fn(*mut c_void, *mut f32) -> i32 =
                                meter.method(3);
                            get_peak(meter.0, &mut peak);
                        }
                        let entry = peaks.entry((name.clone(), pid)).or_default();
                        entry.0 = entry.0.max(peak);
                        // AudioSessionStateActive = 1.
                        entry.1 += (state == 1) as u32;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        for ((device, pid), (peak, active)) in &peaks {
            println!("session\t{pid}\t{peak:.4}\t{active}\t{device}");
        }
    }

    /// Lists the real endpoints of this PC; run with --ignored --nocapture.
    #[test]
    #[ignore]
    fn print_this_pc_outputs() {
        let inventory = inventory().unwrap();
        for device in &inventory.devices {
            println!("{}\t{}", device.id, device.name);
        }
        println!("default: {:?}", inventory.default);
        assert!(inventory.default_device().is_some());
    }
}
