//! Opt-in Windows integration test. Select an idle AVD explicitly.
use super::*;
use eframe::App;
use std::{cell::Cell, io::Write, path::Path, rc::Rc, time::Instant};
use winit::platform::windows::EventLoopBuilderExtWindows;

#[path = "mapping_preview_test.rs"]
mod mapping_preview_test;
#[path = "rolling_replay_test.rs"]
mod rolling_replay_test;

#[test]
#[ignore = "creates a native Windows window and checks the caption and taskbar HICONs"]
fn native_window_uses_the_embedded_brand() {
    struct CloseOnPaint;
    impl App for CloseOnPaint {
        fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |cc| {
            let _texture = install_window(cc, brand);
            assert!(window_icon::matches_brand(cc));
            Ok(Box::new(CloseOnPaint))
        }),
    )
    .unwrap();
}

pub(super) struct WindowTest {
    app: Launcher,
    output: Option<PathBuf>,
    step: usize,
    pending: bool,
    next: Instant,
    deadline: Instant,
    closing_frames: usize,
    game: Option<String>,
    demo: usize,
    finished: Rc<Cell<bool>>,
}

#[test]
#[ignore = "renders the localized help tabs and Settings dialog without starting Android"]
fn capture_localized_session_dialogs() {
    struct Dialogs {
        app: Launcher,
        dir: PathBuf,
        language: usize,
        pending: bool,
        next: Instant,
    }
    impl App for Dialogs {
        fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
            for event in &raw.events {
                if let egui::Event::Screenshot { image, .. } = event {
                    let dir = self.dir.join(self.app.config.locale.tag());
                    std::fs::create_dir_all(&dir).unwrap();
                    if self.app.settings.is_some() {
                        WindowTest::save(&dir, "settings", image);
                        self.app.settings = None;
                        self.language += 1;
                        if let Some(locale) = Locale::ALL.get(self.language) {
                            self.app.config.locale = *locale;
                            self.app.help_tab = help::HelpTab::About;
                            self.app.help_open = true;
                        }
                    } else if self.app.help_tab == help::HelpTab::About {
                        WindowTest::save(&dir, "about", image);
                        self.app.help_tab = help::HelpTab::Shortcuts;
                    } else {
                        WindowTest::save(&dir, "shortcuts", image);
                        self.app.help_open = false;
                        self.app.open_settings();
                    }
                    self.pending = false;
                    self.next = Instant::now() + Duration::from_millis(850);
                }
            }
        }
        fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            if self.language == Locale::ALL.len() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            self.app.update(ctx, frame);
            assert!(self.app.task.is_none());
            if !self.pending && Instant::now() >= self.next {
                self.pending = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
    let state = PathBuf::from(
        std::env::var_os("RUSTEMUDROID_TEST_STATE").expect("select a launcher profile"),
    );
    let dir = PathBuf::from(
        std::env::var_os("RUSTEMUDROID_TEST_CAPTURES").expect("choose a capture directory"),
    );
    let prefs = state.join("launcher.json");
    let mut config = LaunchConfig::load(None, state, None, &prefs);
    config.locale = Locale::ALL[0];
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |cc| {
            let mut app = Launcher::new(install_window(cc, brand), config, prefs, false);
            app.help_open = true;
            Ok(Box::new(Dialogs {
                app,
                dir,
                language: 0,
                pending: false,
                next: Instant::now() + Duration::from_millis(850),
            }))
        }),
    )
    .unwrap();
}

