use super::*;
use serde_json::Value;
use std::process::Command;

const GRID: usize = 12;
const SIGNATURE_SIZE: usize = GRID * GRID * 3;
type Signature = [u8; SIGNATURE_SIZE];

#[derive(PartialEq, Eq)]
enum Step {
    Boot,
    SettingsReference,
    HomeReference,
    Warmup,
    Settings,
    SaveShortcut,
    AfterSave,
    Close,
}

struct ReplayCheck {
    path: PathBuf,
    native: (u32, u32),
    home: Signature,
    settings: Signature,
    quick_settings: Signature,
}

struct ReplayWindow {
    app: Launcher,
    step: Step,
    next: Instant,
    deadline: Instant,
    native: (u32, u32),
    home: Signature,
    settings: Signature,
    check: Option<ReplayCheck>,
    finished: Rc<Cell<Option<ReplayCheck>>>,
}

impl ReplayWindow {
    fn guest(&self, args: &[&str]) -> String {
        self.app
            .controls
            .as_ref()
            .unwrap()
            .guest_output(args)
            .unwrap()
    }

    fn open_settings(&self, wait_for_activity: bool) {
        let mut args = vec!["shell", "am", "start"];
        let expected = if wait_for_activity {
            args.push("-W");
            "Status: ok"
        } else {
            "Starting:"
        };
        args.extend(["-a", "android.settings.SETTINGS"]);
        let reply = self.guest(&args);
        assert!(
            reply.contains(expected) && !reply.contains("Error"),
            "{reply}"
        );
    }

    fn signature(&self) -> Signature {
        let shot = self.app.display.as_ref().unwrap().shot().unwrap();
        assert_eq!(
            (shot.width, shot.height),
            self.native,
            "guest display resized"
        );
        // A grid of area averages tolerates status text, mascot motion and lossy JXL.
        let (width, height) = (shot.width as usize, shot.height as usize);
        let mut output = [0; SIGNATURE_SIZE];
        for row in 0..GRID {
            for col in 0..GRID {
                let mut total = [0; 3];
                let mut count = 0;
                for y in (row * height / GRID..(row + 1) * height / GRID).step_by(4) {
                    for x in (col * width / GRID..(col + 1) * width / GRID).step_by(4) {
                        let offset = (y * width + x) * 4;
                        for (channel, sum) in total.iter_mut().enumerate() {
                            *sum += u32::from(shot.pixels[offset + channel]);
                        }
                        count += 1;
                    }
                }
                let offset = (row * GRID + col) * 3;
                output[offset..offset + 3].copy_from_slice(&total.map(|sum| (sum / count) as u8));
            }
        }
        output
    }

    fn advance(&mut self, step: Step, millis: u64) {
        self.step = step;
        self.next = Instant::now() + Duration::from_millis(millis);
    }
}

