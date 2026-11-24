use super::{
    Editor, FileAction, Locale, Mapping, SchemeError, egui, profile_name, scheme_error, section,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileAction {
    Create,
    Duplicate,
    Rename,
    Delete,
}

impl ProfileAction {
    fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Create => locale.pick("Novo perfil", "New profile", "Nuevo perfil"),
            Self::Duplicate => locale.pick("Duplicar", "Duplicate", "Duplicar"),
            Self::Rename => locale.pick("Renomear", "Rename", "Renombrar"),
            Self::Delete => locale.pick("Excluir perfil", "Delete profile", "Eliminar perfil"),
        }
    }
}

pub(in crate::display::mapping) fn manage_label(locale: Locale) -> &'static str {
    locale.pick("Gerenciar perfis", "Manage profiles", "Gestionar perfiles")
}

impl Editor {
    pub(in crate::display::mapping) fn has_pending_name(&self) -> bool {
        self.profile_action.is_some() && !self.profile_name.trim().is_empty()
    }
}

impl Mapping {
    pub(crate) fn open_profile_manager(&mut self) {
        self.reset_placement();
        self.editor.managing_profiles = true;
    }

    pub(in crate::display::mapping) fn cancel_profile_step(&mut self) -> bool {
        if self.editor.confirm_close {
            self.editor.confirm_close = false;
        } else if self.editor.profile_action.take().is_some() {
            self.editor.profile_name.clear();
        } else if self.editor.managing_profiles {
            self.editor.managing_profiles = false;
        } else {
            return false;
        }
        true
    }

