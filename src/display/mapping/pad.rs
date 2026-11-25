use eframe::egui::Vec2;
use gilrs::{Axis, Button};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(super) const STICK_DEADZONE: f32 = 0.16;

#[derive(Default)]
pub(super) struct Pads {
    buttons: HashMap<Button, HashSet<usize>>,
    left: Stick,
    right: Stick,
}

#[derive(Default)]
struct Stick {
    owner: Option<usize>,
    positions: BTreeMap<usize, Vec2>,
}

impl Stick {
    fn update(&mut self, pad: usize, horizontal: bool, value: f32) {
        let position = self.positions.entry(pad).or_insert(Vec2::ZERO);
        if horizontal {
            position.x = value;
        } else {
            position.y = -value;
        }
        if *position == Vec2::ZERO {
            self.positions.remove(&pad);
        }
        self.pick_owner();
    }

    fn pick_owner(&mut self) {
        let engaged = |v: Vec2| v.length_sq() >= STICK_DEADZONE * STICK_DEADZONE;
        if self
            .owner
            .and_then(|owner| self.positions.get(&owner))
            .is_none_or(|&position| !engaged(position))
        {
            self.owner = self
                .positions
                .iter()
                .find_map(|(&id, &position)| engaged(position).then_some(id));
        }
    }

    fn position(&self) -> Vec2 {
        self.owner
            .and_then(|owner| self.positions.get(&owner).copied())
            .unwrap_or(Vec2::ZERO)
    }
}

impl Pads {
    pub(super) fn button(&mut self, pad: usize, button: Button, pressed: bool) -> bool {
        if pressed {
            let sources = self.buttons.entry(button).or_default();
            return sources.insert(pad) && sources.len() == 1;
        }
        if let Some(sources) = self.buttons.get_mut(&button)
            && sources.remove(&pad)
            && sources.is_empty()
        {
            self.buttons.remove(&button);
            return true;
        }
        false
    }

    pub(super) fn axis(&mut self, pad: usize, axis: Axis, value: f32) {
        match axis {
            Axis::LeftStickX => self.left.update(pad, true, value),
            Axis::LeftStickY => self.left.update(pad, false, value),
            Axis::RightStickX => self.right.update(pad, true, value),
            Axis::RightStickY => self.right.update(pad, false, value),
            _ => {}
        }
    }

    pub(super) fn sticks(&self) -> [Vec2; 2] {
        [self.left.position(), self.right.position()]
    }

    pub(super) fn disconnect(&mut self, pad: usize) -> Vec<Button> {
        for stick in [&mut self.left, &mut self.right] {
            stick.positions.remove(&pad);
            stick.pick_owner();
        }
        let mut released = Vec::new();
        self.buttons.retain(|&button, sources| {
            let last = sources.remove(&pad) && sources.is_empty();
            if last {
                released.push(button);
            }
            !last
        });
        released.sort_unstable_by_key(|&button| button as u8);
        released
    }
}
