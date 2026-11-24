use super::{Binding, Gesture, Mapping, Point, label};
use crate::ui::theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, StrokeKind, Vec2, vec2};

const DIRECTIONS: [Vec2; 4] = [vec2(0., -1.), vec2(0., 1.), vec2(-1., 0.), vec2(1., 0.)];

enum Marker {
    Control(usize),
    Direction(usize, usize),
    Resize(usize),
    Endpoint(usize),
    LegacyButton(String),
    LegacyStick,
}

pub(super) struct Drag {
    marker: Marker,
    offset: Vec2,
    radius: Option<f32>,
}

struct Dpad {
    center: Pos2,
    radius: f32,
}

impl Dpad {
    fn new(center: Pos2, radius: f32, canvas: Rect) -> Self {
        Self {
            center,
            radius: radius * canvas.width().min(canvas.height()),
        }
    }

    fn keycap(&self, direction: usize) -> Rect {
        let spacing = (self.radius * 0.64).max(26.);
        let center = self.center + DIRECTIONS[direction] * spacing;
        Rect::from_center_size(center, vec2(40., 23.))
    }

    fn handle(&self) -> Pos2 {
        self.center + vec2(0.7071, 0.7071) * self.radius.max(36.)
    }

    fn paint(
        &self,
        painter: &egui::Painter,
        directions: [&str; 4],
        selected: bool,
        listening: Option<usize>,
    ) {
        let ring = self.radius;
        let edge = if selected {
            theme::ACCENT
        } else {
            Color32::from_rgb(119, 145, 159)
        };
        painter.circle_filled(self.center, ring, Color32::from_black_alpha(105));
        painter.circle_stroke(self.center, ring, (1.6, edge));
        painter.circle_stroke(self.center, ring * 0.83, (0.8, edge.gamma_multiply(0.45)));
        for direction in DIRECTIONS {
            painter.line_segment(
                [
                    self.center + direction * (ring * 0.36),
                    self.center + direction * (ring * 0.49),
                ],
                (1., edge),
            );
        }
        painter.circle_filled(self.center, 11., Color32::from_rgb(25, 31, 38));
        painter.circle_stroke(self.center, 11., (1.2, edge));
        painter.circle_filled(self.center, 3., edge);
        for (index, input) in directions.into_iter().enumerate() {
            let cap = self.keycap(index);
            let active = listening == Some(index);
            let highlight = if active { theme::ACCENT } else { edge };
            painter.rect_filled(
                cap.translate(vec2(0., 2.)),
                5,
                Color32::from_black_alpha(170),
            );
            painter.rect_filled(cap, 5, Color32::from_rgb(26, 34, 42));
            painter.rect_stroke(
                cap,
                5,
                (if active { 2. } else { 1. }, highlight),
                StrokeKind::Inside,
            );
            let axis = DIRECTIONS[index];
            let cross = vec2(-axis.y, axis.x);
            let middle = cap.left_center() + vec2(10., 0.);
            let tip = middle + axis * 4.;
            painter.line_segment([middle - axis * 4., tip], (1.2, highlight));
            painter.line_segment([tip, middle + cross * 3.], (1.2, highlight));
            painter.line_segment([tip, middle - cross * 3.], (1.2, highlight));
            let input_rect = Rect::from_min_max(cap.min + vec2(17., 0.), cap.max);
            painter.with_clip_rect(input_rect.shrink(2.)).text(
                cap.right_center() - vec2(5., 0.),
                Align2::RIGHT_CENTER,
                if active { "…" } else { short_label(input) },
                FontId::proportional(11.),
                Color32::WHITE,
            );
        }
        if selected {
            let handle = self.handle();
            painter.line_segment(
                [self.center + (handle - self.center) * 0.81, handle],
                (1., edge),
            );
            painter.rect_filled(Rect::from_center_size(handle, Vec2::splat(11.)), 3, edge);
            painter.line_segment(
                [handle + vec2(-2., 2.), handle + vec2(2., -2.)],
                (1.3, Color32::from_rgb(25, 31, 38)),
            );
        }
    }
}

impl Mapping {
    pub(super) fn canvas(&mut self, ui: &mut egui::Ui, rect: Rect) {
        if self.pending.is_none() {
            self.pending_key = None;
        }
        self.interact_with_canvas(ui, rect);
        self.paint_canvas(ui, rect);
    }

