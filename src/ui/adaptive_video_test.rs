use super::*;
use std::{cell::RefCell, fs, io::BufReader, process::Command, result::Result as TestResult};

const NATIVE: (u32, u32) = (1600, 900);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureKind {
    Png,
    Video,
    Gif,
    Moment,
    Replay,
}

impl CaptureKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Video => "mp4",
            Self::Gif => "gif",
            Self::Moment | Self::Replay => "jxl",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Boot,
    Settings,
    TouchRelease,
    TapResult,
    Capture(CaptureKind),
    VideoRunning,
    Saved(CaptureKind),
    ReducedAgain(CaptureKind),
    ReplayReady,
    Fullscreen,
    Windowed,
    PreferenceOff,
    PreferenceOn,
    Close,
}

#[derive(Default)]
struct Evidence {
    frames: Vec<(&'static str, (u32, u32))>,
    captures: Vec<(CaptureKind, PathBuf)>,
    tap: Option<(egui::Pos2, (u32, u32))>,
}

type Outcome = Rc<RefCell<Option<TestResult<Evidence, String>>>>;

struct AdaptiveWindow {
    app: Launcher,
    step: Step,
    since: Instant,
    next_guest_poll: Instant,
    queue: Vec<egui::Event>,
    evidence: Evidence,
    error: Option<String>,
    result: Outcome,
}

impl AdaptiveWindow {
    fn guest(&self, args: &[&str]) -> TestResult<String, String> {
        self.app
            .controls
            .as_ref()
            .ok_or_else(|| "the test AVD controls are missing".to_owned())?
            .guest_output(args)
    }

    fn next(&mut self, step: Step) {
        self.step = step;
        self.since = Instant::now();
        self.next_guest_poll = self.since;
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.queue
            .extend([true, false].map(|pressed| egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers,
            }));
    }

    fn shortcut(&mut self, key: egui::Key) {
        self.key(key, crate::toolbar::PLAYER_MODIFIERS);
    }

    fn observe(&mut self, label: &'static str, original: bool) -> bool {
        let Some(display) = &self.app.display else {
            return false;
        };
        let shot = if original {
            display.native_shot()
        } else {
            display.shot()
        };
        let Some(shot) = shot else { return false };
        let size = (shot.width, shot.height);
        if (original && size != NATIVE) || (!original && (size.0 >= NATIVE.0 || size.1 >= NATIVE.1))
        {
            return false;
        }
        self.evidence.frames.push((label, size));
        true
    }

    fn touch_settings(&mut self, ctx: &egui::Context) -> TestResult<(), String> {
        let before = self.foreground()?;
        if network_is_open(&before) {
            return Err("Network & internet was already open before the touch".into());
        }
        let point = network_row(&self.hierarchy()?)?;
        let rect =
            game_image_rect(ctx).ok_or_else(|| "the Android image was not painted".to_owned())?;
        let pos = rect.min
            + egui::vec2(
                rect.width() * (point.0 as f32 / NATIVE.0 as f32),
                rect.height() * (point.1 as f32 / NATIVE.1 as f32),
            );
        self.queue.push(egui::Event::PointerMoved(pos));
        self.pointer_button(pos, true);
        eprintln!("tap from {before:?}: native={point:?} painted={rect:?} pointer={pos:?}");
        self.evidence.tap = Some((pos, point));
        Ok(())
    }

    fn hierarchy(&self) -> TestResult<String, String> {
        let path = format!(
            "/data/local/tmp/rustemudroid-adaptive-{}.xml",
            std::process::id()
        );
        let dumped = self.guest(&["shell", "uiautomator", "dump", &path])?;
        let xml = self.guest(&["shell", "cat", &path]);
        self.guest(&["shell", "rm", "-f", &path])?;
        xml.map_err(|error| format!("{dumped}; {error}"))
    }

