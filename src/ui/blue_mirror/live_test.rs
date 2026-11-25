//! Opt-in test of the real native launcher, DWM layout and egui input path.
use super::{MirrorPanel, MirrorSession, PreviewRect, egui};
use crate::ui::{LaunchConfig, Launcher, branded_window, install_window};
use eframe::App;
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, ViewportEvent};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use winit::platform::windows::EventLoopBuilderExtWindows;

const OPERATOR_WAIT: Duration = Duration::from_secs(90);
const ROTATION_WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    WaitingForMirror,
    TouchGate,
    RotationPending { expected: [u32; 2], reported: bool },
    CancelDownPosted,
    CancelHidden,
    CancelRestoring,
    CancelUp,
    CancelUpPosted,
    TouchPosted,
    BackGate,
    BackPosted,
    SecondBackOrClose,
    SecondBackPosted,
    CloseGate,
    RequestClose,
    Closing,
    Finished,
}

#[derive(Clone, Copy)]
struct Placement {
    fitted: PreviewRect,
    size: [u32; 2],
    position: Pos2,
    pixels_per_point: f32,
}

impl Placement {
    fn evidence(self, guest_point: [u32; 2]) -> Value {
        json!({
            "guest_point": guest_point,
            "guest_size": self.size,
            "dwm_fitted_rect": [self.fitted.left, self.fitted.top, self.fitted.right, self.fitted.bottom],
            "egui_point": [self.position.x, self.position.y],
            "pixels_per_point": self.pixels_per_point,
        })
    }
}

fn placement(
    panel: &MirrorPanel,
    point: [u32; 2],
    ppp: f32,
    expected_pid: u32,
) -> Option<Placement> {
    let (pid, _) = panel.player?;
    panel.preview.as_ref()?;
    let (_, fitted, _) = panel.layout?;
    let size = panel.size?;
    if pid != expected_pid {
        return None;
    }
    map_placement(fitted, size, point, ppp)
}

fn map_placement(
    fitted: PreviewRect,
    size: [u32; 2],
    point: [u32; 2],
    ppp: f32,
) -> Option<Placement> {
    if point[0] >= size[0] || point[1] >= size[1] || ppp <= 0.0 || !ppp.is_finite() {
        return None;
    }
    let width = f64::from(fitted.right) - f64::from(fitted.left);
    let height = f64::from(fitted.bottom) - f64::from(fitted.top);
    let aspect = (width * f64::from(size[1])) / (height * f64::from(size[0]));
    if !(0.97..=1.03).contains(&aspect) {
        return None;
    }
    let x = f64::from(fitted.left) + (f64::from(point[0]) + 0.5) * width / f64::from(size[0]);
    let y = f64::from(fitted.top) + (f64::from(point[1]) + 0.5) * height / f64::from(size[1]);
    let position = Pos2::new((x / f64::from(ppp)) as f32, (y / f64::from(ppp)) as f32);
    if fitted.guest_point([position.x, position.y], ppp, size) != Some(point) {
        return None;
    }
    Some(Placement {
        fitted,
        size,
        position,
        pixels_per_point: ppp,
    })
}

#[test]
fn rotated_layout_uses_a_point_from_the_new_display() {
    let portrait = PreviewRect {
        left: 140,
        top: 72,
        right: 410,
        bottom: 552,
    };
    let size = [900, 1600];
    assert!(map_placement(portrait, size, [1200, 240], 1.25).is_none());
    let [width, height] = size;
    let center = [width / 2, height / 2];
    assert!(map_placement(portrait, size, center, 1.25).is_some());
}

fn marker(receipt: &Path, suffix: &str) -> PathBuf {
    let mut path = receipt.as_os_str().to_owned();
    path.push(suffix);
    path.into()
}

fn append(file: &mut File, started: Instant, event: &str, details: Value) {
    serde_json::to_writer(
        &mut *file,
        &json!({
            "event": event,
            "elapsed_ms": started.elapsed().as_millis(),
            "details": details,
        }),
    )
    .expect("write private mirror receipt");
    file.write_all(b"\n").expect("end receipt event");
    file.flush().expect("flush receipt event for the operator");
    eprintln!("BlueStacks live test: {event}");
}

struct MirrorWindowTest {
    app: Launcher,
    guest_point: [u32; 2],
    expected_pid: u32,
    path: PathBuf,
    file: File,
    phase: Phase,
    started: Instant,
    deadline: Instant,
    posted: Option<Placement>,
    tested_cancel: bool,
    rotations_checked: u8,
    failure: Option<String>,
    finished: Rc<Cell<bool>>,
}

