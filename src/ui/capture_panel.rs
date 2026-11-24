use super::{Display, ERROR, Launcher, Locale, MINT, MUTED, PANEL, Tool, theme};
use crate::capture::{Event as CaptureEvent, Mode};
use eframe::egui::{self, RichText, vec2};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) fn replay_popup_id() -> egui::Id {
    egui::Id::new("capture-replay-menu")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayState {
    Off,
    Buffering,
    Ready,
    Saving,
}

impl ReplayState {
    fn status(self, locale: Locale) -> (&'static str, egui::Color32) {
        match self {
            Self::Off => (
                locale.pick("Replay desativado", "Replay off", "Repetición desactivada"),
                MUTED,
            ),
            Self::Buffering => (
                locale.pick(
                    "Preenchendo o buffer de replay…",
                    "Buffering replay…",
                    "Llenando el búfer de repetición…",
                ),
                theme::ACCENT,
            ),
            Self::Ready => (
                locale.pick(
                    "Replay pronto · últimos 3 s",
                    "Replay ready · last 3 s",
                    "Repetición lista · últimos 3 s",
                ),
                MINT,
            ),
            Self::Saving => (
                locale.pick(
                    "Salvando replay em JPEG XL…",
                    "Saving replay as JPEG XL…",
                    "Guardando repetición en JPEG XL…",
                ),
                theme::ACCENT,
            ),
        }
    }
}

impl Launcher {
    pub(super) fn capture_dir(&self) -> PathBuf {
        self.prefs_file
            .parent()
            .unwrap_or(Path::new("."))
            .join("captures")
    }

    pub(super) fn record_tool(&self) -> Tool {
        if matches!(self.capture.mode(), Some(Mode::Video)) {
            Tool::StopRecording
        } else {
            Tool::RecordVideo
        }
    }

    pub(super) fn show_capture_tools(&mut self, ui: &mut egui::Ui, interactive: bool) -> bool {
        let locale = self.config.locale;
        let record = self.record_tool();
        ui.separator();
        for tool in [
            Tool::Screenshot,
            record,
            Tool::AnimatedClip,
            Tool::AnimatedGif,
        ] {
            let ready = interactive
                && self.display.as_ref().and_then(Display::shot).is_some()
                && (tool == Tool::Screenshot
                    || tool == Tool::StopRecording
                    || !self.capture.busy());
            ui.add_enabled_ui(ready, |ui| {
                if tool.button(ui, locale) {
                    self.capture_action(tool);
                }
            });
        }
        let save_replay = self.show_replay_tools(ui, interactive);
        if Tool::CapturesFolder.button(ui, locale) {
            self.capture_action(Tool::CapturesFolder);
        }
        save_replay
    }

    pub(super) fn capture_action(&mut self, tool: Tool) {
        let path = self.capture_dir();
        let result = match tool {
            Tool::CapturesFolder => open_capture_dir(&path),
            Tool::SaveReplay => {
                self.save_replay(&path);
                return;
            }
            Tool::StopRecording => {
                self.capture.stop();
                Ok(())
            }
            _ => {
                let Some(shot) = self.display.as_ref().and_then(Display::shot) else {
                    return;
                };
                match tool {
                    Tool::Screenshot => self.capture.png(shot, &path),
                    Tool::RecordVideo if !self.capture.busy() => {
                        self.capture.start(Mode::Video, shot, &path)
                    }
                    Tool::AnimatedClip if !self.capture.busy() => {
                        self.capture.start(Mode::Moment, shot, &path)
                    }
                    Tool::AnimatedGif if !self.capture.busy() => {
                        self.capture.start(Mode::Gif, shot, &path)
                    }
                    _ => return,
                }
            }
        };
        if let Err(error) = result {
            self.capture_failed(error.to_string());
        }
    }

    fn capture_failed(&mut self, details: impl Into<String>) {
        self.capture_notice = Some((CaptureEvent::Failed(details.into()), Instant::now()));
    }

    fn replay_state(&self) -> ReplayState {
        if self.capture.replay_saving() {
            ReplayState::Saving
        } else if !self.capture.replay_armed() {
            ReplayState::Off
        } else if self.capture.replay_ready() {
            ReplayState::Ready
        } else {
            ReplayState::Buffering
        }
    }

    fn show_replay_tools(&mut self, ui: &mut egui::Ui, interactive: bool) -> bool {
        let locale = self.config.locale;
        let popup_id = replay_popup_id();
        let state = self.replay_state();
        let (status, color) = state.status(locale);
        let response = ui
            .add_enabled_ui(interactive, |ui| {
                let label = format!("{}\n{status}", Tool::Replay.label(locale));
                Tool::Replay.icon_button(ui, &label)
            })
            .inner;
        if state != ReplayState::Off {
            ui.painter()
                .circle_filled(response.rect.center() + vec2(9., -8.), 2.5, color);
        }
        if !interactive {
            egui::Popup::close_id(ui.ctx(), popup_id);
            return false;
        }
        egui::Popup::menu(&response)
            .id(popup_id)
            .align(egui::RectAlign::LEFT_END)
            .gap(5.)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                let armed = self.capture.replay_armed();
                let label = if armed {
                    locale.pick(
                        "Desativar buffer de replay",
                        "Disable replay buffer",
                        "Desactivar el búfer de repetición",
                    )
                } else {
                    locale.pick(
                        "Ativar buffer de replay",
                        "Enable replay buffer",
                        "Activar el búfer de repetición",
                    )
                };
                ui.label(RichText::new(status).strong().color(color));
                ui.separator();
                if ui
                    .add_enabled(
                        armed
                            || self
                                .display
                                .as_ref()
                                .and_then(Display::replay_shot)
                                .is_some(),
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    self.set_replay_armed(!armed);
                }
                let save_replay = ui
                    .add_enabled(
                        state == ReplayState::Ready && !self.capture.busy(),
                        egui::Button::new(Tool::SaveReplay.label_with_shortcut(locale)),
                    )
                    .clicked();
                if save_replay {
                    ui.close();
                }
                save_replay
            })
            .is_some_and(|response| response.inner)
    }

    pub(super) fn set_replay_armed(&mut self, armed: bool) {
        if let Err(error) = self.capture.set_replay_armed(armed) {
            self.capture_failed(error.to_string());
        }
    }

    pub(super) fn sample_replay(&mut self, ctx: &egui::Context) {
        if !self.capture.replay_armed() {
            return;
        }
        if let Some(shot) = self.display.as_ref().and_then(Display::replay_shot) {
            self.capture.update_replay(shot);
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            self.set_replay_armed(false);
            self.capture_failed(self.replay_source_unavailable());
        }
    }

    fn replay_source_unavailable(&self) -> &'static str {
        self.config.locale.pick(
            "A imagem do Android não está disponível para replay.",
            "The Android display is unavailable for replay.",
            "La imagen de Android no está disponible para la repetición.",
        )
    }

    fn save_replay(&mut self, path: &Path) {
        let locale = self.config.locale;
        let details = if self
            .display
            .as_ref()
            .and_then(Display::replay_shot)
            .is_none()
        {
            self.set_replay_armed(false);
            self.replay_source_unavailable()
        } else if !self.capture.replay_armed() {
            locale.pick(
                "Ative o buffer de replay no menu da barra lateral.",
                "Enable the replay buffer from the sidebar menu.",
                "Activa el búfer de repetición en el menú de la barra lateral.",
            )
        } else if self.capture.busy() {
            locale.pick(
                "Aguarde a captura em andamento terminar.",
                "Wait for the current capture to finish.",
                "Espera a que termine la captura en curso.",
            )
        } else if !self.capture.replay_ready() {
            locale.pick(
                "Aguarde o buffer acumular três segundos.",
                "Wait for the buffer to collect three seconds.",
                "Espera a que el búfer acumule tres segundos.",
            )
        } else {
            match self.capture.save_replay(path) {
                Ok(()) => self.capture_notice = None,
                Err(error) => self.capture_failed(error.to_string()),
            }
            return;
        };
        self.capture_failed(details);
    }

    pub(super) fn show_capture_notice(&self, ctx: &egui::Context) {
        let locale = self.config.locale;
        let replay = self.replay_state();
        let notice = match self.capture.mode() {
            Some(Mode::Video) => Some((
                locale
                    .pick(
                        "Gravando MP4 sem áudio",
                        "Recording MP4 without audio",
                        "Grabando MP4 sin audio",
                    )
                    .to_owned(),
                None,
                ERROR,
            )),
            Some(Mode::Moment) => Some((
                locale
                    .pick(
                        "Gravando clipe JXL de 3 segundos",
                        "Recording a 3-second JXL clip",
                        "Grabando un clip JXL de 3 segundos",
                    )
                    .to_owned(),
                None,
                theme::ACCENT,
            )),
            Some(Mode::Gif) => Some((
                locale
                    .pick(
                        "Gravando GIF de 3 segundos",
                        "Recording a 3-second GIF",
                        "Grabando un GIF de 3 segundos",
                    )
                    .to_owned(),
                None,
                theme::ACCENT,
            )),
            None => self.capture_notice.as_ref().and_then(|(event, time)| {
                let remaining = Duration::from_secs(10).saturating_sub(time.elapsed());
                if remaining.is_zero() {
                    return None;
                }
                ctx.request_repaint_after(remaining);
                Some(match event {
                    CaptureEvent::Saved(path) => (
                        format!(
                            "{} {}",
                            locale.pick("Salvo", "Saved", "Guardado"),
                            path.file_name()
                                .unwrap_or(path.as_os_str())
                                .to_string_lossy()
                        ),
                        None,
                        MINT,
                    ),
                    CaptureEvent::Failed(error) => (
                        locale
                            .pick("Falha na captura", "Capture failed", "Error en la captura")
                            .to_owned(),
                        Some(error.as_str()),
                        ERROR,
                    ),
                })
            }),
        };
        let replay_notice = matches!(replay, ReplayState::Buffering | ReplayState::Saving);
        if notice.is_none() && !replay_notice {
            return;
        }
        egui::Area::new("capture-notice".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-58., -18.))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(PANEL).show(ui, |ui| {
                    ui.set_max_width(400.);
                    if let Some((message, details, color)) = notice {
                        ui.label(RichText::new(message).color(color));
                        if let Some(details) = details {
                            ui.label(RichText::new(details).small().color(MUTED));
                        }
                    }
                    if replay_notice {
                        let (status, color) = replay.status(locale);
                        ui.label(RichText::new(status).color(color));
                    }
                });
            });
    }
}

fn open_capture_dir(path: &Path) -> std::io::Result<()> {
    use std::process::Command;

    std::fs::create_dir_all(path)?;
    #[cfg(windows)]
    let opener = "explorer.exe";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let opener = "xdg-open";
    Command::new(opener).arg(path).spawn()?;
    Ok(())
}
