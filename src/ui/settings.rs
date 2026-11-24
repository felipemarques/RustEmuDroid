use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use super::{
    Action, BootStage, Gpu, Launcher, Locale, Profile, Sdk, default_kemu_image, gpu_label,
    path_input, theme,
};
use crate::android::{Camera, Hardware, amd_uma_adapter, amd_uma_sdk_image};
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct LaunchConfig {
    pub sdk: String,
    pub state: String,
    pub image: String,
    pub gpu: Gpu,
    pub hardware: Hardware,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub amd_uma_vulkan_by_profile: BTreeMap<PathBuf, bool>,
    pub show_fps: bool,
    #[serde(default = "super::mascot::enabled_by_default")]
    pub animate_mascot: bool,
    #[serde(default)]
    pub locale: Locale,
}

impl LaunchConfig {
    fn boot_changed(&self, other: &Self) -> bool {
        self.sdk != other.sdk
            || self.image != other.image
            || self.state != other.state
            || self.gpu != other.gpu
            || self.hardware != other.hardware
            || self.amd_uma_vulkan() != other.amd_uma_vulkan()
    }

    fn profile_key(&self) -> Option<PathBuf> {
        if self.state.trim().is_empty() {
            return None;
        }
        std::path::absolute(self.state.trim()).ok()
    }

    pub(super) fn amd_uma_vulkan(&self) -> bool {
        self.profile_key()
            .and_then(|key| self.amd_uma_vulkan_by_profile.get(&key).copied())
            .unwrap_or(false)
    }

    fn set_amd_uma_vulkan(&mut self, enabled: bool) {
        if let Some(key) = self.profile_key() {
            if enabled {
                self.amd_uma_vulkan_by_profile.insert(key, true);
            } else {
                self.amd_uma_vulkan_by_profile.remove(&key);
            }
        }
    }

    pub fn load(sdk: Option<PathBuf>, state: PathBuf, gpu: Option<Gpu>, file: &Path) -> Self {
        let mut config = fs::File::open(file)
            .ok()
            .and_then(|f| serde_json::from_reader(f).ok())
            .unwrap_or_else(|| Self {
                sdk: String::new(),
                image: String::new(),
                state: state.display().to_string(),
                gpu: if cfg!(windows) { Gpu::Host } else { Gpu::Auto },
                hardware: Hardware::default(),
                amd_uma_vulkan_by_profile: BTreeMap::new(),
                show_fps: true,
                animate_mascot: true,
                locale: Locale::default(),
            });
        if let Some(sdk) = sdk {
            config.sdk = sdk.display().to_string();
        } else {
            config.migrate_default_sdk(crate::android::default_sdk_path().as_deref());
        }
        if let Some(gpu) = gpu {
            config.gpu = gpu;
        }
        config
    }

