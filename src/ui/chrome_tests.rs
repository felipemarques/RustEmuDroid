use super::*;
use crate::toolbar::PLAYER_MODIFIERS;
use eframe::App;
use egui::{
    Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, ViewportCommand, vec2,
};

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        pressed,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn labels(out: &FullOutput, label: &str) -> Vec<Rect> {
    out.shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == label => Some(text.visual_bounding_rect()),
            _ => None,
        })
        .collect()
}

fn commands(out: &FullOutput) -> &[ViewportCommand] {
    &out.viewport_output[&egui::ViewportId::ROOT].commands
}

// The AMD crash occurred only when the process already owned a live OpenGL context.
#[cfg(windows)]
#[test]
#[ignore = "opens a real Windows OpenGL window; set RUSTEMUDROID_TEST_EXE to the UI binary"]
fn settings_gear_with_live_gl_context() {
    use std::{cell::Cell, rc::Rc, time::Instant};
    use winit::platform::windows::EventLoopBuilderExtWindows;

    struct GearTest {
        app: Launcher,
        ticks: usize,
        capture_requested: bool,
        finished: Rc<Cell<bool>>,
        deadline: Instant,
    }
    impl App for GearTest {
        fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
            if let Some(path) = std::env::var_os("RUSTEMUDROID_TEST_PREVIEW")
                && let Some(Event::Screenshot { image, .. }) = raw
                    .events
                    .iter()
                    .find(|e| matches!(e, Event::Screenshot { .. }))
            {
                use std::io::Write;
                let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
                write!(out, "P6\n{} {}\n255\n", image.width(), image.height()).unwrap();
                for p in &image.pixels {
                    out.write_all(&[p.r(), p.g(), p.b()]).unwrap();
                }
                self.finished.set(true);
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            if matches!(self.ticks, 2 | 3) {
                let p = egui::pos2(ctx.content_rect().right() - 168., 20.);
                raw.events
                    .extend([Event::PointerMoved(p), button(p, self.ticks == 2)]);
            }
        }
        fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            self.app.update(ctx, frame);
            self.ticks += 1;
            // Keep swapping buffers with the modal open after the probe has exited.
            if self.app.settings.is_some()
                && !self.app.adapters.names.is_empty()
                && self.ticks >= 60
                && !self.finished.get()
            {
                if std::env::var_os("RUSTEMUDROID_TEST_PREVIEW").is_some() {
                    if !self.capture_requested {
                        self.capture_requested = true;
                        ctx.send_viewport_cmd(ViewportCommand::Screenshot(
                            egui::UserData::default(),
                        ));
                    }
                } else {
                    self.finished.set(true);
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            if Instant::now() > self.deadline {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    let done = Rc::new(Cell::new(false));
    let finished = done.clone();
    let temp = tempfile::tempdir().unwrap();
    let prefs = temp.path().join("launcher.json");
    std::fs::create_dir_all(temp.path().join("avd")).unwrap();
    std::fs::write(
        temp.path().join("avd/RustEmuDroid.ini"),
        "target=android-36\n",
    )
    .unwrap();
    let config = LaunchConfig::load(None, temp.path().into(), None, &prefs);
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080., 680.])
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Teste da engrenagem",
        options,
        Box::new(move |cc| {
            assert!(cc.gl.is_some());
            theme::install(&cc.egui_ctx);
            let icon =
                eframe::icon_data::from_png_bytes(include_bytes!("../../assets/rustemudroid.png"))
                    .unwrap();
            let texture = cc.egui_ctx.load_texture(
                "brand",
                egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                ),
                egui::TextureOptions::LINEAR,
            );
            Ok(Box::new(GearTest {
                app: Launcher::new(texture, config, prefs, false),
                ticks: 0,
                capture_requested: false,
                finished,
                deadline: Instant::now() + Duration::from_secs(15),
            }))
        }),
    )
    .unwrap();
    assert!(
        done.get(),
        "a engrenagem deveria abrir as configurações e listar as placas"
    );
}

