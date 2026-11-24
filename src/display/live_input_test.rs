use super::*;
use egui::{CentralPanel, Event, Modifiers, RawInput, vec2};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    process::{Command, Output, Stdio},
    thread,
};

const HOME: &str = "packageName=org.rustemudroid.home ";
const SETTINGS: &str = "com.android.settings";

struct Guest {
    adb: OsString,
    serial: String,
    expected: PathBuf,
}

impl Guest {
    fn selected() -> (Self, u16, u16, String) {
        let env = |key| std::env::var(key).unwrap();
        let console = env("RUSTEMUDROID_TEST_CONSOLE").parse::<u16>().unwrap();
        let rpc = env("RUSTEMUDROID_TEST_TOUCH_RPC").parse::<u16>().unwrap();
        let guest = Self {
            adb: std::env::var_os("RUSTEMUDROID_TEST_ADB").unwrap(),
            serial: format!("emulator-{console}"),
            expected: env("RUSTEMUDROID_TEST_AVD").into(),
        };
        assert!(guest.owns(), "selected port belongs to a different AVD");
        assert!(
            guest.foreground().contains(HOME),
            "start from the Home library"
        );
        let device = env("RUSTEMUDROID_TEST_TOUCH_DEVICE");
        assert!(
            device
                .strip_prefix("/dev/input/event")
                .is_some_and(|n| n.parse::<u32>().is_ok())
        );
        let capabilities = guest.text(&["shell", "getevent", "-lp", &device]);
        assert!(
            capabilities.contains("ABS_MT_TRACKING_ID"),
            "{capabilities}"
        );
        (guest, console, rpc, device)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.adb);
        command.args(["-s", &self.serial]);
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn owns(&self) -> bool {
        let output = self.output(&["emu", "avd", "path"]);
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .is_some_and(|line| Path::new(line.trim()) == self.expected)
    }

    fn foreground(&self) -> String {
        self.text(&["shell", "dumpsys", "activity", "top-resumed"])
    }

    fn open_device_info(&self) -> RestoreHome<'_> {
        let restore = RestoreHome(Some(self));
        let started = self.text(&[
            "shell",
            "am",
            "start",
            "-W",
            "--user",
            "current",
            "-a",
            "android.settings.DEVICE_INFO_SETTINGS",
        ]);
        assert!(started.contains("Status: ok"), "{started}");
        assert!(
            self.foreground()
                .contains("packageName=com.android.settings ")
        );
        restore
    }

    fn capture_touch(&self, device: &str) -> thread::JoinHandle<Output> {
        assert!(self.owns());
        let capture = self
            .command()
            .args(["shell", "timeout", "6", "getevent", "-lt", device])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        thread::spawn(move || capture.wait_with_output().unwrap())
    }
}

struct RestoreHome<'a>(Option<&'a Guest>);

impl RestoreHome<'_> {
    fn finish(mut self, display: &Display) {
        let guest = self.0.unwrap();
        assert!(guest.owns());
        display.press("GoHome");
        let until = Instant::now() + Duration::from_secs(8);
        while !guest.foreground().contains(HOME) {
            assert!(
                Instant::now() < until,
                "could not return to the Home library"
            );
            thread::sleep(Duration::from_millis(120));
        }
        self.0 = None;
    }
}

impl Drop for RestoreHome<'_> {
    fn drop(&mut self) {
        if let Some(guest) = self.0.take()
            && guest.owns()
        {
            let _ = guest.output(&["shell", "input", "keyevent", "KEYCODE_HOME"]);
        }
    }
}

#[derive(Default, Debug)]
struct Trace {
    id: u32,
    released: bool,
    horizontal: Vec<u32>,
    vertical: Vec<u32>,
}

impl Trace {
    fn displacement(&self) -> (i64, i64) {
        let displacement = |values: &[u32]| {
            values
                .first()
                .zip(values.last())
                .map_or(0, |(&start, &end)| i64::from(end) - i64::from(start))
        };
        (displacement(&self.horizontal), displacement(&self.vertical))
    }
}

