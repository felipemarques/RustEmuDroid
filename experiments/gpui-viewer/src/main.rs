use gpui::{
    App, AppContext, Application, Bounds, Context, Corners, FocusHandle, GpuSpecs, IntoElement,
    Pixels, Render, RenderImage, Size, Styled, TitlebarOptions, Window, WindowBounds,
    WindowOptions, canvas, div, point, prelude::*, px, rgb, size,
};
use rustemudroid_gpui_viewer::producer::{self, Feed, Frame, Spec, Status};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    path::PathBuf,
    process::ExitCode,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const HELP: &str = "gpui-viewer [--width 1600] [--height 900] [--fps 90] [--seconds 10] \
    [--warmup 2] [--window-width 1040] [--window-height 585] [--raw <FILE|->] [--expect-gpu <NAME>] \
    [--demo] [--lang pt-BR|en-US|es-ES]\nWindow dimensions are physical pixels.";

struct Options {
    input: Spec,
    window_width: u32,
    window_height: u32,
    expect_gpu: Option<String>,
    demo: bool,
    language: Language,
}

impl Options {
    fn client_px(&self) -> [u32; 2] {
        [self.window_width, self.window_height]
    }
}

#[derive(Clone, Copy)]
enum Language {
    Pt,
    En,
    Es,
}

impl Language {
    fn strings(self) -> [&'static str; 4] {
        match self {
            Self::Pt => ["Pausar", "Continuar", "Tela cheia", "Sair da tela cheia"],
            Self::En => ["Pause", "Resume", "Fullscreen", "Exit fullscreen"],
            Self::Es => [
                "Pausar",
                "Continuar",
                "Pantalla completa",
                "Salir de pantalla completa",
            ],
        }
    }
}

fn parse() -> Result<Option<Options>, String> {
    let mut opts = Options {
        input: Spec {
            width: 1600,
            height: 900,
            fps: 90,
            seconds: 10,
            warmup: 2,
            raw: None,
        },
        window_width: 1040,
        window_height: 585,
        expect_gpu: None,
        demo: false,
        language: Language::Pt,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            println!("{HELP}");
            return Ok(None);
        }
        if flag == "--demo" {
            opts.demo = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {}", flag.to_string_lossy()))?;
        let number = || {
            value
                .to_str()
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or_else(|| format!("invalid number for {}", flag.to_string_lossy()))
        };
        match flag.to_str() {
            Some("--width") => opts.input.width = number()?,
            Some("--height") => opts.input.height = number()?,
            Some("--fps") => opts.input.fps = number()?,
            Some("--seconds") => opts.input.seconds = number()?,
            Some("--warmup") => opts.input.warmup = number()?,
            Some("--window-width") => opts.window_width = number()?,
            Some("--window-height") => opts.window_height = number()?,
            Some("--raw") => opts.input.raw = Some(PathBuf::from(value)),
            Some("--lang") => {
                opts.language = match value.to_str().map(str::to_ascii_lowercase).as_deref() {
                    Some("pt-br") => Language::Pt,
                    Some("en-us") => Language::En,
                    Some("es-es") => Language::Es,
                    _ => return Err("expected --lang pt-BR, en-US, or es-ES".into()),
                }
            }
            Some("--expect-gpu") => {
                opts.expect_gpu = Some(value.into_string().map_err(|_| "GPU name must be UTF-8")?)
            }
            _ => return Err(format!("unknown option {}", flag.to_string_lossy())),
        }
    }
    if !(128..=8192).contains(&opts.window_width) || !(128..=8192).contains(&opts.window_height) {
        return Err("window dimensions must be in 128..=8192".into());
    }
    Ok(Some(opts))
}

struct Image {
    image: Arc<RenderImage>,
    sequence: u64,
    produced_at: Instant,
    measured: bool,
}

