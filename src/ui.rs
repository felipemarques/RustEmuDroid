#[cfg(windows)]
mod blue_mirror;
#[cfg(windows)]
mod bluestacks;
mod capture_panel;
mod chrome;
mod help;
pub(crate) mod i18n;
mod mascot;
mod runtime_setup;
mod session_panel;
mod settings;
pub(crate) mod theme;
use settings::LaunchConfig;
pub(crate) use settings::print_gpu_inventory;
pub(crate) use settings::write_json;
#[cfg(test)]
mod chrome_tests;
#[cfg(all(test, windows))]
mod live_window_test;

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::toolbar::consume_player_shortcut;
use capture_panel::replay_popup_id;
use eframe::egui::{self, Color32};
use i18n::Locale;

use crate::{
    Engine, Gpu, Result,
    android::{self, BootEvent, BootOptions, Profile, Sdk, SystemImage},
    capture::{Capture, Event as CaptureEvent},
    controls::{Action, Controls, Update},
    display::Display,
    toolbar::Tool,
};

const BACKGROUND: Color32 = Color32::from_rgb(16, 22, 30);
const PANEL: Color32 = Color32::from_rgb(23, 33, 43);
const BORDER: Color32 = Color32::from_rgb(52, 66, 80);
const INK: Color32 = Color32::from_rgb(234, 241, 247);
const MUTED: Color32 = Color32::from_rgb(164, 183, 199);
const MINT: Color32 = Color32::from_rgb(138, 233, 192);
const ERROR: Color32 = Color32::from_rgb(255, 157, 148);
const APP_TITLE: &str = concat!("RustEmuDroid v", env!("CARGO_PKG_VERSION"));
fn version_label(locale: Locale) -> String {
    format!(
        "{} {}",
        locale.pick("Versão", "Version", "Versión"),
        env!("CARGO_PKG_VERSION")
    )
}

fn branded_window() -> Result<(eframe::NativeOptions, egui::ColorImage)> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/rustemudroid.png"))?;
    let brand = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_decorations(false)
            .with_icon(icon)
            .with_inner_size([1080.0, 680.0])
            .with_min_inner_size([610.0, 510.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    Ok((options, brand))
}

fn install_window(
    creation: &eframe::CreationContext<'_>,
    brand: egui::ColorImage,
) -> egui::TextureHandle {
    #[cfg(windows)]
    window_icon::install(creation);
    theme::install(&creation.egui_ctx);
    let texture =
        creation
            .egui_ctx
            .load_texture("RustEmuDroid", brand.clone(), egui::TextureOptions::LINEAR);
    // Windows: the brand may be drawn over the engine's picture.
    #[cfg(windows)]
    crate::display::remember_texture(texture.id(), &brand);
    texture
}

#[cfg(windows)]
mod window_icon;

pub enum Startup {
    Android,
    #[cfg(windows)]
    BlueStacks(String, Option<String>),
}

pub fn run(
    sdk: Option<PathBuf>,
    state: PathBuf,
    startup: Startup,
    gpu: Option<Gpu>,
    locale: Option<Locale>,
) -> Result<()> {
    #[cfg(windows)]
    android::recover_power_overlay();
    let (options, brand) = branded_window()?;
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |creation| {
            let texture = install_window(creation, brand);
            let prefs_file = state.join("launcher.json");
            let mut config = LaunchConfig::load(sdk, state, gpu, &prefs_file);
            if let Some(locale) = locale {
                config.locale = locale;
            }
            let launcher = Launcher::new(
                texture,
                config,
                prefs_file,
                matches!(&startup, Startup::Android),
            );
            #[cfg(windows)]
            let launcher = {
                let mut launcher = launcher;
                if let Startup::BlueStacks(instance, package) = startup {
                    launcher.blue_mirror.queue(instance, package);
                }
                launcher
            };
            Ok(Box::new(launcher))
        }),
    )?;
    Ok(())
}

fn default_kemu_image(sdk: &Sdk) -> Option<PathBuf> {
    if sdk.is_portable() || !cfg!(windows) {
        return None;
    }
    android::kemu_install_root()
        .map(|path| path.join("system-img"))
        .filter(|path| path.join("system.img").is_file())
}

