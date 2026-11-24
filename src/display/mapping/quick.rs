use super::{
    GameProfiles, Gesture, Layout, Mapping, Point,
    gestures::{DpadStick, dpad_radius},
    label,
};
use crate::ui::{i18n::Locale, theme};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, vec2};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HintDevice {
    #[default]
    KeyboardMouse,
    Gamepad,
}

impl HintDevice {
    fn binding(self, input: &str) -> Option<&str> {
        if input.is_empty() || input.starts_with("Pad:") != (self == Self::Gamepad) {
            return None;
        }
        Some(match input {
            "Pad:DPadUp" => "D-pad ↑",
            "Pad:DPadDown" => "D-pad ↓",
            "Pad:DPadLeft" => "D-pad ←",
            "Pad:DPadRight" => "D-pad →",
            "Pad:LeftTrigger" => "LB / L1",
            "Pad:RightTrigger" => "RB / R1",
            "Pad:LeftTrigger2" => "LT / L2",
            "Pad:RightTrigger2" => "RT / R2",
            "Pad:LeftThumb" => "L3",
            "Pad:RightThumb" => "R3",
            "Mouse:Primary" => "M1",
            "Mouse:Middle" => "M3",
            "Wheel:Up" => "↑",
            "Wheel:Down" => "↓",
            _ => label(input),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct QuickControls {
    pub(super) enabled: bool,
    pub(super) show_hints: bool,
    opacity: f32,
    hint_device: HintDevice,
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
            hint_device: HintDevice::default(),
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
        if self.quick.is_open() {
            self.close_quick();
        } else {
            self.reset_draft();
            self.quick.anchor = Some(anchor);
        }
    }

    pub(crate) fn close_quick(&mut self) {
        self.quick.anchor = None;
    }

    pub(crate) fn panel_rect(&self, viewport: Rect) -> Option<Rect> {
        let bounds = viewport.shrink(7.);
        let (width, height, y) = if self.editing {
            (
                310_f32.min(bounds.width() * 0.48),
                bounds.height(),
                bounds.top(),
            )
        } else {
            let anchor = self.quick.anchor?;
            let h = 360_f32.min(bounds.height());
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
                    egui::ScrollArea::vertical()
                        .max_height(inner.height())
                        .show(&mut content, |ui| self.quick_menu(ui, locale));
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
                    "Mostrar controles no jogo",
                    "Show controls in game",
                    "Mostrar controles en el juego",
                ),
            )
            .changed();
        let device_changed = ui
            .horizontal(|ui| {
                let tool_tip = l.pick(
                    "Escolhe quais controles são mostrados no jogo.",
                    "Choose which controls appear on the game screen.",
                    "Elige qué controles se muestran en el juego.",
                );
                let keyboard = ui
                    .selectable_value(
                        &mut self.quick.hint_device,
                        HintDevice::KeyboardMouse,
                        l.pick("Teclado e mouse", "Keyboard and mouse", "Teclado y ratón"),
                    )
                    .on_hover_text(tool_tip);
                let gamepad = ui
                    .selectable_value(
                        &mut self.quick.hint_device,
                        HintDevice::Gamepad,
                        l.pick("Controle", "Gamepad", "Mando"),
                    )
                    .on_hover_text(tool_tip);
                keyboard.changed() || gamepad.changed()
            })
            .inner;
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
            || device_changed
            || opacity.drag_stopped()
            || (opacity.changed() && !opacity.dragged()))
            && let Err(err) = crate::ui::write_json(&self.quick.path, &self.quick)
        {
            self.error = Some(err.to_string());
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.small(l.pick("Perfil do jogo", "Game profile", "Perfil del juego"));
            if ui
                .add_enabled(
                    self.package.is_some() && !self.load_failed,
                    egui::Button::new(super::editor::manage_label(l)),
                )
                .clicked()
            {
                self.close_quick();
                self.toggle();
                self.open_profile_manager();
            }
        });
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
        self.mouse_sensitivity_ui(ui, l);
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

    pub(crate) fn paint_hints(&self, ui: &egui::Ui, rect: Rect, locale: Locale) {
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
        visit_hints(layout, rect, self.quick.hint_device, locale, |at, label| {
            let font = FontId::proportional(13.);
            let text = paint.layout_no_wrap(label.to_owned(), font, Color32::WHITE);
            let r = Rect::from_center_size(at, text.size() + vec2(12., 6.));
            let (fill, border) = match self.quick.hint_device {
                HintDevice::KeyboardMouse => (Color32::from_rgb(26, 35, 47), theme::ACCENT),
                HintDevice::Gamepad => (
                    Color32::from_rgb(26, 45, 53),
                    Color32::from_rgb(115, 218, 199),
                ),
            };
            paint.rect_filled(r, 6., fill);
            paint.rect_stroke(r, 6., Stroke::new(1_f32, border), egui::StrokeKind::Inside);
            paint.galley(
                Align2::CENTER_CENTER.anchor_size(at, text.size()).min,
                text,
                Color32::WHITE,
            );
        });
    }
}

fn visit_hints(
    layout: &Layout,
    rect: Rect,
    device: HintDevice,
    locale: Locale,
    mut badge: impl FnMut(Pos2, &str),
) {
    fn directions(
        center: Pos2,
        radius: f32,
        keys: [&str; 4],
        device: HintDevice,
        badge: &mut impl FnMut(Pos2, &str),
    ) {
        for (delta, key) in [vec2(0., -1.), vec2(0., 1.), vec2(-1., 0.), vec2(1., 0.)]
            .into_iter()
            .zip(keys)
        {
            if let Some(label) = device.binding(key) {
                badge(center + delta * radius, label);
            }
        }
    }
    let pos = |p: Point| rect.min + rect.size() * vec2(p[0], p[1]);
    let distance = |radius: f32| (radius * (rect.height().min(rect.width()) * 0.64)).max(26.);
    let left_stick = locale.pick("Analógico esq.", "Left stick", "Palanca izq.");
    let right_stick = locale.pick("Analógico dir.", "Right stick", "Palanca der.");
    let gamepad = device == HintDevice::Gamepad;
    for (key, point) in &layout.buttons {
        if let Some(label) = device.binding(key) {
            badge(pos(*point), label);
        }
    }
    if let Some(stick) = layout.stick {
        if gamepad {
            badge(pos(stick), left_stick);
        } else {
            directions(
                pos(stick),
                distance(layout.radius),
                ["W", "S", "A", "D"],
                device,
                &mut badge,
            );
        }
    }
    for binding in &layout.controls {
        let at = pos(binding.at);
        let (first, second) = match &binding.gesture {
            Gesture::Dpad {
                directions: keys,
                radius,
                left_stick: left,
                right_stick: right,
            } => {
                let radius = distance(dpad_radius(*radius));
                directions(
                    at,
                    radius,
                    keys.each_ref().map(String::as_str),
                    device,
                    &mut badge,
                );
                let stick = if gamepad {
                    match DpadStick::from_flags(*left, *right) {
                        DpadStick::Off => None,
                        DpadStick::Left => Some(left_stick),
                        DpadStick::Right => Some(right_stick),
                    }
                } else {
                    None
                };
                if let Some(stick) = stick {
                    let offset = if keys.iter().any(|key| device.binding(key).is_some()) {
                        let space = radius + 22.;
                        if at.y + space + 12. <= rect.bottom() {
                            space
                        } else {
                            -space
                        }
                    } else {
                        0.
                    };
                    badge(at + vec2(0., offset), stick);
                }
                continue;
            }
            Gesture::FreeLook {
                mouse: false,
                directions: keys,
                ..
            } => {
                directions(
                    at,
                    distance(0.10),
                    keys.each_ref().map(String::as_str),
                    device,
                    &mut badge,
                );
                (None, None)
            }
            Gesture::Aim {
                right_stick: true, ..
            } if gamepad => (device.binding(&binding.input), Some(right_stick)),
            Gesture::Rotate {
                wheel: Some(wheel), ..
            } => (
                device.binding(&binding.input),
                device.binding(&wheel.reverse_input),
            ),
            Gesture::MobaMove { .. } => (
                device.binding("Mouse:Secondary"),
                device.binding(&binding.input),
            ),
            Gesture::Scroll { .. } => (
                (!gamepad).then_some(locale.pick("Roda ↑/↓", "Wheel ↑/↓", "Rueda ↑/↓")),
                None,
            ),
            Gesture::EdgeScroll { .. } => (
                (!gamepad).then_some(locale.pick(
                    "Borda da tela",
                    "Screen edge",
                    "Borde de la pantalla",
                )),
                None,
            ),
            _ => (device.binding(binding.primary_input()), None),
        };
        if let (Some(first), Some(second)) = (first, second) {
            badge(at - vec2(0., 12.), first);
            badge(at + vec2(0., 12.), second);
        } else if let Some(label) = first.or(second) {
            badge(at, label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::mapping::Binding;

    #[test]
    fn old_overlay_defaults_to_keyboard_and_remembers_selected_gamepad() {
        let mut quick: QuickControls =
            serde_json::from_str(r#"{"enabled":false,"show_hints":true,"opacity":0.4}"#).unwrap();
        assert!(!quick.enabled && quick.show_hints);
        assert_eq!(quick.opacity, 0.4);
        assert_eq!(quick.hint_device, HintDevice::KeyboardMouse);
        quick.hint_device = HintDevice::Gamepad;
        let restored: QuickControls =
            serde_json::from_value(serde_json::to_value(quick).unwrap()).unwrap();
        assert_eq!(restored.hint_device, HintDevice::Gamepad);
    }

    #[test]
    fn mixed_bindings_filter_hints_without_changing_input_delivery() {
        let layout = Layout {
            buttons: [
                ("Space".into(), [0.5, 0.7]),
                ("Pad:South".into(), [0.6, 0.7]),
            ]
            .into(),
            stick: Some([0.2, 0.8]),
            radius: 0.12,
            controls: vec![
                Binding {
                    input: String::new(),
                    at: [0.8, 0.8],
                    gesture: Gesture::Dpad {
                        directions: [
                            "Up".into(),
                            "Pad:DPadDown".into(),
                            String::new(),
                            String::new(),
                        ],
                        radius: 0.15,
                        left_stick: false,
                        right_stick: false,
                    },
                },
                Binding {
                    input: String::new(),
                    at: [0.8, 0.2],
                    gesture: Gesture::Dpad {
                        directions: Default::default(),
                        radius: 0.15,
                        left_stick: false,
                        right_stick: true,
                    },
                },
            ],
            ..Default::default()
        };
        let hints = |device, locale| {
            let rect = Rect::from_min_max(Pos2::ZERO, egui::pos2(1000., 600.));
            let mut hints = Vec::new();
            visit_hints(&layout, rect, device, locale, |_, badge| {
                hints.push(badge.to_owned())
            });
            hints
        };
        assert_eq!(
            hints(HintDevice::KeyboardMouse, Locale::EnUs),
            ["Space", "W", "S", "A", "D", "Up"]
        );
        assert_eq!(
            hints(HintDevice::Gamepad, Locale::EnUs),
            ["A / ×", "Left stick", "D-pad ↓", "Right stick"]
        );
        assert_eq!(
            hints(HintDevice::Gamepad, Locale::PtBr)[3],
            "Analógico dir."
        );
        assert_eq!(hints(HintDevice::Gamepad, Locale::EsEs)[3], "Palanca der.");

        let mut mapping = Mapping::default();
        mapping.quick.hint_device = HintDevice::Gamepad;
        mapping.package = Some("example.game".into());
        mapping
            .profiles
            .insert("example.game".into(), GameProfiles::from_layout(layout));
        let mut output = Vec::new();
        for input in ["Space", "Pad:South"] {
            mapping.control(input, true, &mut output);
            mapping.control(input, false, &mut output);
        }
        assert_eq!(
            output
                .into_iter()
                .map(|c| (c.pos, c.down))
                .collect::<Vec<_>>(),
            [
                ([0.5, 0.7], true),
                ([0.5, 0.7], false),
                ([0.6, 0.7], true),
                ([0.6, 0.7], false)
            ]
        );
    }
}
