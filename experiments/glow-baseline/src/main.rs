//! Standalone comparison leg for the production Glow texture upload.

mod capture {
    // The production upload accepts this RGBA8 shape. Vec -> Bytes keeps the
    // same allocation as the shared producer's frame.
    #[derive(Clone)]
    pub(crate) struct Shot {
        pub width: u32,
        pub height: u32,
        pub pixels: bytes::Bytes,
    }
}

#[path = "../../../src/display/native_texture.rs"]
mod native_texture;
#[path = "../../gpui-viewer/src/producer.rs"]
mod producer;
#[path = "../../gpui-viewer/src/source.rs"]
mod source;

use capture::Shot;
use clap::Parser;
use eframe::{egui, egui_glow::CallbackFn, glow::HasContext};
use native_texture::NativeTexture;
use producer::{Feed, Frame, Spec};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(about = "Measure the production Glow upload against a shared RGBA frame source")]
struct Options {
    #[arg(long, default_value_t = 1600)]
    width: u32,
    #[arg(long, default_value_t = 900)]
    height: u32,
    #[arg(long, default_value_t = 90)]
    fps: u32,
    #[arg(long, default_value_t = 10)]
    seconds: u32,
    #[arg(long, default_value_t = 2)]
    warmup: u32,
    #[arg(long, default_value_t = 1040)]
    window_width: u32,
    #[arg(long, default_value_t = 585)]
    window_height: u32,
    #[arg(long, allow_hyphen_values = true, value_name = "FILE|-")]
    raw: Option<PathBuf>,
}

impl Options {
    fn window(&self) -> [u32; 2] {
        [self.window_width, self.window_height]
    }

    fn spec(&self) -> Spec {
        Spec {
            width: self.width,
            height: self.height,
            fps: self.fps,
            seconds: self.seconds,
            warmup: self.warmup,
            raw: self.raw.clone(),
        }
    }
}

#[derive(Default)]
struct DrawStats {
    client: Option<[u32; 2]>,
    resized: bool,
    last: Option<u64>,
    first: Option<u64>,
    latency: Vec<f64>,
}

impl DrawStats {
    fn observe(&mut self, window: [u32; 2], started: bool, frame: Option<(u64, Instant)>) {
        self.resized |= started && self.client.is_some_and(|previous| previous != window);
        self.client = Some(window);
        if let Some((sequence, produced_at)) = frame
            && self.last.is_none_or(|last| sequence > last)
        {
            self.first.get_or_insert(sequence);
            self.last = Some(sequence);
            self.latency
                .push(produced_at.elapsed().as_secs_f64() * 1000.);
        }
    }

    fn latency(&mut self) -> Value {
        self.latency.sort_by(f64::total_cmp);
        let count = self.latency.len();
        let percentile =
            |quantile: f64| self.latency[(quantile * count as f64).ceil() as usize - 1];
        json!({"mean": self.latency.iter().sum::<f64>() / count as f64,
               "p50": percentile(0.50), "p95": percentile(0.95), "max": percentile(1.)})
    }
}

type Outcome = Rc<RefCell<Option<Result<Value, String>>>>;

struct Viewer {
    options: Options,
    adapter: String,
    output: Outcome,
    feed: Option<Feed>,
    window_deadline: Instant,
    size_command: Option<[u32; 2]>,
    texture: egui::TextureHandle,
    upload: NativeTexture,
    shot: Option<Shot>,
    measured_frame: Option<(u64, Instant)>,
    draw: Arc<Mutex<DrawStats>>,
    ending: bool,
}

impl Viewer {
    fn accept(&mut self, frame: Frame) {
        self.upload.advance();
        self.measured_frame = frame
            .measured
            .then_some((frame.sequence, frame.produced_at));
        self.shot = Some(Shot {
            width: self.options.width,
            height: self.options.height,
            pixels: frame.rgba.into(),
        });
    }