    fn check_network_page(&self, foreground: &str) -> TestResult<bool, String> {
        if network_is_open(foreground) {
            return Ok(true);
        }
        if !foreground.contains("com.android.settings/.SubSettings") {
            return Ok(false);
        }
        let top = self.guest(&["shell", "dumpsys", "activity", "top"])?;
        let directory = self
            .app
            .prefs_file
            .parent()
            .expect("private test output directory");
        fs::write(directory.join("touch-activity-top.txt"), &top)
            .map_err(|error| error.to_string())?;
        if top.contains("NetworkDashboardFragment") {
            return Ok(true);
        }
        let xml = match self.hierarchy() {
            Ok(xml) => xml,
            Err(error) => {
                fs::write(directory.join("touch-hierarchy-error.txt"), &error)
                    .map_err(|error| error.to_string())?;
                return Ok(false);
            }
        };
        fs::write(directory.join("touch-hierarchy.xml"), &xml)
            .map_err(|error| error.to_string())?;
        network_page(&xml)
    }

    fn pointer_button(&mut self, pos: egui::Pos2, pressed: bool) {
        self.queue.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }

    fn foreground(&self) -> TestResult<String, String> {
        let activity = self.guest(&["shell", "dumpsys", "activity", "activities"])?;
        activity
            .lines()
            .find(|line| line.contains("topResumedActivity") || line.contains("mResumedActivity"))
            .map(|line| line.trim().to_owned())
            .ok_or_else(|| "Android has no resumed Activity".into())
    }

    fn capture(&mut self, kind: CaptureKind) {
        self.app.capture_notice = None;
        match kind {
            CaptureKind::Png => self.shortcut(crate::toolbar::SCREENSHOT_KEY),
            CaptureKind::Video => self.shortcut(egui::Key::R),
            CaptureKind::Gif => self.app.capture_action(Tool::AnimatedGif),
            CaptureKind::Moment => self.shortcut(egui::Key::J),
            CaptureKind::Replay => self.shortcut(egui::Key::H),
        }
        self.next(if kind == CaptureKind::Video {
            Step::VideoRunning
        } else {
            Step::Saved(kind)
        });
    }

