use super::*;
use egui::{Modifiers, Pos2, RawInput, Shape};

const SLOT: PreviewRect = PreviewRect {
    left: 100,
    top: 50,
    right: 500,
    bottom: 250,
};
const GUEST: [u32; 2] = [200, 100];
const CENTER: Pos2 = Pos2::new(300.0, 150.0);
const TOOLBAR: Pos2 = Pos2::new(40.0, 15.0);

fn primary(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn input_frame(panel: &mut MirrorPanel, ctx: &egui::Context, focused: bool, events: Vec<Event>) {
    let _ = ctx.run(
        RawInput {
            focused,
            events,
            ..Default::default()
        },
        |ctx| panel.handle_input(ctx, SLOT, 1.0, GUEST, false),
    );
}

fn mapped_frame(panel: &mut MirrorPanel, ctx: &egui::Context, focused: bool, events: Vec<Event>) {
    let _ = ctx.run(
        RawInput {
            focused,
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(640., 360.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                panel.handle_game_input(ui, SLOT, 1.0, GUEST, false, Locale::EnUs);
            });
        },
    );
}

fn key_event(key: Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn maps_two_keys_to_separate_contacts_and_releases_on_foreground_loss() {
    let (session, input) = MirrorSession::input_recorder();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("map.json");
    std::fs::write(
        &path,
        r#"{"org.example.game":{"buttons":{"J":[0.25,0.75],"K":[1,0.5]},"stick":null}}"#,
    )
    .unwrap();
    let mut panel = MirrorPanel {
        session: Some(session),
        size: Some(GUEST),
        ..Default::default()
    };
    panel.mapped.load(Some(path));
    panel.apply_event(MirrorEvent::Foreground(4, Some("org.example.game".into())));

    let ctx = egui::Context::default();
    mapped_frame(&mut panel, &ctx, true, vec![key_event(Key::J, true)]);
    assert!(
        input.try_recv().is_err(),
        "the preview has not acquired input focus"
    );

    mapped_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    assert!(matches!(
        input.try_recv().unwrap(),
        MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST)
    ));
    mapped_frame(
        &mut panel,
        &ctx,
        true,
        vec![
            key_event(Key::J, false),
            key_event(Key::J, true),
            key_event(Key::K, true),
        ],
    );
    let first = input.try_recv().unwrap();
    let second = input.try_recv().unwrap();
    assert!(matches!((first, second), (
        MirrorInput::Contact { epoch: 4, id: first, action: TouchAction::Down, point: [50, 75], size: GUEST },
        MirrorInput::Contact { epoch: 4, id: second, action: TouchAction::Down, point: [199, 50], size: GUEST }
    ) if first > 0 && second > 0 && first != second));
    assert!(
        input.try_recv().is_err(),
        "mapped keys must not also reach Android as keyboard input"
    );

    panel.apply_event(MirrorEvent::Foreground(5, None));
    assert!(matches!(input.try_recv().unwrap(), MirrorInput::Release));
    assert_eq!(panel.controls_test_state(), (false, false, 0, None));
    mapped_frame(
        &mut panel,
        &ctx,
        true,
        vec![
            key_event(Key::J, false),
            key_event(Key::K, false),
            primary(CENTER, false),
        ],
    );
    assert!(
        input.try_recv().is_err(),
        "late ups belong to the previous input epoch"
    );
    mapped_frame(
        &mut panel,
        &ctx,
        true,
        vec![primary(CENTER, true), primary(CENTER, false)],
    );
    assert!(matches!(
        input.try_iter().collect::<Vec<_>>().as_slice(),
        [
            MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST),
            MirrorInput::Touch(TouchAction::Up, [100, 50], GUEST)
        ]
    ));
}

