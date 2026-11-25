//! Opt-in test of the launcher APK button against a wrapper-owned Pie64_1.
use super::{MirrorSession, egui};
use crate::locale::Locale;
use crate::ui::{LaunchConfig, Launcher, branded_window, install_window};
use egui::{Event, Modifiers, PointerButton, RawInput};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    env, fs,
    io::Write,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use winit::platform::windows::EventLoopBuilderExtWindows;

const LABEL_LOCALES: [Locale; 3] = [Locale::PtBr, Locale::EnUs, Locale::EsEs];
const APK_SHA: &str = "3154276474ca632cc003d54152f46421a0f417ec95de74a1cd539250a34b2895";

struct InstallFixture {
    launcher: Launcher,
    receipt: fs::File,
    directory: PathBuf,
    expected_pid: u32,
    click: Option<egui::Pos2>,
    clicked: bool,
    closing: Option<Instant>,
    closed: bool,
    installed: Rc<Cell<bool>>,
    deadline: Instant,
    label_index: usize,
    label_deadline: Option<Instant>,
    label_rect: Option<egui::Rect>,
}

impl InstallFixture {
    fn record(&mut self, event: &str, details: Value) {
        writeln!(
            self.receipt,
            "{}",
            json!({"event": event, "details": details})
        )
        .unwrap();
        self.receipt.flush().unwrap();
    }

    fn finish(&mut self, error: Option<String>) {
        self.installed.set(error.is_none());
        if let Some(error) = error {
            self.record("failed", json!(error));
        }
        self.launcher.blue_mirror.disconnect();
        self.closing = Some(Instant::now());
    }
}

impl eframe::App for InstallFixture {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        if let Some(Event::Screenshot { image, .. }) = raw
            .events
            .iter()
            .find(|event| matches!(event, Event::Screenshot { .. }))
            && let Some(rect) = self.label_rect.take()
        {
            let ppp = ctx.pixels_per_point();
            let left = ((rect.left() - 5.0) * ppp).floor() as usize;
            let top = ((rect.top() - 4.0) * ppp).floor() as usize;
            let right = (((rect.right() + 5.0) * ppp).ceil() as usize).min(image.width());
            let bottom = (((rect.bottom() + 4.0) * ppp).ceil() as usize).min(image.height());
            if left < right && top < bottom {
                let locale = LABEL_LOCALES[self.label_index].tag();
                let label = image.region_by_pixels([left, top], [right - left, bottom - top]);
                crate::ui::live_window_test::WindowTest::save(
                    &self.directory,
                    &format!("apk-installed-{locale}"),
                    &label,
                );
                self.record("localized_notice", json!(locale));
            }
            self.label_index += 1;
            if let Some(locale) = LABEL_LOCALES.get(self.label_index) {
                self.launcher.config.locale = *locale;
            }
        }
        raw.events.retain(|event| {
            !matches!(
                event,
                Event::Key { .. }
                    | Event::WindowFocused(_)
                    | Event::PointerMoved(_)
                    | Event::PointerButton { .. }
                    | Event::PointerGone
            )
        });
        raw.focused = true;
        if let Some(root) = raw.viewports.get_mut(&raw.viewport_id) {
            root.focused = Some(true);
        }
        if let Some(point) = self.click.take() {
            self.record("clicked_install_button", json!([point.x, point.y]));
            raw.events.push(Event::PointerMoved(point));
            for pressed in [true, false] {
                raw.events.push(Event::PointerButton {
                    pos: point,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            self.clicked = true;
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.closed {
            return;
        }
        if let Some(started) = self.closing {
            if !self.launcher.blue_mirror.poll_cleanup(ctx) {
                self.record("closed", Value::Null);
                self.closed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if started.elapsed() > Duration::from_secs(12) {
                self.installed.set(false);
                self.record("failed", json!("mirror cleanup did not finish"));
                self.closed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }

        self.launcher.update(ctx, frame);
        let panel = &self.launcher.blue_mirror;
        if let Some(deadline) = self.label_deadline {
            if self.label_index == LABEL_LOCALES.len() || Instant::now() >= deadline {
                self.finish(None);
            } else if self.label_rect.is_none()
                && let Some(rect) = panel.test_apk_notice_rect
            {
                self.label_rect = Some(rect);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
        } else if panel
            .player
            .is_some_and(|(pid, _)| pid != self.expected_pid)
        {
            self.finish(Some(
                "launcher attached to an unexpected BlueStacks PID".into(),
            ));
        } else if let Some((result, _)) = &panel.apk_notice {
            if let Err(error) = result {
                self.finish(Some(error.to_string()));
            } else {
                self.record("install_notice_success", Value::Null);
                self.launcher.config.locale = LABEL_LOCALES[0];
                self.label_deadline = Some(Instant::now() + Duration::from_secs(4));
            }
        } else if Instant::now() >= self.deadline {
            let detail = format!(
                "APK notice did not arrive (clicked={}, input error={:?})",
                self.clicked, panel.input_error
            );
            self.finish(Some(detail));
        } else if !self.clicked && panel.preview.is_some() && panel.player.is_some() {
            self.click = panel.test_install_button;
        }
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

#[test]
#[ignore = "requires owner-authorized Pie64_1, wrapper preflight and the designated test APK"]
fn live_player_installs_apk_from_mirror_button() {
    let directory =
        fs::canonicalize(env::var_os("RUSTEMUDROID_INSTALL_LIVE_DIR").unwrap()).unwrap();
    let repo = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    let qa = fs::canonicalize(repo.join(".rustemudroid/qa")).unwrap();
    assert!(directory.starts_with(&qa) && directory != qa);
    let pid = env::var("RUSTEMUDROID_MIRROR_LIVE_PID")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert_ne!(pid, 0);
    let adb = PathBuf::from(env::var_os("RUSTEMUDROID_MIRROR_LIVE_ADB").unwrap());
    assert!(adb.is_absolute() && adb.is_file());
    let apk = fs::canonicalize(directory.join("fixture APK with spaces.apk")).unwrap();
    assert!(apk.starts_with(&directory) && apk.is_file());
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&apk).unwrap())),
        APK_SHA
    );
    let receipt = fs::File::create_new(directory.join("install-events.jsonl")).unwrap();
    let state = directory.join("profile");
    let prefs = state.join("launcher.json");
    let mut config = LaunchConfig::load(None, state, None, &prefs);
    config.locale = Locale::EnUs;
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    let installed = Rc::new(Cell::new(false));
    let result = installed.clone();
    eframe::run_native(
        "RustEmuDroid BlueStacks APK installation fixture",
        options,
        Box::new(move |creation| {
            let mut launcher = Launcher::new(install_window(creation, brand), config, prefs, false);
            let instance = "Pie64_1".to_owned();
            launcher.blue_mirror.instance = instance.clone();
            launcher.blue_mirror.session =
                Some(MirrorSession::start_existing_for_test(instance, pid, adb));
            launcher.blue_mirror.test_apk_path = Some(apk);
            Ok(Box::new(InstallFixture {
                launcher,
                receipt,
                directory,
                expected_pid: pid,
                click: None,
                clicked: false,
                closing: None,
                closed: false,
                installed: result,
                deadline: Instant::now() + Duration::from_secs(100),
                label_index: 0,
                label_deadline: None,
                label_rect: None,
            }))
        }),
    )
    .unwrap();
    assert!(
        installed.get(),
        "APK installation fixture failed; inspect its receipt"
    );
}
