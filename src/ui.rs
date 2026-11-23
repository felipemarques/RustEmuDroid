use std::{
    collections::VecDeque,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use eframe::egui::{self, Color32, RichText, Stroke, pos2, vec2};

use crate::{Gpu, Result, android::Sdk};

const BACKGROUND: Color32 = Color32::from_rgb(16, 22, 30);
const PANEL: Color32 = Color32::from_rgb(23, 33, 43);
const BORDER: Color32 = Color32::from_rgb(52, 66, 80);
const INK: Color32 = Color32::from_rgb(234, 241, 247);
const MUTED: Color32 = Color32::from_rgb(164, 183, 199);
const MINT: Color32 = Color32::from_rgb(138, 233, 192);
const ERROR: Color32 = Color32::from_rgb(255, 157, 148);

pub fn run(sdk: Option<PathBuf>, state: PathBuf) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 570.0])
            .with_min_inner_size([610.0, 510.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |creation| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = BACKGROUND;
            visuals.extreme_bg_color = BACKGROUND;
            visuals.override_text_color = Some(INK);
            creation.egui_ctx.set_visuals(visuals);
            let sdk_path = sdk.or_else(|| Sdk::find(None).ok().map(|sdk| sdk.root));
            Ok(Box::new(Launcher {
                sdk: sdk_path.map_or_else(String::new, |path| path.display().to_string()),
                state: state.display().to_string(),
                image: default_image(),
                gpu: if cfg!(windows) { Gpu::Host } else { Gpu::Auto },
                task: None,
                status: "Aguardando início".into(),
                failed: false,
                output: VecDeque::new(),
            }))
        }),
    )?;
    Ok(())
}

fn default_image() -> String {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|path| path.join("Kemu/system-img"))
        .filter(|path| path.join("system.img").is_file())
        .map_or_else(String::new, |path| path.display().to_string())
}

struct Launcher {
    sdk: String,
    state: String,
    image: String,
    gpu: Gpu,
    task: Option<Receiver<Event>>,
    status: String,
    failed: bool,
    output: VecDeque<String>,
}

enum Event {
    Output(String),
    Finished(std::result::Result<(), String>),
}

impl Launcher {
    fn start(&mut self, check: bool) {
        if self.task.is_some() {
            return;
        }
        let sdk = self.sdk.trim().to_owned();
        let state = PathBuf::from(self.state.trim());
        let image = self.image.trim().to_owned();
        let gpu = self.gpu;
        let (events, receiver) = mpsc::channel();
        self.task = Some(receiver);
        self.failed = false;
        self.status = "Preparando Android…".into();
        self.output.clear();

        thread::spawn(move || {
            let result = launch(&sdk, &state, &image, gpu, check, &events);
            let _ = events.send(Event::Finished(result.map_err(|error| error.to_string())));
        });
    }

    fn receive(&mut self) {
        let Some(task) = &self.task else { return };
        for event in task.try_iter() {
            match event {
                Event::Output(line) => {
                    if line.starts_with("Iniciando ") {
                        self.status = "Iniciando Android…".into();
                    } else if line.starts_with("Boot confirmado ") {
                        self.status = line.clone();
                    }
                    self.output.push_back(line);
                    if self.output.len() > 30 {
                        self.output.pop_front();
                    }
                }
                Event::Finished(result) => {
                    self.task = None;
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
                    return;
                }
            }
        }
    }
}

impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive();
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(32, 26))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            brand_mark(ui);
                            ui.add_space(12.0);
                            ui.label(RichText::new("RustEmuDroid").strong().size(27.0));
                        });
                        ui.add_space(25.0);

                        egui::Frame::new()
                            .fill(PANEL)
                            .stroke(Stroke::new(1.0_f32, BORDER))
                            .corner_radius(16)
                            .inner_margin(20)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new("Configuração do Android").strong().size(19.0),
                                );
                                ui.add_space(14.0);
                                ui.add_enabled_ui(self.task.is_none(), |ui| {
                                    path_input(ui, "Android SDK", &mut self.sdk);
                                    path_input(ui, "Imagem de sistema", &mut self.image);
                                    path_input(ui, "Diretório do perfil", &mut self.state);

                                    ui.label(RichText::new("Renderização").color(MUTED));
                                    egui::ComboBox::from_id_salt("render-mode")
                                        .selected_text(gpu_label(self.gpu))
                                        .show_ui(ui, |ui| {
                                            for gpu in [
                                                Gpu::Auto,
                                                Gpu::Host,
                                                Gpu::Software,
                                                Gpu::SwiftShader,
                                            ] {
                                                ui.selectable_value(
                                                    &mut self.gpu,
                                                    gpu,
                                                    gpu_label(gpu),
                                                );
                                            }
                                        });
                                });
                                ui.add_space(16.0);
                                ui.horizontal_wrapped(|ui| {
                                    let ready =
                                        self.task.is_none() && !self.state.trim().is_empty();
                                    let start = egui::Button::new(
                                        RichText::new("Abrir Android").color(BACKGROUND).strong(),
                                    )
                                    .fill(MINT)
                                    .min_size(vec2(150.0, 34.0));
                                    if ui.add_enabled(ready, start).clicked() {
                                        self.start(false);
                                    }
                                    if ui
                                        .add_enabled(
                                            ready,
                                            egui::Button::new("Verificar boot")
                                                .min_size(vec2(112.0, 34.0)),
                                        )
                                        .clicked()
                                    {
                                        self.start(true);
                                    }
                                });
                            });

                        ui.add_space(18.0);
                        ui.horizontal_wrapped(|ui| {
                            if self.task.is_some() && !self.status.starts_with("Boot confirmado ") {
                                ui.spinner();
                            }
                            ui.label(RichText::new(&self.status).color(if self.failed {
                                ERROR
                            } else {
                                MINT
                            }));
                        });
                        if !self.output.is_empty() {
                            ui.add_space(8.0);
                            egui::CollapsingHeader::new("Saída do launcher").show(ui, |ui| {
                                for line in &self.output {
                                    ui.label(RichText::new(line).monospace().color(MUTED));
                                }
                            });
                        }
                    });
            });
        });
    }
}

fn path_input(ui: &mut egui::Ui, label: &str, value: &mut String) {
    let label = ui.label(RichText::new(label).color(MUTED));
    ui.add_sized(
        [ui.available_width(), 29.0],
        egui::TextEdit::singleline(value)
            .margin(egui::Margin::symmetric(8, 4))
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

fn brand_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 12, MINT);
    let point = |x, y| pos2(rect.left() + x, rect.top() + y);
    let stroke = Stroke::new(3.0_f32, BACKGROUND);
    painter.add(egui::Shape::line(
        vec![
            point(15.0, 35.0),
            point(15.0, 13.0),
            point(28.0, 13.0),
            point(34.0, 19.0),
            point(28.0, 25.0),
            point(15.0, 25.0),
        ],
        stroke,
    ));
    painter.line_segment([point(24.0, 25.0), point(34.0, 35.0)], stroke);
}

fn launch(
    sdk: &str,
    state: &Path,
    image: &str,
    gpu: Gpu,
    check: bool,
    events: &Sender<Event>,
) -> Result<()> {
    let executable = std::env::current_exe()?;
    let command = || {
        let mut command = Command::new(&executable);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // Hide the launcher console.
        }
        command.arg("--state-dir").arg(state);
        if !sdk.is_empty() {
            command.arg("--sdk").arg(sdk);
        }
        command
    };

    if !image.is_empty() || !state.join("avd/RustEmuDroid.ini").is_file() {
        let mut init = command();
        init.arg("init");
        if !image.is_empty() {
            init.arg("--system-dir").arg(image);
        }
        run_command(init, events)?;
    }

    let mut boot = command();
    boot.args(["boot", "--gpu", gpu.as_str()]);
    if check {
        boot.args(["--headless", "--check"]);
    }
    run_command(boot, events)
}

fn run_command(mut command: Command, events: &Sender<Event>) -> Result<()> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("stderr piped");
    let sender = events.clone();
    let read_errors = thread::spawn(move || stream_output(stderr, &sender));
    stream_output(child.stdout.take().expect("stdout piped"), events);
    let status = child.wait()?;
    let error = read_errors.join().ok().flatten().unwrap_or_default();
    if !status.success() {
        return Err(if error.is_empty() {
            format!("O launcher encerrou com {status}.").into()
        } else {
            error.into()
        });
    }
    Ok(())
}

fn stream_output(input: impl std::io::Read, events: &Sender<Event>) -> Option<String> {
    let mut last = None;
    for line in BufReader::new(input)
        .lines()
        .map_while(std::result::Result::ok)
    {
        last = Some(line.clone());
        let _ = events.send(Event::Output(line));
    }
    last
}
