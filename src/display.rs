//! Android Emulator's EmulatorController protocol (the subset used by this view).
mod adaptive_video;
#[cfg(all(test, windows))]
mod dwm_input_lab;
#[cfg(test)]
mod live_input_test;
pub(crate) mod mapping;
mod native_texture;
#[cfg(all(test, windows))]
mod native_texture_test;
#[cfg(all(test, windows))]
mod preview;
#[cfg(test)]
mod profile_shortcut_tests;
mod shared_frames;
mod tilt;

use crate::{capture::Shot, ui::i18n::Locale};
use eframe::egui::{self, ColorImage, Key, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use prost::Message;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
use tonic::{
    Request,
    client::Grpc,
    codec::ProstCodec,
    codegen::http::uri::PathAndQuery,
    metadata::{Ascii, MetadataValue},
    transport::{Channel, Endpoint},
};

pub(crate) const TILT_RESTORE_TIMEOUT: Duration = tilt::RESTORE_TIMEOUT;

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
    #[prost(uint32, tag = "5")]
    display: u32,
    // Folded geometry and a resizable mode disqualify the display from reduction.
    #[prost(message, optional, tag = "7")]
    folded_display: Option<Empty>,
    #[prost(int32, optional, tag = "8")]
    display_mode: Option<i32>,
    // Requests and reports where the pixels travel (inline or a shared file).
    #[prost(message, optional, tag = "6")]
    transport: Option<Transport>,
}
#[derive(Clone, PartialEq, Message)]
struct Transport {
    #[prost(int32, tag = "1")]
    channel: i32,
    #[prost(string, tag = "2")]
    handle: String,
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
    source: adaptive_video::Source,
    blank: bool,
    error: Option<String>,
    video_transport: Option<VideoTransport>,
    video_transport_epoch: u64,
    // Frames with pixels handed to the panel (the shared-memory plan waits for
    // in-band pictures before it tries the file again).
    delivered: u64,
}

/// How the panel's pixels arrive from the emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VideoTransport {
    /// The emulator writes each frame into a file shared with this process.
    SharedMemory,
    /// Each frame travels inside its gRPC message; `fallback` says why the
    /// shared file is not in use when it was wanted.
    Inline { fallback: Option<Fallback> },
}

/// Why the panel continues with inline frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// The Android screen can change size, so no file size bounds every frame.
    ResizableDisplay,
    /// The emulator ignored the request and sent the pixels inline.
    EngineSentInline,
    /// The emulator refused the file: its gRPC status code and message,
    /// shown verbatim as the engine's words.
    EngineRefused { code: String, message: String },
    /// The frame file could not be created or read: the operating system's
    /// message.
    FileError(String),
    /// No picture arrived through the file within this many seconds.
    NoPicture { seconds: u64 },
}

impl VideoTransport {
    pub(crate) fn describe(&self, locale: Locale) -> String {
        let (lead, reason) = match self {
            Self::SharedMemory => {
                return locale
                    .pick(
                        "Vídeo do painel: memória compartilhada com o motor.",
                        "Panel video: memory shared with the engine.",
                        "Vídeo del panel: memoria compartida con el motor.",
                    )
                    .into();
            }
            Self::Inline { fallback: None } => {
                return locale
                    .pick(
                        "Vídeo do painel: quadros dentro do canal de controle.",
                        "Panel video: frames inside the control channel.",
                        "Vídeo del panel: fotogramas dentro del canal de control.",
                    )
                    .into();
            }
            Self::Inline {
                fallback: Some(fallback),
            } => (
                locale.pick(
                    "Vídeo do painel: quadros dentro do canal de controle; a memória compartilhada não foi usada porque",
                    "Panel video: frames inside the control channel; shared memory was not used because",
                    "Vídeo del panel: fotogramas dentro del canal de control; no se usó la memoria compartida porque",
                ),
                match fallback {
                    Fallback::ResizableDisplay => locale
                        .pick(
                            "a tela do Android pode mudar de tamanho.",
                            "the Android screen can change size.",
                            "la pantalla de Android puede cambiar de tamaño.",
                        )
                        .to_string(),
                    Fallback::EngineSentInline => locale
                        .pick(
                            "o motor enviou os quadros dentro das mensagens.",
                            "the engine sent the frames inside the messages.",
                            "el motor envió los fotogramas dentro de los mensajes.",
                        )
                        .to_string(),
                    Fallback::EngineRefused { code, message } => format!(
                        "{} ({code}); {} “{message}”",
                        locale.pick(
                            "o motor recusou o arquivo de quadros",
                            "the engine refused the frame file",
                            "el motor rechazó el archivo de fotogramas",
                        ),
                        locale.pick(
                            "mensagem do motor:",
                            "the engine's message:",
                            "mensaje del motor:",
                        )
                    ),
                    Fallback::FileError(message) => format!(
                        "{} “{message}”",
                        locale.pick(
                            "não foi possível criar ou ler o arquivo de quadros; mensagem do sistema:",
                            "the frame file could not be created or read; the system's message:",
                            "no se pudo crear ni leer el archivo de fotogramas; mensaje del sistema:",
                        )
                    ),
                    Fallback::NoPicture { seconds } => match locale {
                        Locale::PtBr => format!("nenhuma imagem chegou pelo arquivo em {seconds} s."),
                        Locale::EnUs => {
                            format!("no image arrived through the file within {seconds} s.")
                        }
                        Locale::EsEs => {
                            format!("no llegó ninguna imagen por el archivo en {seconds} s.")
                        }
                    },
                },
            ),
        };
        format!("{lead} {reason}")
    }
}

