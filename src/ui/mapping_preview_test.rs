use super::*;
use crate::capture::Shot;

const PACKAGE: &str = "org.supertuxkart.stk";

struct MappingPreview {
    app: Launcher,
    output: PathBuf,
    fixture: tempfile::TempDir,
    step: usize,
    select_pad: std::ops::Range<usize>,
    pending: bool,
    next: Instant,
    deadline: Instant,
}

impl MappingPreview {
    fn prepare(&mut self) {
        let locale = Locale::ALL[self.step / 2];
        self.app.config.locale = locale;
        let frame = self.output.join(locale.tag()).join("game-frame.png");
        let mut display = Display::for_mapping_capture(
            read_frame(&frame),
            self.fixture.path().join("touch-maps.json"),
        );
        display.set_package(Some(PACKAGE));
        if self.step % 2 == 0 {
            display.toggle_controls_menu(140.);
        } else {
            display.toggle_mapping();
            self.select_pad = 0..4;
        }
        self.app.display = Some(display);
        self.pending = false;
        self.next = Instant::now() + Duration::from_millis(550);
    }
}

impl App for MappingPreview {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        raw.focused = true;
        raw.events
            .retain(|event| !matches!(event, egui::Event::WindowFocused(false)));
        let pad = egui::pos2(227., 510.);
        match self.select_pad.next() {
            Some(0) => raw.events.push(egui::Event::PointerMoved(pad)),
            Some(step @ (1 | 2)) => raw.events.push(egui::Event::PointerButton {
                pos: pad,
                button: egui::PointerButton::Primary,
                pressed: step == 1,
                modifiers: egui::Modifiers::NONE,
            }),
            Some(_) => raw.events.push(egui::Event::PointerGone),
            None => {}
        }
        for event in &raw.events {
            if let egui::Event::Screenshot { image, .. } = event {
                let dir = self.output.join(self.app.config.locale.tag());
                let name = if self.step % 2 == 0 {
                    "controls"
                } else {
                    "editor"
                };
                WindowTest::save(&dir, name, image);
                self.step += 1;
                if self.step < Locale::ALL.len() * 2 {
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
        if self.step == Locale::ALL.len() * 2 {
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
#[ignore = "renders mapping UI on captured guest frames without starting Android"]
fn capture_localized_control_panels() {
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
    let config = LaunchConfig::load(None, state, None, &prefs);
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |cc| {
            let mut app = Launcher::new(install_window(cc, brand), config, prefs, false);
            app.playing = true;
            app.home_ready = true;
            app.stage = BootStage::Booted;
            let now = Instant::now();
            let mut preview = MappingPreview {
                app,
                output,
                fixture,
                step: 0,
                select_pad: 0..0,
                pending: false,
                next: now,
                deadline: now + Duration::from_secs(25),
            };
            preview.prepare();
            Ok(Box::new(preview))
        }),
    )
    .unwrap();
}
