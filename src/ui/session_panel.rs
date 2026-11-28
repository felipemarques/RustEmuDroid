use super::Tool;
use super::{
    Action, BACKGROUND, BORDER, BootStage, ERROR, Launcher, MINT, MUTED, PANEL, mascot, theme,
};
use eframe::egui::{self, RichText, vec2};
use std::{sync::atomic::Ordering, time::Duration};

impl Launcher {
    pub(super) fn show_shutdown(&self, ctx: &egui::Context, disconnecting: bool) {
        let locale = self.config.locale;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BACKGROUND))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(((ui.available_height() - 165.) * 0.5).max(0.));
                    mascot::show(ui, self.brand.id(), 72., self.config.animate_mascot, false);
                    ui.add_space(14.);
                    ui.heading(if self.installer.as_ref().is_some_and(|w| w.installing()) {
                        locale.pick(
                            "Cancelando instalação…",
                            "Cancelling installation…",
                            "Cancelando la instalación…",
                        )
                    } else if disconnecting {
                        locale.pick(
                            "Desconectando do BlueStacks…",
                            "Disconnecting from BlueStacks…",
                            "Desconectando de BlueStacks…",
                        )
                    } else if self.task.is_some() {
                        locale.pick(
                            "Desligando Android…",
                            "Shutting down Android…",
                            "Apagando Android…",
                        )
                    } else {
                        locale.pick(
                            "Finalizando capturas…",
                            "Finishing captures…",
                            "Finalizando capturas…",
                        )
                    });
                    ui.label(
                        RichText::new(locale.pick(
                            "A janela fechará automaticamente.",
                            "The window will close automatically.",
                            "La ventana se cerrará automáticamente.",
                        ))
                        .color(MUTED),
                    );
                    ui.add_space(12.);
                    ui.add(egui::Spinner::new().size(22.).color(theme::ACCENT));
                });
            });
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    pub(super) fn show_boot(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let locale = self.config.locale;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(8.));
            mascot::show(
                ui,
                self.brand.id(),
                96.,
                self.config.animate_mascot
                    && !self.help_open
                    && self.settings.is_none()
                    && !self.external_dialog_open(),
                false,
            );
            ui.add_space(12.);
            if self.playing {
                if let Some(Err(error)) = self.controls.as_ref().and_then(|c| c.message.as_ref()) {
                    ui.colored_label(
                        ERROR,
                        locale.pick(
                            "Falha ao abrir a biblioteca.",
                            "Couldn't open the library.",
                            "No se pudo abrir la biblioteca.",
                        ),
                    );
                    ui.collapsing(
                        locale.pick(
                            "Detalhes técnicos",
                            "Technical details",
                            "Detalles técnicos",
                        ),
                        |ui| {
                            ui.label(error);
                        },
                    );
                    if ui
                        .add(theme::button(locale.pick(
                            "Tentar abrir a biblioteca",
                            "Retry opening the library",
                            "Volver a abrir la biblioteca",
                        )))
                        .clicked()
                        && let Some(c) = &mut self.controls
                    {
                        c.send(Action::PrepareHome(locale), ctx);
                    }
                } else {
                    ui.spinner();
                    ui.label(if self.stop.load(Ordering::Acquire) {
                        locale.pick(
                            "Desligando Android…",
                            "Shutting down Android…",
                            "Apagando Android…",
                        )
                    } else if self.stage == BootStage::Booted {
                        locale.pick(
                            "Abrindo a biblioteca de jogos…",
                            "Opening the game library…",
                            "Abriendo la biblioteca de juegos…",
                        )
                    } else {
                        self.starting_label()
                    });
                }
            }
        });
    }

    fn starting_label(&self) -> &'static str {
        let locale = self.config.locale;
        if self.stage == BootStage::Preparing {
            locale.pick(
                "Preparando Android…",
                "Preparing Android…",
                "Preparando Android…",
            )
        } else {
            locale.pick(
                "Iniciando Android…",
                "Starting Android…",
                "Iniciando Android…",
            )
        }
    }

    pub(super) fn show_idle(&mut self, ui: &mut egui::Ui) {
        let locale = self.config.locale;
        let (status, color) = if self.failed {
            (
                locale.pick(
                    "A última operação falhou",
                    "The last operation failed",
                    "La última operación ha fallado",
                ),
                ERROR,
            )
        } else if self.task.is_some() {
            (
                locale.pick(
                    "Verificando inicialização…",
                    "Checking startup…",
                    "Comprobando el inicio…",
                ),
                MINT,
            )
        } else {
            match self.stage {
                BootStage::Stopped => (
                    locale.pick("Sessão encerrada", "Session ended", "Sesión finalizada"),
                    MUTED,
                ),
                BootStage::Verified => (
                    locale.pick(
                        "Inicialização verificada",
                        "Startup verified",
                        "Inicio verificado",
                    ),
                    MINT,
                ),
                _ => (
                    locale.pick(
                        "Nenhuma sessão ativa",
                        "No active session",
                        "Ninguna sesión activa",
                    ),
                    MUTED,
                ),
            }
        };
        let width = (ui.available_width() - 48.).clamp(216., 480.);
        let top_space = ((ui.available_height() - 342.) * 0.45).max(24.);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(top_space);
                    ui.allocate_ui_with_layout(
                        vec2(width, 0.),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            egui::Frame::new()
                                .fill(PANEL)
                                .stroke(egui::Stroke::new(1_f32, BORDER))
                                .corner_radius(18)
                                .inner_margin(24)
                                .show(ui, |ui| {
                                    ui.set_width(width - 48.);
                                    ui.vertical_centered(|ui| {
                                        ui.image((self.brand.id(), vec2(76., 76.)));
                                        ui.add_space(12.);
                                        ui.heading(
                                            RichText::new(locale.pick(
                                                "Emulador Android",
                                                "Android emulator",
                                                "Emulador Android",
                                            ))
                                            .strong()
                                            .size(28.),
                                        );
                                        ui.label(RichText::new(status).size(15.).color(color));
                                        if self.failed && !self.status.is_empty() {
                                            ui.add_space(6.);
                                            failure_reason(ui, &self.status);
                                        }
                                        ui.add_space(18.);
                                        theme::form(ui);
                                        if self.task.is_some() {
                                            ui.add(
                                                egui::Spinner::new().size(22.).color(theme::ACCENT),
                                            );
                                        } else if theme::primary(
                                            ui,
                                            true,
                                            locale.pick(
                                                "Iniciar Android",
                                                "Start Android",
                                                "Iniciar Android",
                                            ),
                                        )
                                        .clicked()
                                        {
                                            self.start(false);
                                        }
                                        if self.failed
                                            && self.stage == BootStage::Preparing
                                            && self.task.is_none()
                                            && ui
                                                .add(theme::button(
                                                    super::runtime_setup::install_label(locale),
                                                ))
                                                .clicked()
                                        {
                                            self.open_runtime_setup();
                                        }
                                        if ui
                                            .add(theme::button(locale.pick(
                                                "Configurações",
                                                "Settings",
                                                "Ajustes",
                                            )))
                                            .clicked()
                                        {
                                            self.open_settings();
                                        }
                                        #[cfg(windows)]
                                        self.blue.entry(ui, locale);
                                        if self.capture_dir().is_dir()
                                            && ui
                                                .add(theme::button(locale.pick(
                                                    "Abrir pasta de capturas",
                                                    "Open captures folder",
                                                    "Abrir carpeta de capturas",
                                                )))
                                                .clicked()
                                        {
                                            self.capture_action(Tool::CapturesFolder);
                                        }
                                    });
                                    if !self.status.is_empty() || !self.output.is_empty() {
                                        ui.add_space(10.);
                                        ui.separator();
                                        egui::CollapsingHeader::new(locale.pick(
                                            "Detalhes técnicos",
                                            "Technical details",
                                            "Detalles técnicos",
                                        ))
                                        .id_salt("idle-details")
                                        .show(ui, |ui| {
                                            if self.failed && !self.status.is_empty() {
                                                ui.label(&self.status);
                                            }
                                            for line in &self.output {
                                                ui.monospace(line);
                                            }
                                        });
                                    }
                                });
                        },
                    );
                });
            });
    }
}

/// Rows of the failure reason shown on the idle screen; the rest is elided and
/// stays available on hover and under "Technical details". Eight rows hold an
/// explained engine exit, its log path and the app that was open.
pub(super) const FAILURE_ROWS: usize = 8;

/// The error behind "The last operation failed", wrapped and selectable so
/// the user sees why without opening the details.
fn failure_reason(ui: &mut egui::Ui, reason: &str) {
    let reason = reason.trim();
    let mut job = egui::text::LayoutJob::default();
    RichText::new(reason).size(13.).color(ERROR).append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    job.wrap.max_rows = FAILURE_ROWS;
    let response = ui.add(egui::Label::new(job).wrap().selectable(true));
    if reason.lines().count() > FAILURE_ROWS || reason.chars().count() > 360 {
        response.on_hover_text(reason);
    }
}
