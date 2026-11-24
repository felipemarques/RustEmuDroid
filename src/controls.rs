use std::{collections::VecDeque, ffi::OsStr, path::PathBuf, process::Command, sync::mpsc, thread};

use eframe::egui;

use crate::ui::i18n::Locale;

const HOME_PACKAGE: &str = "org.rustemudroid.home";
const HOME_ACTIVITY: &str = "org.rustemudroid.home/.HomeActivity";
const HOME_APK_VERSION: u32 = 3;

pub enum Action {
    Louder,
    Quieter,
    ToggleMute,
    Rotate,
    Install(PathBuf),
    AndroidSettings,
    PrepareHome(Locale),
    SetHomeLocale(Locale),
    Applications,
}

pub enum Update {
    Notice(String),
    HomeReady,
    HomeLocaleUpdated,
}

#[derive(Clone)]
struct Guest {
    adb: PathBuf,
    serial: String,
    avd: PathBuf,
    locale: Locale,
}

impl Guest {
    fn home_apk_available(&self) -> bool {
        self.command([
            "shell",
            "cmd",
            "package",
            "path",
            "--user",
            "current",
            HOME_PACKAGE,
        ])
        .is_ok_and(|reply| reply.lines().any(|line| line.starts_with("package:")))
    }

    fn prepare_home(&self, locale: Locale) -> Result<Update, String> {
        use std::io::Write;
        let installed = self.command(["shell", "dumpsys", "package", HOME_PACKAGE])?;
        let version = installed.lines().find_map(|line| {
            line.split_whitespace()
                .next()?
                .strip_prefix("versionCode=")?
                .parse::<u32>()
                .ok()
        });
        // Package settings can outlive an APK the boot-time scanner rejected.
        let reinstall =
            version.unwrap_or_default() < HOME_APK_VERSION || !self.home_apk_available();
        if reinstall {
            let mut apk = tempfile::Builder::new()
                .suffix(".apk")
                .tempfile()
                .map_err(|e| e.to_string())?;
            apk.write_all(include_bytes!("../android/home/home.apk"))
                .map_err(|e| e.to_string())?;
            let message = self.locale.pick(
                "Falha ao instalar a tela inicial.",
                "Could not install the home screen.",
                "No se pudo instalar la pantalla de inicio.",
            );
            let out = self
                .command([
                    OsStr::new("install"),
                    OsStr::new("-r"),
                    OsStr::new("--no-streaming"),
                    apk.path().as_os_str(),
                ])
                .map_err(|detail| format!("{message} {detail}"))?;
            if !out.lines().any(|s| s.trim() == "Success") || !self.home_apk_available() {
                return Err(format!("{message} {out}"));
            }
        }
        self.set_home_locale(locale)?;
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
        if !resolve()?.lines().any(|s| s.trim() == HOME_ACTIVITY) {
            self.command([
                "shell",
                "cmd",
                "package",
                "set-home-activity",
                "--user",
                "current",
                HOME_ACTIVITY,
            ])?;
            let resolved = resolve()?;
            if !resolved.lines().any(|s| s.trim() == HOME_ACTIVITY) {
                let message = self.locale.pick(
                    "O Android selecionou outra tela inicial.",
                    "Android selected a different home screen.",
                    "Android ha seleccionado otra pantalla de inicio.",
                );
                return Err(format!("{message} {resolved}"));
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
            "-n",
            HOME_ACTIVITY,
        ])?;
        if !out.lines().any(|s| s.trim() == "Status: ok") {
            let message = self.locale.pick(
                "Falha ao abrir a tela inicial.",
                "Could not open the home screen.",
                "No se pudo abrir la pantalla de inicio.",
            );
            return Err(format!("{message} {out}"));
        }
        if reinstall {
            let message = self.locale.pick(
                "Falha ao gravar a tela inicial no disco.",
                "Could not flush home screen changes to disk.",
                "No se pudieron guardar en el disco los cambios de la pantalla de inicio.",
            );
            self.command(["shell", "sync"])
                .map_err(|detail| format!("{message} {detail}"))?;
        }
        Ok(Update::HomeReady)
    }

