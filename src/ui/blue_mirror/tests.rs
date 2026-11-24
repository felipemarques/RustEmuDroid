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
        |ctx| panel.handle_input(ctx, SLOT, 1.0, GUEST),
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
fn obscured_preview_releases_input_without_discarding_the_connected_display_size() {
    let mut panel = MirrorPanel {
        size: Some(GUEST),
        last_touch: Some([100, 50]),
        input_focused: true,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let _ = ctx.run(RawInput::default(), |ctx| {
        panel.show(ctx, Locale::EnUs, true, true)
    });

    assert_eq!(panel.size, Some(GUEST));
    assert!(panel.last_touch.is_none());
    assert!(!panel.input_focused);
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
        panel.show(ctx, Locale::EnUs, false, true)
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