#[test]
fn control_profile_shortcut_changes_only_the_selected_games_saved_profile() {
    let (session, input) = MirrorSession::input_recorder();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("maps.json");
    let first = serde_json::json!({"buttons":{"J":[0.25,0.75]},"stick":null});
    let second = serde_json::json!({"buttons":{"K":[0.75,0.25]},"stick":null});
    let original = serde_json::json!({
        "org.example.game":{"active":"First","layouts":{"First":first,"Second":second}},
        "org.example.other":{"active":"Default","layouts":{"Default":first}},
    });
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let mut panel = MirrorPanel {
        session: Some(session),
        size: Some(GUEST),
        ..Default::default()
    };
    panel.mapped.load(Some(path.clone()));
    panel.apply_event(MirrorEvent::Foreground(
        1,
        Some("org.example.game".to_owned()),
    ));
    assert!(panel.mapped.map.captures(Key::J));
    let ctx = egui::Context::default();
    mapped_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    let _ = input.try_recv().unwrap();

    let modifiers = crate::toolbar::PLAYER_MODIFIERS;
    let _ = ctx.run(
        RawInput {
            focused: true,
            modifiers,
            events: vec![Event::Key {
                key: Key::P,
                physical_key: Some(Key::P),
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        },
        |ctx| {
            panel.controls_shortcuts(ctx, Locale::EnUs, false);
            assert!(ctx.input(|i| {
                i.events
                    .iter()
                    .all(|event| !matches!(event, Event::Key { key: Key::P, .. }))
            }));
        },
    );
    assert!(matches!(input.try_recv().unwrap(), MirrorInput::Release));
    assert!(!panel.mapped.map.captures(Key::J));
    assert!(panel.mapped.map.captures(Key::K));
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["org.example.game"]["active"], "Second");
    assert_eq!(saved["org.example.other"]["active"], "Default");
    assert_eq!(
        saved["org.example.other"]["layouts"]["Default"]["buttons"],
        first["buttons"]
    );
}

#[test]
fn selection_discards_old_geometry_and_queues_only_the_latest_instance() {
    let mut panel = MirrorPanel {
        player: Some((731, 1492)),
        size: Some(GUEST),
        last_touch: Some([7, 9]),
        input_focused: true,
        input_error: Some("previous transport went offline".to_owned()),
        view_error: Some("previous window closed".to_owned()),
        ..Default::default()
    };
    panel.queue("Pie64".to_owned(), Some("org.supertuxkart.stk".to_owned()));
    panel.queue("Pie64_1".to_owned(), None);

    assert!(panel.active());
    assert_eq!(panel.pending, Some(("Pie64_1".to_owned(), None)));
    assert!(panel.session.is_none());
    assert!(panel.player.is_none());
    assert!(panel.size.is_none());
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);
    assert!(panel.input_error.is_none());
    assert!(panel.view_error.is_none());

    panel.disconnect();
    assert!(!panel.active());
}

#[test]
fn clicking_outside_the_guest_releases_its_keyboard_focus() {
    let mut panel = MirrorPanel::default();
    let ctx = egui::Context::default();

    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    assert_eq!(panel.last_touch, Some([100, 50]));
    assert!(panel.input_focused);

    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, false)]);
    assert!(panel.last_touch.is_none());
    assert!(panel.input_focused);

    input_frame(&mut panel, &ctx, true, vec![primary(TOOLBAR, true)]);
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);
}

#[test]
fn dragging_outside_or_losing_native_focus_clears_a_held_touch() {
    let mut panel = MirrorPanel::default();
    let ctx = egui::Context::default();
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    input_frame(&mut panel, &ctx, true, vec![Event::PointerMoved(TOOLBAR)]);
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);

    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, false)]);
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    assert!(panel.input_focused);
    input_frame(&mut panel, &ctx, false, vec![]);
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);
}

#[test]
fn releasing_outside_without_a_pointer_move_cancels_the_held_touch() {
    let (session, input) = MirrorSession::input_recorder();
    let mut panel = MirrorPanel {
        session: Some(session),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    input_frame(&mut panel, &ctx, true, vec![primary(TOOLBAR, false)]);
    assert!(matches!(
        input.try_iter().collect::<Vec<_>>().as_slice(),
        [
            MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST),
            MirrorInput::Release
        ]
    ));
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);

    input_frame(
        &mut panel,
        &ctx,
        true,
        vec![primary(CENTER, true), primary(CENTER, false)],
    );
    assert!(matches!(
        input.try_iter().collect::<Vec<_>>().as_slice(),
        [
            MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST),
            MirrorInput::Touch(TouchAction::Up, [100, 50], GUEST)
        ]
    ));
}

