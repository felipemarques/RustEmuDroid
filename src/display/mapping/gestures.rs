use super::{Contact, Point};
use eframe::egui::{Vec2, vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Binding {
    pub input: String,
    pub at: Point,
    pub gesture: Gesture,
}

pub(super) const MAX_TILT: f32 = 45.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DpadStick {
    Off,
    Left,
    Right,
}

impl DpadStick {
    pub(super) fn from_flags(left: bool, right: bool) -> Self {
        // Right takes precedence if an imported profile enables both sticks.
        match (left, right) {
            (_, true) => Self::Right,
            (true, false) => Self::Left,
            (false, false) => Self::Off,
        }
    }

    pub(super) fn flags(self) -> (bool, bool) {
        (self == Self::Left, self == Self::Right)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum TiltAxis {
    #[default]
    X,
    Y,
    Z,
}

pub(super) fn tilt_degrees(degrees: f32) -> f32 {
    if degrees.is_finite() {
        degrees.clamp(-MAX_TILT, MAX_TILT)
    } else {
        0.
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(super) enum Gesture {
    #[default]
    Hold,
    Tap {
        duration_ms: u32,
    },
    Repeat {
        interval_ms: u32,
        hold_ms: u32,
    },
    Swipe {
        end: Point,
        duration_ms: u32,
    },
    Dpad {
        directions: [String; 4],
        radius: f32,
        left_stick: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        right_stick: bool,
    },
    Aim {
        sensitivity: f32,
        mouse: bool,
        right_stick: bool,
        toggle: bool,
    },
    Pinch {
        from: f32,
        to: f32,
        duration_ms: u32,
    },
    Rotate {
        radius: f32,
        degrees: f32,
        duration_ms: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wheel: Option<SteeringWheel>,
    },
    Moba {
        radius: f32,
        quick_cast: bool,
    },
    MobaMove {
        hero: Point,
        radius: f32,
        speed: f32,
    },
    EdgeScroll {
        margin_x: f32,
        margin_y: f32,
        speed_x: f32,
        speed_y: f32,
    },
    Tilt {
        axis: TiltAxis,
        degrees: f32,
    },
    FreeLook {
        mouse: bool,
        directions: [String; 4],
        speed: f32,
        reset_ms: u32,
    },
    Scroll {
        amplitude: f32,
        speed: f32,
        override_wheel: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct SteeringWheel {
    pub start_degrees: f32,
    pub reverse_input: String,
}

impl Gesture {
    pub(super) fn unique_tool(&self) -> bool {
        matches!(
            self,
            Self::MobaMove { .. } | Self::EdgeScroll { .. } | Self::Scroll { .. }
        )
    }

    pub(super) fn needs_binding(&self) -> bool {
        !matches!(
            self,
            Self::Dpad { .. }
                | Self::EdgeScroll { .. }
                | Self::MobaMove { .. }
                | Self::FreeLook { mouse: false, .. }
                | Self::Scroll { .. }
        )
    }

    pub(super) fn endpoint_mut(&mut self) -> Option<&mut Point> {
        match self {
            Self::Swipe { end, .. } => Some(end),
            Self::MobaMove { hero, .. } => Some(hero),
            _ => None,
        }
    }

    pub(super) fn endpoint(&self) -> Option<Point> {
        match *self {
            Self::Swipe { end, .. } => Some(end),
            Self::MobaMove { hero, .. } => Some(hero),
            _ => None,
        }
    }
}

impl Binding {
    pub(super) fn primary_input(&self) -> &str {
        if matches!(self.gesture, Gesture::MobaMove { .. }) {
            "Mouse:Secondary"
        } else {
            &self.input
        }
    }

    pub(super) fn captures(&self, input: &str) -> bool {
        !input.is_empty()
            && match &self.gesture {
                Gesture::Dpad { directions, .. }
                | Gesture::FreeLook {
                    mouse: false,
                    directions,
                    ..
                } => directions.iter().any(|key| key == input),
                Gesture::EdgeScroll { .. } | Gesture::Scroll { .. } => false,
                Gesture::Rotate {
                    wheel: Some(wheel), ..
                } => input == self.input || input == wheel.reverse_input,
                Gesture::MobaMove { .. } => input == "Mouse:Secondary" || input == self.input,
                _ => self.input == input,
            }
    }
}

#[derive(Default)]
pub(super) struct Gestures {
    held: HashSet<String>,
    active: BTreeMap<usize, Active>,
    /// Kept separate because a repeat can temporarily have no down contact.
    deadlines: BTreeMap<usize, f64>,
    toggled: HashSet<usize>,
    edge_scrolling: bool,
    scrolls: VecDeque<Vec2>,
    pub left: Vec2,
    pub right: Vec2,
    pub mouse: Vec2,
}

struct Active {
    started: f64,
    last_moved: f64,
    contacts: Vec<Contact>,
    target: Option<Point>,
    steering: f32,
}

impl Gestures {
    pub(super) fn used(&self, id: i32) -> bool {
        self.active
            .values()
            .any(|active| active.contacts.iter().any(|c| c.id == id))
    }

    pub(super) fn waiting(&self) -> bool {
        self.edge_scrolling
            || !self.active.is_empty()
            || !self.deadlines.is_empty()
            || !self.toggled.is_empty()
            || !self.scrolls.is_empty()
    }

    pub(super) fn queue_scroll(&mut self, direction: Vec2) {
        if self.scrolls.len() == 3 {
            self.scrolls.pop_front();
        }
        self.scrolls.push_back(direction);
    }

    pub(super) fn aiming_mouse(&self, controls: &[Binding]) -> bool {
        controls
            .iter()
            .enumerate()
            .any(|(index, binding)| match binding.gesture {
                Gesture::Aim { mouse: true, .. } => self.aiming(index, binding),
                Gesture::FreeLook { mouse: true, .. } => self.held.contains(&binding.input),
                _ => false,
            })
    }

    fn aiming(&self, index: usize, binding: &Binding) -> bool {
        let Gesture::Aim { toggle, .. } = binding.gesture else {
            return false;
        };
        if toggle {
            self.toggled.contains(&index)
        } else {
            self.held.contains(&binding.input)
        }
    }

    pub(super) fn input(
        &mut self,
        controls: &[Binding],
        input: &str,
        down: bool,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        if !down {
            if !self.held.remove(input) {
                return;
            }
            for (i, binding) in controls
                .iter()
                .enumerate()
                .filter(|(_, b)| b.captures(input))
            {
                match binding.gesture {
                    Gesture::Dpad { .. } => self.update_dpad(i, binding, frame, changes),
                    Gesture::FreeLook {
                        mouse,
                        ref directions,
                        ..
                    } if mouse || self.direction_vector(directions) == Vec2::ZERO => {
                        self.stop(i, changes);
                        self.deadlines.remove(&i);
                    }
                    Gesture::Rotate { wheel: Some(_), .. } => {
                        self.update_wheel(i, binding, frame, changes)
                    }
                    Gesture::Hold | Gesture::Repeat { .. } | Gesture::Aim { toggle: false, .. } => {
                        self.stop(i, changes);
                        self.deadlines.remove(&i);
                    }
                    Gesture::Moba { radius, quick_cast } => {
                        if quick_cast {
                            if self.start(i, &[binding.at], frame, changes)
                                && let Some(active) = self.active.get_mut(&i)
                            {
                                active.target = Some(moba_target(binding.at, radius, frame));
                            }
                        } else {
                            self.stop(i, changes);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        if !self.held.insert(input.to_owned()) {
            return;
        }
        for (i, binding) in controls
            .iter()
            .enumerate()
            .filter(|(_, b)| b.captures(input))
        {
            if let Gesture::MobaMove {
                hero,
                radius,
                speed,
            } = binding.gesture
            {
                self.stop(i, changes);
                self.deadlines.remove(&i);
                if input == "Mouse:Secondary"
                    && let Some(cursor) = frame.cursor
                {
                    let delta = (point(cursor) - point(hero)) * frame.size;
                    let distance = delta.length() / frame.size.min_elem();
                    if distance > 0.01 && self.start(i, &[binding.at], frame, changes) {
                        let target = offset(frame.size, binding.at, delta.normalized() * radius);
                        self.move_to(i, &[target], changes);
                        self.deadlines.insert(
                            i,
                            frame.time + f64::from((distance / speed).clamp(0.035, 5.)),
                        );
                    }
                }
            } else if matches!(binding.gesture, Gesture::Dpad { .. }) {
                self.update_dpad(i, binding, frame, changes);
            } else if matches!(binding.gesture, Gesture::Aim { toggle: true, .. }) {
                if !self.toggled.insert(i) {
                    self.toggled.remove(&i);
                    self.stop(i, changes);
                }
            } else if matches!(
                binding.gesture,
                Gesture::Hold
                    | Gesture::Tap { .. }
                    | Gesture::Swipe { .. }
                    | Gesture::Moba {
                        quick_cast: false,
                        ..
                    }
            ) {
                self.start(i, &[binding.at], frame, changes);
            } else if let Gesture::Repeat { interval_ms, .. } = binding.gesture {
                if self.start(i, &[binding.at], frame, changes) {
                    self.deadlines
                        .insert(i, frame.time + repeat_period(interval_ms));
                }
            } else if let Gesture::Pinch { from, .. } = binding.gesture {
                self.start(i, &pair(frame.size, binding.at, from, 0.), frame, changes);
            } else if let Gesture::Rotate { wheel: Some(_), .. } = binding.gesture {
                self.update_wheel(i, binding, frame, changes);
            } else if let Gesture::Rotate { radius, .. } = binding.gesture {
                self.start(i, &pair(frame.size, binding.at, radius, 0.), frame, changes);
            }
        }
    }

    pub(super) fn frame(
        &mut self,
        controls: &[Binding],
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        let mouse = std::mem::take(&mut self.mouse);
        self.edge_scrolling = false;
        let free_look_active = controls.iter().any(|binding| match &binding.gesture {
            Gesture::FreeLook { mouse: true, .. } => self.held.contains(&binding.input),
            Gesture::FreeLook {
                mouse: false,
                directions,
                ..
            } => directions.iter().any(|key| self.held.contains(key)),
            _ => false,
        });
        if free_look_active {
            for (index, _) in controls
                .iter()
                .enumerate()
                .filter(|(_, binding)| matches!(binding.gesture, Gesture::Aim { .. }))
            {
                self.stop(index, changes);
            }
        }
        for (index, binding) in controls.iter().enumerate() {
            let has_contacts = self.active.contains_key(&index);
            let elapsed = self
                .active
                .get(&index)
                .map_or(0., |active| frame.time - active.started);
            match &binding.gesture {
                Gesture::Hold => {}
                Gesture::MobaMove { .. }
                    if self
                        .deadlines
                        .get(&index)
                        .is_some_and(|end| frame.time >= *end) =>
                {
                    self.stop(index, changes);
                    self.deadlines.remove(&index);
                }
                Gesture::Tap { duration_ms } => {
                    if has_contacts && elapsed >= duration(*duration_ms) {
                        self.stop(index, changes);
                    }
                }
                Gesture::Repeat {
                    interval_ms,
                    hold_ms,
                } => {
                    if !self.held.contains(&binding.input) {
                        continue;
                    }
                    let period = repeat_period(*interval_ms);
                    if has_contacts && elapsed >= duration(*hold_ms).min(period * 0.75) {
                        self.stop(index, changes);
                    }
                    if !self.active.contains_key(&index)
                        && self
                            .deadlines
                            .get(&index)
                            .is_none_or(|deadline| frame.time >= *deadline)
                        && self.start(index, &[binding.at], frame, changes)
                    {
                        self.deadlines.insert(index, frame.time + period);
                    }
                }
                Gesture::Swipe { end, duration_ms } if has_contacts => {
                    let progress = (elapsed / duration(*duration_ms)).clamp(0., 1.) as f32;
                    let at = interpolate(binding.at, *end, progress);
                    self.move_to(index, &[at], changes);
                    if progress >= 1. {
                        self.stop(index, changes);
                    }
                }
                Gesture::Dpad { .. } => self.update_dpad(index, binding, frame, changes),
                Gesture::FreeLook { .. } => {
                    self.update_free_look(index, binding, mouse, frame, changes)
                }
                Gesture::Scroll {
                    amplitude, speed, ..
                } => {
                    self.update_scroll(index, binding.at, *amplitude, *speed, frame, changes);
                }
                Gesture::EdgeScroll {
                    margin_x,
                    margin_y,
                    speed_x,
                    speed_y,
                } => {
                    let velocity = frame.edge_cursor.map_or(Vec2::ZERO, |[x, y]| {
                        vec2(
                            edge_direction(x, *margin_x) * speed_x.clamp(0.20, 2.),
                            edge_direction(y, *margin_y) * speed_y.clamp(0.20, 2.),
                        )
                    });
                    self.update_edge_scroll(index, binding.at, velocity, frame, changes);
                }
                Gesture::Aim {
                    sensitivity,
                    mouse: use_mouse,
                    right_stick,
                    ..
                } => {
                    if free_look_active || !self.aiming(index, binding) {
                        self.stop(index, changes);
                        continue;
                    }
                    let mouse_delta = if *use_mouse { mouse } else { Vec2::ZERO };
                    let stick_delta = if *right_stick {
                        deadzone(self.right)
                            * (frame.dt.min(0.05) * frame.size.x.min(frame.size.y) * 0.55)
                    } else {
                        Vec2::ZERO
                    };
                    let delta = (mouse_delta + stick_delta) * sensitivity.clamp(0.05, 8.);
                    if delta == Vec2::ZERO {
                        if self
                            .active
                            .get(&index)
                            .is_some_and(|active| frame.time - active.last_moved > 0.16)
                        {
                            self.stop(index, changes);
                        }
                        continue;
                    }
                    let step = limited(vec2(delta.x / frame.size.x, delta.y / frame.size.y), 0.20);
                    let base = self
                        .active
                        .get(&index)
                        .and_then(|a| a.contacts.first())
                        .map_or(binding.at, |c| c.pos);
                    let mut target = point(base) + step;
                    if (target - point(binding.at)).length() > 0.26
                        || !(0.001..0.999).contains(&target.x)
                        || !(0.001..0.999).contains(&target.y)
                    {
                        self.stop(index, changes);
                        target = point(binding.at) + step;
                    }
                    self.start(index, &[binding.at], frame, changes);
                    self.move_to(index, &[position(target)], changes);
                    if let Some(active) = self.active.get_mut(&index) {
                        active.last_moved = frame.time;
                    }
                }
                Gesture::Pinch {
                    from,
                    to,
                    duration_ms,
                } if has_contacts => {
                    let progress = (elapsed / duration(*duration_ms)).clamp(0., 1.) as f32;
                    let distance = from + (to - from) * progress;
                    self.move_to(index, &pair(frame.size, binding.at, distance, 0.), changes);
                    if progress >= 1. {
                        self.stop(index, changes);
                    }
                }
                Gesture::Rotate { wheel: Some(_), .. } => {
                    self.update_wheel(index, binding, frame, changes)
                }
                Gesture::Rotate {
                    radius,
                    degrees,
                    duration_ms,
                    wheel: None,
                } if has_contacts => {
                    let progress = (elapsed / duration(*duration_ms)).clamp(0., 1.) as f32;
                    let angle = degrees.clamp(-720., 720.).to_radians() * progress;
                    self.move_to(
                        index,
                        &pair(frame.size, binding.at, *radius, angle),
                        changes,
                    );
                    if progress >= 1. {
                        self.stop(index, changes);
                    }
                }
                Gesture::Moba { radius, quick_cast } if has_contacts => {
                    if *quick_cast {
                        let target = self.active[&index].target.unwrap_or(binding.at);
                        let progress = (elapsed / 0.075).clamp(0., 1.) as f32;
                        self.move_to(index, &[interpolate(binding.at, target, progress)], changes);
                        if progress >= 1. {
                            self.stop(index, changes);
                        }
                    } else {
                        self.move_to(index, &[moba_target(binding.at, *radius, frame)], changes);
                    }
                }
                _ => {}
            }
        }
    }

    fn update_free_look(
        &mut self,
        index: usize,
        binding: &Binding,
        mouse_delta: Vec2,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        let Gesture::FreeLook {
            mouse,
            directions,
            speed,
            reset_ms,
        } = &binding.gesture
        else {
            return;
        };
        let digital = self.direction_vector(directions);
        let enabled = if *mouse {
            self.held.contains(&binding.input)
        } else {
            digital != Vec2::ZERO
        };
        if !enabled {
            self.stop(index, changes);
            self.deadlines.remove(&index);
            return;
        }
        if self
            .deadlines
            .get(&index)
            .is_some_and(|deadline| frame.time < *deadline)
        {
            return;
        }
        self.deadlines.remove(&index);
        let speed = speed.clamp(0.20, 3.);
        let delta = if *mouse {
            mouse_delta / frame.size.x.min(frame.size.y) * speed
        } else {
            limited(digital, 1.) * speed * frame.dt.clamp(0., 0.05)
        };
        let current = self
            .active
            .get(&index)
            .map_or(binding.at, |active| active.contacts[0].pos);
        let next = offset(frame.size, current, limited(delta, 0.20));
        if next == current {
            return;
        }
        self.start(index, &[binding.at], frame, changes);
        self.move_to(index, &[next], changes);
        if (delta.x != 0. && !(0.001..0.999).contains(&next[0]))
            || (delta.y != 0. && !(0.001..0.999).contains(&next[1]))
        {
            self.stop(index, changes);
            self.deadlines
                .insert(index, frame.time + f64::from((*reset_ms).min(1000)) / 1000.);
        }
    }

    fn update_scroll(
        &mut self,
        index: usize,
        anchor: Point,
        amplitude: f32,
        speed: f32,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        let amplitude = amplitude.clamp(0.02, 0.45);
        if let Some(active) = self.active.get(&index) {
            let target = active.target.expect("a scroll has an endpoint");
            let length = (point(target) - point(anchor)) * frame.size;
            let distance = length.length() / frame.size.x.min(frame.size.y);
            let duration = f64::from((distance / speed.clamp(0.20, 3.)).max(0.02));
            let progress = ((frame.time - active.started) / duration).clamp(0., 1.) as f32;
            self.move_to(index, &[interpolate(anchor, target, progress)], changes);
            if progress >= 1. {
                self.stop(index, changes);
            }
        } else if let Some(&direction) = self.scrolls.front() {
            let target = offset(frame.size, anchor, direction * amplitude);
            if target == anchor {
                self.scrolls.pop_front();
            } else if self.start(index, &[anchor], frame, changes) {
                self.scrolls.pop_front();
                self.active
                    .get_mut(&index)
                    .expect("scroll just started")
                    .target = Some(target);
            }
        }
    }

    fn update_wheel(
        &mut self,
        index: usize,
        binding: &Binding,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        let Gesture::Rotate {
            radius,
            degrees,
            duration_ms,
            wheel: Some(wheel),
        } = &binding.gesture
        else {
            return;
        };
        let forward = self.held.contains(&binding.input);
        let reverse = self.held.contains(&wheel.reverse_input);
        if forward == reverse {
            self.stop(index, changes);
            return;
        }
        let position = |turn: f32| {
            let angle = (wheel.start_degrees + turn * degrees).to_radians();
            offset(
                frame.size,
                binding.at,
                vec2(angle.cos(), angle.sin()) * radius.clamp(0.005, 0.4),
            )
        };
        let Some(active) = self.active.get_mut(&index) else {
            self.start(index, &[position(0.)], frame, changes);
            return;
        };
        let elapsed = (frame.time - active.last_moved).max(0.);
        active.last_moved = frame.time;
        let step = (elapsed / duration(*duration_ms)) as f32;
        active.steering = (active.steering + if forward { step } else { -step }).clamp(-1., 1.);
        let pos = position(active.steering);
        self.move_to(index, &[pos], changes);
    }

    fn update_edge_scroll(
        &mut self,
        index: usize,
        anchor: Point,
        velocity: Vec2,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        if velocity == Vec2::ZERO || offset(frame.size, anchor, velocity) == anchor {
            self.stop(index, changes);
            return;
        }
        self.edge_scrolling = true;
        let current = self
            .active
            .get(&index)
            .map_or(anchor, |active| active.contacts[0].pos);
        let size = frame.size;
        let displacement = (point(current) - point(anchor)) * size / size.x.min(size.y);
        let next = displacement + velocity * frame.dt.clamp(0., 0.05);
        let limit = 0.20;
        let position = offset(size, anchor, limited(next, limit));
        if position == current {
            if frame.dt > 0. {
                self.stop(index, changes);
            }
            return;
        }
        self.start(index, &[anchor], frame, changes);
        self.move_to(index, &[position], changes);
        if next.length_sq() >= limit * limit {
            self.stop(index, changes);
        }
    }

    fn update_dpad(
        &mut self,
        index: usize,
        binding: &Binding,
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) {
        let Gesture::Dpad {
            directions,
            radius,
            left_stick,
            right_stick,
        } = &binding.gesture
        else {
            return;
        };
        let analog = match DpadStick::from_flags(*left_stick, *right_stick) {
            DpadStick::Off => Vec2::ZERO,
            DpadStick::Left => deadzone(self.left),
            DpadStick::Right => deadzone(self.right),
        };
        let movement = analog + self.direction_vector(directions);
        if movement != Vec2::ZERO {
            let pos = offset(
                frame.size,
                binding.at,
                limited(movement, 1.) * radius.clamp(0.02, 0.35),
            );
            self.start(index, &[binding.at], frame, changes);
            self.move_to(index, &[pos], changes);
        } else {
            self.stop(index, changes);
        }
    }

    fn direction_vector(&self, directions: &[String; 4]) -> Vec2 {
        let axes = [vec2(0., -1.), vec2(0., 1.), vec2(-1., 0.), vec2(1., 0.)];
        directions
            .iter()
            .zip(axes)
            .filter(|(key, _)| self.held.contains(*key))
            .fold(Vec2::ZERO, |movement, (_, axis)| movement + axis)
    }

    fn start(
        &mut self,
        index: usize,
        points: &[Point],
        frame: &Frame<'_>,
        changes: &mut Vec<Contact>,
    ) -> bool {
        if self.active.contains_key(&index) {
            return false;
        }
        let ids = (1..=9).filter(|id| !frame.taken.contains(id) && !self.used(*id));
        let contacts: Vec<_> = ids
            .zip(points)
            .map(|(id, &pos)| Contact {
                id,
                pos: position(point(pos)),
                down: true,
            })
            .collect();
        if contacts.len() != points.len() {
            return false;
        }
        changes.extend(contacts.iter().copied());
        self.active.insert(
            index,
            Active {
                started: frame.time,
                last_moved: frame.time,
                contacts,
                target: None,
                steering: 0.,
            },
        );
        true
    }

    fn move_to(&mut self, index: usize, points: &[Point], changes: &mut Vec<Contact>) {
        let Some(active) = self.active.get_mut(&index) else {
            return;
        };
        for (contact, &pos) in active.contacts.iter_mut().zip(points) {
            if contact.pos != pos {
                contact.pos = pos;
                changes.push(*contact);
            }
        }
    }

    fn stop(&mut self, index: usize, changes: &mut Vec<Contact>) {
        if let Some(active) = self.active.remove(&index) {
            changes.extend(
                active
                    .contacts
                    .into_iter()
                    .map(|c| Contact { down: false, ..c }),
            );
        }
    }

    pub(super) fn release(&mut self) -> impl Iterator<Item = Contact> {
        let active = std::mem::take(&mut self.active);
        *self = Self::default();
        active
            .into_values()
            .flat_map(|active| active.contacts)
            .map(|c| Contact { down: false, ..c })
    }

    pub(super) fn cancel_aim(&mut self, controls: &[Binding], changes: &mut Vec<Contact>) {
        for (index, binding) in controls.iter().enumerate() {
            if matches!(
                binding.gesture,
                Gesture::Aim { .. } | Gesture::FreeLook { mouse: true, .. }
            ) {
                self.held.remove(&binding.input);
                self.toggled.remove(&index);
                self.deadlines.remove(&index);
                self.stop(index, changes);
            }
        }
    }
}

pub(super) struct Frame<'a> {
    pub time: f64,
    pub dt: f32,
    pub size: Vec2,
    pub taken: &'a [i32],
    pub cursor: Option<Point>,
    pub edge_cursor: Option<Point>,
}

fn edge_direction(cursor: f32, margin: f32) -> f32 {
    let margin = margin.clamp(0.01, 0.25);
    // Drag the map against the edge so the camera moves toward it.
    if cursor < margin {
        1.
    } else if cursor > 1. - margin {
        -1.
    } else {
        0.
    }
}

fn duration(ms: u32) -> f64 {
    f64::from(ms.clamp(20, 10_000)) / 1000.
}

fn repeat_period(ms: u32) -> f64 {
    f64::from(ms.clamp(40, 10_000)) / 1000.
}

fn deadzone(v: Vec2) -> Vec2 {
    if v.length_sq() < 0.16 * 0.16 {
        Vec2::ZERO
    } else {
        limited(v, 1.)
    }
}

fn limited(vector: Vec2, max: f32) -> Vec2 {
    if vector.length_sq() <= max * max {
        vector
    } else {
        vector.normalized() * max
    }
}

fn interpolate(from: Point, to: Point, progress: f32) -> Point {
    position(point(from) + (point(to) - point(from)) * progress)
}

fn moba_target(center: Point, radius: f32, frame: &Frame<'_>) -> Point {
    let Some(cursor) = frame.cursor else {
        return center;
    };
    let delta = point(cursor) - point(center);
    let normalized =
        vec2(delta.x * frame.size.x, delta.y * frame.size.y) / frame.size.x.min(frame.size.y);
    offset(
        frame.size,
        center,
        limited(normalized, radius.clamp(0.005, 0.40)),
    )
}

fn point(p: Point) -> Vec2 {
    vec2(p[0], p[1])
}

fn position(v: Vec2) -> Point {
    [v.x.clamp(0., 1.), v.y.clamp(0., 1.)]
}

fn offset(size: Vec2, base: Point, delta: Vec2) -> Point {
    let scaled = delta * size.x.min(size.y);
    position(point(base) + vec2(scaled.x / size.x, scaled.y / size.y))
}

fn pair(size: Vec2, center: Point, radius: f32, angle: f32) -> [Point; 2] {
    let direction = vec2(angle.cos(), angle.sin()) * radius.clamp(0.005, 0.40);
    [
        offset(size, center, direction),
        offset(size, center, -direction),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(time: f64) -> Frame<'static> {
        Frame {
            time,
            dt: 1. / 60.,
            size: vec2(1600., 900.),
            taken: &[1, 9],
            cursor: Some([0.2, 0.8]),
            edge_cursor: None,
        }
    }

    #[test]
    fn dpad_profiles_preserve_legacy_choices_and_prefer_right_if_both_are_enabled() {
        for (left, right, expected) in [
            (false, None, DpadStick::Off),
            (true, None, DpadStick::Left),
            (false, Some(true), DpadStick::Right),
            (true, Some(true), DpadStick::Right),
        ] {
            let mut json = serde_json::json!({
                "kind": "dpad",
                "directions": ["W", "S", "A", "D"],
                "radius": 0.125,
                "left_stick": left,
            });
            if let Some(right) = right {
                json["right_stick"] = right.into();
            }
            let gesture = serde_json::from_value::<Gesture>(json.clone()).unwrap();
            let Gesture::Dpad {
                left_stick,
                right_stick,
                ..
            } = gesture
            else {
                panic!("expected D-pad");
            };
            assert_eq!(DpadStick::from_flags(left_stick, right_stick), expected);
            assert_eq!(serde_json::to_value(gesture).unwrap(), json);
        }
    }

    #[test]
    fn right_stick_dpad_uses_a_circular_radius_and_keyboard_survives_stick_release() {
        let bindings = [Binding {
            input: String::new(),
            at: [0.5, 0.5],
            gesture: Gesture::Dpad {
                directions: ["W".into(), "S".into(), "A".into(), "D".into()],
                radius: 0.1,
                left_stick: false,
                right_stick: true,
            },
        }];
        let mut gestures = Gestures::default();
        let mut contacts = Vec::new();
        gestures.left = vec2(-1., 0.);
        gestures.right = vec2(0.04, 0.);
        gestures.frame(&bindings, &frame(1.), &mut contacts);
        assert!(contacts.is_empty());

        gestures.right = vec2(1., -1.);
        gestures.frame(&bindings, &frame(1.1), &mut contacts);
        assert_eq!(contacts.len(), 2);
        assert_eq!(contacts[0].pos, bindings[0].at);
        let diagonal = (point(contacts[1].pos) - point(bindings[0].at)) * frame(1.1).size;
        let expected_axis = 90. / 2_f32.sqrt();
        assert!((diagonal.x - expected_axis).abs() < 0.001);
        assert!((diagonal.y + expected_axis).abs() < 0.001);
        let contact_id = contacts[1].id;
        assert_eq!(contact_id, contacts[0].id);

        contacts.clear();
        gestures.input(&bindings, "A", true, &frame(1.2), &mut contacts);
        gestures.right = Vec2::ZERO;
        contacts.clear();
        gestures.frame(&bindings, &frame(1.3), &mut contacts);
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].id, contact_id);
        assert!(contacts[0].down);
        assert!((contacts[0].pos[0] - (0.5 - 90. / 1600.)).abs() < 0.001);
        assert_eq!(contacts[0].pos[1], 0.5);

        contacts.clear();
        gestures.input(&bindings, "A", false, &frame(1.4), &mut contacts);
        gestures.frame(&bindings, &frame(1.4), &mut contacts);
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].id, contact_id);
        assert!(!contacts[0].down);

        contacts.clear();
        gestures.right = vec2(0., 0.5);
        gestures.frame(&bindings, &frame(1.5), &mut contacts);
        assert_eq!(contacts.len(), 2);
        assert!((contacts[1].pos[1] - 0.55).abs() < 0.001);
        let contact_id = contacts[1].id;

        contacts.clear();
        gestures.right = Vec2::ZERO;
        gestures.frame(&bindings, &frame(1.6), &mut contacts);
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].id, contact_id);
        assert!(!contacts[0].down);
    }

    #[test]
    fn keyboard_free_look_restarts_at_anchor_after_the_edge_delay() {
        let bindings = [Binding {
            input: String::new(),
            at: [0.98, 0.5],
            gesture: Gesture::FreeLook {
                mouse: false,
                directions: ["I".into(), "K".into(), "J".into(), "L".into()],
                speed: 3.,
                reset_ms: 80,
            },
        }];
        let mut gestures = Gestures::default();
        let mut contacts = Vec::new();
        gestures.input(&bindings, "L", true, &frame(1.), &mut contacts);
        gestures.frame(&bindings, &frame(1.), &mut contacts);
        assert_eq!(contacts.len(), 3);
        assert_eq!(contacts[0].pos, bindings[0].at);
        assert_eq!(contacts[1].pos, [1., 0.5]);
        assert!(!contacts[2].down);
        assert!(gestures.waiting());
        contacts.clear();
        gestures.frame(&bindings, &frame(1.04), &mut contacts);
        assert!(contacts.is_empty());
        gestures.frame(&bindings, &frame(1.09), &mut contacts);
        assert_eq!(contacts[0].pos, bindings[0].at);
        assert_eq!(contacts[1].pos, [1., 0.5]);
        contacts.clear();
        gestures.input(&bindings, "L", false, &frame(1.1), &mut contacts);
        gestures.input(&bindings, "I", true, &frame(1.1), &mut contacts);
        gestures.frame(&bindings, &frame(1.1), &mut contacts);
        gestures.frame(&bindings, &frame(1.2), &mut contacts);
        assert!(contacts[2].pos[1] < contacts[1].pos[1]);
        gestures.input(&bindings, "I", false, &frame(1.3), &mut contacts);
        gestures.frame(&bindings, &frame(1.3), &mut contacts);
        assert!(!contacts.last().unwrap().down);
        assert!(!gestures.waiting());
    }

    #[test]
    fn mouse_free_look_releases_aim_before_moving_its_own_anchor() {
        let bindings = [
            Binding {
                input: "V".into(),
                at: [0.25, 0.5],
                gesture: Gesture::FreeLook {
                    mouse: true,
                    directions: Default::default(),
                    speed: 1.,
                    reset_ms: 50,
                },
            },
            Binding {
                input: "Q".into(),
                at: [0.5, 0.5],
                gesture: Gesture::Aim {
                    sensitivity: 1.,
                    mouse: true,
                    right_stick: false,
                    toggle: true,
                },
            },
        ];
        let mut gestures = Gestures::default();
        let mut contacts = Vec::new();
        gestures.input(&bindings, "Q", true, &frame(1.), &mut contacts);
        gestures.input(&bindings, "Q", false, &frame(1.), &mut contacts);
        gestures.mouse = vec2(24., 0.);
        gestures.frame(&bindings, &frame(1.), &mut contacts);
        assert_eq!(contacts[0].pos, [0.5, 0.5]);
        contacts.clear();
        gestures.input(&bindings, "V", true, &frame(1.1), &mut contacts);
        assert!(gestures.aiming_mouse(&bindings));
        gestures.mouse = vec2(24., 0.);
        gestures.frame(&bindings, &frame(1.1), &mut contacts);
        assert_eq!(contacts.len(), 3);
        assert!(!contacts[0].down);
        assert_eq!(contacts[1].pos, [0.25, 0.5]);
        assert!(contacts[2].pos[0] > 0.25);
        contacts.clear();
        gestures.input(&bindings, "V", false, &frame(1.2), &mut contacts);
        gestures.mouse = vec2(24., 0.);
        gestures.frame(&bindings, &frame(1.2), &mut contacts);
        assert!(!contacts[0].down);
        assert_eq!(contacts[1].pos, [0.5, 0.5]);
        gestures.cancel_aim(&bindings, &mut contacts);
        assert!(!gestures.aiming_mouse(&bindings));
    }

    #[test]
    fn wheel_scroll_drags_in_both_directions_and_releases_between_strokes() {
        let bindings = [Binding {
            input: String::new(),
            at: [0.5, 0.5],
            gesture: Gesture::Scroll {
                amplitude: 0.20,
                speed: 1.,
                override_wheel: true,
            },
        }];
        let mut gestures = Gestures::default();
        let mut contacts = Vec::new();
        gestures.queue_scroll(vec2(0., 1.));
        gestures.queue_scroll(vec2(0., -1.));
        for time in [1., 1.1, 1.21, 1.22, 1.43] {
            gestures.frame(&bindings, &frame(time), &mut contacts);
        }
        assert_eq!(contacts.iter().filter(|contact| !contact.down).count(), 2);
        assert_eq!(contacts[0].pos, [0.5, 0.5]);
        assert!((contacts[1].pos[1] - 0.60).abs() < 0.001);
        assert!((contacts[3].pos[1] - 0.70).abs() < 0.001);
        assert_eq!(contacts[4].pos, [0.5, 0.5]);
        assert!((contacts.last().unwrap().pos[1] - 0.30).abs() < 0.001);
        assert!(!gestures.waiting());
    }

    #[test]
    fn tap_repeat_and_gesture_timeline_release_every_contact() {
        let bindings = [
            Binding {
                input: "Space".into(),
                at: [0.8, 0.8],
                gesture: Gesture::Tap { duration_ms: 50 },
            },
            Binding {
                input: "Mouse:Primary".into(),
                at: [0.7, 0.8],
                gesture: Gesture::Repeat {
                    interval_ms: 100,
                    hold_ms: 30,
                },
            },
            Binding {
                input: "Pad:South".into(),
                at: [0.5, 0.5],
                gesture: Gesture::Pinch {
                    from: 0.05,
                    to: 0.20,
                    duration_ms: 500,
                },
            },
        ];
        let mut touch = Gestures::default();
        let mut events = Vec::new();
        for binding in &bindings {
            touch.input(&bindings, &binding.input, true, &frame(1.), &mut events);
        }
        assert_eq!(
            events.iter().map(|c| c.id).collect::<Vec<_>>(),
            [2, 3, 4, 5]
        );
        assert!(events.iter().all(|c| c.down));
        touch.input(&bindings, "Space", false, &frame(1.01), &mut events);
        assert!(events.last().unwrap().down, "tap survives keyboard release");
        events.clear();
        touch.frame(&bindings, &frame(1.06), &mut events);
        assert_eq!(
            events
                .iter()
                .filter(|c| !c.down)
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(
            events
                .iter()
                .filter(|c| c.down)
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [4, 5]
        );
        events.clear();
        touch.frame(&bindings, &frame(1.12), &mut events);
        assert!(events.iter().any(|c| c.id == 2 && c.down));
        assert_eq!(touch.release().count(), 3);
        assert!(!touch.waiting());
    }

    #[test]
    fn steering_wheel_holds_one_contact_in_either_direction_and_keeps_old_rotations() {
        let old: Gesture = serde_json::from_value(serde_json::json!({
            "kind":"rotate", "radius":0.1, "degrees":90., "duration_ms":400
        }))
        .unwrap();
        assert!(matches!(old, Gesture::Rotate { wheel: None, .. }));
        let mut controls = [Binding {
            input: "D".into(),
            at: [0.5, 0.5],
            gesture: old,
        }];
        let mut state = Gestures::default();
        let mut changes = Vec::new();
        state.input(&controls, "D", true, &frame(0.), &mut changes);
        assert_eq!(state.release().count(), 2);
        let Gesture::Rotate { wheel, .. } = &mut controls[0].gesture else {
            unreachable!()
        };
        *wheel = Some(SteeringWheel {
            reverse_input: "A".into(),
            start_degrees: -90.,
        });
        for (key, expected_x) in [("D", 0.55625), ("A", 0.44375)] {
            changes.clear();
            state.input(&controls, key, true, &frame(1.), &mut changes);
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].pos, [0.5, 0.4]);
            let id = changes[0].id;
            state.frame(&controls, &frame(1.5), &mut changes);
            assert!(changes.last().unwrap().down && changes.last().unwrap().id == id);
            let end = changes.last().unwrap().pos;
            assert!((end[0] - expected_x).abs() < 0.001 && (end[1] - 0.5).abs() < 0.001);
            state.frame(&controls, &frame(3.), &mut changes);
            assert!(changes.iter().all(|c| c.down && c.id == id));
            state.input(&controls, key, false, &frame(3.1), &mut changes);
            assert!(!changes.last().unwrap().down && !state.waiting());
        }
    }

    #[test]
    fn moba_joystick_uses_hero_relative_pixel_direction_and_click_distance() {
        let bindings = [Binding {
            input: "X".into(),
            at: [0.15, 0.8],
            gesture: Gesture::MobaMove {
                hero: [0.5, 0.5],
                radius: 0.1,
                speed: 1.0,
            },
        }];
        let mut state = Gestures::default();
        let mut events = Vec::new();
        // Ninety pixels right and ninety up in a 1600 × 900 game.
        let click = Frame {
            cursor: Some([0.55625, 0.4]),
            ..frame(1.)
        };
        state.input(&bindings, "Mouse:Secondary", true, &click, &mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].pos, bindings[0].at);
        let end = events[1];
        assert!(end.down && end.id == events[0].id);
        assert!(((end.pos[0] - 0.15) * 1600. - 63.64).abs() < 0.1);
        assert!(((end.pos[1] - 0.8) * 900. + 63.64).abs() < 0.1);
        events.clear();
        state.input(&bindings, "Mouse:Secondary", false, &click, &mut events);
        state.frame(&bindings, &frame(1.1), &mut events);
        assert!(events.is_empty());
        state.frame(&bindings, &frame(1.15), &mut events);
        assert_eq!(events.len(), 1);
        assert!(!events[0].down && events[0].id == end.id);
        assert!(!state.waiting());

        events.clear();
        state.input(&bindings, "Mouse:Secondary", true, &click, &mut events);
        state.input(&bindings, "X", true, &click, &mut events);
        assert_eq!(events.len(), 3);
        assert!(!events[2].down && !state.waiting());
        assert_eq!(state.release().count(), 0);
        events.clear();
        state.input(
            &bindings,
            "Mouse:Secondary",
            true,
            &Frame {
                cursor: Some([0.5, 0.5]),
                ..click
            },
            &mut events,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn relative_aim_and_quick_cast_follow_motion_and_clear_on_cancel() {
        let bindings = [
            Binding {
                input: "Q".into(),
                at: [0.5, 0.5],
                gesture: Gesture::Aim {
                    sensitivity: 1.,
                    mouse: true,
                    right_stick: true,
                    toggle: true,
                },
            },
            Binding {
                input: "E".into(),
                at: [0.8, 0.6],
                gesture: Gesture::Moba {
                    radius: 0.15,
                    quick_cast: true,
                },
            },
        ];
        let mut touch = Gestures::default();
        let mut events = Vec::new();
        touch.input(&bindings, "Q", true, &frame(1.), &mut events);
        touch.input(&bindings, "Q", false, &frame(1.), &mut events);
        assert!(touch.aiming_mouse(&bindings));
        touch.mouse = vec2(32., -45.);
        touch.frame(&bindings, &frame(1.01), &mut events);
        assert_eq!(events.len(), 2);
        let aim = events.last().unwrap();
        assert!((aim.pos[0] - 0.52).abs() < 0.001 && (aim.pos[1] - 0.45).abs() < 0.001);
        touch.cancel_aim(&bindings, &mut events);
        assert!(!touch.aiming_mouse(&bindings));
        assert!(!events.last().unwrap().down);
        events.clear();
        touch.input(&bindings, "E", false, &frame(1.1), &mut events);
        assert!(events.is_empty(), "stray release does not cast");
        touch.input(&bindings, "E", true, &frame(1.2), &mut events);
        assert!(events.is_empty());
        touch.input(&bindings, "E", false, &frame(1.3), &mut events);
        touch.frame(&bindings, &frame(1.4), &mut events);
        assert_eq!(events.len(), 3);
        assert!(events[1].pos[0] < 0.8 && events[1].pos[1] > 0.6);
        assert!(!events[2].down);
    }
}
