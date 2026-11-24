use super::{Launcher, Locale, MINT, MUTED, PANEL, mascot, theme, version_label};
use crate::toolbar::{FULLSCREEN_KEY, PLAYER_SHORTCUTS, Tool, chord};
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HelpTab {
    About,
    Shortcuts,
}

impl Launcher {
    pub(super) fn show_help(&mut self, ctx: &egui::Context) {
        let l = self.config.locale;
        let dialog = egui::Modal::new("help-dialog".into())
            .frame(
                egui::Frame::popup(&ctx.style())
                    .fill(PANEL)
                    .inner_margin(20)
                    .corner_radius(16),
            )
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 60.).clamp(220., 500.));
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size.y = 30.;
                    ui.spacing_mut().button_padding = egui::vec2(11., 5.);
                    ui.selectable_value(
                        &mut self.help_tab,
                        HelpTab::About,
                        l.pick("Sobre", "About", "Acerca de"),
                    );
                    ui.selectable_value(
                        &mut self.help_tab,
                        HelpTab::Shortcuts,
                        l.pick("Atalhos", "Shortcuts", "Atajos"),
                    );
                });
                ui.separator();
                match self.help_tab {
                    HelpTab::About => ui.vertical_centered(|ui| {
                        mascot::show(
                            ui,
                            self.brand.id(),
                            120.,
                            self.config.animate_mascot,
                            self.home_ready,
                        );
                        ui.add_space(8.);
                        ui.heading("RustEmuDroid");
                        ui.label(RichText::new(version_label(l)).color(MINT));
                    }),
                    HelpTab::Shortcuts => ui.vertical(|ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("help-shortcuts")
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                            )
                            .max_height((ctx.content_rect().height() - 155.).clamp(80., 400.))
                            .auto_shrink([false, true])
                            .show(ui, |ui| shortcuts(ui, l));
                    }),
                };
                ui.add_space(12.);
                ui.vertical_centered(|ui| {
                    if ui
                        .add(theme::button(l.pick("Fechar", "Close", "Cerrar")))
                        .clicked()
                    {
                        ui.close();
                    }
                });
            });
        if dialog.should_close() {
            self.help_open = false;
        }
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.);
    ui.label(RichText::new(text).color(MINT).strong());
    ui.add_space(2.);
}

fn entry(ui: &mut egui::Ui, keys: &str, description: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(126., 18.),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(126.);
                ui.label(RichText::new(keys).monospace().strong());
            },
        );
        ui.add(egui::Label::new(description).wrap());
    });
    ui.add_space(5.);
}

