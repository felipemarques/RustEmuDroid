use super::{ERROR, Locale, MUTED, theme};
use crate::bluestacks::{self, BlueStacksError, Instance};
use eframe::egui::{self, RichText};

pub(super) struct BlueStacksPanel {
    instances: Vec<Instance>,
    dialog: Option<LaunchDialog>,
}

struct LaunchDialog {
    instance: String,
    package: String,
    error: Option<BlueStacksError>,
}

impl Default for BlueStacksPanel {
    fn default() -> Self {
        Self {
            instances: bluestacks::instances().unwrap_or_default(),
            dialog: None,
        }
    }
}

impl BlueStacksPanel {
    pub(super) fn entry_requested(&self, ui: &mut egui::Ui, locale: Locale) -> bool {
        !self.instances.is_empty()
            && ui
                .add(theme::button(locale.pick(
                    "Abrir BlueStacks",
                    "Open BlueStacks",
                    "Abrir BlueStacks",
                )))
                .clicked()
    }

    pub(super) fn entry(&mut self, ui: &mut egui::Ui, locale: Locale) {
        if self.entry_requested(ui, locale) {
            self.open();
        }
    }

    pub(super) fn open(&mut self) {
        let (instance, error) = match bluestacks::instances() {
            Ok(instances) => {
                self.instances = instances;
                (
                    self.instances
                        .first()
                        .map(|entry| entry.id.clone())
                        .unwrap_or_default(),
                    None,
                )
            }
            Err(error) => (String::new(), Some(error)),
        };
        self.dialog = Some(LaunchDialog {
            instance,
            package: String::new(),
            error,
        });
    }

    pub(super) fn is_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(super) fn show(&mut self, ctx: &egui::Context, locale: Locale) {
        let Some(entry) = &mut self.dialog else {
            return;
        };
        let mut launch = false;
        let frame = egui::Frame::window(&ctx.style()).inner_margin(22);
        let modal = egui::Modal::new("bluestacks-launch".into())
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 72.).clamp(240., 456.));
                theme::form(ui);
                ui.heading(
                    RichText::new(locale.pick(
                        "Abrir BlueStacks",
                        "Open BlueStacks",
                        "Abrir BlueStacks",
                    ))
                    .strong()
                    .size(22.),
                );
                ui.label(
                    RichText::new(locale.pick(
                        "A instância e os jogos abrem em uma janela do BlueStacks.",
                        "The instance and its games open in a BlueStacks window.",
                        "La instancia y sus juegos se abren en una ventana de BlueStacks.",
                    ))
                    .color(MUTED),
                );
                ui.add_space(8.);
                let name = self
                    .instances
                    .iter()
                    .find(|instance| instance.id == entry.instance)
                    .map(|instance| instance.name.as_str())
                    .unwrap_or("—");
                let label = theme::label(ui, locale.pick("Instância", "Instance", "Instancia"));
                theme::select(ui, "bluestacks-instance", name, |ui| {
                    for instance in &self.instances {
                        let label = if instance.name == instance.id {
                            instance.id.clone()
                        } else {
                            format!("{} ({})", instance.name, instance.id)
                        };
                        ui.selectable_value(&mut entry.instance, instance.id.clone(), label);
                    }
                })
                .labelled_by(label.id);
                let label = theme::label(
                    ui,
                    locale.pick(
                        "Pacote do jogo ou app (opcional)",
                        "Game or app package (optional)",
                        "Paquete del juego o aplicación (opcional)",
                    ),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut entry.package)
                        .desired_width(f32::INFINITY)
                        .hint_text("com.example.game"),
                )
                .labelled_by(label.id);
                if let Some(error) = &entry.error {
                    ui.colored_label(ERROR, error.localized(locale))
                        .on_hover_text(error.to_string());
                }
                ui.add_space(10.);
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    launch = theme::primary(
                        ui,
                        !entry.instance.is_empty(),
                        locale.pick("Abrir", "Open", "Abrir"),
                    )
                    .clicked();
                    if ui
                        .add(theme::button(locale.pick("Cancelar", "Cancel", "Cancelar")))
                        .clicked()
                    {
                        ui.close();
                    }
                });
            });
        if launch {
            let package = entry.package.trim();
            match bluestacks::launch(
                Some(&entry.instance),
                (!package.is_empty()).then_some(package),
            ) {
                Ok(()) => self.dialog = None,
                Err(error) => entry.error = Some(error),
            }
        } else if modal.should_close() {
            self.dialog = None;
        }
    }
}
