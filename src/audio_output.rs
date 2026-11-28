//! Where Android's sound goes on the computer.
//!
//! The default leaves the engine alone: on Windows its output opens the
//! system's default wave output (`WAVE_MAPPER`) and on Linux it opens a sound
//! server stream without naming an output, so both follow the system default
//! device, also when it changes while Android runs. A chosen device is routed
//! per OS:
//!
//! - Linux: `QEMU_PA_SINK`, the engine's variable that names the sound server
//!   output it plays to.
//! - Windows: the engine has no device option, so its own output is turned off
//!   with `-no-audio` and the RustEmuDroid window plays the engine's
//!   `streamAudio` stream on the chosen endpoint.

#[cfg(target_os = "linux")]
mod linux;
// The player half is used by the desktop window only.
#[cfg(windows)]
#[cfg_attr(not(feature = "ui"), allow(dead_code))]
pub(crate) mod windows;

use crate::locale::Locale;

/// The variable the engine's Linux sound output reads for the output it plays to.
pub(crate) const SINK_VARIABLE: &str = "QEMU_PA_SINK";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Device {
    /// Windows endpoint ID or the Linux sound server's output name.
    pub id: String,
    /// Friendly name shown by the system.
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub devices: Vec<Device>,
    /// ID of the current system default output, when the system reports one.
    pub default: Option<String>,
}

impl Inventory {
    #[cfg_attr(not(feature = "ui"), allow(dead_code))]
    pub(crate) fn default_device(&self) -> Option<&Device> {
        let id = self.default.as_deref()?;
        self.devices.iter().find(|device| device.id == id)
    }

    /// A saved value matches a device ID first, then its friendly name
    /// (case-insensitive), which lets the command line take either.
    pub(crate) fn find(&self, saved: &str) -> Option<&Device> {
        let saved = saved.trim();
        self.devices
            .iter()
            .find(|device| device.id == saved)
            .or_else(|| {
                self.devices
                    .iter()
                    .find(|device| device.name.eq_ignore_ascii_case(saved))
            })
    }
}

/// The active output devices of this computer. A failure is described in
/// `locale`; system call names and their codes stay as the system gives them.
pub(crate) fn inventory(locale: Locale) -> Result<Inventory, String> {
    #[cfg(windows)]
    {
        // A Windows audio failure is a call and its code, the same in any language.
        let _ = locale;
        return windows::inventory();
    }
    #[cfg(target_os = "linux")]
    return linux::inventory(locale);
    #[cfg(not(any(windows, target_os = "linux")))]
    return Err(locale
        .pick(
            "sistema operacional sem suporte",
            "unsupported operating system",
            "sistema operativo no compatible",
        )
        .into());
}

/// How the engine runs, as far as its sound is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mode {
    pub windows: bool,
    /// The RustEmuDroid window shows Android and holds the engine's control channel.
    pub embedded: bool,
    /// The boot check runs with `-no-audio` already.
    pub headless: bool,
    /// The engine binary can play sound at all. On Linux `-no-window` selects
    /// the engine's headless build, which has no sound output.
    pub engine_sound: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub set_env: Vec<(&'static str, String)>,
    /// Turn off the engine's own output (Windows, played by the window instead).
    pub no_audio: bool,
    /// Windows endpoint the RustEmuDroid window plays the engine's stream to.
    pub client_endpoint: Option<Device>,
    /// Lines for the boot details.
    pub messages: Vec<String>,
    /// The saved device is not in use: said where the user sees it while
    /// Android runs, and not only in the boot details.
    pub notice: Option<String>,
}

