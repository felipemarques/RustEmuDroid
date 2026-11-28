//! Interactive Windows experiment. DWM supplies pixels and the normal Rust controller supplies input.
use super::*;
use crate::bluestacks::preview::{PreviewRect, ThumbnailPreview};
use crate::controls::Controls;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::io;
use std::sync::mpsc as sync;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::BTreeSet, fs, path::Path};
use windows_sys::Win32::UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::GetForegroundWindow};
use winit::platform::windows::EventLoopBuilderExtWindows;

fn trace(event: &str, args: std::fmt::Arguments<'_>) {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    println!(
        "dwm_trace host_unix_us={} event={event} {args}",
        timestamp.as_micros()
    );
}

fn physical(rect: PreviewRect) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

fn viewport_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::PointerButton { .. }
            | egui::Event::PointerMoved(_)
            | egui::Event::PointerGone
            | egui::Event::MouseWheel { .. }
            | egui::Event::Key { .. }
            | egui::Event::WindowFocused(_)
    )
}

fn contact_phase(active: &mut BTreeSet<i32>, id: i32, pressure: i32) -> &'static str {
    if pressure == 0 {
        if active.remove(&id) {
            "up"
        } else {
            "orphan_up"
        }
    } else if active.insert(id) {
        "down"
    } else {
        "move"
    }
}

struct Lab {
    preview: ThumbnailPreview,
    display: Display,
    controls: Controls,
    deadline: Instant,
    positioned: bool,
    last_geometry: Option<(PreviewRect, PreviewRect, f32, u32)>,
    last_focus: Option<(bool, usize)>,
    package: Option<String>,
}

impl eframe::App for Lab {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        for event in raw.events.iter().filter(|event| viewport_event(event)) {
            trace(
                "viewport_input",
                format_args!("focused={} input={event:?}", raw.focused),
            );
        }
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if Instant::now() >= self.deadline || ctx.input(|i| i.viewport().close_requested()) {
            self.display.release();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let package = self.controls.foreground_package(ctx);
        self.display.set_package(package);
        if self.package.as_deref() != package {
            trace("package", format_args!("android={package:?}"));
            self.package = package.map(str::to_owned);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            let area = ui.max_rect().intersect(ui.clip_rect());
            let scale = ctx.pixels_per_point();
            let Some(slot) = PreviewRect::from_points(area.min.into(), area.max.into(), scale) else {
                self.display.release();
                let _ = self.preview.hide();
                return;
            };
            let shown = match self.preview.position(slot) {
                Ok(shown) => shown,
                Err(error) => {
                    self.last_geometry = None;
                    self.display.release();
                    ui.label(error.to_string());
                    return;
                }
            };
            let rect = Rect::from_min_max(
                Pos2::new(shown.left as f32 / scale, shown.top as f32 / scale),
                Pos2::new(shown.right as f32 / scale, shown.bottom as f32 / scale),
            );
            let [width, height] = self.display.size;
            let aspect = rect.aspect_ratio() * height as f32 / width as f32;
            let hwnd = self.preview.lab_windows().0;
            // The preview owns this HWND on the current thread.
            let dpi = unsafe { GetDpiForWindow(hwnd) };
            let foreground = unsafe { GetForegroundWindow() } as usize;
            let focused = ctx.input(|i| i.focused);
            if self.last_focus != Some((focused, foreground)) {
                trace("focus", format_args!("egui={focused} foreground={foreground:#x} hwnd={hwnd:p}"));
                self.last_focus = Some((focused, foreground));
            }
            if self.last_geometry != Some((slot, shown, scale, dpi)) {
                trace("geometry", format_args!("slot_px={:?} fitted_px={:?} pixels_per_point={scale:.4} dpi={dpi} android=[{width}, {height}] rotation={} aspect_valid={}",
                    physical(slot), physical(shown), self.display.rotation, (0.99..=1.01).contains(&aspect)));
                self.last_geometry = Some((slot, shown, scale, dpi));
            }
            if !(0.99..=1.01).contains(&aspect) {
                self.display.release();
                return;
            }
            if !self.positioned {
                println!("dwm_input_ready guest={width}x{height} rotation={} pixels_per_point={scale}", self.display.rotation);
                self.positioned = true;
            }
            let routed: Vec<_> = ui.input(|input| input.events.iter().filter(|e| viewport_event(e)).cloned().collect());
            for event in &routed {
                let pos = match event {
                    egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) => Some(*pos),
                    _ => None,
                };
                let projection = pos.map(|p| (rect.intersect(area).contains(p),
                    self.display.coordinates(p, rect)));
                let mapped_key = matches!(event, egui::Event::Key { key, .. } if self.display.mapping.captures(*key));
                let mapped_pointer = self.display.mapping.captures_pointer(egui::PointerButton::Primary);
                trace("routing_input", format_args!("focused={focused} input={event:?} inside_and_guest={projection:?} mapped_key={mapped_key} mapped_primary={mapped_pointer} contact_before={:?}", self.display.contact));
            }
            self.display.game_input(ui, rect, area, true, Locale::PtBr);
            if !routed.is_empty() {
                trace("routing_done", format_args!("direct_contact={:?} held={:?}", self.display.contact, self.display.held));
            }
            self.controls.library_wheel(self.display.take_library_wheel(), ctx.input(|i| i.focused), ctx);
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.display.release();
    }
}

