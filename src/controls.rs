use std::{collections::VecDeque, ffi::OsStr, path::PathBuf, process::Command, sync::mpsc, thread};

use eframe::egui;
pub enum Action {
    Louder,
    Quieter,
    ToggleMute,
    Rotate,
    Install(PathBuf),
    AndroidSettings,
    PrepareHome,
    Applications,
}

pub enum Update {
    Notice(String),
    HomeReady,
}

#[derive(Clone)]
struct Guest {
    adb: PathBuf,
    serial: String,
    avd: PathBuf,
}

impl Guest {
    fn prepare_home(&self) -> Result<Update, String> {
        use std::io::Write;
        const PACKAGE: &str = "org.rustemudroid.home";
        const HOME: &str = "org.rustemudroid.home/.HomeActivity";
        let installed = self.command(["shell", "dumpsys", "package", PACKAGE])?;
        if !installed
            .lines()
            .any(|line| line.split_whitespace().next() == Some("versionCode=1"))
        {
            let mut apk = tempfile::Builder::new()
                .suffix(".apk")
                .tempfile()
                .map_err(|e| e.to_string())?;
            apk.write_all(include_bytes!("../android/home/home.apk"))
                .map_err(|e| e.to_string())?;
            let out = self.command([
                OsStr::new("install"),
                OsStr::new("-r"),
                apk.path().as_os_str(),
            ])?;
            if !out.lines().any(|s| s.trim() == "Success") {
                return Err(format!("Instalação da Home falhou: {out}"));
            }
        }
        let resolve = || {
            self.command([
                "shell",
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "--user",
                "current",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.HOME",
            ])
        };
        if !resolve()?.lines().any(|s| s.trim() == HOME) {
            self.command([
                "shell",
                "cmd",
                "package",
                "set-home-activity",
                "--user",
                "current",
                HOME,
            ])?;
            let resolved = resolve()?;
            if !resolved.lines().any(|s| s.trim() == HOME) {
                return Err(format!("O Android selecionou outra Home: {resolved}"));
            }
        }
        let out = self.command([
            "shell",
            "am",
            "start",
            "-W",
            "--user",
            "current",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.HOME",
        ])?;
        if !out.lines().any(|s| s.trim() == "Status: ok") {
            return Err(format!("Falha ao abrir a Home: {out}"));
        }
        Ok(Update::HomeReady)
    }
    fn command(&self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Result<String, String> {
        let mut cmd = Command::new(&self.adb);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW for the adb client.
        }
        let out = cmd
            .args(["-s", &self.serial])
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if out.status.success()
            && !stdout
                .lines()
                .any(|line| line.starts_with("KO:") || line.starts_with("Failure ["))
        {
            Ok(stdout)
        } else {
            Err(
                format!("{stdout} {}", String::from_utf8_lossy(&out.stderr).trim())
                    .trim()
                    .to_owned(),
            )
        }
    }

    fn muted(&self) -> Result<bool, String> {
        match self
            .command(["shell", "cmd", "audio", "is-stream-mute", "3"])
            .as_deref()
        {
            Ok("true") => Ok(true),
            Ok("false") => Ok(false),
            _ => {
                let audio = self.command(["shell", "dumpsys", "audio"])?;
                audio
                    .split_once("- STREAM_MUSIC:")
                    .and_then(|(_, stream)| {
                        stream
                            .lines()
                            .skip(1)
                            .take_while(|s| !s.trim().is_empty())
                            .find_map(|s| s.trim().strip_prefix("Muted: ")?.parse().ok())
                    })
                    .ok_or_else(|| "O Android não informou o estado do áudio de mídia.".to_owned())
            }
        }
    }

