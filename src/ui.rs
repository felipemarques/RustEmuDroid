#[cfg(windows)]
mod bluestacks;
mod capture_panel;
mod chrome;
mod help;
pub(crate) mod i18n;
mod mascot;
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
    time::Duration,
};

use capture_panel::consume_capture_shortcut;
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
    creation
        .egui_ctx
        .load_texture("RustEmuDroid", brand, egui::TextureOptions::LINEAR)
}

#[cfg(windows)]
mod window_icon;

pub fn run(
    sdk: Option<PathBuf>,
    state: PathBuf,
    _play: bool,
    gpu: Option<Gpu>,
    locale: Option<Locale>,
) -> Result<()> {
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
            Ok(Box::new(Launcher::new(texture, config, prefs_file, true)))
        }),
    )?;
    Ok(())
}

fn default_image() -> String {
    android::kemu_install_root()
        .map(|path| path.join("system-img"))
        .filter(|path| path.join("system.img").is_file())
        .map_or_else(String::new, |path| path.display().to_string())
}

struct Launcher {
    brand: egui::TextureHandle,
    config: LaunchConfig,
    prefs_file: PathBuf,
    settings: Option<LaunchConfig>,
    adapters: settings::AdapterList,
    images: settings::ProfileImages,
    restart: bool,
    home_ready: bool,
    task: Option<Session>,
    closing: bool,
    stop: Arc<AtomicBool>,
    display: Option<Display>,
    capture: Capture,
    capture_notice: Option<(CaptureEvent, std::time::Instant)>,
    controls: Option<Controls>,
    help_open: bool,
    help_tab: help::HelpTab,
    #[cfg(windows)]
    blue: bluestacks::BlueStacksPanel,
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
            adapters: Default::default(),
            images: Default::default(),
            restart: false,
            task: None,
            closing: false,
            stop: Arc::new(AtomicBool::new(false)),
            display: None,
            capture: Capture::default(),
            capture_notice: None,
            controls: None,
            help_open: false,
            help_tab: help::HelpTab::About,
            #[cfg(windows)]
            blue: Default::default(),
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
        if self.closing || self.task.is_some() {
            return;
        }
        let sdk = self.config.sdk.trim().to_owned();
        let state = PathBuf::from(self.config.state.trim());
        let image = self.config.image.trim().to_owned();
        let gpu = self.config.gpu;
        let hardware = self.config.hardware.clone();
        let language = self.config.locale;
        let (events, receiver) = mpsc::channel();
        self.failed = false;
        self.home_ready = false;
        self.stage = BootStage::Preparing;
        self.checking_boot = check;
        self.status.clear();
        self.output.clear();

        self.stop = Arc::new(AtomicBool::new(false));
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
                    profile.init(SystemImage::from_dir_localized(
                        PathBuf::from(image),
                        language,
                    )?)?
                } else if needs_init {
                    profile.init(SystemImage::preferred_sdk(&sdk)?)?
                }
                android::boot(
                    &sdk,
                    &profile,
                    BootOptions {
                        gpu,
                        hardware,
                        engine: Engine::Auto,
                        headless: check,
                        check,
                        embedded: !check,
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

    fn receive(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.task else { return };
        let finished = task.worker.is_finished();
        for event in task.events.try_iter() {
            match event {
                BootEvent::Display {
                    console,
                    rpc,
                    avd,
                    adb,
                } => {
                    if self.stop.load(Ordering::Acquire) {
                        continue;
                    }
                    let mut controls = Controls::new(adb, console, avd.clone());
                    controls.set_locale(self.config.locale);
                    self.controls = Some(controls);
                    self.display = Some(Display::open(console, rpc, avd, ctx));
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
        if !finished {
            return;
        }
        // The VM worker owns the child and its cleanup. Join only once it has exited.
        let result =
            self.task.take().unwrap().worker.join().unwrap_or_else(|_| {
                Err("O processo de controle do emulador foi interrompido.".into())
            });
        self.display = None;
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
}

impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.closing {
            self.closing = true;
            self.play = false;
            self.restart = false;
            self.stop.store(true, Ordering::Release);
            self.capture.stop();
            self.display = None;
            self.controls = None;
        }
        self.receive(ctx);
        if self.stop.load(Ordering::Acquire) {
            self.capture.stop();
        }
        while let Some(event) = self.capture.poll() {
            self.capture_notice = Some((event, std::time::Instant::now()));
        }
        if self.capture.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.closing {
            if self.task.is_some() || self.capture.busy() {
                if close_requested {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                self.show_shutdown(ctx);
            } else if !close_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        self.show_main(ctx);
        self.show_capture_notice(ctx);
        self.adapters.poll(ctx);
        if !self.external_dialog_open() {
            self.show_settings(ctx);
        }
        #[cfg(windows)]
        self.blue.show(ctx, self.config.locale);
        if self.help_open {
            self.show_help(ctx);
        }
    }
}

impl Launcher {
    fn show_main(&mut self, ctx: &egui::Context) {
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
        if (self.playing || fullscreen)
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
            })
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        }
        let reveal = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .is_some_and(|p| p.y < 42. || p.x >= i.content_rect().right() - 48.)
        });
        let show_chrome = !fullscreen
            || !self.playing
            || reveal
            || self.settings.is_some()
            || self.help_open
            || self.external_dialog_open()
            || self.display.as_ref().is_some_and(|d| d.controls_open());
        let resizing = chrome::handle_resize(ctx);
        if show_chrome {
            self.title_bar(ctx, fullscreen, resizing);
        }
        if self.playing {
            let tools_interactive = self.home_ready
                && !self.stop.load(Ordering::Acquire)
                && self.settings.is_none()
                && !self.help_open
                && !self.external_dialog_open();
            let capture_interactive = tools_interactive
                && self
                    .display
                    .as_ref()
                    .is_some_and(|display| !display.controls_open());
            if tools_interactive
                && consume_capture_shortcut(ctx, egui::Key::A)
                && let Some(display) = &mut self.display
                && !display.mapping.editing
            {
                display.toggle_mapping();
            }
            if capture_interactive {
                for (key, tool) in crate::toolbar::PLAYER_SHORTCUTS {
                    if tool == Tool::Keymap || !consume_capture_shortcut(ctx, key) {
                        continue;
                    }
                    match tool {
                        Tool::RecordVideo => self.capture_action(self.record_tool()),
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
                                    self.stage == BootStage::Booted
                                        && !self.stop.load(Ordering::Acquire),
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
                                self.show_capture_tools(ui, capture_interactive);
                            }
                        });
                    });
            }
            let interactive = self.settings.is_none()
                && !self.help_open
                && !self.external_dialog_open()
                && !resizing;
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    if self.home_ready
                        && let Some(display) = &mut self.display
                    {
                        display.show(ui, interactive, locale);
                        if let Some(shot) = display.take_new_shot() {
                            self.capture.update(shot);
                        }
                    } else {
                        self.show_boot(ui, ctx);
                    }
                });
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BACKGROUND))
            .show(ctx, |ui| self.show_idle(ui));
    }

    fn external_dialog_open(&self) -> bool {
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
        self.stop.store(true, Ordering::Release);
        self.display.take();
        if let Some(session) = self.task.take() {
            let _ = session.worker.join();
        }
    }
}