#[test]
#[ignore = "interactive DWM and game-input experiment with an existing visible Windows Qt QA emulator"]
fn play_qt_android_through_dwm() {
    let pid: u32 = std::env::var("RUSTEMUDROID_DWM_PID")
        .unwrap()
        .parse()
        .unwrap();
    let created: u64 = std::env::var("RUSTEMUDROID_DWM_CREATION")
        .unwrap()
        .parse()
        .unwrap();
    let avd = PathBuf::from(std::env::var_os("RUSTEMUDROID_TEST_AVD").unwrap());
    let adb = PathBuf::from(std::env::var_os("RUSTEMUDROID_TEST_ADB").unwrap());
    let registration = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
        .join(format!("Temp/avd/running/pid_{pid}.ini"));
    let data = fs::read_to_string(&registration).unwrap();
    let field = |name: &str| {
        data.lines()
            .filter_map(|line| line.split_once('='))
            .find(|(key, _)| key.trim() == name)
            .unwrap()
            .1
            .trim()
    };
    assert_eq!(
        Path::new(field("avd.dir")).canonicalize().unwrap(),
        avd.canonicalize().unwrap()
    );
    let console: u16 = field("port.serial").parse().unwrap();
    let rpc: u16 = field("grpc.port").parse().unwrap();
    let seconds: u64 = std::env::var("RUSTEMUDROID_DWM_SECONDS")
        .unwrap_or_else(|_| "90".into())
        .parse()
        .unwrap();
    assert!((1..=600).contains(&seconds));
    let output = std::process::Command::new(&adb)
        .args(["-s", &format!("emulator-{console}"), "emu", "avd", "path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let reply = String::from_utf8(output.stdout).unwrap();
    // adb emu can return CR-CR-LF when the Windows console relays the AVD path.
    let path = reply
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .expect("empty AVD path");
    assert_eq!(
        Path::new(path)
            .canonicalize()
            .unwrap_or_else(|error| panic!("could not resolve ADB AVD path {path:?}: {error}")),
        avd.canonicalize().unwrap()
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    // The lab takes one native frame to establish orientation, then runs only input RPCs.
    let (ready, target) = sync::sync_channel(1);
    let selected_avd = avd.clone();
    let worker = std::thread::spawn(move || -> DisplayResult<[usize; 2]> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                let (channel, token) = open_controller(console, rpc, &selected_avd, &rx)
                    .await?
                    .ok_or("input window already closed")?;
                let mut control = Grpc::new(channel).max_decoding_message_size(64 * 1024 * 1024);
                control.ready().await?;
                let sample: tonic::Response<Frame> = control
                    .unary(
                        authorized(
                            Format {
                                format: 1,
                                ..Default::default()
                            },
                            &token,
                        ),
                        PathAndQuery::from_static(
                            "/android.emulation.control.EmulatorController/getScreenshot",
                        ),
                        ProstCodec::default(),
                    )
                    .await?;
                let geometry = sample
                    .into_inner()
                    .format
                    .as_ref()
                    .and_then(adaptive_video::NativeDisplay::conventional)
                    .ok_or("the lab requires a conventional primary display")?;
                ready.send(geometry)?;
                let mut delivered = [0, 0];
                let mut sequence = 0;
                let mut active = BTreeSet::new();
                while let Some(event) = rx.recv().await {
                    sequence += 1;
                    let kind = match &event {
                        Input::Touch(event) => {
                            for touch in &event.touches {
                                let phase =
                                    contact_phase(&mut active, touch.identifier, touch.pressure);
                                trace(
                                    "touch",
                                    format_args!(
                                        "seq={sequence} id={} phase={phase} x={} y={} pressure={}",
                                        touch.identifier, touch.x, touch.y, touch.pressure
                                    ),
                                );
                            }
                            0
                        }
                        Input::Key(key) => {
                            trace(
                                "key",
                                format_args!(
                                    "seq={sequence} key={:?} action={}",
                                    key.key, key.event
                                ),
                            );
                            1
                        }
                        // The lab shows no engine picture to check.
                        Input::Sample(..) => continue,
                    };
                    let reply = tokio::time::timeout(
                        Duration::from_secs(3),
                        deliver(event, &mut control, &token),
                    )
                    .await;
                    let status = match &reply {
                        Ok(Ok(())) => "ok",
                        Ok(Err(_)) => "error",
                        Err(_) => "timeout",
                    };
                    let method = ["sendTouch", "sendKey"][kind];
                    trace(
                        "rpc_reply",
                        format_args!("seq={sequence} method={method} status={status}"),
                    );
                    reply??;
                    delivered[kind] += 1;
                }
                Ok(delivered)
            })
    });
    let geometry = target
        .recv_timeout(Duration::from_secs(15))
        .expect("no Android display response");
    let mut display = Display::new(Default::default(), tx);
    display.size = geometry.size.map(|n| n as usize);
    display.rotation = geometry.rotation;
    let scratch = tempfile::tempdir().unwrap();
    let mappings = scratch.path().join("touch-maps.json");
    if avd.join("touch-maps.json").is_file() {
        fs::copy(avd.join("touch-maps.json"), &mappings).unwrap();
    }
    display.mapping.load(mappings);
    let controls = Controls::new(adb, console, avd);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([800., 450.]),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "RustEmuDroid · DWM input lab",
        options,
        Box::new(move |cc| {
            let RawWindowHandle::Win32(hwnd) = cc.window_handle()?.as_raw() else {
                return Err(io::Error::other("Windows destination required").into());
            };
            let preview = ThumbnailPreview::attach_qt_emulator(hwnd.hwnd.get() as _, pid, created)?;
            let (dest, root, renderer) = preview.lab_windows();
            trace("identity", format_args!("pid={pid} creation_filetime={created} console={console} grpc={rpc} dest_pid={} dest={dest:p} qt_root={root:p} qt_renderer={renderer:p}", std::process::id()));
            Ok(Box::new(Lab {
                preview,
                display,
                controls,
                deadline: Instant::now() + Duration::from_secs(seconds),
                positioned: false,
                last_geometry: None,
                last_focus: None,
                package: None,
            }))
        }),
    )
    .unwrap();
    let [touch, key] = worker.join().unwrap().unwrap();
    let delivered = touch + key;
    assert!(
        delivered > 0,
        "interact with the Android surface during the run"
    );
    println!("dwm_controller_input_rpcs={delivered} geometry_snapshots=1 video_streams=0");
    trace("summary", format_args!("touch={touch} key={key}"));
}

