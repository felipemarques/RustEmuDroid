use super::{BORDER, ERROR, INK, Launcher, MINT, MUTED, Tool};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, ViewportCommand, vec2,
};

const HEIGHT: f32 = 40.;
const BAR: Color32 = Color32::from_rgb(30, 34, 51);
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

impl Launcher {
    pub(super) fn title_bar(&mut self, ctx: &egui::Context, fullscreen: bool, resizing: bool) {
        let locale = self.config.locale;
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        egui::TopBottomPanel::top("title-bar")
            .exact_height(HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(BAR)
                    .inner_margin(egui::Margin::symmetric(5, 0)),
            )
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let caption = ui.interact(rect, ui.id().with("caption"), Sense::click_and_drag());
                if !fullscreen && !resizing {
                    if caption.double_clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                    } else if caption.drag_started_by(egui::PointerButton::Primary) {
                        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                    }
                }
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.;
                    let (brand, _) = ui.allocate_exact_size(vec2(139., 32.), Sense::hover());
                    let p = ui.painter();
                    let icon =
                        Rect::from_center_size(brand.left_center() + vec2(18., 0.), vec2(27., 27.));
                    p.image(
                        self.brand.id(),
                        icon,
                        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        Color32::WHITE,
                    );
                    let name = brand.left_center() + vec2(38., -6.);
                    p.text(
                        name,
                        Align2::LEFT_CENTER,
                        "RustEmuDroid",
                        FontId::proportional(14.),
                        INK,
                    );
                    p.text(
                        name + vec2(0., 14.),
                        Align2::LEFT_CENTER,
                        VERSION,
                        FontId::proportional(10.),
                        MUTED,
                    );

                    if self.playing {
                        let ready = self.home_ready && !self.stopping();
                        ui.add_enabled_ui(ready, |ui| {
                            if Tool::Library.button(ui, locale)
                                && let Some(c) = &mut self.controls
                            {
                                c.send(crate::controls::Action::Applications, ctx);
                            }
                            for (tool, key) in [
                                (Tool::Back, "GoBack"),
                                (Tool::Home, "GoHome"),
                                (Tool::Recents, "AppSwitch"),
                            ] {
                                if tool.button(ui, locale)
                                    && let Some(display) = &self.display
                                {
                                    display.press(key);
                                }
                            }
                        });
                    }
                    if self.config.show_fps && self.has_player_view() {
                        // A panel that shows the engine's own window processes no frames.
                        let through = self
                            .display
                            .as_ref()
                            .is_some_and(crate::display::Display::panel_is_transparent);
                        if self.config.android_fps || through {
                            let fps = self.controls.as_ref().and_then(|c| c.surface_fps.value());
                            #[cfg(windows)]
                            let fps = if self.blue_mirror.active() { self.blue_mirror.surface_fps() } else { fps };
                            let value = fps.map_or_else(|| "—".to_owned(), |v| format!("{v:.0}"));
                            divider(ui);
                            ui.label(RichText::new(format!("Android {value} FPS")).size(12.).color(MINT))
                                .on_hover_text(locale.pick(
                                    "Quadros apresentados na superfície do jogo em primeiro plano. O traço indica que não há medição disponível.",
                                    "Frames presented on the foreground game surface. A dash means a measurement is unavailable.",
                                    "Fotogramas presentados en la superficie del juego en primer plano. El guion indica que no hay medición disponible.",
                                ));
                        } else if let Some(display) = &self.display {
                            divider(ui);
                            ui.label(RichText::new(format!("{:.0} FPS", display.fps())).size(12.).color(MINT))
                                .on_hover_text(locale.pick(
                                    "Quadros por segundo processados pelo painel Rust",
                                    "Frames processed per second by the Rust panel",
                                    "Fotogramas por segundo procesados por el panel Rust",
                                ));
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Tool::Close.button(ui, locale) {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        if !fullscreen {
                            let size = if maximized {
                                Tool::Restore
                            } else {
                                Tool::Maximize
                            };
                            if size.button(ui, locale) {
                                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                            }
                        }
                        if Tool::Minimize.button(ui, locale) {
                            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                        }
                        divider(ui);
                        if self.has_player_view() && Tool::Fullscreen.button(ui, locale) {
                            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
                        }
                        if self.playing {
                            ui.add_enabled_ui(!self.stopping(), |ui| {
                                if Tool::Stop.button(ui, locale) {
                                    self.request_stop();
                                }
                            });
                        }
                        if Tool::Help.button(ui, locale) {
                            self.help_tab = super::help::HelpTab::About;
                            self.help_open = true;
                        }
                        if Tool::Settings.button(ui, locale) {
                            self.open_settings();
                        }
                        if let Some(message) =
                            self.controls.as_ref().and_then(|c| c.message.as_ref())
                        {
                            let (short, full, color) = match message {
                                Ok(text) => (text.as_str(), text, MUTED),
                                Err(error) => (
                                    locale.pick(
                                        "Falha no controle",
                                        "Control failed",
                                        "Error del control",
                                    ),
                                    error,
                                    ERROR,
                                ),
                            };
                            ui.add(
                                egui::Label::new(RichText::new(short).size(12.).color(color))
                                    .truncate(),
                            )
                            .on_hover_text(full);
                        }
                    });
                });
            });
    }
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9., 16.), Sense::hover());
    ui.painter()
        .vline(rect.center().x, rect.y_range(), (1., BORDER));
}

/// Ask the window manager to resize. Only a gesture starting on an edge belongs to the window.
pub(super) fn handle_resize(ctx: &egui::Context) -> bool {
    use egui::CursorIcon as Cursor;
    use egui::viewport::ResizeDirection as Edge;

    let (pos, rect, pressed, down) = ctx.input(|i| {
        let v = i.viewport();
        let down = i.pointer.primary_down();
        let pos = if v.maximized == Some(true) || v.fullscreen == Some(true) {
            None
        } else if down {
            i.pointer.press_origin()
        } else {
            i.pointer.hover_pos()
        };
        (pos, i.content_rect(), i.pointer.primary_pressed(), down)
    });
    let Some(pos) = pos.filter(|p| rect.contains(*p)) else {
        return false;
    };
    let left = pos.x - rect.left() < 4.;
    let right = rect.right() - pos.x < 4.;
    let top = pos.y - rect.top() < 4.;
    let bottom = rect.bottom() - pos.y < 4.;
    let (edge, cursor) = match (left, right, top, bottom) {
        (true, _, true, _) => (Edge::NorthWest, Cursor::ResizeNwSe),
        (_, true, true, _) => (Edge::NorthEast, Cursor::ResizeNeSw),
        (true, _, _, true) => (Edge::SouthWest, Cursor::ResizeNeSw),
        (_, true, _, true) => (Edge::SouthEast, Cursor::ResizeNwSe),
        (true, ..) => (Edge::West, Cursor::ResizeHorizontal),
        (_, true, ..) => (Edge::East, Cursor::ResizeHorizontal),
        (_, _, true, _) => (Edge::North, Cursor::ResizeVertical),
        (_, _, _, true) => (Edge::South, Cursor::ResizeVertical),
        _ => return false,
    };
    ctx.set_cursor_icon(cursor);
    if pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(edge));
    }
    down || pressed
}
