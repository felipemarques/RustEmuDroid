use eframe::egui::{self, Color32, Key, Rect, Vec2, vec2};
use gilrs::{Axis, Button, EventType, Gilrs};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

type Point = [f32; 2];

#[derive(Default, Clone, Serialize, Deserialize)]
struct Layout {
    buttons: BTreeMap<String, Point>,
    stick: Option<Point>,
    #[serde(default = "default_radius")]
    radius: f32,
}
fn default_radius() -> f32 {
    0.10
}

#[derive(Clone, Copy)]
pub(super) struct Contact {
    pub id: i32,
    pub pos: Point,
    pub down: bool,
}

#[derive(Default)]
pub(crate) struct Mapping {
    profiles: BTreeMap<String, Layout>,
    path: PathBuf,
    package: Option<String>,
    pub editing: bool,
    draft: Layout,
    pending: Option<Point>,
    place_stick: bool,
    error: Option<String>,
    gamepad: Option<Gilrs>,
    pad_initialized: bool,
    active: HashMap<String, Contact>,
    directions: HashSet<String>,
    analog: [f32; 2],
    stick_touch: Option<Contact>,
}

impl Mapping {
    pub fn load(&mut self, path: PathBuf) {
        if let Ok(file) = std::fs::File::open(&path) {
            match serde_json::from_reader(file) {
                Ok(profiles) => self.profiles = profiles,
                Err(error) => self.error = Some(format!("Falha ao ler mapeamentos: {error}")),
            }
        }
        self.path = path;
    }

