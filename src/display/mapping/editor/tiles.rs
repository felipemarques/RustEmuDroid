use crate::ui::theme;
use eframe::egui::{self, Color32, FontId, Painter, Pos2, Shape, Stroke, StrokeKind, vec2};

pub(super) fn tile(
    ui: &mut egui::Ui,
    width: f32,
    icon: usize,
    name: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 66.), egui::Sense::click_and_drag());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, name)
    });
    if ui.is_rect_visible(rect) {
        let highlighted = response.hovered() || response.has_focus();
        let (fill, border) = if selected {
            (Color32::from_rgb(66, 47, 42), theme::ACCENT)
        } else if highlighted {
            (
                Color32::from_rgb(47, 60, 75),
                Color32::from_rgb(147, 163, 182),
            )
        } else {
            (Color32::from_rgb(29, 39, 51), Color32::from_rgb(57, 70, 86))
        };
        let icon_color = if selected || highlighted {
            theme::ACCENT
        } else {
            Color32::from_rgb(208, 220, 234)
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            8,
            fill,
            Stroke::new(1_f32, border),
            StrokeKind::Inside,
        );
        draw(
            painter,
            egui::pos2(rect.center().x, rect.top() + 21.),
            icon,
            icon_color,
        );
        let mut layout = egui::text::LayoutJob::simple(
            name.to_owned(),
            FontId::proportional(11.),
            ui.visuals().text_color(),
            rect.width() - 6.,
        );
        layout.halign = egui::Align::Center;
        let galley = painter.layout_job(layout);
        let offset = vec2(rect.width() * 0.5, 35. + (26. - galley.size().y) * 0.5);
        painter.galley(rect.min + offset, galley, ui.visuals().text_color());
    }
    response
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text(name)
}

pub(super) fn draw(painter: &Painter, center: Pos2, icon: usize, color: Color32) {
    let stroke = Stroke::new(1.5_f32, color);
    let point = |x, y| center + vec2(x, y);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    };
    let ring = |x, y, radius| {
        painter.circle_stroke(point(x, y), radius, stroke);
    };
    let dot = |x, y| {
        painter.circle_filled(point(x, y), 2., color);
    };
    match icon {
        0 => {
            ring(0., 0., 8.);
            ring(0., 0., 4.);
            dot(0., 0.);
        }
        1 => {
            ring(0., 0., 6.);
            dot(0., 0.);
            line(&[(-10., 0.), (-8., 0.)]);
            line(&[(8., 0.), (10., 0.)]);
            line(&[(0., -10.), (0., -8.)]);
            line(&[(0., 8.), (0., 10.)]);
        }
        2 => {
            ring(-5., 2., 4.);
            ring(5., -2., 4.);
            line(&[(-8., -5.), (-5., -7.), (-1., -7.)]);
            line(&[(8., 5.), (5., 7.), (1., 7.)]);
        }
        3 => {
            ring(-6., 6., 2.5);
            line(&[(-4., 4.), (7., -7.)]);
            line(&[(1., -7.), (7., -7.), (7., -1.)]);
        }
        4 => {
            painter.add(Shape::closed_line(
                [
                    (-3., -9.),
                    (3., -9.),
                    (3., -3.),
                    (9., -3.),
                    (9., 3.),
                    (3., 3.),
                    (3., 9.),
                    (-3., 9.),
                    (-3., 3.),
                    (-9., 3.),
                    (-9., -3.),
                    (-3., -3.),
                ]
                .map(|(x, y)| point(x, y))
                .into(),
                stroke,
            ));
        }
        5 => {
            ring(0., 0., 7.);
            dot(0., 0.);
            line(&[(-10., 0.), (-5., 0.)]);
            line(&[(5., 0.), (10., 0.)]);
            line(&[(0., -10.), (0., -5.)]);
            line(&[(0., 5.), (0., 10.)]);
        }
        6 => {
            line(&[(-7., 7.), (-1., -8.), (2., -2.), (8., 1.), (-7., 7.)]);
            line(&[(-7., 1.), (-9., 3.)]);
            line(&[(-1., 7.), (-3., 9.)]);
        }
        7 => {
            line(&[(-2., 2.), (-8., 8.), (-8., 3.)]);
            line(&[(-8., 8.), (-3., 8.)]);
            line(&[(2., -2.), (8., -8.), (8., -3.)]);
            line(&[(8., -8.), (3., -8.)]);
        }
        8 => {
            painter.add(Shape::line(
                (0..=20)
                    .map(|step| {
                        let angle = step as f32 * 0.25;
                        point(angle.cos() * 7.5, angle.sin() * 7.5)
                    })
                    .collect(),
                stroke,
            ));
            line(&[(-2., -8.), (3., -7.), (1., -3.)]);
        }
        9 => {
            let body = egui::Rect::from_center_size(center, vec2(12., 19.));
            painter.rect_stroke(body, 5, stroke, StrokeKind::Middle);
            line(&[(0., -6.), (0., -2.)]);
        }
        _ => {}
    }
}

pub(super) fn dock_icon(painter: &Painter, center: Pos2, left: bool, color: Color32) {
    let frame = egui::Rect::from_center_size(center, vec2(17., 13.));
    let stroke = Stroke::new(1.4_f32, color);
    painter.rect_stroke(frame, 2, stroke, StrokeKind::Middle);
    let x = center.x + if left { -3. } else { 3. };
    painter.vline(x, frame.y_range(), stroke);
}