    fn migrate_default_sdk(&mut self, default: Option<&Path>) {
        // Older clients saved the detected system SDK in the same field as a manual override.
        let saved = Path::new(&self.sdk);
        if !self.sdk.is_empty()
            && Some(saved) == default
            && Sdk::find(Some(saved.to_owned())).is_err()
        {
            self.sdk.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_runtime_stays_portable_and_saved_overrides_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let settings = state.join("launcher.json");
        let mut config = LaunchConfig::load(None, state.clone(), None, &settings);
        assert!(config.sdk.is_empty());
        assert!(config.image.is_empty());
        write_json(&settings, &config).unwrap();
        assert!(
            LaunchConfig::load(None, state.clone(), None, &settings)
                .sdk
                .is_empty()
        );

        config.sdk = temp.path().join("older-sdk").display().to_string();
        config.image = temp.path().join("custom-image").display().to_string();
        write_json(&settings, &config).unwrap();
        let saved = LaunchConfig::load(None, state.clone(), None, &settings);
        assert_eq!(saved.sdk, config.sdk);
        assert_eq!(saved.image, config.image);
        let explicit = temp.path().join("another-sdk");
        let selected = LaunchConfig::load(Some(explicit.clone()), state, None, &settings);
        assert_eq!(Path::new(&selected.sdk), explicit);
        assert_eq!(selected.image, config.image);

        let mut migrated = saved;
        migrated.migrate_default_sdk(Some(&temp.path().join("system-sdk")));
        assert_eq!(migrated.sdk, config.sdk);
        migrated.migrate_default_sdk(Some(Path::new(&config.sdk)));
        assert!(migrated.sdk.is_empty());
        assert_eq!(migrated.image, config.image);
    }

    #[test]
    fn amd_uma_vulkan_pref_defaults_off_and_is_scoped_to_the_selected_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut original: LaunchConfig = serde_json::from_value(serde_json::json!({
            "sdk": "",
            "state": dir.path(),
            "image": "",
            "gpu": "Auto",
            "hardware": {
                "adapter": "",
                "camera": "Profile",
                "webcam_index": 0
            },
            "show_fps": true
        }))
        .unwrap();
        let old = original.clone();
        assert!(!original.amd_uma_vulkan());
        original.set_amd_uma_vulkan(true);
        assert!(original.boot_changed(&old));

        let mut saved: LaunchConfig =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        assert!(saved.amd_uma_vulkan());
        let first = saved.state.clone();
        saved.state = dir.path().join("other").display().to_string();
        assert!(!saved.amd_uma_vulkan());
        saved.state = first;
        assert!(saved.amd_uma_vulkan());
        saved.set_amd_uma_vulkan(false);
        assert!(!saved.amd_uma_vulkan());
        assert!(!saved.boot_changed(&old));
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

fn camera_label(camera: Camera, locale: Locale) -> &'static str {
    match camera {
        Camera::Profile => locale.pick(
            "Padrão da imagem Android",
            "Android image default",
            "Predeterminada de la imagen Android",
        ),
        Camera::EmulatedFront => locale.pick(
            "Câmera frontal simulada",
            "Simulated front camera",
            "Cámara frontal simulada",
        ),
        Camera::WebcamFront => locale.pick(
            "Webcam do computador · frontal",
            "Host webcam · front",
            "Cámara web del equipo · frontal",
        ),
        Camera::WebcamBack => locale.pick(
            "Webcam do computador · traseira",
            "Host webcam · rear",
            "Cámara web del equipo · trasera",
        ),
    }
}

#[derive(Default)]
pub(super) struct ProfileImages {
    sdk: Option<String>,
    original: Option<String>,
    google: PathBuf,
    play_ready: bool,
    has_berberis_image: bool,
    amd_uma_google_image: bool,
    amd_uma_google_profile: bool,
    amd_uma_original_profile: bool,
}

impl ProfileImages {
    fn supports_amd_uma(&self, draft: &LaunchConfig, base: &Path, google: &Path) -> bool {
        if !cfg!(windows) || !amd_uma_adapter(draft.gpu, &draft.hardware.adapter) {
            return false;
        }
        let state = Path::new(draft.state.trim());
        if state.as_os_str().is_empty() {
            return false;
        }
        let image = draft.image.trim();
        if !image.is_empty() {
            return self.amd_uma_google_image
                && std::path::absolute(image).is_ok_and(|path| path == self.google);
        }
        (state == google && self.amd_uma_google_profile)
            || (state == base && self.amd_uma_original_profile)
    }