    fn perform(&self, action: Action) -> Result<Update, String> {
        let avd = self.command(["emu", "avd", "path"])?;
        if avd
            .lines()
            .next()
            .map(str::trim)
            .map(PathBuf::from)
            .as_ref()
            != Some(&self.avd)
        {
            return Err("A sessão do Android mudou.".into());
        }
        let notice = match action {
            Action::PrepareHome => return self.prepare_home(),
            Action::Applications => {
                self.command([
                    "shell",
                    "am",
                    "start",
                    "--user",
                    "current",
                    "-a",
                    "android.intent.action.MAIN",
                    "-c",
                    "android.intent.category.HOME",
                    "-n",
                    "org.rustemudroid.home/.HomeActivity",
                    "--ez",
                    "apps",
                    "true",
                ])?;
                "Aplicativos".into()
            }
            Action::Louder | Action::Quieter => {
                let (direction, key) = if matches!(action, Action::Louder) {
                    ("RAISE", "KEYCODE_VOLUME_UP")
                } else {
                    ("LOWER", "KEYCODE_VOLUME_DOWN")
                };
                let adjusted =
                    self.command(["shell", "cmd", "audio", "adj-volume", "3", direction]);
                if !adjusted.is_ok_and(|r| r.contains("adjustStreamVolume")) {
                    self.command(["shell", "input", "keyevent", key])?;
                }
                let reply = self.command([
                    "shell",
                    "cmd",
                    "media_session",
                    "volume",
                    "--stream",
                    "3",
                    "--get",
                ])?;
                let level = reply
                    .lines()
                    .find_map(|s| s.split_once("volume is ")?.1.split_whitespace().next());
                level
                    .map(|n| format!("Volume de mídia: {n}"))
                    .ok_or("O Android não confirmou o volume de mídia.")?
            }
            Action::ToggleMute => {
                let was_muted = self.muted()?;
                let cmd = if was_muted { "adj-unmute" } else { "adj-mute" };
                let _ = self.command(["shell", "cmd", "audio", cmd, "3"]);
                if self.muted()? == was_muted {
                    self.command(["shell", "input", "keyevent", "KEYCODE_VOLUME_MUTE"])?;
                    if self.muted()? == was_muted {
                        return Err("O Android não alterou o som de mídia.".into());
                    }
                }
                if was_muted {
                    "Som de mídia ativado"
                } else {
                    "Mídia silenciada"
                }
                .into()
            }
            Action::Rotate => {
                self.command(["emu", "rotate"])?;
                "Rotação solicitada ao Android".into()
            }
            Action::Install(path) => {
                let reply =
                    self.command([OsStr::new("install"), OsStr::new("-r"), path.as_os_str()])?;
                if !reply.lines().any(|line| line.trim() == "Success") {
                    return Err(format!("Falha ao instalar o APK. {reply}"));
                }
                "APK instalado".into()
            }
            Action::AndroidSettings => {
                self.command(["shell", "am", "start", "-a", "android.settings.SETTINGS"])?;
                "Configurações do Android abertas".into()
            }
        };
        Ok(Update::Notice(notice))
    }
}

pub struct Controls {
    guest: Guest,
    pending: Option<mpsc::Receiver<Result<Update, String>>>,
    queue: VecDeque<Action>,
    pub message: Option<Result<String, String>>,
    foreground: Option<String>,
    foreground_query: Option<mpsc::Receiver<Option<String>>>,
    next_foreground: std::time::Instant,
}

