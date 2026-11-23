mod editor;
mod gestures;

use crate::ui::i18n::Locale;
use eframe::egui::{self, Color32, Key, PointerButton, Rect, Vec2, vec2};
use gestures::{Binding, Frame, Gesture, Gestures};
use gilrs::{Axis, Button, EventType, Gilrs};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

type Point = [f32; 2];

#[derive(Default, Clone, Serialize, Deserialize)]
struct Layout {
    buttons: BTreeMap<String, Point>,
    stick: Option<Point>,
    #[serde(default = "default_radius")]
    radius: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    controls: Vec<Binding>,
}
fn default_radius() -> f32 {
    0.10
}

#[derive(Clone, Serialize, Deserialize)]
struct GameProfiles {
    active: String,
    layouts: BTreeMap<String, Layout>,
}

#[derive(Deserialize, Serialize)]
struct SchemeFile {
    version: u32,
    name: String,
    layout: Layout,
}

impl Default for GameProfiles {
    fn default() -> Self {
        Self::from_layout(Layout {
            radius: default_radius(),
            ..Default::default()
        })
    }
}

impl GameProfiles {
    fn from_layout(layout: Layout) -> Self {
        Self {
            active: "Default".into(),
            layouts: BTreeMap::from([("Default".into(), layout)]),
        }
    }