    pub(super) fn profile_controls(&mut self, ui: &mut egui::Ui, locale: Locale) {
        if self.editor.managing_profiles {
            ui.horizontal_wrapped(|ui| {
                section(
                    ui,
                    locale.pick(
                        "Perfis deste jogo",
                        "Profiles for this game",
                        "Perfiles de este juego",
                    ),
                );
                if ui
                    .add_enabled(
                        self.editor.profile_action.is_none(),
                        egui::Button::new(locale.pick(
                            "Editar controles",
                            "Edit controls",
                            "Editar controles",
                        )),
                    )
                    .clicked()
                {
                    self.editor.managing_profiles = false;
                }
            });
            return;
        }
        let label = section(
            ui,
            locale.pick("Perfil do jogo", "Game profile", "Perfil del juego"),
        );
        let mut profile = self.draft_profiles.active.clone();
        ui.add_enabled_ui(self.editor.profile_action.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                let button = manage_label(locale);
                let font = egui::TextStyle::Button.resolve(ui.style());
                let button_width = ui
                    .painter()
                    .layout_no_wrap(button.to_owned(), font, ui.visuals().text_color())
                    .size()
                    .x
                    + 2. * ui.spacing().button_padding.x
                    + ui.spacing().item_spacing.x;
                egui::ComboBox::from_id_salt("mapping-profile")
                    .width((ui.available_width() - button_width - 2.).max(72.))
                    .truncate()
                    .selected_text(profile_name(&profile, locale))
                    .show_ui(ui, |ui| {
                        for name in self.draft_profiles.layouts.keys() {
                            ui.selectable_value(
                                &mut profile,
                                name.clone(),
                                profile_name(name, locale),
                            );
                        }
                    })
                    .response
                    .labelled_by(label.id)
                    .on_hover_text(profile_name(&profile, locale));
                if profile != self.draft_profiles.active {
                    self.select_profile(&profile);
                    self.editor.file_notice = None;
                }
                if ui.button(button).clicked() {
                    self.open_profile_manager();
                }
            });
        });
    }

    pub(super) fn profile_manager(&mut self, ui: &mut egui::Ui, locale: Locale) {
        use ProfileAction::*;
        if let Some(action) = self.editor.profile_action {
            if action == Delete {
                ui.label(format!(
                    "{} “{}”?",
                    action.label(locale),
                    profile_name(&self.draft_profiles.active, locale)
                ));
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(locale.pick("Excluir", "Delete", "Eliminar"))
                        .clicked()
                    {
                        self.delete_profile();
                        self.editor.profile_action = None;
                    }
                    self.cancel_profile_button(ui, locale);
                });
            } else {
                section(ui, action.label(locale));
                if action != Create {
                    ui.weak(profile_name(&self.draft_profiles.active, locale));
                }
                self.profile_name_form(ui, locale, action);
            }
            return;
        }
        let mut selection = None;
        egui::ScrollArea::vertical()
            .id_salt("game-profile-list")
            .max_height(130.)
            .show(ui, |ui| {
                for name in self.draft_profiles.layouts.keys() {
                    let label = profile_name(name, locale);
                    let row = egui::Button::selectable(self.draft_profiles.active == *name, label)
                        .min_size(egui::vec2(ui.available_width(), 27.))
                        .truncate();
                    if ui.add(row).on_hover_text(label).clicked() {
                        selection = Some(name.clone());
                    }
                }
            });
        if let Some(name) = selection
            && name != self.draft_profiles.active
        {
            self.select_profile(&name);
            self.editor.file_notice = None;
        }
        ui.weak(locale.pick(
            "Cada perfil guarda as posições e teclas deste jogo. Salve para usar o perfil selecionado.",
            "Each profile stores this game’s control positions and bindings. Save to use the selected profile.",
            "Cada perfil guarda las posiciones y asignaciones de este juego. Guarda para usar el perfil seleccionado.",
        ));
        let button_width =
            ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5 - 1.).max(40.);
        for pair in [[Create, Duplicate], [Rename, Delete]] {
            ui.horizontal_wrapped(|ui| {
                for action in pair {
                    let enabled = action != Delete || self.draft_profiles.layouts.len() > 1;
                    let response = ui.add_enabled(
                        enabled,
                        egui::Button::new(action.label(locale))
                            .min_size(egui::vec2(button_width, 29.)),
                    );
                    if response.clicked() {
                        self.editor.profile_name = if action == Rename {
                            profile_name(&self.draft_profiles.active, locale).to_owned()
                        } else {
                            String::new()
                        };
                        self.editor.focus_name = action != Delete;
                        self.editor.profile_action = Some(action);
                        self.editor.file_notice = None;
                    } else if !enabled {
                        response.on_disabled_hover_text(locale.pick(
                            "Mantenha pelo menos um perfil para este jogo.",
                            "Keep at least one profile for this game.",
                            "Mantén al menos un perfil para este juego.",
                        ));
                    }
                }
            });
        }
        ui.separator();
        ui.add_enabled_ui(self.editor.file_picker.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for action in [FileAction::Import, FileAction::Export] {
                    if ui.button(action.label(locale)).clicked() {
                        self.pick_profile_file(ui.ctx(), action, locale);
                    }
                }
            });
        });
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

    fn profile_name_form(&mut self, ui: &mut egui::Ui, locale: Locale, action: ProfileAction) {
        let label = ui.label(locale.pick("Nome do perfil", "Profile name", "Nombre del perfil"));
        let id = ui.make_persistent_id("game-profile-name");
        if std::mem::take(&mut self.editor.focus_name) {
            ui.memory_mut(|memory| memory.request_focus(id));
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            let end = self.editor.profile_name.chars().count();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(end),
                )));
            egui::TextEdit::store_state(ui.ctx(), id, state);
        }
        let input = ui
            .add(
                egui::TextEdit::singleline(&mut self.editor.profile_name)
                    .id(id)
                    .char_limit(48)
                    .desired_width(ui.available_width()),
            )
            .labelled_by(label.id);
        let name = self.editor.profile_name.trim().to_owned();
        let unchanged = action == ProfileAction::Rename
            && name == profile_name(&self.draft_profiles.active, locale);
        let available = !unchanged
            && self.name_is_available(&name)
            && !self
                .draft_profiles
                .layouts
                .keys()
                .any(|key| profile_name(key, locale) == name);
        if !available && !unchanged && !name.is_empty() {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                scheme_error(&SchemeError::InvalidName, locale),
            );
        }
        ui.horizontal_wrapped(|ui| {
            let submit = ui
                .add_enabled(
                    available,
                    egui::Button::new(locale.pick("Aplicar", "Apply", "Aplicar")),
                )
                .clicked()
                || (available
                    && input.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if submit {
                match action {
                    ProfileAction::Create => self.create_profile(&name),
                    ProfileAction::Duplicate => self.copy_profile(&name),
                    ProfileAction::Rename => self.rename_profile(&name),
                    ProfileAction::Delete => unreachable!(),
                };
                self.editor.profile_action = None;
                self.editor.profile_name.clear();
            }
            self.cancel_profile_button(ui, locale);
        });
    }

    fn cancel_profile_button(&mut self, ui: &mut egui::Ui, locale: Locale) {
        if ui.button(locale.pick("Voltar", "Back", "Volver")).clicked() {
            self.editor.profile_action = None;
            self.editor.profile_name.clear();
        }
    }

    pub(super) fn confirm_close_panel(&mut self, ui: &mut egui::Ui, locale: Locale) {
        ui.strong(locale.pick(
            "Descartar as alterações?",
            "Discard your changes?",
            "¿Descartar los cambios?",
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(locale.pick("Continuar editando", "Continue editing", "Seguir editando"))
                .clicked()
            {
                self.editor.confirm_close = false;
            }
            if ui
                .button(locale.pick("Descartar", "Discard", "Descartar"))
                .clicked()
            {
                self.close_editor();
            }
        });
    }
}
