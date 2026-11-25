use crate::ui::i18n::Locale;
use eframe::egui::{self, Color32, Pos2, Shape, Stroke, Vec2};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Louder,
    Quieter,
    Mute,
    Back,
    Home,
    Recents,
    Rotate,
    Install,
    Settings,
    Library,
    Keymap,
    Screenshot,
    RecordVideo,
    StopRecording,
    AnimatedClip,
    AnimatedGif,
    Replay,
    SaveReplay,
    CapturesFolder,
    Fullscreen,
    Help,
    Stop,
    Minimize,
    Maximize,
    Restore,
    Close,
}

pub(crate) const PROFILE_KEY: egui::Key = egui::Key::P;
pub(crate) const SCREENSHOT_KEY: egui::Key = egui::Key::S;
pub(crate) const PLAYER_MODIFIERS: egui::Modifiers =
    egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT);
pub(crate) const FULLSCREEN_KEY: egui::Key = egui::Key::F11;
pub(crate) const PLAYER_SHORTCUTS: [(egui::Key, Tool); 5] = [
    (egui::Key::A, Tool::Keymap),
    (SCREENSHOT_KEY, Tool::Screenshot),
    (egui::Key::R, Tool::RecordVideo),
    (egui::Key::J, Tool::AnimatedClip),
    (egui::Key::H, Tool::SaveReplay),
];

pub(crate) fn chord(key: egui::Key, locale: Locale) -> String {
    format!(
        "Ctrl+{}+{}",
        locale.pick("Shift", "Shift", "Mayús"),
        key.name()
    )
}

pub(crate) fn consume_player_shortcut(ctx: &egui::Context, key: egui::Key, enabled: bool) -> bool {
    if !ctx.input(|input| {
        input.events.iter().any(
            |event| matches!(event, egui::Event::Key { key: event_key, .. } if *event_key == key),
        )
    }) {
        return false;
    }
    let id = egui::Id::new(("player-shortcut-held", key));
    let mut consumed = ctx.data(|data| data.get_temp(id).unwrap_or(false));
    let activated = ctx.input_mut(|input| {
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
            if *event_key != key {
                return true;
            }
            if !*pressed {
                return !std::mem::take(&mut consumed);
            }
            if !*repeat {
                consumed = enabled && modifiers.matches_exact(PLAYER_MODIFIERS);
                activated |= consumed;
            }
            !consumed
        });
        activated
    });
    ctx.data_mut(|data| {
        if consumed {
            data.insert_temp(id, true);
        } else {
            data.remove::<bool>(id);
        }
    });
    activated
}

