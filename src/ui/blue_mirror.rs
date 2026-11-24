use super::{Locale, Tool, theme};
use crate::bluestacks::{
    mirror_control::{KeyAction, TouchAction},
    mirror_session::{MirrorEvent, MirrorInput, MirrorSession},
    preview::{PreviewRect, ThumbnailPreview},
};
use eframe::egui::{self, Event, Key, PointerButton};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::{Duration, Instant};

#[cfg(test)]
mod live_test;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct MirrorPanel {
    pending: Option<(String, Option<String>)>,
    instance: String,
    session: Option<MirrorSession>,
    retiring: Vec<MirrorSession>,
    preview: Option<ThumbnailPreview>,
    layout: Option<(PreviewRect, PreviewRect, Instant)>,
    destination: isize,
    player: Option<(u32, u64)>,
    size: Option<[u32; 2]>,
    last_touch: Option<[u32; 2]>,
    input_geometry: Option<(PreviewRect, f32, [u32; 2])>,
    input_focused: bool,
    refresh_at: Option<Instant>,
    retry_at: Option<Instant>,
    input_error: Option<String>,
    view_error: Option<String>,
}

impl MirrorPanel {
    pub(super) fn queue(&mut self, instance: String, package: Option<String>) {
        self.disconnect();
        self.pending = Some((instance, package));
    }

    pub(super) fn disconnect(&mut self) {
        self.release();
        if let Some(preview) = self.preview.take() {
            let _ = preview.detach();
        }
        let mut retiring = std::mem::take(&mut self.retiring);
        if let Some(session) = self.session.take() {
            session.cancel();
            retiring.push(session);
        }
        *self = Self {
            retiring,
            ..Default::default()
        };
    }