fn parse_touch_events(log: &str) -> (usize, Vec<Trace>) {
    let mut slot = 0;
    let mut active = BTreeMap::<u32, usize>::new();
    let mut peak = 0;
    let mut events = Vec::<Trace>::new();
    for line in log.lines() {
        let mut fields = line.split_whitespace().rev();
        let Some(value) = fields
            .next()
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        else {
            continue;
        };
        match fields.next() {
            Some("ABS_MT_SLOT") => slot = value,
            Some("ABS_MT_TRACKING_ID") if value == u32::MAX => {
                if let Some(index) = active.remove(&slot) {
                    events[index].released = true;
                }
            }
            Some("ABS_MT_TRACKING_ID") => {
                active.insert(slot, events.len());
                events.push(Trace {
                    id: value,
                    ..Trace::default()
                });
                peak = peak.max(active.len());
            }
            Some("ABS_MT_POSITION_X") => {
                if let Some(&index) = active.get(&slot) {
                    events[index].horizontal.push(value);
                }
            }
            Some("ABS_MT_POSITION_Y") => {
                if let Some(&index) = active.get(&slot) {
                    events[index].vertical.push(value);
                }
            }
            _ => {}
        }
    }
    (peak, events)
}

#[test]
#[ignore = "requires a running selected idle AVD with getevent access and the RustEmuDroid Home"]
fn advanced_swipe_and_pinch_reach_selected_android() {
    let (guest, console, rpc, device) = Guest::selected();
    // Use a system information page as the recipient of test gestures. No installed
    // game receives a control and edits are isolated in a host temporary directory.
    let restore = guest.open_device_info();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("gesture-test.json");
    std::fs::write(&file, r#"{"com.android.settings":{"active":"Probe","layouts":{"Probe":{
        "buttons":{},"stick":null,"radius":0.1,"controls":[
          {"input":"P","at":[0.4,0.66],"gesture":{"kind":"tap","duration_ms":65}},
          {"input":"S","at":[0.36,0.69],"gesture":{"kind":"swipe","end":[0.71,0.71],"duration_ms":400}},
          {"input":"Z","at":[0.52,0.72],"gesture":{"kind":"pinch","from":0.10,"to":0.30,"duration_ms":400}}
        ]}}}}"#).unwrap();
    let ctx = egui::Context::default();
    let mut display = Display::open(console, rpc, guest.expected.clone(), &ctx);
    display.mapping.load(file);
    display.set_package(Some(SETTINGS));
    assert!(
        [Key::P, Key::S, Key::Z]
            .into_iter()
            .all(|key| display.mapping.captures(key))
    );
    let tick = |display: &mut Display, time, events| {
        let _ = ctx.run(
            RawInput {
                time: Some(time),
                events,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(960., 640.))),
                ..Default::default()
            },
            |ctx| {
                CentralPanel::default().show(ctx, |ui| display.show(ui, true, Locale::default()));
            },
        );
    };
    let ready = Instant::now();
    while display.texture.is_none() {
        assert!(
            ready.elapsed() < Duration::from_secs(20),
            "no display frames after connect"
        );
        tick(&mut display, ready.elapsed().as_secs_f64(), vec![]);
        thread::sleep(Duration::from_millis(50));
    }

    let capture = guest.capture_touch(&device);
    thread::sleep(Duration::from_millis(300));
    let key = |key, pressed| Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let start = ready.elapsed().as_secs_f64() + 1.;
    for (index, target) in [Key::P, Key::S, Key::Z].into_iter().enumerate() {
        let origin = start + index as f64;
        tick(
            &mut display,
            origin,
            vec![key(target, true), key(target, false)],
        );
        for (step, delta) in [0.04, 0.12, 0.24, 0.33, 0.46].into_iter().enumerate() {
            if step == 4 && target == Key::Z {
                // End the pinch via focus loss while both pointers are still active.
                tick(
                    &mut display,
                    origin + delta,
                    vec![Event::WindowFocused(false)],
                );
            } else {
                tick(&mut display, origin + delta, vec![]);
            }
            thread::sleep(Duration::from_millis(75));
        }
    }
    let result = capture.join().unwrap();
    let log = String::from_utf8(result.stdout).unwrap();
    let (peak, contacts) = parse_touch_events(&log);
    restore.finish(&display);
    assert_eq!(peak, 2, "{log}");
    assert_eq!(
        contacts.len(),
        4,
        "{}\n{log}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        contacts.iter().all(|contact| contact.released),
        "{contacts:?}"
    );
    assert_ne!(contacts[2].id, contacts[3].id, "{contacts:?}");
    let (swipe_x, swipe_y) = contacts[1].displacement();
    assert!(swipe_x.abs() + swipe_y.abs() > 20);
    let [a, b] = &contacts[2..] else {
        unreachable!()
    };
    let (ax, ay) = a.displacement();
    let (bx, by) = b.displacement();
    let dot = ax * bx + ay * by;
    assert!(dot < 0, "{contacts:?}");
    println!(
        "Android input device {device}: mapped keyboard tap, moving swipe, two simultaneous pinch IDs, all released"
    );
}