    fn refresh(&mut self, sdk_path: &str, base: &Path) {
        let explicit_sdk = sdk_path.trim();
        let sdk = Sdk::find((!explicit_sdk.is_empty()).then(|| explicit_sdk.into())).ok();
        let original_dir = sdk
            .as_ref()
            .and_then(|sdk| Profile::new(base.into()).system_dir(sdk).ok())
            .or_else(|| sdk.as_ref().and_then(default_kemu_image));
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
            });
        self.google = sdk
            .as_ref()
            .map_or(Path::new(sdk_path), |sdk| sdk.root.as_path())
            .join("system-images/android-37.0/google_apis_playstore/x86_64");
        self.play_ready = self.google.join("system.img").is_file();
        self.amd_uma_google_image = false;
        self.amd_uma_google_profile = false;
        self.amd_uma_original_profile = false;
        if cfg!(windows)
            && let Some(sdk) = &sdk
        {
            self.amd_uma_google_image = amd_uma_sdk_image(sdk, &self.google);
            let profile_supports_amd_uma = |state: &Path| {
                if state.join("avd/RustEmuDroid.ini").is_file() {
                    Profile::new(state.into())
                        .system_dir(sdk)
                        .ok()
                        .is_some_and(|dir| amd_uma_sdk_image(sdk, &dir))
                } else {
                    self.amd_uma_google_image
                }
            };
            self.amd_uma_google_profile = profile_supports_amd_uma(&base.join("google-play-37"));
            self.amd_uma_original_profile = profile_supports_amd_uma(base);
        }
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
        let locale = self.config.locale;
        let running = self.playing;
        let mut apply = false;
        let mut android_settings = false;
        let mut check_boot = false;
        #[cfg(windows)]
        let mut open_bluestacks = false;
        let base_state = self.prefs_file.parent().unwrap_or(Path::new("."));
        if self.images.sdk.as_deref() != Some(&draft.sdk) {
            self.images.refresh(&draft.sdk, base_state);
        }
        let frame = egui::Frame::window(&ctx.style()).inner_margin(20);
        let dialog = egui::Modal::new("launcher-settings".into()).frame(frame).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 72.).clamp(240., 580.);
            ui.set_width(width);
            theme::form(ui);
            ui.heading(RichText::new(locale.pick("Configurações do emulador", "Emulator settings", "Ajustes del emulador")).strong().size(22.));
            ui.add_space(6.);
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 158.).max(120.))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_space(4.);
                    ui.label(RichText::new(locale.pick("Perfil do Android", "Android profile", "Perfil de Android")).strong());
                    let google = base_state.join("google-play-37");
                    ui.columns(2, |col| {
                        if theme::profile_card(&mut col[0], "Original", self.images.original.as_deref().unwrap_or_else(|| locale.pick("Imagem padrão", "Default image", "Imagen predeterminada")), Path::new(&draft.state) == base_state).clicked() {
                            draft.state = base_state.display().to_string();
                            draft.image.clear();
                        }
                        let desc = if self.images.play_ready {"Google Play Store"} else {locale.pick("Imagem não instalada", "Image not installed", "Imagen sin instalar")};
                        let pick = col[1].add_enabled_ui(self.images.play_ready, |ui|
                            theme::profile_card(ui, "Android 17", desc, Path::new(&draft.state) == google)).inner;
                        if pick.clicked() {
                            draft.state = google.display().to_string();
                            draft.image = if google.join("avd/RustEmuDroid.ini").is_file() {String::new()} else {self.images.google.display().to_string()};
                        }
                    });
                    theme::label(ui, locale.pick("Cada perfil tem seus próprios aplicativos e dados.", "Each profile has its own apps and data.", "Cada perfil tiene sus propias aplicaciones y datos."));
                    ui.add_space(7.);
                    let language = theme::label(ui, locale.pick("Idioma", "Language", "Idioma"));
                    theme::select(ui, "launcher-language", draft.locale.name(), |ui| {
                        for language in Locale::ALL {
                            ui.selectable_value(&mut draft.locale, language, language.name());
                        }
                    }).labelled_by(language.id);
                    ui.add_space(7.);
                    ui.label(RichText::new(locale.pick("Gráficos", "Graphics", "Gráficos")).strong());
                    ui.columns(2, |col| {
                        let label = theme::label(&mut col[0], locale.pick("Renderização", "Rendering", "Renderizado"));
                        theme::select(&mut col[0], "render-mode", gpu_label(draft.gpu, locale), |ui| {
                            for gpu in [Gpu::Auto, Gpu::Host, Gpu::Software, Gpu::SwiftShader] {
                                ui.selectable_value(&mut draft.gpu, gpu, gpu_label(gpu, locale));
                            }
                        }).labelled_by(label.id);
                        let label = theme::label(&mut col[1], locale.pick("Placa de vídeo Vulkan", "Vulkan graphics card", "Tarjeta gráfica Vulkan"));
                        col[1].add_enabled_ui(matches!(draft.gpu, Gpu::Host | Gpu::Auto), |ui| {
                            let name = if draft.hardware.adapter.is_empty() {locale.pick("Seleção automática", "Automatic selection", "Selección automática").into()} else {draft.hardware.adapter.clone()};
                            theme::select(ui, "adapter", &name, |ui| {
                                ui.selectable_value(&mut draft.hardware.adapter, String::new(), locale.pick("Seleção automática", "Automatic selection", "Selección automática"));
                                for name in &self.adapters.names {
                                    ui.selectable_value(&mut draft.hardware.adapter, name.clone(), name);
                                }
                            }).labelled_by(label.id);
                        });
                    });
                    if self.adapters.pending.is_some() { ui.weak(locale.pick("Consultando placas de vídeo…", "Searching for graphics cards…", "Buscando tarjetas gráficas…")); }
                    else if self.adapters.error { ui.weak(locale.pick("Consulta às placas indisponível. A seleção automática continua disponível.", "Graphics card detection failed. Automatic selection is still available.", "No se han detectado las tarjetas gráficas. La selección automática sigue disponible.")); }
                    if self.images.supports_amd_uma(draft, base_state, &google) {
                        let mut enabled = draft.amd_uma_vulkan();
                        if ui.checkbox(&mut enabled, locale.pick(
                            "Memória Vulkan compartilhada (Radeon 780M)",
                            "Shared Vulkan memory (Radeon 780M)",
                            "Memoria Vulkan compartida (Radeon 780M)",
                        )).on_hover_text(locale.pick(
                            "Opção experimental que pode corrigir falhas na alocação de memória Vulkan em jogos. Salve e reinicie o Android para aplicar.",
                            "Experimental option that may fix Vulkan memory allocation crashes in games. Save and restart Android to apply.",
                            "Opción experimental que puede corregir fallos de asignación de memoria Vulkan en juegos. Guarda y reinicia Android para aplicar el cambio.",
                        )).changed() {
                            draft.set_amd_uma_vulkan(enabled);
                        }
                    }
                    ui.checkbox(&mut draft.show_fps, locale.pick("Mostrar FPS no painel", "Show FPS in the title bar", "Mostrar FPS en la barra de título"));
                    ui.checkbox(&mut draft.animate_mascot, locale.pick(
                        "Animar o mascote",
                        "Animate the mascot",
                        "Animar la mascota",
                    )).on_hover_text(locale.pick(
                        "Respeita a preferência de movimento reduzido do sistema.",
                        "Respects your system’s reduced-motion setting.",
                        "Respeta la preferencia de movimiento reducido del sistema.",
                    ));
                    ui.add_space(6.);
                    ui.label(RichText::new(locale.pick("Câmera", "Camera", "Cámara")).strong());
                    ui.columns(2, |col| {
                        let label = theme::label(&mut col[0], locale.pick("Origem da imagem", "Camera source", "Fuente de la cámara"));
                        theme::select(&mut col[0], "camera", camera_label(draft.hardware.camera, locale), |ui| {
                            for camera in [Camera::Profile, Camera::EmulatedFront, Camera::WebcamFront, Camera::WebcamBack] {
                                ui.selectable_value(&mut draft.hardware.camera, camera, camera_label(camera, locale));
                            }
                        }).labelled_by(label.id);
                        if matches!(draft.hardware.camera, Camera::WebcamFront | Camera::WebcamBack) {
                            let label = theme::label(&mut col[1], locale.pick("Índice da webcam (0 = primeira)", "Webcam index (0 = first)", "Índice de la cámara web (0 = primera)"));
                            col[1].add(egui::DragValue::new(&mut draft.hardware.webcam_index).range(0..=255)).labelled_by(label.id);
                        }
                    });
                    ui.add_space(6.);
                    ui.separator();
                    if self.images.has_berberis_image && Path::new(&draft.state) == base_state {
                        egui::CollapsingHeader::new(locale.pick("Compatibilidade de jogos", "Game compatibility", "Compatibilidad de juegos")).id_salt("game-compatibility").show(ui, |ui| {
                            ui.checkbox(&mut draft.hardware.arm_lite, locale.pick("Tradução ARM leve para PUBG", "Lightweight ARM translation for PUBG", "Traducción ARM ligera para PUBG"));
                            theme::label(ui, locale.pick("No Android 16 do Kemu, vale para todos os apps ARM deste perfil e pode alterar o FPS.", "On Kemu's Android 16, this applies to all ARM apps in this profile and can affect FPS.", "En Android 16 de Kemu se aplica a todas las aplicaciones ARM de este perfil y puede afectar a los FPS."));
                        });
                    }
                    egui::CollapsingHeader::new(locale.pick("Runtime, imagem e perfil", "Runtime, image, and profile", "Entorno, imagen y perfil")).id_salt("advanced-paths").show(ui, |ui| {
                        path_input(ui, locale.pick("Pasta do runtime (vazio = automático)", "Runtime folder (empty = automatic)", "Carpeta del entorno (vacía = automático)"), &mut draft.sdk);
                        path_input(ui, locale.pick("Imagem de sistema", "System image", "Imagen del sistema"), &mut draft.image);
                        path_input(ui, locale.pick("Diretório do perfil", "Profile folder", "Carpeta del perfil"), &mut draft.state);
                    });
                    if !self.output.is_empty() {
                        egui::CollapsingHeader::new(locale.pick("Saída do launcher", "Launcher output", "Salida del iniciador")).id_salt("launcher-output").show(ui, |ui| {
                            for line in &self.output {
                                ui.monospace(line);
                            }
                        });
                    }
                    #[cfg(windows)]
                    { open_bluestacks = self.blue.entry_requested(ui, locale); }
                    if running {
                        android_settings = ui
                            .add_enabled(
                                self.stage == BootStage::Booted
                                    && self.controls.as_ref().is_some_and(|c| !c.busy()),
                                theme::button(locale.pick("Abrir configurações do Android", "Open Android settings", "Abrir ajustes de Android")),
                            )
                            .clicked();
                    } else if !draft.boot_changed(&self.config) {
                        check_boot = ui
                            .add_enabled(self.task.is_none(), theme::button(locale.pick("Verificar boot", "Verify startup", "Comprobar el inicio")))
                            .clicked();
                    }
                });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if running && draft.boot_changed(&self.config) {locale.pick("Salvar e reiniciar Android", "Save and restart Android", "Guardar y reiniciar Android")} else {locale.pick("Salvar", "Save", "Guardar")};
                apply = theme::primary(ui, !draft.state.trim().is_empty() && (self.task.is_none() || running), label).clicked();
                if ui.add(theme::button(locale.pick("Cancelar", "Cancel", "Cancelar"))).clicked() { ui.close(); }
            });
        });
        if apply {
            let changed = draft.boot_changed(&self.config);
            if let Err(error) = write_json(&self.prefs_file, draft) {
                self.status = error.to_string();
                self.failed = true;
                self.settings = None;
                return;
            }
            let language_changed = self.config.locale != draft.locale;
            self.config = draft.clone();
            if language_changed && let Some(controls) = &mut self.controls {
                controls.set_locale(self.config.locale);
                if !changed && self.stage == BootStage::Booted {
                    controls.send(Action::SetHomeLocale(self.config.locale), ctx);
                }
            }
            if changed && running {
                self.restart = true;
                self.request_stop();
            }
        }
        if apply || dialog.should_close() || android_settings || check_boot {
            self.settings = None;
        }
        #[cfg(windows)]
        if open_bluestacks {
            self.blue.open();
        }
        if android_settings && let Some(c) = &mut self.controls {
            c.send(Action::AndroidSettings, ctx);
        }
        if check_boot {
            self.start(true);
        }
    }
}
