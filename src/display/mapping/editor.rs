use super::{Gesture, Mapping};
use crate::ui::{i18n::Locale, theme};
use eframe::egui;
use std::{path::PathBuf, sync::mpsc};

#[derive(Default)]
pub(super) struct Editor {
    profile_name: String,
    confirm_delete: bool,
    file_picker: Option<FilePicker>,
    file_notice: Option<(FileAction, Result<(), String>)>,
}

#[derive(Clone, Copy)]
enum FileAction {
    Import,
    Export,
}

struct FilePicker {
    action: FileAction,
    package: String,
    profile: String,
    selection: mpsc::Receiver<Option<PathBuf>>,
}

impl Mapping {
    pub(super) fn advanced_toolbar(&mut self, ui: &mut egui::Ui, locale: Locale) {
        self.poll_profile_file();
        let advanced = egui::CollapsingHeader::new(locale.pick(
            "Controles avançados",
            "Advanced controls",
            "Controles avanzados",
        ))
        .id_salt("advanced-controls")
        .default_open(true)
        .show(ui, |ui| {
            ui.add_enabled_ui(self.package.is_some(), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(190.)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        self.profile_controls(ui, locale);
                        ui.separator();
                        self.gesture_tools(ui, locale);
                        self.binding_settings(ui, locale);
                        self.placement_hint(ui, locale);
                    });
            });
        });
        if advanced.fully_closed()
            && (self.placing_control || self.placing_endpoint || self.await_binding)
        {
            self.reset_placement();
        }
    }

    fn profile_controls(&mut self, ui: &mut egui::Ui, locale: Locale) {
        let mut profile = self.draft_profiles.active.clone();
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(locale.pick("Perfil", "Profile", "Perfil"));
            egui::ComboBox::from_id_salt("mapping-profile")
                .selected_text(profile_name(&profile, locale))
                .show_ui(ui, |ui| {
                    for name in self.draft_profiles.layouts.keys() {
                        ui.selectable_value(&mut profile, name.clone(), profile_name(name, locale));
                    }
                })
                .response
                .labelled_by(label.id);
            if profile != self.draft_profiles.active {
                self.select_profile(&profile);
                self.editor.confirm_delete = false;
            }
            ui.menu_button(
                locale.pick("Novo perfil", "New profile", "Nuevo perfil"),
                |ui| self.new_profile(ui, locale, false),
            );
            ui.menu_button(locale.pick("Duplicar", "Duplicate", "Duplicar"), |ui| {
                self.new_profile(ui, locale, true)
            });
            ui.add_enabled_ui(self.editor.file_picker.is_none(), |ui| {
                for action in [FileAction::Import, FileAction::Export] {
                    if ui.button(action.label(locale)).clicked() {
                        self.pick_profile_file(ui.ctx(), action, locale);
                    }
                }
            });
            if self.draft_profiles.layouts.len() > 1
                && ui
                    .button(locale.pick("Excluir perfil", "Delete profile", "Eliminar perfil"))
                    .clicked()
            {
                self.editor.confirm_delete = true;
            }
        });
        if self.editor.confirm_delete {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} {}",
                    locale.pick("Excluir o perfil", "Delete profile", "Eliminar el perfil"),
                    profile_name(&self.draft_profiles.active, locale),
                ));
                if ui
                    .button(locale.pick("Excluir", "Delete", "Eliminar"))
                    .clicked()
                {
                    self.delete_profile();
                    self.editor.confirm_delete = false;
                }
                if ui
                    .button(locale.pick("Cancelar", "Cancel", "Cancelar"))
                    .clicked()
                {
                    self.editor.confirm_delete = false;
                }
            });
        }
        if let Some((action, result)) = &self.editor.file_notice {
            match result {
                Ok(()) => {
                    ui.label(match action {
                        FileAction::Import => locale.pick(
                            "Esquema importado. Salve para aplicar.",
                            "Scheme imported. Save to apply.",
                            "Esquema importado. Guarda para aplicarlo.",
                        ),
                        FileAction::Export => locale.pick(
                            "Esquema exportado.",
                            "Scheme exported.",
                            "Esquema exportado.",
                        ),
                    });
                }
                Err(error) => {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        locale.pick(
                            "Falha ao transferir o esquema.",
                            "Couldn't transfer the scheme.",
                            "No se pudo transferir el esquema.",
                        ),
                    )
                    .on_hover_text(error);
                }
            }
        }
    }

    fn new_profile(&mut self, ui: &mut egui::Ui, locale: Locale, copy: bool) {
        let label = ui.label(locale.pick("Nome do perfil", "Profile name", "Nombre del perfil"));
        let input = ui
            .add(egui::TextEdit::singleline(&mut self.editor.profile_name).char_limit(48))
            .labelled_by(label.id);
        let name = self.editor.profile_name.trim().to_owned();
        let available = !name.is_empty()
            && name.chars().count() <= 48
            && !name.chars().any(char::is_control)
            && !self.draft_profiles.layouts.contains_key(&name);
        let action = if copy {
            locale.pick("Duplicar", "Duplicate", "Duplicar")
        } else {
            locale.pick("Criar", "Create", "Crear")
        };
        let submit = ui
            .add_enabled(available, egui::Button::new(action))
            .clicked()
            || (available
                && input.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if submit {
            let created = if copy {
                self.copy_profile(&name)
            } else {
                self.create_profile(&name)
            };
            if created {
                self.editor.profile_name.clear();
                ui.close();
            }
        }
    }

    fn pick_profile_file(&mut self, ctx: &egui::Context, action: FileAction, locale: Locale) {
        let Some(package) = self.package.clone() else {
            return;
        };
        let profile = self.draft_profiles.active.clone();
        let (tx, selection) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title(action.label(locale))
                .add_filter(
                    locale.pick("Esquema JSON", "JSON scheme", "Esquema JSON"),
                    &["json"],
                );
            let selection = match action {
                FileAction::Import => dialog.pick_file(),
                FileAction::Export => dialog.set_file_name("controls.json").save_file(),
            };
            let _ = tx.send(selection);
            ctx.request_repaint();
        });
        self.editor.file_picker = Some(FilePicker {
            action,
            package,
            profile,
            selection,
        });
        self.editor.file_notice = None;
    }

    fn poll_profile_file(&mut self) {
        let Some(picker) = &self.editor.file_picker else {
            return;
        };
        let choice = match picker.selection.try_recv() {
            Ok(path) => path,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let picker = self.editor.file_picker.take().expect("picker is active");
        let Some(path) = choice else { return };
        if self.package.as_deref() != Some(&picker.package)
            || self.draft_profiles.active != picker.profile
        {
            return;
        }
        let result = match picker.action {
            FileAction::Import => self.import_profile(&path),
            FileAction::Export => self.export_profile(&path),
        };
        self.editor.file_notice = Some((picker.action, result));
    }

    fn gesture_tools(&mut self, ui: &mut egui::Ui, locale: Locale) {
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(locale.pick("Novo controle", "New control", "Nuevo control"));
            egui::ComboBox::from_id_salt("mapping-gesture")
                .selected_text(gesture_name(&self.preset, locale))
                .show_ui(ui, |ui| {
                    for preset in presets() {
                        let selected =
                            std::mem::discriminant(&self.preset) == std::mem::discriminant(&preset);
                        if ui
                            .selectable_label(selected, gesture_name(&preset, locale))
                            .clicked()
                        {
                            self.preset = preset;
                        }
                    }
                })
                .response
                .labelled_by(label.id);
            let place = if self.placing_control {
                locale.pick(
                    "Cancelar posicionamento",
                    "Cancel placement",
                    "Cancelar colocación",
                )
            } else {
                locale.pick(
                    "Posicionar no Android",
                    "Place on Android screen",
                    "Colocar en Android",
                )
            };
            if ui.selectable_label(self.placing_control, place).clicked() {
                self.placing_control = !self.placing_control;
                self.placing_endpoint = false;
                self.await_binding = false;
                self.direction_binding = None;
                self.pending = None;
                self.place_stick = false;
            }
        });
    }

    fn binding_settings(&mut self, ui: &mut egui::Ui, locale: Locale) {
        if self.draft.controls.is_empty() {
            return;
        }
        let selected = self.selected_control;
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(locale.pick("Controle", "Control", "Control"));
            let text = self
                .draft
                .controls
                .get(selected.unwrap_or(usize::MAX))
                .map(|binding| gesture_name(&binding.gesture, locale))
                .unwrap_or_else(|| {
                    locale.pick(
                        "Selecionar controle",
                        "Select a control",
                        "Seleccionar control",
                    )
                });
            egui::ComboBox::from_id_salt("mapping-binding")
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for (index, binding) in self.draft.controls.iter().enumerate() {
                        let label = format!(
                            "{} · {}",
                            gesture_name(&binding.gesture, locale),
                            if binding.input.is_empty() {
                                locale.pick("Sem tecla", "Unbound", "Sin tecla")
                            } else {
                                &binding.input
                            }
                        );
                        ui.selectable_value(&mut self.selected_control, Some(index), label);
                    }
                })
                .response
                .labelled_by(label.id);
        });
        if selected != self.selected_control {
            self.await_binding = false;
            self.placing_endpoint = false;
            self.direction_binding = None;
        }
        let Some(index) = self.selected_control else {
            return;
        };
        let Some(binding) = self.draft.controls.get_mut(index) else {
            return;
        };
        let mut direction_binding = self.direction_binding;
        let mut set_binding = false;
        let mut set_endpoint = false;
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            if !matches!(binding.gesture, Gesture::Dpad { .. }) {
                let input = if binding.input.is_empty() {
                    locale.pick(
                        "Definir tecla ou botão",
                        "Assign key or button",
                        "Asignar tecla o botón",
                    )
                } else {
                    &binding.input
                };
                set_binding = ui
                    .selectable_label(self.await_binding && direction_binding.is_none(), input)
                    .clicked();
            }
            match &mut binding.gesture {
                Gesture::Hold => {}
                Gesture::Tap { duration_ms } => {
                    ui.add(
                        egui::Slider::new(duration_ms, 10..=1000)
                            .suffix(" ms")
                            .text(locale.pick("Duração", "Duration", "Duración")),
                    );
                }
                Gesture::Repeat {
                    interval_ms,
                    hold_ms,
                } => {
                    ui.add(
                        egui::Slider::new(interval_ms, 20..=2000)
                            .suffix(" ms")
                            .text(locale.pick("Intervalo", "Interval", "Intervalo")),
                    );
                    ui.add(
                        egui::Slider::new(hold_ms, 10..=(*interval_ms).max(10))
                            .suffix(" ms")
                            .text(locale.pick("Pressionamento", "Press time", "Pulsación")),
                    );
                }
                Gesture::Swipe { duration_ms, .. } => {
                    ui.add(
                        egui::Slider::new(duration_ms, 30..=3000)
                            .suffix(" ms")
                            .text(locale.pick("Duração", "Duration", "Duración")),
                    );
                    set_endpoint = ui
                        .selectable_label(
                            self.placing_endpoint,
                            locale.pick(
                                "Definir ponto final",
                                "Set end point",
                                "Definir punto final",
                            ),
                        )
                        .clicked();
                }
                Gesture::Dpad {
                    directions,
                    radius,
                    left_stick,
                } => {
                    for (direction, name) in [
                        locale.pick("Cima", "Up", "Arriba"),
                        locale.pick("Baixo", "Down", "Abajo"),
                        locale.pick("Esquerda", "Left", "Izquierda"),
                        locale.pick("Direita", "Right", "Derecha"),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let label = format!("{name} · {}", directions[direction]);
                        if ui
                            .selectable_label(
                                self.await_binding && direction_binding == Some(direction),
                                label,
                            )
                            .clicked()
                        {
                            direction_binding = Some(direction);
                            set_binding = true;
                        }
                    }
                    ui.add(
                        egui::Slider::new(radius, 0.035..=0.25)
                            .text(locale.pick("Raio", "Radius", "Radio")),
                    );
                    ui.checkbox(
                        left_stick,
                        locale.pick("Analógico esquerdo", "Left stick", "Palanca izquierda"),
                    );
                }
                Gesture::Aim {
                    sensitivity,
                    mouse,
                    right_stick,
                    toggle,
                } => {
                    ui.add(egui::Slider::new(sensitivity, 0.1..=5.0).text(locale.pick(
                        "Sensibilidade",
                        "Sensitivity",
                        "Sensibilidad",
                    )));
                    ui.checkbox(mouse, locale.pick("Mouse", "Mouse", "Ratón"));
                    ui.checkbox(
                        right_stick,
                        locale.pick("Analógico direito", "Right stick", "Palanca derecha"),
                    );
                    ui.checkbox(
                        toggle,
                        locale.pick(
                            "Alternar com tecla",
                            "Toggle with key",
                            "Alternar con tecla",
                        ),
                    );
                }
                Gesture::Moba { radius, quick_cast } => {
                    ui.add(egui::Slider::new(radius, 0.035..=0.35).text(locale.pick(
                        "Raio da habilidade",
                        "Ability radius",
                        "Radio de la habilidad",
                    )));
                    ui.checkbox(
                        quick_cast,
                        locale.pick("Conjuração rápida", "Quick cast", "Lanzamiento rápido"),
                    );
                }
                Gesture::Pinch {
                    from,
                    to,
                    duration_ms,
                } => {
                    ui.add(egui::Slider::new(from, 0.02..=0.35).text(locale.pick(
                        "Raio inicial",
                        "Start radius",
                        "Radio inicial",
                    )));
                    ui.add(egui::Slider::new(to, 0.02..=0.35).text(locale.pick(
                        "Raio final",
                        "End radius",
                        "Radio final",
                    )));
                    ui.add(
                        egui::Slider::new(duration_ms, 30..=3000)
                            .suffix(" ms")
                            .text(locale.pick("Duração", "Duration", "Duración")),
                    );
                }
                Gesture::Rotate {
                    radius,
                    degrees,
                    duration_ms,
                } => {
                    ui.add(
                        egui::Slider::new(radius, 0.02..=0.35)
                            .text(locale.pick("Raio", "Radius", "Radio")),
                    );
                    ui.add(
                        egui::Slider::new(degrees, -360.0..=360.0)
                            .suffix("°")
                            .text(locale.pick("Ângulo", "Angle", "Ángulo")),
                    );
                    ui.add(
                        egui::Slider::new(duration_ms, 30..=3000)
                            .suffix(" ms")
                            .text(locale.pick("Duração", "Duration", "Duración")),
                    );
                }
            }
            remove = ui
                .button(locale.pick("Remover controle", "Remove control", "Quitar control"))
                .clicked();
        });

        if remove {
            self.draft.controls.remove(index);
            self.reset_placement();
        } else if set_binding {
            self.await_binding = !self.await_binding || self.direction_binding != direction_binding;
            self.direction_binding = if self.await_binding {
                direction_binding
            } else {
                None
            };
            self.placing_endpoint = false;
            self.placing_control = false;
            self.pending = None;
            if self.await_binding {
                ui.memory_mut(|memory| {
                    if let Some(focused) = memory.focused() {
                        memory.surrender_focus(focused);
                    }
                });
            }
        } else if set_endpoint {
            self.placing_endpoint = !self.placing_endpoint;
            self.await_binding = false;
            self.direction_binding = None;
            self.placing_control = false;
        }
    }

    fn placement_hint(&self, ui: &mut egui::Ui, locale: Locale) {
        let hint = if self.placing_endpoint {
            locale.pick(
                "Clique na tela para marcar onde o deslize termina.",
                "Click the screen to mark where the swipe ends.",
                "Haz clic en la pantalla para marcar el final del deslizamiento.",
            )
        } else if self.placing_control {
            locale.pick(
                "Clique na tela do Android para posicionar o controle.",
                "Click the Android screen to place the control.",
                "Haz clic en la pantalla de Android para colocar el control.",
            )
        } else if self.await_binding {
            locale.pick(
                "Pressione uma tecla ou um botão do controle.",
                "Press a keyboard key or controller button.",
                "Pulsa una tecla o un botón del mando.",
            )
        } else {
            return;
        };
        ui.label(egui::RichText::new(hint).color(theme::ACCENT));
    }
}