    fn finish(&self, ctx: &egui::Context, result: Result<Value, String>) {
        *self.output.borrow_mut() = Some(result);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn report(&self, status: producer::Status) -> Result<Value, String> {
        let (Some(start), Some(end)) = (status.started, status.ended) else {
            return Err("The shared source did not finish its measurement window".into());
        };
        let elapsed = end.duration_since(start).as_secs_f64();
        let mut draw = self.draw.lock().unwrap();
        let Some(window) = draw.client else {
            return Err("Glow did not paint a framebuffer".into());
        };
        let count = draw.latency.len();
        if elapsed == 0. || count == 0 || draw.resized || window != self.options.window() {
            return Err("No measured Glow frames or the physical client geometry changed".into());
        }
        let source = if self.options.raw.is_some() {
            "raw"
        } else {
            "synthetic"
        };
        Ok(json!({
            "backend": "glow", "adapter": self.adapter, "source": source, "layers": 1,
            "vsync_requested": true,
            "width": self.options.width, "height": self.options.height, "target_fps": self.options.fps,
            "window_width": window[0], "window_height": window[1], "warmup_seconds": self.options.warmup,
            "elapsed_seconds": elapsed, "produced": status.produced, "replaced": status.replaced,
            "late_slots": status.late_slots, "duplicates": status.duplicates,
            "processed": count, "processed_fps": count as f64 / elapsed,
            "first_processed_sequence": draw.first, "last_processed_sequence": draw.last,
            "latency_ms": draw.latency(),
            "pixel_transport": "CPU-visible RGBA8 to the production Glow texture upload",
            "bytes_per_frame": u64::from(self.options.width) * u64::from(self.options.height) * 4,
        }))
    }

    fn paint(&mut self, ctx: &egui::Context) {
        let started = self.feed.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let panel = ui.max_rect();
                let rect = if let Some(shot) = &self.shot {
                    let extent = egui::vec2(shot.width as f32, shot.height as f32);
                    let scale = (panel.width() / extent.x).min(panel.height() / extent.y);
                    let fitted = egui::Rect::from_center_size(panel.center(), extent * scale);
                    self.upload
                        .queue(ui.painter(), fitted, self.texture.id(), shot);
                    ui.painter().image(
                        self.texture.id(),
                        fitted,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        egui::Color32::WHITE,
                    );
                    fitted
                } else {
                    panel
                };
                // This callback runs in the Glow paint pass after the production
                // upload and image commands, rather than during layout/repaint requests.
                let draw = self.draw.clone();
                let frame = self.measured_frame;
                ui.painter().add(egui::PaintCallback {
                    rect,
                    callback: Arc::new(CallbackFn::new(move |info, _| {
                        draw.lock()
                            .unwrap()
                            .observe(info.screen_size_px, started, frame);
                    })),
                });
            });
    }
}

impl eframe::App for Viewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(feed) = &self.feed {
            let status = feed.status();
            if let Some(error) = status.error.as_ref() {
                self.finish(ctx, Err(error.clone()));
                return;
            }
            if self.ending {
                self.finish(ctx, self.report(status));
                return;
            }
            if status.ended.is_some() {
                // Give the final available sequence one paint pass before closing.
                self.ending = true;
                ctx.request_repaint();
            }
            if let Some(frame) = feed.take() {
                self.accept(frame);
            }
        } else if Instant::now() >= self.window_deadline {
            self.finish(
                ctx,
                Err(format!(
                    "Could not obtain physical client size {:?}",
                    self.options.window()
                )),
            );
            return;
        } else {
            let painted = self.draw.lock().unwrap().client;
            let desired = self.options.window();
            if painted == Some(desired) {
                let wake = ctx.clone();
                match producer::spawn(self.options.spec(), move || wake.request_repaint()) {
                    Ok(feed) => self.feed = Some(feed),
                    Err(error) => self.finish(ctx, Err(error.to_string())),
                }
            } else {
                if let Some(observed) = painted
                    && self.size_command != Some(observed)
                {
                    self.size_command = Some(observed);
                    let points =
                        egui::vec2(desired[0] as f32, desired[1] as f32) / ctx.pixels_per_point();
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(points));
                }
                ctx.request_repaint_after(Duration::from_millis(25));
            }
        }
        self.paint(ctx);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse();
    if options.window().contains(&0) {
        return Err("Physical window dimensions must be positive".into());
    }
    let [width, height] = options.window();
    let viewport = egui::ViewportBuilder::default().with_inner_size([width as f32, height as f32]);
    let native = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        vsync: true,
        ..Default::default()
    };
    let result: Outcome = Default::default();
    let output = result.clone();
    eframe::run_native(
        "RustEmuDroid Glow baseline",
        native,
        Box::new(move |creation| {
            let gl = creation.gl.as_deref();
            if !native_texture::supported(gl) {
                return Err(
                    "The production native texture uploader requires OpenGL 3 or newer".into(),
                );
            }
            let adapter = unsafe { gl.unwrap().get_parameter_string(eframe::glow::RENDERER) };
            let texture = creation.egui_ctx.load_texture(
                "Android",
                egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT),
                egui::TextureOptions::LINEAR,
            );
            Ok(Box::new(Viewer {
                options,
                output,
                texture,
                adapter,
                feed: None,
                window_deadline: Instant::now() + Duration::from_secs(8),
                size_command: None,
                upload: Default::default(),
                shot: None,
                measured_frame: None,
                draw: Default::default(),
                ending: false,
            }))
        }),
    )?;
    let summary = result
        .borrow_mut()
        .take()
        .ok_or("The benchmark window closed before completing")??;
    println!("{summary}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_callbacks_count_distinct_measured_sequences_in_the_fixed_physical_client() {
        let mut draw = DrawStats::default();
        let window = [1040, 585];
        let now = Instant::now();
        draw.observe([1280, 720], false, None);
        draw.observe(window, false, None);
        draw.observe(window, true, Some((52, now)));
        draw.observe(window, true, Some((52, now)));
        draw.observe(window, true, Some((50, now)));
        draw.observe(window, true, Some((55, now)));
        assert_eq!(
            (draw.first, draw.last, draw.latency.len()),
            (Some(52), Some(55), 2)
        );
        assert!(!draw.resized);
        draw.observe([1039, 585], true, None);
        assert!(draw.resized);
    }
}