struct Launcher {
    brand: egui::TextureHandle,
    config: LaunchConfig,
    prefs_file: PathBuf,
    settings: Option<LaunchConfig>,
    installer: Option<runtime_setup::Wizard>,
    adapters: settings::AdapterList,
    images: settings::ProfileImages,
    restart: bool,
    home_ready: bool,
    task: Option<Session>,
    closing: bool,
    stop: Arc<AtomicBool>,
    stop_after_tilt: Option<std::time::Instant>,
    display: Option<Display>,
    capture: Capture,
    pending_captures: Vec<Tool>,
    native_frame_wait: Option<std::time::Instant>,
    capture_notice: Option<(CaptureEvent, std::time::Instant)>,
    controls: Option<Controls>,
    help_open: bool,
    help_tab: help::HelpTab,
    #[cfg(windows)]
    blue: bluestacks::BlueStacksPanel,
    #[cfg(windows)]
    blue_mirror: blue_mirror::MirrorPanel,
    playing: bool,
    play: bool,
    stage: BootStage,
    checking_boot: bool,
    status: String,
    failed: bool,
    output: VecDeque<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum BootStage {
    #[default]
    Idle,
    Preparing,
    Starting,
    Booted,
    Verified,
    Stopped,
}

struct Session {
    events: Receiver<BootEvent>,
    worker: thread::JoinHandle<std::result::Result<(), String>>,
}

impl Launcher {
    fn new(
        brand: egui::TextureHandle,
        config: LaunchConfig,
        prefs_file: PathBuf,
        play: bool,
    ) -> Self {
        Self {
            brand,
            config,
            prefs_file,
            home_ready: false,
            settings: None,
            installer: None,
            adapters: Default::default(),
            images: Default::default(),
            restart: false,
            task: None,
            closing: false,
            stop: Arc::new(AtomicBool::new(false)),
            stop_after_tilt: None,
            display: None,
            capture: Capture::default(),
            pending_captures: Vec::new(),
            native_frame_wait: None,
            capture_notice: None,
            controls: None,
            help_open: false,
            help_tab: help::HelpTab::About,
            #[cfg(windows)]
            blue: Default::default(),
            #[cfg(windows)]
            blue_mirror: Default::default(),
            playing: false,
            play,
            stage: BootStage::Idle,
            checking_boot: false,
            status: String::new(),
            failed: false,
            output: VecDeque::new(),
        }
    }

    fn start(&mut self, check: bool) {
        if self.closing || self.task.is_some() || self.installer.is_some() {
            return;
        }
        #[cfg(windows)]
        self.blue_mirror.disconnect();
        let sdk = self.config.sdk.trim().to_owned();
        let state = PathBuf::from(self.config.state.trim());
        let base_state = self
            .prefs_file
            .parent()
            .and_then(|p| std::path::absolute(p).ok());
        let image = self.config.image.trim().to_owned();
        let gpu = self.config.gpu;
        let hardware = self.config.hardware.clone();
        let amd_uma_vulkan = self.config.amd_uma_vulkan();
        let vulkan_cache_runtime = if cfg!(windows) {
            self.config.vulkan_cache_runtime().map(PathBuf::from)
        } else {
            None
        };
        let performance_engine = if cfg!(windows) {
            self.config.performance_engine().cloned()
        } else {
            None
        };
        let performance_power = cfg!(windows) && self.config.performance_power();
        let engine_panel = cfg!(windows) && self.config.panel == settings::PanelChoice::Automatic;
        let language = self.config.locale;
        let (events, receiver) = mpsc::channel();
        self.failed = false;
        self.home_ready = false;
        self.stage = BootStage::Preparing;
        self.checking_boot = check;
        self.status.clear();
        self.output.clear();

        self.stop = Arc::new(AtomicBool::new(false));
        self.stop_after_tilt = None;
        let stop = self.stop.clone();
        self.playing = !check;
        let worker = thread::spawn(move || {
            let result = (|| -> Result<()> {
                let sdk =
                    Sdk::find_localized((!sdk.is_empty()).then(|| PathBuf::from(sdk)), language)?;
                let state = std::path::absolute(state)?;
                let needs_init = !state.join("avd/RustEmuDroid.ini").is_file();
                let profile = Profile::localized(state, language);
                if !image.is_empty() {
                    let path = PathBuf::from(image);
                    sdk.require_matching_image(&path)?;
                    let matches = !needs_init
                        && profile
                            .system_dir(&sdk)
                            .ok()
                            .and_then(|current| current.canonicalize().ok())
                            .is_some_and(|current| {
                                path.canonicalize()
                                    .is_ok_and(|selected| selected == current)
                            });
                    if !matches {
                        profile.init(SystemImage::from_dir_localized(path, language)?)?
                    }
                } else if needs_init {
                    let image = runtime_setup::initial_image(
                        &sdk,
                        base_state.as_ref() == Some(&profile.root),
                        language,
                    )?;
                    profile.init(image)?
                }
                android::boot(
                    &sdk,
                    &profile,
                    BootOptions {
                        gpu,
                        hardware,
                        amd_uma_vulkan,
                        vulkan_cache_runtime,
                        performance_engine,
                        performance_power,
                        engine: Engine::Auto,
                        headless: check,
                        check,
                        embedded: !check,
                        engine_panel,
                        timeout: Duration::from_secs(180),
                    },
                    &stop,
                    |e| {
                        println!("{e}");
                        let _ = events.send(e);
                    },
                )
            })();
            result.map_err(|error| error.to_string())
        });
        self.task = Some(Session {
            events: receiver,
            worker,
        });
    }

