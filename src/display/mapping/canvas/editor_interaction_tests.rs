use super::{super::Layout, *};
use crate::{
    display::{Display, Format, Frame},
    locale::Locale,
};
use egui::{Event, Modifiers, PointerButton};

const PACKAGE: &str = "test.game";
const CENTER: Point = [0.34, 0.48];

struct EditorWindow {
    ctx: egui::Context,
    display: Display,
    size: Vec2,
}

impl EditorWindow {
    fn open(path: std::path::PathBuf) -> Self {
        Self::open_with_size(path, vec2(1080., 680.))
    }

    fn open_with_size(path: std::path::PathBuf, size: Vec2) -> Self {
        let (input, _) = tokio::sync::mpsc::unbounded_channel();
        let mut display = Display::new(Default::default(), input);
        display.state.lock().unwrap().latest = Some(Frame {
            format: Some(Format {
                width: 160,
                height: 90,
                format: 1,
                ..Default::default()
            }),
            pixels: vec![255; 160 * 90 * 4].into(),
        });
        display.mapping.load(path);
        display.set_package(Some(PACKAGE));
        let mut window = Self {
            ctx: egui::Context::default(),
            display,
            size,
        };
        window.frame(vec![]);
        window
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.ctx.run(
            egui::RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                ..Default::default()
            },
            |ctx| {
                egui::SidePanel::right("sidebar")
                    .exact_width(46.)
                    .resizable(false)
                    .show(ctx, |_| {});
                egui::CentralPanel::default()
                    .show(ctx, |ui| self.display.show(ui, true, Locale::EnUs));
            },
        )
    }

    fn point_at(&mut self, pos: Pos2, pressed: bool) {
        self.frame(vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }

    fn click(&mut self, pos: Pos2) {
        self.point_at(pos, true);
        self.point_at(pos, false);
    }

    fn click_label(&mut self, label: &str) {
        self.frame(vec![]);
        let output = self.frame(vec![]);
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing editor label {label:?}"));
        self.click(position);
    }

    fn open_editor(&mut self) {
        self.display.toggle_controls_menu(145.);
        self.click_label("Controls editor");
        assert!(self.display.mapping.editing);
    }

    fn canvas(&mut self) -> Rect {
        let texture = self.display.texture.as_ref().unwrap().id();
        self.frame(vec![])
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh.calc_bounds()),
                _ => None,
            })
            .expect("displayed guest canvas")
    }

    fn select_dpad(&mut self, canvas: Rect) {
        self.click(screen(canvas, CENTER));
        self.frame(vec![]);
        assert_eq!(self.display.mapping.selected_control, Some(0));
    }

    fn save(&mut self) {
        self.click_label("Save");
        assert!(!self.display.mapping.editing);
    }
}

#[test]
fn compact_editor_docks_keep_the_guest_size_and_every_point_reachable() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("touch-maps.json");
    let mut window = EditorWindow::open_with_size(path, vec2(610., 510.));
    let image = window.canvas();

    window.display.toggle_mapping();
    assert_eq!(window.canvas(), image);
    let mapping = &window.display.mapping;
    let right = mapping.blocked_rect.unwrap();
    assert!(mapping.canvas_contains(image, image.center()));
    assert!(mapping.canvas_contains(image, image.left_center()));

    window.display.mapping.quick.editor_left = true;
    assert_eq!(window.canvas(), image);
    let mapping = &window.display.mapping;
    let left = mapping.blocked_rect.unwrap();
    assert!(mapping.canvas_contains(image, image.center()));
    assert!(mapping.canvas_contains(image, image.right_center()));
    assert!(
        !left.intersects(right),
        "a point covered by both docks is unreachable"
    );
}

fn layout(radius: f32) -> serde_json::Value {
    serde_json::json!({
        "buttons": {}, "stick": null, "controls": [{"input": "", "at": CENTER,
            "gesture": {"kind": "dpad", "directions": ["W", "S", "A", "D"],
                "radius": radius, "left_stick": true}
        }]
    })
}

fn assert_radius(layout: &Layout, expected: f32) {
    let Gesture::Dpad { radius, .. } = layout.controls[0].gesture else {
        panic!("expected a D-pad in the saved profile");
    };
    assert!(
        (radius - expected).abs() < 0.0001,
        "expected radius {expected}, got {radius}"
    );
}