#[test]
fn back_releases_the_primary_contact_before_navigation_and_ignores_its_old_up() {
    for back in [
        Event::PointerButton {
            pos: CENTER,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::Key {
            key: Key::Escape,
            physical_key: Some(Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        },
    ] {
        let (session, input) = MirrorSession::input_recorder();
        let mut panel = MirrorPanel {
            session: Some(session),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
        assert!(matches!(
            input.try_recv().unwrap(),
            MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST)
        ));

        input_frame(&mut panel, &ctx, true, vec![back]);
        input_frame(&mut panel, &ctx, true, vec![primary(CENTER, false)]);
        assert!(matches!(
            input.try_iter().collect::<Vec<_>>().as_slice(),
            [MirrorInput::Release, MirrorInput::Back]
        ));
        assert!(panel.last_touch.is_none());
    }
}

#[test]
fn changed_preview_geometry_discards_unfinished_contact_before_reprojecting_input() {
    let (session, input) = MirrorSession::input_recorder();
    let mut panel = MirrorPanel {
        session: Some(session),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    assert!(matches!(
        input.try_recv().unwrap(),
        MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST)
    ));

    let shifted = PreviewRect {
        left: 150,
        right: 550,
        ..SLOT
    };
    let _ = ctx.run(
        RawInput {
            focused: true,
            events: vec![Event::PointerMoved(CENTER), primary(CENTER, false)],
            ..Default::default()
        },
        |ctx| panel.handle_input(ctx, shifted, 1.0, GUEST, false),
    );
    assert!(matches!(
        input.try_iter().collect::<Vec<_>>().as_slice(),
        [MirrorInput::Release]
    ));
    assert!(panel.last_touch.is_none());
}

#[test]
fn native_resize_gesture_keeps_its_pointer_press_out_of_the_guest() {
    let (session, input) = MirrorSession::input_recorder();
    let mut panel = MirrorPanel {
        session: Some(session),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let _ = ctx.run(
        RawInput {
            focused: true,
            events: vec![primary(CENTER, true)],
            ..Default::default()
        },
        |ctx| panel.handle_input(ctx, SLOT, 1.0, GUEST, true),
    );
    assert!(input.try_recv().is_err());
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);
}

fn rotate_widget_frame(
    panel: &mut MirrorPanel,
    ctx: &egui::Context,
    focused: bool,
    events: Vec<Event>,
) -> egui::Response {
    let mut response = None;
    let _ = ctx.run(
        RawInput {
            focused,
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                response = Some(panel.rotate_button(ui, Locale::EnUs));
            });
        },
    );
    response.unwrap()
}

#[test]
fn clicking_rotate_releases_held_input_and_waits_for_a_rotation_result() {
    let (session, input) = MirrorSession::input_recorder();
    let mut panel = MirrorPanel {
        session: Some(session),
        size: Some(GUEST),
        layout: Some((SLOT, SLOT, Instant::now())),
        last_touch: Some([100, 50]),
        input_focused: true,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let center = rotate_widget_frame(&mut panel, &ctx, true, vec![])
        .rect
        .center();
    rotate_widget_frame(
        &mut panel,
        &ctx,
        true,
        vec![Event::PointerMoved(center), primary(center, true)],
    );
    rotate_widget_frame(&mut panel, &ctx, true, vec![primary(center, false)]);
    assert!(matches!(
        input.try_iter().collect::<Vec<_>>().as_slice(),
        [MirrorInput::Release, MirrorInput::Rotate]
    ));
    assert!(panel.rotation_pending);
    assert!(!panel.input_focused);
    assert_eq!(panel.size, Some(GUEST));
    assert!(!rotate_widget_frame(&mut panel, &ctx, true, vec![]).enabled());

    panel.apply_event(MirrorEvent::Ready(GUEST));
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, false)]);
    assert!(input.try_recv().is_err());
    assert!(panel.rotation_pending);

    // A game can keep its requested orientation. The acknowledged size still unlocks input.
    panel.apply_event(MirrorEvent::RotationChecked(GUEST));
    assert!(!panel.rotation_pending);
    assert!(panel.layout.is_none());
    assert!(rotate_widget_frame(&mut panel, &ctx, true, vec![]).enabled());
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, true)]);
    assert!(matches!(
        input.try_recv().unwrap(),
        MirrorInput::Touch(TouchAction::Down, [100, 50], GUEST)
    ));
}