    fn drive(&mut self, ctx: &egui::Context) -> TestResult<(), String> {
        let step = self.step;
        let native_window = step == Step::Fullscreen;
        if self.app.failed {
            return Err(self.app.status.clone());
        }
        if let Some((CaptureEvent::Failed(error), _)) = &self.app.capture_notice {
            return Err(error.clone());
        }
        if self.since.elapsed()
            > Duration::from_secs(if self.step == Step::Boot { 190 } else { 35 })
        {
            return Err(format!("{:?} timed out: {}", self.step, self.app.status));
        }
        match step {
            Step::Boot if self.app.home_ready && self.observe("initial reduced frame", false) => {
                let reply = self.guest(&["shell", "wm", "size"])?;
                if reply
                    .lines()
                    .last()
                    .is_none_or(|size| !size.trim_end().ends_with("1600x900"))
                {
                    return Err(format!("expected Android size 1600x900, got {reply}"));
                }
                let opened = self.guest(&[
                    "shell",
                    "am",
                    "start",
                    "-W",
                    "-a",
                    "android.settings.SETTINGS",
                ])?;
                if !opened.contains("Status: ok") {
                    return Err(format!("could not open Android Settings: {opened}"));
                }
                self.next(Step::Settings);
            }
            Step::Settings if self.since.elapsed() >= Duration::from_millis(750) => {
                self.touch_settings(ctx)?;
                self.next(Step::TouchRelease);
            }
            Step::TouchRelease if self.since.elapsed() >= Duration::from_millis(120) => {
                let (pos, _) = self.evidence.tap.expect("touch coordinate was recorded");
                self.pointer_button(pos, false);
                self.next(Step::TapResult);
            }
            Step::TapResult
                if self.since.elapsed() >= Duration::from_millis(750)
                    && Instant::now() >= self.next_guest_poll =>
            {
                self.next_guest_poll = Instant::now() + Duration::from_millis(250);
                let foreground = self.foreground()?;
                let network = self.check_network_page(&foreground)?;
                if network && self.observe("reduced Settings after touch", false) {
                    eprintln!("mapped tap opened {foreground}");
                    self.next(Step::Capture(CaptureKind::Png));
                } else if self.since.elapsed() >= Duration::from_secs(15) {
                    let directory = self
                        .app
                        .prefs_file
                        .parent()
                        .expect("private test output directory");
                    if let Ok(top) = self.guest(&["shell", "dumpsys", "activity", "top"]) {
                        let _ = fs::write(directory.join("touch-activity-timeout.txt"), top);
                    }
                    return Err(format!(
                        "the scaled pointer did not open Network & internet: {foreground}"
                    ));
                }
            }
            Step::Capture(kind) => self.capture(kind),
            Step::VideoRunning
                if self.app.capture.mode().is_some()
                    && self.since.elapsed() >= Duration::from_secs(2) =>
            {
                if !self.observe("native frame during MP4 recording", true) {
                    return Err("MP4 started with a reduced preview".into());
                }
                self.shortcut(egui::Key::R);
                self.next(Step::Saved(CaptureKind::Video));
            }
            Step::Saved(kind) => {
                if let Some((CaptureEvent::Saved(path), _)) = self.app.capture_notice.take() {
                    if path.extension().is_none_or(|ext| ext != kind.extension()) {
                        return Err(format!(
                            "unexpected output for {kind:?}: {}",
                            path.display()
                        ));
                    }
                    self.evidence.captures.push((kind, path));
                    if kind == CaptureKind::Replay {
                        if !self.app.capture.replay_armed()
                            || !self.observe("native replay frame", true)
                        {
                            return Err("replay did not stay armed with native video".into());
                        }
                        self.app.set_replay_armed(false);
                        if self.app.capture.replay_ready() {
                            return Err("disarmed replay kept a ready buffer".into());
                        }
                    }
                    self.next(Step::ReducedAgain(kind));
                }
            }
            Step::ReducedAgain(kind) if self.observe("reduced again after capture", false) => {
                match kind {
                    CaptureKind::Png => self.next(Step::Capture(CaptureKind::Video)),
                    CaptureKind::Video => self.next(Step::Capture(CaptureKind::Gif)),
                    CaptureKind::Gif => self.next(Step::Capture(CaptureKind::Moment)),
                    CaptureKind::Moment => {
                        self.app.set_replay_armed(true);
                        self.next(Step::ReplayReady);
                    }
                    CaptureKind::Replay => {
                        self.key(crate::toolbar::FULLSCREEN_KEY, egui::Modifiers::NONE);
                        self.next(Step::Fullscreen);
                    }
                }
            }
            Step::ReplayReady
                if self.app.capture.replay_ready()
                    && self.observe("native replay buffer", true) =>
            {
                self.next(Step::Capture(CaptureKind::Replay));
            }
            Step::Fullscreen | Step::Windowed => {
                if ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) != native_window {
                    return Ok(());
                }
                if native_window && self.observe("native in fullscreen", true) {
                    self.key(crate::toolbar::FULLSCREEN_KEY, egui::Modifiers::NONE);
                    self.next(Step::Windowed);
                } else if !native_window && self.observe("reduced after exiting fullscreen", false)
                {
                    self.app.config.adaptive_video = false;
                    self.next(Step::PreferenceOff);
                }
            }
            Step::PreferenceOff if self.observe("native with adaptive video disabled", true) => {
                self.app.config.adaptive_video = true;
                self.next(Step::PreferenceOn);
            }
            Step::PreferenceOn if self.observe("reduced after enabling preference", false) => {
                self.guest(&["shell", "input", "keyevent", "KEYCODE_HOME"])?;
                self.next(Step::Close);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            _ => {}
        }
        Ok(())
    }
}