#[test]
fn trace_contact_ids_preserve_interleaved_press_move_release() {
    let mut active = BTreeSet::new();
    assert_eq!(contact_phase(&mut active, 0, 1024), "down");
    assert_eq!(contact_phase(&mut active, 3, 1024), "down");
    assert_eq!(contact_phase(&mut active, 0, 1024), "move");
    assert_eq!(contact_phase(&mut active, 0, 0), "up");
    assert_eq!(contact_phase(&mut active, 0, 0), "orphan_up");
    assert_eq!(contact_phase(&mut active, 3, 0), "up");
    assert!(active.is_empty());
}

#[test]
fn fitted_input_projection_is_stable_across_scale_and_resize() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut display = Display::new(Default::default(), tx);
    display.size = [1600, 900];
    let normal = Rect::from_min_size(Pos2::new(1., 4.), egui::vec2(800., 450.));
    let resized = Rect::from_min_size(Pos2::new(3., 12.), egui::vec2(640., 360.));
    for rect in [normal, resized] {
        assert_eq!(display.coordinates(rect.center(), rect), (800, 450));
        assert_eq!(display.coordinates(rect.right_bottom(), rect), (1599, 899));
        assert_eq!(display.coordinates(rect.left_top(), rect), (0, 0));
    }
}
