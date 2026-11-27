//! Windows Core Audio (MMDevice API and WASAPI), called through their
//! documented COM interfaces. Only the methods used here are declared.

use super::{Device, Inventory};
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

fn check(hr: i32, what: &str) -> Result<(), String> {
    if hr < 0 {
        Err(format!("{what}: 0x{:08X}", hr as u32))
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

fn enumerator() -> Result<Com, String> {
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

unsafe fn device_id(device: &Com) -> Result<String, String> {
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

unsafe fn default_endpoint(enumerator: &Com) -> Result<Com, String> {
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

struct Output {
    client: Com,
    render: Com,
    frames: u32,
}

unsafe fn open_output(enumerator: &Com, endpoint: Option<&str>) -> Result<Output, String> {
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
                    return Err("device not active".into());
                }
                device
            }
            None => default_endpoint(enumerator)?,
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
        let start: unsafe extern "system" fn(*mut c_void) -> i32 = client.method(10);
        check(start(client.0), "IAudioClient::Start")?;
        Ok(Output {
            client,
            render,
            frames,
        })
    }
}

impl Output {
    /// Writes what the queue holds and returns the frames waiting in the
    /// queue and the device buffer while playing. An error means the device
    /// must be reopened.
    unsafe fn feed(&self, queue: &Queue, primed: &mut bool) -> Result<Option<usize>, i32> {
        unsafe {
            let padding_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 =
                self.client.method(6);
            let mut padding = 0;
            let hr = padding_fn(self.client.0, &mut padding);
            if hr < 0 {
                return Err(hr);
            }
            let mut samples = queue.samples.lock().unwrap();
            let channels = CHANNELS as usize;
            let queued = samples.len() / channels;
            let drop = excess_frames(queued, padding as usize);
            samples.drain(..drop * channels);
            let queued = queued - drop;
            if !*primed {
                if queued < PRIME_FRAMES {
                    return Ok(None);
                }
                *primed = true;
            } else if queued == 0 {
                *primed = padding > 0;
                return Ok(Some(padding as usize));
            }
            let buffered = queued + padding as usize;
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
            let frames = queued.min((self.frames - padding) as usize);
            if frames == 0 {
                return Ok(Some(buffered));
            }
            let get_buffer: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> i32 =
                self.render.method(3);
            let release: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32 =
                self.render.method(4);
            let mut data = std::ptr::null_mut();
            let hr = get_buffer(self.render.0, frames as u32, &mut data);
            if hr < 0 {
                return Err(hr);
            }
            let target = std::slice::from_raw_parts_mut(data as *mut i16, frames * channels);
            for (slot, sample) in target.iter_mut().zip(samples.drain(..frames * channels)) {
                *slot = sample;
            }
            let hr = release(self.render.0, frames as u32, 0);
            if hr < 0 {
                return Err(hr);
            }
            Ok(Some(buffered))
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

/// Plays `queue` on `endpoint` until `stop` is set. If the endpoint goes away
/// the sound moves to the current system default and `notice` is called once
/// with the reason.
pub(crate) fn play(
    endpoint: String,
    queue: Arc<Queue>,
    stop: Arc<AtomicBool>,
    mut notice: impl FnMut(&str),
) {
    let _com = Apartment::enter();
    let enumerator = match enumerator() {
        Ok(enumerator) => enumerator,
        Err(error) => {
            eprintln!("RustEmuDroid audio: {error}");
            return;
        }
    };
    let mut target = Some(endpoint);
    // QA only: once a second, how much audio waits in the client while playing.
    let mut stats = std::env::var_os("RUSTEMUDROID_AUDIO_STATS").map(|_| Stats::default());
    while !stop.load(Ordering::Acquire) {
        let output = match unsafe { open_output(&enumerator, target.as_deref()) } {
            Ok(output) => output,
            Err(error) if target.is_some() => {
                // The chosen device is gone: keep Android audible on the default.
                target = None;
                notice(&error);
                continue;
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        let mut primed = false;
        let mut retried = Instant::now();
        while !stop.load(Ordering::Acquire) {
            let fed = unsafe { output.feed(&queue, &mut primed) };
            if let (Some(stats), Ok(Some(frames))) = (stats.as_mut(), &fed) {
                stats.add(*frames);
            }
            if fed.is_err() {
                // Device removed or its format changed: reopen.
                if retried.elapsed() < Duration::from_millis(100) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                retried = Instant::now();
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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
    fn slow_drift_is_nudged_one_frame_at_a_time() {
        assert_eq!(nudge(LOW_FRAMES - 1), 1);
        assert_eq!(nudge(PRIME_FRAMES), 0);
        assert_eq!(nudge(HIGH_FRAMES), 0);
        assert_eq!(nudge(HIGH_FRAMES + 1), -1);
        assert!(
            LOW_FRAMES < PRIME_FRAMES && PRIME_FRAMES < HIGH_FRAMES && HIGH_FRAMES < MAX_FRAMES
        );
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
            std::thread::spawn(move || play(cable.id, queue, stop, |e| panic!("{e}")))
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
