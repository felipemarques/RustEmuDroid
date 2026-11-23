use super::{BACKGROUND, BORDER, INK, MUTED, PANEL};
use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2, vec2};

pub(super) const ACCENT: Color32 = Color32::from_rgb(255, 162, 116);
const FIELD: Color32 = Color32::from_rgb(34, 45, 58);
const HOVER: Color32 = Color32::from_rgb(48, 62, 77);
const SELECTED: Color32 = Color32::from_rgb(69, 49, 44);

pub(super) fn install(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BACKGROUND;
    v.window_fill = PANEL;
    v.extreme_bg_color = BACKGROUND;
    v.faint_bg_color = FIELD;
    v.window_stroke = Stroke::new(1_f32, BORDER);
    v.window_corner_radius = 14.into();
    v.menu_corner_radius = 9.into();
    v.selection.bg_fill = SELECTED;
    v.selection.stroke = Stroke::new(1.5_f32, ACCENT);
    for (widget, fill, border) in [
        (&mut v.widgets.noninteractive, PANEL, BORDER),
        (&mut v.widgets.inactive, FIELD, BORDER),
        (&mut v.widgets.hovered, HOVER, MUTED),
        (&mut v.widgets.active, SELECTED, ACCENT),
        (&mut v.widgets.open, FIELD, ACCENT),
    ] {
        widget.weak_bg_fill = fill;
        widget.bg_fill = fill;
        widget.bg_stroke = Stroke::new(1_f32, border);
        widget.fg_stroke = Stroke::new(1.5_f32, INK);
        widget.corner_radius = 7.into();
        widget.expansion = 0.;
    }
    ctx.set_visuals(v);
}

pub(super) fn form(ui: &mut egui::Ui) {
    ui.spacing_mut().button_padding = vec2(12., 7.);
    ui.spacing_mut().item_spacing = vec2(10., 6.);
    ui.spacing_mut().interact_size.y = 32.;
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.));
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.));
}

pub(super) fn label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).size(13.).color(MUTED))
}

pub(crate) fn button(label: &str) -> egui::Button<'_> {
    egui::Button::new(label).min_size(vec2(78., 33.))
}

pub(crate) fn primary(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for (state, fill, edge) in [
            (&mut w.inactive, ACCENT, ACCENT),
            (&mut w.hovered, Color32::from_rgb(255, 186, 150), INK),
            (&mut w.active, Color32::from_rgb(231, 144, 100), INK),
        ] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.bg_stroke = Stroke::new(1_f32, edge);
        }
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).strong().color(BACKGROUND))
                .min_size(vec2(86., 33.)),
        )
    })
    .inner
}

pub(super) fn select(
    ui: &mut egui::Ui,
    id: &str,
    text: &str,
    options: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let width = ui.available_width();
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .truncate()
        .selected_text(text)
        .icon(|ui, rect, visuals, open| {
            let c = rect.center();
            let dy = if open { -2. } else { 2. };
            ui.painter().add(egui::Shape::line(
                vec![c + vec2(-4., -dy), c + vec2(0., dy), c + vec2(4., -dy)],
                visuals.fg_stroke,
            ));
        })
        .show_ui(ui, |ui| {
            ui.set_min_width((width - 18.).max(60.));
            options(ui);
        })
        .response
        .on_hover_text(text)
}

pub(super) fn profile_card(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    selected: bool,
) -> egui::Response {
    let (r, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            ui.is_enabled(),
            selected,
            title,
        )
    });
    let visuals = ui.style().interact(&response);
    let border = if selected || response.has_focus() {
        Stroke::new(1.5_f32, ACCENT)
    } else {
        visuals.bg_stroke
    };
    let painter = ui.painter();
    painter.rect(
        r,
        9.,
        if selected {
            SELECTED
        } else {
            visuals.weak_bg_fill
        },
        border,
        egui::StrokeKind::Inside,
    );
    let origin = r.left_top() + vec2(12., 9.);
    painter.text(
        origin,
        egui::Align2::LEFT_TOP,
        title,
        FontId::proportional(16.),
        INK,
    );
    painter.text(
        origin + vec2(0., 23.),
        egui::Align2::LEFT_TOP,
        subtitle,
        FontId::proportional(12.),
        MUTED,
    );
    let dot = r.right_center() - vec2(16., 0.);
    painter.circle_stroke(
        dot,
        6.,
        Stroke::new(1.3_f32, if selected { ACCENT } else { MUTED }),
    );
    if selected {
        painter.circle_filled(dot, 3., ACCENT);
    }
    response
}