impl MirrorWindowTest {
    fn write(&mut self, event: &str, details: Value) {
        append(&mut self.file, self.started, event, details);
    }

    fn advance(&mut self, phase: Phase) {
        self.phase = phase;
        self.deadline = Instant::now() + OPERATOR_WAIT;
    }

    fn fail(&mut self, reason: String) {
        self.write(
            "failed",
            json!({"phase": format!("{:?}", self.phase), "reason": reason, "mirror": self.geometry()}),
        );
        self.failure = Some(reason);
        self.advance(Phase::RequestClose);
    }

    fn layout(&self, ctx: &egui::Context) -> Option<Placement> {
        placement(
            &self.app.blue_mirror,
            self.guest_point,
            ctx.pixels_per_point(),
            self.expected_pid,
        )
    }

    fn rotation_layout(&self, ctx: &egui::Context) -> Option<(Placement, [u32; 2])> {
        let panel = &self.app.blue_mirror;
        let [width, height] = panel.size?;
        let center = [width / 2, height / 2];
        placement(panel, center, ctx.pixels_per_point(), self.expected_pid)
            .map(|layout| (layout, center))
    }

    fn geometry(&self) -> Value {
        let panel = &self.app.blue_mirror;
        json!({
            "guest_size": panel.size,
            "rotation_pending": panel.rotation_pending,
            "dwm_fitted_rect": panel.layout.map(|(_, rect, _)| [rect.left, rect.top, rect.right, rect.bottom]),
            "preview_error": panel.view_error,
            "input_error": panel.input_error,
        })
    }

