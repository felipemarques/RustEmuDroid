use super::*;
use std::fs;

#[test]
fn installed_image_uses_the_right_profile_and_preserves_the_existing_avd() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    let base = temp.path().join("state");
    for tool in ["emulator/emulator", "platform-tools/adb"] {
        let file = root.join(format!("{tool}{}", std::env::consts::EXE_SUFFIX));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, []).unwrap();
    }
    for image in [ORIGINAL_ID, GOOGLE_ID] {
        let folder = image_path(&root, image);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("system.img"), []).unwrap();
    }
    let config = LaunchConfig::load(
        Some(temp.path().join("custom-sdk")),
        base.clone(),
        None,
        &base.join("launcher.json"),
    );
    let mut wizard = Wizard::new(&config, &config);
    wizard.destination = root.clone();
    let original = wizard
        .installed_config(&config, &config, &base, ORIGINAL_ID)
        .unwrap();
    assert_eq!(Path::new(&original.state), base);
    assert_eq!(Path::new(&original.image), image_path(&root, ORIGINAL_ID));
    assert!(!base.exists());
    let sdk = Sdk::find(Some(root.clone())).unwrap();
    Profile::new(base.clone())
        .init(super::super::SystemImage::from_sdk(&sdk, ORIGINAL_ID).unwrap())
        .unwrap();
    assert_eq!(
        Path::new(
            &wizard
                .installed_config(&config, &config, &base, ORIGINAL_ID)
                .unwrap()
                .state
        ),
        base.join("portable-original")
    );
    let config = original;
    let mut wizard = Wizard::new(&config, &config);
    wizard.destination = root.clone();
    assert!(
        wizard
            .installed_config(&config, &config, &base, ORIGINAL_ID)
            .unwrap()
            .image
            .is_empty()
    );
    let fallback = base.join("portable-original");
    Profile::new(fallback.clone())
        .init(super::super::SystemImage::from_sdk(&sdk, ORIGINAL_ID).unwrap())
        .unwrap();
    assert_eq!(
        Path::new(
            &wizard
                .installed_config(&config, &config, &base, ORIGINAL_ID)
                .unwrap()
                .state
        ),
        fallback
    );
    wizard.choice = ImageChoice::GooglePlay;
    let play = wizard
        .installed_config(&config, &config, &base, GOOGLE_ID)
        .unwrap();
    assert_eq!(Path::new(&play.state), base.join("google-play-37"));
    assert_eq!(Path::new(&play.image), image_path(&root, GOOGLE_ID));
    let displaced = base.join("google-play-37/avd/RustEmuDroid.avd/config.ini");
    fs::create_dir_all(displaced.parent().unwrap()).unwrap();
    fs::write(&displaced, "custom data").unwrap();
    assert_eq!(
        Path::new(
            &wizard
                .installed_config(&config, &config, &base, GOOGLE_ID)
                .unwrap()
                .state
        ),
        base.join("portable-google-play")
    );
    assert_eq!(fs::read_to_string(displaced).unwrap(), "custom data");
    let mut changed = config.clone();
    changed.sdk = "new SDK override".into();
    assert!(
        wizard
            .installed_config(&changed, &config, &base, GOOGLE_ID)
            .is_err()
    );
    assert!(
        wizard
            .installed_config(&config, &changed, &base, GOOGLE_ID)
            .is_err()
    );
}

#[test]
fn install_terms_require_an_explicit_click_and_remain_usable_in_all_languages() {
    use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
    let ctx = egui::Context::default();
    let root = tempfile::tempdir().unwrap();
    let prefs = root.path().join("launcher.json");
    for (l, terms, accept, install) in [
        (
            Locale::PtBr,
            "Termos do Google",
            "Li e aceito os termos do Google para estes pacotes.",
            "Baixar e instalar",
        ),
        (
            Locale::EnUs,
            "Google license terms",
            "I have read and accept Google's terms for these packages.",
            "Download and install",
        ),
        (
            Locale::EsEs,
            "Condiciones de Google",
            "He leído y acepto las condiciones de Google para estos paquetes.",
            "Descargar e instalar",
        ),
    ] {
        let mut config = LaunchConfig::load(None, root.path().into(), None, &prefs);
        config.locale = l;
        let brand = ctx.load_texture(
            "brand",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            Default::default(),
        );
        let mut app = Launcher::new(brand, config, prefs.clone(), false);
        app.open_runtime_setup();
        let wizard = app.installer.as_mut().unwrap();
        wizard.phase = Phase::Terms(InstallPlan {
            image: ORIGINAL_ID.into(),
            total_bytes: 3 * 1024 * 1024,
            licenses: vec![crate::runtime_install::LicenseTerms {
                id: "fixture".into(),
                text: "Fixture license for layout and consent testing. ".repeat(200),
            }],
            components: vec![],
        });
        wizard.destination = root.path().join("runtime");
        let bounds = Rect::from_min_size(Pos2::ZERO, egui::vec2(610., 510.));
        let render = |app: &mut Launcher, events| {
            ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(bounds),
                    ..Default::default()
                },
                |ctx| app.show_runtime_setup(ctx),
            )
        };
        render(&mut app, vec![]);
        let output = render(&mut app, vec![]);
        let locate = |s: &str| {
            output
                .shapes
                .iter()
                .find_map(|entry| match &entry.shape {
                    egui::Shape::Text(text) if text.galley.text() == s => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing label {s}"))
        };
        let click = |app: &mut Launcher, p| {
            for pressed in [true, false] {
                render(
                    app,
                    vec![
                        Event::PointerMoved(p),
                        Event::PointerButton {
                            pos: p,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        assert!(bounds.contains(locate(terms)));
        click(&mut app, locate(install));
        assert!(matches!(
            app.installer.as_ref().unwrap().phase,
            Phase::Terms(_)
        ));
        assert!(!app.installer.as_ref().unwrap().accepted);
        click(&mut app, locate(accept));
        assert!(app.installer.as_ref().unwrap().accepted);
        assert!(!root.path().join("runtime").exists() && !prefs.exists());
    }
}

#[test]
fn cancel_download_keeps_the_job_alive_until_worker_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let config = LaunchConfig::load(
        None,
        root.path().into(),
        None,
        &root.path().join("launcher.json"),
    );
    let mut wizard = Wizard::new(&config, &config);
    let (tx, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    wizard.phase = Phase::Download {
        events,
        cancel: Arc::clone(&cancel),
        progress: None,
    };
    wizard.cancel();
    wizard.poll();
    assert!(cancel.load(Ordering::Acquire) && wizard.installing());
    tx.send(Message::Finished(Err("cancelled".into()))).unwrap();
    wizard.poll();
    assert!(matches!(wizard.phase, Phase::Choose));
}
