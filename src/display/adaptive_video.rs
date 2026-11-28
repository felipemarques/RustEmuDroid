//! Window-sized transport for the primary Android Emulator display.

#[cfg(test)]
pub(super) mod fake_engine;
mod shared;

use super::{
    DisplayResult, Fallback, Shared, StreamEnded, VideoTransport,
    engine_rpc::{Format, Frame, authorized},
    shared_frames::{self, SharedFrames},
};
use eframe::egui::{self, Rect};
use std::{
    fs, io,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::{sync::watch, time};
use tonic::{
    client::Grpc,
    codec::ProstCodec,
    codegen::http::uri::PathAndQuery,
    metadata::{Ascii, MetadataValue},
    transport::Channel,
};

const RESIZE_SETTLE: Duration = Duration::from_millis(250);
const FIRST_REDUCED_FRAME: Duration = Duration::from_secs(2);
/// `RUSTEMUDROID_VIDEO_TRANSPORT=inline` keeps every frame in its message and
/// `=shared` asks for shared memory; without it the system decides
/// (`shared_frames::DEFAULT`).
pub(super) const TRANSPORT_ENV: &str = "RUSTEMUDROID_VIDEO_TRANSPORT";

/// Whether shared memory is to be tried, and if not, why.
fn shared_wanted(value: Option<&str>, by_default: bool) -> Result<(), Fallback> {
    let value = value.map(str::trim);
    if value.is_some_and(|value| value.eq_ignore_ascii_case("inline")) {
        Err(Fallback::TurnedOff)
    } else if by_default || value.is_some_and(|value| value.eq_ignore_ascii_case("shared")) {
        Ok(())
    } else {
        Err(Fallback::OffByDefault)
    }
}

/// What a display session is told about shared memory. The tests replace the
/// limits and the way a region is made.
pub(super) struct Setup {
    /// `Err` keeps every frame in its message and is the reason reported.
    shared: Result<(), Fallback>,
    timing: shared::Timing,
    create: Box<dyn Fn(usize) -> io::Result<SharedFrames>>,
}

impl Setup {
    pub fn from_environment() -> Self {
        Self {
            shared: shared_wanted(
                std::env::var(TRANSPORT_ENV).ok().as_deref(),
                shared_frames::DEFAULT,
            ),
            timing: shared::Timing::default(),
            create: Box::new(SharedFrames::create),
        }
    }
}

pub(super) fn fixed_raster(avd: &Path) -> Option<[u32; 2]> {
    parse_fixed_raster(&fs::read_to_string(avd.join("hardware-qemu.ini")).ok()?)
}

fn parse_fixed_raster(hardware: &str) -> Option<[u32; 2]> {
    let mut size = [0; 2];
    for (key, value) in hardware.lines().filter_map(|line| line.split_once('=')) {
        let key = key.trim();
        let value = value.trim();
        match key {
            "hw.lcd.width" | "hw.lcd.height" => {
                let axis = usize::from(key.ends_with("height"));
                if size[axis] != 0 {
                    return None;
                }
                size[axis] = value.parse().ok()?;
            }
            "hw.resizable.configs" if !value.is_empty() => return None,
            "hw.sensor.hinge"
            | "hw.sensor.roll"
            | "hw.multi_display_window"
            | "hw.hotplug_multi_display"
                if !matches!(value, "false" | "no" | "0" | "") =>
            {
                return None;
            }
            "hw.device.name" => {
                let name = value.to_ascii_lowercase();
                if ["resizable", "fold", "rollable"]
                    .iter()
                    .any(|part| name.contains(part))
                {
                    return None;
                }
            }
            _ if [
                "hw.displayRegion.",
                "hw.display1.",
                "hw.display2.",
                "hw.display3.",
            ]
            .iter()
            .any(|prefix| key.starts_with(prefix))
                && (key.ends_with(".width") || key.ends_with(".height"))
                && value != "0" =>
            {
                return None;
            }
            _ => {}
        }
    }
    (!size.contains(&0)).then_some(size)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NativeDisplay {
    pub size: [u32; 2],
    pub rotation: i32,
}

impl NativeDisplay {
    pub fn conventional(format: &Format) -> Option<Self> {
        let size = [format.width, format.height];
        let rotation = format
            .rotation
            .as_ref()
            .map_or(0, |rotation| rotation.value);
        (format.format == 1
            && !size.contains(&0)
            && (0..=3).contains(&rotation)
            && format.display == 0
            && format.folded_display.is_none()
            && format.display_mode.is_none())
        .then_some(Self { size, rotation })
    }

    fn matches_raster(self, mut raster: [u32; 2]) -> bool {
        if self.rotation % 2 != 0 {
            raster.swap(0, 1);
        }
        self.size == raster
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reduction {
    size: [u32; 2],
    native: NativeDisplay,
}

impl Reduction {
    fn accepts(self, frame: &Frame) -> DisplayResult<bool> {
        let Some(actual) = frame.format.as_ref().and_then(NativeDisplay::conventional) else {
            return Ok(false);
        };
        if actual.rotation != self.native.rotation {
            return Ok(false);
        }
        let [width, height] = self.size;
        if actual.size != self.size
            || frame.pixels.len() as u64 != u64::from(width) * u64::from(height) * 4
        {
            return Err("Android Emulator did not return the requested video raster".into());
        }
        Ok(true)
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Request {
    generation: u64,
    reduction: Option<Reduction>,
    // No subscription at all: the panel shows the engine's own window and
    // nothing in the client needs pixels.
    paused: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Source {
    generation: u64,
    pub visibility_epoch: u64,
    pub scaled: bool,
    can_adapt: bool,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            generation: 0,
            visibility_epoch: 0,
            scaled: false,
            can_adapt: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    physical_size: [u32; 2],
    pixels_per_point: u32,
    reduction: Reduction,
}

impl Layout {
    fn measure(rect: Rect, scale: f32, native: NativeDisplay) -> Option<Self> {
        if !rect.is_finite() || !rect.is_positive() || !scale.is_finite() || scale <= 0. {
            return None;
        }
        let min = (rect.min.to_vec2() * scale).round();
        let max = (rect.max.to_vec2() * scale).round();
        let physical_size = [(max.x - min.x) as u32, (max.y - min.y) as u32];
        if physical_size.contains(&0) {
            return None;
        }
        let [width, height] = native.size;
        let (mut divisor, mut remainder) = (width, height);
        while remainder != 0 {
            (divisor, remainder) = (remainder, divisor % remainder);
        }
        // Cover the fitted pixels in a whole native-aspect block so the
        // decoder never relies on SDK rounding or needs to enlarge the image.
        let ratio = [width / divisor, height / divisor];
        let blocks = physical_size[0]
            .div_ceil(ratio[0])
            .max(physical_size[1].div_ceil(ratio[1]));
        (blocks < divisor).then(|| Self {
            physical_size,
            pixels_per_point: scale.to_bits(),
            reduction: Reduction {
                size: ratio.map(|edge| edge * blocks),
                native,
            },
        })
    }
}

pub(super) struct Preference {
    sender: watch::Sender<Request>,
    requested: Request,
    enabled: bool,
    settled: Option<(Layout, Instant)>,
}

impl Preference {
    pub fn new() -> (Self, watch::Receiver<Request>) {
        let (sender, receiver) = watch::channel(Request::default());
        (
            Self {
                sender,
                requested: Request::default(),
                enabled: false,
                settled: None,
            },
            receiver,
        )
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.settled = None;
            self.request(None);
        }
    }

    /// Stops or restarts the subscription. Either way the frames received
    /// before the change stop counting as current.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn set_paused(&mut self, paused: bool) {
        if self.requested.paused != paused {
            self.requested.paused = paused;
            self.requested.reduction = None;
            self.requested.generation += 1;
            self.settled = None;
            self.sender.send_replace(self.requested);
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn paused(&self) -> bool {
        self.requested.paused
    }

    pub fn accepts_native(&self, source: Source) -> bool {
        !source.scaled && self.requested.generation == source.generation
    }

    /// Whether `source` came from the subscription asked for last.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn is_current(&self, source: Source) -> bool {
        !self.requested.paused && self.requested.generation == source.generation
    }

    pub fn update(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        windowed: bool,
        native: Option<NativeDisplay>,
        source: Source,
    ) {
        if !self.enabled || self.requested.paused {
            return;
        }
        let layout = if windowed && source.can_adapt {
            native.and_then(|native| Layout::measure(rect, ctx.pixels_per_point(), native))
        } else {
            None
        };
        if let Some(remaining) = self.update_layout(layout, source, Instant::now()) {
            ctx.request_repaint_after(remaining);
        }
    }

    fn update_layout(
        &mut self,
        layout: Option<Layout>,
        source: Source,
        now: Instant,
    ) -> Option<Duration> {
        let Some(layout) = layout else {
            self.settled = None;
            if source.can_adapt {
                self.request(None);
            }
            return None;
        };
        let since = match self.settled {
            Some((previous, since)) if previous == layout => since,
            _ => {
                self.settled = Some((layout, now));
                self.request(None);
                now
            }
        };
        let elapsed = now.duration_since(since);
        if elapsed < RESIZE_SETTLE {
            return Some(RESIZE_SETTLE - elapsed);
        }
        if self.requested.reduction.is_none() && self.accepts_native(source) {
            self.request(Some(layout.reduction));
        }
        None
    }

    fn request(&mut self, reduction: Option<Reduction>) {
        if self.requested.reduction != reduction {
            self.requested.generation += 1;
            self.requested.reduction = reduction;
            self.sender.send_replace(self.requested);
        }
    }
}

// A clean return asks the worker to refresh the native geometry. Errors on a
// scaled subscription disable reduction for this session.
async fn receive(
    video: &mut Grpc<Channel>,
    token: &MetadataValue<Ascii>,
    ctx: &egui::Context,
    state: &Mutex<Shared>,
    request: Request,
    fixed_raster: Option<[u32; 2]>,
) -> DisplayResult<()> {
    let deadline = request
        .reduction
        .map(|_| time::Instant::now() + FIRST_REDUCED_FRAME);
    let [width, height] = request.reduction.map_or([0, 0], |reduction| reduction.size);
    let start = async {
        video.ready().await?;
        let reply = video
            .server_streaming(
                authorized(
                    Format {
                        format: 1,
                        width,
                        height,
                        ..Default::default()
                    },
                    token,
                ),
                PathAndQuery::from_static(
                    "/android.emulation.control.EmulatorController/streamScreenshot",
                ),
                ProstCodec::<Format, Frame>::default(),
            )
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(reply.into_inner())
    };
    let mut frames = if let Some(deadline) = deadline {
        time::timeout_at(deadline, start).await??
    } else {
        start.await?
    };
    let mut first = deadline;
    let scaled = request.reduction.is_some();
    loop {
        let message = if let Some(deadline) = first {
            time::timeout_at(deadline, frames.message()).await??
        } else {
            frames.message().await?
        };
        let Some(frame) = message else {
            return Err(StreamEnded.into());
        };
        if frame.pixels.is_empty() {
            let mut shared = state.lock().unwrap();
            if !shared.blank {
                shared.blank = true;
                shared.latest = None;
                shared.source.visibility_epoch += 1;
                drop(shared);
                ctx.request_repaint();
            }
            if scaled && first.is_none() {
                return Ok(());
            }
            continue;
        }
        if let Some(reduction) = request.reduction
            && !reduction.accepts(&frame)?
        {
            return Ok(());
        }
        first = None;
        let can_adapt = fixed_raster.is_some_and(|raster| {
            scaled
                || frame
                    .format
                    .as_ref()
                    .and_then(NativeDisplay::conventional)
                    .is_some_and(|native| native.matches_raster(raster))
        });
        let mut shared = state.lock().unwrap();
        shared.latest = Some(frame);
        shared.delivered += 1;
        shared.source = Source {
            generation: request.generation,
            visibility_epoch: shared.source.visibility_epoch,
            scaled,
            can_adapt,
        };
        shared.blank = false;
        drop(shared);
        ctx.request_repaint();
    }
}

pub(super) async fn run(
    mut video: Grpc<Channel>,
    token: &MetadataValue<Ascii>,
    ctx: &egui::Context,
    state: &Mutex<Shared>,
    mut requests: watch::Receiver<Request>,
    mut fixed_raster: Option<[u32; 2]>,
    setup: Setup,
) -> DisplayResult<()> {
    use shared::Plan;
    let mut reacquire = None;
    let report = |fallback| {
        state
            .lock()
            .unwrap()
            .set_video_transport(VideoTransport::Inline { fallback })
    };
    // The native raster bounds every frame of the session, so it sizes the
    // region of every subscription.
    let native_bytes = fixed_raster.map(|[width, height]| {
        u64::from(width)
            .saturating_mul(u64::from(height))
            .saturating_mul(4)
    });
    let mut plan = match (setup.shared, native_bytes) {
        _ if !shared_frames::SUPPORTED => {
            report(None);
            Plan::Inline
        }
        (Err(reason), _) => {
            report(Some(reason));
            Plan::Inline
        }
        (Ok(()), None) => {
            report(Some(Fallback::ResizableDisplay));
            Plan::Inline
        }
        (Ok(()), Some(frame)) if frame > shared_frames::MAX_REGION => {
            report(Some(Fallback::FrameTooLarge {
                frame,
                region: shared_frames::MAX_REGION,
            }));
            Plan::Inline
        }
        (Ok(()), Some(_)) => {
            shared_frames::remove_leftovers();
            Plan::UntilFirstPicture
        }
    };
    loop {
        let mut request = *requests.borrow_and_update();
        if request.paused {
            // An in-band look that a pause cut short proves nothing later: a
            // picture after the pause may only mean that the screen came back.
            if plan == Plan::Doubt {
                plan = Plan::UntilFirstPicture;
            }
            // Nothing is asked of the engine until a feature needs pixels.
            if requests.changed().await.is_err() {
                return Ok(());
            }
            continue;
        }
        if fixed_raster.is_none() || reacquire == Some(request.generation) {
            request.reduction = None;
        }
        // A new region for every subscription; the one before is kept for a
        // while after its stream (see `Lingering`).
        let mut region = None;
        if let (Plan::Shared, Some(bytes)) = (plan, native_bytes) {
            match (setup.create)(bytes as usize) {
                Ok(created) => {
                    region = Some(shared::Lingering::new(created, setup.timing.linger));
                }
                Err(error) => {
                    plan = Plan::Inline;
                    report(Some(Fallback::RegionError(error.to_string())));
                }
            }
        }
        let result = if let Some(region) = region.as_mut().and_then(shared::Lingering::region) {
            // Until a picture has come through a region, a subscription owes
            // one in time: the stream it replaces was showing pictures.
            let proven =
                state.lock().unwrap().video_transport == Some(VideoTransport::SharedMemory);
            let subscription = shared::receive_shared(
                &mut video,
                token,
                ctx,
                state,
                request,
                fixed_raster,
                region,
                setup.timing,
                (!proven).then_some(setup.timing.first_picture),
            );
            tokio::select! {
                biased;
                changed = requests.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    continue;
                }
                ended = subscription => match ended {
                    Ok(result) => result,
                    Err(shared::Stop::Evidence(fallback)) => {
                        // The same request again, in-band, as every later one.
                        plan = Plan::Inline;
                        report(Some(fallback));
                        continue;
                    }
                    Err(shared::Stop::Silent) => {
                        // The same request again, in-band, to see whether the
                        // emulator has pictures.
                        plan = Plan::Doubt;
                        continue;
                    }
                }
            }
        } else {
            let shown = state.lock().unwrap().delivered;
            let first_picture = async {
                loop {
                    time::sleep(setup.timing.picture_poll).await;
                    if state.lock().unwrap().delivered > shown {
                        return;
                    }
                }
            };
            let doubt = time::sleep(setup.timing.first_picture);
            tokio::select! {
                biased;
                changed = requests.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    continue;
                }
                () = first_picture, if matches!(plan, Plan::UntilFirstPicture | Plan::Doubt) => {
                    if plan == Plan::Doubt {
                        // The emulator has pictures, as before, and the
                        // region got none in between.
                        plan = Plan::Inline;
                        report(Some(Fallback::NoPicture {
                            seconds: setup.timing.first_picture.as_secs(),
                        }));
                    } else {
                        // The screen is up: the same request again, through a
                        // region.
                        plan = Plan::Shared;
                    }
                    continue;
                }
                () = doubt, if plan == Plan::Doubt => {
                    // Nothing here either: the emulator had no picture, which
                    // says nothing about the region.
                    plan = Plan::UntilFirstPicture;
                    continue;
                }
                result = receive(&mut video, token, ctx, state, request, fixed_raster) => result,
            }
        };
        if request.reduction.is_none() {
            return result;
        }
        reacquire = Some(request.generation);
        if result.is_err() {
            fixed_raster = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{
        Display, Input,
        engine_rpc::{Empty, Rotation},
    };
    use eframe::egui::{pos2, vec2};
    use tokio::sync::mpsc;

    fn frame([width, height]: [u32; 2], rotation: i32) -> Frame {
        Frame {
            format: Some(Format {
                format: 1,
                width,
                height,
                rotation: Some(Rotation { value: rotation }),
                ..Default::default()
            }),
            pixels: vec![0; (width * height * 4) as usize].into(),
        }
    }

    #[test]
    fn covered_physical_layout_is_debounced_and_changes_restore_native() {
        let native = NativeDisplay {
            size: [1600, 900],
            rotation: 0,
        };
        let rect = Rect::from_min_size(pos2(1.2, 96.875), vec2(900., 506.25));
        let layout = Layout::measure(rect, 1.25, native).unwrap();
        assert_eq!(layout.physical_size, [1125, 633]);
        assert_eq!(layout.reduction.size, [1136, 639]);
        assert!(Layout::measure(rect, 2., native).is_none());
        let dpi_change = Layout::measure(rect, 1.5, native).unwrap();

        let (mut preference, requests) = Preference::new();
        let full = Source::default();
        let now = Instant::now();
        assert!(requests.borrow().reduction.is_none());
        preference.set_enabled(true);
        assert_eq!(
            preference.update_layout(Some(layout), full, now),
            Some(RESIZE_SETTLE)
        );
        assert!(requests.borrow().reduction.is_none());
        preference.update_layout(Some(layout), full, now + RESIZE_SETTLE);
        assert_eq!(requests.borrow().reduction, Some(layout.reduction));
        preference.update_layout(Some(dpi_change), full, now + RESIZE_SETTLE);
        let resized = *requests.borrow();
        assert!(resized.reduction.is_none());
        assert!(!preference.accepts_native(full));
        preference.update_layout(Some(dpi_change), full, now + RESIZE_SETTLE * 2);
        assert!(requests.borrow().reduction.is_none());
        let fresh = Source {
            generation: resized.generation,
            ..full
        };
        preference.update_layout(Some(dpi_change), fresh, now + RESIZE_SETTLE * 2);
        assert_eq!(requests.borrow().reduction, Some(dpi_change.reduction));
        preference.update_layout(None, fresh, now + RESIZE_SETTLE * 2);
        assert!(requests.borrow().reduction.is_none());
    }

    #[test]
    fn a_pause_and_its_end_make_older_frames_stale_and_ask_for_the_native_picture() {
        let (mut preference, requests) = Preference::new();
        let before = Source::default();
        assert!(preference.accepts_native(before) && preference.is_current(before));
        preference.set_paused(true);
        let paused = *requests.borrow();
        assert!(paused.paused && preference.paused());
        assert!(!preference.accepts_native(before) && !preference.is_current(before));
        // No layout becomes a reduction while nothing is subscribed.
        preference.set_enabled(true);
        let ctx = egui::Context::default();
        let native = NativeDisplay {
            size: [1600, 900],
            rotation: 0,
        };
        let rect = Rect::from_min_size(pos2(0., 0.), vec2(800., 450.));
        let latest = Source {
            generation: paused.generation,
            ..before
        };
        preference.update(&ctx, rect, true, Some(native), latest);
        assert_eq!(requests.borrow().generation, paused.generation);
        assert!(!preference.is_current(latest));

        preference.set_paused(false);
        let resumed = *requests.borrow();
        assert!(!resumed.paused && resumed.reduction.is_none());
        assert!(!preference.accepts_native(latest));
        let fresh = Source {
            generation: resumed.generation,
            ..before
        };
        assert!(preference.accepts_native(fresh) && preference.is_current(fresh));
        // Asking for the state it is in changes nothing.
        preference.set_paused(false);
        assert_eq!(requests.borrow().generation, resumed.generation);
    }

    #[test]
    fn native_geometry_drives_contacts_and_capture_across_raster_switches() {
        let (input, mut events) = mpsc::unbounded_channel();
        let mut display = Display::new(Default::default(), input);
        let (preference, requests) = Preference::new();
        display.video_preference = Some(preference);
        let view = Rect::from_min_size(pos2(0., 0.), vec2(160., 90.));
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let full = Source::default();
            display.upload(ctx, frame([160, 90], 0), full);
            assert_eq!(display.take_new_native_shot().unwrap().width, 160);
            display.contact = Some((78, 31));
            display.held.push("A".into());

            display.set_adaptive_video(true);
            display
                .video_preference
                .as_mut()
                .unwrap()
                .request(Some(Reduction {
                    size: [80, 45],
                    native: NativeDisplay {
                        size: [160, 90],
                        rotation: 0,
                    },
                }));
            let scaled = Source {
                generation: requests.borrow().generation,
                scaled: true,
                ..full
            };
            assert!(display.native_shot().is_none());
            display.upload(ctx, frame([80, 45], 0), scaled);
            assert_eq!(display.shot().unwrap().width, 80);
            assert_eq!(display.coordinates(view.center(), view), (80, 45));
            assert_eq!(display.coordinates(view.max, view), (159, 89));
            assert_eq!(display.contact, Some((78, 31)));
            assert_eq!(display.held, ["A"]);
            assert!(events.try_recv().is_err());
            assert!(display.take_new_native_shot().is_none());

            display.set_adaptive_video(false);
            display.upload(ctx, frame([160, 90], 0), full); // Queued before the current request.
            assert!(display.take_new_native_shot().is_none());
            let fresh = Source {
                generation: requests.borrow().generation,
                ..full
            };
            display.upload(ctx, frame([160, 90], 0), fresh);
            assert_eq!(display.take_new_native_shot().unwrap().width, 160);
            assert_eq!(display.contact, Some((78, 31)));
            assert_eq!(display.held, ["A"]);
            assert!(events.try_recv().is_err());

            display.state.lock().unwrap().blank = true;
            assert!(display.stream_available());
            assert!(display.native_shot().is_none());
            {
                let mut shared = display.state.lock().unwrap();
                shared.blank = false;
                shared.source.visibility_epoch += 1;
            }
            assert!(display.native_shot().is_none());
            let fresh = Source {
                visibility_epoch: 1,
                ..fresh
            };
            display.upload(ctx, frame([160, 90], 0), fresh);
            assert!(display.native_shot().is_some());

            display.upload(ctx, frame([90, 160], 1), fresh);
            let portrait = Rect::from_min_size(pos2(0., 0.), vec2(90., 160.));
            assert_eq!(display.coordinates(portrait.max, portrait), (0, 89));
            let Input::Touch(release) = events.try_recv().unwrap() else {
                panic!("expected touch release")
            };
            assert_eq!(
                (
                    release.touches[0].x,
                    release.touches[0].y,
                    release.touches[0].pressure
                ),
                (78, 31, 0)
            );
            let Input::Key(release) = events.try_recv().unwrap() else {
                panic!("expected key release on a native rotation")
            };
            assert_eq!((release.key.as_str(), release.event), ("A", 1));
            assert!(display.held.is_empty());
        });
    }

    #[test]
    fn incompatible_reduced_reply_reacquires_native_geometry() {
        let reduction = Reduction {
            size: [80, 45],
            native: NativeDisplay {
                size: [160, 90],
                rotation: 0,
            },
        };
        let mut scaled = frame([80, 45], 0);
        assert!(reduction.accepts(&scaled).unwrap());
        assert!(!reduction.accepts(&frame([45, 80], 1)).unwrap());
        assert!(reduction.accepts(&frame([160, 90], 0)).is_err());
        scaled.format.as_mut().unwrap().display = 1;
        assert!(!reduction.accepts(&scaled).unwrap());
        scaled.format.as_mut().unwrap().display = 0;
        scaled.format.as_mut().unwrap().folded_display = Some(Empty {});
        assert!(!reduction.accepts(&scaled).unwrap());
        scaled.format.as_mut().unwrap().folded_display = None;
        scaled.format.as_mut().unwrap().display_mode = Some(1);
        assert!(!reduction.accepts(&scaled).unwrap());
    }

    #[test]
    fn only_a_verified_fixed_primary_raster_is_eligible() {
        let ini = "hw.lcd.width = 1600\nhw.lcd.height = 900\nhw.sensor.hinge = false\nhw.sensor.roll = false\nhw.display1.width = 0\n";
        let size = parse_fixed_raster(ini).unwrap();
        assert_eq!(size, [1600, 900]);
        for unsupported in [
            "hw.device.name = resizable",
            "hw.resizable.configs = phone-0-1080-2400-420",
            "hw.sensor.hinge = true",
            "hw.displayRegion.0.1.width = 408",
            "hw.display2.width = 640",
        ] {
            assert!(parse_fixed_raster(&format!("{ini}{unsupported}\n")).is_none());
        }
        let rotated = NativeDisplay {
            size: [900, 1600],
            rotation: 1,
        };
        assert!(rotated.matches_raster(size));
        let ambiguous = NativeDisplay {
            size: [1280, 720],
            rotation: 0,
        };
        assert!(!ambiguous.matches_raster(size));

        use prost::Message;
        let wire = Format::decode(&[0x08, 1, 0x18, 80, 0x20, 45, 0x40, 2][..]).unwrap();
        assert_eq!(wire.display_mode, Some(2));
        assert!(NativeDisplay::conventional(&wire).is_none());
    }

    #[test]
    fn the_shared_transport_request_and_reply_have_the_emulators_wire_format() {
        use super::shared::shared_frame_len;
        use crate::display::{engine_rpc::Transport, shared_frames::MMAP};
        use prost::Message;
        let request = Format {
            format: 1,
            width: 800,
            height: 450,
            transport: Some(Transport {
                channel: MMAP,
                handle: "file:///tmp/f".into(),
            }),
            ..Default::default()
        };
        // ImageFormat.transport is field 6: ImageTransport{channel = 1, handle = 2}.
        let bytes = request.encode_to_vec();
        let tail = [&[0x32, 17, 0x08, 1, 0x12, 13][..], b"file:///tmp/f"].concat();
        assert!(bytes.ends_with(&tail), "{bytes:?}");
        // A reply for the shared file: format with transport, no image bytes.
        let reply = Frame {
            format: Some(request.clone()),
            pixels: Default::default(),
        };
        let decoded = Frame::decode(&reply.encode_to_vec()[..]).unwrap();
        let format = decoded.format.unwrap();
        assert_eq!(
            shared_frame_len(&format, &decoded.pixels),
            Some(800 * 450 * 4)
        );
        // Inline pixels, other channels, other formats and empty rasters are not read from the file.
        assert_eq!(shared_frame_len(&format, &[1, 2, 3, 4]), None);
        let mut inline = format.clone();
        inline.transport = None;
        assert_eq!(shared_frame_len(&inline, &[]), None);
        let mut png = format.clone();
        png.format = 0;
        assert_eq!(shared_frame_len(&png, &[]), None);
        let mut blank = format;
        blank.width = 0;
        assert_eq!(shared_frame_len(&blank, &[]), None);
    }

    #[test]
    fn the_end_of_the_stream_is_told_in_every_language_and_other_errors_verbatim() {
        use crate::{display::StreamError, ui::i18n::Locale};
        let ended: Box<dyn std::error::Error + Send + Sync> = StreamEnded.into();
        assert_eq!(StreamError::from_error(ended.as_ref()), StreamError::Ended);
        assert_eq!(ended.to_string(), "O motor encerrou o fluxo de vídeo.");
        let texts =
            [Locale::PtBr, Locale::EnUs, Locale::EsEs].map(|l| StreamError::Ended.describe(l));
        assert_eq!(texts[0], "O motor encerrou o fluxo de vídeo.");
        assert_eq!(texts[1], "The engine closed the video stream.");
        assert_eq!(texts[2], "El motor cerró el flujo de vídeo.");
        let other: Box<dyn std::error::Error + Send + Sync> = "status: Unavailable".into();
        let other = StreamError::from_error(other.as_ref());
        assert_eq!(other, StreamError::Other("status: Unavailable".into()));
        assert_eq!(other.describe(Locale::EsEs), "status: Unavailable");
    }

    #[test]
    fn the_transport_variable_turns_shared_memory_on_or_off_and_the_system_decides_without_it() {
        for by_default in [true, false] {
            assert_eq!(
                shared_wanted(Some(" Inline "), by_default),
                Err(Fallback::TurnedOff)
            );
            assert_eq!(shared_wanted(Some("shared"), by_default), Ok(()));
            assert_eq!(shared_wanted(Some(" SHARED\n"), by_default), Ok(()));
        }
        for undecided in [None, Some(""), Some("mmap")] {
            assert_eq!(shared_wanted(undecided, true), Ok(()));
            assert_eq!(shared_wanted(undecided, false), Err(Fallback::OffByDefault));
        }
        // By default only where it has run against the engine.
        assert_eq!(shared_frames::DEFAULT, cfg!(windows));
    }

    #[test]
    fn the_transport_is_described_in_every_language_and_reported_once_per_change() {
        use crate::ui::i18n::Locale;
        let transports = [
            VideoTransport::SharedMemory,
            VideoTransport::Inline { fallback: None },
            VideoTransport::Inline {
                fallback: Some(Fallback::ResizableDisplay),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::EngineSentInline),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::EngineRefused {
                    code: "PermissionDenied".into(),
                    message: "Rejected shared memory handle".into(),
                }),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::RegionError("Acesso negado. (os error 5)".into())),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::TurnedOff),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::NotWritten),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::FrameTooLarge {
                    frame: 5_760_000,
                    region: 1_440_000,
                }),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::OffByDefault),
            },
            VideoTransport::Inline {
                fallback: Some(Fallback::NoPicture { seconds: 3 }),
            },
        ];
        for locale in [Locale::PtBr, Locale::EnUs, Locale::EsEs] {
            let texts: Vec<_> = transports.iter().map(|t| t.describe(locale)).collect();
            for (index, text) in texts.iter().enumerate() {
                assert!(text.ends_with('.') || text.ends_with('”'), "{text}");
                assert!(
                    texts[index + 1..].iter().all(|other| other != text),
                    "{text}"
                );
            }
            // The engine's and the system's own words are quoted verbatim.
            assert!(
                texts[4].contains("(PermissionDenied)")
                    && texts[4].ends_with("“Rejected shared memory handle”")
            );
            assert!(texts[5].ends_with("“Acesso negado. (os error 5)”"));
            // The variable and its values are written as they are typed.
            assert!(texts[6].contains("RUSTEMUDROID_VIDEO_TRANSPORT=inline"));
            assert!(texts[9].contains("RUSTEMUDROID_VIDEO_TRANSPORT=shared"));
            assert!(texts[10].contains("3 s"));
            assert!(texts[8].contains("5760000 bytes") && texts[8].contains("1440000 bytes"));
        }
        assert_eq!(
            transports[4].describe(Locale::PtBr),
            "Vídeo do painel: quadros dentro do canal de controle; a memória compartilhada não foi usada porque o motor recusou a região de memória (PermissionDenied); mensagem do motor: “Rejected shared memory handle”"
        );
        assert_eq!(
            transports[6].describe(Locale::EsEs),
            "Vídeo del panel: fotogramas dentro del canal de control; no se usó la memoria compartida porque la variable RUSTEMUDROID_VIDEO_TRANSPORT=inline la desactivó."
        );
        assert_eq!(
            transports[6].describe(Locale::EnUs),
            "Panel video: frames inside the control channel; shared memory was not used because the variable RUSTEMUDROID_VIDEO_TRANSPORT=inline turned it off."
        );
        assert_eq!(
            transports[9].describe(Locale::PtBr),
            "Vídeo do painel: quadros dentro do canal de controle; a memória compartilhada não foi usada porque neste sistema ela ainda não foi testada com o motor e só é ligada pela variável RUSTEMUDROID_VIDEO_TRANSPORT=shared."
        );
        assert_eq!(
            transports[9].describe(Locale::EnUs),
            "Panel video: frames inside the control channel; shared memory was not used because on this system it has not been tested with the engine yet and is only turned on by the variable RUSTEMUDROID_VIDEO_TRANSPORT=shared."
        );
        assert_eq!(
            transports[9].describe(Locale::EsEs),
            "Vídeo del panel: fotogramas dentro del canal de control; no se usó la memoria compartida porque en este sistema todavía no se ha probado con el motor y solo la activa la variable RUSTEMUDROID_VIDEO_TRANSPORT=shared."
        );
        assert_eq!(
            transports[7].describe(Locale::PtBr),
            "Vídeo do painel: quadros dentro do canal de controle; a memória compartilhada não foi usada porque o motor anunciou quadros e nada mudou na região de memória criada pelo painel."
        );
        assert_eq!(
            transports[7].describe(Locale::EnUs),
            "Panel video: frames inside the control channel; shared memory was not used because the engine announced frames and nothing changed in the memory region the panel created."
        );
        assert_eq!(
            transports[5].describe(Locale::EsEs),
            "Vídeo del panel: fotogramas dentro del canal de control; no se usó la memoria compartida porque no se pudo crear ni leer la región de memoria; mensaje del sistema: “Acesso negado. (os error 5)”"
        );
        assert_eq!(
            transports[8].describe(Locale::EnUs),
            "Panel video: frames inside the control channel; shared memory was not used because a frame of 5760000 bytes does not fit in the memory region of 1440000 bytes."
        );
        assert_eq!(
            transports[0].describe(Locale::EnUs),
            "Panel video: memory shared with the engine."
        );
        assert_eq!(
            transports[10].describe(Locale::PtBr),
            "Vídeo do painel: quadros dentro do canal de controle; a memória compartilhada não foi usada porque nenhuma imagem chegou pela região de memória em 3 s, embora o motor as enviasse dentro das mensagens antes e depois."
        );
        assert_eq!(
            transports[10].describe(Locale::EnUs),
            "Panel video: frames inside the control channel; shared memory was not used because no picture came through the memory region within 3 s, although the engine sent pictures inside the messages before and after."
        );
        assert_eq!(
            transports[10].describe(Locale::EsEs),
            "Vídeo del panel: fotogramas dentro del canal de control; no se usó la memoria compartida porque no llegó ninguna imagen por la región de memoria en 3 s, aunque el motor las enviaba dentro de los mensajes antes y después."
        );
        assert_eq!(
            transports[3].describe(Locale::EnUs),
            "Panel video: frames inside the control channel; shared memory was not used because the engine sent the frames inside the messages."
        );

        let mut display = Display::empty_for_capture_wait_test();
        assert_eq!(display.take_video_transport_change(), None);
        display
            .state
            .lock()
            .unwrap()
            .set_video_transport(VideoTransport::SharedMemory);
        assert_eq!(
            display.take_video_transport_change(),
            Some(VideoTransport::SharedMemory)
        );
        display
            .state
            .lock()
            .unwrap()
            .set_video_transport(VideoTransport::SharedMemory);
        assert_eq!(display.take_video_transport_change(), None);
        display
            .state
            .lock()
            .unwrap()
            .set_video_transport(transports[4].clone());
        assert_eq!(
            display.take_video_transport_change(),
            Some(transports[4].clone())
        );
    }
}