#[test]
#[ignore = "boots a selected idle AVD and verifies its Android Home"]
fn cold_start_selects_android_home() {
    use crate::android::Camera;
    let state = PathBuf::from(std::env::var_os("RUSTEMUDROID_TEST_STATE").unwrap());
    let ctx = egui::Context::default();
    let brand = ctx.load_texture(
        "brand",
        egui::ColorImage::filled([2, 2], INK),
        Default::default(),
    );
    let prefs = state.join("launcher.json");
    let config = LaunchConfig::load(None, state, None, &prefs);
    let mut app = Launcher::new(brand, config, prefs, true);
    let mut frame = eframe::Frame::_new_kittest();
    let until = std::time::Instant::now() + Duration::from_secs(200);
    while !app.home_ready && std::time::Instant::now() < until {
        let _ = ctx.run(RawInput::default(), |ctx| app.update(ctx, &mut frame));
        assert!(!app.failed, "{}", app.status);
        if let Some(Err(error)) = app.controls.as_ref().and_then(|c| c.message.as_ref()) {
            panic!("{error}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(app.home_ready, "{}", app.status);
    let controls = app.controls.as_ref().unwrap();
    let resolved = controls
        .guest_output(&[
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.HOME",
        ])
        .unwrap();
    assert!(resolved.contains("org.rustemudroid.home/.HomeActivity"));
    if app.config.hardware.camera == Camera::EmulatedFront {
        let camera = controls
            .guest_output(&["shell", "dumpsys", "media.camera"])
            .unwrap();
        assert!(
            camera
                .lines()
                .any(|s| s.trim().eq_ignore_ascii_case("Facing: Front")),
            "câmera frontal simulada ausente"
        );
    }
}

#[test]
fn title_bar_layout_and_window_actions() {
    for (playing, width) in [(false, 610.), (true, 610.), (true, 1080.)] {
        let ctx = egui::Context::default();
        let brand = ctx.load_texture(
            "brand",
            egui::ColorImage::filled([2, 2], INK),
            Default::default(),
        );
        let brand_id = brand.id();
        let config = LaunchConfig {
            sdk: String::new(),
            state: String::new(),
            image: String::new(),
            gpu: Gpu::Auto,
            hardware: Default::default(),
            amd_uma_vulkan_by_profile: Default::default(),
            vulkan_cache_runtime_by_profile: Default::default(),
            performance_engine_by_profile: Default::default(),
            performance_power_by_profile: Default::default(),
            show_fps: true,
            android_fps: false,
            adaptive_video: false,
            panel: Default::default(),
            animate_mascot: true,
            locale: Locale::default(),
        };
        let mut app = Launcher::new(brand, config, Default::default(), false);
        app.playing = playing;
        let mut frame = eframe::Frame::_new_kittest();
        let mut tick = |events| {
            ctx.run(
                RawInput {
                    events,
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 510.))),
                    ..Default::default()
                },
                |ctx| app.update(ctx, &mut frame),
            )
        };
        tick(vec![]);
        let out = tick(vec![]);
        let name = labels(&out, "RustEmuDroid");
        let version = labels(&out, concat!("v", env!("CARGO_PKG_VERSION")));
        assert_eq!((name.len(), version.len()), (1, 1));
        assert!(name[0].bottom() <= version[0].top() + 1. && version[0].bottom() <= 40.);
        assert_eq!(
            out.shapes
                .iter()
                .filter(|s| s.shape.texture_id() == brand_id
                    && s.shape.visual_bounding_rect().bottom() <= 40.)
                .count(),
            1
        );
        let help = labels(&out, "?")[0].center();
        tick(vec![Event::PointerMoved(help), button(help, true)]);
        tick(vec![button(help, false)]);
        let dialog = tick(vec![]);
        assert_eq!(labels(&dialog, &version_label(Locale::default())).len(), 1);
        let close = labels(&dialog, "Fechar")[0].center();
        tick(vec![Event::PointerMoved(close), button(close, true)]);
        tick(vec![button(close, false)]);
        assert!(labels(&tick(vec![]), &version_label(Locale::default())).is_empty());
        for (offset, want) in [
            (21., ViewportCommand::Close),
            (55., ViewportCommand::Maximized(true)),
            (89., ViewportCommand::Minimized(true)),
        ] {
            let pos = egui::pos2(width - offset, 20.);
            tick(vec![Event::PointerMoved(pos), button(pos, true)]);
            let out = tick(vec![button(pos, false)]);
            assert!(
                commands(&out).contains(&want),
                "button at {pos:?}: {:?}",
                commands(&out)
            );
        }
        let pos = name[0].center();
        tick(vec![Event::PointerMoved(pos), button(pos, true)]);
        let moved = pos + vec2(20., 0.);
        assert!(
            commands(&tick(vec![Event::PointerMoved(moved)])).contains(&ViewportCommand::StartDrag)
        );
        tick(vec![button(moved, false)]);
        // Let the previous caption and window gestures expire before a new double click.
        for _ in 0..60 {
            tick(vec![]);
        }
        tick(vec![Event::PointerMoved(pos), button(pos, true)]);
        tick(vec![button(pos, false)]);
        tick(vec![button(pos, true)]);
        assert!(
            commands(&tick(vec![button(pos, false)])).contains(&ViewportCommand::Maximized(true))
        );
        let corner = egui::pos2(width - 1., 509.);
        let out = tick(vec![Event::PointerMoved(corner), button(corner, true)]);
        assert!(commands(&out).contains(&ViewportCommand::BeginResize(
            egui::viewport::ResizeDirection::SouthEast
        )));
    }
}