impl App for AdaptiveWindow {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        if !self.queue.is_empty() || matches!(self.step, Step::TouchRelease | Step::TapResult) {
            raw.focused = true;
            raw.events
                .retain(|event| !matches!(event, egui::Event::WindowFocused(false)));
            raw.events.append(&mut self.queue);
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
        ctx.request_repaint_after(Duration::from_millis(16));
        if self.step == Step::Close {
            if self.app.closing && self.app.task.is_none() && self.result.borrow().is_none() {
                self.result.replace(Some(match self.error.take() {
                    Some(error) => Err(error),
                    None => Ok(std::mem::take(&mut self.evidence)),
                }));
            }
            return;
        }
        if let Err(error) = self.drive(ctx) {
            self.error = Some(error);
            self.next(Step::Close);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn network_is_open(activity: &str) -> bool {
    activity.contains("com.android.settings/") && activity.contains("NetworkDashboardActivity")
}

fn network_label(text: &str) -> bool {
    matches!(
        text,
        "network & internet" | "rede e internet" | "redes e internet" | "red e internet"
    )
}

fn network_page(xml: &str) -> TestResult<bool, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|error| error.to_string())?;
    let labels: Vec<_> = doc
        .descendants()
        .filter_map(|node| node.attribute("text"))
        .map(str::to_lowercase)
        .collect();
    Ok(labels.iter().any(|label| network_label(label))
        && labels.iter().any(|label| {
            matches!(
                label.as_str(),
                "airplane mode"
                    | "modo avião"
                    | "modo avión"
                    | "private dns"
                    | "dns privado"
                    | "data saver"
                    | "economia de dados"
                    | "ahorro de datos"
                    | "hotspot & tethering"
            )
        }))
}

fn game_image_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    ctx.graphics(|graphics| {
        graphics
            .get(egui::LayerId::background())?
            .all_entries()
            .filter_map(|entry| match &entry.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    let rect = mesh.calc_bounds();
                    let scale = rect.width() / rect.height();
                    (rect.width() > 300.0 && (scale - 16.0 / 9.0).abs() < 0.02).then_some(rect)
                }
                _ => None,
            })
            .max_by(|a, b| a.area().total_cmp(&b.area()))
    })
}

fn network_row(xml: &str) -> TestResult<(u32, u32), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|error| error.to_string())?;
    let mut points = doc.descendants().filter_map(|node| {
        if !network_label(&node.attribute("text")?.to_lowercase()) {
            return None;
        }
        let bounds: Vec<_> = node
            .attribute("bounds")?
            .split(['[', ']', ','])
            .filter_map(|part| part.parse::<u32>().ok())
            .collect();
        let [left, top, right, bottom] = *bounds.as_slice() else {
            return None;
        };
        (left < right && top < bottom && right <= NATIVE.0 && bottom <= NATIVE.1)
            .then_some(((left + right) / 2, (top + bottom) / 2))
    });
    match (points.next(), points.next()) {
        (Some(point), None) => Ok(point),
        _ => Err("expected one visible Network & internet row in QA Android Settings".into()),
    }
}

fn probe_capture(kind: CaptureKind, path: &Path) -> (u64, u64, u64) {
    if kind == CaptureKind::Png {
        let mut png = png::Decoder::new(BufReader::new(fs::File::open(path).unwrap()))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; png.output_buffer_size().unwrap()];
        let info = png.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), NATIVE);
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert!(
            pixels[..info.buffer_size()]
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != pixels[..3])
        );
        return (u64::from(info.width), u64::from(info.height), 1);
    }
    let output =
        Command::new(std::env::var_os("RUSTEMUDROID_FFPROBE").unwrap_or_else(|| "ffprobe".into()))
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,width,height,nb_read_frames",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let streams = response["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    let stream = &streams[0];
    let width = stream["width"].as_u64().unwrap();
    let height = stream["height"].as_u64().unwrap();
    let frames = stream["nb_read_frames"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    match kind {
        CaptureKind::Video => {
            assert_eq!(stream["codec_name"], "h264");
            assert_eq!((width, height), (u64::from(NATIVE.0), u64::from(NATIVE.1)));
            assert!(frames >= 10);
        }
        CaptureKind::Gif | CaptureKind::Moment | CaptureKind::Replay => {
            let codec = if kind == CaptureKind::Gif {
                "gif"
            } else {
                "jpegxl_anim"
            };
            assert_eq!(stream["codec_name"], codec);
            assert_eq!((width, height, frames), (720, 405, 30));
        }
        CaptureKind::Png => unreachable!(),
    }
    (width, height, frames)
}

