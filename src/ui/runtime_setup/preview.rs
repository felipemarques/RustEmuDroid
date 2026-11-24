use super::*;
use crate::ui::{branded_window, install_window, live_window_test::WindowTest};
use eframe::App;
use winit::platform::windows::EventLoopBuilderExtWindows;

struct Preview {
    app: Launcher,
    root: PathBuf,
    locale: usize,
    capture_after: Instant,
    pending: bool,
}

impl App for Preview {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        raw.focused = true;
        raw.events
            .retain(|event| matches!(event, egui::Event::Screenshot { .. }));
        for event in &raw.events {
            if let egui::Event::Screenshot { image, .. } = event {
                let output = self.root.join(Locale::ALL[self.locale].tag());
                std::fs::create_dir_all(&output).unwrap();
                WindowTest::save(&output, "setup", image);
                self.locale += 1;
                self.capture_after = Instant::now() + Duration::from_millis(400);
                self.pending = false;
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(locale) = Locale::ALL.get(self.locale) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        self.app.config.locale = *locale;
        self.app.installer.as_mut().unwrap().locale = *locale;
        self.app.update(ctx, frame);
        if !self.pending && Instant::now() >= self.capture_after {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.pending = true;
        }
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

#[test]
#[ignore = "renders the installer terms from live Google metadata without downloading packages or starting Android"]
fn capture_localized_runtime_setup() {
    let output =
        std::env::var_os("RUSTEMUDROID_TEST_CAPTURES").expect("select a screenshot directory");
    let plan = runtime_install::plan(ImageChoice::Original, Locale::EnUs).unwrap();
    let destination = runtime_install::default_destination().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let prefs = temp.path().join("launcher.json");
    let config = LaunchConfig::load(None, temp.path().into(), None, &prefs);
    let (mut options, brand) = branded_window().unwrap();
    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "RustEmuDroid",
        options,
        Box::new(move |cc| {
            let mut app = Launcher::new(install_window(cc, brand), config, prefs, false);
            app.open_runtime_setup();
            let wizard = app.installer.as_mut().unwrap();
            wizard.phase = Phase::Terms(plan);
            wizard.destination = destination;
            Ok(Box::new(Preview {
                app,
                root: output.into(),
                locale: 0,
                capture_after: Instant::now() + Duration::from_millis(400),
                pending: false,
            }))
        }),
    )
    .unwrap();
}
