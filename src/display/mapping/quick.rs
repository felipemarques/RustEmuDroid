use super::{GameProfiles, Gesture, Mapping, Point};
use crate::ui::{i18n::Locale, theme};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, vec2};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct QuickControls {
    pub(super) enabled: bool,
    pub(super) show_hints: bool,
    opacity: f32,
    #[serde(skip)]
    anchor: Option<f32>,
    #[serde(skip)]
    pub(super) editor_left: bool,
    #[serde(skip)]
    path: PathBuf,
}

impl Default for QuickControls {
    fn default() -> Self {
        Self {
            enabled: true,
            show_hints: false,
            opacity: 0.8,
            anchor: None,
            editor_left: false,
            path: PathBuf::new(),
        }
    }
}

impl QuickControls {
    pub(super) fn load(&mut self, maps: &Path) {
        let path = maps.with_file_name("control-overlay.json");
        if let Some(saved) = std::fs::File::open(&path)
            .ok()
            .and_then(|f| serde_json::from_reader(f).ok())
        {
            *self = saved;
        }
        self.opacity = self.opacity.clamp(0.15, 1.);
        self.path = path;
    }
    pub(crate) fn is_open(&self) -> bool {
        self.anchor.is_some()
    }
    pub(crate) fn preview_hints(&self) -> bool {
        self.is_open() && self.enabled && self.show_hints
    }
}

impl Mapping {
    pub(crate) fn toggle_quick(&mut self, anchor: f32) {
        if self.editing {
            return;
        }
        self.quick.anchor = if self.quick.is_open() {
            None
        } else {
            Some(anchor)
        };
    }

    pub(crate) fn close_quick(&mut self) {
        self.quick.anchor = None;
    }

    pub(crate) fn panel_rect(&self, viewport: Rect) -> Option<Rect> {
        let bounds = viewport.shrink(7.);
        let (width, height, y) = if self.editing {
            (
                310_f32.min(bounds.width() * 0.52),
                bounds.height(),
                bounds.top(),
            )
        } else {
            let anchor = self.quick.anchor?;
            let h = 278_f32.min(bounds.height());
            (
                272_f32.min(bounds.width()),
                h,
                anchor.clamp(bounds.top(), bounds.bottom() - h),
            )
        };
        let x = if self.editing && self.quick.editor_left {
            bounds.left()
        } else {
            bounds.right() - width
        };
        Some(Rect::from_min_size(egui::pos2(x, y), vec2(width, height)))
    }