    pub(crate) fn paint_canvas(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter().with_clip_rect(rect);
        for (input, at) in &self.draft.buttons {
            let active = self.pending_key.as_deref() == Some(input.as_str());
            paint_key(
                &painter,
                screen(rect, *at),
                if active { "…" } else { label(input) },
                active,
            );
        }
        if let Some(at) = self.draft.stick {
            Dpad::new(screen(rect, at), self.draft.radius, rect).paint(
                &painter,
                ["W", "S", "A", "D"],
                false,
                None,
            );
        }
        for (index, binding) in self.draft.controls.iter().enumerate() {
            if self.selected_control != Some(index) {
                self.paint_control(&painter, rect, index, binding);
            }
        }
        if let Some(index) = self.selected_control
            && let Some(binding) = self.draft.controls.get(index)
        {
            self.paint_control(&painter, rect, index, binding);
        }
        if let Some(point) = self.pending
            && self.pending_key.is_none()
        {
            paint_key(&painter, screen(rect, point), "…", true);
        }
        if self.editor.palette_drag
            && let Some(pos) = ui.input(|input| input.pointer.hover_pos())
            && self.canvas_contains(rect.intersect(ui.clip_rect()), pos)
        {
            if let Gesture::Dpad {
                directions, radius, ..
            } = &self.preset
            {
                Dpad::new(pos, *radius, rect).paint(
                    &painter,
                    directions.each_ref().map(String::as_str),
                    true,
                    None,
                );
            } else {
                paint_key(&painter, pos, gesture_icon(&self.preset), true);
            }
        }
    }

    fn paint_control(&self, painter: &egui::Painter, rect: Rect, index: usize, binding: &Binding) {
        let at = screen(rect, binding.at);
        let selected = self.selected_control == Some(index);
        let listening = selected && self.await_binding;
        if let Gesture::Dpad {
            directions, radius, ..
        } = &binding.gesture
        {
            let listening = listening.then_some(self.direction_binding).flatten();
            Dpad::new(at, *radius, rect).paint(
                painter,
                directions.each_ref().map(String::as_str),
                selected,
                listening,
            );
            return;
        }
        if let Gesture::Swipe { end, .. } = binding.gesture {
            let end = screen(rect, end);
            painter.arrow(at, end - at, (1.6, theme::ACCENT));
            painter.circle_filled(end, 5., Color32::from_rgb(26, 34, 42));
            painter.circle_stroke(end, 5., (1.5, theme::ACCENT));
        }
        paint_key(
            painter,
            at,
            if listening {
                "…"
            } else if binding.input.is_empty() {
                gesture_icon(&binding.gesture)
            } else {
                label(&binding.input)
            },
            selected,
        );
    }

    pub(super) fn canvas_contains(&self, rect: Rect, pos: Pos2) -> bool {
        rect.contains(pos)
            && self
                .blocked_rect
                .is_none_or(|blocked| !blocked.contains(pos))
    }

