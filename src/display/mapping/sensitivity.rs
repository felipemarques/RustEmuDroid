use super::{GameProfiles, Mapping};
use crate::ui::i18n::Locale;
use eframe::egui::{self, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f32; 2]", into = "[f32; 2]")]
pub(super) struct MouseSensitivity([f32; 2]);

impl Default for MouseSensitivity {
    fn default() -> Self {
        Self([1., 1.])
    }
}

impl MouseSensitivity {
    const RANGE: std::ops::RangeInclusive<f32> = 0.1..=5.;

    pub(super) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub(super) fn scale(self, delta: Vec2) -> Vec2 {
        delta * Vec2::from(self.0)
    }
}

impl TryFrom<[f32; 2]> for MouseSensitivity {
    type Error = &'static str;

    fn try_from(axes: [f32; 2]) -> Result<Self, Self::Error> {
        if axes.iter().all(|v| Self::RANGE.contains(v)) {
            Ok(Self(axes))
        } else {
            Err("mouse sensitivity must be between 0.10 and 5.00 on both axes")
        }
    }
}

impl From<MouseSensitivity> for [f32; 2] {
    fn from(value: MouseSensitivity) -> Self {
        value.0
    }
}

impl Mapping {
    pub(super) fn mouse_sensitivity_ui(&mut self, ui: &mut egui::Ui, l: Locale) {
        ui.label(l.pick(
            "Sensibilidade do mouse",
            "Mouse sensitivity",
            "Sensibilidad del ratón",
        ));
        let mut commit = false;
        ui.add_enabled_ui(self.package.is_some() && !self.load_failed, |ui| {
            ui.horizontal(|ui| {
                for (label, value) in ["X", "Y"]
                    .into_iter()
                    .zip(&mut self.draft.mouse_sensitivity.0)
                {
                    let label = ui.label(label);
                    let response = ui
                        .add(
                            egui::DragValue::new(value)
                                .range(MouseSensitivity::RANGE)
                                .speed(0.02)
                                .fixed_decimals(2)
                                .update_while_editing(false),
                        )
                        .labelled_by(label.id);
                    commit |=
                        response.drag_stopped() || (response.changed() && !response.dragged());
                }
            });
        });
        if commit {
            self.save_mouse_sensitivity();
        }
    }

    fn save_mouse_sensitivity(&mut self) {
        let Some(package) = &self.package else { return };
        let previous = self
            .profiles
            .get(package)
            .and_then(GameProfiles::layout)
            .map(|layout| layout.mouse_sensitivity)
            .unwrap_or_default();
        if self.load_failed || self.draft.mouse_sensitivity == previous {
            return;
        }
        // Persist only the quick-menu adjustment. Other games and schemes share this file.
        let mut next = self.profiles.clone();
        let game = next.entry(package.clone()).or_default();
        let Some(layout) = game.layouts.get_mut(&game.active) else {
            return;
        };
        layout.mouse_sensitivity = self.draft.mouse_sensitivity;
        match crate::ui::write_json(&self.path, &next) {
            Ok(()) => {
                self.profiles = next;
                self.error = None;
            }
            Err(error) => {
                self.draft.mouse_sensitivity = previous;
                self.error = Some(error.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_scheme_keeps_unit_gain_and_new_axes_round_trip_per_scheme() {
        let temp = tempfile::tempdir().unwrap();
        let maps = temp.path().join("touch-maps.json");
        std::fs::write(&maps, r#"{"demo":{"active":"A","layouts":{"A":{"buttons":{"Q":[0.3,0.7]},"stick":null},"B":{"buttons":{},"stick":null}}}}"#).unwrap();
        let mut map = Mapping::default();
        map.load(maps.clone());
        map.select(Some("demo"));
        assert!(map.draft.mouse_sensitivity.is_default());
        map.draft.mouse_sensitivity = MouseSensitivity([2., 0.5]);
        map.save_mouse_sensitivity();
        let mut restored = Mapping::default();
        restored.load(maps);
        let game = &restored.profiles["demo"];
        assert_eq!(
            game.layouts["A"]
                .mouse_sensitivity
                .scale(Vec2::new(4., -8.)),
            Vec2::new(8., -4.)
        );
        assert!(game.layouts["A"].buttons.contains_key("Q"));
        assert!(game.layouts["B"].mouse_sensitivity.is_default());
        assert!(serde_json::from_str::<MouseSensitivity>("[1.0,-1.0]").is_err());
    }
}