impl Tool {
    pub(crate) fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Louder => locale.pick("Aumentar volume", "Volume up", "Subir volumen"),
            Self::Quieter => locale.pick("Diminuir volume", "Volume down", "Bajar volumen"),
            Self::Mute => locale.pick(
                "Silenciar / reativar mídia",
                "Mute or unmute media",
                "Silenciar o reactivar el sonido",
            ),
            Self::Back => locale.pick("Voltar", "Back", "Atrás"),
            Self::Home => locale.pick("Início", "Home", "Inicio"),
            Self::Recents => locale.pick(
                "Aplicativos recentes",
                "Recent apps",
                "Aplicaciones recientes",
            ),
            Self::Rotate => locale.pick("Girar tela", "Rotate screen", "Girar pantalla"),
            Self::Install => locale.pick("Instalar APK", "Install APK", "Instalar APK"),
            Self::Settings => locale.pick("Configurações", "Settings", "Ajustes"),
            Self::Library => locale.pick(
                "Biblioteca de aplicativos",
                "App library",
                "Biblioteca de aplicaciones",
            ),
            Self::Keymap => locale.pick(
                "Mapear teclado e controle",
                "Map keyboard and controller",
                "Asignar teclado y mando",
            ),
            Self::Screenshot => locale.pick(
                "Capturar tela PNG",
                "Capture PNG screenshot",
                "Capturar pantalla en PNG",
            ),
            Self::RecordVideo => locale.pick(
                "Gravar MP4 sem áudio",
                "Record MP4 without audio",
                "Grabar MP4 sin audio",
            ),
            Self::StopRecording => {
                locale.pick("Parar gravação", "Stop recording", "Detener la grabación")
            }
            Self::AnimatedClip => locale.pick(
                "Gravar os próximos 3 s em JPEG XL",
                "Record the next 3 seconds as JPEG XL",
                "Grabar los próximos 3 s en JPEG XL",
            ),
            Self::AnimatedGif => locale.pick(
                "Gravar os próximos 3 s em GIF",
                "Record the next 3 seconds as GIF",
                "Grabar los próximos 3 s en GIF",
            ),
            Self::Replay => locale.pick(
                "Replay dos últimos 3 s",
                "Replay the last 3 seconds",
                "Repetición de los últimos 3 s",
            ),
            Self::SaveReplay => locale.pick(
                "Salvar os últimos 3 s em JPEG XL",
                "Save the last 3 seconds as JPEG XL",
                "Guardar los últimos 3 s en JPEG XL",
            ),
            Self::CapturesFolder => locale.pick(
                "Abrir pasta de capturas",
                "Open captures folder",
                "Abrir carpeta de capturas",
            ),
            Self::Fullscreen => locale.pick(
                "Alternar tela cheia (F11)",
                "Toggle fullscreen (F11)",
                "Alternar pantalla completa (F11)",
            ),
            Self::Help => locale.pick("Ajuda", "Help", "Ayuda"),
            Self::Stop => locale.pick("Encerrar Android", "Stop Android", "Apagar Android"),
            Self::Minimize => locale.pick("Minimizar", "Minimize", "Minimizar"),
            Self::Maximize => locale.pick("Maximizar", "Maximize", "Maximizar"),
            Self::Restore => locale.pick("Restaurar janela", "Restore window", "Restaurar ventana"),
            Self::Close => locale.pick("Fechar janela", "Close window", "Cerrar ventana"),
        }
    }

    pub(crate) fn label_with_shortcut(self, locale: Locale) -> Cow<'static, str> {
        let base = self.label(locale);
        let action = if self == Self::StopRecording {
            Self::RecordVideo
        } else {
            self
        };
        if let Some((key, _)) = PLAYER_SHORTCUTS.iter().find(|(_, tool)| *tool == action) {
            Cow::Owned(format!("{base} ({})", chord(*key, locale)))
        } else {
            Cow::Borrowed(base)
        }
    }

    pub(crate) fn icon_button(self, ui: &mut egui::Ui, label: &str) -> egui::Response {
        let response = ui.add(
            egui::Button::new("")
                .min_size(Vec2::splat(32.0))
                .frame_when_inactive(false),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let color = ui.style().interact(&response).fg_stroke.color;
        self.paint(ui.painter(), response.rect.center(), color);
        response.on_hover_text(label)
    }

    pub fn button(self, ui: &mut egui::Ui, locale: Locale) -> bool {
        let response = self.icon_button(ui, &self.label_with_shortcut(locale));
        if !matches!(self, Self::Louder | Self::Quieter) {
            return response.clicked();
        }
        let now = ui.input(|i| i.time);
        let id = response.id.with("repeat");
        let down = response.is_pointer_button_down_on();
        let clicked = response.clicked();
        ui.ctx().data_mut(|data| {
            let (next, repeated) = data
                .get_temp::<(f64, bool)>(id)
                .unwrap_or((now + 0.4, false));
            if down {
                let due = now >= next;
                data.insert_temp(id, (if due { now + 0.18 } else { next }, due || repeated));
                due
            } else {
                data.remove::<(f64, bool)>(id);
                clicked && !repeated
            }
        })
    }

    fn paint(self, painter: &egui::Painter, center: Pos2, color: Color32) {
        let point = |(x, y): (f32, f32)| center + egui::vec2(x, y);
        let stroke = Stroke::new(1.65_f32, color);
        let path = |points: &[(f32, f32)]| {
            painter.add(Shape::line(
                points.iter().copied().map(point).collect(),
                stroke,
            ));
        };
        match self {
            Self::Screenshot => {
                path(&[
                    (-8., 6.),
                    (-8., -4.),
                    (-4., -4.),
                    (-2., -7.),
                    (3., -7.),
                    (5., -4.),
                    (8., -4.),
                    (8., 6.),
                    (-8., 6.),
                ]);
                painter.circle_stroke(center + egui::vec2(0., 1.), 3., stroke);
            }
            Self::RecordVideo | Self::StopRecording => {
                let body = egui::Rect::from_min_max(point((-9., -5.5)), point((3., 5.5)));
                painter.rect_stroke(body, 2., stroke, egui::StrokeKind::Middle);
                path(&[(3., -1.5), (9., -5.), (9., 5.), (3., 1.5)]);
                if self == Self::StopRecording {
                    painter.rect_filled(
                        egui::Rect::from_center_size(body.center(), Vec2::splat(6.)),
                        1.,
                        Color32::from_rgb(255, 130, 127),
                    );
                }
            }
            Self::AnimatedClip => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, Vec2::new(16., 12.)),
                    2.,
                    stroke,
                    egui::StrokeKind::Middle,
                );
                path(&[(-2., -3.), (3., 0.), (-2., 3.), (-2., -3.)]);
            }
            Self::AnimatedGif => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, Vec2::new(22., 13.)),
                    2.,
                    stroke,
                    egui::StrokeKind::Middle,
                );
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "GIF",
                    egui::FontId::proportional(9.),
                    color,
                );
            }
            Self::Replay | Self::SaveReplay => {
                let arc = (0..=24)
                    .map(|i| {
                        let angle = (-145.0_f32 + i as f32 * 285. / 24.).to_radians();
                        center + Vec2::angled(angle) * 7.5
                    })
                    .collect();
                painter.add(Shape::line(arc, stroke));
                path(&[(-7., -8.), (-7., -4.), (-3., -4.)]);
                path(&[(0., -3.), (0., 0.), (3., 2.)]);
            }
            Self::CapturesFolder => path(&[
                (-8., 6.),
                (-8., -6.),
                (-2., -6.),
                (0., -3.),
                (8., -3.),
                (8., 6.),
                (-8., 6.),
            ]),
            Self::Keymap => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, Vec2::new(18., 13.)),
                    2.,
                    stroke,
                    egui::StrokeKind::Middle,
                );
                for x in [-5., 0., 5.] {
                    for y in [-2.5, 0.5] {
                        painter.circle_filled(center + egui::vec2(x, y), 0.9, color);
                    }
                }
                path(&[(-4., 3.5), (4., 3.5)]);
            }
            Self::Library => {
                for x in [-4., 4.] {
                    for y in [-4., 4.] {
                        painter.rect_stroke(
                            egui::Rect::from_center_size(
                                center + egui::vec2(x, y),
                                Vec2::splat(5.5),
                            ),
                            1.,
                            stroke,
                            egui::StrokeKind::Middle,
                        );
                    }
                }
            }
            Self::Help => {
                painter.circle_stroke(center, 7.5, stroke);
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "?",
                    egui::FontId::proportional(13.),
                    color,
                );
            }
            Self::Stop => {
                path(&[(0., -8.), (0., -1.)]);
                let arc = (0..=24)
                    .map(|i| {
                        let angle = (-56. + i as f32 * 292. / 24.).to_radians();
                        center + Vec2::angled(angle) * 7.
                    })
                    .collect();
                painter.add(Shape::line(arc, stroke));
            }
            Self::Minimize => path(&[(-5., 0.), (5., 0.)]),
            Self::Maximize => path(&[(-5., -5.), (5., -5.), (5., 5.), (-5., 5.), (-5., -5.)]),
            Self::Restore => {
                path(&[(-5., -2.), (2., -2.), (2., 5.), (-5., 5.), (-5., -2.)]);
                path(&[(-2., -4.), (-2., -5.), (5., -5.), (5., 2.), (4., 2.)]);
            }
            Self::Close => {
                path(&[(-5., -5.), (5., 5.)]);
                path(&[(-5., 5.), (5., -5.)]);
            }
            Self::Louder | Self::Quieter | Self::Mute => {
                path(&[
                    (-8., -3.),
                    (-5., -3.),
                    (0., -7.),
                    (0., 7.),
                    (-5., 3.),
                    (-8., 3.),
                    (-8., -3.),
                ]);
                if self == Self::Mute {
                    path(&[(3., -2.5), (8., 2.5)]);
                    path(&[(3., 2.5), (8., -2.5)]);
                } else {
                    path(&[(3., 0.), (9., 0.)]);
                    if self == Self::Louder {
                        path(&[(6., -3.), (6., 3.)]);
                    }
                }
            }
            Self::Back => {
                path(&[(-2., -6.), (-8., 0.), (-2., 6.)]);
                path(&[(-8., 0.), (8., 0.)]);
            }
            Self::Home => {
                painter.circle_stroke(center, 7.0, stroke);
            }
            Self::Recents => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, Vec2::splat(14.)),
                    2.,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
            Self::Rotate => {
                path(&[
                    (-6., -2.),
                    (-6., -5.),
                    (4., -5.),
                    (4., 2.),
                    (-6., 2.),
                    (-6., -2.),
                ]);
                path(&[(-8., 5.), (-5., 8.), (1., 8.), (6., 5.), (8., 1.)]);
                path(&[(5., 1.), (8., 0.), (8., 4.)]);
            }
            Self::Install => {
                path(&[(0., -8.), (0., 3.)]);
                path(&[(-4., -1.), (0., 3.), (4., -1.)]);
                path(&[(-7., 3.), (-7., 7.), (7., 7.), (7., 3.)]);
            }
            Self::Settings => {
                painter.circle_stroke(center, 5.7, stroke);
                painter.circle_stroke(center, 2., stroke);
                for i in 0..8 {
                    let dir = Vec2::angled(i as f32 * std::f32::consts::FRAC_PI_4);
                    painter.line_segment([center + dir * 5.5, center + dir * 8.], stroke);
                }
            }
            Self::Fullscreen => {
                for x in [-1., 1.] {
                    for y in [-1., 1.] {
                        path(&[(x * 3., y * 7.), (x * 7., y * 7.), (x * 7., y * 3.)]);
                    }
                }
            }
        }
    }
}