    fn interact_with_canvas(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let visible = rect.intersect(ui.clip_rect());
        let response = ui.interact(
            visible,
            ui.id().with("mapping-canvas"),
            egui::Sense::click_and_drag(),
        );
        if self.package.is_none() {
            return;
        }
        let (pos, released) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.button_released(egui::PointerButton::Primary),
            )
        });
        if self.editor.palette_drag {
            if released {
                self.editor.palette_drag = false;
                self.placing_control = false;
                if let Some(pos) = pos.filter(|pos| self.canvas_contains(visible, *pos)) {
                    self.place_control(normalized(rect, pos));
                }
            }
            return;
        }
        if !self.await_binding && self.pending_key.is_none() {
            if response.drag_started_by(egui::PointerButton::Primary)
                && !self.placing_control
                && !self.placing_endpoint
                && let Some(origin) = ui.input(|input| input.pointer.press_origin())
                && self.canvas_contains(visible, origin)
                && let Some(marker) = self.hit_test(rect, origin)
            {
                self.start_canvas_drag(rect, marker, origin);
            }
            if let Some(drag) = self.canvas_drag.take() {
                if let Some(pos) = pos.filter(|pos| self.canvas_contains(visible, *pos)) {
                    self.update_canvas_drag(rect, pos, &drag);
                }
                if !released {
                    self.canvas_drag = Some(drag);
                }
                return;
            }
        }
        let Some(pos) = response
            .interact_pointer_pos()
            .or(pos.filter(|_| response.hovered()))
            .filter(|pos| self.canvas_contains(visible, *pos))
        else {
            return;
        };
        if self.await_binding {
            return;
        }
        let marker = self.hit_test(rect, pos);
        if response.secondary_clicked() {
            match marker {
                Some(
                    Marker::Control(index)
                    | Marker::Direction(index, _)
                    | Marker::Resize(index)
                    | Marker::Endpoint(index),
                ) => {
                    self.draft.controls.remove(index);
                    self.reset_placement();
                }
                Some(Marker::LegacyStick) => self.draft.stick = None,
                Some(Marker::LegacyButton(input)) => {
                    self.draft.buttons.remove(&input);
                }
                None => {}
            }
            self.pending = None;
            self.pending_key = None;
            return;
        }
        if !response.clicked() {
            if let Some(marker) = marker {
                if let Marker::Direction(index, direction) = marker
                    && let Gesture::Dpad { directions, .. } = &self.draft.controls[index].gesture
                    && !directions[direction].is_empty()
                {
                    response.on_hover_text(label(&directions[direction]));
                }
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            return;
        }
        let at = normalized(rect, pos);
        if self.placing_control {
            self.place_control(at);
        } else if self.placing_endpoint {
            if let Some(binding) = self
                .selected_control
                .and_then(|index| self.draft.controls.get_mut(index))
                && let Gesture::Swipe { end, .. } = &mut binding.gesture
            {
                *end = at;
            }
            self.placing_endpoint = false;
        } else {
            self.pending = None;
            self.pending_key = None;
            match marker {
                Some(Marker::Direction(index, direction)) => {
                    self.selected_control = Some(index);
                    self.direction_binding = Some(direction);
                    self.await_binding = true;
                    ui.memory_mut(|memory| {
                        if let Some(focused) = memory.focused() {
                            memory.surrender_focus(focused);
                        }
                    });
                }
                Some(Marker::Control(index)) => {
                    self.selected_control = Some(index);
                    self.await_binding =
                        !matches!(self.draft.controls[index].gesture, Gesture::Dpad { .. });
                    self.direction_binding = None;
                }
                Some(Marker::Endpoint(index) | Marker::Resize(index)) => {
                    self.selected_control = Some(index);
                }
                Some(Marker::LegacyButton(input)) => {
                    self.selected_control = None;
                    self.pending = self.draft.buttons.get(&input).copied();
                    self.pending_key = Some(input);
                }
                Some(Marker::LegacyStick) => self.selected_control = None,
                None if self.place_stick => self.draft.stick = Some(at),
                None if self.place_button => {
                    self.selected_control = None;
                    self.pending = Some(at);
                }
                None => self.selected_control = None,
            }
        }
    }

    fn place_control(&mut self, at: Point) {
        self.reset_placement();
        if self.draft.controls.len() >= 64 {
            return;
        }
        let mut gesture = self.preset.clone();
        if let Gesture::Swipe { end, .. } = &mut gesture {
            *end = [at[0], (at[1] - 0.18).max(0.)];
        }
        self.selected_control = Some(self.draft.controls.len());
        self.await_binding =
            self.editor.preset_input.is_empty() && !matches!(gesture, Gesture::Dpad { .. });
        self.draft.controls.push(Binding {
            input: self.editor.preset_input.to_owned(),
            at,
            gesture,
        });
    }

    fn hit_test(&self, rect: Rect, pos: Pos2) -> Option<Marker> {
        let hit = |index, binding: &Binding| {
            let at = screen(rect, binding.at);
            if let Gesture::Dpad { radius, .. } = binding.gesture {
                let dpad = Dpad::new(at, radius, rect);
                if self.selected_control == Some(index) && dpad.handle().distance(pos) <= 9. {
                    return Some(Marker::Resize(index));
                }
                return (0..4)
                    .find(|&direction| dpad.keycap(direction).contains(pos))
                    .map(|direction| Marker::Direction(index, direction))
                    .or_else(|| (at.distance(pos) <= 14.).then_some(Marker::Control(index)));
            }
            let text = if binding.input.is_empty() {
                gesture_icon(&binding.gesture)
            } else {
                label(&binding.input)
            };
            if key_rect(at, text).expand(3.).contains(pos) {
                return Some(Marker::Control(index));
            }
            if let Gesture::Swipe { end, .. } = binding.gesture
                && screen(rect, end).distance(pos) <= 10.
            {
                return Some(Marker::Endpoint(index));
            }
            None
        };
        if let Some(index) = self.selected_control
            && let Some(marker) = self
                .draft
                .controls
                .get(index)
                .and_then(|binding| hit(index, binding))
        {
            return Some(marker);
        }
        self.draft
            .controls
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, binding)| hit(index, binding))
            .or_else(|| {
                self.draft
                    .stick
                    .filter(|at| {
                        key_rect(screen(rect, *at), "WASD / L")
                            .expand(3.)
                            .contains(pos)
                    })
                    .map(|_| Marker::LegacyStick)
            })
            .or_else(|| {
                self.draft
                    .buttons
                    .iter()
                    .rev()
                    .find(|(input, at)| {
                        key_rect(screen(rect, **at), label(input))
                            .expand(3.)
                            .contains(pos)
                    })
                    .map(|(input, _)| Marker::LegacyButton(input.clone()))
            })
    }

    fn start_canvas_drag(&mut self, rect: Rect, marker: Marker, origin: Pos2) {
        let at = match marker {
            Marker::Control(index) | Marker::Resize(index) => {
                self.selected_control = Some(index);
                self.draft.controls[index].at
            }
            Marker::Endpoint(index) => {
                let Gesture::Swipe { end, .. } = self.draft.controls[index].gesture else {
                    return;
                };
                self.selected_control = Some(index);
                end
            }
            Marker::LegacyStick => self.draft.stick.unwrap_or_default(),
            Marker::LegacyButton(ref input) => self.draft.buttons[input],
            Marker::Direction(..) => return,
        };
        self.pending = None;
        let radius = if let Marker::Resize(index) = marker
            && let Gesture::Dpad { radius, .. } = self.draft.controls[index].gesture
        {
            Some(radius)
        } else {
            None
        };
        self.canvas_drag = Some(Drag {
            marker,
            offset: screen(rect, at) - origin,
            radius,
        });
    }

    fn update_canvas_drag(&mut self, rect: Rect, pos: Pos2, drag: &Drag) {
        let at = normalized(rect, pos + drag.offset);
        match drag.marker {
            Marker::Control(index) => self.draft.controls[index].at = at,
            Marker::Endpoint(index) => {
                if let Gesture::Swipe { end, .. } = &mut self.draft.controls[index].gesture {
                    *end = at;
                }
            }
            Marker::Resize(index) => {
                let center = screen(rect, self.draft.controls[index].at);
                if let Some(initial_radius) = drag.radius
                    && let Gesture::Dpad { radius, .. } = &mut self.draft.controls[index].gesture
                {
                    let delta = (pos - center).length() - drag.offset.length();
                    let minimum = rect.width().min(rect.height());
                    // Match the radius range used by the gameplay gesture.
                    *radius = (initial_radius + delta / minimum).clamp(0.02, 0.35);
                }
            }
            Marker::LegacyStick => self.draft.stick = Some(at),
            Marker::LegacyButton(ref input) => {
                self.draft.buttons.insert(input.clone(), at);
            }
            Marker::Direction(..) => {}
        }
    }
}

