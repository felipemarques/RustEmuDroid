//! Opt-in launcher test against an already running, operator-owned Pie64 lab.
use super::{MirrorSession, egui};
use crate::locale::Locale;
use crate::ui::{LaunchConfig, Launcher, branded_window, install_window};
use eframe::App;
use egui::{Event, Key, Modifiers, PointerButton, RawInput};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    env, fs,
    io::Write,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use winit::platform::windows::EventLoopBuilderExtWindows;

struct Fixture {
    app: Launcher,
    directory: PathBuf,
    receipt: fs::File,
    pid: u32,
    finished: Rc<Cell<bool>>,
    deadline: Instant,
    last: Value,
    focus: bool,
    command: Option<String>,
    screenshot: bool,
    closing: bool,
    fitted: Option<super::PreviewRect>,
}

fn key(raw: &mut RawInput, key: Key, pressed: bool, modifiers: Modifiers) {
    raw.events.push(Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    });
}

fn click(raw: &mut RawInput, point: egui::Pos2) {
    raw.events.push(Event::PointerMoved(point));
    for pressed in [true, false] {
        raw.events.push(Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

impl Fixture {
    fn preview_position(&self, ctx: &egui::Context, x: f32, y: f32) -> egui::Pos2 {
        let rect = self.fitted.expect("the playing viewport was fitted");
        egui::pos2(
            (rect.left as f32 + (rect.right - rect.left) as f32 * x) / ctx.pixels_per_point(),
            (rect.top as f32 + (rect.bottom - rect.top) as f32 * y) / ctx.pixels_per_point(),
        )
    }

    fn record(&mut self, kind: &str, value: Value) {
        writeln!(self.receipt, "{}", json!({"event": kind, "details": value})).unwrap();
        self.receipt.flush().unwrap();
    }
}

impl App for Fixture {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        if self.screenshot
            && let Some(Event::Screenshot { image, .. }) = raw
                .events
                .iter()
                .find(|event| matches!(event, Event::Screenshot { .. }))
        {
            let (editing, _, _, _) = self.app.blue_mirror.controls_test_state();
            let scene = if editing { "editor" } else { "quick" };
            let capture = format!("{scene}-{}", self.app.config.locale.tag());
            crate::ui::live_window_test::WindowTest::save(&self.directory, &capture, image);
            self.screenshot = false;
            self.record("screenshot", json!(capture));
        }
        raw.events.retain(|event| {
            !matches!(
                event,
                Event::Key { .. }
                    | Event::WindowFocused(_)
                    | Event::PointerMoved(_)
                    | Event::PointerButton { .. }
                    | Event::MouseWheel { .. }
                    | Event::PointerGone
            )
        });
        raw.focused = self.focus;
        raw.modifiers = Modifiers::NONE;
        if let Some(root) = raw.viewports.get_mut(&raw.viewport_id) {
            root.focused = Some(self.focus);
        }
        let Some(command) = self.command.take() else {
            return;
        };
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            command: true,
            ..Modifiers::NONE
        };
        match command.as_str() {
            "editor" | "profile" => {
                let shortcut = if command == "editor" { Key::A } else { Key::P };
                raw.modifiers = modifiers;
                key(raw, shortcut, true, modifiers);
                key(raw, shortcut, false, modifiers);
            }
            "escape" => {
                key(raw, Key::Escape, true, Modifiers::NONE);
                key(raw, Key::Escape, false, Modifiers::NONE);
            }
            "quick" => {
                let point = self
                    .app
                    .blue_mirror
                    .controls_test_button()
                    .expect("Keymap button");
                click(raw, point);
            }
            "focus-preview" => {
                click(raw, self.preview_position(ctx, 0.51, 0.18));
            }
            "wheel-focus" => {
                click(raw, self.preview_position(ctx, 0.36, 0.44));
            }
            "wheel-hover" => {
                raw.events
                    .push(Event::PointerMoved(self.preview_position(ctx, 0.36, 0.44)));
                raw.events.push(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, 1.0),
                    modifiers: Modifiers::NONE,
                });
            }
            "start-drag" | "drag" | "stop-drag" => {
                let start = command == "start-drag";
                let point = self.preview_position(ctx, if start { 0.23 } else { 0.30 }, 0.77);
                raw.events.push(Event::PointerMoved(point));
                if command != "drag" {
                    raw.events.push(Event::PointerButton {
                        pos: point,
                        button: PointerButton::Primary,
                        pressed: start,
                        modifiers: Modifiers::NONE,
                    });
                }
            }
            "save" => {
                let panel = self
                    .app
                    .blue_mirror
                    .mapped
                    .map
                    .blocked_rect
                    .expect("Editor panel");
                click(
                    raw,
                    egui::pos2(panel.right() - panel.width() * 0.24, panel.bottom() - 30.),
                );
            }
            "hold" | "release" => {
                for input in [Key::A, Key::Space] {
                    key(raw, input, command == "hold", Modifiers::NONE);
                }
            }
            "blur" | "focus" => {
                self.focus = command == "focus";
                raw.focused = self.focus;
                if let Some(root) = raw.viewports.get_mut(&raw.viewport_id) {
                    root.focused = Some(self.focus);
                }
                raw.events.push(Event::WindowFocused(self.focus));
            }
            _ => unreachable!(),
        }
        self.record("posted", json!(command));
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let request = self.directory.join("request");
        if let Ok(command) = fs::read_to_string(&request) {
            fs::remove_file(request).unwrap();
            let command = command.trim();
            self.deadline = Instant::now() + Duration::from_secs(90);
            match command {
                "close" => self.closing = true,
                "pt-br" | "en-us" | "es-es" => {
                    self.app.config.locale = Locale::ALL
                        .into_iter()
                        .find(|locale| locale.tag() == command)
                        .unwrap();
                    self.screenshot = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                }
                "editor" | "profile" | "escape" | "quick" | "focus-preview" | "hold"
                | "release" | "blur" | "focus" | "start-drag" | "drag" | "stop-drag" | "save"
                | "wheel-focus" | "wheel-hover" => {
                    self.command = Some(command.to_owned());
                }
                _ => panic!("unknown fixture request"),
            }
        }
        if Instant::now() >= self.deadline && !self.closing {
            self.closing = true;
            self.record("timeout", Value::Null);
        }
        if self.closing {
            self.app.blue_mirror.disconnect();
            if !self.app.blue_mirror.poll_cleanup(ctx) {
                self.record("closed", Value::Null);
                self.finished.set(Instant::now() < self.deadline);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        self.app.update(ctx, frame);
        let panel = &self.app.blue_mirror;
        if let Some((_, rect, _)) = panel.layout {
            self.fitted = Some(rect);
        }
        if let Some((pid, _)) = panel.player {
            assert_eq!(pid, self.pid);
        }
        let (editor, image, contacts, foreground) = panel.controls_test_state();
        let state = json!({
            "playing": panel.player.is_some() && panel.preview.is_some(),
            "rect": panel.layout.map(|(_, r, _)| [r.left, r.top, r.right, r.bottom]),
            "size": panel.size,
            "error": panel.input_error,
            "focused": panel.input_focused,
            "editor_open": editor,
            "controls_open": panel.controls_open(),
            "editor_image": image,
            "contacts": contacts,
            "foreground": foreground,
        });
        if state != self.last {
            self.record("state", state.clone());
            self.last = state;
        }
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

#[test]
#[ignore = "requires owner-authorized Pie64_1, ADB and a real Windows launcher"]
fn live_mapping_routes_input_through_the_launcher() {
    let directory =
        fs::canonicalize(env::var_os("RUSTEMUDROID_MAPPING_LIVE_DIR").unwrap()).unwrap();
    let repo = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert!(directory.starts_with(repo.join(".rustemudroid/qa")));
    let instance = "Pie64_1".to_owned();
    let pid = env::var("RUSTEMUDROID_MIRROR_LIVE_PID")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert_ne!(pid, 0);
    let adb = PathBuf::from(env::var_os("RUSTEMUDROID_MIRROR_LIVE_ADB").unwrap());
    assert!(adb.is_absolute() && adb.is_file());
    assert!(!directory.join("request").exists());
    let receipt = fs::File::create_new(directory.join("mapping-events.jsonl")).unwrap();
    let state = directory.join("profile");
    let prefs = state.join("launcher.json");
    let maps = crate::bluestacks::mapping_file(&state, &instance);
    assert!(maps.as_deref().is_some_and(std::path::Path::is_file));
    let config = LaunchConfig::load(None, state, None, &prefs);
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    let finished = Rc::new(Cell::new(false));
    let report = finished.clone();
    eframe::run_native(
        "RustEmuDroid BlueStacks mapping fixture",
        options,
        Box::new(move |creation| {
            let mut app = Launcher::new(install_window(creation, brand), config, prefs, false);
            app.blue_mirror.instance = instance.clone();
            app.blue_mirror.mapped.load(maps);
            app.blue_mirror.session =
                Some(MirrorSession::start_existing_for_test(instance, pid, adb));
            Ok(Box::new(Fixture {
                app,
                directory,
                receipt,
                pid,
                finished: report,
                deadline: Instant::now() + Duration::from_secs(90),
                last: Value::Null,
                focus: true,
                command: None,
                screenshot: false,
                closing: false,
                fitted: None,
            }))
        }),
    )
    .unwrap();
    assert!(finished.get(), "mapping fixture timed out");
}