    pub(crate) fn floating_panel(&mut self, ctx: &egui::Context, bounds: Rect, locale: Locale) {
        let Some(rect) = self.panel_rect(bounds) else {
            return;
        };
        let editor = self.editing;
        let popup_open = egui::Popup::is_any_open(ctx);
        let escape = !popup_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if escape {
            if editor {
                if !self.cancel_edit_step() {
                    self.toggle();
                }
            } else {
                self.close_quick();
            }
            return;
        }
        if !editor
            && !popup_open
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| bounds.contains(p) && !rect.contains(p))
            })
        {
            self.close_quick();
            return;
        }
        let id = if editor {
            "floating-controls-editor"
        } else {
            "quick-game-controls"
        };
        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Foreground)
            .movable(false)
            .constrain(false)
            .fixed_pos(rect.min)
            .default_size(rect.size())
            .show(ctx, |ui| {
                let card = egui::Frame::new()
                    .fill(Color32::from_rgb(26, 35, 47))
                    .stroke(Stroke::new(1_f32, Color32::from_rgb(66, 80, 95)))
                    .corner_radius(11)
                    .inner_margin(12)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 3],
                        blur: 12,
                        spread: 0,
                        color: Color32::from_black_alpha(88),
                    });
                let (bounds, _) = ui.allocate_exact_size(rect.size(), egui::Sense::hover());
                let inner = bounds - card.total_margin();
                ui.painter().add(card.paint(inner));
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(inner));
                content.set_clip_rect(inner);
                if editor {
                    self.control_editor(&mut content, locale, inner.size());
                } else {
                    self.quick_menu(&mut content, locale);
                }
            });
    }

    fn quick_menu(&mut self, ui: &mut egui::Ui, l: Locale) {
        ui.spacing_mut().item_spacing.y = 6.;
        ui.horizontal(|ui| {
            ui.heading(l.pick("Controles do jogo", "Game controls", "Controles del juego"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("×")
                    .on_hover_text(l.pick("Fechar", "Close", "Cerrar"))
                    .clicked()
                {
                    self.close_quick();
                }
            });
        });
        ui.separator();
        let changed = ui
            .checkbox(
                &mut self.quick.enabled,
                l.pick(
                    "Ativar mapeamento",
                    "Enable mapping",
                    "Activar asignaciones",
                ),
            )
            .changed();
        let hints_changed = ui
            .checkbox(
                &mut self.quick.show_hints,
                l.pick(
                    "Mostrar teclas no jogo",
                    "Show keys in game",
                    "Mostrar teclas en el juego",
                ),
            )
            .changed();
        let opacity = ui
            .add_enabled_ui(self.quick.show_hints, |ui| {
                ui.horizontal(|ui| {
                    ui.label(l.pick("Opacidade", "Opacity", "Opacidad"));
                    ui.add(egui::Slider::new(&mut self.quick.opacity, 0.15..=1.).show_value(false))
                })
                .inner
            })
            .inner;
        if (changed
            || hints_changed
            || opacity.drag_stopped()
            || (opacity.changed() && !opacity.dragged()))
            && let Err(err) = crate::ui::write_json(&self.quick.path, &self.quick)
        {
            self.error = Some(err.to_string());
        }
        ui.separator();
        ui.small(l.pick("Esquema do jogo", "Game scheme", "Esquema del juego"));
        let profiles = self.package.as_ref().and_then(|p| self.profiles.get(p));
        let current = profiles.map(|p| p.active.as_str()).unwrap_or("Default");
        let mut picked = None;
        egui::ComboBox::from_id_salt("quick-scheme")
            .width(ui.available_width())
            .selected_text(super::editor::profile_name(current, l))
            .show_ui(ui, |ui| {
                if let Some(game) = profiles {
                    for name in game.layouts.keys() {
                        if ui
                            .selectable_label(name == current, super::editor::profile_name(name, l))
                            .clicked()
                        {
                            picked = Some(name.clone());
                        }
                    }
                } else {
                    ui.label(super::editor::profile_name(current, l));
                }
            });
        if let Some(name) = picked
            && let Some(package) = &self.package
        {
            let mut next = self.profiles.clone();
            if let Some(game) = next.get_mut(package) {
                game.active = name;
                match crate::ui::write_json(&self.path, &next) {
                    Ok(()) => {
                        self.profiles = next;
                        self.reset_draft();
                    }
                    Err(err) => self.error = Some(err.to_string()),
                }
            }
        }
        ui.add_space(3.);
        if theme::primary(
            ui,
            true,
            l.pick(
                "Editor de controles",
                "Controls editor",
                "Editor de controles",
            ),
        )
        .clicked()
        {
            self.close_quick();
            self.toggle();
        }
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    pub(crate) fn paint_hints(&self, ui: &egui::Ui, rect: Rect) {
        let Some(layout) = self
            .package
            .as_ref()
            .and_then(|p| self.profiles.get(p))
            .and_then(GameProfiles::layout)
        else {
            return;
        };
        let mut paint = ui.painter().with_clip_rect(rect);
        paint.set_opacity(self.quick.opacity);
        let pos = |p: Point| rect.min + rect.size() * vec2(p[0], p[1]);
        let badge = |at: Pos2, key: &str| {
            if key.is_empty() {
                return;
            }
            let label = key.strip_prefix("Pad:").unwrap_or(key);
            let font = FontId::proportional(13.);
            let text = paint.layout_no_wrap(label.to_owned(), font, Color32::WHITE);
            let r = Rect::from_center_size(at, text.size() + vec2(12., 6.));
            paint.rect_filled(r, 6., Color32::from_rgb(26, 35, 47));
            paint.rect_stroke(
                r,
                6.,
                Stroke::new(1_f32, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
            paint.galley(
                Align2::CENTER_CENTER.anchor_size(at, text.size()).min,
                text,
                Color32::WHITE,
            );
        };
        for (key, point) in &layout.buttons {
            badge(pos(*point), key);
        }
        let pad = |p: Point, radius: f32, directions: [&str; 4]| {
            let center = pos(p);
            let r = radius * rect.height().min(rect.width());
            for (delta, key) in [vec2(0., -1.), vec2(0., 1.), vec2(-1., 0.), vec2(1., 0.)]
                .into_iter()
                .zip(directions)
            {
                badge(center + delta * (r * 0.64).max(26.), key);
            }
        };
        if let Some(stick) = layout.stick {
            pad(stick, layout.radius, ["W", "S", "A", "D"]);
        }
        for binding in &layout.controls {
            if let Gesture::Dpad {
                directions, radius, ..
            } = &binding.gesture
            {
                pad(
                    binding.at,
                    *radius,
                    directions.each_ref().map(String::as_str),
                );
            } else {
                badge(pos(binding.at), &binding.input);
            }
        }
    }
}
