mod chrome;
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

use eframe::egui::{self, Color32, RichText, vec2};

use crate::{
    Engine, Gpu, Result,
    android::{self, BootEvent, BootOptions, Profile, Sdk, SystemImage},
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
const VERSION_LABEL: &str = concat!("Versão ", env!("CARGO_PKG_VERSION"));

pub fn run(sdk: Option<PathBuf>, state: PathBuf, _play: bool, gpu: Option<Gpu>) -> Result<()> {
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
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |creation| {
            theme::install(&creation.egui_ctx);
            let prefs_file = state.join("launcher.json");
            let config = LaunchConfig::load(sdk, state, gpu, &prefs_file);
            let texture =
                creation
                    .egui_ctx
                    .load_texture("RustEmuDroid", brand, egui::TextureOptions::LINEAR);
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
    controls: Option<Controls>,
    help_open: bool,
    playing: bool,
    play: bool,
    status: String,
    failed: bool,
    output: VecDeque<String>,
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
            controls: None,
            help_open: false,
            playing: false,
            play,
            status: "Aguardando início".into(),
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
        let (events, receiver) = mpsc::channel();
        self.failed = false;
        self.home_ready = false;
        self.status = "Preparando Android…".into();
        self.output.clear();

        self.stop = Arc::new(AtomicBool::new(false));
        let stop = self.stop.clone();
        self.playing = !check;
        let worker = thread::spawn(move || {
            let result = (|| -> Result<()> {
                let sdk = Sdk::find((!sdk.is_empty()).then(|| PathBuf::from(sdk)))?;
                let state = std::path::absolute(state)?;
                let needs_init = !state.join("avd/RustEmuDroid.ini").is_file();
                let profile = Profile::new(state);
                if !image.is_empty() {
                    profile.init(SystemImage::from_dir(PathBuf::from(image))?)?
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
                    self.controls = Some(Controls::new(adb, console, avd.clone()));
                    self.display = Some(Display::open(console, rpc, avd, ctx));
                }
                BootEvent::Message(line) => {
                    if line.starts_with("Iniciando ") {
                        self.status = "Iniciando Android…".into();
                    } else if line.starts_with("Boot confirmado ") {
                        self.status = line.clone();
                        if !self.stop.load(Ordering::Acquire)
                            && let Some(controls) = &mut self.controls
                        {
                            controls.send(Action::PrepareHome, ctx);
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
                self.status = if self.status.starts_with("Boot confirmado ") {
                    format!("{} Sessão encerrada.", self.status)
                } else {
                    "Sessão encerrada.".into()
                };
            }
            Err(error) => {
                self.status = error;
                self.failed = true;
            }
        }
    }

    fn show_shutdown(&self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BACKGROUND))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(((ui.available_height() - 165.) * 0.5).max(0.));
                    ui.image((self.brand.id(), vec2(72., 72.)));
                    ui.add_space(14.);
                    ui.heading("Desligando Android…");
                    ui.label(RichText::new("A janela fechará automaticamente.").color(MUTED));
                    ui.add_space(12.);
                    ui.add(egui::Spinner::new().size(22.).color(theme::ACCENT));
                });
            });
        ctx.request_repaint_after(Duration::from_millis(50));
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
            self.display = None;
            self.controls = None;
        }
        self.receive(ctx);
        if self.closing {
            if self.task.is_some() {
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
        self.adapters.poll(ctx);
        self.show_settings(ctx);
        if self.help_open {
            let dialog = egui::Modal::new("help-dialog".into())
                .frame(
                    egui::Frame::popup(&ctx.style())
                        .fill(PANEL)
                        .inner_margin(24)
                        .corner_radius(16),
                )
                .show(ctx, |ui| {
                    ui.set_width(250.0);
                    ui.vertical_centered(|ui| {
                        ui.image((self.brand.id(), vec2(120.0, 120.0)));
                        ui.add_space(8.0);
                        ui.heading("RustEmuDroid");
                        ui.label(RichText::new(VERSION_LABEL).color(MINT));
                        ui.add_space(18.0);
                        if ui.add(theme::button("Fechar")).clicked() {
                            ui.close();
                        }
                    });
                });
            if dialog.should_close() {
                self.help_open = false;
            }
        }
    }
}

impl Launcher {
    fn show_main(&mut self, ctx: &egui::Context) {
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
                            key: egui::Key::F11,
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
            || self.display.as_ref().is_some_and(|d| d.mapping.editing);
        let resizing = chrome::handle_resize(ctx);
        if show_chrome {
            self.title_bar(ctx, fullscreen, resizing);
        }
        if self.playing {
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
                                    self.status.starts_with("Boot confirmado")
                                        && !self.stop.load(Ordering::Acquire),
                                );
                            }
                            if self.home_ready
                                && let Some(display) = &mut self.display
                                && crate::toolbar::Tool::Keymap.button(ui)
                            {
                                display.toggle_mapping();
                            }
                        });
                    });
            }
            if self.home_ready
                && let Some(display) = &mut self.display
                && display.mapping.editing
            {
                egui::TopBottomPanel::bottom("mapping-tools")
                    .show(ctx, |ui| display.mapping.toolbar(ui));
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    if self.home_ready
                        && let Some(display) = &mut self.display
                    {
                        display.show(ui, self.settings.is_none() && !self.help_open && !resizing);
                    } else {
                        self.show_boot(ui, ctx);
                    }
                });
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BACKGROUND))
            .show(ctx, |ui| self.show_boot(ui, ctx));
    }
    fn show_boot(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(8.));
            ui.image((self.brand.id(), vec2(96., 96.)));
            ui.add_space(12.);
            if self.playing {
                if let Some(Err(error)) = self.controls.as_ref().and_then(|c| c.message.as_ref()) {
                    ui.colored_label(ERROR, error);
                    if ui.add(theme::button("Tentar abrir a Home")).clicked()
                        && let Some(c) = &mut self.controls
                    {
                        c.send(Action::PrepareHome, ctx);
                    }
                } else {
                    ui.spinner();
                    ui.label(if self.status.starts_with("Boot confirmado") {
                        "Abrindo a biblioteca de jogos…"
                    } else {
                        &self.status
                    });
                }
            } else {
                ui.label(&self.status);
                if ui
                    .add_enabled(self.task.is_none(), egui::Button::new("Iniciar emulador"))
                    .clicked()
                {
                    self.start(false);
                }
            }
        });
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

fn gpu_label(gpu: Gpu) -> &'static str {
    match gpu {
        Gpu::Auto => "Automática",
        Gpu::Host => "GPU do computador",
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
