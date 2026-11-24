//! Android Emulator's EmulatorController protocol (the subset used by this view).
#[cfg(test)]
mod live_input_test;
mod mapping;
#[cfg(all(test, windows))]
mod preview;

use crate::{capture::Shot, ui::i18n::Locale};
use eframe::egui::{self, ColorImage, Key, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use prost::Message;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tonic::{
    Request,
    client::Grpc,
    codec::ProstCodec,
    codegen::http::uri::PathAndQuery,
    metadata::{Ascii, MetadataValue},
    transport::{Channel, Endpoint},
};

type DisplayResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, PartialEq, Message)]
struct Format {
    #[prost(int32, tag = "1")]
    format: i32,
    #[prost(message, optional, tag = "2")]
    rotation: Option<Rotation>,
    #[prost(uint32, tag = "3")]
    width: u32,
    #[prost(uint32, tag = "4")]
    height: u32,
}
#[derive(Clone, PartialEq, Message)]
struct Rotation {
    #[prost(int32, tag = "1")]
    value: i32,
}
#[derive(Clone, PartialEq, Message)]
struct Frame {
    #[prost(message, optional, tag = "1")]
    format: Option<Format>,
    #[prost(bytes = "bytes", tag = "4")]
    pixels: prost::bytes::Bytes,
}
#[derive(Clone, PartialEq, Message)]
struct Touch {
    #[prost(int32, tag = "1")]
    x: i32,
    #[prost(int32, tag = "2")]
    y: i32,
    #[prost(int32, tag = "3")]
    identifier: i32,
    #[prost(int32, tag = "4")]
    pressure: i32,
}
#[derive(Clone, PartialEq, Message)]
struct TouchEvent {
    #[prost(message, repeated, tag = "1")]
    touches: Vec<Touch>,
}
#[derive(Clone, PartialEq, Message)]
struct Keyboard {
    #[prost(int32, tag = "2")]
    event: i32,
    #[prost(string, tag = "4")]
    key: String,
}
#[derive(Clone, PartialEq, Message)]
struct Empty {}
enum Input {
    Touch(TouchEvent),
    Key(Keyboard),
}

fn credentials(console: u16, rpc: u16, avd: &Path) -> Option<MetadataValue<Ascii>> {
    let roots = if cfg!(windows) {
        vec![PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Temp")]
    } else {
        let mut roots = vec![avd.parent()?.parent()?.join("home")];
        roots.extend(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from));
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(info) = fs::metadata("/proc/self") {
                roots.push(PathBuf::from(format!("/run/user/{}", info.uid())));
            }
        }
        roots
    };
    for entry in roots
        .iter()
        .filter_map(|r| fs::read_dir(r.join("avd/running")).ok())
        .flatten()
        .flatten()
    {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("ini") {
            continue;
        }
        let Ok(data) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let value = |k: &str| {
            data.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(name, _)| name.trim() == k)
                .map(|(_, v)| v.trim())
        };
        if value("port.serial").and_then(|v| v.parse().ok()) == Some(console)
            && value("grpc.port").and_then(|v| v.parse().ok()) == Some(rpc)
            && value("avd.dir").map(Path::new) == Some(avd)
        {
            let mut token: MetadataValue<Ascii> =
                format!("Bearer {}", value("grpc.token")?).parse().ok()?;
            token.set_sensitive(true);
            return Some(token);
        }
    }
    None
}

fn authorized<T>(body: T, token: &MetadataValue<Ascii>) -> Request<T> {
    let mut request = Request::new(body);
    request
        .metadata_mut()
        .insert("authorization", token.clone());
    request
}
async fn deliver(
    input: Input,
    client: &mut Grpc<Channel>,
    token: &MetadataValue<Ascii>,
) -> DisplayResult<()> {
    client.ready().await?;
    let _: tonic::Response<Empty> = match input {
        Input::Touch(event) => {
            client
                .unary(
                    authorized(event, token),
                    PathAndQuery::from_static(
                        "/android.emulation.control.EmulatorController/sendTouch",
                    ),
                    ProstCodec::default(),
                )
                .await?
        }
        Input::Key(k) => {
            client
                .unary(
                    authorized(k, token),
                    PathAndQuery::from_static(
                        "/android.emulation.control.EmulatorController/sendKey",
                    ),
                    ProstCodec::default(),
                )
                .await?
        }
    };
    Ok(())
}