impl Image {
    fn from_rgba(mut frame: Frame, input: &Spec) -> Self {
        // GPUI stores BGRA. Reuse the producer-owned Vec for the channel swap,
        // then pass it to GPUI; the sprite atlas still uploads it to the GPU.
        for pixel in frame.rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let buffer = image::RgbaImage::from_raw(input.width, input.height, frame.rgba)
            .expect("producer emits one validated RGBA frame");
        Self {
            image: Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                buffer
            )])),
            sequence: frame.sequence,
            produced_at: frame.produced_at,
            measured: frame.measured,
        }
    }
}

#[derive(Default)]
struct Measurements {
    first: Option<u64>,
    last: Option<u64>,
    lags: Vec<f64>,
}

impl Measurements {
    fn painted(&mut self, image: &Image) {
        let now = Instant::now();
        if !image.measured || self.last.is_some_and(|last| image.sequence <= last) {
            return;
        }
        self.first.get_or_insert(image.sequence);
        self.last = Some(image.sequence);
        self.lags
            .push((now - image.produced_at).as_secs_f64() * 1000.0);
    }

    fn report(
        &mut self,
        status: Status,
        opts: &Options,
        gpu: &GpuSpecs,
        viewport: Bounds<Pixels>,
        scale: f32,
    ) -> Result<Value, String> {
        if let Some(error) = status.error {
            return Err(error);
        }
        let (Some(start), Some(end)) = (status.started, status.ended) else {
            return Err("measurement did not finish".into());
        };
        let elapsed = end.saturating_duration_since(start).as_secs_f64();
        if self.lags.is_empty() || elapsed <= 0.0 {
            return Err("no new frame was painted during the measurement window".into());
        }
        self.lags.sort_unstable_by(f64::total_cmp);
        let count = self.lags.len();
        let percentile = |percent: usize| self.lags[(count * percent).div_ceil(100) - 1];
        Ok(json!({
            "backend": "gpui",
            "source": if opts.input.raw.is_some() { "raw" } else { "synthetic" },
            "width": opts.input.width, "height": opts.input.height, "target_fps": opts.input.fps,
            "window_width": opts.window_width, "window_height": opts.window_height,
            "client_width_px": (f32::from(viewport.size.width) * scale).round() as u32,
            "client_height_px": (f32::from(viewport.size.height) * scale).round() as u32,
            "scale_factor": scale, "layers": 1, "warmup_seconds": opts.input.warmup,
            "bytes_per_frame": opts.input.width as u64 * opts.input.height as u64 * 4,
            "pixel_transport": "rgba_vec_in_place_bgra_then_gpui_atlas_upload",
            "requested_seconds": opts.input.seconds, "elapsed_seconds": elapsed,
            "produced": status.produced, "replaced": status.replaced, "late_slots": status.late_slots,
            "duplicates": status.duplicates, "processed": count, "processed_fps": count as f64 / elapsed,
            "first_processed_sequence": self.first, "last_processed_sequence": self.last,
            "measurement_point": "first successful GPUI paint_image for the source sequence",
            "latency_ms": {"mean": self.lags.iter().sum::<f64>() / count as f64,
                "p50": percentile(50), "p95": percentile(95), "max": *self.lags.last().unwrap()},
            "adapter": gpu,
        }))
    }
}

type Outcome = Rc<RefCell<Option<Result<Value, String>>>>;

struct Viewer {
    opts: Rc<Options>,
    feed: Option<Feed>,
    gpu: GpuSpecs,
    current: Option<Rc<Image>>,
    retired: Option<Arc<RenderImage>>,
    stats: Rc<RefCell<Measurements>>,
    outcome: Outcome,
    finishing: bool,
    window_deadline: Instant,
    resize_request: Option<([u32; 2], f32)>,
    measured_scale: Option<f32>,
    paused: bool,
    focus: FocusHandle,
}

fn physical_size(size: Size<Pixels>, scale: f32) -> [u32; 2] {
    [size.width, size.height].map(|dimension| (f32::from(dimension) * scale).round() as u32)
}

fn fitted(viewport: Bounds<Pixels>, input: &Spec) -> Bounds<Pixels> {
    let width = f32::from(viewport.size.width);
    let height = f32::from(viewport.size.height);
    let scale = (width / input.width as f32).min(height / input.height as f32);
    let child = size(
        px(input.width as f32 * scale),
        px(input.height as f32 * scale),
    );
    let origin = point(
        viewport.origin.x + (viewport.size.width - child.width) / 2.0,
        viewport.origin.y + (viewport.size.height - child.height) / 2.0,
    );
    Bounds::new(origin, child)
}