#[test]
fn settings_combo_and_footer_at_minimum_window_size() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let dir = tempfile::tempdir().unwrap();
    let prefs = dir.path().join("launcher.json");
    let config = LaunchConfig::load(None, dir.path().into(), Some(Gpu::Host), &prefs);
    let brand = ctx.load_texture(
        "brand",
        egui::ColorImage::filled([2, 2], INK),
        Default::default(),
    );
    let mut app = Launcher::new(brand, config.clone(), prefs, false);
    app.settings = Some(config);
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(610., 510.));
    let mut tick = |events| {
        ctx.run(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| app.show_settings(ctx, None),
        )
    };
    for _ in 0..3 {
        tick(vec![]);
    }
    let out = tick(vec![]);
    let select = labels(&out, "GPU do computador")[0].center();
    let cancel = labels(&out, "Cancelar")[0].center();
    assert!(screen.contains(select) && screen.contains(cancel));
    tick(vec![Event::PointerMoved(select), button(select, true)]);
    tick(vec![button(select, false)]);
    let software = labels(&tick(vec![]), "Software")[0].center();
    tick(vec![Event::PointerMoved(software), button(software, true)]);
    tick(vec![button(software, false)]);
    assert_eq!(labels(&tick(vec![]), "Software").len(), 1);
    tick(vec![Event::PointerMoved(cancel), button(cancel, true)]);
    tick(vec![button(cancel, false)]);
    assert!(app.settings.is_none());
    assert!(matches!(app.config.gpu, Gpu::Host));
    assert!(!app.prefs_file.is_file());
}