fn profile_name(name: &str, locale: Locale) -> &str {
    if name == "Default" {
        locale.pick("Padrão", "Default", "Predeterminado")
    } else {
        name
    }
}

fn gesture_name(gesture: &Gesture, locale: Locale) -> &'static str {
    match gesture {
        Gesture::Hold => locale.pick("Toque mantido", "Held touch", "Toque mantenido"),
        Gesture::Tap { .. } => locale.pick("Toque rápido", "Tap", "Toque rápido"),
        Gesture::Repeat { .. } => {
            locale.pick("Toques repetidos", "Repeated taps", "Toques repetidos")
        }
        Gesture::Swipe { .. } => locale.pick("Deslizar", "Swipe", "Deslizar"),
        Gesture::Dpad { .. } => {
            locale.pick("Direcional avançado", "Advanced D-pad", "Cruceta avanzada")
        }
        Gesture::Aim { .. } => locale.pick("Mira ou câmera", "Aim or camera", "Mira o cámara"),
        Gesture::Moba { .. } => locale.pick("Habilidade MOBA", "MOBA ability", "Habilidad MOBA"),
        Gesture::Pinch { .. } => {
            locale.pick("Pinça para zoom", "Pinch to zoom", "Pellizcar para ampliar")
        }
        Gesture::Rotate { .. } => locale.pick(
            "Girar com dois dedos",
            "Two-finger rotation",
            "Giro con dos dedos",
        ),
    }
}

