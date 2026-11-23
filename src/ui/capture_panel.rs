use super::{Display, ERROR, Launcher, MINT, MUTED, PANEL, Tool, theme};
use crate::capture::{Event as CaptureEvent, Mode};
use eframe::egui::{self, RichText, vec2};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) const CAPTURE_MODIFIERS: egui::Modifiers =
    egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT);

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

    pub(super) fn show_capture_tools(&mut self, ui: &mut egui::Ui, interactive: bool) {
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
        if Tool::CapturesFolder.button(ui, locale) {
            self.capture_action(Tool::CapturesFolder);
        }
    }

    pub(super) fn capture_action(&mut self, tool: Tool) {
        let path = self.capture_dir();
        let result = match tool {
            Tool::CapturesFolder => open_capture_dir(&path),
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
            self.capture_notice = Some((CaptureEvent::Failed(error.to_string()), Instant::now()));
        }
    }

    pub(super) fn show_capture_notice(&self, ctx: &egui::Context) {
        let locale = self.config.locale;
        let (message, details, color) = match self.capture.mode() {
            Some(Mode::Video) => (
                locale
                    .pick(
                        "Gravando MP4 sem áudio",
                        "Recording MP4 without audio",
                        "Grabando MP4 sin audio",
                    )
                    .to_owned(),
                None,
                ERROR,
            ),
            Some(Mode::Moment) => (
                locale
                    .pick(
                        "Gravando clipe JXL de 3 segundos",
                        "Recording a 3-second JXL clip",
                        "Grabando un clip JXL de 3 segundos",
                    )
                    .to_owned(),
                None,
                theme::ACCENT,
            ),
            Some(Mode::Gif) => (
                locale
                    .pick(
                        "Gravando GIF de 3 segundos",
                        "Recording a 3-second GIF",
                        "Grabando un GIF de 3 segundos",
                    )
                    .to_owned(),
                None,
                theme::ACCENT,
            ),
            None => {
                let Some((event, time)) = &self.capture_notice else {
                    return;
                };
                let expires = Duration::from_secs(10);
                if time.elapsed() >= expires {
                    return;
                }
                ctx.request_repaint_after(expires.saturating_sub(time.elapsed()));
                match event {
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
                }
            }
        };
        egui::Area::new("capture-notice".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-58., -18.))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(PANEL).show(ui, |ui| {
                    ui.set_max_width(400.);
                    ui.label(RichText::new(message).color(color));
                    if let Some(details) = details {
                        ui.label(RichText::new(details).small().color(MUTED));
                    }
                });
            });
    }
}

pub(super) fn consume_capture_shortcut(ctx: &egui::Context, key: egui::Key) -> bool {
    ctx.input_mut(|input| {
        let mut activated = false;
        input.events.retain(|event| {
            let egui::Event::Key {
                key: event_key,
                modifiers,
                pressed,
                repeat,
                ..
            } = event
            else {
                return true;
            };
            if *event_key != key || !modifiers.matches_exact(CAPTURE_MODIFIERS) {
                return true;
            }
            activated |= *pressed && !*repeat;
            false
        });
        activated
    })
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