fn shortcuts(ui: &mut egui::Ui, l: Locale) {
    heading(
        ui,
        l.pick(
            "Durante a sessão Android",
            "During an Android session",
            "Durante la sesión de Android",
        ),
    );
    ui.label(RichText::new(l.pick(
        "Feche a ajuda e o editor para usar os atalhos de captura. Os arquivos ficam na pasta de capturas da barra lateral.",
        "Close Help and the controls editor to capture. Open the folder on the sidebar to find your files.",
        "Cierra la ayuda y el editor para capturar. Encontrarás los archivos en la carpeta de la barra lateral.",
    )).size(13.).color(MUTED));
    ui.add_space(7.);
    for (key, tool) in PLAYER_SHORTCUTS {
        let description = match tool {
            Tool::Keymap => l.pick("Abra o editor sobre o jogo para associar teclas, mouse e controle aos pontos de toque.",
                "Open the editor over your game to bind keys, mouse and controller inputs to touch targets.",
                "Abre el editor sobre el juego para asignar el teclado, el ratón y el mando a puntos táctiles."),
            Tool::Screenshot => l.pick("Salve a imagem atual do Android em PNG.", "Save the current Android frame as a PNG.",
                "Guarda la imagen actual de Android como PNG."),
            Tool::RecordVideo => l.pick("Pressione uma vez para gravar um MP4 sem áudio. Pressione de novo para parar e salvar.",
                "Press once to record a silent MP4. Press again to stop and save it.",
                "Pulsa una vez para grabar un MP4 sin sonido. Vuelve a pulsar para detenerlo y guardarlo."),
            Tool::AnimatedClip => l.pick("Grave os próximos três segundos em JPEG XL animado. O clipe é salvo automaticamente.",
                "Record the next three seconds as an animated JPEG XL. The clip saves automatically.",
                "Graba los tres segundos siguientes en JPEG XL animado. El clip se guarda automáticamente."),
            Tool::SaveReplay => l.pick("Ative o buffer no menu de replay da barra lateral. Quando estiver pronto, salve os últimos três segundos em JPEG XL animado. O buffer fica na memória até você desativá-lo ou encerrar a sessão.",
                "Enable the buffer in the sidebar replay menu. When it is ready, save the last three seconds as an animated JPEG XL. The buffer stays in memory until you disable it or end the session.",
                "Activa el búfer en el menú de repetición de la barra lateral. Cuando esté listo, guarda los últimos tres segundos en JPEG XL animado. El búfer se guarda en la memoria hasta que lo desactives o termines la sesión."),
            other => other.label(l),
        };
        entry(ui, &chord(key, l), description);
        if tool == Tool::Keymap {
            entry(ui, &chord(crate::toolbar::PROFILE_KEY, l), l.pick(
                "Durante o jogo, passe ao próximo perfil de controles salvo. Depois do último, volte ao primeiro.",
                "During play, switch to the next saved control profile. After the last profile, return to the first.",
                "Durante el juego, cambia al siguiente perfil de controles guardado. Después del último, vuelve al primero.",
            ));
        }
    }
    heading(ui, l.pick("Janela", "Window", "Ventana"));
    entry(
        ui,
        FULLSCREEN_KEY.name(),
        l.pick(
            "Entre ou saia da tela cheia durante a sessão.",
            "Enter or leave fullscreen during a session.",
            "Entra o sal de la pantalla completa durante la sesión.",
        ),
    );
    entry(
        ui,
        "Esc",
        l.pick(
            "Feche esta ajuda.",
            "Close this help window.",
            "Cierra esta ayuda.",
        ),
    );
    #[cfg(windows)]
    entry(ui, "Alt+F4", l.pick("Feche o RustEmuDroid. A janela aguarda o Android desligar e a captura em andamento terminar.",
        "Close RustEmuDroid. The window waits for Android to shut down and the current capture to finish.",
        "Cierra RustEmuDroid. La ventana espera a que Android se apague y termine la captura en curso."));

    heading(
        ui,
        l.pick(
            "No jogo e no editor",
            "In games and the editor",
            "En el juego y el editor",
        ),
    );
    entry(ui, "Esc", l.pick("No editor, cancele a atribuição em andamento ou feche o painel. Durante o jogo, solte a mira capturada ou volte no Android.",
        "In the editor, cancel the current binding or close the panel. During a game, release captured aim or go back in Android.",
        "En el editor, cancela la asignación en curso o cierra el panel. En el juego, libera la mira capturada o vuelve atrás en Android."));
    entry(
        ui,
        "W / A / S / D",
        l.pick(
            "Mova o direcional virtual depois de posicioná-lo e salvar o perfil do jogo.",
            "Move the virtual D-pad after placing it and saving the game's profile.",
            "Mueve la cruceta virtual después de colocarla y guardar el perfil del juego.",
        ),
    );
    entry(
        ui,
        "Enter",
        l.pick(
            "Confirme o nome ao criar ou duplicar um perfil no editor.",
            "Confirm the name when creating or duplicating an editor profile.",
            "Confirma el nombre al crear o duplicar un perfil en el editor.",
        ),
    );
    entry(
        ui,
        l.pick("Clique direito", "Right-click", "Clic derecho"),
        l.pick(
            "Remova o controle sob o ponteiro no editor.",
            "Remove the control under the pointer in the editor.",
            "Elimina el control bajo el puntero en el editor.",
        ),
    );
    ui.label(RichText::new(l.pick(
        "O editor mostra as atribuições do perfil do jogo. Com o Android em foco, teclas livres como as setas, Tab, Enter, espaço, Backspace e Delete são enviadas ao aplicativo.",
        "The editor shows bindings for the current game profile. With Android focused, unbound keys such as arrows, Tab, Enter, Space, Backspace and Delete are sent to the app.",
        "El editor muestra las asignaciones del perfil del juego. Cuando Android tiene el foco, las teclas libres como las flechas, Tab, Intro, espacio, Retroceso y Supr se envían a la aplicación.",
    )).size(13.).color(MUTED));
}