    fn set_home_locale(&self, locale: Locale) -> Result<Update, String> {
        let message = self.locale.pick(
            "O Android não confirmou o idioma da tela inicial.",
            "Android did not confirm the home screen language.",
            "Android no ha confirmado el idioma de la pantalla de inicio.",
        );
        let reply = self
            .command([
                "shell",
                "am",
                "broadcast",
                "--user",
                "current",
                "-n",
                "org.rustemudroid.home/.HomeLocaleReceiver",
                "-a",
                "org.rustemudroid.home.SET_LOCALE",
                "--es",
                "locale",
                locale.tag(),
            ])
            .map_err(|detail| format!("{message} {detail}"))?;
        let confirmed = reply.lines().any(|line| {
            line.strip_prefix("Broadcast completed: result=")
                .and_then(|value| value.split([',', ' ']).next())
                == Some("-1")
        });
        if !confirmed {
            return Err(format!("{message} {reply}"));
        }
        Ok(Update::HomeLocaleUpdated)
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
                    .ok_or_else(|| {
                        self.locale
                            .pick(
                                "O Android não informou o estado do áudio de mídia.",
                                "Android did not report the media audio state.",
                                "Android no ha comunicado el estado del sonido multimedia.",
                            )
                            .to_owned()
                    })
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
            return Err(self
                .locale
                .pick(
                    "A sessão do Android mudou.",
                    "The Android session changed.",
                    "La sesión de Android ha cambiado.",
                )
                .into());
        }
        let notice = match action {
            Action::PrepareHome(locale) => return self.prepare_home(locale),
            Action::SetHomeLocale(locale) => return self.set_home_locale(locale),
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
                self.locale
                    .pick("Aplicativos", "Apps", "Aplicaciones")
                    .into()
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
                    .map(|n| {
                        format!(
                            "{}{n}",
                            self.locale.pick(
                                "Volume de mídia ",
                                "Media volume ",
                                "Volumen multimedia ",
                            )
                        )
                    })
                    .ok_or_else(|| {
                        self.locale.pick(
                            "O Android não confirmou o volume de mídia.",
                            "Android did not confirm the media volume.",
                            "Android no ha confirmado el volumen multimedia.",
                        )
                    })?
            }
            Action::ToggleMute => {
                let was_muted = self.muted()?;
                let cmd = if was_muted { "adj-unmute" } else { "adj-mute" };
                let _ = self.command(["shell", "cmd", "audio", cmd, "3"]);
                if self.muted()? == was_muted {
                    self.command(["shell", "input", "keyevent", "KEYCODE_VOLUME_MUTE"])?;
                    if self.muted()? == was_muted {
                        return Err(self
                            .locale
                            .pick(
                                "O Android não alterou o som de mídia.",
                                "Android did not change the media mute state.",
                                "Android no ha cambiado el estado del sonido multimedia.",
                            )
                            .into());
                    }
                }
                if was_muted {
                    self.locale.pick(
                        "Som de mídia ativado",
                        "Media sound on",
                        "Sonido multimedia activado",
                    )
                } else {
                    self.locale.pick(
                        "Mídia silenciada",
                        "Media muted",
                        "Sonido multimedia silenciado",
                    )
                }
                .into()
            }
            Action::Rotate => {
                self.command(["emu", "rotate"])?;
                self.locale
                    .pick(
                        "Rotação solicitada ao Android",
                        "Rotation requested",
                        "Rotación solicitada",
                    )
                    .into()
            }
            Action::Install(path) => {
                let reply =
                    self.command([OsStr::new("install"), OsStr::new("-r"), path.as_os_str()])?;
                if !reply.lines().any(|line| line.trim() == "Success") {
                    let message = self.locale.pick(
                        "Falha ao instalar o APK.",
                        "Could not install the APK.",
                        "No se pudo instalar el APK.",
                    );
                    return Err(format!("{message} {reply}"));
                }
                self.locale
                    .pick("APK instalado", "APK installed", "APK instalado")
                    .into()
            }
            Action::AndroidSettings => {
                self.command(["shell", "am", "start", "-a", "android.settings.SETTINGS"])?;
                self.locale
                    .pick(
                        "Configurações do Android abertas",
                        "Opened Android settings",
                        "Ajustes de Android abiertos",
                    )
                    .into()
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
    #[cfg(all(test, windows))]
    pub(crate) fn for_screenshot(package: &str, root: &std::path::Path) -> Self {
        let mut controls = Self::new(root.join("no-adb.exe"), 0, root.into());
        controls.foreground = Some(package.into());
        controls.next_foreground += std::time::Duration::from_secs(60);
        controls
    }

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
                locale: Locale::default(),
            },
            pending: None,
            queue: VecDeque::new(),
            message: None,
            foreground: None,
            foreground_query: None,
            next_foreground: std::time::Instant::now(),
        }
    }
    pub fn set_locale(&mut self, locale: Locale) {
        if self.guest.locale != locale {
            self.guest.locale = locale;
            self.message = None;
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
            Action::Install(_) => Some(Ok(self
                .guest
                .locale
                .pick("Instalando APK…", "Installing APK…", "Instalando APK…")
                .into())),
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
                Err(mpsc::TryRecvError::Disconnected) => Err(self
                    .guest
                    .locale
                    .pick(
                        "O controle do Android foi interrompido.",
                        "Android controls were interrupted.",
                        "Se han interrumpido los controles de Android.",
                    )
                    .into()),
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
                        crate::toolbar::audio_buttons(ui, &mut controls, Locale::default());
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