#[test]
fn settings_save_android_surface_fps_without_restarting_play() {
    for locale in Locale::ALL {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let mut config = LaunchConfig::load(None, dir.path().into(), Some(Gpu::Host), &prefs);
        config.locale = locale;
        let brand = ctx.load_texture(
            "brand",
            egui::ColorImage::filled([2, 2], INK),
            Default::default(),
        );
        let mut app = Launcher::new(brand, config.clone(), prefs.clone(), false);
        app.playing = true;
        app.stage = BootStage::Booted;
        app.settings = Some(config);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1080., 680.));
        let mut tick = |events| {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| app.show_settings(ctx, None),
            )
        };
        for _ in 0..3 {
            tick(vec![]);
        }
        let default = locale.pick("Painel Rust", "Rust panel", "Panel Rust");
        let android = locale.pick(
            "Superfície do jogo no Android",
            "Android game surface",
            "Superficie del juego en Android",
        );
        let save = locale.pick("Salvar", "Save", "Guardar");
        let seek = |out: &FullOutput, text: &str| {
            let targets = labels(out, text);
            assert_eq!(targets.len(), 1, "{text} for {}", locale.tag());
            let point = targets[0].center();
            assert!(
                screen.contains(point),
                "{text} is outside the settings window"
            );
            point
        };
        let adapt = seek(
            &tick(vec![]),
            locale.pick(
                "Ajustar o vídeo ao tamanho da janela",
                "Fit video frames to the window",
                "Ajustar el vídeo al tamaño de la ventana",
            ),
        );
        tick(vec![Event::PointerMoved(adapt), button(adapt, true)]);
        tick(vec![button(adapt, false)]);
        let combo = seek(&tick(vec![]), default);
        tick(vec![Event::PointerMoved(combo), button(combo, true)]);
        tick(vec![button(combo, false)]);
        let option = seek(&tick(vec![]), android);
        tick(vec![Event::PointerMoved(option), button(option, true)]);
        tick(vec![button(option, false)]);
        let selected = tick(vec![]);
        seek(&selected, android);
        let save = seek(&selected, save);
        tick(vec![Event::PointerMoved(save), button(save, true)]);
        tick(vec![button(save, false)]);
        assert!(app.settings.is_none() && app.config.android_fps && app.config.show_fps);
        assert!(!app.restart && !app.stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(app.task.is_none());
        let saved = LaunchConfig::load(None, dir.path().into(), None, &prefs);
        assert!(saved.android_fps && saved.show_fps);
        assert!(saved.adaptive_video);
    }
}

#[cfg(windows)]
#[test]
fn settings_restore_default_vulkan_runtime_for_the_running_profile() {
    for locale in Locale::ALL {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let profile = dir.path().join("android-17");
        let mut config = LaunchConfig::load(None, profile.clone(), Some(Gpu::Host), &prefs);
        config.locale = locale;
        config.set_vulkan_cache_runtime(Some(dir.path().join("prepared").join("sdk")));
        let brand = ctx.load_texture(
            "brand",
            egui::ColorImage::filled([2, 2], INK),
            Default::default(),
        );
        let mut app = Launcher::new(brand, config.clone(), prefs.clone(), false);
        app.playing = true;
        app.stage = BootStage::Booted;
        app.settings = Some(config);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1080., 1200.));
        let mut tick = |events| {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| app.show_settings(ctx, None),
            )
        };
        let locate = |out: &FullOutput, text: &str| {
            let targets = labels(out, text);
            assert_eq!(
                targets.len(),
                1,
                "{text} for {}: {:?}",
                locale.tag(),
                out.shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            );
            let point = targets[0].center();
            assert!(screen.contains(point), "{text} is outside the window");
            point
        };
        for _ in 0..3 {
            tick(vec![]);
        }
        let inside_scroll_area = locate(
            &tick(vec![]),
            locale.pick("Painel Rust", "Rust panel", "Panel Rust"),
        );
        let scroll_down = |point| {
            vec![
                Event::PointerMoved(point),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., -650.),
                    modifiers: Modifiers::NONE,
                },
            ]
        };
        tick(scroll_down(inside_scroll_area));
        for _ in 0..15 {
            tick(vec![]);
        }
        let section = locate(
            &tick(vec![]),
            locale.pick(
                "Runtime, imagem e perfil",
                "Runtime, image, and profile",
                "Entorno, imagen y perfil",
            ),
        );
        tick(vec![Event::PointerMoved(section), button(section, true)]);
        tick(vec![button(section, false)]);
        for _ in 0..15 {
            tick(vec![]);
        }
        tick(scroll_down(inside_scroll_area));
        for _ in 0..15 {
            tick(vec![]);
        }
        let opened = tick(vec![]);
        locate(
            &opened,
            locale.pick(
                "Escolher pasta do motor…",
                "Choose runtime folder…",
                "Elegir carpeta del motor…",
            ),
        );
        let use_default = locate(
            &opened,
            locale.pick(
                "Voltar ao motor padrão",
                "Use default engine",
                "Volver al motor predeterminado",
            ),
        );
        tick(vec![
            Event::PointerMoved(use_default),
            button(use_default, true),
        ]);
        tick(vec![button(use_default, false)]);
        let updated = tick(vec![]);
        locate(
            &updated,
            locale.pick("Motor padrão", "Default engine", "Motor predeterminado"),
        );
        let save = locate(
            &updated,
            locale.pick(
                "Salvar e reiniciar Android",
                "Save and restart Android",
                "Guardar y reiniciar Android",
            ),
        );
        tick(vec![Event::PointerMoved(save), button(save, true)]);
        tick(vec![button(save, false)]);
        assert!(app.settings.is_none());
        assert!(app.restart && app.stop.load(std::sync::atomic::Ordering::Acquire));
        let saved = LaunchConfig::load(None, profile.clone(), None, &prefs);
        assert_eq!(saved.state, profile.display().to_string());
        assert!(saved.vulkan_cache_runtime().is_none());
    }
}

