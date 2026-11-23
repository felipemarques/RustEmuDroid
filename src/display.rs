//! Android Emulator's EmulatorController protocol (the subset used by this view).
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
    Touch(Touch),
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
        Input::Touch(t) => {
            client
                .unary(
                    authorized(TouchEvent { touches: vec![t] }, token),
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
    state: Arc<Mutex<Shared>>,
    input: mpsc::UnboundedSender<Input>,
    texture: Option<TextureHandle>,
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
        Self::new(state, input)
    }
    fn new(state: Arc<Mutex<Shared>>, input: mpsc::UnboundedSender<Input>) -> Self {
        Self {
            state,
            input,
            texture: None,
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
    fn key(&self, key: &str, event: i32) {
        let _ = self.input.send(Input::Key(Keyboard {
            event,
            key: key.into(),
        }));
    }
    fn touch(&self, xy: (i32, i32), pressure: i32) {
        let _ = self.input.send(Input::Touch(Touch {
            x: xy.0,
            y: xy.1,
            pressure,
        }));
    }
    fn release(&mut self) {
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
        self.size = size;
        self.rotation = format.rotation.unwrap_or_default().value;
        let mut pixels = vec![egui::Color32::TRANSPARENT; size[0] * size[1]];
        bytemuck::cast_slice_mut(&mut pixels).copy_from_slice(&frame.pixels);
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
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let (frame, error) = {
            let mut shared = self.state.lock().unwrap();
            (shared.latest.take(), shared.error.clone())
        };
        if let Some(frame) = frame {
            self.upload(ui.ctx(), frame)
        }
        if let Some(error) = error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
            return;
        }
        let Some(texture) = &self.texture else {
            ui.spinner();
            ui.label("Conectando a tela do Android…");
            return;
        };
        let size = Vec2::new(self.size[0] as f32, self.size[1] as f32);
        let outer = ui.available_rect_before_wrap();
        let scale = (outer.width() / size.x).min(outer.height() / size.y);
        let rect = Rect::from_center_size(outer.center(), size * scale);
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        let (events, modifiers, focused) = ui.input(|i| (i.events.clone(), i.modifiers, i.focused));
        if focused {
            for (key, pressed) in [
                ("Shift", modifiers.shift),
                ("Control", modifiers.ctrl),
                ("Alt", modifiers.alt),
                ("Meta", modifiers.mac_cmd),
            ] {
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
                } => {
                    if pressed && rect.contains(pos) {
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
                    if let Some(name) = key_name(key) {
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
                .show(ctx, |ui| display.show(ui));
        });
        assert_eq!(display.texture.as_ref().unwrap().size(), [16, 9]);
        for (x, y, pressure) in [(8, 4, 1024), (12, 6, 1024), (12, 6, 0)] {
            let Input::Touch(t) = received.try_recv().unwrap() else {
                panic!("expected touch")
            };
            assert_eq!((t.x, t.y, t.pressure), (x, y, pressure));
        }
        for event in [0, 1] {
            let Input::Key(k) = received.try_recv().unwrap() else {
                panic!("expected key")
            };
            assert_eq!((k.key.as_str(), k.event), ("w", event));
        }
    }
}