impl FileAction {
    fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Import => locale.pick("Importar esquema…", "Import scheme…", "Importar esquema…"),
            Self::Export => locale.pick("Exportar esquema…", "Export scheme…", "Exportar esquema…"),
        }
    }
}

fn presets() -> [Gesture; 9] {
    [
        Gesture::Hold,
        Gesture::Tap { duration_ms: 100 },
        Gesture::Repeat {
            interval_ms: 180,
            hold_ms: 65,
        },
        Gesture::Swipe {
            end: [0.75, 0.5],
            duration_ms: 260,
        },
        Gesture::Dpad {
            directions: ["W".into(), "S".into(), "A".into(), "D".into()],
            radius: 0.1,
            left_stick: true,
        },
        Gesture::Aim {
            sensitivity: 1.0,
            mouse: true,
            right_stick: true,
            toggle: false,
        },
        Gesture::Moba {
            radius: 0.15,
            quick_cast: false,
        },
        Gesture::Pinch {
            from: 0.06,
            to: 0.18,
            duration_ms: 350,
        },
        Gesture::Rotate {
            radius: 0.11,
            degrees: 90.,
            duration_ms: 400,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::mapping::Binding;

    #[test]
    fn localized_control_button_releases_focus_before_capturing_a_key() {
        let ctx = egui::Context::default();
        let mut mapping = Mapping::default();
        mapping.select(Some("org.example.game"));
        mapping.toggle();
        mapping.draft.controls.push(Binding {
            input: String::new(),
            at: [0.5, 0.5],
            gesture: Gesture::Tap { duration_ms: 100 },
        });
        mapping.selected_control = Some(0);

        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(610., 510.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        mapping.binding_settings(ui, Locale::EsEs);
                    });
                },
            )
        };
        render(Vec::new());
        let out = render(Vec::new());
        let assign = out
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape
                    && text.galley.text() == "Asignar tecla o botón"
                {
                    Some(text.visual_bounding_rect().center())
                } else {
                    None
                }
            })
            .unwrap();
        let click = |pressed| egui::Event::PointerButton {
            pos: assign,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(vec![egui::Event::PointerMoved(assign), click(true)]);
        render(vec![click(false)]);
        assert!(mapping.await_binding);
        assert!(!ctx.wants_keyboard_input());
    }
}