    fn receive(&mut self, ctx: &egui::Context, gl: Option<&eframe::glow::Context>) {
        let Some(task) = &self.task else { return };
        let finished = task.worker.is_finished();
        for event in task.events.try_iter() {
            match event {
                BootEvent::Display {
                    console,
                    rpc,
                    avd,
                    adb,
                    engine_window,
                } => {
                    if self.stop.load(Ordering::Acquire) {
                        continue;
                    }
                    let mut controls = Controls::new(adb, console, avd.clone());
                    controls.set_locale(self.config.locale);
                    self.controls = Some(controls);
                    let mut display = Display::open(console, rpc, avd, ctx);
                    display.enable_native_texture(gl);
                    #[cfg(windows)]
                    if let Some(launcher) = engine_window {
                        display.watch_engine_window(launcher);
                    }
                    #[cfg(not(windows))]
                    let _ = engine_window;
                    self.display = Some(display);
                }
                event => {
                    let line = event.to_string();
                    if matches!(event, BootEvent::Starting(_)) {
                        self.stage = BootStage::Starting;
                    } else if matches!(event, BootEvent::Ready(_)) {
                        self.stage = BootStage::Booted;
                        self.status = line.clone();
                        if !self.stop.load(Ordering::Acquire)
                            && let Some(controls) = &mut self.controls
                        {
                            controls.send(Action::PrepareHome(self.config.locale), ctx);
                        }
                    }
                    self.output.push_back(line);
                    if self.output.len() > 30 {
                        self.output.pop_front();
                    }
                }
            }
        }
        // The panel's video transport goes to the boot details, once per change.
        if let Some(transport) = self
            .display
            .as_mut()
            .and_then(Display::take_video_transport_change)
        {
            self.output
                .push_back(transport.describe(self.config.locale));
            if self.output.len() > 30 {
                self.output.pop_front();
            }
        }
        // So does what the panel shows: the engine's own window or copied frames.
        if let Some(path) = self
            .display
            .as_mut()
            .and_then(Display::take_panel_path_change)
        {
            let line = path.describe(self.config.locale);
            println!("{line}");
            self.output.push_back(line);
            if self.output.len() > 30 {
                self.output.pop_front();
            }
        }
        if !finished {
            return;
        }
        // The VM worker owns the child and its cleanup. Join only once it has exited.
        let result =
            self.task.take().unwrap().worker.join().unwrap_or_else(|_| {
                Err("O processo de controle do emulador foi interrompido.".into())
            });
        self.stop_after_tilt = None;
        self.display = None;
        self.pending_captures.clear();
        self.native_frame_wait = None;
        self.set_replay_armed(false);
        self.capture.stop();
        self.controls = None;
        self.home_ready = false;
        self.playing = false;
        self.play = std::mem::take(&mut self.restart) && !self.closing && result.is_ok();
        if self.play {
            ctx.request_repaint();
        }
        if !self.closing && ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        match result {
            Ok(()) => {
                self.stage = if self.checking_boot && self.stage == BootStage::Booted {
                    BootStage::Verified
                } else {
                    BootStage::Stopped
                };
            }
            Err(error) => {
                self.status = error;
                self.failed = true;
            }
        }
    }

