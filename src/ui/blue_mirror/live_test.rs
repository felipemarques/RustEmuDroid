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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    WaitingForMirror,
    TouchGate,
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
    if pid != expected_pid
        || point[0] >= size[0]
        || point[1] >= size[1]
        || ppp <= 0.0
        || !ppp.is_finite()
    {
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
            json!({"phase": format!("{:?}", self.phase), "reason": reason}),
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

    fn post(
        &mut self,
        ctx: &egui::Context,
        raw: &mut RawInput,
        button: PointerButton,
        phase: Phase,
    ) {
        let Some(layout) = self.layout(ctx) else {
            self.fail(
                "The confirmed DWM layout or guest dimensions changed before input".to_owned(),
            );
            return;
        };
        raw.focused = true;
        raw.modifiers = Modifiers::NONE;
        raw.viewports.entry(raw.viewport_id).or_default().focused = Some(true);
        raw.events.push(Event::PointerMoved(layout.position));
        for pressed in [true, false] {
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

impl App for MirrorWindowTest {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        // This fixture drives only its own egui RawInput. It never sends native OS input.
        raw.events.clear();
        match self.phase {
            Phase::TouchGate if marker(&self.path, ".touch").is_file() => {
                self.post(ctx, raw, PointerButton::Primary, Phase::TouchPosted);
            }
            Phase::BackGate if marker(&self.path, ".back").is_file() => {
                self.post(ctx, raw, PointerButton::Secondary, Phase::BackPosted);
            }
            Phase::SecondBackOrClose if marker(&self.path, ".back2").is_file() => {
                self.post(ctx, raw, PointerButton::Secondary, Phase::SecondBackPosted);
            }
            Phase::SecondBackOrClose | Phase::CloseGate
                if marker(&self.path, ".close").is_file() =>
            {
                self.advance(Phase::RequestClose);
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
    for suffix in [".touch", ".back", ".back2", ".close"] {
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