#[cfg(windows)]
#[test]
fn settings_offer_performance_options_listed_by_the_engine_manifest() {
    for locale in Locale::ALL {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let profile = dir.path().join("android-17");
        let engine = dir.path().join("perf-engine");
        std::fs::create_dir_all(&engine).unwrap();
        let hash = "0".repeat(64);
        std::fs::write(
            engine.join("rustemudroid-engine.json"),
            serde_json::json!({
                "schema": 1,
                "kind": "rustemudroid-performance-engine",
                "engine_version": "37.3.10",
                "build_id": "test",
                "emulator_sha256": hash,
                "qemu_sha256": hash,
                "gfxstream_sha256": hash,
                // No vcpu_placement: that option must not be offered.
                "capabilities": ["tsc_sync", "vulkan_cached_host", "asg_spin",
                    "vulkan_native_swapchain", "guest_kernel_parameters"],
            })
            .to_string(),
        )
        .unwrap();
        let mut config = LaunchConfig::load(None, profile.clone(), Some(Gpu::Host), &prefs);
        config.locale = locale;
        config.set_performance_engine(Some(engine.join("sdk")));
        let brand = ctx.load_texture(
            "brand",
            egui::ColorImage::filled([2, 2], INK),
            Default::default(),
        );
        let mut app = Launcher::new(brand, config.clone(), prefs.clone(), false);
        app.settings = Some(config);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1080., 1200.));
        let mut tick = |events| {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| app.show_settings(ctx, None),
            )
        };
        let texts = |out: &FullOutput| {
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let locate = |out: &FullOutput, text: &str| {
            let targets = labels(out, text);
            assert_eq!(
                targets.len(),
                1,
                "{text} for {}: {:?}",
                locale.tag(),
                texts(out)
            );
            let point = targets[0].center();
            assert!(screen.contains(point), "{text} is outside the window");
            point
        };
        for _ in 0..3 {
            tick(vec![]);
        }
        let inside_scroll_area = locate(
            &tick(vec![]),
            locale.pick("Painel Rust", "Rust panel", "Panel Rust"),
        );
        let scroll_down = |point| {
            vec![
                Event::PointerMoved(point),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., -650.),
                    modifiers: Modifiers::NONE,
                },
            ]
        };
        tick(scroll_down(inside_scroll_area));
        for _ in 0..15 {
            tick(vec![]);
        }
        let section = locate(
            &tick(vec![]),
            locale.pick(
                "Runtime, imagem e perfil",
                "Runtime, image, and profile",
                "Entorno, imagen y perfil",
            ),
        );
        tick(vec![Event::PointerMoved(section), button(section, true)]);
        tick(vec![button(section, false)]);
        for _ in 0..3 {
            for _ in 0..15 {
                tick(vec![]);
            }
            tick(scroll_down(inside_scroll_area));
        }
        for _ in 0..15 {
            tick(vec![]);
        }
        let opened = tick(vec![]);
        let shown = texts(&opened);
        assert!(
            shown
                .iter()
                .all(|text| !text.contains("VulkanNativeSwapchain")
                    && !text.contains("RUSTEMUDROID_QA_")),
            "technical names stay in the hover text: {shown:?}"
        );
        assert!(
            labels(
                &opened,
                locale.pick(
                    "Núcleos dedicados para o Android",
                    "Dedicated cores for Android",
                    "Núcleos dedicados para Android",
                )
            )
            .is_empty()
        );
        locate(
            &opened,
            locale.pick(
                "Compor a tela com Vulkan",
                "Compose the screen with Vulkan",
                "Componer la pantalla con Vulkan",
            ),
        );
        let kernel = locate(
            &opened,
            locale.pick(
                "Reduzir proteções do kernel do Android",
                "Reduce Android kernel protections",
                "Reducir las protecciones del kernel de Android",
            ),
        );
        assert!(shown.iter().any(|text| text.starts_with(locale.pick(
            "Atenção:",
            "Warning:",
            "Atención:"
        ))));
        tick(vec![Event::PointerMoved(kernel), button(kernel, true)]);
        tick(vec![button(kernel, false)]);
        let save = locate(&tick(vec![]), locale.pick("Salvar", "Save", "Guardar"));
        tick(vec![Event::PointerMoved(save), button(save, true)]);
        tick(vec![button(save, false)]);
        assert!(app.settings.is_none());
        let saved = LaunchConfig::load(None, profile.clone(), None, &prefs);
        let selected = saved.performance_engine().unwrap();
        assert_eq!(selected.path, engine.join("sdk"));
        assert!(selected.options.guest_kernel_parameters, "{}", locale.tag());
        assert!(selected.options.clock_sync && selected.options.dedicated_vcpu_cores);
    }
}