#[test]
#[ignore = "requires a running selected idle AVD with getevent access and the RustEmuDroid Home"]
fn edge_scroll_reaches_selected_android() {
    let (guest, console, rpc, device) = Guest::selected();
    let restore = guest.open_device_info();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("gesture-test.json");
    std::fs::write(
        &file,
        r#"{"com.android.settings":{"active":"Probe","layouts":{"Probe":{
        "buttons":{},"stick":null,"controls":[{"input":"","at":[0.5,0.5],"gesture":{
          "kind":"edge-scroll","margin_x":0.06,"margin_y":0.06,"speed_x":0.75,"speed_y":0.75
        }}]}}}}"#,
    )
    .unwrap();
    let ctx = egui::Context::default();
    let mut display = Display::open(console, rpc, guest.expected.clone(), &ctx);
    display.mapping.load(file);
    display.set_package(Some(SETTINGS));

    let screen = Rect::from_min_size(Pos2::ZERO, vec2(840., 840.));
    let tick = |display: &mut Display, time, focused, events| {
        let mut outer = Rect::NOTHING;
        let _ = ctx.run(
            RawInput {
                time: Some(time),
                focused,
                events,
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    outer = ui.available_rect_before_wrap();
                    display.show(ui, true, Locale::default());
                });
            },
        );
        outer
    };
    let ready = Instant::now();
    let (outer, view) = loop {
        let outer = tick(&mut display, ready.elapsed().as_secs_f64(), true, vec![]);
        if display.texture.is_some() {
            let android = vec2(display.size[0] as f32, display.size[1] as f32);
            let scale = (outer.width() / android.x).min(outer.height() / android.y);
            break (
                outer,
                Rect::from_center_size(outer.center(), android * scale),
            );
        }
        assert!(
            ready.elapsed() < Duration::from_secs(20),
            "no display frames after connect"
        );
        thread::sleep(Duration::from_millis(50));
    };

    let letterbox = if outer.width() - view.width() > 8. {
        egui::pos2(outer.left() + 2., view.center().y)
    } else {
        egui::pos2(view.center().x, outer.top() + 2.)
    };
    assert!(outer.contains(letterbox) && !view.contains(letterbox));
    let right = egui::pos2(view.right() - 2., view.center().y);
    let top = egui::pos2(view.center().x, view.top() + 2.);
    let expected_delta = |offset| {
        let (x, y) = display.coordinates(view.center(), view);
        let (end_x, end_y) = display.coordinates(view.center() + offset, view);
        (i64::from(end_x - x), i64::from(end_y - y))
    };
    let leftward = expected_delta(vec2(-30., 0.));
    let downward = expected_delta(vec2(0., 30.));

    let capture = guest.capture_touch(&device);
    thread::sleep(Duration::from_millis(300));
    let mut time = ready.elapsed().as_secs_f64();
    let mut step = |display: &mut Display, focused, events| {
        time += 1. / 60.;
        tick(display, time, focused, events);
        thread::sleep(Duration::from_millis(15));
    };
    step(&mut display, true, vec![Event::PointerMoved(letterbox)]);
    step(&mut display, true, vec![Event::PointerMoved(right)]);
    for _ in 0..23 {
        step(&mut display, true, vec![]);
    }
    step(&mut display, true, vec![Event::PointerMoved(letterbox)]);
    step(&mut display, true, vec![]);
    step(&mut display, true, vec![Event::PointerMoved(top)]);
    for _ in 0..8 {
        step(&mut display, true, vec![]);
    }
    step(&mut display, false, vec![Event::WindowFocused(false)]);
    step(&mut display, false, vec![Event::PointerMoved(right)]);

    let result = capture.join().unwrap();
    let log = String::from_utf8(result.stdout).unwrap();
    let (peak, contacts) = parse_touch_events(&log);
    restore.finish(&display);
    assert_eq!(
        peak,
        1,
        "{}\n{log}",
        String::from_utf8_lossy(&result.stderr)
    );
    let [first, repeated, top] = contacts.as_slice() else {
        panic!("expected two right-edge drags and one top-edge drag: {contacts:?}\n{log}");
    };
    for (contact, (dx, dy)) in [(first, leftward), (repeated, leftward), (top, downward)] {
        let (x, y) = contact.displacement();
        assert!(
            contact.released && x * dx + y * dy > 0,
            "{contact:?}\n{log}"
        );
        assert_eq!(
            x * dy - y * dx,
            0,
            "unexpected cross-axis movement: {contact:?}"
        );
    }
    println!(
        "Android input device {device}: two right-edge drags, one top-edge drag, all released"
    );
}