fn drag_radius(expected: f32) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("touch-maps.json");
    let initial = 0.12;
    crate::ui::write_json(&path, &serde_json::json!({ PACKAGE: layout(initial) })).unwrap();
    let mut window = EditorWindow::open(path.clone());
    window.open_editor();
    let canvas = window.canvas();
    window.select_dpad(canvas);

    let handle = Dpad::new(screen(canvas, CENTER), initial, canvas).handle();
    let delta = vec2(0.7071, 0.7071) * ((expected - initial) * canvas.size().min_elem());
    let target = handle + delta;
    window.point_at(handle, true);
    window.frame(vec![Event::PointerMoved(target)]);
    assert!(
        window.display.mapping.canvas_drag.is_some(),
        "the handle did not begin dragging"
    );
    window.point_at(target, false);
    window.save();

    let mut reopened = EditorWindow::open(path);
    assert_radius(reopened.display.mapping.layout().unwrap(), expected);
    reopened.open_editor();
    let canvas = reopened.canvas();
    reopened.select_dpad(canvas);
    assert_radius(&reopened.display.mapping.draft, expected);
}

#[test]
fn dpad_radius_editor_saves_large_drag() {
    drag_radius(0.30);
}

#[test]
fn dpad_radius_editor_saves_small_drag() {
    drag_radius(0.025);
}

#[test]
fn dpad_radius_editor_preserves_imported_value_on_selection() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("touch-maps.json");
    let imported = folder.path().join("imported.json");
    let expected = 0.39;
    crate::ui::write_json(
        &imported,
        &serde_json::json!({
            "version": 1, "name": "Imported", "layout": layout(expected)
        }),
    )
    .unwrap();
    let mut window = EditorWindow::open(path.clone());
    window.open_editor();
    window.display.mapping.import_profile(&imported).unwrap();
    let canvas = window.canvas();
    window.select_dpad(canvas);
    window.save();
    let reopened = EditorWindow::open(path);
    assert_radius(reopened.display.mapping.layout().unwrap(), expected);
}

#[test]
fn quick_menu_can_duplicate_edit_and_restore_a_game_profile() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("touch-maps.json");
    let other = "test.other.game";
    crate::ui::write_json(
        &path,
        &serde_json::json!({ PACKAGE: layout(0.12), other: layout(0.21) }),
    )
    .unwrap();
    let mut window = EditorWindow::open_with_size(path.clone(), vec2(610., 510.));
    let canvas = window.canvas();
    window.display.toggle_controls_menu(145.);
    window.click_label("Manage profiles");
    window.click_label("Duplicate");
    window.frame(vec![]);
    window.frame(vec![Event::Text("Driving".into())]);
    window.click_label("Apply");
    window.click_label("Default");
    assert_eq!(window.display.mapping.draft_profiles.active, "Default");
    window.click_label("Driving");
    assert_eq!(window.display.mapping.draft_profiles.active, "Driving");
    window.click_label("Edit controls");
    assert_eq!(window.canvas(), canvas);

    let destination = [0.47, 0.67];
    let start = screen(canvas, CENTER);
    let end = screen(canvas, destination);
    window.point_at(start, true);
    window.frame(vec![Event::PointerMoved(end)]);
    window.point_at(end, false);
    window.save();

    let mut reopened = EditorWindow::open(path.clone());
    let active = &reopened.display.mapping.profiles[PACKAGE];
    assert_eq!(active.active, "Driving");
    let moved = active.layout().unwrap().controls[0].at;
    assert!((moved[0] - destination[0]).abs() < 0.002, "{moved:?}");
    assert!((moved[1] - destination[1]).abs() < 0.002, "{moved:?}");
    assert_eq!(active.layouts["Default"].controls[0].at, CENTER);
    reopened.display.toggle_controls_menu(145.);
    reopened.click_label("Driving");
    reopened.click_label("Default");

    let mut restored = EditorWindow::open(path);
    assert_eq!(restored.display.mapping.profiles[PACKAGE].active, "Default");
    assert_eq!(
        restored.display.mapping.layout().unwrap().controls[0].at,
        CENTER
    );
    restored.display.set_package(Some(other));
    let unaffected = &restored.display.mapping.profiles[other];
    assert_eq!(unaffected.layouts.len(), 1);
    assert_radius(unaffected.layout().unwrap(), 0.21);
}