impl Shared {
    fn set_video_transport(&mut self, transport: VideoTransport) {
        if self.video_transport.as_ref() != Some(&transport) {
            if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
                println!("video_transport={transport:?}");
            }
            self.video_transport = Some(transport);
            self.video_transport_epoch += 1;
        }
    }
}

async fn open_controller(
    console: u16,
    rpc: u16,
    avd: &Path,
    input: &mpsc::UnboundedReceiver<Input>,
) -> DisplayResult<Option<(Channel, MetadataValue<Ascii>)>> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let token = loop {
        if input.is_closed() {
            return Ok(None);
        }
        if let Some(token) = credentials(console, rpc, avd) {
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
            return Ok(None);
        }
        match endpoint.connect().await {
            Ok(channel) => break channel,
            Err(error) if Instant::now() >= deadline => return Err(error.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    };
    Ok(Some((channel, token)))
}

async fn stream(
    console: u16,
    rpc: u16,
    avd: PathBuf,
    ctx: egui::Context,
    state: Arc<Mutex<Shared>>,
    mut input: mpsc::UnboundedReceiver<Input>,
    tilt: tilt::Worker,
    video_requests: watch::Receiver<adaptive_video::Request>,
) -> DisplayResult<()> {
    let Some((channel, token)) = open_controller(console, rpc, &avd, &input).await? else {
        return Ok(());
    };
    let mut control = Grpc::new(channel.clone());
    let sensor = channel.clone();
    let video = Grpc::new(channel).max_decoding_message_size(64 * 1024 * 1024);
    let inputs = async {
        while let Some(event) = input.recv().await {
            deliver(event, &mut control, &token).await?
        }
        Ok(())
    };
    let fixed_raster = adaptive_video::fixed_raster(&avd);
    let video = adaptive_video::run(
        video,
        &token,
        &ctx,
        &state,
        video_requests,
        fixed_raster,
        &avd,
    );
    let (cancel_tilt, tilt_shutdown) = tokio::sync::oneshot::channel();
    let session = async {
        let result = tokio::select! { result=inputs=>result, result=video=>result };
        let _ = cancel_tilt.send(());
        result
    };
    let (result, ()) = tokio::join!(session, tilt.run(sensor, token.clone(), tilt_shutdown));
    result
}

pub struct Display {
    pub(crate) mapping: mapping::Mapping,
    state: Arc<Mutex<Shared>>,
    input: mpsc::UnboundedSender<Input>,
    texture: Option<TextureHandle>,
    native_texture: Option<native_texture::NativeTexture>,
    shot: Option<Shot>,
    shot_changed: bool,
    video_preference: Option<adaptive_video::Preference>,
    shot_source: adaptive_video::Source,
    native_display: Option<adaptive_video::NativeDisplay>,
    stream_failed: bool,
    video_transport_seen: u64,
    // The input coordinate space, independent of the Shot's transport raster.
    size: [usize; 2],
    rotation: i32,
    contact: Option<(i32, i32)>,
    library_wheel: i32,
    held: Vec<String>,
    shortcut_modifiers: [bool; 2],
    profile_notice: Option<(String, Instant)>,
    count: u32,
    since: Instant,
    fps: f32,
    tilt: Option<tilt::Tilt>,
}
impl Display {
    pub fn open(console: u16, rpc: u16, avd: PathBuf, ctx: &egui::Context) -> Self {
        let (input, rx) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(Shared::default()));
        let shared = state.clone();
        let ctx = ctx.clone();
        let mappings = avd.join("touch-maps.json");
        let (tilt, tilt_worker) = tilt::Tilt::channel();
        let (video_preference, video_requests) = adaptive_video::Preference::new();
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Into::into)
                .and_then(|rt| {
                    rt.block_on(stream(
                        console,
                        rpc,
                        avd,
                        ctx.clone(),
                        shared.clone(),
                        rx,
                        tilt_worker,
                        video_requests,
                    ))
                });
            if let Err(e) = result {
                shared.lock().unwrap().error = Some(e.to_string());
                ctx.request_repaint();
            }
        });
        let mut display = Self::new(state, input);
        display.tilt = Some(tilt);
        display.video_preference = Some(video_preference);
        display.mapping.load(mappings);
        display
    }
    fn new(state: Arc<Mutex<Shared>>, input: mpsc::UnboundedSender<Input>) -> Self {
        Self {
            mapping: mapping::Mapping::default(),
            state,
            input,
            texture: None,
            native_texture: None,
            shot: None,
            shot_changed: false,
            video_preference: None,
            shot_source: adaptive_video::Source::default(),
            native_display: None,
            stream_failed: false,
            video_transport_seen: 0,
            size: [1, 1],
            rotation: 0,
            contact: None,
            library_wheel: 0,
            held: vec![],
            shortcut_modifiers: [false; 2],
            profile_notice: None,
            count: 0,
            since: Instant::now(),
            fps: 0.0,
            tilt: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn empty_for_capture_wait_test() -> Self {
        let state = Shared {
            blank: true,
            ..Default::default()
        };
        Self::new(Arc::new(Mutex::new(state)), mpsc::unbounded_channel().0)
    }
    pub(crate) fn enable_native_texture(&mut self, gl: Option<&eframe::glow::Context>) {
        if native_texture::supported(gl) {
            self.native_texture = Some(native_texture::NativeTexture::default());
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

    pub(crate) fn set_adaptive_video(&mut self, enabled: bool) {
        if let Some(preference) = &mut self.video_preference {
            preference.set_enabled(enabled);
        }
    }

    /// The video transport, once per change (for the boot details).
    pub(crate) fn take_video_transport_change(&mut self) -> Option<VideoTransport> {
        let (epoch, transport) = {
            let state = self.state.lock().unwrap();
            (state.video_transport_epoch, state.video_transport.clone())
        };
        if epoch == self.video_transport_seen {
            return None;
        }
        self.video_transport_seen = epoch;
        transport
    }

    pub(crate) fn stream_available(&self) -> bool {
        !self.stream_failed
    }

    pub(crate) fn native_shot(&self) -> Option<&Shot> {
        self.shot().filter(|_| {
            if !self.has_current_native_raster() {
                return false;
            }
            let state = self.state.lock().unwrap();
            state.error.is_none()
                && !state.blank
                && state.source.visibility_epoch == self.shot_source.visibility_epoch
        })
    }

    fn has_current_native_raster(&self) -> bool {
        self.video_preference
            .as_ref()
            .is_none_or(|preference| preference.accepts_native(self.shot_source))
    }

    pub(crate) fn rotation(&self) -> i32 {
        self.rotation
    }

    pub(crate) fn take_new_shot(&mut self) -> Option<&Shot> {
        std::mem::take(&mut self.shot_changed)
            .then_some(self.shot.as_ref())
            .flatten()
    }

    pub(crate) fn take_new_native_shot(&mut self) -> Option<&Shot> {
        if self.shot_changed && self.native_shot().is_none() {
            self.shot_changed = false;
            return None;
        }
        self.take_new_shot()
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
        if let Some(tilt) = &mut self.tilt {
            tilt.restore();
        }
        let released = self.mapping.release();
        self.release_contacts(released);
    }

    pub(crate) fn tilt_restored(&self) -> bool {
        self.tilt.as_ref().is_none_or(tilt::Tilt::restored)
    }

    fn release_contacts(&mut self, released: Vec<mapping::Contact>) {
        self.mapped(released);
        if let Some(pos) = self.contact.take() {
            self.touch(pos, 0)
        }
        for key in std::mem::take(&mut self.held) {
            self.key(&key, 1)
        }
    }
    fn upload(&mut self, ctx: &egui::Context, frame: Frame, source: adaptive_video::Source) {
        let Some(format) = frame.format else { return };
        let size = [format.width as usize, format.height as usize];
        if size.contains(&0)
            || size[0].checked_mul(size[1]).and_then(|p| p.checked_mul(4))
                != Some(frame.pixels.len())
        {
            return;
        }
        let same_raster = self
            .shot
            .as_ref()
            .is_some_and(|shot| (shot.width, shot.height) == (format.width, format.height));
        if !source.scaled {
            let rotation = format
                .rotation
                .as_ref()
                .map_or(0, |rotation| rotation.value);
            if self.size != size || self.rotation != rotation {
                // Release held input when the native coordinate space changes.
                let released = self.mapping.release_contacts();
                self.release_contacts(released);
            }
            self.size = size;
            self.rotation = rotation;
            self.native_display = adaptive_video::NativeDisplay::conventional(&format);
        }
        self.shot_source = source;
        // Capture the same pixels and orientation sent to the desktop texture.
        self.shot = Some(Shot {
            width: format.width,
            height: format.height,
            pixels: frame.pixels,
        });
        self.shot_changed = true;
        if let Some(native) = &mut self.native_texture {
            native.advance();
            self.texture.get_or_insert_with(|| {
                let placeholder = ColorImage::filled([1, 1], egui::Color32::TRANSPARENT);
                ctx.load_texture("Android", placeholder, TextureOptions::LINEAR)
            });
        } else {
            let mut pixels = vec![egui::Color32::TRANSPARENT; size[0] * size[1]];
            let shot = self.shot.as_ref().unwrap();
            bytemuck::cast_slice_mut(&mut pixels).copy_from_slice(&shot.pixels);
            let image = ColorImage::new(size, pixels);
            if let Some(texture) = &mut self.texture {
                if same_raster {
                    texture.set_partial([0, 0], image, TextureOptions::LINEAR)
                } else {
                    texture.set(image, TextureOptions::LINEAR)
                }
            } else {
                self.texture = Some(ctx.load_texture("Android", image, TextureOptions::LINEAR));
            }
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
        self.library_wheel = 0;
        let bounds = ui.available_rect_before_wrap();
        self.mapping.blocked_rect = self.mapping.panel_rect(bounds);
        let menu = self.mapping.quick.is_open();
        let modal_keyboard = egui::Popup::is_any_open(ui.ctx())
            || ui.memory(|m| m.top_modal_layer().is_some())
            || (self.controls_open() && ui.input(|i| i.key_pressed(Key::Escape)));
        let key = crate::toolbar::PROFILE_KEY;
        let can_switch = interactive
            && !self.controls_open()
            && ui.input(|i| i.focused && i.key_pressed(key))
            && !modal_keyboard
            && !ui
                .memory(|m| m.focused())
                .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
        if crate::toolbar::consume_player_shortcut(ui.ctx(), key, can_switch) {
            let (changed, message) = self.mapping.cycle_profile(locale);
            if changed {
                self.release();
            }
            self.shortcut_modifiers = [true; 2];
            self.profile_notice = Some((message, Instant::now()));
        }
        let modifiers = ui.input(|i| i.modifiers);
        self.shortcut_modifiers[0] &= modifiers.ctrl;
        self.shortcut_modifiers[1] &= modifiers.shift;
        self.show_game(ui, interactive && !menu && !modal_keyboard, locale);
        if let Some((message, since)) = &self.profile_notice {
            let duration = Duration::from_secs(2);
            if let Some(remaining) = duration.checked_sub(since.elapsed()) {
                if interactive && !self.controls_open() {
                    let painter = ui.painter();
                    let text = painter.layout(
                        message.clone(),
                        egui::FontId::proportional(15.),
                        egui::Color32::WHITE,
                        (bounds.width() - 40.).clamp(1., 480.),
                    );
                    let rect = egui::Align2::CENTER_TOP.anchor_size(
                        bounds.center_top() + egui::vec2(0., 14.),
                        text.size() + egui::vec2(24., 14.),
                    );
                    painter.rect_filled(rect, 7., egui::Color32::from_rgb(28, 36, 49));
                    painter.galley(rect.min + egui::vec2(12., 7.), text, egui::Color32::WHITE);
                }
                ui.ctx().request_repaint_after(remaining);
            } else {
                self.profile_notice = None;
            }
        }
        if interactive && self.controls_open() {
            self.mapping.floating_panel(ui.ctx(), bounds, locale);
        }
    }

    fn show_game(&mut self, ui: &mut egui::Ui, interactive: bool, locale: Locale) {
        let (frame, source, error) = {
            let mut shared = self.state.lock().unwrap();
            (shared.latest.take(), shared.source, shared.error.clone())
        };
        if let Some(frame) = frame {
            self.upload(ui.ctx(), frame, source)
        }
        self.stream_failed = error.is_some();
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
        let (viewport, windowed) = ui.input(|i| {
            let fullscreen = i.viewport().fullscreen == Some(true);
            let viewport = if fullscreen { i.content_rect() } else { outer };
            (
                viewport,
                !fullscreen && i.viewport().minimized != Some(true),
            )
        });
        let scale = (viewport.width() / size.x).min(viewport.height() / size.y);
        let rect = Rect::from_center_size(viewport.center(), size * scale);
        if let Some(preference) = &mut self.video_preference {
            preference.update(
                ui.ctx(),
                rect,
                windowed && ui.is_rect_visible(rect),
                self.native_display,
                self.shot_source,
            );
        }
        if ui.is_rect_visible(rect)
            && let (Some(native), Some(shot)) = (&self.native_texture, &self.shot)
        {
            native.queue(ui.painter(), rect, texture.id(), shot);
        }
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        self.game_input(ui, rect, outer, interactive, locale);
    }

    fn game_input(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        outer: Rect,
        interactive: bool,
        locale: Locale,
    ) {
        if self.mapping.editing {
            ui.painter()
                .rect_filled(rect, 0., egui::Color32::from_black_alpha(83));
        }
        if !interactive {
            if self.mapping.editing {
                self.mapping.paint_canvas(ui, rect);
            } else if self.mapping.quick.preview_hints() {
                self.mapping.paint_hints(ui, rect, locale);
            }
            self.release();
            return;
        }
        let (events, mut modifiers, focused) =
            ui.input(|i| (i.events.clone(), i.modifiers, i.focused));
        modifiers.ctrl &= !self.shortcut_modifiers[0];
        modifiers.shift &= !self.shortcut_modifiers[1];
        if !focused
            || events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(false)))
        {
            self.release();
            return;
        }
        let aiming_before = self.mapping.aims_pointer();
        let changes = self.mapping.frame(ui, rect, &events, modifiers, locale);
        self.mapped(changes);
        if let Some(tilt) = &mut self.tilt {
            tilt.set(self.mapping.tilt_offset());
        }
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
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                } if !modifiers.any()
                    && self.contact.is_none()
                    && !self.mapping.owns_wheel(delta)
                    && ui.input(|i| {
                        !i.pointer.any_down()
                            && i.pointer
                                .hover_pos()
                                .is_some_and(|p| rect.intersect(ui.clip_rect()).contains(p))
                    }) =>
                {
                    // Fractions of the visible list height, in ten-thousandths.
                    let fraction = match unit {
                        egui::MouseWheelUnit::Line => delta.y * 0.12,
                        egui::MouseWheelUnit::Point => delta.y / rect.height().max(1.),
                        egui::MouseWheelUnit::Page => delta.y,
                    };
                    self.library_wheel = (self.library_wheel
                        - (fraction.clamp(-3., 3.) * 10_000.).round() as i32)
                        .clamp(-30_000, 30_000);
                }
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
    pub(crate) fn take_library_wheel(&mut self) -> i32 {
        std::mem::take(&mut self.library_wheel)
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
                    ..Default::default()
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
                ..Default::default()
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