#[derive(Default)]
struct Shared {
    latest: Option<Frame>,
    error: Option<String>,
}

async fn stream(
    console: u16,
    rpc: u16,
    avd: PathBuf,
    ctx: egui::Context,
    state: Arc<Mutex<Shared>>,
    mut input: mpsc::UnboundedReceiver<Input>,
) -> DisplayResult<()> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let token = loop {
        if input.is_closed() {
            return Ok(());
        }
        if let Some(token) = credentials(console, rpc, &avd) {
            break token;
        }
        if Instant::now() >= deadline {
            return Err("O motor não anunciou o canal de vídeo autenticado.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{rpc}"))?
        .initial_stream_window_size(16 * 1024 * 1024)
        .initial_connection_window_size(32 * 1024 * 1024)
        .connect_timeout(Duration::from_secs(5));
    let channel = loop {
        if input.is_closed() {
            return Ok(());
        }
        match endpoint.connect().await {
            Ok(channel) => break channel,
            Err(error) if Instant::now() >= deadline => return Err(error.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    };
    let mut control = Grpc::new(channel.clone());
    let mut video = Grpc::new(channel).max_decoding_message_size(64 * 1024 * 1024);
    video.ready().await?;
    let request = authorized(
        Format {
            format: 1,
            ..Default::default()
        },
        &token,
    );
    let mut frames = video
        .server_streaming(
            request,
            PathAndQuery::from_static(
                "/android.emulation.control.EmulatorController/streamScreenshot",
            ),
            ProstCodec::<Format, Frame>::default(),
        )
        .await?
        .into_inner();
    let inputs = async {
        while let Some(event) = input.recv().await {
            deliver(event, &mut control, &token).await?
        }
        Ok(())
    };
    let video = async {
        while let Some(frame) = frames.message().await? {
            if frame.pixels.is_empty() {
                continue;
            }
            state.lock().unwrap().latest = Some(frame);
            ctx.request_repaint();
        }
        Err("O motor encerrou o fluxo de vídeo.".into())
    };
    tokio::select! { result=inputs=>result, result=video=>result }
}

pub struct Display {
    pub(crate) mapping: mapping::Mapping,
    state: Arc<Mutex<Shared>>,
    input: mpsc::UnboundedSender<Input>,
    texture: Option<TextureHandle>,
    shot: Option<Shot>,
    shot_changed: bool,
    size: [usize; 2],
    rotation: i32,
    contact: Option<(i32, i32)>,
    held: Vec<String>,
    count: u32,
    since: Instant,
    fps: f32,
}
impl Display {
    pub fn open(console: u16, rpc: u16, avd: PathBuf, ctx: &egui::Context) -> Self {
        let (input, rx) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(Shared::default()));
        let shared = state.clone();
        let ctx = ctx.clone();
        let mappings = avd.join("touch-maps.json");
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Into::into)
                .and_then(|rt| {
                    rt.block_on(stream(console, rpc, avd, ctx.clone(), shared.clone(), rx))
                });
            if let Err(e) = result {
                shared.lock().unwrap().error = Some(e.to_string());
                ctx.request_repaint();
            }
        });
        let mut display = Self::new(state, input);
        display.mapping.load(mappings);
        display
    }
    fn new(state: Arc<Mutex<Shared>>, input: mpsc::UnboundedSender<Input>) -> Self {
        Self {
            mapping: mapping::Mapping::default(),
            state,
            input,
            texture: None,
            shot: None,
            shot_changed: false,
            size: [1, 1],
            rotation: 0,
            contact: None,
            held: vec![],
            count: 0,
            since: Instant::now(),
            fps: 0.0,
        }
    }
    pub fn press(&self, key: &str) {
        self.key(key, 2);
    }
    pub fn fps(&self) -> f32 {
        if self.since.elapsed() > Duration::from_secs(2) {
            0.0
        } else {
            self.fps
        }
    }
    pub(crate) fn shot(&self) -> Option<&Shot> {
        self.shot.as_ref()
    }

    pub(crate) fn take_new_shot(&mut self) -> Option<&Shot> {
        std::mem::take(&mut self.shot_changed)
            .then_some(self.shot.as_ref())
            .flatten()
    }
    fn key(&self, key: &str, event: i32) {
        let _ = self.input.send(Input::Key(Keyboard {
            event,
            key: key.into(),
        }));
    }
    fn touch(&self, xy: (i32, i32), pressure: i32) {
        let _ = self.input.send(Input::Touch(TouchEvent {
            touches: vec![Touch {
                x: xy.0,
                y: xy.1,
                identifier: 0,
                pressure,
            }],
        }));
    }
    fn mapped(&self, contacts: Vec<mapping::Contact>) {
        let unit = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let mut touches: Vec<Touch> = vec![];
        for contact in contacts {
            let pos = Pos2::new(contact.pos[0], contact.pos[1]);
            // A repeated ID indicates another phase of the same contact. Distinct
            // fingers from the same phase travel in one event.
            if touches.iter().any(|t| t.identifier == contact.id) {
                let _ = self.input.send(Input::Touch(TouchEvent {
                    touches: std::mem::take(&mut touches),
                }));
            }
            let (x, y) = self.coordinates(pos, unit);
            touches.push(Touch {
                x,
                y,
                identifier: contact.id,
                pressure: if contact.down { 1024 } else { 0 },
            });
        }
        if !touches.is_empty() {
            let _ = self.input.send(Input::Touch(TouchEvent { touches }));
        }
    }
    pub fn set_package(&mut self, package: Option<&str>) {
        if self.mapping.package_changed(package) {
            self.release();
            self.mapping.select(package);
        }
    }
    pub fn toggle_mapping(&mut self) {
        self.release();
        self.mapping.close_quick();
        self.mapping.toggle();
    }
    pub(crate) fn controls_open(&self) -> bool {
        self.mapping.editing || self.mapping.quick.is_open()
    }
    pub(crate) fn toggle_controls_menu(&mut self, anchor: f32) {
        self.release();
        if self.mapping.editing {
            self.mapping.toggle();
        } else {
            self.mapping.toggle_quick(anchor);
        }
    }
    pub(crate) fn release(&mut self) {
        let released = self.mapping.release();
        self.mapped(released);
        if let Some(pos) = self.contact.take() {
            self.touch(pos, 0)
        }
        for key in std::mem::take(&mut self.held) {
            self.key(&key, 1)
        }
    }
    fn upload(&mut self, ctx: &egui::Context, frame: Frame) {
        let Some(format) = frame.format else { return };
        let size = [format.width as usize, format.height as usize];
        if size.contains(&0)
            || size[0].checked_mul(size[1]).and_then(|p| p.checked_mul(4))
                != Some(frame.pixels.len())
        {
            return;
        }
        let same_size = self.size == size;
        let rotation = format.rotation.unwrap_or_default().value;
        if !same_size || self.rotation != rotation {
            self.release();
        }
        self.size = size;
        self.rotation = rotation;
        let mut pixels = vec![egui::Color32::TRANSPARENT; size[0] * size[1]];
        bytemuck::cast_slice_mut(&mut pixels).copy_from_slice(&frame.pixels);
        // Capture the same pixels and orientation sent to the desktop texture.
        self.shot = Some(Shot {
            width: format.width,
            height: format.height,
            pixels: frame.pixels,
        });
        self.shot_changed = true;
        let image = ColorImage::new(size, pixels);
        if let Some(texture) = &mut self.texture {
            if same_size {
                texture.set_partial([0, 0], image, TextureOptions::LINEAR)
            } else {
                texture.set(image, TextureOptions::LINEAR)
            }
        } else {
            self.texture = Some(ctx.load_texture("Android", image, TextureOptions::LINEAR));
        }
        self.count += 1;
        let elapsed = self.since.elapsed();
        if elapsed.as_secs_f32() >= 1.0 {
            self.fps = self.count as f32 / elapsed.as_secs_f32();
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!(
                    "display_frames={} interval_ms={} size={}x{}",
                    self.count,
                    elapsed.as_millis(),
                    size[0],
                    size[1]
                );
            }
            self.count = 0;
            self.since = Instant::now();
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, interactive: bool, locale: Locale) {
        let bounds = ui.available_rect_before_wrap();
        self.mapping.blocked_rect = self.mapping.panel_rect(bounds);
        let menu = self.mapping.quick.is_open();
        let modal_keyboard = self.controls_open()
            && (egui::Popup::is_any_open(ui.ctx()) || ui.input(|i| i.key_pressed(Key::Escape)));
        self.show_game(ui, interactive && !menu && !modal_keyboard, locale);
        if interactive && self.controls_open() {
            self.mapping.floating_panel(ui.ctx(), bounds, locale);
        }
    }

    fn show_game(&mut self, ui: &mut egui::Ui, interactive: bool, locale: Locale) {
        let (frame, error) = {
            let mut shared = self.state.lock().unwrap();
            (shared.latest.take(), shared.error.clone())
        };
        if let Some(frame) = frame {
            self.upload(ui.ctx(), frame)
        }
        if let Some(error) = error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                locale.pick(
                    "Falha na conexão com a tela do Android.",
                    "Couldn't connect to the Android display.",
                    "No se pudo conectar con la pantalla de Android.",
                ),
            )
            .on_hover_text(error);
            return;
        }
        let Some(texture) = &self.texture else {
            ui.spinner();
            ui.label(locale.pick(
                "Conectando à tela do Android…",
                "Connecting to the Android display…",
                "Conectando con la pantalla de Android…",
            ));
            return;
        };
        let size = Vec2::new(self.size[0] as f32, self.size[1] as f32);
        let outer = ui.available_rect_before_wrap();
        // In fullscreen the edge bars cover the guest. Revealing tools never scales its image.
        let viewport = ui.input(|i| {
            if i.viewport().fullscreen == Some(true) {
                i.content_rect()
            } else {
                outer
            }
        });
        let scale = (viewport.width() / size.x).min(viewport.height() / size.y);
        let rect = Rect::from_center_size(viewport.center(), size * scale);
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        if self.mapping.editing {
            ui.painter()
                .rect_filled(rect, 0., egui::Color32::from_black_alpha(83));
        }
        if !interactive {
            if self.mapping.editing {
                self.mapping.paint_canvas(ui, rect);
            } else if self.mapping.quick.preview_hints() {
                self.mapping.paint_hints(ui, rect);
            }
            self.release();
            return;
        }
        let (events, modifiers, focused) = ui.input(|i| (i.events.clone(), i.modifiers, i.focused));
        if !focused
            || events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(false)))
        {
            self.release();
            return;
        }
        let aiming_before = self.mapping.aims_pointer();
        let changes = self.mapping.frame(ui, rect, &events);
        self.mapped(changes);
        if self.mapping.editing {
            return;
        }
        if self.mapping.captures_pointer(egui::PointerButton::Primary)
            && let Some(xy) = self.contact.take()
        {
            self.touch(xy, 0);
        }
        {
            for (key, pressed) in [
                ("Shift", modifiers.shift),
                ("Control", modifiers.ctrl),
                ("Alt", modifiers.alt),
                ("Meta", modifiers.mac_cmd),
            ] {
                let pressed = pressed && !self.mapping.captures_input(key);
                let held = self.held.iter().position(|k| k == key);
                if pressed && held.is_none() {
                    self.key(key, 0);
                    self.held.push(key.into());
                } else if !pressed && let Some(i) = held {
                    self.key(key, 1);
                    self.held.swap_remove(i);
                }
            }
        }
        for event in events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    ..
                } if !self.mapping.captures_pointer(egui::PointerButton::Primary) => {
                    if pressed && rect.intersect(outer).contains(pos) {
                        let xy = self.coordinates(pos, rect);
                        self.touch(xy, 1024);
                        self.contact = Some(xy);
                    } else if !pressed && let Some(xy) = self.contact.take() {
                        self.touch(xy, 0)
                    }
                }
                egui::Event::PointerMoved(pos) if self.contact.is_some() => {
                    let xy = self.coordinates(pos, rect);
                    if Some(xy) != self.contact {
                        self.touch(xy, 1024);
                        self.contact = Some(xy);
                    }
                }
                egui::Event::PointerGone => {
                    if let Some(pos) = self.contact.take() {
                        self.touch(pos, 0);
                    }
                }
                egui::Event::WindowFocused(false) => self.release(),
                egui::Event::Key {
                    key,
                    pressed,
                    repeat: false,
                    ..
                } => {
                    if !(key == Key::Escape && aiming_before)
                        && !self.mapping.captures(key)
                        && let Some(name) = key_name(key)
                    {
                        if pressed {
                            if !self.held.contains(&name) {
                                self.key(&name, 0);
                                self.held.push(name)
                            }
                        } else if let Some(i) = self.held.iter().position(|held| held == &name) {
                            self.held.swap_remove(i);
                            self.key(&name, 1)
                        }
                    }
                }
                _ => {}
            }
        }
    }
    fn coordinates(&self, p: Pos2, rect: Rect) -> (i32, i32) {
        let max_x = self.size[0] as i32 - 1;
        let max_y = self.size[1] as i32 - 1;
        let x = (((p.x - rect.left()) / rect.width()) * self.size[0] as f32) as i32;
        let y = (((p.y - rect.top()) / rect.height()) * self.size[1] as f32) as i32;
        let (x, y) = (x.clamp(0, max_x), y.clamp(0, max_y));
        match self.rotation {
            1 => (max_y - y, x),
            2 => (max_x - x, max_y - y),
            3 => (y, max_x - x),
            _ => (x, y),
        }
    }
}