fn exit_with(outcome: &Outcome, result: Result<Value, String>, cx: &mut App) {
    *outcome.borrow_mut() = Some(result);
    cx.defer(|cx| cx.quit());
}

impl Viewer {
    fn prepare_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.finishing {
            return Ok(());
        }
        let scale = window.scale_factor();
        if !scale.is_finite() || scale <= 0.0 {
            return Err(format!("GPUI reported invalid display scale {scale}"));
        }
        let [width, height] = self.opts.client_px();
        let [actual_w, actual_h] = physical_size(window.viewport_size(), scale);
        if let Some(original_scale) = self.measured_scale {
            if !self.opts.demo
                && ([actual_w, actual_h] != [width, height] || scale != original_scale)
            {
                return Err(format!(
                    "Client changed during measurement: expected {width}x{height} physical pixels at scale {original_scale}, got {actual_w}x{actual_h} at scale {scale}"
                ));
            }
            return Ok(());
        }
        if [actual_w, actual_h] != [width, height] {
            if Instant::now() >= self.window_deadline {
                return Err(format!(
                    "Could not obtain {width}x{height} physical client pixels: GPUI reported {actual_w}x{actual_h} at scale {scale}"
                ));
            }
            if self.resize_request != Some(([actual_w, actual_h], scale)) {
                self.resize_request = Some(([actual_w, actual_h], scale));
                window.resize(size(px(width as f32 / scale), px(height as f32 / scale)));
            }
            window.request_animation_frame();
            return Ok(());
        }