pub(super) fn audio_buttons(
    ui: &mut egui::Ui,
    controls: &mut crate::controls::Controls,
    locale: Locale,
) {
    use crate::controls::Action;
    for (tool, action) in [
        (Tool::Louder, Action::Louder),
        (Tool::Quieter, Action::Quieter),
        (Tool::Mute, Action::ToggleMute),
    ] {
        if tool.button(ui, locale) {
            controls.send(action, ui.ctx());
        }
    }
}

pub fn session_tools(
    ui: &mut egui::Ui,
    controls: &mut crate::controls::Controls,
    ready: bool,
    locale: Locale,
) {
    use crate::controls::Action;
    let ctx = ui.ctx().clone();
    ui.add_enabled_ui(ready, |ui| {
        audio_buttons(ui, controls, locale);
        ui.separator();
        if Tool::Rotate.button(ui, locale) {
            controls.send(Action::Rotate, &ctx);
        }
        if Tool::Install.button(ui, locale)
            && !controls.busy()
            && let Some(path) = rfd::FileDialog::new()
                .set_title(locale.pick("Instalar APK", "Install APK", "Instalar APK"))
                .add_filter(
                    locale.pick("Pacote Android", "Android package", "Paquete Android"),
                    &["apk"],
                )
                .pick_file()
        {
            controls.send(Action::Install(path), &ctx);
        }
    });
}
