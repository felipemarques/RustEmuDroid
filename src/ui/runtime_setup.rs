use super::{ERROR, LaunchConfig, Launcher, Locale, Profile, Sdk, theme, write_json};
use crate::runtime_install::{self, ImageChoice, InstallPlan, Progress};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) fn install_label(l: Locale) -> &'static str {
    l.pick(
        "Instalar runtime ou imagem Android",
        "Install Android runtime or image",
        "Instalar entorno o imagen de Android",
    )
}

pub(super) const ORIGINAL_ID: &str = ImageChoice::Original.package();
pub(super) const GOOGLE_ID: &str = ImageChoice::GooglePlay.package();

pub(super) fn profile_dirs(base: &Path, choice: ImageChoice) -> [PathBuf; 2] {
    match choice {
        ImageChoice::Original => [base.into(), base.join("portable-original")],
        ImageChoice::GooglePlay => [
            base.join("google-play-37"),
            base.join("portable-google-play"),
        ],
    }
}

pub(super) fn image_path(root: &Path, id: &str) -> PathBuf {
    id.split(';').fold(root.to_path_buf(), |p, s| p.join(s))
}

pub(super) fn managed(sdk: &Sdk) -> bool {
    sdk.is_portable()
        && runtime_install::default_destination().is_ok_and(|destination| {
            destination == sdk.root
                || destination.canonicalize().is_ok_and(|managed| {
                    sdk.root
                        .canonicalize()
                        .is_ok_and(|selected| managed == selected)
                })
        })
}

enum Message {
    Progress(Progress),
    Finished(Result<String, String>),
}

enum Phase {
    Choose,
    Planning(Receiver<Result<(PathBuf, InstallPlan), String>>),
    Terms(InstallPlan),
    Download {
        events: Receiver<Message>,
        cancel: Arc<AtomicBool>,
        progress: Option<Progress>,
    },
    Failed(String),
    Ready {
        image: String,
        error: Option<String>,
    },
}

pub(super) struct Wizard {
    choice: ImageChoice,
    phase: Phase,
    destination: PathBuf,
    accepted: bool,
    saved_sdk: String,
    selected_sdk: String,
    locale: Locale,
}

impl Wizard {
    fn new(config: &LaunchConfig, selected: &LaunchConfig) -> Self {
        Self {
            choice: ImageChoice::Original,
            phase: Phase::Choose,
            destination: PathBuf::new(),
            accepted: false,
            saved_sdk: config.sdk.clone(),
            selected_sdk: selected.sdk.clone(),
            locale: selected.locale,
        }
    }

    pub(super) fn installing(&self) -> bool {
        matches!(self.phase, Phase::Download { .. })
    }

    pub(super) fn cancel(&self) {
        if let Phase::Download { cancel, .. } = &self.phase {
            cancel.store(true, Ordering::Release);
        }
    }