#[test]
fn idle_panel_localizes_session_controls_and_keeps_diagnostics_collapsed() {
    for (locale, start, settings, status, details) in [
        (
            Locale::PtBr,
            "Iniciar Android",
            "Configurações",
            "Sessão encerrada",
            "Detalhes técnicos",
        ),
        (
            Locale::EnUs,
            "Start Android",
            "Settings",
            "Session ended",
            "Technical details",
        ),
        (
            Locale::EsEs,
            "Iniciar Android",
            "Ajustes",
            "Sesión finalizada",
            "Detalles técnicos",
        ),
    ] {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let config = LaunchConfig::load(None, dir.path().into(), None, &prefs);
        assert_eq!(config.locale, Locale::PtBr);
        let mut legacy = serde_json::to_value(&config).unwrap();
        legacy.as_object_mut().unwrap().remove("locale");
        write_json(&prefs, &legacy).unwrap();
        let mut config = LaunchConfig::load(None, dir.path().into(), None, &prefs);
        assert_eq!(config.locale, Locale::PtBr);
        config.locale = locale;
        write_json(&prefs, &config).unwrap();
        let config = LaunchConfig::load(None, dir.path().into(), None, &prefs);
        assert_eq!(config.locale, locale);
        assert_eq!(serde_json::to_value(locale).unwrap(), locale.tag());
        let brand = ctx.load_texture(
            "brand",
            egui::ColorImage::filled([2, 2], INK),
            Default::default(),
        );
        let mut app = Launcher::new(brand, config, prefs, false);
        app.stage = BootStage::Stopped;
        app.output.push_back("sys.boot_completed=1".into());
        let mut frame = eframe::Frame::_new_kittest();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(610., 510.));
        let mut tick = || {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| app.update(ctx, &mut frame),
            )
        };
        tick();
        let out = tick();
        for expected in [start, settings, status, details] {
            let rects = labels(&out, expected);
            assert_eq!(rects.len(), 1, "{locale:?} {expected}");
            assert!(
                screen.contains_rect(rects[0]),
                "{locale:?} {expected}: {:?}",
                rects[0]
            );
        }
        assert!(labels(&out, "sys.boot_completed=1").is_empty());
    }
}