impl WindowTest {
    fn exercise_game_tools(&mut self, ctx: &egui::Context) {
        match self.demo {
            0 => {
                let package = self.game.as_deref().unwrap();
                assert!(
                    package
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
                );
                let result = self
                    .app
                    .controls
                    .as_ref()
                    .unwrap()
                    .guest_output(&[
                        "shell",
                        "monkey",
                        "-p",
                        package,
                        "-c",
                        "android.intent.category.LAUNCHER",
                        "1",
                    ])
                    .unwrap();
                assert!(result.contains("Events injected: 1"), "{result}");
                self.next = Instant::now() + Duration::from_secs(10);
            }
            1 => self.app.capture_action(Tool::Screenshot),
            2..=6 if self.app.capture.busy() && self.demo != 3 => return,
            2 => {
                self.app.capture_action(Tool::RecordVideo);
                self.next = Instant::now() + Duration::from_secs(3);
            }
            3 => self.app.capture_action(Tool::StopRecording),
            4 => self.app.capture_action(Tool::AnimatedClip),
            5 => self.app.capture_action(Tool::AnimatedGif),
            6 => {
                for ext in ["png", "mp4", "jxl", "gif"] {
                    assert!(
                        std::fs::read_dir(self.app.capture_dir())
                            .unwrap()
                            .flatten()
                            .any(|file| file.path().extension().is_some_and(|e| e == ext)),
                        "missing {ext}"
                    );
                }
                self.app.display.as_mut().unwrap().toggle_mapping();
                self.next = Instant::now() + Duration::from_secs(3);
            }
            7 => self.capture(ctx, "editor"),
            _ => unreachable!(),
        }
        self.demo += 1;
    }

    fn capture(&mut self, ctx: &egui::Context, name: &'static str) {
        self.pending = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(name)));
    }

    pub(super) fn save(directory: &Path, name: &str, image: &egui::ColorImage) {
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
                if *name == "editor" {
                    self.app.display.as_mut().unwrap().toggle_mapping();
                    self.game = None;
                    self.pending = false;
                    continue;
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
                    "idle" => self.app.start(false),
                    _ => {}
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
        assert!(!self.app.failed, "{}", self.app.status);
        if !self.app.home_ready
            && !self.app.closing
            && let Some(Err(error)) = self.app.controls.as_ref().and_then(|c| c.message.as_ref())
        {
            panic!("Android Home could not be prepared: {error}");
        }
        assert!(
            Instant::now() < self.deadline,
            "tempo limite: {}",
            self.app.status
        );
        if self.app.closing {
            if self.app.task.is_some() {
                self.closing_frames += 1;
                if self.step == 6 && !self.pending {
                    self.capture(ctx, "shutdown");
                }
            } else {
                assert!(self.closing_frames > 1 && self.step == 7);
                self.finished.set(true);
            }
        } else if self.step == 5 && self.app.task.is_none() && !self.pending {
            self.capture(ctx, "idle");
        } else if self.app.home_ready && !self.pending && Instant::now() >= self.next {
            match self.step {
                0 => {
                    self.step = 1;
                    self.next = Instant::now() + Duration::from_secs(2);
                }
                1 => self.capture(ctx, "games"),
                2 => self.capture(ctx, "about"),
                3 => self.capture(ctx, "settings"),
                4 if self.game.is_some() => self.exercise_game_tools(ctx),
                4 => {
                    self.app.stop.store(true, Ordering::Release);
                    self.step = 5;
                }
                5 => {}
                6 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
    let mut config = LaunchConfig::load(None, state, None, &prefs);
    if let Ok(tag) = std::env::var("RUSTEMUDROID_TEST_LOCALE") {
        config.locale = serde_json::from_value(serde_json::Value::String(tag))
            .expect("expected pt-br, en-us or es-es");
    }
    let output = std::env::var_os("RUSTEMUDROID_TEST_CAPTURES").map(PathBuf::from);
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
    }
    let game = std::env::var("RUSTEMUDROID_TEST_GAME").ok();
    let prefs = if game.is_some() {
        output
            .as_ref()
            .expect("choose a fresh capture directory for the game test")
            .join("launcher.json")
    } else {
        prefs
    };
    let done = Rc::new(Cell::new(false));
    let finished = done.clone();
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid · window test",
        options,
        Box::new(move |cc| {
            let texture = install_window(cc, brand);
            assert!(
                window_icon::matches_brand(cc),
                "Windows must receive the brand for both window icon sizes"
            );
            let now = Instant::now();
            Ok(Box::new(WindowTest {
                app: Launcher::new(texture, config, prefs, true),
                output,
                step: 0,
                pending: false,
                next: now,
                deadline: now + Duration::from_secs(420),
                closing_frames: 0,
                game,
                demo: 0,
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