    pub fn wants_package(&self) -> bool {
        self.editing || !self.profiles.is_empty()
    }
    pub(super) fn package_changed(&self, package: Option<&str>) -> bool {
        self.package.as_deref() != package
    }
    pub(super) fn select(&mut self, package: Option<&str>) {
        self.package = package.map(str::to_owned);
        self.reset_draft();
    }
    fn reset_draft(&mut self) {
        self.draft = self
            .package
            .as_ref()
            .and_then(|p| self.profiles.get(p))
            .cloned()
            .unwrap_or_else(|| Layout {
                radius: default_radius(),
                ..Default::default()
            });
        self.pending = None;
        self.place_stick = false;
    }
    pub(super) fn toggle(&mut self) {
        self.editing = !self.editing;
        self.reset_draft();
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Mapeamento");
            ui.label(
                self.package
                    .as_deref()
                    .unwrap_or("Identificando aplicativo…"),
            );
            ui.separator();
            if ui.selectable_label(!self.place_stick, "Botão").clicked() {
                self.place_stick = false;
                self.pending = None;
            }
            if ui
                .selectable_label(self.place_stick, "Direcional")
                .clicked()
            {
                self.place_stick = true;
                self.pending = None;
            }
            if self.place_stick {
                ui.add(egui::Slider::new(&mut self.draft.radius, 0.035..=0.22).text("raio"));
            }
            if crate::ui::theme::primary(ui, self.package.is_some(), "Salvar").clicked() {
                let mut next = self.profiles.clone();
                let package = self.package.clone().unwrap();
                if self.draft.buttons.is_empty() && self.draft.stick.is_none() {
                    next.remove(&package);
                } else {
                    next.insert(package, self.draft.clone());
                }
                match crate::ui::write_json(&self.path, &next) {
                    Ok(()) => {
                        self.profiles = next;
                        self.editing = false;
                        self.pending = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            if ui.add(crate::ui::theme::button("Cancelar")).clicked() {
                self.editing = false;
                self.reset_draft();
            }
        });
        ui.small(if self.place_stick { "Clique no centro do direcional do jogo para usar WASD ou o analógico esquerdo. Clique direito remove." }
            else if self.pending.is_some() { "Pressione a tecla ou o botão do controle que deve tocar nesse ponto." }
            else { "Clique em um botão do jogo e pressione uma tecla ou botão do controle. Clique direito em uma marca para removê-la." });
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    fn layout(&self) -> Option<&Layout> {
        self.package.as_ref().and_then(|p| self.profiles.get(p))
    }
    pub fn captures(&self, key: Key) -> bool {
        self.layout().is_some_and(|layout| {
            layout.buttons.contains_key(key.name())
                || (layout.stick.is_some() && direction(key.name()).is_some())
        })
    }

    pub(super) fn release(&mut self) -> Vec<Contact> {
        self.directions.clear();
        self.analog = [0.; 2];
        let mut ups: Vec<_> = self
            .active
            .drain()
            .map(|(_, c)| Contact { down: false, ..c })
            .collect();
        if let Some(c) = self.stick_touch.take() {
            ups.push(Contact { down: false, ..c });
        }
        if let Some(pad) = &mut self.gamepad {
            while pad.next_event().is_some() {}
        }
        ups
    }

    fn control(&mut self, input: &str, down: bool, changes: &mut Vec<Contact>) {
        if self.editing {
            if down && let Some(point) = self.pending.take() {
                self.draft.buttons.insert(input.to_owned(), point);
            }
            return;
        }
        // Exact button bindings take precedence over the directional preset.
        if let Some(&pos) = self.layout().and_then(|map| map.buttons.get(input)) {
            if down && !self.active.contains_key(input) {
                if let Some(id) = (1..=8).find(|id| !self.active.values().any(|c| c.id == *id)) {
                    let c = Contact {
                        id,
                        pos,
                        down: true,
                    };
                    self.active.insert(input.to_owned(), c);
                    changes.push(c);
                }
            } else if !down && let Some(c) = self.active.remove(input) {
                changes.push(Contact { down: false, ..c });
            }
        } else if direction(input).is_some() && self.layout().is_some_and(|l| l.stick.is_some()) {
            if down {
                self.directions.insert(input.into());
            } else {
                self.directions.remove(input);
            }
        }
    }

    fn update_stick(&mut self, size: Vec2, changes: &mut Vec<Contact>) {
        let Some((center, radius)) = self.layout().and_then(|map| Some((map.stick?, map.radius)))
        else {
            return;
        };
        let mut movement = vec2(self.analog[0], self.analog[1]);
        if movement.length() < 0.16 {
            movement = Vec2::ZERO;
        }
        for dir in &self.directions {
            movement += direction(dir).unwrap_or_default();
        }
        if movement.length_sq() > 1. {
            movement = movement.normalized();
        }
        if movement == Vec2::ZERO {
            if let Some(c) = self.stick_touch.take() {
                changes.push(Contact { down: false, ..c });
            }
            return;
        }
        let displacement = movement * (radius * size.x.min(size.y));
        let pos = [
            (center[0] + displacement.x / size.x).clamp(0., 1.),
            (center[1] + displacement.y / size.y).clamp(0., 1.),
        ];
        if self.stick_touch.is_none() {
            changes.push(Contact {
                id: 9,
                pos: center,
                down: true,
            });
        }
        if self.stick_touch.is_none_or(|c| c.pos != pos) {
            let c = Contact {
                id: 9,
                pos,
                down: true,
            };
            self.stick_touch = Some(c);
            changes.push(c);
        }
    }

    fn overlay(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let to_screen = |point: Point| rect.min + rect.size() * vec2(point[0], point[1]);
        let color = Color32::from_rgb(255, 175, 94);
        let painter = ui.painter().with_clip_rect(rect);
        for (input, pos) in &self.draft.buttons {
            let p = to_screen(*pos);
            painter.circle_filled(p, 17., Color32::from_black_alpha(200));
            painter.circle_stroke(p, 17., (2., color));
            painter.text(
                p,
                egui::Align2::CENTER_CENTER,
                label(input),
                egui::FontId::proportional(12.),
                Color32::WHITE,
            );
        }
        if let Some(center) = self.draft.stick {
            let p = to_screen(center);
            painter.circle_stroke(
                p,
                self.draft.radius * rect.width().min(rect.height()),
                (2., color),
            );
            painter.text(
                p,
                egui::Align2::CENTER_CENTER,
                "WASD / L",
                egui::FontId::proportional(14.),
                color,
            );
        }
        if let Some(point) = self.pending {
            painter.circle_stroke(to_screen(point), 18., (3., Color32::WHITE));
        }
        let response = ui.interact(rect, ui.id().with("place-touch"), egui::Sense::click());
        if self.package.is_none() {
            return;
        }
        if let Some(p) = response.interact_pointer_pos() {
            let at = [
                (p.x - rect.left()) / rect.width(),
                (p.y - rect.top()) / rect.height(),
            ];
            if response.clicked() {
                if self.place_stick {
                    self.draft.stick = Some(at);
                } else {
                    self.pending = Some(at);
                }
            } else if response.secondary_clicked() {
                self.draft
                    .buttons
                    .retain(|_, v| to_screen(*v).distance(p) > 20.);
                if self
                    .draft
                    .stick
                    .is_some_and(|v| to_screen(v).distance(p) < 24.)
                {
                    self.draft.stick = None;
                }
                self.pending = None;
            }
        }
    }

    pub(super) fn frame(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        events: &[egui::Event],
    ) -> Vec<Contact> {
        let mut changes = vec![];
        if !self.editing && self.layout().is_none() {
            return changes;
        }
        if self.editing {
            self.overlay(ui, rect);
        }
        if !self.pad_initialized {
            self.gamepad = Gilrs::new().ok();
            self.pad_initialized = true;
        }
        for event in events {
            if let egui::Event::Key {
                key,
                pressed,
                repeat: false,
                ..
            } = event
                && *key != Key::F11
            {
                self.control(key.name(), *pressed, &mut changes);
                if !self.editing && direction(key.name()).is_some() {
                    self.update_stick(rect.size(), &mut changes);
                }
            }
        }
        while let Some(event) = self.gamepad.as_mut().and_then(Gilrs::next_event) {
            match event.event {
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _)
                    if b != Button::Unknown =>
                {
                    self.control(
                        &format!("Pad:{b:?}"),
                        matches!(event.event, EventType::ButtonPressed(..)),
                        &mut changes,
                    );
                }
                EventType::AxisChanged(Axis::LeftStickX, value, _) => self.analog[0] = value,
                EventType::AxisChanged(Axis::LeftStickY, value, _) => self.analog[1] = -value,
                EventType::Disconnected => changes.extend(self.release()),
                _ => {}
            }
        }
        if !self.editing {
            self.update_stick(rect.size(), &mut changes);
        }
        if self.gamepad.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        changes
    }
}

fn direction(input: &str) -> Option<Vec2> {
    match input {
        "W" => Some(vec2(0., -1.)),
        "S" => Some(vec2(0., 1.)),
        "A" => Some(vec2(-1., 0.)),
        "D" => Some(vec2(1., 0.)),
        _ => None,
    }
}

fn label(input: &str) -> &str {
    match input {
        "Pad:South" => "A / ×",
        "Pad:East" => "B / ○",
        "Pad:West" => "X / □",
        "Pad:North" => "Y / △",
        other => other.strip_prefix("Pad:").unwrap_or(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_contacts_for_keyboard_gamepad_and_analog_stick() {
        let mut map = Mapping::default();
        map.select(Some("test.game"));
        map.toggle();
        for (name, point) in [("Space", [0.75, 0.8]), ("Pad:South", [0.85, 0.75])] {
            map.pending = Some(point);
            map.control(name, true, &mut vec![]);
        }
        map.draft.stick = Some([0.25, 0.65]);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("touch-maps.json");
        crate::ui::write_json(&path, &BTreeMap::from([("test.game", &map.draft)])).unwrap();
        let mut map = Mapping::default();
        map.load(path);
        map.select(Some("test.game"));
        let mut out = vec![];
        map.control("Space", true, &mut out);
        map.control("Space", true, &mut out);
        map.control("Pad:South", true, &mut out);
        map.control("W", true, &mut out);
        map.update_stick(vec2(1600., 900.), &mut out);
        assert_eq!(out.iter().map(|c| c.id).collect::<Vec<_>>(), [1, 2, 9, 9]);
        assert_eq!(out[0].pos, [0.75, 0.8]);
        assert_eq!(out[1].pos, [0.85, 0.75]);
        assert!((out[3].pos[1] - 0.55).abs() < 0.0001);
        assert!(out.iter().all(|c| c.down));
        map.control("W", false, &mut out);
        map.analog = [1., 0.];
        map.update_stick(vec2(1600., 900.), &mut out);
        assert!((out.last().unwrap().pos[0] - 0.30625).abs() < 0.0001);
        let released = map.release();
        assert_eq!(released.len(), 3);
        assert!(released.iter().all(|c| !c.down));
        map.select(Some("different.game"));
        assert!(!map.captures(Key::Space));
        map.select(Some("test.game"));
        assert!(map.captures(Key::Space));
    }
}