fn private_directory(variable: &str) -> PathBuf {
    let directory =
        PathBuf::from(std::env::var_os(variable).expect("select an explicit private QA directory"));
    assert!(directory.is_absolute(), "{variable} must be absolute");
    let canonical = directory
        .canonicalize()
        .expect("create the QA directory before testing");
    assert!(
        canonical.is_dir()
            && canonical
                .ancestors()
                .any(|path| path.ends_with(".rustemudroid/qa")),
        "{variable} must be under .rustemudroid/qa"
    );
    directory
}

#[test]
fn adaptive_touch_target_requires_a_unique_visible_network_row() {
    let row = r#"<node text="Network &amp; internet" bounds="[20,240][260,300]"/>"#;
    assert_eq!(
        network_row(&format!("<hierarchy>{row}</hierarchy>")),
        Ok((140, 270))
    );
    assert!(network_row(&format!("<hierarchy>{row}{row}</hierarchy>")).is_err());
    assert!(network_row(&row.replace("300", "1200")).is_err());
    assert!(!network_page(&format!("<hierarchy>{row}</hierarchy>")).unwrap());
    assert!(
        network_page(&format!(
            "<hierarchy>{row}<node text=\"Private DNS\"/></hierarchy>"
        ))
        .unwrap()
    );
}

#[test]
#[ignore = "boots an explicit idle Windows QA AVD and encodes captures with installed FFmpeg/ffprobe"]
fn adaptive_video_preserves_live_capture_and_touch() {
    let state = private_directory("RUSTEMUDROID_TEST_ADAPTIVE_STATE");
    let root = private_directory("RUSTEMUDROID_TEST_ADAPTIVE_OUTPUT");
    let avd = state.join("avd/RustEmuDroid.avd").canonicalize().unwrap();
    assert!(avd.starts_with(state.canonicalize().unwrap()) && avd.join("config.ini").is_file());
    let ini = fs::read_to_string(state.join("avd/RustEmuDroid.ini")).unwrap();
    let declared = ini
        .lines()
        .find_map(|line| line.strip_prefix("path="))
        .unwrap();
    assert_eq!(
        Path::new(declared).canonicalize().unwrap(),
        avd,
        "AVD must stay inside the QA profile"
    );
    assert!(
        !root
            .canonicalize()
            .unwrap()
            .starts_with(state.canonicalize().unwrap())
    );
    let output = tempfile::Builder::new()
        .prefix("adaptive-video-")
        .tempdir_in(root)
        .unwrap()
        .keep();
    let prefs = output.join("launcher.json");
    let mut config = LaunchConfig::load(None, state.clone(), None, &state.join("launcher.json"));
    config.state = state.to_str().unwrap().to_owned();
    config.adaptive_video = true;
    config.android_fps = false;
    fs::write(&prefs, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let result: Outcome = Default::default();
    let finish = result.clone();
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid adaptive video test",
        options,
        Box::new(move |cc| {
            Ok(Box::new(AdaptiveWindow {
                app: Launcher::new(install_window(cc, brand), config, prefs, true),
                step: Step::Boot,
                since: Instant::now(),
                next_guest_poll: Instant::now(),
                queue: Vec::new(),
                evidence: Evidence::default(),
                error: None,
                result: finish,
            }))
        }),
    )
    .unwrap();
    let evidence = result
        .borrow_mut()
        .take()
        .expect("window closed before the test VM stopped")
        .unwrap();
    let captures: Vec<_> = evidence.captures.iter().map(|(kind, path)| {
        let (width, height, frames) = probe_capture(*kind, path);
        serde_json::json!({"kind": format!("{kind:?}"), "file": path.file_name().unwrap().to_string_lossy(),
                           "width": width, "height": height, "frames": frames})
    }).collect();
    let (pointer, guest) = evidence.tap.expect("guest touch did not complete");
    let receipt = serde_json::json!({"frames": evidence.frames, "captures": captures,
        "mapped_touch": {"window_points": [pointer.x, pointer.y], "target_native": [guest.0, guest.1]},
        "normal_vm_shutdown": true});
    fs::write(
        output.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    eprintln!("adaptive video evidence: {}", output.display());
}