impl App for ReplayWindow {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let due = Instant::now() >= self.next;
        if due && matches!(self.step, Step::HomeReference | Step::SaveShortcut) {
            ctx.input_mut(|input| {
                input.events.push(egui::Event::Key {
                    key: egui::Key::H,
                    physical_key: Some(egui::Key::H),
                    pressed: true,
                    repeat: false,
                    modifiers: crate::ui::capture_panel::CAPTURE_MODIFIERS,
                });
            });
        }
        self.app.update(ctx, frame);
        ctx.request_repaint_after(Duration::from_millis(16));
        assert!(!self.app.failed, "{}", self.app.status);
        assert!(
            Instant::now() < self.deadline,
            "Android replay timed out: {}",
            self.app.status
        );
        if self.step == Step::Close {
            if self.app.closing
                && self.app.task.is_none()
                && let Some(check) = self.check.take()
            {
                self.finished.set(Some(check));
            }
            return;
        }
        if !self.app.home_ready {
            if let Some(Err(error)) = self.app.controls.as_ref().and_then(|c| c.message.as_ref()) {
                panic!("Android Home could not be prepared: {error}");
            }
            return;
        }
        if !due {
            return;
        }
        match self.step {
            Step::Boot => {
                let Some(shot) = self.app.display.as_ref().and_then(Display::shot) else {
                    return;
                };
                self.native = (shot.width, shot.height);
                assert!(shot.width >= GRID as u32 && shot.height >= GRID as u32);
                assert!(!self.app.capture.replay_armed());
                // Prewarm the activity before arming and let Android's splash animation finish.
                self.open_settings(true);
                self.advance(Step::SettingsReference, 1_500);
            }
            Step::SettingsReference => {
                self.settings = self.signature();
                self.guest(&["shell", "input", "keyevent", "KEYCODE_HOME"]);
                self.advance(Step::HomeReference, 1_200);
            }
            Step::HomeReference => {
                self.home = self.signature();
                assert!(
                    distance(&self.settings, &self.home) >= 8.0,
                    "Android Settings and Home look too similar"
                );
                assert!(!self.app.capture.replay_ready());
                assert!(!self.app.capture.replay_saving());
                assert!(!self.app.capture_dir().exists());
                self.app.capture_notice = None;
                self.app.set_replay_armed(true);
                assert!(self.app.capture.replay_armed());
                self.advance(Step::Warmup, 3_350);
            }
            Step::Warmup => {
                assert!(
                    self.app.capture.replay_ready(),
                    "replay did not sample the idle Android Home"
                );
                self.open_settings(false);
                self.advance(Step::Settings, 200);
            }
            Step::Settings => {
                assert!(
                    self.next.elapsed() < Duration::from_secs(4),
                    "Settings did not reach the displayed reference"
                );
                if distance(&self.signature(), &self.settings) < 4.0 {
                    self.advance(Step::SaveShortcut, 550);
                }
            }
            Step::SaveShortcut => {
                let at_save = self.signature();
                assert!(
                    distance(&at_save, &self.settings) < 6.0,
                    "Settings changed before saving"
                );
                self.settings = at_save;
                self.guest(&["shell", "cmd", "statusbar", "expand-settings"]);
                self.advance(Step::AfterSave, 1_000);
            }
            Step::AfterSave => {
                assert!(
                    self.next.elapsed() < Duration::from_secs(30),
                    "replay save did not complete"
                );
                let path = match &self.app.capture_notice {
                    Some((CaptureEvent::Saved(path), _)) => path.clone(),
                    Some((CaptureEvent::Failed(error), _)) => panic!("replay save failed: {error}"),
                    None => return,
                };
                let quick_settings = self.signature();
                for earlier in [&self.home, &self.settings] {
                    assert!(
                        distance(earlier, &quick_settings) >= 8.0,
                        "post-save Quick Settings did not appear clearly"
                    );
                }
                self.check = Some(ReplayCheck {
                    path,
                    native: self.native,
                    home: self.home,
                    settings: self.settings,
                    quick_settings,
                });
                self.guest(&["shell", "cmd", "statusbar", "collapse"]);
                self.guest(&["shell", "input", "keyevent", "KEYCODE_HOME"]);
                self.advance(Step::Close, 0);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Step::Close => {}
        }
    }
}

fn distance(left: &[u8], right: &[u8]) -> f64 {
    assert_eq!(left.len(), right.len());
    let difference: u32 = left
        .iter()
        .zip(right)
        .map(|(a, b)| u32::from(a.abs_diff(*b)))
        .sum();
    f64::from(difference) / left.len() as f64
}