/// Decides the engine arguments, environment and messages for a saved choice.
/// `saved` is empty for the system default, which changes nothing: the engine
/// starts as it did before the setting existed, in the environment the user
/// launched the client with (an exported `QEMU_PA_SINK` included). `inventory`
/// is only consulted for a saved device.
pub(crate) fn plan(
    saved: &str,
    saved_name: &str,
    inventory: impl FnOnce() -> Result<Inventory, String>,
    mode: Mode,
    locale: Locale,
) -> Plan {
    let mut plan = Plan::default();
    let saved = saved.trim();
    if saved.is_empty() || mode.headless {
        return plan;
    }
    let shown = if saved_name.trim().is_empty() {
        saved
    } else {
        saved_name.trim()
    };
    let fallback = locale.pick(
        "O Android usa a saída padrão do sistema.",
        "Android uses the system default output.",
        "Android usa la salida predeterminada del sistema.",
    );
    let inventory = match inventory() {
        Ok(inventory) => inventory,
        Err(error) => {
            plan.notice = Some(format!(
                "{} ({error}). {fallback}",
                locale.pick(
                    "Não foi possível listar as saídas de áudio",
                    "Could not list the audio outputs",
                    "No se han podido enumerar las salidas de audio",
                )
            ));
            return plan;
        }
    };
    let Some(device) = inventory.find(saved) else {
        plan.notice = Some(format!(
            "{} “{shown}” {} {fallback}",
            locale.pick(
                "A saída de áudio salva",
                "The saved audio output",
                "La salida de audio guardada",
            ),
            locale.pick(
                "não está conectada.",
                "is not connected.",
                "no está conectada.",
            ),
        ));
        return plan;
    };
    let chosen = format!(
        "{}: {}",
        locale.pick("Saída de áudio", "Audio output", "Salida de audio"),
        device.name
    );
    if !mode.windows {
        plan.set_env.push((SINK_VARIABLE, device.id.clone()));
        plan.messages.push(format!("{chosen}."));
        if !mode.engine_sound {
            plan.messages.push(
                locale
                    .pick(
                        "Neste modo o motor roda sem janela (-no-window) e não reproduz som. A saída escolhida vale quando o motor tiver som.",
                        "In this mode the engine runs without a window (-no-window) and plays no sound. The chosen output applies once the engine has sound.",
                        "En este modo el motor se ejecuta sin ventana (-no-window) y no reproduce sonido. La salida elegida se aplicará cuando el motor tenga sonido.",
                    )
                    .into(),
            );
        }
    } else if mode.embedded {
        plan.no_audio = true;
        plan.client_endpoint = Some(device.clone());
        plan.messages.push(format!(
            "{chosen} ({}).",
            locale.pick(
                "reproduzido pela janela do RustEmuDroid",
                "played by the RustEmuDroid window",
                "reproducido por la ventana de RustEmuDroid",
            )
        ));
    } else {
        plan.notice = Some(format!(
            "{} {fallback}",
            locale.pick(
                "Uma saída de áudio específica só funciona na janela do RustEmuDroid.",
                "A specific audio output works only in the RustEmuDroid window.",
                "Una salida de audio concreta solo funciona en la ventana de RustEmuDroid.",
            )
        ));
    }
    plan
}