    fn prepare(&mut self, ctx: &egui::Context) {
        let choice = self.choice;
        let locale = self.locale;
        let (tx, rx) = mpsc::channel();
        self.phase = Phase::Planning(rx);
        self.accepted = false;
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = (|| -> crate::Result<_> {
                let destination = runtime_install::default_destination().map_err(|cause| {
                    format!(
                        "{} {cause}",
                        locale.pick(
                            "Não foi possível localizar a pasta de dados do usuário.",
                            "Could not locate your user data directory.",
                            "No se pudo localizar el directorio de datos del usuario."
                        )
                    )
                })?;
                Ok((destination, runtime_install::plan(choice, locale)?))
            })()
            .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn download(&mut self, ctx: &egui::Context) {
        if !self.accepted || !matches!(self.phase, Phase::Terms(_)) {
            return;
        }
        let Phase::Terms(plan) = std::mem::replace(&mut self.phase, Phase::Choose) else {
            unreachable!()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        let destination = self.destination.clone();
        let (tx, events) = mpsc::channel();
        self.phase = Phase::Download {
            events,
            cancel,
            progress: None,
        };
        let ctx = ctx.clone();
        let locale = self.locale;
        thread::spawn(move || {
            let mut last: Option<(String, Instant)> = None;
            let result = runtime_install::install(
                &plan,
                &destination,
                |update| {
                    if last.as_ref().is_none_or(|(component, time)| {
                        *component != update.component
                            || time.elapsed() >= Duration::from_millis(100)
                    }) {
                        last = Some((update.component.clone(), Instant::now()));
                        let _ = tx.send(Message::Progress(update));
                        ctx.request_repaint();
                    }
                },
                &stop,
                locale,
            )
            .map(|()| plan.image)
            .map_err(|e| e.to_string());
            let _ = tx.send(Message::Finished(result));
            ctx.request_repaint();
        });
    }

    pub(super) fn poll(&mut self) {
        let disconnected = || {
            self.locale
                .pick(
                    "O instalador foi interrompido.",
                    "The installer stopped unexpectedly.",
                    "El instalador se interrumpió.",
                )
                .to_owned()
        };
        match &mut self.phase {
            Phase::Planning(rx) => match rx.try_recv() {
                Ok(Ok((path, plan))) => {
                    self.destination = path;
                    self.phase = Phase::Terms(plan);
                }
                Ok(Err(error)) => self.phase = Phase::Failed(error),
                Err(mpsc::TryRecvError::Disconnected) => self.phase = Phase::Failed(disconnected()),
                Err(mpsc::TryRecvError::Empty) => {}
            },
            Phase::Download {
                events,
                cancel,
                progress,
            } => loop {
                match events.try_recv() {
                    Ok(Message::Progress(value)) => *progress = Some(value),
                    Ok(Message::Finished(result)) => {
                        self.phase = match result {
                            Ok(image) => Phase::Ready { image, error: None },
                            Err(_) if cancel.load(Ordering::Acquire) => Phase::Choose,
                            Err(error) => Phase::Failed(error),
                        };
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.phase = Phase::Failed(disconnected());
                        break;
                    }
                }
            },
            _ => {}
        }
    }

    fn installed_config(
        &self,
        current: &LaunchConfig,
        selected: &LaunchConfig,
        base: &Path,
        id: &str,
    ) -> crate::Result<LaunchConfig> {
        let l = self.locale;
        if current.sdk != self.saved_sdk || selected.sdk != self.selected_sdk {
            return Err(l.pick("A seleção do runtime mudou. Feche e reabra o assistente para selecionar o runtime instalado.",
                "The selected runtime changed. Close and reopen setup to select the installed runtime.",
                "Cambió el entorno seleccionado. Cierra y vuelve a abrir el asistente para seleccionar el entorno instalado.").into());
        }
        let sdk = Sdk::find_localized(Some(self.destination.clone()), l)?;
        super::SystemImage::from_sdk(&sdk, id)?;
        // An existing AVD may store a path relative to the previously selected SDK.
        // Resolving it against the new runtime could silently redirect its data to another image.
        let previous = Sdk::find_localized(
            (!current.sdk.trim().is_empty()).then(|| PathBuf::from(current.sdk.trim())),
            l,
        )
        .ok();
        let requested = image_path(&self.destination, id);
        let image = requested.canonicalize()?;
        let [primary, fallback] = profile_dirs(base, self.choice);
        let mut candidates = [(primary, previous.as_ref()), (fallback, Some(&sdk))];
        // Once a conflicting primary has been isolated, keep using its portable sibling.
        if candidates[1].0.join("avd/RustEmuDroid.ini").is_file() {
            candidates.swap(0, 1);
        }
        for (target, existing_sdk) in candidates {
            let target = std::path::absolute(target)?;
            let ini = target.join("avd/RustEmuDroid.ini");
            let new = !ini.exists() && !target.join("avd/RustEmuDroid.avd").exists();
            if !new
                && !existing_sdk
                    .and_then(|runtime| {
                        Profile::localized(target.clone(), l)
                            .system_dir(runtime)
                            .ok()
                    })
                    .and_then(|p| p.canonicalize().ok())
                    .is_some_and(|p| p == image)
            {
                continue;
            }
            let mut config = selected.clone();
            config.sdk = self.destination.to_string_lossy().into_owned();
            config.state = target.to_string_lossy().into_owned();
            config.image = if new {
                requested.to_string_lossy().into_owned()
            } else {
                String::new()
            };
            return Ok(config);
        }
        Err(l.pick("Os diretórios de perfil existentes usam outras imagens. Selecione outro diretório de dados.",
            "The existing profile directories use other images. Choose a different data directory.",
            "Los directorios de perfil existentes usan otras imágenes. Elige otro directorio de datos.").into())
    }
}

impl Drop for Wizard {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Wizard {
    fn show_phase(&mut self, ui: &mut egui::Ui, idle: bool, pending_settings: bool) {
        let locale = self.locale;
        match &self.phase {
            Phase::Choose => {
                ui.label(locale.pick(
                    "Escolha a imagem Android para este computador.",
                    "Choose an Android image for this computer.",
                    "Elige una imagen de Android para este equipo.",
                ));
                ui.columns(2, |columns| {
                    let images = [
                        (ImageChoice::Original, "Original", "Android 15 · AOSP"),
                        (
                            ImageChoice::GooglePlay,
                            "Google Play",
                            "Android 17 · Play Store",
                        ),
                    ];
                    for (column, (choice, title, subtitle)) in columns.iter_mut().zip(images) {
                        if theme::profile_card(column, title, subtitle, self.choice == choice)
                            .clicked()
                        {
                            self.choice = choice;
                        }
                    }
                });
            }
            Phase::Planning(_) => {
                ui.spinner();
                ui.label(locale.pick(
                    "Consultando o catálogo do Google…",
                    "Checking the Google package catalog…",
                    "Consultando el catálogo de paquetes de Google…",
                ));
            }
            Phase::Terms(plan) => {
                show_terms(ui, plan, &self.destination, &mut self.accepted, locale);
                if !idle {
                    ui.label(locale.pick(
                        "Encerre a sessão Android para instalar.",
                        "End the Android session to install.",
                        "Finaliza la sesión de Android para instalar.",
                    ));
                }
            }
            Phase::Download {
                progress, cancel, ..
            } => {
                let heading = if cancel.load(Ordering::Acquire) {
                    locale.pick("Cancelando…", "Cancelling…", "Cancelando…")
                } else {
                    locale.pick(
                        "Instalando runtime",
                        "Installing runtime",
                        "Instalando el entorno",
                    )
                };
                ui.strong(heading);
                if let Some(progress) = progress {
                    ui.label(&progress.component);
                    let fraction = progress.bytes as f64 / progress.total_bytes.max(1) as f64;
                    ui.add(egui::ProgressBar::new(fraction.clamp(0., 1.) as f32).show_percentage());
                    ui.label(format!(
                        "{} / {}",
                        size(progress.bytes, locale),
                        size(progress.total_bytes, locale)
                    ));
                } else {
                    ui.spinner();
                }
            }
            Phase::Failed(message) => {
                ui.colored_label(ERROR, message.as_str());
            }
            Phase::Ready { image, error } => {
                ui.strong(locale.pick(
                    "Instalação concluída",
                    "Installation complete",
                    "Instalación completada",
                ));
                ui.monospace(image.as_str());
                ui.label(self.destination.display().to_string());
                let selected = self.selected_sdk.trim();
                if !selected.is_empty() && Path::new(selected) != self.destination {
                    let notice = locale.pick(
                        "Ao iniciar, o runtime instalado substituirá esta seleção:",
                        "Starting this profile replaces the currently selected runtime:",
                        "Al iniciar, este entorno sustituirá la selección actual:",
                    );
                    ui.label(format!("{notice} {}", self.selected_sdk));
                }
                if pending_settings {
                    ui.label(locale.pick(
                        "Ao iniciar, os ajustes pendentes também serão salvos.",
                        "Starting also saves your pending settings.",
                        "Al iniciar, también se guardarán los ajustes pendientes.",
                    ));
                }
                if let Some(error) = error {
                    ui.colored_label(ERROR, error.as_str());
                }
            }
        }
    }

    fn show_actions(&self, ui: &mut egui::Ui, idle: bool) -> Option<Action> {
        let locale = self.locale;
        let primary = match &self.phase {
            Phase::Choose => Some((
                locale.pick("Consultar download", "Review download", "Revisar descarga"),
                true,
                Action::Plan,
            )),
            Phase::Failed(_) => Some((
                locale.pick("Tentar novamente", "Retry", "Reintentar"),
                true,
                Action::Plan,
            )),
            Phase::Terms(plan) => Some((
                locale.pick(
                    "Baixar e instalar",
                    "Download and install",
                    "Descargar e instalar",
                ),
                idle && self.accepted && !plan.licenses.is_empty(),
                Action::Install,
            )),
            Phase::Ready { .. } => Some((
                locale.pick(
                    "Usar e iniciar este perfil",
                    "Use and start this profile",
                    "Usar e iniciar este perfil",
                ),
                idle,
                Action::Start,
            )),
            Phase::Download { cancel, .. } => Some((
                locale.pick(
                    "Cancelar instalação",
                    "Cancel installation",
                    "Cancelar instalación",
                ),
                !cancel.load(Ordering::Acquire),
                Action::Cancel,
            )),
            Phase::Planning(_) => None,
        };
        let mut action = None;
        if let Some((label, enabled, next)) = primary
            && theme::primary(ui, enabled, label).clicked()
        {
            action = Some(next);
        }
        if matches!(self.phase, Phase::Terms(_) | Phase::Failed(_)) {
            let label = locale.pick("Alterar imagem", "Change image", "Cambiar imagen");
            if ui.add(theme::button(label)).clicked() {
                action = Some(Action::Choose);
            }
        }
        if !self.installing() {
            let label = locale.pick("Fechar", "Close", "Cerrar");
            if ui.add(theme::button(label)).clicked() {
                action = Some(Action::Close);
            }
        }
        action
    }
}

fn show_terms(
    ui: &mut egui::Ui,
    plan: &InstallPlan,
    destination: &Path,
    accepted: &mut bool,
    locale: Locale,
) {
    ui.monospace(&plan.image);
    let download = locale.pick("Download de até", "Download up to", "Descarga de hasta");
    ui.label(format!("{download} {}", size(plan.total_bytes, locale)));
    ui.label(locale.pick(
        "Emulador, ferramentas ADB e imagem Android. Pacotes já instalados são reutilizados.",
        "Emulator, ADB tools and Android image. Installed packages are reused.",
        "Emulador, herramientas ADB e imagen de Android. Se reutilizan los paquetes ya instalados.",
    ));
    let label = locale.pick("Destino", "Destination", "Destino");
    ui.label(format!("{label} · {}", display_path(destination)))
        .on_hover_text(destination.display().to_string());
    ui.separator();
    ui.strong(locale.pick(
        "Termos do Google",
        "Google license terms",
        "Condiciones de Google",
    ));
    egui::ScrollArea::vertical()
        .id_salt("package-license-text")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .max_height((ui.ctx().content_rect().height() * 0.29).clamp(100., 260.))
        .show(ui, |ui| {
            for (index, license) in plan.licenses.iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                ui.add(egui::Label::new(license.as_str()).wrap().selectable(true));
            }
        });
    ui.checkbox(
        accepted,
        locale.pick(
            "Li e aceito os termos do Google para estes pacotes.",
            "I have read and accept Google's terms for these packages.",
            "He leído y acepto las condiciones de Google para estos paquetes.",
        ),
    );
}

impl Launcher {
    pub(super) fn open_runtime_setup(&mut self) {
        if self.installer.is_none() && !self.closing {
            self.installer = Some(Wizard::new(
                &self.config,
                self.settings.as_ref().unwrap_or(&self.config),
            ));
        }
    }

    pub(super) fn show_runtime_setup(&mut self, ctx: &egui::Context) {
        let Some(wizard) = &mut self.installer else {
            return;
        };
        let idle = self.task.is_none() && !self.playing;
        let pending_settings = self
            .settings
            .as_ref()
            .is_some_and(|draft| draft != &self.config);
        let dialog = egui::Modal::new("portable-runtime-setup".into())
            .frame(egui::Frame::window(&ctx.style()).inner_margin(20))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 72.).clamp(240., 580.));
                theme::form(ui);
                ui.heading(install_label(wizard.locale));
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("runtime-setup-body")
                    .max_height((ctx.content_rect().height() - 175.).max(100.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| wizard.show_phase(ui, idle, pending_settings));
                ui.separator();
                ui.horizontal_wrapped(|ui| wizard.show_actions(ui, idle))
                    .inner
            });
        let action = if dialog.should_close() && !wizard.installing() {
            Some(Action::Close)
        } else {
            dialog.inner
        };
        match action {
            Some(Action::Plan) => wizard.prepare(ctx),
            Some(Action::Install) if idle => wizard.download(ctx),
            Some(Action::Cancel) => wizard.cancel(),
            Some(Action::Choose) => {
                wizard.accepted = false;
                wizard.phase = Phase::Choose;
            }
            Some(Action::Close) => self.installer = None,
            Some(Action::Start) if idle => {
                let Phase::Ready { image, .. } = &wizard.phase else {
                    return;
                };
                let config = wizard
                    .installed_config(
                        &self.config,
                        self.settings.as_ref().unwrap_or(&self.config),
                        self.prefs_file.parent().unwrap_or(Path::new(".")),
                        image,
                    )
                    .and_then(|config| {
                        write_json(&self.prefs_file, &config)?;
                        Ok(config)
                    });
                match config {
                    Ok(config) => {
                        self.config = config;
                        self.images = Default::default();
                        self.settings = None;
                        self.installer = None;
                        self.start(false);
                    }
                    Err(e) => {
                        if let Phase::Ready { error, .. } = &mut wizard.phase {
                            *error = Some(e.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

enum Action {
    Plan,
    Install,
    Start,
    Cancel,
    Close,
    Choose,
}

fn size(bytes: u64, locale: Locale) -> String {
    let size = if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB", bytes as f64 / (1024. * 1024. * 1024.))
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024. * 1024.))
    };
    if locale == Locale::EnUs {
        size
    } else {
        size.replace('.', ",")
    }
}

fn display_path(path: &Path) -> String {
    let (variable, prefix) = if cfg!(windows) {
        ("LOCALAPPDATA", "%LOCALAPPDATA%")
    } else {
        ("HOME", "~")
    };
    std::env::var_os(variable)
        .and_then(|base| path.strip_prefix(Path::new(&base)).ok())
        .map(|relative| Path::new(prefix).join(relative).display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(all(test, windows))]
mod preview;
#[cfg(test)]
mod tests;
