use super::{
    Event, Key, Locale, MirrorInput, MirrorPanel, PointerButton, PreviewRect, Tool, TouchAction,
};
use crate::display::mapping::{Contact, Mapping};
use crate::toolbar::{PROFILE_KEY, consume_player_shortcut};
use eframe::egui::{self, Color32, Pos2, Rect};
use std::{
    collections::HashSet,
    io::Cursor,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct MappedControls {
    pub(super) map: Mapping,
    pub(super) epoch: Option<u64>,
    configured: bool,
    foreground: Option<String>,
    active: HashSet<u64>,
    next_frame: u64,
    pending: Option<u64>,
    png: Option<Vec<u8>>,
    image: Option<egui::TextureHandle>,
    image_error: bool,
    notice: Option<(String, Instant)>,
    shortcut_modifiers: [bool; 2],
    chrome: bool,
    editor_chrome: Option<bool>,
    editor_fit: Option<(PreviewRect, PreviewRect)>,
    #[cfg(test)]
    controls_button_center: Option<Pos2>,
}

impl MappedControls {
    pub(super) fn load(&mut self, path: Option<PathBuf>) {
        if let Some(path) = path {
            self.map.use_touch_only_preview();
            self.map.load(path);
            self.configured = true;
        }
    }

    pub(super) fn set_foreground(&mut self, epoch: u64, package: Option<&str>) {
        self.epoch = Some(epoch);
        self.foreground = package.filter(|_| self.configured).map(str::to_owned);
        if self.configured && self.map.package_changed(package) {
            self.map.select(package);
            // A dirty draft can still be saved for its original package using its original frame.
            if !self.map.editor_is_pinned() || self.image.is_none() {
                self.invalidate_frame();
            }
        }
    }

    pub(super) fn has_contacts(&self) -> bool {
        !self.active.is_empty()
    }

    pub(super) fn release_contacts(&mut self) {
        self.active.clear();
        self.map.release();
    }

    pub(super) fn invalidate_frame(&mut self) {
        self.pending = None;
        self.png = None;
        self.image = None;
        self.image_error = false;
        self.editor_fit = None;
    }

    pub(super) fn set_chrome(&mut self, shown: bool) {
        self.chrome = shown;
    }

    pub(super) fn receive_frame(&mut self, token: u64, image: Result<Vec<u8>, String>) {
        if self.pending != Some(token) {
            return;
        }
        self.pending = None;
        match image {
            Ok(png) => self.png = Some(png),
            Err(error) => {
                eprintln!("BlueStacks mirror (mapping frame): {error}");
                self.image_error = true;
            }
        }
    }

    fn contact(&mut self, contact: Contact, size: [u32; 2]) -> Option<MirrorInput> {
        let epoch = self.epoch.filter(|_| self.foreground.is_some())?;
        let id = u64::try_from(contact.id).ok().filter(|id| *id > 0)?;
        if size.contains(&0) || !contact.pos.iter().all(|value| value.is_finite()) {
            return None;
        }
        let point: [u32; 2] = std::array::from_fn(|axis| {
            (contact.pos[axis].clamp(0., 1.) * size[axis] as f32).floor() as u32
        });
        let point = [point[0].min(size[0] - 1), point[1].min(size[1] - 1)];
        let action = if contact.down {
            if self.active.insert(id) {
                TouchAction::Down
            } else {
                TouchAction::Move
            }
        } else if self.active.remove(&id) {
            TouchAction::Up
        } else {
            return None;
        };
        Some(MirrorInput::Contact {
            epoch,
            id,
            action,
            point,
            size,
        })
    }
}

impl MirrorPanel {
    pub(in crate::ui) fn controls_open(&self) -> bool {
        self.mapped.map.editing || self.mapped.map.quick.is_open()
    }

    pub(in crate::ui) fn controls_chrome(&self) -> Option<bool> {
        self.mapped.editor_chrome.filter(|_| self.controls_open())
    }

    #[cfg(test)]
    pub(super) fn controls_test_state(&self) -> (bool, bool, usize, Option<(u64, &str)>) {
        let mapped = &self.mapped;
        (
            mapped.map.editing,
            mapped.image.is_some(),
            mapped.active.len(),
            mapped.epoch.zip(mapped.foreground.as_deref()),
        )
    }

    #[cfg(test)]
    pub(super) fn controls_test_button(&self) -> Option<Pos2> {
        self.mapped.controls_button_center
    }

    fn controls_available(&self) -> bool {
        self.size.is_some() && !self.rotation_pending && self.mapped.foreground.is_some()
    }

    pub(super) fn controls_button(&mut self, ui: &mut egui::Ui, l: Locale, host_gesture: bool) {
        ui.add_enabled_ui(
            !host_gesture && (self.controls_available() || self.controls_open()),
            |ui| {
                let tip = if self.mapped.foreground.is_some() {
                    Tool::Keymap.label(l)
                } else {
                    l.pick(
                        "Abra um jogo no BlueStacks para editar seus controles.",
                        "Open a game in BlueStacks to edit its controls.",
                        "Abre un juego en BlueStacks para editar sus controles.",
                    )
                };
                let button = Tool::Keymap.icon_button(ui, tip);
                #[cfg(test)]
                {
                    self.mapped.controls_button_center = Some(button.rect.center());
                }
                if button.clicked() {
                    self.toggle_controls(false, button.rect.bottom());
                }
            },
        );
    }

    fn toggle_controls(&mut self, direct: bool, anchor: f32) {
        if !self.controls_open() {
            self.mapped.editor_chrome = Some(self.mapped.chrome);
        }
        self.release();
        if self.mapped.map.editing || direct {
            self.mapped.map.close_quick();
            self.mapped.map.toggle();
        } else {
            self.mapped.map.toggle_quick(anchor);
        }
        if !self.controls_open() {
            self.mapped.invalidate_frame();
            self.mapped.editor_chrome = None;
        }
    }

    pub(super) fn controls_shortcuts(
        &mut self,
        ctx: &egui::Context,
        l: Locale,
        host_gesture: bool,
    ) {
        let focused = ctx.input(|i| i.focused);
        let open = self.controls_open();
        let allowed = self.controls_available()
            && !open
            && focused
            && !host_gesture
            && !egui::Popup::is_any_open(ctx)
            && ctx.memory(|m| {
                m.top_modal_layer().is_none()
                    && m.focused()
                        .is_none_or(|id| egui::TextEdit::load_state(ctx, id).is_none())
            });
        let open_editor = consume_player_shortcut(ctx, Key::A, allowed);
        if open_editor {
            self.toggle_controls(true, 0.);
        }
        let switch_profile =
            consume_player_shortcut(ctx, PROFILE_KEY, allowed && !self.controls_open());
        if switch_profile {
            let (changed, message) = self.mapped.map.cycle_profile(l);
            if changed {
                self.release();
            }
            self.mapped.notice = Some((message, Instant::now()));
        }
        let modifiers = ctx.input(|i| i.modifiers);
        self.mapped.shortcut_modifiers[0] &= modifiers.ctrl;
        self.mapped.shortcut_modifiers[1] &= modifiers.shift;
        if open_editor || switch_profile {
            self.mapped.shortcut_modifiers = [modifiers.ctrl, modifiers.shift];
        }
    }

    pub(super) fn controls_notice(&mut self, ui: &mut egui::Ui) {
        if let Some((message, time)) = &self.mapped.notice {
            if let Some(remaining) = Duration::from_secs(2).checked_sub(time.elapsed()) {
                if ui.available_width() > 260. {
                    ui.add_sized(
                        [130., ui.spacing().interact_size.y],
                        egui::Label::new(message).truncate(),
                    );
                }
                ui.ctx().request_repaint_after(remaining);
            } else {
                self.mapped.notice = None;
            }
        }
    }

    pub(super) fn show_controls(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        slot: PreviewRect,
        ppp: f32,
        l: Locale,
        host_gesture: bool,
    ) {
        if let Some(preview) = &mut self.preview {
            let _ = preview.hide();
        }
        if let Some((slot, fitted, _)) = self.layout.take() {
            self.mapped.editor_fit = Some((slot, fitted));
        }
        if let Some(size) = self.size {
            if let Some(png) = self.mapped.png.take() {
                match decode_png(&png, size) {
                    Ok(image) => {
                        self.mapped.image = Some(ui.ctx().load_texture(
                            "blue-mapping-frame",
                            image,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                    Err(error) => {
                        eprintln!("BlueStacks mirror (mapping image): {error}");
                        self.mapped.image_error = true;
                    }
                }
            }
            if self.mapped.image.is_none()
                && self.mapped.pending.is_none()
                && !self.mapped.image_error
                && !self.mapped.map.editor_is_pinned()
                && self.controls_available()
                && let Some(epoch) = self.mapped.epoch
            {
                self.mapped.next_frame = self.mapped.next_frame.wrapping_add(1);
                let token = self.mapped.next_frame;
                if self.send(MirrorInput::CaptureEditor { token, epoch }) {
                    self.mapped.pending = Some(token);
                } else {
                    self.mapped.image_error = true;
                }
            }
            let fitted = self
                .mapped
                .editor_fit
                .filter(|(previous, _)| *previous == slot)
                .map(|(_, fitted)| fitted)
                .unwrap_or_else(|| {
                    slot.fit(PreviewRect {
                        left: 0,
                        top: 0,
                        right: size[0] as i32,
                        bottom: size[1] as i32,
                    })
                });
            let canvas = fitted_rect(fitted, ppp);
            if let Some(texture) = &self.mapped.image {
                ui.painter().image(
                    texture.id(),
                    canvas,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    Color32::WHITE,
                );
            }
            if self.mapped.pending.is_some() || self.mapped.image_error {
                let message = if self.mapped.image_error {
                    l.pick(
                        "A imagem do jogo está indisponível.",
                        "The game image is unavailable.",
                        "La imagen del juego no está disponible.",
                    )
                } else {
                    l.pick(
                        "Obtendo uma imagem do jogo…",
                        "Capturing a game image…",
                        "Capturando una imagen del juego…",
                    )
                };
                ui.painter().text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    egui::FontId::proportional(15.),
                    Color32::WHITE,
                );
            }
            self.mapped.map.blocked_rect = self.mapped.map.panel_rect(area);
            if self.mapped.map.editing {
                ui.painter()
                    .rect_filled(canvas, 0., Color32::from_black_alpha(60));
                let (events, modifiers, focused) =
                    ui.input(|i| (i.events.clone(), i.modifiers, i.focused));
                if focused && !host_gesture && !egui::Popup::is_any_open(ui.ctx()) {
                    self.mapped.map.frame(ui, canvas, &events, modifiers, l);
                } else {
                    self.mapped.map.paint_canvas(ui, canvas);
                }
            }
        }
        self.mapped.map.floating_panel(ui.ctx(), area, l);
        if !self.controls_open() {
            self.mapped.invalidate_frame();
            self.mapped.editor_chrome = None;
            self.mapped.map.blocked_rect = None;
            ui.ctx().request_repaint();
        }
    }

    pub(super) fn handle_game_input(
        &mut self,
        ui: &mut egui::Ui,
        rect: PreviewRect,
        ppp: f32,
        size: [u32; 2],
        host_gesture: bool,
        l: Locale,
    ) {
        if !self.input_allowed(ui.ctx(), rect, ppp, size, host_gesture) {
            return;
        }
        if self.mapped.foreground.is_some() && self.mapped.map.has_preview_mapping() {
            let (mut events, mut modifiers) =
                ui.input(|input| (input.events.clone(), input.modifiers));
            // Feed mappings only after a click inside the preview has taken keyboard focus.
            let start = if self.input_focused {
                Some(0)
            } else {
                events.iter().position(|event| matches!(event, Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. }
                    if rect.guest_point([pos.x, pos.y], ppp, size).is_some()))
            };
            if let Some(start) = start {
                let aiming = self.mapped.map.aims_pointer();
                let end = events[start..]
                    .iter()
                    .position(|event| match event {
                        Event::PointerGone => !aiming,
                        Event::PointerButton {
                            button: PointerButton::Primary,
                            pos,
                            pressed: true,
                            ..
                        } => rect.guest_point([pos.x, pos.y], ppp, size).is_none() && !aiming,
                        _ => false,
                    })
                    .map_or(events.len(), |n| start + n);
                events.truncate(end);
                modifiers.ctrl &= !self.mapped.shortcut_modifiers[0];
                modifiers.shift &= !self.mapped.shortcut_modifiers[1];
                if !self.input_focused {
                    self.mapped.release_contacts();
                }
                let canvas = fitted_rect(rect, ppp);
                let changes = self
                    .mapped
                    .map
                    .frame(ui, canvas, &events[start..], modifiers, l);
                for change in changes {
                    if let Some(input) = self.mapped.contact(change, size) {
                        self.send(input);
                    }
                }
                if aiming {
                    ui.input_mut(|i| {
                        i.events.retain(|event| {
                            !matches!(
                                event,
                                Event::Key {
                                    key: Key::Escape,
                                    ..
                                }
                            )
                        })
                    });
                }
            }
        }
        self.handle_input(ui.ctx(), rect, ppp, size, host_gesture);
    }
}

fn fitted_rect(rect: PreviewRect, ppp: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left as f32 / ppp, rect.top as f32 / ppp),
        Pos2::new(rect.right as f32 / ppp, rect.bottom as f32 / ppp),
    )
}