    fn stopping(&self) -> bool {
        self.stop_after_tilt.is_some() || self.stop.load(Ordering::Acquire)
    }

    fn request_stop(&mut self) {
        self.pending_captures.clear();
        self.native_frame_wait = None;
        if self.stopping() {
            return;
        }
        self.set_replay_armed(false);
        if let Some(display) = &mut self.display {
            display.release();
            if !display.tilt_restored() {
                self.stop_after_tilt =
                    Some(std::time::Instant::now() + crate::display::TILT_RESTORE_TIMEOUT);
                return;
            }
        }
        self.stop.store(true, Ordering::Release);
    }

    fn poll_stop(&mut self, ctx: &egui::Context) {
        if let Some(deadline) = self.stop_after_tilt {
            if self.display.as_ref().is_none_or(Display::tilt_restored)
                || std::time::Instant::now() >= deadline
            {
                self.stop_after_tilt = None;
                self.stop.store(true, Ordering::Release);
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
    }
}

/// `RUSTEMUDROID_FRAME_STATS`: how many input events (and pointer button
/// events among them) reached the client, at most four lines a second, for
/// tests of its responsiveness.
fn trace_input(ctx: &egui::Context) {
    use std::cell::Cell;
    thread_local! {
        static SINCE: Cell<(Option<Instant>, usize, usize)> = const { Cell::new((None, 0, 0)) };
    }
    if std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_none() {
        return;
    }
    let (events, buttons) = ctx.input(|input| {
        let buttons = input
            .events
            .iter()
            .filter(|event| matches!(event, egui::Event::PointerButton { .. }))
            .count();
        (input.events.len(), buttons)
    });
    SINCE.with(|since| {
        let (last, count, pressed) = since.get();
        let (count, pressed) = (count + events, pressed + buttons);
        let now = Instant::now();
        if count > 0
            && last.is_none_or(|last| now.duration_since(last) >= Duration::from_millis(250))
        {
            println!(
                "client_input at_ms={} events={count} buttons={pressed}",
                crate::display::trace_clock_ms()
            );
            since.set((Some(now), 0, 0));
        } else {
            since.set((last, count, pressed));
        }
    });
}

impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        trace_input(ctx);
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.closing {
            self.closing = true;
            #[cfg(windows)]
            {
                self.blue_mirror.cancel_apk();
                self.blue_mirror.hide();
            }
            self.play = false;
            self.restart = false;
            self.request_stop();
            self.capture.stop();
            self.controls = None;
            if let Some(wizard) = &self.installer {
                wizard.cancel();
            }
        }
        if let Some(wizard) = &mut self.installer {
            wizard.poll();
        }
        self.poll_stop(ctx);
        self.receive(ctx, frame.gl().map(|gl| gl.as_ref()));
        if self.stop.load(Ordering::Acquire) {
            self.capture.stop();
            if self.closing {
                self.display = None;
            }
        }
        while let Some(event) = self.capture.poll() {
            self.capture_notice = Some((event, std::time::Instant::now()));
        }
        if self.capture.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        #[cfg(windows)]
        let disconnecting = {
            if self.closing && !self.capture.busy() && self.blue_mirror.active() {
                self.blue_mirror.disconnect();
            }
            self.blue_mirror.poll_cleanup(ctx)
        };
        #[cfg(not(windows))]
        let disconnecting = false;
        if self.closing {
            if disconnecting
                || self.task.is_some()
                || self.capture.busy()
                || self.installer.as_ref().is_some_and(|w| w.installing())
            {
                if close_requested {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                self.show_shutdown(ctx, disconnecting);
            } else if !close_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        #[cfg(windows)]
        self.blue_mirror.update(
            ctx,
            frame,
            self.task.is_none(),
            &self.config.sdk,
            self.prefs_file
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        );
        self.show_main(ctx, frame);
        self.show_capture_notice(ctx);
        self.adapters.poll(ctx);
        if !self.external_dialog_open() {
            self.show_settings(ctx, Some(frame));
        }
        #[cfg(windows)]
        if let Some((instance, package)) = self.blue.show(ctx, self.config.locale) {
            self.settings = None;
            self.play = false;
            self.restart = false;
            if self.task.is_some() {
                self.request_stop();
            }
            self.blue_mirror.queue(instance, package);
            ctx.request_repaint();
        }
        if self.help_open {
            self.show_help(ctx);
        }
        self.show_runtime_setup(ctx);
        // Last: whatever this frame drew over the panel stays visible over
        // an engine surface held by the client's window.
        if let Some(display) = &mut self.display {
            display.end_frame(ctx);
        }
    }
}

impl Launcher {
    fn has_player_view(&self) -> bool {
        #[cfg(windows)]
        if self.blue_mirror.active() {
            return true;
        }
        self.playing
    }

    fn show_main(&mut self, ctx: &egui::Context, _frame: &eframe::Frame) {
        let locale = self.config.locale;
        if std::mem::take(&mut self.play) {
            self.start(false)
        }
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if let Some(controls) = &mut self.controls
            && let Some(Update::HomeReady) = controls.poll(ctx)
        {
            self.home_ready = true;
        }
        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if self.home_ready
            && let (Some(controls), Some(display)) = (&mut self.controls, &mut self.display)
            && display.mapping.wants_package()
        {
            display.set_package(controls.foreground_package(ctx));
        }
        // A panel that shows the engine's own picture processes no frames to count.
        let through = self.display.as_ref().is_some_and(Display::frames_bypassed);
        let android_fps = self.config.show_fps && (self.config.android_fps || through);
        let native_fps = android_fps && self.playing && self.home_ready && !self.stopping();
        if let Some(controls) = &mut self.controls {
            controls.update_surface_fps(native_fps, ctx);
        }
        #[cfg(windows)]
        self.blue_mirror.update_surface_fps(
            self.config.show_fps && self.config.android_fps && self.blue_mirror.active(),
            ctx,
        );
        let toggling_fullscreen = (self.has_player_view() || fullscreen)
            && ctx.input(|i| {
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key {
                            key: crate::toolbar::FULLSCREEN_KEY,
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )
                })
            });
        if toggling_fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        }
        let reveal = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .is_some_and(|p| p.y < 42. || p.x >= i.content_rect().right() - 48.)
        });
        let replay_menu_was_open = egui::Popup::is_id_open(ctx, replay_popup_id());
        let show_chrome = !fullscreen
            || !self.has_player_view()
            || reveal
            || self.settings.is_some()
            || self.help_open
            || self.external_dialog_open()
            || replay_menu_was_open
            || self.display.as_ref().is_some_and(|d| d.controls_open());
        #[cfg(windows)]
        let show_chrome = if self.blue_mirror.active()
            && self.settings.is_none()
            && !self.help_open
            && !self.external_dialog_open()
        {
            self.blue_mirror.controls_chrome().unwrap_or(show_chrome)
        } else {
            show_chrome
        };
        let resizing = chrome::handle_resize(ctx);
        if show_chrome {
            self.title_bar(ctx, fullscreen, resizing);
        }
        #[cfg(windows)]
        if self.blue_mirror.active() {
            let obscured = self.settings.is_some() || self.help_open || self.external_dialog_open();
            let notice = capture_panel::result_notice(self.capture_notice.as_ref(), locale, ctx);
            if let Some(tool) = self.blue_mirror.show(
                ctx,
                locale,
                obscured,
                show_chrome,
                resizing || toggling_fullscreen,
                notice,
            ) {
                if tool == Tool::Install {
                    self.blue_mirror.choose_apk(ctx, _frame, locale);
                } else {
                    self.capture_action(tool);
                }
            }
            return;
        }
        if self.playing {
            let stopping = self.stopping();
            let tools_interactive = self.home_ready
                && !stopping
                && self.settings.is_none()
                && !self.help_open
                && !self.external_dialog_open();
            let capture_interactive = tools_interactive
                && self
                    .display
                    .as_ref()
                    .is_some_and(|display| !display.controls_open());
            if tools_interactive
                && consume_player_shortcut(ctx, egui::Key::A, true)
                && let Some(display) = &mut self.display
                && !display.mapping.editing
            {
                display.toggle_mapping();
            }
            let mut save_replay = false;
            if capture_interactive {
                for (key, tool) in crate::toolbar::PLAYER_SHORTCUTS {
                    if tool == Tool::Keymap || !consume_player_shortcut(ctx, key, true) {
                        continue;
                    }
                    match tool {
                        Tool::RecordVideo => self.capture_action(self.record_tool()),
                        Tool::SaveReplay => save_replay = true,
                        other => self.capture_action(other),
                    }
                }
            }
            if show_chrome {
                egui::SidePanel::right("android-tools")
                    .resizable(false)
                    .exact_width(46.)
                    .frame(egui::Frame::new().fill(PANEL).inner_margin(5))
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            if let Some(controls) = &mut self.controls {
                                crate::toolbar::session_tools(
                                    ui,
                                    controls,
                                    self.stage == BootStage::Booted && !stopping,
                                    locale,
                                );
                            }
                            if self.home_ready
                                && let Some(display) = &mut self.display
                            {
                                let anchor = ui.next_widget_position().y;
                                if crate::toolbar::Tool::Keymap.button(ui, locale) {
                                    display.toggle_controls_menu(anchor);
                                }
                            }
                            if self.home_ready {
                                save_replay |= self.show_capture_tools(ui, capture_interactive);
                            }
                        });
                    });
            }
            let interactive = self.settings.is_none()
                && !self.help_open
                && !self.external_dialog_open()
                && !replay_menu_was_open
                && !egui::Popup::is_id_open(ctx, replay_popup_id())
                && !self.stopping()
                && !resizing;
            let capturing = !self.pending_captures.is_empty()
                || self.capture.mode().is_some()
                || self.capture.replay_armed();
            let adapt_video = self.config.adaptive_video && !capturing;
            let turned = self.controls.as_mut().is_some_and(Controls::take_rotation);
            #[cfg(windows)]
            if let Some(display) = &mut self.display {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = _frame.window_handle()
                    && let RawWindowHandle::Win32(handle) = handle.as_raw()
                {
                    display.set_host_window(handle.hwnd.get());
                }
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    if self.home_ready
                        && let Some(display) = &mut self.display
                    {
                        let rotation = display.rotation();
                        display.set_adaptive_video(adapt_video);
                        display.set_frames_wanted(capturing);
                        if turned {
                            display.refresh_geometry();
                        }
                        display.show(ui, interactive, locale);
                        if let Some(controls) = &mut self.controls {
                            controls.library_wheel(
                                display.take_library_wheel(),
                                interactive && !display.controls_open() && ctx.input(|i| i.focused),
                                ctx,
                            );
                        }
                        if rotation != display.rotation() {
                            self.capture.reset_replay();
                        }
                        if self.capture.mode().is_some()
                            && let Some(shot) = display.take_new_native_shot()
                        {
                            self.capture.update(shot);
                        }
                    } else {
                        self.show_boot(ui, ctx);
                    }
                });
            self.poll_native_captures(ctx);
            self.sample_replay(ctx);
            if save_replay {
                self.capture_action(Tool::SaveReplay);
            }
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BACKGROUND))
            .show(ctx, |ui| self.show_idle(ui));
    }

    fn external_dialog_open(&self) -> bool {
        if self.installer.is_some() {
            return true;
        }
        #[cfg(windows)]
        return self.blue.is_open();
        #[cfg(not(windows))]
        false
    }
}

fn path_input(ui: &mut egui::Ui, label: &str, value: &mut String) {
    let label = theme::label(ui, label);
    ui.add_sized(
        [ui.available_width(), 34.0],
        egui::TextEdit::singleline(value)
            .margin(egui::Margin::symmetric(10, 7))
            .font(egui::TextStyle::Monospace),
    )
    .labelled_by(label.id);
    ui.add_space(9.0);
}

fn gpu_label(gpu: Gpu, locale: Locale) -> &'static str {
    match gpu {
        Gpu::Auto => locale.pick("Automática", "Automatic", "Automática"),
        Gpu::Host => locale.pick("GPU do computador", "Host GPU", "GPU del equipo"),
        Gpu::Software => "Software",
        Gpu::SwiftShader => "SwiftShader",
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.request_stop();
        while let Some(deadline) = self.stop_after_tilt {
            if self.display.as_ref().is_none_or(Display::tilt_restored)
                || std::time::Instant::now() >= deadline
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.stop.store(true, Ordering::Release);
        self.display.take();
        if let Some(session) = self.task.take() {
            let _ = session.worker.join();
        }
    }
}