    pub(super) fn poll_cleanup(&mut self, ctx: &egui::Context) -> bool {
        self.retiring.retain(|session| !session.is_finished());
        if !self.retiring.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(75));
            return true;
        }
        false
    }

    pub(super) fn active(&self) -> bool {
        self.pending.is_some() || self.session.is_some()
    }

    pub(super) fn update(&mut self, frame: &eframe::Frame, native_stopped: bool, sdk: &str) {
        if !self.active() {
            return;
        }
        if native_stopped
            && self.retiring.is_empty()
            && let Some((instance, package)) = self.pending.take()
        {
            let hint = (!sdk.trim().is_empty())
                .then(|| std::path::Path::new(sdk.trim()).join("platform-tools/adb.exe"));
            self.session = Some(MirrorSession::start(instance.clone(), package, hint));
            self.instance = instance;
        }
        if let Ok(handle) = frame.window_handle()
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            self.destination = handle.hwnd.get();
        }
        let mut release = false;
        if let Some(session) = &self.session {
            for event in session.events.try_iter() {
                match event {
                    MirrorEvent::Player(pid, created) => self.player = Some((pid, created)),
                    MirrorEvent::Ready(size) => {
                        release |= self.size.is_some_and(|previous| previous != size);
                        self.size = Some(size);
                        self.input_error = None;
                    }
                    MirrorEvent::ViewOnly(error) => {
                        eprintln!("BlueStacks mirror (input): {error}");
                        self.size = None;
                        self.input_error = Some(error);
                        release = true;
                    }
                    MirrorEvent::Failed(error) => {
                        eprintln!("BlueStacks mirror (player): {error}");
                        self.size = None;
                        self.player = None;
                        self.preview = None;
                        self.layout = None;
                        self.view_error = Some(error);
                        release = true;
                    }
                }
            }
        }
        if release {
            self.release();
        }
    }

    fn send(&self, input: MirrorInput) {
        if let Some(session) = &self.session {
            let _ = session.input.send(input);
        }
    }

    fn release(&mut self) {
        if self.input_focused || self.last_touch.is_some() {
            self.send(MirrorInput::Release);
            self.last_touch = None;
            self.input_focused = false;
        }
        self.input_geometry = None;
    }

    fn navigate(&mut self, input: MirrorInput) {
        if self.last_touch.is_some() {
            self.release();
        }
        self.send(input);
    }

    fn hide(&mut self) {
        self.release();
        self.layout = None;
        if let Some(preview) = &mut self.preview {
            let _ = preview.hide();
        }
    }

    fn toolbar(&mut self, ctx: &egui::Context, l: Locale, host_gesture: bool) -> bool {
        let mut disconnect = false;
        egui::TopBottomPanel::top("blue-session").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("BlueStacks · {}", self.instance));
                ui.add_enabled_ui(self.size.is_some() && !host_gesture, |ui| {
                    for (tool, input) in [
                        (Tool::Back, MirrorInput::Back),
                        (Tool::Home, MirrorInput::Home),
                        (Tool::Recents, MirrorInput::Recents),
                    ] {
                        if tool.button(ui, l) {
                            self.navigate(input);
                        }
                    }
                    for (tool, key) in [(Tool::Louder, 24), (Tool::Quieter, 25), (Tool::Mute, 164)] {
                        if tool.button(ui, l) {
                            self.send(MirrorInput::Key(KeyAction::Down, key, 0, 0));
                            self.send(MirrorInput::Key(KeyAction::Up, key, 0, 0));
                        }
                    }
                });
                if let Some(reason) = &self.input_error {
                    let tip = if reason.contains("ADB access is disabled") {
                        l.pick(
                            "Para controlar o jogo, ative o ADB nas configurações avançadas do BlueStacks e reconecte.",
                            "To control the game, enable ADB in BlueStacks advanced settings and reconnect.",
                            "Para controlar el juego, activa ADB en los ajustes avanzados de BlueStacks y vuelve a conectarte.",
                        )
                    } else { reason };
                    ui.label(l.pick("Somente visualização", "View only", "Solo visualización")).on_hover_text(tip);
                }
                disconnect = ui.add(theme::button(l.pick("Desconectar", "Disconnect", "Desconectar"))).clicked();
            });
        });
        disconnect
    }

    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        l: Locale,
        obscured: bool,
        chrome: bool,
        host_gesture: bool,
    ) {
        ctx.request_repaint_after(Duration::from_millis(200));
        if obscured {
            self.hide();
            return;
        }
        if chrome && self.toolbar(ctx, l, host_gesture) {
            self.disconnect();
            if ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            ctx.request_repaint();
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let area = ui.max_rect().intersect(ui.clip_rect());
                let ppp = ctx.pixels_per_point();
                let slot = PreviewRect::from_points(
                    [area.left(), area.top()],
                    [area.right(), area.bottom()],
                    ppp,
                );
                let now = Instant::now();
                if self.preview.is_none()
                    && let Some((pid, created)) = self.player
                    && self.destination != 0
                    && self.retry_at.is_none_or(|time| time <= now)
                {
                    self.retry_at = Some(now + Duration::from_secs(1));
                    match ThumbnailPreview::attach(self.destination as _, pid, created) {
                        Ok(preview) => {
                            self.preview = Some(preview);
                            self.view_error = None;
                        }
                        Err(e) => self.view_error = Some(e.to_string()),
                    }
                }
                let Some(slot) = slot else {
                    self.hide();
                    return;
                };
                if let Some(preview) = &mut self.preview {
                    // DWM updates live content independently. Recheck source geometry at 5 Hz
                    // and immediately when the destination size changes.
                    let positioned = match self.layout {
                        Some((previous, fitted, checked))
                            if previous == slot
                                && now.duration_since(checked) < Duration::from_millis(200) =>
                        {
                            Ok(fitted)
                        }
                        _ => preview
                            .position(slot)
                            .inspect(|fitted| self.layout = Some((slot, *fitted, now))),
                    };
                    match positioned {
                        Ok(rect) => {
                            let shown = [
                                (rect.right - rect.left) as u32,
                                (rect.bottom - rect.top) as u32,
                            ];
                            if let Some(size) = self.size {
                                let aspect = (shown[0] as f64 * size[1] as f64)
                                    / (shown[1] as f64 * size[0] as f64);
                                if (0.97..=1.03).contains(&aspect) {
                                    self.handle_input(ctx, rect, ppp, size, host_gesture);
                                } else {
                                    self.release();
                                    if self.refresh_at.is_none_or(|time| time <= now) {
                                        self.refresh_size(shown, now);
                                    }
                                }
                            } else if self.input_error.is_none()
                                && self.refresh_at.is_none_or(|time| time <= now)
                            {
                                self.refresh_size(shown, now);
                            }
                            return;
                        }
                        Err(e) => {
                            self.release();
                            self.preview = None;
                            self.layout = None;
                            self.view_error = Some(e.to_string());
                        }
                    }
                }
                ui.centered_and_justified(|ui| {
                    if let Some(error) = &self.view_error {
                        ui.label(format!(
                            "{}\n{error}",
                            l.pick(
                                "Verifique se a janela do BlueStacks está aberta e restaurada.",
                                "Check that the BlueStacks window is open and restored.",
                                "Comprueba que la ventana de BlueStacks esté abierta y restaurada.",
                            )
                        ));
                    } else {
                        ui.label(l.pick(
                            "Aguardando a janela do BlueStacks…",
                            "Waiting for the BlueStacks window…",
                            "Esperando la ventana de BlueStacks…",
                        ));
                    }
                });
            });
    }

    fn refresh_size(&mut self, shown: [u32; 2], now: Instant) {
        self.send(MirrorInput::Refresh(shown));
        self.refresh_at = Some(now + Duration::from_secs(1));
    }

    fn handle_input(
        &mut self,
        ctx: &egui::Context,
        rect: PreviewRect,
        ppp: f32,
        size: [u32; 2],
        host_gesture: bool,
    ) {
        if host_gesture || !ctx.input(|i| i.focused) {
            self.release();
            return;
        }
        let geometry = (rect, ppp, size);
        if self
            .input_geometry
            .is_some_and(|previous| previous != geometry)
        {
            self.release();
        }
        self.input_geometry = Some(geometry);
        let point = |pos: egui::Pos2| rect.guest_point([pos.x, pos.y], ppp, size);
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        ..
                    } => {
                        if *pressed {
                            if let Some(p) = point(*pos) {
                                if self.last_touch.is_none() {
                                    self.send(MirrorInput::Touch(TouchAction::Down, p, size));
                                    self.last_touch = Some(p);
                                    self.input_focused = true;
                                    self.input_geometry = Some(geometry);
                                }
                            } else {
                                self.release();
                            }
                        } else if self.last_touch.is_some() {
                            if let Some(p) = point(*pos) {
                                self.last_touch = None;
                                self.send(MirrorInput::Touch(TouchAction::Up, p, size));
                            } else {
                                self.release();
                            }
                        }
                    }
                    Event::PointerMoved(pos) if self.last_touch.is_some() => {
                        if let Some(p) = point(*pos) {
                            self.send(MirrorInput::Touch(TouchAction::Move, p, size));
                            self.last_touch = Some(p);
                        } else {
                            self.release();
                        }
                    }
                    Event::PointerGone => self.release(),
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Secondary,
                        pressed: true,
                        ..
                    } if point(*pos).is_some() => self.navigate(MirrorInput::Back),
                    Event::MouseWheel { unit, delta, .. } => {
                        if let Some(p) = i.pointer.hover_pos().and_then(point) {
                            let scale = match unit {
                                egui::MouseWheelUnit::Point => 1. / 40.,
                                egui::MouseWheelUnit::Line => 1.,
                                egui::MouseWheelUnit::Page => 1.,
                            };
                            self.send(MirrorInput::Scroll(
                                p,
                                size,
                                delta.x * scale,
                                delta.y * scale,
                            ));
                        }
                    }
                    Event::Key {
                        key,
                        pressed,
                        repeat,
                        modifiers,
                        ..
                    } if self.input_focused => {
                        if *key == Key::Escape && *pressed && self.last_touch.is_some() {
                            self.navigate(MirrorInput::Back);
                            continue;
                        }
                        if let Some(code) = keycode(*key) {
                            let meta = u32::from(modifiers.shift)
                                | (u32::from(modifiers.alt) << 1)
                                | (u32::from(modifiers.ctrl) << 12);
                            self.send(MirrorInput::Key(
                                if *pressed {
                                    KeyAction::Down
                                } else {
                                    KeyAction::Up
                                },
                                code,
                                u32::from(*repeat),
                                meta,
                            ));
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

fn keycode(key: Key) -> Option<u32> {
    Some(match key {
        Key::ArrowUp => 19,
        Key::ArrowDown => 20,
        Key::ArrowLeft => 21,
        Key::ArrowRight => 22,
        Key::Tab => 61,
        Key::Space => 62,
        Key::Enter => 66,
        Key::Backspace => 67,
        Key::Escape => 4,
        Key::Delete => 112,
        _ => {
            let name = key.name();
            let [ch] = name.as_bytes() else { return None };
            match ch {
                b'A'..=b'Z' => u32::from(*ch - b'A') + 29,
                b'0'..=b'9' => u32::from(*ch - b'0') + 7,
                _ => return None,
            }
        }
    })
}