impl Controls {
    #[cfg(test)]
    pub(crate) fn guest_output(&self, args: &[&str]) -> Result<String, String> {
        self.guest.command(args.iter().copied())
    }
    pub fn new(adb: PathBuf, port: u16, avd: PathBuf) -> Self {
        Self {
            guest: Guest {
                adb,
                serial: format!("emulator-{port}"),
                avd,
            },
            pending: None,
            queue: VecDeque::new(),
            message: None,
            foreground: None,
            foreground_query: None,
            next_foreground: std::time::Instant::now(),
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn foreground_package(&mut self, ctx: &egui::Context) -> Option<&str> {
        if let Some(query) = &self.foreground_query {
            match query.try_recv() {
                Ok(package) => {
                    self.foreground = package;
                    self.foreground_query = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.foreground = None;
                    self.foreground_query = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let now = std::time::Instant::now();
        if self.foreground_query.is_none() && now >= self.next_foreground {
            let guest = self.guest.clone();
            let ctx = ctx.clone();
            let (tx, rx) = mpsc::channel();
            self.foreground_query = Some(rx);
            self.next_foreground = now + std::time::Duration::from_secs(1);
            thread::spawn(move || {
                let package = guest
                    .command(["shell", "dumpsys", "activity", "top-resumed"])
                    .ok()
                    .and_then(|s| {
                        s.lines().find_map(|line| {
                            line.trim()
                                .strip_prefix("packageName=")?
                                .split_whitespace()
                                .next()
                                .map(str::to_owned)
                        })
                    });
                let _ = tx.send(package);
                ctx.request_repaint();
            });
        }
        self.foreground.as_deref()
    }

    pub fn send(&mut self, action: Action, ctx: &egui::Context) {
        if self.busy() {
            self.queue.push_back(action);
            return;
        }
        self.message = match action {
            Action::Install(_) => Some(Ok("Instalando APK…".into())),
            _ => None,
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let guest = self.guest.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(guest.perform(action));
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Update> {
        if let Some(rx) = &self.pending {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("O controle do Android foi interrompido.".into())
                }
            };
            self.pending = None;
            if let Some(action) = self.queue.pop_front() {
                self.send(action, ctx);
            }
            match result {
                Ok(Update::Notice(text)) => self.message = Some(Ok(text)),
                Ok(update) => return Some(update),
                Err(error) => self.message = Some(Err(error)),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput, pos2, vec2};
    use std::time::{Duration, Instant};

    // Click the actual egui widgets and independently read AudioService on a selected live AVD.
    #[test]
    #[ignore = "requires an explicitly selected test AVD with media volume between its limits"]
    fn audio_buttons_change_android_media_volume() {
        let env = |k| std::env::var(k).unwrap();
        let mut controls = Controls::new(
            env("RUSTEMUDROID_TEST_ADB").into(),
            env("RUSTEMUDROID_TEST_PORT").parse().unwrap(),
            env("RUSTEMUDROID_TEST_AVD").into(),
        );
        let guest = controls.guest.clone();
        let read = |cmd| {
            guest
                .command(["shell", "cmd", "audio", cmd, "3"])
                .unwrap()
                .split_whitespace()
                .last()
                .unwrap()
                .parse::<i32>()
                .unwrap()
        };
        let before = read("get-stream-volume");
        assert!(before > 0 && before < read("get-max-volume") && !guest.muted().unwrap());
        let ctx = egui::Context::default();
        let mut draw = |events| {
            let raw = RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(100., 140.),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.;
                        crate::toolbar::audio_buttons(ui, &mut controls);
                    });
            });
            let until = Instant::now() + Duration::from_secs(10);
            while controls.busy() && Instant::now() < until {
                controls.poll(&ctx);
                thread::sleep(Duration::from_millis(10));
            }
            assert!(!controls.busy());
            if let Some(Err(msg)) = &controls.message {
                panic!("{msg}");
            }
        };
        draw(vec![]);
        let mut click = |row: u8| {
            let pos = pos2(16., 16. + 32. * f32::from(row));
            let event = |pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            draw(vec![Event::PointerMoved(pos), event(true)]);
            draw(vec![event(false)]);
        };
        click(0);
        assert_eq!(read("get-stream-volume"), before + 1);
        click(1);
        assert_eq!(read("get-stream-volume"), before);
        click(2);
        assert!(guest.muted().unwrap());
        click(2);
        assert!(!guest.muted().unwrap());
        assert_eq!(read("get-stream-volume"), before);
    }
}
