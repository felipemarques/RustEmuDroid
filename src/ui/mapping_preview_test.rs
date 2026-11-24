use super::*;
use crate::capture::Shot;

const PACKAGE: &str = "org.supertuxkart.stk";
const SCENES: [&str; 6] = ["games", "controls", "editor", "replay", "idle", "shutdown"];

struct MappingPreview {
    app: Launcher,
    output: PathBuf,
    fixture: tempfile::TempDir,
    step: usize,
    pending: bool,
    next: Instant,
    deadline: Instant,
    shutdown: Option<std::sync::mpsc::Sender<()>>,
}

impl Drop for MappingPreview {
    fn drop(&mut self) {
        // Launcher joins its worker when the native window is destroyed.
        drop(self.shutdown.take());
    }
}

impl MappingPreview {
    fn prepare(&mut self) {
        let scene = SCENES[self.step % SCENES.len()];
        let locale = Locale::ALL[self.step / SCENES.len()];
        self.app.config.locale = locale;
        if let Some(session) = self.app.task.take() {
            drop(self.shutdown.take());
            session.worker.join().unwrap().unwrap();
        }
        self.app.closing = scene == "shutdown";
        self.app.playing = matches!(scene, "games" | "controls" | "editor" | "replay");
        self.app.home_ready = self.app.playing;
        self.app.stage = if self.app.playing {
            BootStage::Booted
        } else {
            BootStage::Stopped
        };
        self.app.controls = None;
        self.app.display = if self.app.playing {
            let filename = if scene == "games" {
                "home-frame.png"
            } else {
                "game-frame.png"
            };
            let frame = self.output.join(locale.tag()).join(filename);
            let state = self.fixture.path();
            let mut display =
                Display::for_mapping_capture(read_frame(&frame), state.join("touch-maps.json"));
            let package = if scene == "games" {
                "org.rustemudroid.home"
            } else {
                PACKAGE
            };
            self.app.controls = Some(Controls::for_screenshot(package, state));
            display.set_package(Some(package));
            match scene {
                "games" | "replay" => {}
                "controls" => {
                    display.toggle_controls_menu(140.);
                }
                "editor" => {
                    display.toggle_mapping();
                }
                _ => unreachable!(),
            }
            Some(display)
        } else {
            None
        };
        if self.app.closing {
            let (finish, wait) = std::sync::mpsc::channel();
            let (_, events) = std::sync::mpsc::channel();
            self.shutdown = Some(finish);
            self.app.task = Some(Session {
                events,
                worker: thread::spawn(move || {
                    let _ = wait.recv();
                    Ok(())
                }),
            });
        }
        self.pending = false;
        self.next = Instant::now() + Duration::from_millis(550);
    }
}

impl App for MappingPreview {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        raw.focused = true;
        raw.events
            .retain(|event| matches!(event, egui::Event::Screenshot { .. }));
        for event in &raw.events {
            if let egui::Event::Screenshot { image, .. } = event {
                let dir = self.output.join(self.app.config.locale.tag());
                let name = SCENES[self.step % SCENES.len()];
                WindowTest::save(&dir, name, image);
                self.step += 1;
                if self.step < Locale::ALL.len() * SCENES.len() {
                    self.prepare();
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        assert!(
            Instant::now() < self.deadline,
            "timed out capturing controls"
        );
        if self.step == Locale::ALL.len() * SCENES.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if SCENES[self.step % SCENES.len()] == "replay" {
            egui::Popup::open_id(ctx, capture_panel::replay_popup_id());
        } else {
            egui::Popup::close_id(ctx, capture_panel::replay_popup_id());
        }
        self.app.update(ctx, frame);
        assert_eq!(self.app.task.is_some(), self.app.closing);
        if !self.pending && Instant::now() >= self.next {
            self.pending = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

fn read_frame(path: &Path) -> Shot {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().unwrap();
    let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut bytes).unwrap();
    bytes.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => bytes,
        png::ColorType::Rgb => bytes
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        other => panic!("expected RGB or RGBA PNG, got {other:?}"),
    };
    Shot {
        width: info.width,
        height: info.height,
        pixels: pixels.into(),
    }
}

#[test]
#[ignore = "renders the session and its tools on captured guest frames without starting Android"]
fn capture_localized_session_frames() {
    let output = PathBuf::from(
        std::env::var_os("RUSTEMUDROID_TEST_CAPTURES").expect("choose a capture directory"),
    );
    let fixture = tempfile::tempdir().unwrap();
    write_json(
        &fixture.path().join("touch-maps.json"),
        &serde_json::json!({
            PACKAGE: {"active": "SuperTuxKart", "layouts": {"SuperTuxKart": {
                "buttons": {"Space": [0.56, 0.78]}, "stick": null, "controls": [{
                    "input": "", "at": [0.22, 0.76], "gesture": {
                        "kind": "dpad", "directions": ["W", "S", "A", "D"],
                        "radius": 0.13, "left_stick": true
                    }
                }]
            }}}
        }),
    )
    .unwrap();
    write_json(
        &fixture.path().join("control-overlay.json"),
        &serde_json::json!({"show_hints": true}),
    )
    .unwrap();
    let state = fixture.path().to_path_buf();
    let prefs = state.join("launcher.json");
    let mut config = LaunchConfig::load(None, state, None, &prefs);
    config.show_fps = false;
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |cc| {
            let app = Launcher::new(install_window(cc, brand), config, prefs, false);
            let now = Instant::now();
            let mut preview = MappingPreview {
                app,
                output,
                fixture,
                step: 0,
                pending: false,
                next: now,
                deadline: now + Duration::from_secs(25),
                shutdown: None,
            };
            preview.prepare();
            Ok(Box::new(preview))
        }),
    )
    .unwrap();
}
