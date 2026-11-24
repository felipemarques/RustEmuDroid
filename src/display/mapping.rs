mod canvas;
mod editor;
mod gestures;
mod quick;

use eframe::egui::{self, Key, PointerButton, Rect, Vec2, vec2};
use gestures::{Binding, Frame, Gesture, Gestures, tilt_degrees};
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

#[derive(Debug, PartialEq, Eq)]
enum SchemeError {
    File(String),
    TooLarge,
    InvalidData,
    InvalidName,
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
    pending_key: Option<String>,
    place_stick: bool,
    place_button: bool,
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
    wheel_releases: HashSet<&'static str>,
    analog: [f32; 2],
    stick_touch: Option<Contact>,
    gestures: Gestures,
    tilt_held: HashSet<String>,
    grabbed_at: Option<egui::Context>,
    modifiers: [bool; 4],
    canvas_drag: Option<canvas::Drag>,
    pub(crate) blocked_rect: Option<Rect>,
    pub(crate) quick: quick::QuickControls,
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
        self.quick.load(&path);
        self.path = path;
    }

    pub fn wants_package(&self) -> bool {
        self.editing || self.quick.is_open() || !self.profiles.is_empty()
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
        self.pending_key = None;
        self.place_stick = false;
        self.place_button = false;
        self.selected_control = None;
        self.placing_control = false;
        self.placing_endpoint = false;
        self.await_binding = false;
        self.direction_binding = None;
        self.canvas_drag = None;
        self.editor.palette_drag = false;
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

    fn import_profile(&mut self, path: &Path) -> Result<(), SchemeError> {
        let file =
            std::fs::File::open(path).map_err(|error| SchemeError::File(error.to_string()))?;
        let size = file
            .metadata()
            .map_err(|error| SchemeError::File(error.to_string()))?
            .len();
        if size > 262_144 {
            return Err(SchemeError::TooLarge);
        }
        let record: SchemeFile = serde_json::from_reader(file).map_err(|error| {
            if error.is_io() {
                SchemeError::File(error.to_string())
            } else {
                SchemeError::InvalidData
            }
        })?;
        if record.version != 1 || !valid_layout(&record.layout) {
            return Err(SchemeError::InvalidData);
        }
        if !self.insert_profile(&record.name, record.layout) {
            return Err(SchemeError::InvalidName);
        }
        Ok(())
    }

    fn export_profile(&self, path: &Path) -> Result<(), SchemeError> {
        crate::ui::write_json(
            path,
            &SchemeFile {
                version: 1,
                name: self.draft_profiles.active.clone(),
                layout: self.draft.clone(),
            },
        )
        .map_err(|error| SchemeError::File(error.to_string()))
    }
    pub(super) fn toggle(&mut self) {
        self.editing = !self.editing;
        self.reset_draft();
    }

    pub(crate) fn cancel_edit_step(&mut self) -> bool {
        if self.pending.is_some()
            || self.placing_control
            || self.placing_endpoint
            || self.await_binding
            || self.editor.palette_drag
            || self.canvas_drag.is_some()
            || self.place_stick
            || self.place_button
        {
            self.reset_placement();
            return true;
        }
        false
    }

    fn layout(&self) -> Option<&Layout> {
        if !self.quick.enabled {
            return None;
        }
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
        self.tilt_held.clear();
        self.release_contacts()
    }

    pub(super) fn release_contacts(&mut self) -> Vec<Contact> {
        self.canvas_drag = None;
        if self.editor.palette_drag {
            self.editor.palette_drag = false;
            self.placing_control = false;
        }
        self.directions.clear();
        self.wheel_releases.clear();
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

    pub(super) fn tilt_offset(&self) -> [f32; 3] {
        let mut offset = [0.; 3];
        if !self.tilt_held.is_empty()
            && let Some(layout) = self.layout()
        {
            for binding in &layout.controls {
                if let Gesture::Tilt { axis, degrees } = binding.gesture
                    && self.tilt_held.contains(&binding.input)
                {
                    offset[axis as usize] += tilt_degrees(degrees);
                }
            }
        }
        offset.map(tilt_degrees)
    }

    fn control(&mut self, input: &str, down: bool, changes: &mut Vec<Contact>) {
        if self.editing {
            if down && let Some(point) = self.pending.take() {
                if let Some(old_key) = self.pending_key.take() {
                    self.draft.buttons.remove(&old_key);
                }
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
                    && let Gesture::Dpad { directions, .. } | Gesture::FreeLook { directions, .. } =
                        &mut binding.gesture
                    && let Some(slot) = directions.get_mut(direction)
                {
                    *slot = input.to_owned();
                } else if !matches!(binding.gesture, Gesture::MobaMove { .. })
                    || input != "Mouse:Secondary"
                {
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
                if advanced.iter().any(|binding| {
                    matches!(binding.gesture, Gesture::Tilt { .. }) && binding.captures(input)
                }) {
                    if pressed {
                        self.tilt_held.insert(input.into());
                    } else {
                        self.tilt_held.remove(input);
                    }
                }
                let taken = self.taken();
                self.gestures.left = vec2(self.analog[0], self.analog[1]);
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
        let capture_already_active = self.await_binding;
        let visible = rect.intersect(ui.clip_rect());
        if self.editing {
            self.canvas(ui, rect);
        } else if self.quick.show_hints {
            self.paint_hints(ui, rect);
        }
        if !self.pad_initialized {
            self.gamepad = Gilrs::new().ok();
            self.pad_initialized = true;
        }
        let (time, dt, pointer, pointer_busy, modifiers) = ui.input(|input| {
            (
                input.time,
                input.stable_dt,
                input.pointer.hover_pos(),
                input.pointer.any_down() || input.pointer.any_released(),
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
            edge_cursor: None,
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
        for wheel in std::mem::take(&mut self.wheel_releases) {
            self.handle_input(wheel, false, &frame, &mut changes);
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
                } if !pressed || visible.contains(pos) || self.grabbed_at.is_some() => {
                    // Clicking to place a legacy marker still waits for a keyboard/gamepad key.
                    if !self.editing
                        || (capture_already_active
                            && self.await_binding
                            && self.canvas_contains(visible, pos))
                    {
                        // Use the click position even if the pointer moved again in this UI frame.
                        let click = Frame {
                            cursor: rect.contains(pos).then(|| {
                                [
                                    (pos.x - rect.left()) / rect.width(),
                                    (pos.y - rect.top()) / rect.height(),
                                ]
                            }),
                            ..frame
                        };
                        self.handle_input(pointer_id(button), pressed, &click, &mut changes);
                    }
                }
                egui::Event::MouseWheel { delta, .. }
                    if pointer.is_some_and(|p| {
                        visible.contains(p) && (!self.editing || self.canvas_contains(visible, p))
                    }) || self.grabbed_at.is_some() =>
                {
                    if let Some(key) = wheel_id(delta) {
                        if !self.editing
                            && self.grabbed_at.is_none()
                            && !pointer_busy
                            && pointer.is_some_and(|p| visible.contains(p))
                            && let Some(override_wheel) = self.layout().and_then(|layout| {
                                layout
                                    .controls
                                    .iter()
                                    .find_map(|binding| match binding.gesture {
                                        Gesture::Scroll { override_wheel, .. } => {
                                            Some(override_wheel)
                                        }
                                        _ => None,
                                    })
                            })
                        {
                            let direction = if delta.x.abs() > delta.y.abs() {
                                vec2(delta.x.signum(), 0.)
                            } else {
                                vec2(0., delta.y.signum())
                            };
                            self.gestures.queue_scroll(direction);
                            if override_wheel {
                                continue;
                            }
                        }
                        self.handle_input(key, true, &frame, &mut changes);
                        let directional = !self.editing
                            && self.layout().is_some_and(|layout| {
                                !layout.buttons.contains_key(key)
                                    && layout.controls.iter().any(|binding| {
                                        matches!(binding.gesture, Gesture::Dpad { .. })
                                            && binding.captures(key)
                                    })
                            });
                        if directional {
                            // Give the D-pad one frame to apply the wheel direction before releasing it.
                            self.wheel_releases.insert(key);
                        } else {
                            self.handle_input(key, false, &frame, &mut changes);
                        }
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
            let aim = self.gestures.aiming_mouse(advanced);
            let edge_visible = visible.intersect(ui.available_rect_before_wrap());
            let edge_cursor =
                if !aim && !pointer_busy && pointer.is_some_and(|p| edge_visible.contains(p)) {
                    frame.cursor
                } else {
                    None
                };
            self.gestures.frame(
                advanced,
                &Frame {
                    taken: &taken,
                    edge_cursor,
                    ..frame
                },
                &mut changes,
            );
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
        && layout
            .controls
            .iter()
            .filter(|binding| matches!(binding.gesture, Gesture::MobaMove { .. }))
            .take(2)
            .count()
            <= 1
        && layout
            .controls
            .iter()
            .filter(|binding| matches!(binding.gesture, Gesture::Scroll { .. }))
            .take(2)
            .count()
            <= 1
        && layout
            .controls
            .iter()
            .filter(|binding| matches!(binding.gesture, Gesture::EdgeScroll { .. }))
            .take(2)
            .count()
            <= 1
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
                    Gesture::FreeLook {
                        directions,
                        speed,
                        reset_ms,
                        ..
                    } => {
                        directions.iter().all(|key| input(key))
                            && (0.20..=3.).contains(speed)
                            && *reset_ms <= 1000
                    }
                    Gesture::Scroll {
                        amplitude, speed, ..
                    } => {
                        binding.input.is_empty()
                            && (0.02..=0.45).contains(amplitude)
                            && (0.20..=3.).contains(speed)
                    }
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
                    Gesture::MobaMove {
                        hero,
                        radius: value,
                        speed,
                    } => {
                        point(*hero)
                            && radius(*value)
                            && (0.1..=5.).contains(speed)
                            && binding.input != "Mouse:Secondary"
                    }
                    Gesture::EdgeScroll {
                        margin_x,
                        margin_y,
                        speed_x,
                        speed_y,
                    } => {
                        binding.input.is_empty()
                            && [margin_x, margin_y]
                                .into_iter()
                                .all(|margin| (0.01..=0.25).contains(margin))
                            && [speed_x, speed_y]
                                .into_iter()
                                .all(|speed| (0.20..=2.).contains(speed))
                    }
                    Gesture::Tilt { degrees, .. } => *degrees == tilt_degrees(*degrees),
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
        "Mouse:Secondary" => "M2",
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
    fn scroll_override_respects_the_game_canvas_and_existing_wheel_mapping() {
        use egui::{Event, Modifiers, MouseWheelUnit, pos2};

        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(0., 0.), vec2(900., 700.));
        let rect = Rect::from_min_size(pos2(100., 100.), vec2(700., 500.));
        let tick = |map: &mut Mapping, pointer| {
            let events = vec![
                Event::PointerMoved(pointer),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: vec2(0., 1.),
                    modifiers: Modifiers::NONE,
                },
            ];
            let mut contacts = Vec::new();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: events.clone(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        contacts.extend(map.frame(ui, rect, &events));
                    });
                },
            );
            contacts
        };
        for override_wheel in [true, false] {
            let mut map = Mapping::default();
            map.pad_initialized = true;
            map.profiles.insert(
                "test.game".into(),
                GameProfiles::from_layout(Layout {
                    buttons: BTreeMap::from([(String::from("Wheel:Up"), [0.8, 0.8])]),
                    controls: vec![Binding {
                        input: String::new(),
                        at: [0.5, 0.5],
                        gesture: Gesture::Scroll {
                            amplitude: 0.18,
                            speed: 1.,
                            override_wheel,
                        },
                    }],
                    ..Default::default()
                }),
            );
            map.select(Some("test.game"));
            assert!(tick(&mut map, pos2(450., 40.)).is_empty());
            let contacts = tick(&mut map, rect.center());
            let positions: Vec<_> = contacts
                .iter()
                .map(|contact| (contact.pos, contact.down))
                .collect();
            if override_wheel {
                assert_eq!(positions, [([0.5, 0.5], true)]);
            } else {
                assert_eq!(
                    positions,
                    [([0.8, 0.8], true), ([0.8, 0.8], false), ([0.5, 0.5], true)]
                );
            }
            let released = map.release();
            assert_eq!(released.len(), 1);
            assert!(!released[0].down);
        }
    }

    #[test]
    fn edge_scroll_repeats_bounded_touch_drags_only_over_the_active_guest() {
        use crate::display::{Display, Format, Frame, Input, Touch};
        use egui::{Event, pos2};

        let (input, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut display = Display::new(Default::default(), input);
        display.state.lock().unwrap().latest = Some(Frame {
            format: Some(Format {
                width: 160,
                height: 96,
                ..Default::default()
            }),
            pixels: vec![255; 160 * 96 * 4].into(),
        });
        display.mapping.pad_initialized = true;
        let layout = Layout {
            radius: default_radius(),
            controls: vec![Binding {
                input: String::new(),
                at: [0.5, 0.5],
                gesture: Gesture::EdgeScroll {
                    margin_x: 0.06,
                    margin_y: 0.06,
                    speed_x: 0.75,
                    speed_y: 0.50,
                },
            }],
            ..Default::default()
        };
        assert!(valid_layout(&layout));
        display
            .mapping
            .profiles
            .insert("test.game".into(), GameProfiles::from_layout(layout));
        display.set_package(Some("test.game"));
        let ctx = egui::Context::default();
        let mut time = 1.;
        let mut render = |display: &mut Display, events: Vec<Event>| {
            time += 1. / 60.;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(960., 640.))),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        display.show(ui, true, crate::ui::i18n::Locale::default());
                    });
                },
            );
            let mut contacts = Vec::<Touch>::new();
            while let Ok(event) = received.try_recv() {
                let Input::Touch(touches) = event else {
                    panic!("edge scroll should send touch events");
                };
                contacts.extend(touches.touches);
            }
            contacts
        };

        let letterbox = pos2(943., 10.);
        let right = pos2(943., 320.);
        assert!(render(&mut display, vec![Event::PointerMoved(letterbox)]).is_empty());
        let mut pan = render(&mut display, vec![Event::PointerMoved(right)]);
        for _ in 0..35 {
            pan.extend(render(&mut display, vec![]));
        }
        assert_eq!((pan[0].x, pan[0].y, pan[0].pressure), (80, 48, 1024));
        assert!(pan.iter().any(|touch| touch.x <= 65));
        assert!(pan.iter().any(|touch| touch.pressure == 0));
        assert!(
            pan.iter()
                .all(|touch| touch.y == 48 && (60..=80).contains(&touch.x))
        );
        assert!(
            pan.iter()
                .filter(|touch| touch.x == 80 && touch.pressure > 0)
                .count()
                > 1
        );

        let release = render(&mut display, vec![Event::PointerMoved(letterbox)]);
        assert_eq!(release.len(), 1);
        assert_eq!(release[0].pressure, 0);
        assert!(render(&mut display, vec![]).is_empty());

        let top = pos2(480., 42.);
        let mut vertical = render(&mut display, vec![Event::PointerMoved(top)]);
        vertical.extend(render(&mut display, vec![]));
        assert!(
            vertical
                .iter()
                .any(|touch| touch.y > 48 && touch.pressure > 0)
        );
        let release = render(&mut display, vec![Event::WindowFocused(false)]);
        assert_eq!(release.len(), 1);
        assert_eq!(release[0].pressure, 0);

        display.set_package(Some("other.game"));
        assert!(render(&mut display, vec![Event::PointerMoved(right)]).is_empty());
    }

    #[test]
    fn configured_tilt_is_relative_bounded_and_releases_without_touch_contacts() {
        use gestures::TiltAxis;

        let binding = |input: &str, axis, degrees| Binding {
            input: input.into(),
            at: [0.5, 0.5],
            gesture: Gesture::Tilt { axis, degrees },
        };
        let mut layout = Layout {
            radius: default_radius(),
            controls: vec![
                binding("Q", TiltAxis::X, 30.),
                binding("E", TiltAxis::X, 35.),
                binding("J", TiltAxis::Y, -12.),
            ],
            ..Default::default()
        };
        assert!(valid_layout(&layout));
        layout.controls.push(binding("K", TiltAxis::Z, 90.));
        assert!(!valid_layout(&layout));
        assert_eq!(tilt_degrees(f32::NAN), 0.);
        assert_eq!(tilt_degrees(-90.), -45.);
        assert_eq!(tilt_degrees(f32::INFINITY), 0.);
        let mut mapping = Mapping::default();
        mapping
            .profiles
            .insert("test.game".into(), GameProfiles::from_layout(layout));
        mapping.select(Some("test.game"));
        let frame = Frame {
            time: 0.,
            dt: 1. / 60.,
            size: vec2(160., 90.),
            taken: &[],
            cursor: None,
            edge_cursor: None,
        };
        let mut contacts = vec![];
        for key in ["Q", "E", "J", "K"] {
            assert!(mapping.captures_input(key));
            mapping.handle_input(key, true, &frame, &mut contacts);
        }
        assert_eq!(mapping.tilt_offset(), [45., -12., 45.]);
        assert!(contacts.is_empty());
        assert!(mapping.release_contacts().is_empty());
        assert_eq!(mapping.tilt_offset(), [45., -12., 45.]);
        mapping.handle_input("Q", false, &frame, &mut contacts);
        assert_eq!(mapping.tilt_offset(), [35., -12., 45.]);
        assert!(mapping.release().is_empty());
        assert_eq!(mapping.tilt_offset(), [0.; 3]);
    }

    #[test]
    fn dpad_taps_in_one_frame_send_ordered_android_contacts() {
        use crate::display::{Display, Format, Frame, Input};

        let (input, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut display = Display::new(Default::default(), input);
        display.state.lock().unwrap().latest = Some(Frame {
            format: Some(Format {
                width: 160,
                height: 96,
                ..Default::default()
            }),
            pixels: vec![255; 160 * 96 * 4].into(),
        });
        display.mapping.pad_initialized = true;
        display.mapping.profiles.insert(
            "test.game".into(),
            GameProfiles::from_layout(Layout {
                controls: vec![Binding {
                    input: String::new(),
                    at: [0.5, 0.5],
                    gesture: Gesture::Dpad {
                        directions: ["I".into(), "K".into(), "J".into(), "L".into()],
                        radius: 0.125,
                        left_stick: true,
                    },
                }],
                ..Default::default()
            }),
        );
        display.set_package(Some("test.game"));
        let events = [Key::I, Key::J]
            .into_iter()
            .flat_map(|key| {
                [true, false].map(|pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
            })
            .collect();
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(960., 640.))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    display.show(ui, true, crate::ui::i18n::Locale::default());
                });
            },
        );

        let mut contacts = Vec::new();
        while let Ok(event) = received.try_recv() {
            let Input::Touch(touch_event) = event else {
                panic!("a mapped key was passed through to Android");
            };
            let [touch] = touch_event.touches.as_slice() else {
                panic!("each phase of the same contact requires its own event");
            };
            contacts.push((touch.identifier, touch.x, touch.y, touch.pressure));
        }
        assert_eq!(
            contacts,
            [
                (1, 80, 48, 1024),
                (1, 80, 36, 1024),
                (1, 80, 36, 0),
                (1, 80, 48, 1024),
                (1, 68, 48, 1024),
                (1, 68, 48, 0),
            ]
        );
    }

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
        assert_eq!(
            loaded.import_profile(&share_path),
            Err(SchemeError::InvalidName)
        );
        let original = std::fs::read_to_string(&share_path).unwrap();
        std::fs::write(
            &share_path,
            original.replace("0.72", "1.1").replace("Race", "Broken"),
        )
        .unwrap();
        assert_eq!(
            loaded.import_profile(&share_path),
            Err(SchemeError::InvalidData)
        );
        assert_eq!(loaded.draft_profiles.active, "Race");
        assert_eq!(loaded.draft.controls[0].at, [0.72, 0.9]);

        std::fs::write(&share_path, b"{").unwrap();
        assert_eq!(
            loaded.import_profile(&share_path),
            Err(SchemeError::InvalidData)
        );

        std::fs::write(&share_path, vec![0; 262_145]).unwrap();
        assert_eq!(
            loaded.import_profile(&share_path),
            Err(SchemeError::TooLarge)
        );
    }
}
