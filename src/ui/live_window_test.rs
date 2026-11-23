//! Opt-in Windows integration test. Select an idle AVD explicitly.
use super::*;
use eframe::App;
use std::{cell::Cell, io::Write, path::Path, rc::Rc, time::Instant};
use winit::platform::windows::EventLoopBuilderExtWindows;

struct WindowTest {
    app: Launcher,
    output: Option<PathBuf>,
    step: usize,
    pending: bool,
    next: Instant,
    deadline: Instant,
    closing_frames: usize,
    finished: Rc<Cell<bool>>,
}

impl WindowTest {
    fn capture(&mut self, ctx: &egui::Context, name: &'static str) {
        self.pending = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(name)));
    }

    fn save(directory: &Path, name: &str, image: &egui::ColorImage) {
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(directory.join(format!("{name}.ppm"))).unwrap(),
        );
        write!(file, "P6\n{} {}\n255\n", image.width(), image.height()).unwrap();
        for pixel in &image.pixels {
            file.write_all(&[pixel.r(), pixel.g(), pixel.b()]).unwrap();
        }
    }
}

impl App for WindowTest {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        for event in &raw.events {
            if let egui::Event::Screenshot {
                user_data, image, ..
            } = event
                && let Some(name) = user_data
                    .data
                    .as_ref()
                    .and_then(|v| v.downcast_ref::<&str>())
            {
                if let Some(dir) = &self.output {
                    Self::save(dir, name, image);
                }
                self.step += 1;
                self.pending = false;
                self.next = Instant::now() + Duration::from_millis(400);
                match *name {
                    "games" => self.app.help_open = true,
                    "about" => {
                        self.app.help_open = false;
                        self.app.open_settings();
                    }
                    "settings" => self.app.settings = None,
                    _ => {}
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
        assert!(!self.app.failed, "{}", self.app.status);
        assert!(
            Instant::now() < self.deadline,
            "tempo limite: {}",
            self.app.status
        );
        if self.app.closing {
            if self.app.task.is_some() {
                self.closing_frames += 1;
                if self.step == 4 && !self.pending {
                    self.capture(ctx, "shutdown");
                }
            } else {
                assert!(self.closing_frames > 1 && self.step == 5);
                self.finished.set(true);
            }
        } else if self.app.home_ready && !self.pending && Instant::now() >= self.next {
            match self.step {
                0 => {
                    self.step = 1;
                    self.next = Instant::now() + Duration::from_secs(2);
                }
                1 => self.capture(ctx, "games"),
                2 => self.capture(ctx, "about"),
                3 => self.capture(ctx, "settings"),
                4 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                _ => unreachable!(),
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

#[test]
#[ignore = "opens the selected idle Android profile and closes its native window"]
fn real_window_paints_until_android_exits() {
    let state =
        PathBuf::from(std::env::var_os("RUSTEMUDROID_TEST_STATE").expect("select an idle profile"));
    let prefs = state.join("launcher.json");
    let config = LaunchConfig::load(None, state, None, &prefs);
    let output = std::env::var_os("RUSTEMUDROID_TEST_CAPTURES").map(PathBuf::from);
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
    }
    let done = Rc::new(Cell::new(false));
    let finished = done.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080., 680.])
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|b| {
            b.with_any_thread(true);
        })),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "RustEmuDroid · window test",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let icon =
                eframe::icon_data::from_png_bytes(include_bytes!("../../assets/rustemudroid.png"))?;
            let texture = cc.egui_ctx.load_texture(
                "brand",
                egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                ),
                egui::TextureOptions::LINEAR,
            );
            let now = Instant::now();
            Ok(Box::new(WindowTest {
                app: Launcher::new(texture, config, prefs, true),
                output,
                step: 0,
                pending: false,
                next: now,
                deadline: now + Duration::from_secs(240),
                closing_frames: 0,
                finished,
            }))
        }),
    )
    .unwrap();
    assert!(
        done.get(),
        "the native window closed before the VM had stopped"
    );
}