fn key_name(k: Key) -> Option<String> {
    let key = match k {
        Key::Escape => "GoBack",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::Space => " ",
        Key::Backspace => "Backspace",
        Key::Tab => "Tab",
        Key::Enter => "Enter",
        Key::Delete => "Delete",
        _ if k.name().len() == 1 => return Some(k.name().to_ascii_lowercase()),
        _ => return None,
    };
    Some(key.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput, pos2, vec2};

    #[test]
    #[ignore = "requires a running selected test AVD with RustEmuDroid Home in the foreground"]
    fn integrated_video_and_toolbar_home_key() {
        use std::{process::Command, thread};
        let console = std::env::var("RUSTEMUDROID_TEST_CONSOLE").unwrap();
        let rpc = std::env::var("RUSTEMUDROID_TEST_TOUCH_RPC").unwrap();
        let avd = PathBuf::from(std::env::var_os("RUSTEMUDROID_TEST_AVD").unwrap());
        let adb = std::env::var_os("RUSTEMUDROID_TEST_ADB").unwrap();
        let serial = format!("emulator-{console}");
        let guest = |args: &[&str]| {
            let output = Command::new(&adb)
                .args(["-s", &serial])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(
            Path::new(
                guest(&["emu", "avd", "path"])
                    .lines()
                    .next()
                    .unwrap()
                    .trim()
            ),
            avd
        );
        let foreground = || guest(&["shell", "dumpsys", "activity", "top-resumed"]);
        assert!(foreground().contains("packageName=org.rustemudroid.home "));
        let ctx = egui::Context::default();
        let display = Display::open(console.parse().unwrap(), rpc.parse().unwrap(), avd, &ctx);
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let state = display.state.lock().unwrap();
            assert!(state.error.is_none(), "{:?}", state.error);
            if state.latest.as_ref().is_some_and(|f| {
                f.pixels
                    .chunks_exact(4)
                    .take(300_000)
                    .any(|p| p[0].abs_diff(p[2]) > 30)
            }) {
                break;
            }
            drop(state);
            assert!(Instant::now() < until, "No colored Android frame received");
            thread::sleep(Duration::from_millis(100));
        }
        guest(&[
            "shell",
            "am",
            "start",
            "-W",
            "-a",
            "android.settings.SETTINGS",
        ]);
        assert!(foreground().contains("packageName=com.android.settings "));
        display.press("GoHome");
        let until = Instant::now() + Duration::from_secs(8);
        while !foreground().contains("packageName=org.rustemudroid.home ") {
            assert!(
                Instant::now() < until,
                "The Home button did not return to the game library"
            );
            thread::sleep(Duration::from_millis(120));
        }
    }

    // Run against an existing idle test AVD or entirely in memory.
    #[test]
    fn saves_bindings_in_editor_and_releases_both_fingers_on_focus_loss() {
        use std::{
            process::{Command, Stdio},
            thread,
        };
        let live = std::env::var("RUSTEMUDROID_TEST_TOUCH_RPC").ok();
        let ctx = egui::Context::default();
        let (sender, mut received) = mpsc::unbounded_channel();
        let temp = tempfile::tempdir().unwrap();
        let maps = temp.path().join("touch-maps.json");
        let mut adb_args = None;
        let mut display = if let Some(rpc) = live {
            let console = std::env::var("RUSTEMUDROID_TEST_CONSOLE").unwrap();
            let serial = format!("emulator-{console}");
            let avd = PathBuf::from(std::env::var_os("RUSTEMUDROID_TEST_AVD").unwrap());
            let adb = std::env::var_os("RUSTEMUDROID_TEST_ADB").unwrap();
            let check = Command::new(&adb)
                .args(["-s", &serial, "emu", "avd", "path"])
                .output()
                .unwrap();
            assert_eq!(
                Path::new(
                    String::from_utf8_lossy(&check.stdout)
                        .lines()
                        .next()
                        .unwrap()
                        .trim()
                ),
                avd
            );
            adb_args = Some((adb, serial));
            Display::open(console.parse().unwrap(), rpc.parse().unwrap(), avd, &ctx)
        } else {
            let d = Display::new(Arc::default(), sender);
            d.state.lock().unwrap().latest = Some(Frame {
                format: Some(Format {
                    format: 1,
                    width: 160,
                    height: 90,
                    rotation: None,
                }),
                pixels: vec![255; 160 * 90 * 4].into(),
            });
            d
        };
        display.mapping.load(maps.clone());
        display.set_package(Some("test.game"));
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(960., 640.));
        let tick = |d: &mut Display, events| {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| d.show(ui, true, Locale::default()));
                },
            )
        };
        let until = Instant::now() + Duration::from_secs(25);
        while display.texture.is_none() && Instant::now() < until {
            tick(&mut display, vec![]);
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            display.texture.is_some(),
            "Android display did not produce a frame"
        );
        let click = |d: &mut Display, pos| {
            for pressed in [true, false] {
                tick(
                    d,
                    vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        let label_pos = |d: &mut Display, wanted: &str| {
            let _ = tick(d, vec![]);
            let out = tick(d, vec![]);
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == wanted => {
                        Some(t.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!(
                        "Missing label {wanted:?}; visible: {:?}",
                        out.shapes
                            .iter()
                            .filter_map(|s| match &s.shape {
                                egui::Shape::Text(t) => Some(t.galley.text()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    )
                })
        };
        let key = |key, pressed| Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let image_bounds = |d: &mut Display| {
            let texture = d.texture.as_ref().unwrap().id();
            tick(d, vec![])
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Mesh(m) if m.texture_id == texture => Some(m.calc_bounds()),
                    _ => None,
                })
                .unwrap()
        };
        let original = image_bounds(&mut display);
        display.toggle_controls_menu(145.);
        assert_eq!(original, image_bounds(&mut display));
        let editor = label_pos(&mut display, "Editor de controles");
        click(&mut display, editor);
        assert!(display.mapping.editing);
        assert_eq!(original, image_bounds(&mut display));
        let held = label_pos(&mut display, "Toque mantido");
        click(&mut display, held);
        click(&mut display, pos2(490., 300.));
        tick(&mut display, vec![key(Key::P, true), key(Key::P, false)]);
        let stick = label_pos(&mut display, "Direcional");
        let wheel = pos2(180., 420.);
        let drag = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        tick(
            &mut display,
            vec![Event::PointerMoved(stick), drag(stick, true)],
        );
        tick(
            &mut display,
            vec![Event::PointerMoved(stick - vec2(24., 0.))],
        );
        tick(&mut display, vec![Event::PointerMoved(wheel)]);
        tick(&mut display, vec![drag(wheel, false)]);
        for (direction, new_key) in [("W", Key::I), ("S", Key::K), ("A", Key::J), ("D", Key::L)] {
            let cap = label_pos(&mut display, direction);
            click(&mut display, cap);
            tick(&mut display, vec![key(new_key, true), key(new_key, false)]);
        }
        let save = label_pos(&mut display, "Salvar");
        let _ = tick(&mut display, vec![Event::PointerMoved(save)]);
        assert!(
            display.mapping.blocked_rect.unwrap().contains(save),
            "save at {save:?}, panel {:?}",
            display.mapping.blocked_rect
        );
        click(&mut display, save);
        assert!(
            !display.mapping.editing,
            "save at {save:?} on {:?}, panel {:?}",
            ctx.layer_id_at(save),
            display.mapping.blocked_rect
        );
        assert!(
            [Key::P, Key::I, Key::K, Key::J, Key::L]
                .into_iter()
                .all(|k| display.mapping.captures(k))
        );
        assert!(maps.is_file());
        let capture = adb_args.map(|(adb, serial)| {
            let device = std::env::var("RUSTEMUDROID_TEST_TOUCH_DEVICE").unwrap();
            assert!(
                device
                    .strip_prefix("/dev/input/event")
                    .is_some_and(|n| n.parse::<u32>().is_ok())
            );
            Command::new(adb)
                .args([
                    "-s", &serial, "shell", "timeout", "3", "getevent", "-lt", &device,
                ])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        });
        if capture.is_some() {
            thread::sleep(Duration::from_millis(500));
        }
        tick(&mut display, vec![key(Key::P, true), key(Key::I, true)]);
        if capture.is_some() {
            thread::sleep(Duration::from_millis(150));
        }
        tick(&mut display, vec![Event::WindowFocused(false)]);
        if let Some(capture) = capture {
            let bytes = capture.wait_with_output().unwrap().stdout;
            let log = String::from_utf8_lossy(&bytes);
            let ids: Vec<_> = log
                .lines()
                .filter(|line| line.contains("ABS_MT_TRACKING_ID"))
                .filter_map(|line| line.split_whitespace().last())
                .collect();
            assert_eq!(ids.len(), 4, "{log}");
            assert_ne!(ids[0], ids[1]);
            assert_eq!(&ids[2..], &["ffffffff", "ffffffff"]);
            println!("Android multitouch: {ids:?}");
        } else {
            let mut ids = vec![];
            while let Ok(event) = received.try_recv() {
                let Input::Touch(t) = event else {
                    panic!("binding leaked a keyboard event to Android")
                };
                ids.extend(
                    t.touches
                        .into_iter()
                        .map(|touch| (touch.identifier, touch.pressure)),
                );
            }
            let distinct: std::collections::HashSet<_> = ids
                .iter()
                .filter(|(_, p)| *p > 0)
                .map(|(id, _)| *id)
                .collect();
            assert_eq!(distinct.len(), 2, "{ids:?}");
            assert!(distinct.iter().all(|id| ids.contains(&(*id, 0))));
        }
    }

    #[test]
    fn displays_unaligned_wire_pixels_and_delivers_press_drag_release_and_keys() {
        // A protobuf bytes field can start at any byte offset in an HTTP/2 frame.
        let mut packet = vec![0; 1 + 4 * 16 * 9];
        packet[1..].fill(255);
        let pixels = prost::bytes::Bytes::from(packet).slice(1..);
        let (input, mut received) = mpsc::unbounded_channel();
        let mut display = Display::new(Arc::default(), input);
        display.state.lock().unwrap().latest = Some(Frame {
            format: Some(Format {
                format: 1,
                width: 16,
                height: 9,
                rotation: None,
            }),
            pixels,
        });
        let ctx = egui::Context::default();
        let pointer = |pressed| Event::PointerButton {
            button: PointerButton::Primary,
            pressed,
            pos: pos2(80.0, 45.0),
            modifiers: Modifiers::NONE,
        };
        let key = |pressed| Event::Key {
            key: Key::W,
            physical_key: Some(Key::W),
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(160.0, 90.0))),
            events: vec![
                pointer(true),
                Event::PointerMoved(pos2(120.0, 60.0)),
                pointer(false),
                key(true),
                key(false),
            ],
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| display.show(ui, true, Locale::default()));
        });
        assert_eq!(display.texture.as_ref().unwrap().size(), [16, 9]);
        for (x, y, pressure) in [(8, 4, 1024), (12, 6, 1024), (12, 6, 0)] {
            let Input::Touch(t) = received.try_recv().unwrap() else {
                panic!("expected touch")
            };
            assert_eq!(t.touches.len(), 1);
            let touch = &t.touches[0];
            assert_eq!((touch.x, touch.y, touch.pressure), (x, y, pressure));
        }
        for event in [0, 1] {
            let Input::Key(k) = received.try_recv().unwrap() else {
                panic!("expected key")
            };
            assert_eq!((k.key.as_str(), k.event), ("w", event));
        }
    }
}
