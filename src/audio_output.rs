//! Where Android's sound goes on the computer.
//!
//! The default leaves the engine alone: its Windows back end opens the wave
//! mapper (`WAVE_MAPPER`) and its PulseAudio back end opens a stream without a
//! sink, so both follow the system default device, also when it changes while
//! Android runs. A chosen device is routed per OS:
//!
//! - Linux: `QEMU_PA_SINK`, the PulseAudio sink option of the engine's QEMU.
//! - Windows: the engine has no device option, so its own output is turned off
//!   with `-no-audio` and the RustEmuDroid window plays the engine's
//!   `streamAudio` gRPC stream through the chosen endpoint (WASAPI).

#[cfg(target_os = "linux")]
mod linux;
// The player half is used by the desktop window only.
#[cfg(windows)]
#[cfg_attr(not(feature = "ui"), allow(dead_code))]
pub(crate) mod windows;

use crate::locale::Locale;

/// The QEMU option read by the engine's PulseAudio back end (`audio/paaudio.c`).
pub(crate) const PULSE_SINK_VARIABLE: &str = "QEMU_PA_SINK";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Device {
    /// Windows endpoint ID or PulseAudio sink name.
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

/// The active output devices of this computer.
pub(crate) fn inventory() -> Result<Inventory, String> {
    #[cfg(windows)]
    return windows::inventory();
    #[cfg(target_os = "linux")]
    return linux::inventory();
    #[allow(unreachable_code)]
    Err("unsupported operating system".into())
}

/// How the engine runs, as far as its sound is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mode {
    pub windows: bool,
    /// The RustEmuDroid window shows Android and holds the gRPC channel.
    pub embedded: bool,
    /// The boot check runs with `-no-audio` already.
    pub headless: bool,
    /// The engine binary can play sound at all. On Linux `-no-window` selects
    /// the headless QEMU, whose PulseAudio library is a stub.
    pub engine_sound: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub set_env: Vec<(&'static str, String)>,
    pub remove_env: Vec<&'static str>,
    /// Turn off the engine's own output (Windows, played by the window instead).
    pub no_audio: bool,
    /// Windows endpoint the RustEmuDroid window plays the engine's stream to.
    pub client_endpoint: Option<String>,
    /// Lines for the boot details.
    pub messages: Vec<String>,
}

/// Decides the engine arguments, environment and messages for a saved choice.
/// `saved` is empty for the system default. `inventory` is only consulted for
/// a saved device.
pub(crate) fn plan(
    saved: &str,
    saved_name: &str,
    inventory: impl FnOnce() -> Result<Inventory, String>,
    mode: Mode,
    locale: Locale,
) -> Plan {
    let mut plan = Plan::default();
    if !mode.windows {
        // "System default" means the server's default sink, not an inherited override.
        plan.remove_env.push(PULSE_SINK_VARIABLE);
    }
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
            plan.messages.push(format!(
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
        plan.messages.push(format!(
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
        plan.remove_env.clear();
        plan.set_env.push((PULSE_SINK_VARIABLE, device.id.clone()));
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
        plan.client_endpoint = Some(device.id.clone());
        plan.messages.push(format!(
            "{chosen} ({}).",
            locale.pick(
                "reproduzido pela janela do RustEmuDroid",
                "played by the RustEmuDroid window",
                "reproducido por la ventana de RustEmuDroid",
            )
        ));
    } else {
        plan.messages.push(format!(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Inventory {
        Inventory {
            devices: vec![
                Device {
                    id: "{0.0.0.00000000}.{aaaa}".into(),
                    name: "Speakers (Realtek(R) Audio)".into(),
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
            "Speakers (Realtek(R) Audio)"
        );
    }

    #[test]
    fn system_default_changes_nothing_and_never_lists_devices() {
        let never = || -> Result<Inventory, String> { panic!("listed devices") };
        let plan = plan("", "", never, WINDOWS_PANEL, Locale::EnUs);
        assert_eq!(plan, Plan::default());
        // Linux drops an inherited sink override, so the server's default applies.
        let plan = super::plan("  ", "", never, LINUX_QT, Locale::EnUs);
        assert_eq!(plan.remove_env, [PULSE_SINK_VARIABLE]);
        assert!(plan.set_env.is_empty() && plan.messages.is_empty() && !plan.no_audio);
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
        assert_eq!(
            plan.client_endpoint.as_deref(),
            Some("{0.0.0.00000000}.{bbbb}")
        );
        assert!(plan.set_env.is_empty() && plan.remove_env.is_empty());
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
        assert_eq!(
            plan.messages,
            [
                "Uma saída de áudio específica só funciona na janela do RustEmuDroid. O Android usa a saída padrão do sistema."
            ]
        );
    }

    #[test]
    fn linux_routes_the_sink_through_the_engine() {
        let inventory = Inventory {
            devices: vec![Device {
                id: "alsa_output.usb-headset.analog-stereo".into(),
                name: "USB Headset Analog Stereo".into(),
            }],
            default: None,
        };
        let plan = plan(
            "alsa_output.usb-headset.analog-stereo",
            "USB Headset Analog Stereo",
            || Ok(inventory.clone()),
            LINUX_QT,
            Locale::EsEs,
        );
        assert_eq!(
            plan.set_env,
            [(
                PULSE_SINK_VARIABLE,
                "alsa_output.usb-headset.analog-stereo".to_string()
            )]
        );
        assert!(plan.remove_env.is_empty() && !plan.no_audio && plan.client_endpoint.is_none());
        assert_eq!(
            plan.messages,
            ["Salida de audio: USB Headset Analog Stereo."]
        );
        // The headless QEMU cannot play it; say so instead of failing quietly.
        let silent = Mode {
            engine_sound: false,
            ..LINUX_QT
        };
        let plan = super::plan(
            "alsa_output.usb-headset.analog-stereo",
            "",
            || Ok(inventory),
            silent,
            Locale::EnUs,
        );
        assert_eq!(plan.set_env.len(), 1);
        assert!(plan.messages[1].contains("-no-window"));
    }

    #[test]
    fn a_missing_device_falls_back_and_says_so() {
        for mode in [WINDOWS_PANEL, LINUX_QT] {
            let plan = plan(
                "{0.0.0.00000000}.{gone}",
                "Bluetooth Speaker",
                || Ok(devices()),
                mode,
                Locale::EnUs,
            );
            assert!(plan.set_env.is_empty() && !plan.no_audio && plan.client_endpoint.is_none());
            assert_eq!(
                plan.messages,
                [
                    "The saved audio output “Bluetooth Speaker” is not connected. Android uses the system default output."
                ]
            );
        }
        let plan = plan(
            "sink",
            "",
            || Err("pactl not found".into()),
            LINUX_QT,
            Locale::PtBr,
        );
        assert_eq!(plan.remove_env, [PULSE_SINK_VARIABLE]);
        assert_eq!(
            plan.messages,
            [
                "Não foi possível listar as saídas de áudio (pactl not found). O Android usa a saída padrão do sistema."
            ]
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
}