#[test]
fn rotation_requires_focus_and_adopts_only_the_verified_android_dimensions() {
    let (session, input) = MirrorSession::input_recorder();
    let mut panel = MirrorPanel {
        session: Some(session),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    assert!(!rotate_widget_frame(&mut panel, &ctx, true, vec![]).enabled());
    panel.apply_event(MirrorEvent::Ready(GUEST));
    assert!(!rotate_widget_frame(&mut panel, &ctx, false, vec![]).enabled());
    panel.request_rotation(false);
    assert!(input.try_recv().is_err());

    panel.request_rotation(true);
    assert!(matches!(input.try_recv().unwrap(), MirrorInput::Rotate));
    assert_eq!(panel.size, Some(GUEST));
    panel.apply_event(MirrorEvent::RotationChecked([100, 200]));
    assert_eq!(panel.size, Some([100, 200]));
    assert!(!panel.rotation_pending);
}

#[test]
fn rotation_cannot_stay_pending_when_the_worker_receiver_has_closed() {
    let (session, input) = MirrorSession::input_recorder();
    drop(input);
    let mut panel = MirrorPanel {
        session: Some(session),
        size: Some(GUEST),
        ..Default::default()
    };
    panel.request_rotation(true);
    assert!(!panel.rotation_pending);
    assert_eq!(panel.size, Some(GUEST));
}

#[test]
fn obscured_preview_releases_input_without_discarding_the_connected_display_size() {
    let (session, input) = MirrorSession::input_recorder();
    let mut panel = MirrorPanel {
        session: Some(session),
        size: Some(GUEST),
        last_touch: Some([100, 50]),
        input_focused: true,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let _ = ctx.run(RawInput::default(), |ctx| {
        panel.show(ctx, Locale::EnUs, true, true, false)
    });

    assert_eq!(panel.size, Some(GUEST));
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);
    assert!(matches!(
        input.try_iter().collect::<Vec<_>>().as_slice(),
        [MirrorInput::Release]
    ));

    // The up event belongs to the contact the modal cancelled, even when the preview is visible again.
    input_frame(&mut panel, &ctx, true, vec![primary(CENTER, false)]);
    assert!(input.try_recv().is_err());
}

#[test]
fn view_only_does_not_lose_the_player_while_waiting_for_a_preview() {
    let mut panel = MirrorPanel {
        instance: "Pie64".to_owned(),
        player: Some((731, 1492)),
        input_error: Some("ADB access is disabled".to_owned()),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let out = ctx.run(RawInput::default(), |ctx| {
        panel.show(ctx, Locale::EnUs, false, true, false)
    });

    assert_eq!(panel.player, Some((731, 1492)));
    assert!(panel.size.is_none());
    assert!(!panel.input_focused);
    assert!(out.shapes.iter().any(|shape| {
        matches!(&shape.shape, Shape::Text(text) if text.galley.text() == "View only")
    }));
}

#[test]
fn translates_guest_navigation_letters_and_digits_without_global_shortcuts() {
    for (key, code) in [
        (Key::Escape, 4),
        (Key::ArrowLeft, 21),
        (Key::Enter, 66),
        (Key::A, 29),
        (Key::Z, 54),
        (Key::Num0, 7),
        (Key::Num9, 16),
    ] {
        assert_eq!(keycode(key), Some(code));
    }
    assert_eq!(keycode(Key::F11), None);
}