fn run(mut command: Command) -> Vec<u8> {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn check_replay(check: ReplayCheck) {
    assert_eq!(check.path.extension().unwrap(), "jxl");
    let mut probe = Command::new("ffprobe");
    probe
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_frames",
            "-show_entries",
            "stream=codec_name,width,height:frame=best_effort_timestamp_time,duration_time",
            "-of",
            "json",
        ])
        .arg(&check.path);
    let content: Value = serde_json::from_slice(&run(probe)).unwrap();
    let streams = content["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["codec_name"], "jpegxl_anim");
    let width = streams[0]["width"].as_u64().unwrap();
    let height = streams[0]["height"].as_u64().unwrap();
    let (native_width, native_height) = (u64::from(check.native.0), u64::from(check.native.1));
    let native_long_edge = native_width.max(native_height);
    assert_eq!(width.max(height), native_long_edge.min(720));
    assert!((width * native_height).abs_diff(height * native_width) <= native_long_edge);
    let timestamps = content["frames"].as_array().unwrap();
    assert_eq!(timestamps.len(), 30);
    for (index, frame) in timestamps.iter().enumerate() {
        let seconds = |field: &str| frame[field].as_str().unwrap().parse::<f64>().unwrap();
        let pts = seconds("best_effort_timestamp_time");
        assert!(
            (pts - index as f64 / 10.0).abs() < 0.002,
            "frame {index} at {pts}s"
        );
        assert!(
            (seconds("duration_time") - 0.1).abs() < 0.002,
            "frame {index} has wrong duration"
        );
    }

    let mut ffmpeg =
        Command::new(std::env::var_os("RUSTEMUDROID_FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
    ffmpeg
        .args(["-v", "error", "-xerror", "-nostdin", "-i"])
        .arg(&check.path)
        .args([
            "-vf",
            "scale=12:12:flags=area,format=rgb24",
            "-fps_mode",
            "passthrough",
            "-threads",
            "1",
            "-f",
            "rawvideo",
            "pipe:1",
        ]);
    let decoded = run(ffmpeg);
    assert_eq!(decoded.len(), 30 * SIGNATURE_SIZE);
    for (index, pixels) in decoded.chunks_exact(SIGNATURE_SIZE).enumerate() {
        let home = distance(pixels, &check.home);
        let settings = distance(pixels, &check.settings);
        let quick_settings = distance(pixels, &check.quick_settings);
        let scene = format!(
            "frame {index}: Home {home:.2}, Settings {settings:.2}, Quick Settings {quick_settings:.2}"
        );
        if index < 5 {
            assert!(home < 6.0 && home < settings, "{scene}");
        } else if index >= 26 {
            assert!(settings < 6.0 && settings < home, "{scene}");
        }
        assert!(
            quick_settings > 4.0 || quick_settings >= home.min(settings),
            "{scene}"
        );
    }
    eprintln!(
        "{}: {width}x{height}, 30 frames at 10 FPS (3.0 s), Android Home before Settings",
        check.path.display()
    );
}

#[test]
#[ignore = "requires a native Windows window, an explicitly selected idle AVD, and FFmpeg/ffprobe with animated JPEG XL"]
fn rolling_replay_saves_retrospective_android_frames() {
    let state = PathBuf::from(
        std::env::var_os("RUSTEMUDROID_TEST_STATE").expect("select an idle Android profile"),
    );
    assert!(
        state.join("avd/RustEmuDroid.ini").is_file(),
        "select an existing test profile"
    );
    let mut config = LaunchConfig::load(None, state.clone(), None, &state.join("launcher.json"));
    config.state = state
        .to_str()
        .expect("test profile path must be UTF-8")
        .to_owned();
    let output = std::env::var_os("RUSTEMUDROID_TEST_CAPTURES");
    let directory = if let Some(root) = &output {
        std::fs::create_dir_all(root).unwrap();
        tempfile::Builder::new()
            .prefix("rolling-replay-")
            .tempdir_in(root)
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    };
    let prefs = directory.path().join("launcher.json");
    let result = Rc::new(Cell::new(None));
    let finished = result.clone();
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid replay test",
        options,
        Box::new(move |cc| {
            let now = Instant::now();
            Ok(Box::new(ReplayWindow {
                app: Launcher::new(install_window(cc, brand), config, prefs, true),
                step: Step::Boot,
                next: now,
                deadline: now + Duration::from_secs(420),
                native: (0, 0),
                home: [0; SIGNATURE_SIZE],
                settings: [0; SIGNATURE_SIZE],
                check: None,
                finished,
            }))
        }),
    )
    .unwrap();
    let check = result
        .take()
        .expect("window closed before the test Android VM exited");
    if output.is_some() {
        let _ = directory.keep();
    }
    check_replay(check);
}