#[test]
fn idle_panel_shows_the_failure_reason_without_expanding_details() {
    let reason = r"No permission to read D:\engines\stage (Access is denied. (os error 5)).";
    let long = format!("{reason} {}", "More context. ".repeat(80));
    for (locale, failed) in [
        (Locale::PtBr, "A última operação falhou"),
        (Locale::EnUs, "The last operation failed"),
        (Locale::EsEs, "La última operación ha fallado"),
    ] {
        for text in [reason.to_owned(), long.clone()] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let dir = tempfile::tempdir().unwrap();
            let prefs = dir.path().join("launcher.json");
            let mut config = LaunchConfig::load(None, dir.path().into(), None, &prefs);
            config.locale = locale;
            let brand = ctx.load_texture(
                "brand",
                egui::ColorImage::filled([2, 2], INK),
                Default::default(),
            );
            let mut app = Launcher::new(brand, config, prefs, false);
            app.failed = true;
            app.status = text.clone();
            let mut frame = eframe::Frame::_new_kittest();
            let screen = Rect::from_min_size(Pos2::ZERO, vec2(610., 510.));
            let mut tick = || {
                ctx.run(
                    RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ctx| app.update(ctx, &mut frame),
                )
            };
            tick();
            let out = tick();
            assert_eq!(labels(&out, failed).len(), 1, "{locale:?}");
            // Shown once, on screen, while the details section stays collapsed.
            let shown: Vec<_> = out
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Text(shape) if shape.galley.text().starts_with(reason) => Some((
                        shape.visual_bounding_rect(),
                        shape.galley.rows.len(),
                        shape.galley.elided,
                    )),
                    _ => None,
                })
                .collect();
            assert_eq!(shown.len(), 1, "{locale:?}");
            let (rect, rows, elided) = shown[0];
            assert!(rows <= 6, "{locale:?}: {rows}");
            assert_eq!(elided, text.len() > reason.len(), "{locale:?}");
            assert!(screen.contains_rect(rect), "{locale:?}: {rect:?}");
        }
    }
}

#[test]
fn capture_shortcut_consumes_activation_and_repeat_before_android_input() {
    let ctx = egui::Context::default();
    let send = |key, pressed, modifiers| Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    };
    let out = ctx.run(
        RawInput {
            events: vec![
                send(egui::Key::R, true, PLAYER_MODIFIERS),
                send(egui::Key::R, true, PLAYER_MODIFIERS),
                send(egui::Key::R, false, PLAYER_MODIFIERS),
                send(egui::Key::J, true, egui::Modifiers::NONE),
            ],
            ..Default::default()
        },
        |ctx| {
            assert!(consume_player_shortcut(ctx, egui::Key::R, true));
            assert!(!consume_player_shortcut(ctx, egui::Key::R, true));
            assert!(!consume_player_shortcut(ctx, egui::Key::J, true));
            assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                Event::Key {
                    key: egui::Key::J,
                    ..
                }
            ))));
        },
    );
    assert!(commands(&out).is_empty());
}

#[test]
fn replay_shortcut_consumes_the_chord_and_preserves_plain_h_and_forward_jxl() {
    use crate::toolbar::PLAYER_SHORTCUTS;

    assert!(PLAYER_SHORTCUTS.contains(&(egui::Key::H, Tool::SaveReplay)));
    assert!(PLAYER_SHORTCUTS.contains(&(egui::Key::J, Tool::AnimatedClip)));
    let ctx = egui::Context::default();
    let send = |key, pressed, modifiers| Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    };
    let plain_h = send(egui::Key::H, true, egui::Modifiers::NONE);
    let plain_h_release = send(egui::Key::H, false, PLAYER_MODIFIERS);
    let _ = ctx.run(
        RawInput {
            events: vec![
                send(egui::Key::H, true, PLAYER_MODIFIERS),
                send(egui::Key::H, true, PLAYER_MODIFIERS),
                send(egui::Key::H, false, egui::Modifiers::NONE),
                send(egui::Key::J, true, PLAYER_MODIFIERS),
                plain_h.clone(),
                plain_h_release.clone(),
            ],
            ..Default::default()
        },
        |ctx| {
            assert!(consume_player_shortcut(ctx, egui::Key::H, true));
            assert!(!consume_player_shortcut(ctx, egui::Key::H, true));
            assert!(consume_player_shortcut(ctx, egui::Key::J, true));
            ctx.input(|i| assert_eq!(i.events, [plain_h.clone(), plain_h_release.clone()]));
        },
    );
}