fn decode_png(bytes: &[u8], size: [u32; 2]) -> Result<egui::ColorImage, String> {
    const MAX_BYTES: usize = 48 * 1024 * 1024;
    let pixels = u64::from(size[0]) * u64::from(size[1]);
    if pixels == 0 || pixels > (MAX_BYTES / 4) as u64 {
        return Err("Android display size exceeds the editor image limit".to_owned());
    }
    let mut decoder =
        png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: MAX_BYTES });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
    let info = reader.info();
    if [info.width, info.height] != size {
        return Err("Android screenshot does not match the active display geometry".to_owned());
    }
    let length = reader
        .output_buffer_size()
        .filter(|n| *n <= MAX_BYTES)
        .ok_or_else(|| "Android screenshot exceeds the editor image limit".to_owned())?;
    let mut raw = vec![0; length];
    let output = reader.next_frame(&mut raw).map_err(|err| err.to_string())?;
    let size = size.map(|axis| axis as usize);
    let raw = &raw[..output.buffer_size()];
    match output.color_type {
        png::ColorType::Rgb => Ok(egui::ColorImage::from_rgb(size, raw)),
        png::ColorType::Rgba => Ok(egui::ColorImage::from_rgba_unmultiplied(size, raw)),
        _ => Err("Unsupported Android screenshot color format".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outdated_editor_frames_cannot_replace_the_current_capture() {
        let mut controls = MappedControls {
            pending: Some(2),
            ..Default::default()
        };
        controls.receive_frame(1, Ok(vec![1]));
        assert!(controls.png.is_none());
        assert_eq!(controls.pending, Some(2));
        controls.receive_frame(2, Ok(vec![2]));
        assert_eq!(controls.png.as_deref(), Some(&[2][..]));
        controls.invalidate_frame();
        controls.receive_frame(2, Ok(vec![2]));
        assert!(controls.png.is_none());
    }
}