fn screen(rect: Rect, at: Point) -> Pos2 {
    rect.min + rect.size() * vec2(at[0], at[1])
}

fn normalized(rect: Rect, pos: Pos2) -> Point {
    let at = (pos - rect.min) / rect.size();
    [at.x.clamp(0., 1.), at.y.clamp(0., 1.)]
}

fn key_rect(at: Pos2, text: &str) -> Rect {
    let width = 20. + text.chars().count().min(16) as f32 * 6.;
    Rect::from_center_size(at, vec2(width.max(32.), 31.))
}

fn paint_key(painter: &egui::Painter, at: Pos2, text: &str, selected: bool) {
    let cap = key_rect(at, text);
    let edge = if selected {
        theme::ACCENT
    } else {
        Color32::from_rgb(119, 145, 159)
    };
    painter.rect_filled(
        cap.translate(vec2(0., 2.)),
        8,
        Color32::from_black_alpha(170),
    );
    painter.rect_filled(cap, 8, Color32::from_rgb(26, 34, 42));
    painter.rect_stroke(cap, 8, (1.4, edge), StrokeKind::Inside);
    painter.with_clip_rect(cap.shrink(4.)).text(
        at,
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.),
        Color32::WHITE,
    );
}

fn short_label(input: &str) -> &str {
    match input {
        "Up" | "ArrowUp" => "↑",
        "Down" | "ArrowDown" => "↓",
        "Left" | "ArrowLeft" => "←",
        "Right" | "ArrowRight" => "→",
        "Mouse:Primary" => "M1",
        "Mouse:Secondary" => "M2",
        "Mouse:Middle" => "M3",
        "Mouse:Extra1" => "M4",
        "Mouse:Extra2" => "M5",
        "Wheel:Up" => "W↑",
        "Wheel:Down" => "W↓",
        "Wheel:Left" => "W←",
        "Wheel:Right" => "W→",
        "Pad:South" => "A",
        "Pad:East" => "B",
        "Pad:West" => "X",
        "Pad:North" => "Y",
        "Pad:DPadUp" => "P↑",
        "Pad:DPadDown" => "P↓",
        "Pad:DPadLeft" => "P←",
        "Pad:DPadRight" => "P→",
        "" => "·",
        other => label(other),
    }
}

