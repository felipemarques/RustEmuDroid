use super::{Locale, Tool, theme};
use crate::bluestacks::{
    mirror_control::{KeyAction, TouchAction},
    mirror_session::{MirrorEvent, MirrorInput, MirrorSession, ScreenshotSource, SurfaceFpsQuery},
    player_process::PlayerLifetime,
    preview::{PreviewRect, ThumbnailPreview},
};
use crate::surface_fps::SurfaceFps;
use eframe::egui::{self, Event, Key, PointerButton};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use super::capture_panel::CaptureNoticeStatus;

mod controls;
use controls::MappedControls;

#[cfg(test)]
mod live_mapping_test;
#[cfg(test)]
mod live_test;
#[cfg(test)]
mod tests;

const PREVIEW_CHECK_INTERVAL: Duration = Duration::from_millis(200);

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
    player_lifetime: Option<Arc<PlayerLifetime>>,
    player_check_at: Option<Instant>,
    size: Option<[u32; 2]>,
    last_touch: Option<[u32; 2]>,
    input_geometry: Option<(PreviewRect, f32, [u32; 2])>,
    input_focused: bool,
    rotation_pending: bool,
    refresh_at: Option<Instant>,
    retry_at: Option<Instant>,
    input_error: Option<String>,
    view_error: Option<String>,
    mapped: MappedControls,
    surface_fps: SurfaceFps,
    surface_foreground: Option<(u64, String)>,
    surface_query: Option<SurfaceFpsQuery>,
}

impl MirrorPanel {
    pub(super) fn queue(&mut self, instance: String, package: Option<String>) {
        self.disconnect();
        self.pending = Some((instance, package));
    }

    pub(super) fn disconnect(&mut self) {
        self.reset_surface_fps();
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

    pub(super) fn update_surface_fps(&mut self, enabled: bool, ctx: &egui::Context) {
        if !enabled
            || self
                .surface_query
                .as_ref()
                .is_some_and(|query| !query.current())
        {
            self.reset_surface_fps();
        }
        if !enabled {
            return;
        }
        let Some((epoch, package)) = &self.surface_foreground else {
            return;
        };
        if self.surface_query.is_none() {
            self.surface_query = self
                .session
                .as_ref()
                .and_then(|session| session.surface_fps_query(*epoch));
        }
        if let Some(query) = self.surface_query.clone() {
            self.surface_fps
                .update(Some(package), ctx, move |command| query.run(command));
        }
    }

    pub(super) fn surface_fps(&self) -> Option<f32> {
        self.surface_query
            .as_ref()
            .filter(|query| query.current())?;
        self.surface_fps.value()
    }

    fn reset_surface_fps(&mut self) {
        if let Some(query) = self.surface_query.take() {
            query.revoke();
            self.surface_fps.reset();
        }
    }

    pub(super) fn update(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        native_stopped: bool,
        sdk: &str,
        state_root: &Path,
    ) {
        if !self.active() {
            return;
        }
        if native_stopped
            && self.retiring.is_empty()
            && let Some((instance, package)) = self.pending.take()
        {
            let hint = (!sdk.trim().is_empty())
                .then(|| std::path::Path::new(sdk.trim()).join("platform-tools/adb.exe"));
            self.mapped
                .load(crate::bluestacks::mapping_file(state_root, &instance));
            self.session = Some(MirrorSession::start(instance.clone(), package, hint));
            self.instance = instance;
        }
        if let Ok(handle) = frame.window_handle()
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            self.destination = handle.hwnd.get();
        }
        while let Some(event) = self
            .session
            .as_ref()
            .and_then(|session| session.events.try_recv().ok())
        {
            self.apply_event(event);
        }
        self.poll_player(ctx, Instant::now());
    }

