use super::*;
use crate::toolbar::{PLAYER_MODIFIERS, PROFILE_KEY};
use egui::{Event, Modifiers};

fn key(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn frame(
    ctx: &egui::Context,
    display: &mut Display,
    events: Vec<Event>,
    modifiers: Modifiers,
    interactive: bool,
) -> egui::FullOutput {
    let mut raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 600.))),
        modifiers,
        events,
        ..Default::default()
    };
    raw.viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .fullscreen = Some(true);
    ctx.run(raw, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| display.show(ui, interactive, Locale::EnUs));
    })
}

fn shows(output: &egui::FullOutput, label: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|entry| matches!(&entry.shape, egui::Shape::Text(text) if text.galley.text() == label))
}

#[test]
fn fullscreen_profile_chord_releases_game_input_cycles_and_respects_overlays() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("touch-maps.json");
    let original = serde_json::json!({
        "race": { "active": "Alpha", "layouts": {
            "Alpha": {"stick":[0.3,0.7], "buttons":{"Space":[0.8,0.4]},
                "controls":[{"input":"Q", "at":[0.5,0.5], "gesture":{"kind":"tilt","axis":"x","degrees":25.0}}]},
            "Beta": {"stick":null, "buttons":{"P":[0.9,0.4], "Control":[0.4,0.3], "Shift":[0.5,0.2]}},
            "Zulu": {"stick":null, "buttons":{}}
        }},
        "other": { "active":"Solo", "layouts": {"Solo":{"stick":null, "buttons":{}}}}
    });
    crate::ui::write_json(&path, &original).unwrap();
    let (send, mut inputs) = mpsc::unbounded_channel();
    let mut display = Display::new(Default::default(), send);
    display.mapping.load(path.clone());
    display.set_package(Some("race"));
    display.state.lock().unwrap().latest = Some(Frame {
        format: Some(Format {
            width: 160,
            height: 90,
            ..Default::default()
        }),
        pixels: vec![255; 160 * 90 * 4].into(),
    });
    let ctx = egui::Context::default();
    let plain = Modifiers::NONE;
    let chord = PLAYER_MODIFIERS;
    let cycle = || key(PROFILE_KEY, true, chord);
    let up = || key(PROFILE_KEY, false, plain);
    let active = || {
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["other"]["active"], "Solo");
        saved["race"]["active"].as_str().unwrap().to_owned()
    };
    frame(&ctx, &mut display, vec![], plain, true);
    frame(
        &ctx,
        &mut display,
        [Key::W, Key::Space, Key::Q, Key::X]
            .map(|k| key(k, true, plain))
            .to_vec(),
        plain,
        true,
    );
    assert_eq!(display.mapping.tilt_offset(), [25., 0., 0.]);
    assert!(display.held.iter().any(|k| k == "x"));
    while inputs.try_recv().is_ok() {}
    let output = frame(&ctx, &mut display, vec![cycle()], chord, true);
    assert!(shows(&output, "Active profile · Beta"));
    assert_eq!(active(), "Beta");
    assert_eq!(display.mapping.tilt_offset(), [0.; 3]);
    let mut released_touches = vec![];
    let mut released_keys = vec![];
    while let Ok(event) = inputs.try_recv() {
        match event {
            Input::Touch(touch) => {
                for point in touch.touches {
                    assert_eq!(point.pressure, 0);
                    released_touches.push(point.identifier);
                }
            }
            Input::Key(key) => {
                assert_eq!(key.event, 1);
                released_keys.push(key.key);
            }
        }
    }
    released_touches.sort();
    assert_eq!(released_touches, [1, 9]);
    assert_eq!(released_keys, ["x"]);
    frame(&ctx, &mut display, vec![cycle()], chord, true); // OS repeat
    frame(&ctx, &mut display, vec![up()], chord, true); // P is up before Ctrl/Shift
    assert!(inputs.try_recv().is_err() && active() == "Beta");
    let plain_p = key(PROFILE_KEY, true, plain);
    frame(&ctx, &mut display, vec![plain_p, up()], plain, true);
    assert!(matches!(inputs.try_recv(), Ok(Input::Touch(t)) if t.touches[0].pressure > 0));
    for name in ["Zulu", "Alpha"] {
        frame(&ctx, &mut display, vec![cycle(), up()], plain, true);
        assert_eq!(active(), name);
    }
    frame(&ctx, &mut display, vec![cycle(), up()], plain, false);
    display.toggle_mapping();
    frame(&ctx, &mut display, vec![cycle(), up()], plain, true);
    assert_eq!(active(), "Alpha");
    display.toggle_mapping();
    display.set_package(Some("other"));
    let output = frame(&ctx, &mut display, vec![cycle(), up()], plain, true);
    assert!(shows(&output, "Solo · Only profile for this game"));
    // Removing the backing file makes any attempted rewrite observable on the next chord.
    fs::remove_file(&path).unwrap();
    frame(&ctx, &mut display, vec![cycle(), up()], plain, true);
    assert!(!path.exists());
}