    fn post(
        &mut self,
        ctx: &egui::Context,
        raw: &mut RawInput,
        button: PointerButton,
        presses: &[bool],
        phase: Phase,
    ) {
        let Some(layout) = self.layout(ctx) else {
            self.fail(
                "The confirmed DWM layout or guest dimensions changed before input".to_owned(),
            );
            return;
        };
        focus_fixture_input(raw);
        raw.events.push(Event::PointerMoved(layout.position));
        for &pressed in presses {
            raw.events.push(Event::PointerButton {
                pos: layout.position,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        self.posted = Some(layout);
        self.advance(phase);
    }

    fn confirm_posted(&mut self, ctx: &egui::Context, event: &str, next: Phase) {
        let Some(layout) = self.posted.take() else {
            self.fail("No input was inserted into RawInput".to_owned());
            return;
        };
        if (event == "touch_sent" && !self.app.blue_mirror.input_focused)
            || self.app.blue_mirror.last_touch.is_some()
            || self.layout(ctx).is_none_or(|current| {
                current.fitted != layout.fitted || current.size != layout.size
            })
        {
            self.fail(
                "The live frame did not retain the expected preview geometry and input focus"
                    .to_owned(),
            );
            return;
        }
        self.write(event, layout.evidence(self.guest_point));
        self.advance(next);
    }
}

fn focus_fixture_input(raw: &mut RawInput) {
    raw.focused = true;
    raw.modifiers = Modifiers::NONE;
    raw.viewports.entry(raw.viewport_id).or_default().focused = Some(true);
}

impl App for MirrorWindowTest {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        // This fixture drives only its own egui RawInput. It never sends native OS input.
        raw.events.clear();
        if !matches!(
            self.phase,
            Phase::RequestClose | Phase::Closing | Phase::Finished
        ) && marker(&self.path, ".close").is_file()
        {
            if matches!(self.phase, Phase::SecondBackOrClose | Phase::CloseGate) {
                self.advance(Phase::RequestClose);
            } else {
                self.fail(
                    "Operator requested shutdown before the input proof completed".to_owned(),
                );
            }
        }
        match self.phase {
            Phase::TouchGate if !self.tested_cancel && marker(&self.path, ".cancel").is_file() => {
                self.post(
                    ctx,
                    raw,
                    PointerButton::Primary,
                    &[true],
                    Phase::CancelDownPosted,
                );
            }
            Phase::CancelHidden | Phase::CancelRestoring => focus_fixture_input(raw),
            Phase::CancelUp => {
                self.post(
                    ctx,
                    raw,
                    PointerButton::Primary,
                    &[false],
                    Phase::CancelUpPosted,
                );
            }
            Phase::TouchGate if marker(&self.path, ".touch").is_file() => {
                self.post(
                    ctx,
                    raw,
                    PointerButton::Primary,
                    &[true, false],
                    Phase::TouchPosted,
                );
            }
            Phase::BackGate if marker(&self.path, ".back").is_file() => {
                self.post(
                    ctx,
                    raw,
                    PointerButton::Secondary,
                    &[true, false],
                    Phase::BackPosted,
                );
            }
            Phase::SecondBackOrClose if marker(&self.path, ".back2").is_file() => {
                self.post(
                    ctx,
                    raw,
                    PointerButton::Secondary,
                    &[true, false],
                    Phase::SecondBackPosted,
                );
            }
            _ => {}
        }
        if self.phase == Phase::RequestClose {
            raw.viewports
                .entry(raw.viewport_id)
                .or_default()
                .events
                .push(ViewportEvent::Close);
            self.advance(Phase::Closing);
            self.write("closing", json!({"via": "Launcher viewport close"}));
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
        assert!(
            self.app.task.is_none(),
            "the native Android runtime must stay stopped in this fixture"
        );

        if !matches!(
            self.phase,
            Phase::RequestClose | Phase::Closing | Phase::Finished
        ) {
            if let Some(error) = self.app.blue_mirror.input_error.clone() {
                self.fail(error);
            } else if Instant::now() >= self.deadline {
                let why = self.app.blue_mirror.view_error.clone();
                self.fail(format!("Timed out in {:?}: {why:?}", self.phase));
            }
        }
        match self.phase {
            Phase::WaitingForMirror => {
                if let Some(layout) = self.layout(ctx) {
                    self.write("ready", layout.evidence(self.guest_point));
                    self.advance(Phase::TouchGate);
                }
            }
            Phase::TouchGate if self.rotations_checked < 2 => {
                let marker_name = if self.rotations_checked == 0 {
                    ".rotate"
                } else {
                    ".rotate2"
                };
                if marker(&self.path, marker_name).is_file()
                    && let Some((layout, center)) = self.rotation_layout(ctx)
                {
                    let [width, height] = layout.size;
                    if width == height {
                        self.fail(
                            "A square guest cannot prove the display axes were exchanged"
                                .to_owned(),
                        );
                    } else {
                        let expected = [height, width];
                        self.app.blue_mirror.request_rotation(true);
                        self.write(
                            "rotation_requested",
                            json!({"expected_size": expected, "before": layout.evidence(center)}),
                        );
                        self.phase = Phase::RotationPending {
                            expected,
                            reported: false,
                        };
                        self.deadline = Instant::now() + ROTATION_WAIT;
                    }
                }
            }
            Phase::RotationPending { expected, reported }
                if !self.app.blue_mirror.rotation_pending =>
            {
                if !reported {
                    self.write(
                        "rotation_size_reported",
                        json!({"expected_size": expected, "mirror": self.geometry()}),
                    );
                    self.phase = Phase::RotationPending {
                        expected,
                        reported: true,
                    };
                }
                if let Some((layout, center)) = self.rotation_layout(ctx)
                    && layout.size == expected
                {
                    self.rotations_checked += 1;
                    let event = if self.rotations_checked == 1 {
                        "rotated"
                    } else {
                        "rotation_restored"
                    };
                    self.write(event, layout.evidence(center));
                    self.advance(Phase::TouchGate);
                }
            }
            Phase::CancelDownPosted => {
                let posted = self.posted.take();
                if let Some(layout) = posted
                    && self.app.blue_mirror.last_touch == Some(self.guest_point)
                {
                    self.write("cancel_down_sent", layout.evidence(self.guest_point));
                    self.app.help_open = true;
                    self.advance(Phase::CancelHidden);
                } else {
                    self.fail(
                        "The fixture did not establish a primary touch before opening Help"
                            .to_owned(),
                    );
                }
            }
            Phase::CancelHidden => {
                let mirror = &self.app.blue_mirror;
                if self.app.help_open
                    && mirror.layout.is_none()
                    && mirror.last_touch.is_none()
                    && !mirror.input_focused
                    && ctx.input(|input| input.pointer.primary_down())
                {
                    self.write(
                        "cancel_hidden",
                        json!({"help_open": true, "primary_still_down": true}),
                    );
                    self.app.help_open = false;
                    self.advance(Phase::CancelRestoring);
                } else {
                    self.fail("Help did not cancel the guest contact while the fixture button remained down".to_owned());
                }
            }
            Phase::CancelRestoring if self.layout(ctx).is_some() => self.advance(Phase::CancelUp),
            Phase::CancelUpPosted => {
                let posted = self.posted.take();
                let mirror = &self.app.blue_mirror;
                if let Some(layout) = posted
                    && mirror.last_touch.is_none()
                    && !mirror.input_focused
                    && !ctx.input(|input| input.pointer.primary_down())
                {
                    self.write("cancel_up_ignored", layout.evidence(self.guest_point));
                    self.tested_cancel = true;
                    self.advance(Phase::TouchGate);
                } else {
                    self.fail(
                        "The previous button release became guest input after Help closed"
                            .to_owned(),
                    );
                }
            }
            Phase::TouchPosted => self.confirm_posted(ctx, "touch_sent", Phase::BackGate),
            Phase::BackPosted => self.confirm_posted(ctx, "back_sent", Phase::SecondBackOrClose),
            Phase::SecondBackPosted => self.confirm_posted(ctx, "back2_sent", Phase::CloseGate),
            Phase::Closing if self.app.closing && self.app.blue_mirror.retiring.is_empty() => {
                let success = self.failure.is_none();
                self.write("closed_clean", json!({"success": success}));
                self.finished.set(success);
                self.advance(Phase::Finished);
            }
            _ => {}
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "requires an operator-owned BlueStacks player, an authorized ADB connection and a real Windows window"]
fn live_player_routes_raw_input_through_the_launcher() {
    let instance = env::var("RUSTEMUDROID_MIRROR_LIVE_INSTANCE")
        .expect("select the running BlueStacks instance");
    let pid: u32 = env::var("RUSTEMUDROID_MIRROR_LIVE_PID")
        .expect("supply the operator-owned HD-Player PID")
        .parse()
        .expect("PID must be a positive integer");
    assert!(pid != 0);
    let adb =
        PathBuf::from(env::var_os("RUSTEMUDROID_MIRROR_LIVE_ADB").expect("choose the SDK adb.exe"));
    assert!(adb.is_absolute() && adb.is_file());
    let point = env::var("RUSTEMUDROID_MIRROR_LIVE_POINT")
        .expect("supply the approved Android guest point as x,y");
    let (x, y) = point.split_once(',').expect("point must contain a comma");
    let guest_point = [
        x.trim().parse().expect("invalid guest x"),
        y.trim().parse().expect("invalid guest y"),
    ];
    let given = PathBuf::from(
        env::var_os("RUSTEMUDROID_MIRROR_LIVE_RECEIPT").expect("choose a private JSONL path"),
    );
    assert!(given.is_absolute());
    let directory = fs::canonicalize(given.parent().expect("receipt needs a directory"))
        .expect("private directory must exist");
    let repo = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert!(
        !directory.starts_with(&repo) || directory.starts_with(repo.join(".rustemudroid/qa")),
        "choose a path outside the worktree or inside its private .rustemudroid/qa directory"
    );
    let path = directory.join(given.file_name().unwrap());
    for suffix in [
        ".rotate", ".rotate2", ".cancel", ".touch", ".back", ".back2", ".close",
    ] {
        assert!(
            !marker(&path, suffix).exists(),
            "test markers must begin absent"
        );
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .expect("create new private receipt");
    let started = Instant::now();
    append(
        &mut file,
        started,
        "starting",
        json!({"instance": instance, "pid": pid, "source_pixels_captured": false}),
    );

    let state = tempfile::tempdir_in(&directory).expect("temporary launcher state");
    let prefs = state.path().join("launcher.json");
    let config = LaunchConfig::load(None, state.path().to_owned(), None, &prefs);
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    let finished = Rc::new(Cell::new(false));
    let report = finished.clone();
    eframe::run_native(
        "RustEmuDroid BlueStacks live fixture",
        options,
        Box::new(move |creation| {
            let mut app = Launcher::new(install_window(creation, brand), config, prefs, false);
            app.blue_mirror.instance = instance.clone();
            app.blue_mirror.session =
                Some(MirrorSession::start_existing_for_test(instance, pid, adb));
            Ok(Box::new(MirrorWindowTest {
                app,
                guest_point,
                expected_pid: pid,
                path,
                file,
                phase: Phase::WaitingForMirror,
                started,
                deadline: started + OPERATOR_WAIT,
                posted: None,
                tested_cancel: false,
                rotations_checked: 0,
                failure: None,
                finished: report,
            }))
        }),
    )
    .expect("run native RustEmuDroid fixture");
    assert!(
        finished.get(),
        "mirror fixture did not complete; inspect the private receipt"
    );
}

#[test]
#[ignore = "requires the operator's BlueStacks lab with an animated game in the foreground"]
fn live_player_measures_and_disables_android_surface_fps() {
    struct FpsWindow {
        app: Launcher,
        pid: u32,
        package: String,
        file: File,
        started: Instant,
        sample: Option<Instant>,
        epoch: Option<u64>,
        count: u32,
        disabling: Option<(Instant, super::SurfaceFpsQuery)>,
        closing: bool,
        passed: bool,
        finished: Rc<Cell<bool>>,
    }

    impl App for FpsWindow {
        fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            if self.closing {
                if !self.app.blue_mirror.poll_cleanup(ctx) {
                    append(&mut self.file, self.started, "closed_clean", json!({}));
                    self.finished.set(self.passed);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                return;
            }
            if self.started.elapsed() >= Duration::from_secs(32) {
                append(
                    &mut self.file,
                    self.started,
                    "timed_out",
                    json!({"samples": self.count}),
                );
                self.app.blue_mirror.disconnect();
                self.closing = true;
                ctx.request_repaint();
                return;
            }
            self.app.update(ctx, frame);
            let panel = &self.app.blue_mirror;
            if let Some((pid, _)) = panel.player {
                assert_eq!(pid, self.pid, "unexpected BlueStacks player");
            }
            if let Some((since, query)) = &self.disabling {
                assert!(panel.surface_fps().is_none());
                assert!(!query.current() && panel.surface_query.is_none());
                if since.elapsed() >= Duration::from_millis(650) {
                    append(
                        &mut self.file,
                        self.started,
                        "disabled",
                        json!({"samples": self.count}),
                    );
                    self.passed = true;
                    self.app.blue_mirror.disconnect();
                    self.closing = true;
                }
            } else if let Some((epoch, package)) = &panel.surface_foreground
                && package == &self.package
                && let Some(value) = panel.surface_fps()
                && let Some(sample) = panel.surface_fps.sample_time()
                && self.sample.is_none_or(|previous| sample > previous)
            {
                assert!(
                    (10.0..=120.0).contains(&value),
                    "implausible game FPS {value}"
                );
                assert_eq!(
                    *self.epoch.get_or_insert(*epoch),
                    *epoch,
                    "foreground changed during sampling"
                );
                self.count += 1;
                self.sample = Some(sample);
                append(
                    &mut self.file,
                    self.started,
                    "sample",
                    json!({
                        "number": self.count, "fps": value, "package": package, "epoch": epoch
                    }),
                );
                if self.count == 3 {
                    self.disabling = Some((Instant::now(), panel.surface_query.clone().unwrap()));
                    self.app.config.android_fps = false;
                }
            }
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    let instance = env::var("RUSTEMUDROID_MIRROR_LIVE_INSTANCE").expect("select the running lab");
    let pid: u32 = env::var("RUSTEMUDROID_MIRROR_LIVE_PID")
        .unwrap()
        .parse()
        .unwrap();
    let adb = PathBuf::from(env::var_os("RUSTEMUDROID_MIRROR_LIVE_ADB").unwrap());
    let package = env::var("RUSTEMUDROID_SURFACE_FPS_PACKAGE").expect("selected foreground game");
    assert!(
        pid != 0
            && adb.is_absolute()
            && adb.is_file()
            && crate::bluestacks::valid_package(&package)
    );
    let receipt = PathBuf::from(env::var_os("RUSTEMUDROID_MIRROR_LIVE_RECEIPT").unwrap());
    assert!(receipt.is_absolute());
    let dir = receipt.parent().unwrap().canonicalize().unwrap();
    let root = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert!(!dir.starts_with(&root) || dir.starts_with(root.join(".rustemudroid/qa")));
    let file = File::create_new(dir.join(receipt.file_name().unwrap())).unwrap();
    let state = tempfile::tempdir_in(&dir).unwrap();
    let prefs = state.path().join("launcher.json");
    let mut config = LaunchConfig::load(None, state.path().to_owned(), None, &prefs);
    config.show_fps = true;
    config.android_fps = true;
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    let finished = Rc::new(Cell::new(false));
    let result = finished.clone();
    eframe::run_native(
        "RustEmuDroid BlueStacks Surface FPS fixture",
        options,
        Box::new(move |creation| {
            let mut app = Launcher::new(install_window(creation, brand), config, prefs, false);
            app.blue_mirror.instance = instance.clone();
            app.blue_mirror.session =
                Some(MirrorSession::start_existing_for_test(instance, pid, adb));
            Ok(Box::new(FpsWindow {
                app,
                pid,
                file,
                package,
                started: Instant::now(),
                sample: None,
                epoch: None,
                count: 0,
                disabling: None,
                closing: false,
                passed: false,
                finished: result,
            }))
        }),
    )
    .unwrap();
    assert!(finished.get(), "Surface FPS fixture did not complete");
}