    fn poll_player(&mut self, ctx: &egui::Context, now: Instant) {
        if let Some(lifetime) = &self.player_lifetime
            && self.player_check_at.is_none_or(|time| time <= now)
        {
            if lifetime.is_live() {
                self.player_check_at = Some(now + PREVIEW_CHECK_INTERVAL);
                return;
            }
            self.disconnect();
            if ctx.input(|input| input.viewport().fullscreen.unwrap_or(false)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            ctx.request_repaint();
        }
    }

    fn apply_event(&mut self, event: MirrorEvent) {
        match event {
            MirrorEvent::Player(pid, created, lifetime) => {
                self.player = Some((pid, created));
                self.player_lifetime = Some(lifetime);
                self.player_check_at = None;
            }
            MirrorEvent::Foreground(epoch, package) => {
                let foreground = package.as_deref().map(|package| (epoch, package));
                if self
                    .surface_foreground
                    .as_ref()
                    .map(|(epoch, package)| (*epoch, package.as_str()))
                    != foreground
                {
                    self.reset_surface_fps();
                    self.surface_foreground = package.clone().map(|package| (epoch, package));
                }
                if self.mapped.epoch != Some(epoch) {
                    self.release();
                    self.mapped.set_foreground(epoch, package.as_deref());
                }
            }
            MirrorEvent::EditorFrame(token, image) => self.mapped.receive_frame(token, image),
            MirrorEvent::Ready(size) => self.set_display_size(size),
            MirrorEvent::RotationChecked(size) => {
                self.rotation_pending = false;
                self.layout = None;
                self.refresh_at = None;
                self.set_display_size(size);
            }
            MirrorEvent::ViewOnly(error) => {
                self.reset_surface_fps();
                self.surface_foreground = None;
                eprintln!("BlueStacks mirror (input): {error}");
                self.size = None;
                self.input_error = Some(error);
                self.rotation_pending = false;
                self.release();
            }
            MirrorEvent::Failed(error) => {
                self.reset_surface_fps();
                self.surface_foreground = None;
                eprintln!("BlueStacks mirror (player): {error}");
                self.size = None;
                self.player = None;
                self.player_lifetime = None;
                self.player_check_at = None;
                self.preview = None;
                self.layout = None;
                self.view_error = Some(error);
                self.rotation_pending = false;
                self.release();
            }
        }
    }

    fn set_display_size(&mut self, size: [u32; 2]) {
        if self.size.is_some_and(|previous| previous != size) {
            self.release();
            self.mapped.invalidate_frame();
        }
        self.size = Some(size);
        self.input_error = None;
    }

    fn send(&self, input: MirrorInput) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.input.send(input).is_ok())
    }

    fn screenshot_available(&self) -> bool {
        self.session.is_some()
            && self.size.is_some()
            && self.input_error.is_none()
            && !self.rotation_pending
    }

    pub(super) fn screenshot_source(&self) -> Option<ScreenshotSource> {
        if self.screenshot_available() {
            self.session.as_ref()?.screenshot_source(self.size?)
        } else {
            None
        }
    }

    fn release(&mut self) {
        if self.input_focused || self.last_touch.is_some() || self.mapped.has_contacts() {
            self.send(MirrorInput::Release);
            self.last_touch = None;
            self.input_focused = false;
        }
        self.mapped.release_contacts();
        self.input_geometry = None;
    }

    fn navigate(&mut self, input: MirrorInput) {
        if self.last_touch.is_some() || self.mapped.has_contacts() {
            self.release();
        }
        self.send(input);
    }

    fn request_rotation(&mut self, focused: bool) {
        if focused && self.size.is_some() && !self.rotation_pending {
            self.release();
            self.rotation_pending = self
                .session
                .as_ref()
                .is_some_and(|session| session.input.send(MirrorInput::Rotate).is_ok());
        }
    }

    fn rotate_button(&mut self, ui: &mut egui::Ui, l: Locale) -> egui::Response {
        let focused = ui.input(|input| input.focused);
        ui.add_enabled_ui(
            focused && self.size.is_some() && !self.rotation_pending,
            |ui| {
                let response = Tool::Rotate.icon_button(ui, Tool::Rotate.label(l));
                if response.clicked() {
                    self.request_rotation(focused);
                }
                response
            },
        )
        .inner
    }

    pub(super) fn hide(&mut self) {
        self.release();
        self.layout = None;
        if let Some(preview) = &mut self.preview {
            let _ = preview.hide();
        }
    }

    fn screenshot_button(
        &self,
        ui: &mut egui::Ui,
        l: Locale,
        host_gesture: bool,
    ) -> egui::Response {
        ui.add_enabled_ui(
            self.screenshot_available() && !host_gesture && !self.controls_open(),
            |ui| {
                let tool = Tool::Screenshot;
                let response = tool.icon_button(ui, &tool.label_with_shortcut(l));
                if self.input_error.is_some() {
                    response.on_disabled_hover_text(l.pick(
                        "A captura PNG precisa de uma conexão ADB ativa com o BlueStacks.",
                        "PNG capture requires an active ADB connection to BlueStacks.",
                        "La captura PNG requiere una conexión ADB activa con BlueStacks.",
                    ))
                } else {
                    response
                }
            },
        )
        .inner
    }

    fn toolbar(
        &mut self,
        ctx: &egui::Context,
        l: Locale,
        host_gesture: bool,
        capture_notice: Option<CaptureNoticeStatus<'_>>,
    ) -> (bool, Option<Tool>) {
        let mut disconnect = false;
        let mut action = None;
        egui::TopBottomPanel::top("blue-session").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("BlueStacks · {}", self.instance));
                ui.add_enabled_ui(self.size.is_some() && !host_gesture && !self.controls_open(), |ui| {
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
                    self.rotate_button(ui, l);
                });
                self.controls_button(ui, l, host_gesture);
                if self.screenshot_button(ui, l, host_gesture).clicked() {
                    action = Some(Tool::Screenshot);
                }
                if Tool::CapturesFolder.button(ui, l) {
                    action = Some(Tool::CapturesFolder);
                }
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
                disconnect = ui.add_enabled(!self.controls_open(), theme::button(l.pick("Desconectar", "Disconnect", "Desconectar"))).clicked();
                if let Some((message, details, color)) = &capture_notice {
                    let response = ui.add(egui::Label::new(egui::RichText::new(message).color(*color)).truncate());
                    response.on_hover_text(details.unwrap_or(message));
                } else {
                    self.controls_notice(ui);
                }
            });
        });
        (disconnect, action)
    }

    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        l: Locale,
        obscured: bool,
        chrome: bool,
        host_gesture: bool,
        capture_notice: Option<CaptureNoticeStatus<'_>>,
    ) -> Option<Tool> {
        ctx.request_repaint_after(PREVIEW_CHECK_INTERVAL);
        if obscured {
            self.hide();
            return None;
        }
        self.mapped.set_chrome(chrome);
        let capture = self.controls_shortcuts(ctx, l, host_gesture);
        let mut action = capture.then_some(Tool::Screenshot);
        if chrome {
            let (disconnect, toolbar_action) = self.toolbar(ctx, l, host_gesture, capture_notice);
            if disconnect {
                self.disconnect();
                if ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
                ctx.request_repaint();
                return None;
            }
            action = toolbar_action.or(action);
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
                if self.controls_open() {
                    self.show_controls(ui, area, slot, ppp, l, host_gesture);
                    return;
                }
                if let Some(preview) = &mut self.preview {
                    // DWM updates live content independently. Recheck source geometry at 5 Hz
                    // and immediately when the destination size changes.
                    let positioned = match self.layout {
                        Some((previous, fitted, checked))
                            if previous == slot
                                && now.duration_since(checked) < PREVIEW_CHECK_INTERVAL =>
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
                                    self.handle_game_input(ui, rect, ppp, size, host_gesture, l);
                                } else {
                                    self.release();
                                    if !self.rotation_pending
                                        && self.refresh_at.is_none_or(|time| time <= now)
                                    {
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
        action
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
        let geometry = (rect, ppp, size);
        let point = |pos: egui::Pos2| rect.guest_point([pos.x, pos.y], ppp, size);
        if !self.input_allowed(ctx, rect, ppp, size, host_gesture) {
            return;
        }
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
                                self.input_focused = true;
                                self.input_geometry = Some(geometry);
                                if self.last_touch.is_none()
                                    && !self.mapped.map.captures_pointer(PointerButton::Primary)
                                {
                                    self.send(MirrorInput::Touch(TouchAction::Down, p, size));
                                    self.last_touch = Some(p);
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
                    Event::PointerGone if !self.mapped.map.aims_pointer() => self.release(),
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Secondary,
                        pressed: true,
                        ..
                    } if point(*pos).is_some()
                        && !self.mapped.map.captures_pointer(PointerButton::Secondary) =>
                    {
                        self.navigate(MirrorInput::Back)
                    }
                    Event::MouseWheel { unit, delta, .. } => {
                        if !self.mapped.map.owns_wheel(*delta)
                            && let Some(p) = i.pointer.hover_pos().and_then(point)
                        {
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
                    } if self.input_focused
                        && !self.mapped.map.captures(*key)
                        && !(*key == Key::Escape && self.mapped.map.aims_pointer()) =>
                    {
                        if *key == Key::Escape
                            && *pressed
                            && (self.last_touch.is_some() || self.mapped.has_contacts())
                        {
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

    fn input_allowed(
        &mut self,
        ctx: &egui::Context,
        rect: PreviewRect,
        ppp: f32,
        size: [u32; 2],
        host_gesture: bool,
    ) -> bool {
        let focused = ctx.input(|i| {
            i.focused
                && !i
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::WindowFocused(false)))
        });
        if host_gesture || self.rotation_pending || !focused {
            self.release();
            return false;
        }
        let geometry = (rect, ppp, size);
        if self
            .input_geometry
            .is_some_and(|previous| previous != geometry)
        {
            self.release();
        }
        self.input_geometry = Some(geometry);
        true
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
