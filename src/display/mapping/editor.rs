mod tiles;

use super::{Gesture, Mapping, SchemeError};
use crate::ui::{i18n::Locale, theme};
use eframe::egui;
use std::{path::PathBuf, sync::mpsc};

#[derive(Default)]
pub(super) struct Editor {
    pub(super) palette_drag: bool,
    pub(super) preset_input: &'static str,
    active_tile: usize,
    profile_name: String,
    confirm_delete: bool,
    file_picker: Option<FilePicker>,
    file_notice: Option<(FileAction, Result<(), SchemeError>)>,
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
    pub(super) fn control_editor(&mut self, ui: &mut egui::Ui, locale: Locale, size: egui::Vec2) {
        self.poll_profile_file();
        let bounds = egui::Rect::from_min_size(ui.next_widget_position(), size);
        let footer =
            egui::Rect::from_min_max(egui::pos2(bounds.left(), bounds.bottom() - 35.), bounds.max);
        ui.spacing_mut().item_spacing.y = 5.;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(28., 28.), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 8, egui::Color32::from_rgb(66, 47, 42));
            tiles::draw(ui.painter(), rect.center(), 4, theme::ACCENT);
            ui.label(
                egui::RichText::new(locale.pick(
                    "Editor de controles",
                    "Controls editor",
                    "Editor de controles",
                ))
                .strong()
                .size(17.),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, dock) =
                    ui.allocate_exact_size(egui::vec2(28., 28.), egui::Sense::click());
                let target_is_left = !self.quick.editor_left;
                let label = if target_is_left {
                    locale.pick(
                        "Mover para a esquerda",
                        "Move to the left",
                        "Mover a la izquierda",
                    )
                } else {
                    locale.pick(
                        "Mover para a direita",
                        "Move to the right",
                        "Mover a la derecha",
                    )
                };
                dock.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label)
                });
                if dock.hovered() || dock.has_focus() {
                    ui.painter()
                        .rect_filled(rect, 6, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                tiles::dock_icon(
                    ui.painter(),
                    rect.center(),
                    target_is_left,
                    ui.visuals().text_color(),
                );
                if dock.on_hover_text(label).clicked() {
                    self.quick.editor_left = target_is_left;
                }
            });
        });
        let package = self.package.as_deref().unwrap_or(locale.pick(
            "Identificando aplicativo…",
            "Identifying app…",
            "Identificando aplicación…",
        ));
        ui.add(egui::Label::new(egui::RichText::new(package).size(11.).weak()).truncate())
            .on_hover_text(package);
        ui.separator();

        let edit_enabled = self.package.is_some();
        ui.add_enabled_ui(edit_enabled, |ui| self.profile_controls(ui, locale));
        ui.separator();
        let body_height = (footer.top()
            - ui.next_widget_position().y
            - 10.
            - if self.error.is_some() { 20. } else { 0. })
        .max(44.);
        egui::ScrollArea::vertical()
            .id_salt("mapping-editor-scroll")
            .max_height(body_height)
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: false,
                ..egui::scroll_area::ScrollSource::ALL
            })
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(edit_enabled, |ui| {
                    self.placement_hint(ui, locale);
                    if self.place_stick {
                        ui.add(
                            egui::Slider::new(&mut self.draft.radius, 0.035..=0.22)
                                .text(locale.pick("Raio", "Radius", "Radio")),
                        );
                    }
                    if self.selected_control.is_some() {
                        section(
                            ui,
                            locale.pick(
                                "Configuração do controle",
                                "Control settings",
                                "Ajustes del control",
                            ),
                        );
                        self.binding_settings(ui, locale);
                        ui.separator();
                    }
                    self.gesture_tools(ui, locale);
                    if self.selected_control.is_none() && !self.draft.controls.is_empty() {
                        ui.separator();
                        self.binding_settings(ui, locale);
                    }
                });
            });
        if let Some(error) = &self.error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                locale.pick(
                    "Falha no mapeamento",
                    "Mapping error",
                    "Error de asignación",
                ),
            )
            .on_hover_text(error);
        }
        ui.painter().hline(
            bounds.x_range(),
            footer.top() - 5.,
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        let mut footer_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(footer)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        {
            let ui = &mut footer_ui;
            let width = (footer.width() - ui.spacing().item_spacing.x) * 0.5;
            let size = egui::vec2(width, 33.);
            if ui
                .add_sized(
                    size,
                    egui::Button::new(locale.pick("Cancelar", "Cancel", "Cancelar")),
                )
                .clicked()
            {
                self.toggle();
            }
            let save = egui::RichText::new(locale.pick("Salvar", "Save", "Guardar"))
                .strong()
                .color(egui::Color32::from_rgb(22, 28, 36));
            if ui
                .add_enabled(
                    edit_enabled && !self.load_failed,
                    egui::Button::new(save).min_size(size).fill(theme::ACCENT),
                )
                .clicked()
            {
                self.save();
            }
        }
    }

    fn profile_controls(&mut self, ui: &mut egui::Ui, locale: Locale) {
        let mut profile = self.draft_profiles.active.clone();
        let label = section(
            ui,
            locale.pick(
                "Esquema de controle",
                "Control scheme",
                "Esquema de control",
            ),
        );
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("mapping-profile")
                .width((ui.available_width() - 38.).max(75.))
                .truncate()
                .selected_text(profile_name(&profile, locale))
                .show_ui(ui, |ui| {
                    for name in self.draft_profiles.layouts.keys() {
                        ui.selectable_value(&mut profile, name.clone(), profile_name(name, locale));
                    }
                })
                .response
                .labelled_by(label.id)
                .on_hover_text(profile_name(&profile, locale));
            if profile != self.draft_profiles.active {
                self.select_profile(&profile);
                self.editor.confirm_delete = false;
            }
            let more = locale.pick(
                "Opções do esquema",
                "Scheme options",
                "Opciones del esquema",
            );
            let menu = ui.menu_button(egui::RichText::new("…").strong(), |ui| {
                ui.menu_button(
                    locale.pick("Novo perfil", "New profile", "Nuevo perfil"),
                    |ui| self.new_profile(ui, locale, false),
                );
                ui.menu_button(locale.pick("Duplicar", "Duplicate", "Duplicar"), |ui| {
                    self.new_profile(ui, locale, true)
                });
                ui.separator();
                ui.add_enabled_ui(self.editor.file_picker.is_none(), |ui| {
                    for action in [FileAction::Import, FileAction::Export] {
                        if ui.button(action.label(locale)).clicked() {
                            self.pick_profile_file(ui.ctx(), action, locale);
                            ui.close();
                        }
                    }
                });
                if self.draft_profiles.layouts.len() > 1
                    && ui
                        .button(locale.pick("Excluir perfil", "Delete profile", "Eliminar perfil"))
                        .clicked()
                {
                    self.editor.confirm_delete = true;
                    ui.close();
                }
            });
            menu.response.on_hover_text(more);
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
                    let response =
                        ui.colored_label(egui::Color32::LIGHT_RED, scheme_error(error, locale));
                    if let SchemeError::File(details) = error {
                        response.on_hover_text(details);
                    }
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
        section(
            ui,
            locale.pick("Adicionar controles", "Add controls", "Añadir controles"),
        );
        ui.weak(locale.pick(
            "Arraste um controle para a tela.",
            "Drag a control onto the screen.",
            "Arrastra un control a la pantalla.",
        ));
        let labels = [
            locale.pick("Toque mantido", "Held touch", "Toque mantenido"),
            locale.pick("Toque rápido", "Tap", "Toque rápido"),
            locale.pick("Toque repetido", "Repeated taps", "Toques repetidos"),
            locale.pick("Deslizar", "Swipe", "Deslizar"),
            locale.pick("Direcional", "D-pad", "Cruceta"),
            locale.pick("Mira ou câmera", "Aim or camera", "Mira o cámara"),
            locale.pick("Habilidade MOBA", "MOBA ability", "Habilidad MOBA"),
            locale.pick("Pinça de zoom", "Pinch zoom", "Zoom con pellizco"),
            locale.pick("Girar", "Rotate", "Girar"),
            locale.pick("Roda do mouse", "Mouse wheel", "Rueda del ratón"),
        ];
        let width = ((ui.available_width() - 2. * ui.spacing().item_spacing.x) / 3.).max(46.);
        for row in (0..labels.len()).step_by(3) {
            ui.horizontal(|ui| {
                for (index, &name) in labels.iter().enumerate().skip(row).take(3) {
                    let selected = self.placing_control && self.editor.active_tile == index;
                    let response = tiles::tile(ui, width, index, name, selected);
                    if response.drag_started() || response.clicked() {
                        let drag = response.drag_started();
                        self.reset_placement();
                        if !selected || drag {
                            self.preset = presets()[index].clone();
                            self.editor.preset_input = if index == 9 { "Wheel:Up" } else { "" };
                            self.editor.active_tile = index;
                            self.editor.palette_drag = drag;
                            self.placing_control = true;
                        }
                    }
                }
            });
        }
        ui.menu_button(
            locale.pick(
                "Controles clássicos",
                "Classic controls",
                "Controles clásicos",
            ),
            |ui| {
                if ui.button(locale.pick("Botão", "Button", "Botón")).clicked() {
                    self.reset_placement();
                    self.place_button = true;
                    ui.close();
                }
                if ui
                    .button(locale.pick("Direcional", "Directional control", "Control direccional"))
                    .clicked()
                {
                    self.reset_placement();
                    self.place_stick = true;
                    ui.close();
                }
            },
        );
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
                                binding_input_name(&binding.input, locale)
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
        ui.vertical(|ui| {
            ui.spacing_mut().slider_width = 88.;
            if binding.input.starts_with("Wheel:") {
                let label =
                    ui.label(locale.pick("Roda do mouse", "Mouse wheel", "Rueda del ratón"));
                egui::ComboBox::from_id_salt("mapping-wheel-direction")
                    .selected_text(binding_input_name(&binding.input, locale))
                    .show_ui(ui, |ui| {
                        for input in ["Wheel:Up", "Wheel:Down", "Wheel:Left", "Wheel:Right"] {
                            ui.selectable_value(
                                &mut binding.input,
                                input.to_owned(),
                                binding_input_name(input, locale),
                            );
                        }
                    })
                    .response
                    .labelled_by(label.id);
            }
            if !matches!(binding.gesture, Gesture::Dpad { .. }) {
                let input = if binding.input.starts_with("Wheel:") {
                    locale.pick("Alterar entrada", "Change input", "Cambiar entrada")
                } else if binding.input.is_empty() {
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
                    ui.horizontal_wrapped(|ui| {
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
                    });
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
                "Pressione uma tecla ou botão. Para atribuir o mouse, clique ou role sobre a tela do jogo.",
                "Press a key or controller button. To bind the mouse, click or scroll over the game.",
                "Pulsa una tecla o botón. Para asignar el ratón, haz clic o desplaza sobre el juego.",
            )
        } else if self.pending.is_some() {
            locale.pick(
                "Pressione uma tecla ou um botão para atribuir o toque.",
                "Press a key or button to assign the touch.",
                "Pulsa una tecla o un botón para asignar el toque.",
            )
        } else if self.place_stick {
            locale.pick(
                "Clique no centro do direcional do jogo.",
                "Click the center of the in-game stick.",
                "Haz clic en el centro del mando virtual.",
            )
        } else if self.place_button {
            locale.pick(
                "Clique no botão do jogo para marcar o ponto de toque.",
                "Click the in-game button to mark the touch point.",
                "Haz clic en el botón del juego para marcar el punto táctil.",
            )
        } else {
            return;
        };
        ui.label(egui::RichText::new(hint).color(theme::ACCENT));
    }
}

fn scheme_error(error: &SchemeError, locale: Locale) -> &'static str {
    match error {
        SchemeError::File(_) => locale.pick(
            "Falha ao acessar o arquivo do esquema.",
            "Couldn't access the scheme file.",
            "No se pudo acceder al archivo del esquema.",
        ),
        SchemeError::TooLarge => locale.pick(
            "O arquivo do esquema excede 256 KiB.",
            "The scheme file exceeds 256 KiB.",
            "El archivo del esquema supera los 256 KiB.",
        ),
        SchemeError::InvalidData => locale.pick(
            "O esquema contém dados inválidos ou usa uma versão incompatível.",
            "The scheme contains invalid data or uses an unsupported version.",
            "El esquema contiene datos no válidos o utiliza una versión incompatible.",
        ),
        SchemeError::InvalidName => locale.pick(
            "O nome do esquema é inválido ou já existe.",
            "The scheme name is invalid or already exists.",
            "El nombre del esquema no es válido o ya existe.",
        ),
    }
}

fn section(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(text).size(12.).strong())
}

pub(super) fn profile_name(name: &str, locale: Locale) -> &str {
    if name == "Default" {
        locale.pick("Padrão", "Default", "Predeterminado")
    } else {
        name
    }
}

fn binding_input_name(input: &str, locale: Locale) -> &str {
    match input {
        "Wheel:Up" => locale.pick("Rolar para cima", "Scroll up", "Desplazar arriba"),
        "Wheel:Down" => locale.pick("Rolar para baixo", "Scroll down", "Desplazar abajo"),
        "Wheel:Left" => locale.pick(
            "Rolar para a esquerda",
            "Scroll left",
            "Desplazar a la izquierda",
        ),
        "Wheel:Right" => locale.pick(
            "Rolar para a direita",
            "Scroll right",
            "Desplazar a la derecha",
        ),
        _ => input,
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

fn presets() -> [Gesture; 10] {
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
        Gesture::Swipe {
            end: [0.75, 0.5],
            duration_ms: 220,
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