    fn layout(&self) -> Option<&Layout> {
        self.layouts
            .get(&self.active)
            .or_else(|| self.layouts.values().next())
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredMapping {
    Profiles(GameProfiles),
    Legacy(Layout),
}

#[derive(Clone, Copy)]
pub(super) struct Contact {
    pub id: i32,
    pub pos: Point,
    pub down: bool,
}

#[derive(Default)]
pub(crate) struct Mapping {
    profiles: BTreeMap<String, GameProfiles>,
    path: PathBuf,
    package: Option<String>,
    pub editing: bool,
    draft: Layout,
    draft_profiles: GameProfiles,
    pending: Option<Point>,
    place_stick: bool,
    selected_control: Option<usize>,
    placing_control: bool,
    placing_endpoint: bool,
    await_binding: bool,
    direction_binding: Option<usize>,
    preset: Gesture,
    error: Option<String>,
    load_failed: bool,
    gamepad: Option<Gilrs>,
    pad_initialized: bool,
    active: HashMap<String, Contact>,
    directions: HashSet<String>,
    analog: [f32; 2],
    stick_touch: Option<Contact>,
    gestures: Gestures,
    grabbed_at: Option<egui::Context>,
    modifiers: [bool; 4],
    control_dragging: Option<usize>,
    editor: editor::Editor,
}

impl Mapping {
    pub fn load(&mut self, path: PathBuf) {
        if let Ok(file) = std::fs::File::open(&path) {
            match serde_json::from_reader::<_, BTreeMap<String, StoredMapping>>(file) {
                Ok(games) => {
                    self.profiles = games
                        .into_iter()
                        .map(|(package, record)| {
                            let mut mapping = match record {
                                StoredMapping::Profiles(profiles) => profiles,
                                StoredMapping::Legacy(layout) => GameProfiles::from_layout(layout),
                            };
                            if mapping.layouts.is_empty() {
                                mapping = GameProfiles::default();
                            } else if !mapping.layouts.contains_key(&mapping.active) {
                                mapping.active =
                                    mapping.layouts.first_key_value().unwrap().0.clone();
                            }
                            (package, mapping)
                        })
                        .collect();
                    self.load_failed = false;
                    self.error = None;
                }
                Err(error) => {
                    self.load_failed = true;
                    self.error = Some(error.to_string());
                }
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
        self.draft_profiles = self
            .package
            .as_ref()
            .and_then(|p| self.profiles.get(p))
            .cloned()
            .unwrap_or_default();
        self.draft = self.draft_profiles.layout().cloned().unwrap_or_default();
        self.reset_placement();
    }

    fn reset_placement(&mut self) {
        self.pending = None;
        self.place_stick = false;
        self.selected_control = None;
        self.placing_control = false;
        self.placing_endpoint = false;
        self.await_binding = false;
        self.direction_binding = None;
        self.control_dragging = None;
    }

    fn sync_draft(&mut self) {
        self.draft_profiles
            .layouts
            .insert(self.draft_profiles.active.clone(), self.draft.clone());
    }

    fn select_profile(&mut self, name: &str) {
        if self.draft_profiles.layouts.contains_key(name) {
            self.sync_draft();
            self.draft_profiles.active = name.to_owned();
            self.draft = self.draft_profiles.layouts[name].clone();
            self.reset_placement();
        }
    }

    fn create_profile(&mut self, name: &str) -> bool {
        self.insert_profile(
            name,
            Layout {
                radius: default_radius(),
                ..Default::default()
            },
        )
    }

    fn copy_profile(&mut self, name: &str) -> bool {
        self.insert_profile(name, self.draft.clone())
    }

    fn insert_profile(&mut self, name: &str, layout: Layout) -> bool {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > 48
            || name.chars().any(char::is_control)
            || self.draft_profiles.layouts.contains_key(name)
        {
            return false;
        }
        self.draft_profiles.layouts.insert(name.into(), layout);
        self.select_profile(name);
        true
    }

    fn delete_profile(&mut self) -> bool {
        if self.draft_profiles.layouts.len() <= 1 {
            return false;
        }
        self.draft_profiles
            .layouts
            .remove(&self.draft_profiles.active);
        let (name, layout) = self.draft_profiles.layouts.first_key_value().unwrap();
        self.draft_profiles.active = name.clone();
        self.draft = layout.clone();
        self.reset_placement();
        true
    }

    fn save(&mut self) {
        let Some(package) = self.package.clone() else {
            return;
        };
        if self.load_failed {
            return;
        }
        self.sync_draft();
        let mut next = self.profiles.clone();
        next.insert(package, self.draft_profiles.clone());
        match crate::ui::write_json(&self.path, &next) {
            Ok(()) => {
                self.profiles = next;
                self.editing = false;
                self.reset_placement();
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn import_profile(&mut self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        if file.metadata().map_err(|error| error.to_string())?.len() > 262_144 {
            return Err("Exceeded 256 KiB".into());
        }
        let record: SchemeFile =
            serde_json::from_reader(file).map_err(|error| error.to_string())?;
        if record.version != 1 || !valid_layout(&record.layout) {
            return Err("Unsupported or invalid mapping data".into());
        }
        if !self.insert_profile(&record.name, record.layout) {
            return Err("Invalid or duplicate profile name".into());
        }
        Ok(())
    }

    fn export_profile(&self, path: &Path) -> Result<(), String> {
        crate::ui::write_json(
            path,
            &SchemeFile {
                version: 1,
                name: self.draft_profiles.active.clone(),
                layout: self.draft.clone(),
            },
        )
        .map_err(|error| error.to_string())
    }
    pub(super) fn toggle(&mut self) {
        self.editing = !self.editing;
        self.reset_draft();
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui, locale: Locale) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(locale.pick("Mapeamento", "Mapping", "Asignación"));
            ui.label(self.package.as_deref().unwrap_or(locale.pick(
                "Identificando aplicativo…",
                "Identifying app…",
                "Identificando aplicación…",
            )));
            ui.separator();
            if ui
                .selectable_label(!self.place_stick, locale.pick("Botão", "Button", "Botón"))
                .clicked()
            {
                self.reset_placement();
            }
            if ui
                .selectable_label(
                    self.place_stick,
                    locale.pick("Direcional", "Directional control", "Control direccional"),
                )
                .clicked()
            {
                self.reset_placement();
                self.place_stick = true;
            }
            if self.place_stick {
                ui.add(
                    egui::Slider::new(&mut self.draft.radius, 0.035..=0.22)
                        .text(locale.pick("raio", "radius", "radio")),
                );
            }
            if crate::ui::theme::primary(
                ui,
                self.package.is_some() && !self.load_failed,
                locale.pick("Salvar", "Save", "Guardar"),
            )
            .clicked()
            {
                self.save();
            }
            if ui
                .add(crate::ui::theme::button(
                    locale.pick("Cancelar", "Cancel", "Cancelar"),
                ))
                .clicked()
            {
                self.editing = false;
                self.reset_draft();
            }
        });
        if !self.editing {
            return;
        }
        self.advanced_toolbar(ui, locale);
        ui.small(if self.place_stick { locale.pick("Clique no centro do direcional do jogo para usar WASD ou o analógico esquerdo. Clique direito remove.", "Click the center of the in-game stick to use WASD or the left stick. Right-click to remove.", "Haz clic en el centro del mando virtual para usar WASD o la palanca izquierda. Haz clic derecho para quitarlo.") }
            else if self.pending.is_some() { locale.pick("Pressione a tecla ou o botão do controle que deve tocar nesse ponto.", "Press the key or controller button that should tap this point.", "Pulsa la tecla o el botón del mando que deba tocar este punto.") }
            else { locale.pick("Clique em um botão do jogo e pressione uma tecla ou botão do controle. Clique direito em uma marca para removê-la.", "Click an in-game button and press a key or controller button. Right-click a marker to remove it.", "Haz clic en un botón del juego y pulsa una tecla o un botón del mando. Haz clic derecho en un marcador para quitarlo.") });
        if let Some(error) = &self.error {
            ui.colored_label(
                Color32::LIGHT_RED,
                locale.pick(
                    "Falha no mapeamento",
                    "Mapping error",
                    "Error de asignación",
                ),
            )
            .on_hover_text(error);
        }
    }

    fn layout(&self) -> Option<&Layout> {
        self.package
            .as_ref()
            .and_then(|p| self.profiles.get(p))
            .and_then(GameProfiles::layout)
    }

    pub fn captures(&self, key: Key) -> bool {
        self.captures_input(key.name())
    }

    pub(super) fn captures_input(&self, input: &str) -> bool {
        self.layout().is_some_and(|layout| {
            layout.buttons.contains_key(input)
                || (layout.stick.is_some() && direction(input).is_some())
                || layout
                    .controls
                    .iter()
                    .any(|binding| binding.captures(input))
        })
    }

    pub(super) fn captures_pointer(&self, button: PointerButton) -> bool {
        self.grabbed_at.is_some() || self.captures_input(pointer_id(button))
    }

    pub(super) fn aims_pointer(&self) -> bool {
        self.grabbed_at.is_some()
    }

    fn set_mouse_capture(&mut self, active: Option<(&egui::Context, egui::Pos2)>) {
        use egui::{ViewportCommand, viewport::CursorGrab};
        if let Some((ctx, center)) = active {
            if self.grabbed_at.is_none() {
                self.grabbed_at = Some(ctx.clone());
                ctx.send_viewport_cmd(ViewportCommand::CursorPosition(center));
                ctx.send_viewport_cmd(ViewportCommand::CursorGrab(if cfg!(windows) {
                    CursorGrab::Confined
                } else {
                    CursorGrab::Locked
                }));
            }
        } else if let Some(ctx) = self.grabbed_at.take() {
            ctx.send_viewport_cmd(ViewportCommand::CursorGrab(CursorGrab::None));
            ctx.send_viewport_cmd(ViewportCommand::CursorVisible(true));
        }
    }

    pub(super) fn release(&mut self) -> Vec<Contact> {
        self.directions.clear();
        self.analog = [0.; 2];
        self.modifiers = [false; 4];
        self.set_mouse_capture(None);
        let mut ups: Vec<_> = self
            .active
            .drain()
            .map(|(_, c)| Contact { down: false, ..c })
            .collect();
        if let Some(c) = self.stick_touch.take() {
            ups.push(Contact { down: false, ..c });
        }
        ups.extend(self.gestures.release());
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
                if let Some(id) = (1..=8).find(|id| {
                    !self.active.values().any(|c| c.id == *id) && !self.gestures.used(*id)
                }) {
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

    fn advanced_layout<'a>(
        profiles: &'a BTreeMap<String, GameProfiles>,
        package: Option<&str>,
    ) -> &'a [Binding] {
        package
            .and_then(|p| profiles.get(p))
            .and_then(GameProfiles::layout)
            .map_or(&[], |layout| &layout.controls)
    }

    fn taken(&self) -> Vec<i32> {
        let mut taken: Vec<_> = self.active.values().map(|c| c.id).collect();
        if self.layout().is_some_and(|layout| layout.stick.is_some()) {
            taken.push(9);
        }
        taken
    }

    fn handle_input(
        &mut self,
        input: &str,
        pressed: bool,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        if self.editing && self.await_binding {
            if pressed
                && let Some(binding) = self
                    .selected_control
                    .and_then(|i| self.draft.controls.get_mut(i))
            {
                if let Some(direction) = self.direction_binding.take()
                    && let Gesture::Dpad { directions, .. } = &mut binding.gesture
                    && let Some(slot) = directions.get_mut(direction)
                {
                    *slot = input.to_owned();
                } else {
                    binding.input = input.to_owned();
                }
                self.await_binding = false;
            }
            return;
        }

        if !self.editing
            && !self
                .layout()
                .is_some_and(|layout| layout.buttons.contains_key(input))
        {
            let advanced = Self::advanced_layout(&self.profiles, self.package.as_deref());
            if advanced.iter().any(|binding| binding.captures(input)) {
                let taken = self.taken();
                self.gestures.input(
                    advanced,
                    input,
                    pressed,
                    &Frame {
                        taken: &taken,
                        ..*frame
                    },
                    changes,
                );
                return;
            }
        }
        self.control(input, pressed, changes);
        if !self.editing && direction(input).is_some() {
            self.update_stick(frame.size, changes);
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
        for (index, binding) in self.draft.controls.iter().enumerate() {
            let pos = to_screen(binding.at);
            let selected = self.selected_control == Some(index);
            let edge = if selected { Color32::WHITE } else { color };
            if let Gesture::Swipe { end, .. } = binding.gesture {
                let end = to_screen(end);
                painter.line_segment([pos, end], (2., edge));
                painter.circle_stroke(end, 7., (2., edge));
            }
            painter.circle_filled(pos, 18., Color32::from_black_alpha(220));
            painter.circle_stroke(pos, 18., (2., edge));
            let text = label(&binding.input);
            let index_number = (index + 1).to_string();
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                if text.is_empty() { &index_number } else { text },
                egui::FontId::proportional(12.),
                Color32::WHITE,
            );
        }
        let response = ui.interact(
            rect,
            ui.id().with("place-touch"),
            egui::Sense::click_and_drag(),
        );
        if self.package.is_none() {
            return;
        }
        if let Some(p) = response.interact_pointer_pos() {
            let at = [
                ((p.x - rect.left()) / rect.width()).clamp(0., 1.),
                ((p.y - rect.top()) / rect.height()).clamp(0., 1.),
            ];
            let closest = |p: egui::Pos2| {
                self.draft
                    .controls
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, binding)| to_screen(binding.at).distance(p) < 23.)
                    .map(|(index, _)| index)
            };
            let selected = closest(p);
            if self.await_binding {
                return;
            }
            if response.drag_started() && !self.placing_control && !self.placing_endpoint {
                let origin = ui.input(|input| input.pointer.press_origin()).unwrap_or(p);
                self.control_dragging = closest(origin);
                if self.control_dragging.is_some() {
                    self.selected_control = self.control_dragging;
                }
            }
            if response.dragged()
                && let Some(index) = self.control_dragging
                && let Some(binding) = self.draft.controls.get_mut(index)
            {
                binding.at = at;
            }
            if response.drag_stopped() {
                self.control_dragging = None;
            }
            if response.clicked() {
                if self.placing_control {
                    let mut gesture = self.preset.clone();
                    if let Gesture::Swipe { end, .. } = &mut gesture {
                        *end = [at[0], (at[1] - 0.18).max(0.)];
                    }
                    self.selected_control = Some(self.draft.controls.len());
                    self.draft.controls.push(Binding {
                        input: String::new(),
                        at,
                        gesture,
                    });
                    self.placing_control = false;
                    self.pending = None;
                } else if self.placing_endpoint {
                    if let Some(binding) = self
                        .selected_control
                        .and_then(|i| self.draft.controls.get_mut(i))
                        && let Gesture::Swipe { end, .. } = &mut binding.gesture
                    {
                        *end = at;
                    }
                    self.placing_endpoint = false;
                } else if let Some(index) = selected {
                    self.selected_control = Some(index);
                    self.pending = None;
                } else if self.place_stick {
                    self.draft.stick = Some(at);
                } else {
                    self.pending = Some(at);
                    self.selected_control = None;
                }
            } else if response.secondary_clicked() {
                if let Some(index) = selected {
                    self.draft.controls.remove(index);
                    self.reset_placement();
                    return;
                }
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
        if (!self.editing && self.layout().is_none()) || rect.width() < 1. || rect.height() < 1. {
            return changes;
        }
        if self.editing {
            self.overlay(ui, rect);
        }
        if !self.pad_initialized {
            self.gamepad = Gilrs::new().ok();
            self.pad_initialized = true;
        }
        let (time, dt, pointer, modifiers) = ui.input(|input| {
            (
                input.time,
                input.stable_dt,
                input.pointer.hover_pos(),
                input.modifiers,
            )
        });
        let frame = Frame {
            time,
            dt,
            size: rect.size(),
            taken: &[],
            cursor: pointer.map(|p| {
                [
                    ((p.x - rect.left()) / rect.width()).clamp(0., 1.),
                    ((p.y - rect.top()) / rect.height()).clamp(0., 1.),
                ]
            }),
        };
        let text_input = self.editing
            && ui
                .ctx()
                .memory(|memory| memory.focused())
                .is_some_and(|focus| egui::TextEdit::load_state(ui.ctx(), focus).is_some());
        let keys = [
            ("Shift", modifiers.shift),
            ("Control", modifiers.ctrl),
            ("Alt", modifiers.alt),
            ("Meta", modifiers.mac_cmd),
        ];
        for (index, (key, held)) in keys.into_iter().enumerate() {
            if self.modifiers[index] != held {
                self.modifiers[index] = held;
                if !text_input {
                    self.handle_input(key, held, &frame, &mut changes);
                }
            }
        }
        for event in events {
            match *event {
                egui::Event::Key {
                    key,
                    pressed,
                    repeat: false,
                    ..
                } if key != Key::F11 && !text_input => {
                    if key == Key::Escape && pressed && self.grabbed_at.is_some() {
                        let advanced =
                            Self::advanced_layout(&self.profiles, self.package.as_deref());
                        self.gestures.cancel_aim(advanced, &mut changes);
                    } else {
                        self.handle_input(key.name(), pressed, &frame, &mut changes);
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } if !pressed || rect.contains(pos) || self.grabbed_at.is_some() => {
                    // Clicking to place a legacy marker still waits for a keyboard/gamepad key.
                    if !self.editing || self.await_binding {
                        self.handle_input(pointer_id(button), pressed, &frame, &mut changes);
                    }
                }
                egui::Event::MouseWheel { delta, .. }
                    if pointer.is_some_and(|p| rect.contains(p)) || self.grabbed_at.is_some() =>
                {
                    if let Some(key) = wheel_id(delta) {
                        self.handle_input(key, true, &frame, &mut changes);
                        self.handle_input(key, false, &frame, &mut changes);
                    }
                }
                egui::Event::MouseMoved(delta) if !self.editing => {
                    self.gestures.mouse += delta / ui.ctx().pixels_per_point()
                }
                _ => {}
            }
        }
        while let Some(event) = self.gamepad.as_mut().and_then(Gilrs::next_event) {
            match event.event {
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _)
                    if b != Button::Unknown =>
                {
                    self.handle_input(
                        &format!("Pad:{b:?}"),
                        matches!(event.event, EventType::ButtonPressed(..)),
                        &frame,
                        &mut changes,
                    );
                }
                EventType::AxisChanged(Axis::LeftStickX, value, _) => self.analog[0] = value,
                EventType::AxisChanged(Axis::LeftStickY, value, _) => self.analog[1] = -value,
                EventType::AxisChanged(Axis::RightStickX, value, _) => {
                    self.gestures.right.x = value
                }
                EventType::AxisChanged(Axis::RightStickY, value, _) => {
                    self.gestures.right.y = -value
                }
                EventType::Disconnected => changes.extend(self.release()),
                _ => {}
            }
        }
        if !self.editing {
            self.update_stick(rect.size(), &mut changes);
            let advanced = Self::advanced_layout(&self.profiles, self.package.as_deref());
            let taken = self.taken();
            self.gestures.left = vec2(self.analog[0], self.analog[1]);
            self.gestures.frame(
                advanced,
                &Frame {
                    taken: &taken,
                    ..frame
                },
                &mut changes,
            );
            let aim = self.gestures.aiming_mouse(advanced);
            self.set_mouse_capture(aim.then_some((ui.ctx(), rect.center())));
            if aim {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            }
        }
        if self.gamepad.is_some() || self.gestures.waiting() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        changes
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        self.set_mouse_capture(None);
    }
}

fn valid_layout(layout: &Layout) -> bool {
    let point = |p: Point| p.into_iter().all(|v| (0. ..=1.).contains(&v));
    let input = |name: &str| name.chars().count() <= 48 && !name.chars().any(char::is_control);
    let duration = |ms| (20..=10_000).contains(&ms);
    let radius = |value| (0.005..=0.40).contains(&value);
    layout.buttons.len() <= 64
        && layout.controls.len() <= 64
        && layout.stick.is_none_or(point)
        && radius(layout.radius)
        && layout
            .buttons
            .iter()
            .all(|(key, at)| input(key) && point(*at))
        && layout.controls.iter().all(|binding| {
            point(binding.at)
                && input(&binding.input)
                && match &binding.gesture {
                    Gesture::Hold => true,
                    Gesture::Tap { duration_ms } => duration(*duration_ms),
                    Gesture::Repeat {
                        interval_ms,
                        hold_ms,
                    } => (40..=10_000).contains(interval_ms) && duration(*hold_ms),
                    Gesture::Swipe { end, duration_ms } => point(*end) && duration(*duration_ms),
                    Gesture::Dpad {
                        directions,
                        radius: value,
                        ..
                    } => directions.iter().all(|direction| input(direction)) && radius(*value),
                    Gesture::Aim { sensitivity, .. } => (0.05..=8.).contains(sensitivity),
                    Gesture::Pinch {
                        from,
                        to,
                        duration_ms,
                    } => radius(*from) && radius(*to) && duration(*duration_ms),
                    Gesture::Rotate {
                        radius: value,
                        degrees,
                        duration_ms,
                    } => {
                        radius(*value)
                            && (-720. ..=720.).contains(degrees)
                            && duration(*duration_ms)
                    }
                    Gesture::Moba { radius: value, .. } => radius(*value),
                }
        })
}

fn pointer_id(button: PointerButton) -> &'static str {
    match button {
        PointerButton::Primary => "Mouse:Primary",
        PointerButton::Secondary => "Mouse:Secondary",
        PointerButton::Middle => "Mouse:Middle",
        PointerButton::Extra1 => "Mouse:Extra1",
        PointerButton::Extra2 => "Mouse:Extra2",
    }
}

fn wheel_id(delta: Vec2) -> Option<&'static str> {
    if delta == Vec2::ZERO {
        None
    } else if delta.x.abs() > delta.y.abs() {
        Some(if delta.x > 0. {
            "Wheel:Right"
        } else {
            "Wheel:Left"
        })
    } else {
        Some(if delta.y > 0. {
            "Wheel:Up"
        } else {
            "Wheel:Down"
        })
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

    #[test]
    fn sharing_profiles_keeps_legacy_bindings_and_rejects_invalid_coordinates() {
        let temp = tempfile::tempdir().unwrap();
        let mapping_path = temp.path().join("touch-maps.json");
        let share_path = temp.path().join("scheme.json");
        std::fs::write(
            &mapping_path,
            r#"{"test.game":{"buttons":{"Space":[0.8,0.8]},"stick":null}}"#,
        )
        .unwrap();
        let mut map = Mapping::default();
        map.load(mapping_path.clone());
        map.select(Some("test.game"));
        assert!(map.copy_profile("Race"));
        map.draft.controls.push(Binding {
            input: "Pad:South".into(),
            at: [0.72, 0.9],
            gesture: Gesture::Repeat {
                interval_ms: 100,
                hold_ms: 35,
            },
        });
        map.export_profile(&share_path).unwrap();
        map.save();

        let mut loaded = Mapping::default();
        loaded.load(mapping_path);
        loaded.select(Some("test.game"));
        assert_eq!(loaded.draft_profiles.active, "Race");
        assert!(loaded.captures_input("Space") && loaded.captures_input("Pad:South"));
        loaded.select(Some("other.game"));
        loaded.import_profile(&share_path).unwrap();
        assert_eq!(loaded.draft.controls.len(), 1);
        assert!(loaded.import_profile(&share_path).is_err());
        let original = std::fs::read_to_string(&share_path).unwrap();
        std::fs::write(
            &share_path,
            original.replace("0.72", "1.1").replace("Race", "Broken"),
        )
        .unwrap();
        assert!(loaded.import_profile(&share_path).is_err());
        assert_eq!(loaded.draft_profiles.active, "Race");
        assert_eq!(loaded.draft.controls[0].at, [0.72, 0.9]);
    }
}