#[test]
fn replay_menu_shows_localized_opt_in_and_session_stop_disarms() {
    let ctx = egui::Context::default();
    let dir = tempfile::tempdir().unwrap();
    let prefs = dir.path().join("launcher.json");
    let brand = ctx.load_texture(
        "brand",
        egui::ColorImage::filled([2, 2], INK),
        Default::default(),
    );
    for (locale, off, enable) in [
        (Locale::PtBr, "Replay desativado", "Ativar buffer de replay"),
        (Locale::EnUs, "Replay off", "Enable replay buffer"),
        (
            Locale::EsEs,
            "Repetición desactivada",
            "Activar el búfer de repetición",
        ),
    ] {
        let mut config = LaunchConfig::load(None, dir.path().into(), None, &prefs);
        config.locale = locale;
        let mut app = Launcher::new(brand.clone(), config, prefs.clone(), false);
        assert!(!app.capture.replay_armed());
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(610., 510.));
        let mut render = || {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    egui::Popup::open_id(ctx, replay_popup_id());
                    egui::SidePanel::right("android-tools").show(ctx, |ui| {
                        assert!(!app.show_capture_tools(ui, true));
                    });
                },
            )
        };
        render();
        let out = render();
        for text in [off, enable, &Tool::SaveReplay.label_with_shortcut(locale)] {
            let rects = labels(&out, text);
            assert_eq!(rects.len(), 1, "{locale:?} {text}");
            assert!(screen.contains_rect(rects[0]), "{locale:?} {text}");
        }
        app.set_replay_armed(true);
        assert!(app.capture.replay_armed());
        app.request_stop();
        assert!(!app.capture.replay_armed());
        assert!(!app.capture.replay_ready());
    }
}

#[test]
fn closing_waits_for_vm_without_blocking_frames_or_restarting() {
    let ctx = egui::Context::default();
    let brand = ctx.load_texture(
        "brand",
        egui::ColorImage::filled([2, 2], INK),
        Default::default(),
    );
    let dir = tempfile::tempdir().unwrap();
    let prefs = dir.path().join("launcher.json");
    let config = LaunchConfig::load(None, dir.path().into(), None, &prefs);
    let mut app = Launcher::new(brand, config, prefs, true);
    let mut frame = eframe::Frame::_new_kittest();
    let tick = |app: &mut Launcher, frame: &mut eframe::Frame, close| {
        let mut input = RawInput::default();
        if close {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        }
        ctx.run(input, |ctx| app.update(ctx, frame))
    };

    // A native close arriving before the first paint must not auto-start Android.
    let out = tick(&mut app, &mut frame, true);
    assert!(!commands(&out).contains(&ViewportCommand::CancelClose));
    assert!(app.task.is_none() && !app.play);

    app.closing = false;
    app.stop = Arc::new(AtomicBool::new(false));
    app.restart = true;
    app.playing = true;
    let (events, receiver) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    app.task = Some(Session {
        events: receiver,
        worker: thread::spawn(move || {
            let _ = gate.recv();
            Ok(())
        }),
    });
    // Late boot notifications must not reopen controls while stopping.
    events
        .send(BootEvent::Display {
            console: 5554,
            rpc: 0,
            avd: dir.path().into(),
            adb: Default::default(),
            engine_window: None,
        })
        .unwrap();

    for close in [true, false, true] {
        let out = tick(&mut app, &mut frame, close);
        assert!(app.stop.load(Ordering::Acquire));
        assert!(app.controls.is_none() && app.display.is_none());
        assert!(!app.restart);
        assert_eq!(labels(&out, "Desligando Android…").len(), 1);
        assert_eq!(
            commands(&out).contains(&ViewportCommand::CancelClose),
            close
        );
        assert!(!commands(&out).contains(&ViewportCommand::Close));
    }
    release.send(()).unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let out = tick(&mut app, &mut frame, false);
        if commands(&out).contains(&ViewportCommand::Close) {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "a janela não fechou após a saída da VM"
        );
        thread::yield_now();
    }
    assert!(app.task.is_none() && !app.play);
    assert!(!commands(&tick(&mut app, &mut frame, true)).contains(&ViewportCommand::CancelClose));
}
