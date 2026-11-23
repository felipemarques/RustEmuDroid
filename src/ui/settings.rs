use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use super::{Action, Gpu, Launcher, Profile, Sdk, default_image, gpu_label, path_input, theme};
use crate::android::{Camera, Hardware};
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct LaunchConfig {
    pub sdk: String,
    pub state: String,
    pub image: String,
    pub gpu: Gpu,
    pub hardware: Hardware,
    pub show_fps: bool,
}

impl LaunchConfig {
    fn boot_changed(&self, other: &Self) -> bool {
        self.sdk != other.sdk
            || self.image != other.image
            || self.state != other.state
            || self.gpu != other.gpu
            || self.hardware != other.hardware
    }
    pub fn load(sdk: Option<PathBuf>, state: PathBuf, gpu: Option<Gpu>, file: &Path) -> Self {
        let mut config = fs::File::open(file)
            .ok()
            .and_then(|f| serde_json::from_reader(f).ok())
            .unwrap_or_else(|| Self {
                sdk: Sdk::find(None)
                    .ok()
                    .map(|s| s.root.display().to_string())
                    .unwrap_or_default(),
                image: if state.join("avd/RustEmuDroid.ini").is_file() {
                    String::new()
                } else {
                    default_image()
                },
                state: state.display().to_string(),
                gpu: if cfg!(windows) { Gpu::Host } else { Gpu::Auto },
                hardware: Hardware::default(),
                show_fps: true,
            });
        if let Some(sdk) = sdk {
            config.sdk = sdk.display().to_string();
        }
        if let Some(gpu) = gpu {
            config.gpu = gpu;
        }
        config
    }
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> crate::Result<()> {
    let parent = path.parent().ok_or("Diretório de destino inválido.")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(temp.as_file_mut(), value)?;
    temp.flush()?;
    temp.persist(path)?;
    Ok(())
}

// Called only by the probe process. Loading/unloading a Vulkan ICD in the live
// OpenGL frontend can invalidate state shared by both parts of a graphics driver.
pub(crate) fn print_gpu_inventory() -> crate::Result<()> {
    serde_json::to_writer(io::stdout().lock(), &vulkan_adapters())?;
    Ok(())
}

fn vulkan_adapters() -> Vec<String> {
    // The names come from the same Vulkan API the emulator uses to select a physical device.
    unsafe {
        let Ok(entry) = ash::Entry::load() else {
            return vec![];
        };
        let Ok(instance) = entry.create_instance(&ash::vk::InstanceCreateInfo::default(), None)
        else {
            return vec![];
        };
        let mut names: Vec<_> = instance
            .enumerate_physical_devices()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|device| {
                let properties = instance.get_physical_device_properties(device);
                properties
                    .device_name_as_c_str()
                    .ok()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .collect();
        instance.destroy_instance(None);
        names.sort();
        names.dedup();
        names
    }
}

#[derive(Default)]
pub(super) struct AdapterList {
    pub(super) names: Vec<String>,
    pending: Option<(Child, Instant)>,
    error: bool,
}

impl AdapterList {
    fn request(&mut self) {
        if self.pending.is_some() || !self.names.is_empty() {
            return;
        }
        let exe = std::env::current_exe();
        #[cfg(test)]
        let exe = std::env::var_os("RUSTEMUDROID_TEST_EXE").map_or(exe, |path| Ok(path.into()));
        let child = exe.and_then(|path| {
            let mut cmd = Command::new(path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            cmd.arg("gpu-inventory")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
        });
        self.error = child.is_err();
        self.pending = child.ok().map(|child| (child, Instant::now()));
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        let Some((child, start)) = &mut self.pending else {
            return;
        };
        match child.try_wait() {
            Ok(None) if start.elapsed() < Duration::from_secs(8) => {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Ok(Some(status)) if status.success() => {
                let result = child
                    .stdout
                    .take()
                    .and_then(|out| serde_json::from_reader(out).ok());
                self.error = result.is_none();
                self.names = result.unwrap_or_default();
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                self.error = true;
            }
        }
        self.pending = None;
    }
}

impl Drop for AdapterList {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.pending.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn camera_label(camera: Camera) -> &'static str {
    match camera {
        Camera::Profile => "Padrão da imagem Android",
        Camera::EmulatedFront => "Câmera frontal simulada",
        Camera::WebcamFront => "Webcam do computador · frontal",
        Camera::WebcamBack => "Webcam do computador · traseira",
    }
}

#[derive(Default)]
pub(super) struct ProfileImages {
    sdk: Option<String>,
    original: String,
    google: PathBuf,
    play_ready: bool,
    has_berberis_image: bool,
}

impl ProfileImages {
    fn refresh(&mut self, sdk_path: &str, base: &Path) {
        let sdk = Sdk::find(Some(PathBuf::from(sdk_path))).ok();
        let original_dir = sdk
            .as_ref()
            .and_then(|sdk| Profile::new(base.into()).system_dir(sdk).ok())
            .or_else(|| {
                let path = default_image();
                (!path.is_empty()).then(|| PathBuf::from(path))
            });
        self.has_berberis_image = original_dir
            .as_deref()
            .is_some_and(crate::android::is_kemu_image);
        self.original = fs::read_to_string(base.join("avd/RustEmuDroid.ini"))
            .ok()
            .and_then(|ini| {
                ini.lines()
                    .find_map(|l| l.trim().strip_prefix("target=android-").map(str::to_owned))
            })
            .map(|api| match api.as_str() {
                "36" => "Android 16 · API 36".into(),
                _ => format!("API {api}"),
            })
            .unwrap_or_else(|| "Imagem padrão".into());
        self.google =
            Path::new(sdk_path).join("system-images/android-37.0/google_apis_playstore/x86_64");
        self.play_ready = self.google.join("system.img").is_file();
        self.sdk = Some(sdk_path.to_owned());
    }
}

impl Launcher {
    pub(super) fn open_settings(&mut self) {
        self.adapters.request();
        self.images.sdk = None;
        self.settings = Some(self.config.clone());
    }

    pub(super) fn show_settings(&mut self, ctx: &egui::Context) {
        let Some(draft) = &mut self.settings else {
            return;
        };
        let running = self.playing;
        let mut apply = false;
        let mut android_settings = false;
        let mut check_boot = false;
        let base_state = self.prefs_file.parent().unwrap_or(Path::new("."));
        if self.images.sdk.as_deref() != Some(&draft.sdk) {
            self.images.refresh(&draft.sdk, base_state);
        }
        let frame = egui::Frame::window(&ctx.style()).inner_margin(20);
        let dialog = egui::Modal::new("launcher-settings".into()).frame(frame).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 72.).clamp(240., 580.);
            ui.set_width(width);
            theme::form(ui);
            ui.heading(RichText::new("Configurações do emulador").strong().size(22.));
            ui.add_space(6.);
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 158.).max(120.))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_space(4.);
                    ui.label(RichText::new("Perfil do Android").strong());
                    let google = base_state.join("google-play-37");
                    ui.columns(2, |col| {
                        if theme::profile_card(&mut col[0], "Original", &self.images.original, Path::new(&draft.state) == base_state).clicked() {
                            draft.state = base_state.display().to_string();
                            draft.image = if base_state.join("avd/RustEmuDroid.ini").is_file() {String::new()} else {default_image()};
                        }
                        let desc = if self.images.play_ready {"Google Play Store"} else {"Imagem ausente no SDK"};
                        let pick = col[1].add_enabled_ui(self.images.play_ready, |ui|
                            theme::profile_card(ui, "Android 17", desc, Path::new(&draft.state) == google)).inner;
                        if pick.clicked() {
                            draft.state = google.display().to_string();
                            draft.image = if google.join("avd/RustEmuDroid.ini").is_file() {String::new()} else {self.images.google.display().to_string()};
                        }
                    });
                    theme::label(ui, "Cada perfil tem seus próprios aplicativos e dados.");
                    ui.add_space(7.);
                    ui.label(RichText::new("Gráficos").strong());
                    ui.columns(2, |col| {
                        let label = theme::label(&mut col[0], "Renderização");
                        theme::select(&mut col[0], "render-mode", gpu_label(draft.gpu), |ui| {
                            for gpu in [Gpu::Auto, Gpu::Host, Gpu::Software, Gpu::SwiftShader] {
                                ui.selectable_value(&mut draft.gpu, gpu, gpu_label(gpu));
                            }
                        }).labelled_by(label.id);
                        let label = theme::label(&mut col[1], "Placa de vídeo Vulkan");
                        col[1].add_enabled_ui(matches!(draft.gpu, Gpu::Host | Gpu::Auto), |ui| {
                            let name = if draft.hardware.adapter.is_empty() {"Seleção automática".into()} else {draft.hardware.adapter.clone()};
                            theme::select(ui, "adapter", &name, |ui| {
                                ui.selectable_value(&mut draft.hardware.adapter, String::new(), "Seleção automática");
                                for name in &self.adapters.names {
                                    ui.selectable_value(&mut draft.hardware.adapter, name.clone(), name);
                                }
                            }).labelled_by(label.id);
                        });
                    });
                    if self.adapters.pending.is_some() { ui.weak("Consultando placas de vídeo…"); }
                    else if self.adapters.error { ui.weak("Consulta às placas indisponível. A seleção automática continua disponível."); }
                    ui.checkbox(&mut draft.show_fps, "Mostrar FPS no painel");
                    ui.add_space(6.);
                    ui.label(RichText::new("Câmera").strong());
                    ui.columns(2, |col| {
                        let label = theme::label(&mut col[0], "Origem da imagem");
                        theme::select(&mut col[0], "camera", camera_label(draft.hardware.camera), |ui| {
                            for camera in [Camera::Profile, Camera::EmulatedFront, Camera::WebcamFront, Camera::WebcamBack] {
                                ui.selectable_value(&mut draft.hardware.camera, camera, camera_label(camera));
                            }
                        }).labelled_by(label.id);
                        if matches!(draft.hardware.camera, Camera::WebcamFront | Camera::WebcamBack) {
                            let label = theme::label(&mut col[1], "Índice da webcam (0 = primeira)");
                            col[1].add(egui::DragValue::new(&mut draft.hardware.webcam_index).range(0..=255)).labelled_by(label.id);
                        }
                    });
                    ui.add_space(6.);
                    ui.separator();
                    if self.images.has_berberis_image && Path::new(&draft.state) == base_state {
                        egui::CollapsingHeader::new("Compatibilidade de jogos").show(ui, |ui| {
                            ui.checkbox(&mut draft.hardware.arm_lite, "Tradução ARM leve para PUBG");
                            theme::label(ui, "No Android 16 do Kemu, vale para todos os apps ARM deste perfil e pode alterar o FPS.");
                        });
                    }
                    egui::CollapsingHeader::new("SDK, imagem e perfil").show(ui, |ui| {
                        path_input(ui, "Android SDK", &mut draft.sdk);
                        path_input(ui, "Imagem de sistema", &mut draft.image);
                        path_input(ui, "Diretório do perfil", &mut draft.state);
                    });
                    if !self.output.is_empty() {
                        egui::CollapsingHeader::new("Saída do launcher").show(ui, |ui| {
                            for line in &self.output {
                                ui.monospace(line);
                            }
                        });
                    }
                    if running {
                        android_settings = ui
                            .add_enabled(
                                self.status.starts_with("Boot confirmado")
                                    && self.controls.as_ref().is_some_and(|c| !c.busy()),
                                theme::button("Abrir configurações do Android"),
                            )
                            .clicked();
                    } else if !draft.boot_changed(&self.config) {
                        check_boot = ui
                            .add_enabled(self.task.is_none(), theme::button("Verificar boot"))
                            .clicked();
                    }
                });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if running && draft.boot_changed(&self.config) {"Salvar e reiniciar Android"} else {"Salvar"};
                apply = theme::primary(ui, !draft.state.trim().is_empty() && (self.task.is_none() || running), label).clicked();
                if ui.add(theme::button("Cancelar")).clicked() { ui.close(); }
            });
        });
        if apply {
            let changed = draft.boot_changed(&self.config);
            if let Err(error) = write_json(&self.prefs_file, draft) {
                self.status = format!("Falha ao salvar configurações: {error}");
                self.failed = true;
                self.settings = None;
                return;
            }
            self.config = draft.clone();
            if changed && running {
                self.restart = true;
                self.stop.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        if apply || dialog.should_close() || android_settings || check_boot {
            self.settings = None;
        }
        if android_settings && let Some(c) = &mut self.controls {
            c.send(Action::AndroidSettings, ctx);
        }
        if check_boot {
            self.start(true);
        }
    }
}