        let (wake, incoming) = async_channel::bounded(1);
        let wake = move || {
            let _ = wake.try_send(());
        };
        let input = self.opts.input.clone();
        let feed = if self.opts.demo {
            producer::spawn_demo(input, wake)
        } else {
            producer::spawn(input, wake)
        };
        self.feed = Some(feed.map_err(|error| error.to_string())?);
        self.measured_scale = Some(scale);
        cx.spawn(async move |view, cx| {
            while incoming.recv().await.is_ok() {
                if view
                    .update(cx, |view, cx| {
                        if !view.paused {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Err(error) = self.prepare_window(window, cx) {
            self.finishing = true;
            self.feed = None;
            exit_with(&self.outcome, Err(error), cx);
        }
        if !self.paused
            && let Some(frame) = self.feed.as_ref().and_then(Feed::take)
        {
            // Keep the preceding scene's image live until the following replacement.
            if let Some(retired) = self.retired.take() {
                let _ = window.drop_image(retired);
            }
            let image = Rc::new(Image::from_rgba(frame, &self.opts.input));
            self.retired = self
                .current
                .replace(image)
                .map(|old| Arc::clone(&old.image));
        }
        let status = self.feed.as_ref().map(Feed::status).unwrap_or_default();
        let ends = status.ended.is_some()
            && !self.finishing
            && (!self.opts.demo || status.error.is_some());
        self.finishing |= ends;
        let measuring = self.feed.is_some() && !self.opts.demo;
        let image = self.current.clone();
        let opts = Rc::clone(&self.opts);
        let stats = Rc::clone(&self.stats);
        let outcome = Rc::clone(&self.outcome);
        let gpu = self.gpu.clone();
        let picture = canvas(
            |_, _, _| (),
            move |bounds, (), win, cx| {
                if measuring && physical_size(bounds.size, win.scale_factor()) != opts.client_px() {
                    let [actual_w, actual_h] = physical_size(bounds.size, win.scale_factor());
                    let [width, height] = opts.client_px();
                    exit_with(&outcome, Err(format!(
                        "Drawable viewport differs from requested {width}x{height} physical client pixels: {actual_w}x{actual_h}"
                    )), cx);
                    return;
                }
                if let Some(image) = image {
                    if let Err(error) = win.paint_image(
                        fitted(bounds, &opts.input),
                        Corners::default(),
                        Arc::clone(&image.image),
                        0,
                        false,
                    ) {
                        exit_with(
                            &outcome,
                            Err(format!("GPUI sprite upload/paint failed: {error}")),
                            cx,
                        );
                        return;
                    }
                    if measuring {
                        stats.borrow_mut().painted(&image);
                    }
                }
                if ends {
                    let result =
                        stats
                            .borrow_mut()
                            .report(status, &opts, &gpu, bounds, win.scale_factor());
                    exit_with(&outcome, result, cx);
                }
            },
        )
        .size_full()
        .bg(rgb(0x000000));
        if !self.opts.demo {
            return picture.into_any_element();
        }
        let [pause, resume, enter_fullscreen, exit_fullscreen] = self.opts.language.strings();
        let fullscreen = window.is_fullscreen();
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(|event, win, _| {
                if event.keystroke.key == "escape" {
                    win.remove_window();
                }
            })
            .child(picture)
            .child(
                div()
                    .absolute()
                    .top(px(16.))
                    .left(px(16.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x142332))
                    .text_color(rgb(0xe8f1fb))
                    .child("RustEmuDroid · GPUI")
                    .child(
                        div()
                            .id("demo-pause")
                            .cursor_pointer()
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .bg(rgb(0x294765))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.paused = !view.paused;
                                cx.notify();
                            }))
                            .child(if self.paused { resume } else { pause }),
                    )
                    .child(
                        div()
                            .id("demo-fullscreen")
                            .cursor_pointer()
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .bg(rgb(0x294765))
                            .on_click(cx.listener(|_, _, win, cx| {
                                win.toggle_fullscreen();
                                cx.notify();
                            }))
                            .child(if fullscreen {
                                exit_fullscreen
                            } else {
                                enter_fullscreen
                            }),
                    ),
            )
            .into_any_element()
    }
}

fn run(opts: Options) -> Result<Value, String> {
    let demo = opts.demo;
    let opts = Rc::new(opts);
    let outcome = Outcome::default();
    let result = Rc::clone(&outcome);
    Application::new().run(move |cx| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(
            None,
            size(px(opts.window_width as f32), px(opts.window_height as f32)),
            cx,
        );
        let view_result = Rc::clone(&result);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("RustEmuDroid · GPUI".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..WindowOptions::default()
            },
            |win, cx| {
                let gpu = win.gpu_specs().unwrap_or_default();
                let matches = opts.expect_gpu.as_ref().is_none_or(|name| {
                    gpu.device_name
                        .to_lowercase()
                        .contains(&name.to_lowercase())
                });
                let unsupported =
                    gpu.device_name.is_empty() || gpu.is_software_emulated || !matches;
                if unsupported {
                    exit_with(
                        &view_result,
                        Err(format!("unexpected rendering adapter: {gpu:?}")),
                        cx,
                    );
                }
                cx.new(|cx: &mut Context<Viewer>| {
                    let focus = cx.focus_handle();
                    if demo {
                        focus.focus(win);
                    }
                    Viewer {
                        opts,
                        feed: None,
                        gpu,
                        current: None,
                        retired: None,
                        stats: Default::default(),
                        outcome: view_result,
                        finishing: unsupported,
                        window_deadline: Instant::now() + Duration::from_secs(8),
                        resize_request: None,
                        measured_scale: None,
                        paused: false,
                        focus,
                    }
                })
            },
        );
        if let Err(error) = opened {
            exit_with(&result, Err(format!("GPUI window failed: {error}")), cx);
        } else {
            cx.activate(true);
        }
    });
    outcome.borrow_mut().take().unwrap_or_else(|| {
        if demo {
            Ok(Value::Null)
        } else {
            Err("window closed before the measurement completed".into())
        }
    })
}

fn main() -> ExitCode {
    match parse().and_then(|opts| opts.map(run).transpose()) {
        Ok(Some(report)) if !report.is_null() => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gpui-viewer: {error}");
            ExitCode::FAILURE
        }
    }
}