fn gesture_icon(gesture: &Gesture) -> &'static str {
    match gesture {
        Gesture::Hold | Gesture::Tap { .. } => "●",
        Gesture::Swipe { .. } => "↗",
        Gesture::Repeat { .. } => "×2",
        Gesture::Dpad { .. } => "✚",
        Gesture::Aim { .. } => "⊕",
        Gesture::Pinch { .. } => "↔",
        Gesture::Rotate { .. } => "↻",
        Gesture::Moba { .. } => "◎",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, PointerButton, pos2};

    #[test]
    fn directional_drop_rebind_move_and_resize_use_canvas_coordinates() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(80., 60.), vec2(800., 500.));
        let mut mapping = Mapping::default();
        mapping.pad_initialized = true;
        mapping.select(Some("test.game"));
        mapping.toggle();
        mapping.preset = Gesture::Dpad {
            directions: ["W".into(), "S".into(), "A".into(), "D".into()],
            radius: 0.12,
            left_stick: true,
        };
        mapping.blocked_rect = Some(Rect::from_min_size(pos2(650., 60.), vec2(230., 500.)));
        let render = |mapping: &mut Mapping, events: Vec<Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    events: events.clone(),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(960., 620.))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        mapping.frame(ui, rect, &events);
                    });
                },
            );
        };
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        render(&mut mapping, vec![]);
        mapping.editor.palette_drag = true;
        let blocked = pos2(760., 200.);
        render(
            &mut mapping,
            vec![Event::PointerMoved(blocked), button(blocked, true)],
        );
        render(&mut mapping, vec![button(blocked, false)]);
        assert!(mapping.draft.controls.is_empty());

        mapping.editor.palette_drag = true;
        let at = [0.25, 0.6];
        let center = screen(rect, at);
        render(
            &mut mapping,
            vec![Event::PointerMoved(blocked), button(blocked, true)],
        );
        render(
            &mut mapping,
            vec![Event::PointerMoved(center), button(center, false)],
        );
        assert_eq!(mapping.draft.controls[0].at, at);

        let north = Dpad::new(center, 0.12, rect).keycap(0).center();
        render(
            &mut mapping,
            vec![Event::PointerMoved(north), button(north, true)],
        );
        render(&mut mapping, vec![button(north, false)]);
        assert_eq!(mapping.direction_binding, Some(0));
        render(
            &mut mapping,
            vec![Event::Key {
                key: Key::ArrowUp,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(!mapping.await_binding);

        let new_at = [0.45, 0.5];
        let new_center = screen(rect, new_at);
        render(
            &mut mapping,
            vec![Event::PointerMoved(center), button(center, true)],
        );
        render(&mut mapping, vec![Event::PointerMoved(new_center)]);
        render(&mut mapping, vec![button(new_center, false)]);
        assert_eq!(mapping.draft.controls[0].at, new_at);

        let handle = Dpad::new(new_center, 0.12, rect).handle();
        let new_handle = Dpad::new(new_center, 0.2, rect).handle();
        render(
            &mut mapping,
            vec![Event::PointerMoved(handle), button(handle, true)],
        );
        render(&mut mapping, vec![Event::PointerMoved(new_handle)]);
        render(&mut mapping, vec![]);
        render(&mut mapping, vec![button(new_handle, false)]);
        let Gesture::Dpad {
            directions, radius, ..
        } = &mapping.draft.controls[0].gesture
        else {
            panic!("expected the D-pad");
        };
        assert_eq!(directions, &[Key::ArrowUp.name(), "S", "A", "D"]);
        assert!((radius - 0.2).abs() < 0.0001);
    }
}