/// Checks a device named on the command line (`boot --audio-output`). A saved
/// device that cannot be used falls back and says so (`plan`); one asked for
/// explicitly is a usage error instead, and the text says why. `Ok` means
/// `plan` routes the sound to it.
pub(crate) fn check_explicit(
    requested: &str,
    inventory: impl FnOnce() -> Result<Inventory, String>,
    mode: Mode,
    locale: Locale,
) -> Result<(), String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(());
    }
    if mode.headless {
        return Err(locale
            .pick(
                "--audio-output não pode ser usado com --headless: nesse modo o motor roda sem som.",
                "--audio-output cannot be used with --headless: in that mode the engine runs without sound.",
                "--audio-output no se puede usar con --headless: en ese modo el motor se ejecuta sin sonido.",
            )
            .into());
    }
    if mode.windows && !mode.embedded {
        return Err(locale
            .pick(
                "No Windows, boot não aceita --audio-output: o motor só toca na saída padrão do sistema, e apenas a janela do RustEmuDroid reproduz o som em outro dispositivo. Escolha o dispositivo em Configurações → Som → Saída de áudio.",
                "On Windows, boot does not accept --audio-output: the engine only plays on the system default output, and only the RustEmuDroid window plays the sound on another device. Choose the device in Settings → Sound → Audio output.",
                "En Windows, boot no admite --audio-output: el motor solo reproduce en la salida predeterminada del sistema, y únicamente la ventana de RustEmuDroid reproduce el sonido en otro dispositivo. Elige el dispositivo en Ajustes → Sonido → Salida de audio.",
            )
            .into());
    }
    let inventory = inventory().map_err(|error| {
        format!(
            "{} ({error}).",
            locale.pick(
                "--audio-output não pode ser atendido: não foi possível listar as saídas de áudio",
                "--audio-output cannot be honoured: the audio outputs could not be listed",
                "--audio-output no se puede aplicar: no se han podido enumerar las salidas de audio",
            )
        )
    })?;
    if inventory.find(requested).is_some() {
        return Ok(());
    }
    let mut text = format!(
        "{} “{requested}”. {}",
        locale.pick(
            "--audio-output: nenhuma saída de áudio tem o ID ou o nome",
            "--audio-output: no audio output has the ID or name",
            "--audio-output: ninguna salida de audio tiene el ID o el nombre",
        ),
        locale.pick(
            "Saídas disponíveis (ID, nome):",
            "Available outputs (ID, name):",
            "Salidas disponibles (ID, nombre):",
        ),
    );
    for device in &inventory.devices {
        text.push_str(&format!("\n  {}\t{}", device.id, device.name));
    }
    if inventory.devices.is_empty() {
        text.push_str(locale.pick("\n  (nenhuma)", "\n  (none)", "\n  (ninguna)"));
    }
    Err(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Inventory {
        Inventory {
            devices: vec![
                Device {
                    id: "{0.0.0.00000000}.{aaaa}".into(),
                    name: "Speakers (Built-in Audio)".into(),
                },
                Device {
                    id: "{0.0.0.00000000}.{bbbb}".into(),
                    name: "Headphones (USB)".into(),
                },
            ],
            default: Some("{0.0.0.00000000}.{aaaa}".into()),
        }
    }

    const WINDOWS_PANEL: Mode = Mode {
        windows: true,
        embedded: true,
        headless: false,
        engine_sound: true,
    };
    const LINUX_QT: Mode = Mode {
        windows: false,
        embedded: true,
        headless: false,
        engine_sound: true,
    };

    #[test]
    fn devices_match_by_id_then_by_name() {
        let inventory = devices();
        assert_eq!(
            inventory.find("{0.0.0.00000000}.{bbbb}").unwrap().name,
            "Headphones (USB)"
        );
        assert_eq!(
            inventory.find(" headphones (usb) ").unwrap().id,
            "{0.0.0.00000000}.{bbbb}"
        );
        assert!(inventory.find("HDMI").is_none());
        assert_eq!(
            inventory.default_device().unwrap().name,
            "Speakers (Built-in Audio)"
        );
    }

    #[test]
    fn system_default_changes_nothing_and_never_lists_devices() {
        let never = || -> Result<Inventory, String> { panic!("listed devices") };
        // No argument, no variable set and none removed: on Linux an exported
        // QEMU_PA_SINK reaches the engine as the user launched it.
        for mode in [
            WINDOWS_PANEL,
            LINUX_QT,
            Mode {
                engine_sound: false,
                ..LINUX_QT
            },
            Mode {
                embedded: false,
                ..LINUX_QT
            },
        ] {
            for saved in ["", "  "] {
                assert_eq!(
                    plan(saved, "", never, mode, Locale::EnUs),
                    Plan::default(),
                    "{mode:?}"
                );
            }
        }
        // Every field of a plan: nothing in it can remove a variable.
        let Plan {
            set_env,
            no_audio,
            client_endpoint,
            messages,
            notice,
        } = Plan::default();
        assert!(set_env.is_empty() && !no_audio && client_endpoint.is_none());
        assert!(messages.is_empty() && notice.is_none());
    }

    #[test]
    fn windows_panel_plays_the_chosen_endpoint_itself() {
        let plan = plan(
            "{0.0.0.00000000}.{bbbb}",
            "Headphones (USB)",
            || Ok(devices()),
            WINDOWS_PANEL,
            Locale::EnUs,
        );
        assert!(plan.no_audio);
        assert_eq!(plan.client_endpoint.as_ref(), Some(&devices().devices[1]));
        assert!(plan.set_env.is_empty() && plan.notice.is_none());
        assert_eq!(
            plan.messages,
            ["Audio output: Headphones (USB) (played by the RustEmuDroid window)."]
        );
    }

    #[test]
    fn windows_engine_window_keeps_the_system_default() {
        let mode = Mode {
            embedded: false,
            ..WINDOWS_PANEL
        };
        let plan = plan("Headphones (USB)", "", || Ok(devices()), mode, Locale::PtBr);
        assert!(!plan.no_audio && plan.client_endpoint.is_none());
        assert!(plan.messages.is_empty());
        assert_eq!(
            plan.notice.as_deref(),
            Some(
                "Uma saída de áudio específica só funciona na janela do RustEmuDroid. O Android usa a saída padrão do sistema."
            )
        );
    }

    #[test]
    fn linux_routes_the_sink_through_the_engine() {
        let inventory = Inventory {
            devices: vec![Device {
                id: "output.usb-headset.analog-stereo".into(),
                name: "USB Headset Analog Stereo".into(),
            }],
            default: None,
        };
        let plan = plan(
            "output.usb-headset.analog-stereo",
            "USB Headset Analog Stereo",
            || Ok(inventory.clone()),
            LINUX_QT,
            Locale::EsEs,
        );
        assert_eq!(
            plan.set_env,
            [(
                SINK_VARIABLE,
                "output.usb-headset.analog-stereo".to_string()
            )]
        );
        assert!(!plan.no_audio && plan.client_endpoint.is_none() && plan.notice.is_none());
        assert_eq!(
            plan.messages,
            ["Salida de audio: USB Headset Analog Stereo."]
        );
        // The headless engine cannot play it; say so instead of failing quietly.
        let silent = Mode {
            engine_sound: false,
            ..LINUX_QT
        };
        let plan = super::plan(
            "output.usb-headset.analog-stereo",
            "",
            || Ok(inventory),
            silent,
            Locale::EnUs,
        );
        assert_eq!(plan.set_env.len(), 1);
        assert!(plan.messages[1].contains("-no-window"));
    }

    #[test]
    fn a_missing_saved_device_falls_back_and_tells_the_user() {
        for mode in [WINDOWS_PANEL, LINUX_QT] {
            let plan = plan(
                "{0.0.0.00000000}.{gone}",
                "Bluetooth Speaker",
                || Ok(devices()),
                mode,
                Locale::EnUs,
            );
            // The engine starts as with the system default.
            assert!(plan.set_env.is_empty() && !plan.no_audio && plan.client_endpoint.is_none());
            assert!(plan.messages.is_empty());
            assert_eq!(
                plan.notice.as_deref(),
                Some(
                    "The saved audio output “Bluetooth Speaker” is not connected. Android uses the system default output."
                )
            );
        }
        let plan = plan(
            "sink",
            "",
            || Err("No such file or directory (os error 2)".into()),
            LINUX_QT,
            Locale::PtBr,
        );
        assert!(plan.set_env.is_empty());
        assert_eq!(
            plan.notice.as_deref(),
            Some(
                "Não foi possível listar as saídas de áudio (No such file or directory (os error 2)). O Android usa a saída padrão do sistema."
            )
        );
    }

    #[test]
    fn the_boot_check_ignores_the_choice() {
        let mode = Mode {
            headless: true,
            ..WINDOWS_PANEL
        };
        let plan = plan(
            "Headphones (USB)",
            "",
            || panic!("listed devices"),
            mode,
            Locale::EnUs,
        );
        assert_eq!(plan, Plan::default());
    }

    const LINUX_BOOT: Mode = Mode {
        windows: false,
        embedded: false,
        headless: false,
        engine_sound: true,
    };

    #[test]
    fn an_explicit_device_must_exist_and_the_error_lists_the_others() {
        let never = || -> Result<Inventory, String> { panic!("listed devices") };
        assert_eq!(check_explicit(" ", never, LINUX_BOOT, Locale::EnUs), Ok(()));
        for known in ["{0.0.0.00000000}.{bbbb}", "headphones (usb)"] {
            assert_eq!(
                check_explicit(known, || Ok(devices()), LINUX_BOOT, Locale::EnUs),
                Ok(())
            );
        }
        assert_eq!(
            check_explicit("hedphones", || Ok(devices()), LINUX_BOOT, Locale::EnUs).unwrap_err(),
            "--audio-output: no audio output has the ID or name “hedphones”. Available outputs (ID, name):\n  {0.0.0.00000000}.{aaaa}\tSpeakers (Built-in Audio)\n  {0.0.0.00000000}.{bbbb}\tHeadphones (USB)"
        );
        assert!(
            check_explicit("x", || Ok(Inventory::default()), LINUX_BOOT, Locale::EsEs)
                .unwrap_err()
                .ends_with("Salidas disponibles (ID, nombre):\n  (ninguna)")
        );
        assert_eq!(
            check_explicit("x", || Err("timeout".into()), LINUX_BOOT, Locale::PtBr).unwrap_err(),
            "--audio-output não pode ser atendido: não foi possível listar as saídas de áudio (timeout)."
        );
    }

    #[test]
    fn an_explicit_device_is_refused_where_it_can_never_apply() {
        let never = || -> Result<Inventory, String> { panic!("listed devices") };
        for locale in [Locale::PtBr, Locale::EnUs, Locale::EsEs] {
            let headless = Mode {
                headless: true,
                engine_sound: false,
                ..LINUX_BOOT
            };
            let error = check_explicit("Headphones (USB)", never, headless, locale).unwrap_err();
            assert!(error.contains("--audio-output") && error.contains("--headless"));
            // The engine has no device option on Windows and `boot` has no player.
            let windows = Mode {
                windows: true,
                ..LINUX_BOOT
            };
            let error = check_explicit("Headphones (USB)", never, windows, locale).unwrap_err();
            assert!(error.contains("Windows") && error.contains("boot"));
        }
        assert_eq!(
            check_explicit(
                "Headphones (USB)",
                never,
                Mode {
                    headless: true,
                    ..WINDOWS_PANEL
                },
                Locale::EnUs
            )
            .unwrap_err(),
            "--audio-output cannot be used with --headless: in that mode the engine runs without sound."
        );
    }
}
