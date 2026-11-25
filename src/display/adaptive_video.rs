//! Window-sized transport for the primary Android Emulator display.

use super::{DisplayResult, Format, Frame, Shared, authorized};
use eframe::egui::{self, Rect};
use std::{
    fs,
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

    pub fn accepts_native(&self, source: Source) -> bool {
        !source.scaled && self.requested.generation == source.generation
    }

    pub fn update(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        windowed: bool,
        native: Option<NativeDisplay>,
        source: Source,
    ) {
        if !self.enabled {
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
            return Err("O motor encerrou o fluxo de vídeo.".into());
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
) -> DisplayResult<()> {
    let mut reacquire = None;
    loop {
        let mut request = *requests.borrow_and_update();
        if fixed_raster.is_none() || reacquire == Some(request.generation) {
            request.reduction = None;
        }
        tokio::select! {
            biased;
            changed = requests.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
            }
            result = receive(&mut video, token, ctx, state, request, fixed_raster) => {
                if request.reduction.is_none() {
                    return result;
                }
                reacquire = Some(request.generation);
                if result.is_err() {
                    fixed_raster = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Display, Empty, Input, Rotation};
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
}
