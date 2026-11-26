//! AMD graphics on Linux (amdgpu with RADV) and the stock Android Emulator.
//!
//! When the host Vulkan vendor is AMD on Linux, the emulator turns on two
//! features by itself (`emugl_config.cpp`, bug 324086743):
//!
//! - `VulkanAllocateDeviceMemoryOnly` hides from Android every Vulkan memory
//!   type without DEVICE_LOCAL. The only mappable heap left is the video
//!   memory the processor can reach, whose size is the card's PCI BAR: 256 MB
//!   when "Above 4G Decoding" / "Resizable BAR" is off in the firmware. Unity
//!   games then stop with `Vulkan - Out of memory!`.
//! - `VulkanAllocateHostMemory` backs host-visible Vulkan memory with ordinary
//!   process memory that the driver imports as user-pointer buffers. The
//!   engine fork()s helpers (adb, `chattr`); a fork write-protects those
//!   pages, amdgpu can not get them back within about one second and rejects
//!   the next command submission (`The CS has been rejected (-16)`). The
//!   engine treats that as a lost device and aborts. The same rejection was
//!   seen once with no helper involved; its cause was not identified.
//!
//! What the client does, only on Linux, only for the host or automatic GPU
//! mode, when an AMD card exists and the chosen Vulkan adapter does not name
//! another vendor:
//!
//! 1. It writes the library built from `native/linux/userptr_guard.c` next to
//!    the managed runtime and adds it to the engine's `LD_PRELOAD`. The
//!    library keeps user-pointer buffers out of fork() and does nothing in a
//!    process that never registers one.
//!
//!    As a second line of defence the library resends a submission the
//!    driver rejected with EBUSY, which means the driver could not get the
//!    buffers' pages in time and submitted nothing. It first makes those
//!    pages present and writable again from user space, where there is no
//!    time limit, and tries for at most 30 seconds. If the driver still
//!    rejects, the engine ends as before and `engine_faults` reports the
//!    lost device.
//! 2. It chooses a memory mode ([`Memory`]). A default only uses a mode that
//!    was measured, and everything was measured on Linux 6.8 (Radeon RX 580
//!    8 GB with a 256 MB window):
//!    - A processor-visible window smaller than the video memory: *full
//!      size*, whatever the kernel. The emulator's options stay, and the
//!      library is started with `RUSTEMUDROID_AMD_VISIBLE_VRAM=all`: the
//!      driver's memory query then reports the whole video memory as
//!      processor visible, so the one heap Android sees has its real size.
//!    - A window that covers the video memory, or sizes that can not be read:
//!      *stock*. Only the library is added.
//!    - *All types* (`-VulkanAllocateDeviceMemoryOnly` in the `-feature` list,
//!      so Android also gets the memory types outside the card) is never
//!      chosen automatically. On Linux 6.8 KVM can not map the driver's
//!      system-memory pages into the guest, and games that use them freeze
//!      the VM with `kvm run failed Bad address` (reported by
//!      `engine_faults`). The kernel sources say Linux 6.13 maps them; nobody
//!      has run it, so the mode stays behind the override.
//!
//!    `VulkanAllocateHostMemory` is never changed.
//!
//! The note for the boot details is written before the engine starts: it says
//! what the client prepared, not what the engine then did. With `-no-window`
//! the automatic GPU mode makes the engine pick its software renderer on
//! Linux, where none of this takes effect; it matters with the host GPU mode.
//!
//! `RUSTEMUDROID_AMD_LINUX`, in the client's environment, overrides this for
//! diagnosis:
//!
//! - `off`: none of the above; the engine starts as before.
//! - `stock`, `full-size`, `all-types`: the library is loaded and that memory
//!   mode is used, whatever the card.
//!
//! Variables the library reads inside the engine process:
//!
//! - `RUSTEMUDROID_AMD_VISIBLE_VRAM`: owned by the client. It is set to `all`
//!   in the full size mode and removed from the engine's environment in the
//!   other two, so a value left in the client's own environment never reaches
//!   the engine.
//! - `RUSTEMUDROID_USERPTR_GUARD=0` (the library does nothing) and
//!   `RUSTEMUDROID_USERPTR_GUARD_LOG=1` (a line every 64 buffers): the user's
//!   own switches, passed through as they are.
//! - `RUSTEMUDROID_USERPTR_GUARD_TEST=allow-fork`: for testing only, also
//!   passed through. The buffers stay visible to fork(), which brings the
//!   driver's rejection back on purpose to exercise the second line of
//!   defence.

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use super::{Gpu, Locale};

const OVERRIDE_VARIABLE: &str = "RUSTEMUDROID_AMD_LINUX";
/// Read by the library: `all` makes the driver's memory query report the whole
/// video memory as processor visible.
const VISIBLE_VRAM_VARIABLE: &str = "RUSTEMUDROID_AMD_VISIBLE_VRAM";
const VISIBLE_VRAM_ALL: &str = "all";
const PRELOAD_VARIABLE: &str = "LD_PRELOAD";
const SYSFS_DRM: &str = "/sys/class/drm";
const KERNEL_RELEASE: &str = "/proc/sys/kernel/osrelease";
/// PCI vendor of AMD graphics cards, as sysfs prints it.
const AMD_VENDOR: &str = "0x1002";

/// The preload library, compiled by `build.rs` for Linux targets.
#[cfg(target_os = "linux")]
const GUARD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libuserptr_guard.so"));
/// Never used: [`Host::detect`] finds nothing outside Linux.
#[cfg(not(target_os = "linux"))]
const GUARD: &[u8] = &[];

/// `RUSTEMUDROID_AMD_LINUX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Override {
    Auto,
    Off,
    Stock,
    FullSize,
    AllTypes,
}

impl Override {
    fn parse(value: Option<&str>) -> Self {
        match value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("off") => Self::Off,
            Some("stock") => Self::Stock,
            Some("full-size") => Self::FullSize,
            Some("all-types") => Self::AllTypes,
            _ => Self::Auto,
        }
    }

    fn from_environment() -> Self {
        Self::parse(env::var(OVERRIDE_VARIABLE).ok().as_deref())
    }

    /// Shown in the boot details when the user chose the memory mode.
    const fn shown(self) -> &'static str {
        match self {
            Self::Auto | Self::Off => "",
            Self::Stock => " (RUSTEMUDROID_AMD_LINUX=stock)",
            Self::FullSize => " (RUSTEMUDROID_AMD_LINUX=full-size)",
            Self::AllTypes => " (RUSTEMUDROID_AMD_LINUX=all-types)",
        }
    }
}

/// What Android gets to see of the card's memory. The library is loaded in
/// all three modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Memory {
    /// The emulator's own options: Android maps only the processor-visible
    /// window of the video memory.
    Stock,
    /// The emulator's own options, and the library reports the whole video
    /// memory as processor visible (`RUSTEMUDROID_AMD_VISIBLE_VRAM=all`).
    FullSize,
    /// `-VulkanAllocateDeviceMemoryOnly`: Android also gets the memory types
    /// outside the card. Only through the override: it froze the VM on the one
    /// kernel it was measured on.
    AllTypes,
}

/// A Linux kernel release, as far as the advice needs it. The memory mode
/// does not depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Kernel {
    major: u32,
    minor: u32,
}

impl Kernel {
    /// The first release whose KVM should map the graphics driver's
    /// system-memory pages into the guest, going by the kernel sources. Not
    /// measured.
    const MAPS_DRIVER_MEMORY: Self = Self {
        major: 6,
        minor: 13,
    };

    /// `6.8.0-151-generic`, `6.13.0`, `7.0.0-44-generic`, `6.13-rc3`.
    fn parse(release: &str) -> Option<Self> {
        let mut parts = release.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?;
        let digits = minor
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(minor.len());
        Some(Self {
            major,
            minor: minor[..digits].parse().ok()?,
        })
    }

    fn read(release: &Path) -> Option<Self> {
        Self::parse(&fs::read_to_string(release).ok()?)
    }
}

impl fmt::Display for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// One AMD card from `/sys/class/drm/card*/device`. Sizes are in bytes and
/// missing with a driver other than amdgpu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Card {
    /// `mem_info_vis_vram_total`: the VRAM the processor can reach (the PCI BAR).
    visible: Option<u64>,
    /// `mem_info_vram_total`.
    total: Option<u64>,
}

impl Card {
    /// The processor reaches less memory than the card has.
    fn small_window(&self) -> bool {
        self.window_mb().is_some()
    }

    /// Visible and total VRAM in MB, when the window is smaller than the VRAM.
    fn window_mb(&self) -> Option<(u64, u64)> {
        const MB: u64 = 1024 * 1024;
        match (self.visible, self.total) {
            (Some(visible), Some(total)) if visible < total => Some((visible / MB, total / MB)),
            _ => None,
        }
    }
}

/// Every AMD card under `drm` (`/sys/class/drm`), in card order.
fn amd_cards(drm: &Path) -> Vec<Card> {
    let mut cards: Vec<(u32, Card)> = fs::read_dir(drm)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            // `card0`, not its connectors (`card0-DP-1`) nor `renderD128`.
            let index: u32 = entry
                .file_name()
                .to_str()?
                .strip_prefix("card")?
                .parse()
                .ok()?;
            let device = entry.path().join("device");
            let vendor = fs::read_to_string(device.join("vendor")).ok()?;
            if !vendor.trim().eq_ignore_ascii_case(AMD_VENDOR) {
                return None;
            }
            let bytes = |name: &str| -> Option<u64> {
                fs::read_to_string(device.join(name))
                    .ok()?
                    .trim()
                    .parse()
                    .ok()
            };
            Some((
                index,
                Card {
                    visible: bytes("mem_info_vis_vram_total"),
                    total: bytes("mem_info_vram_total"),
                },
            ))
        })
        .collect();
    cards.sort_by_key(|(index, _)| *index);
    cards.into_iter().map(|(_, card)| card).collect()
}

/// The card that decides when there are several. The Vulkan adapter name can
/// not be matched to a sysfs card here, so a card with a small window comes
/// first and then the one with the most memory (the discrete card).
fn deciding_card(cards: Vec<Card>) -> Option<Card> {
    cards
        .into_iter()
        .max_by_key(|card| (card.small_window(), card.total.unwrap_or(0)))
}

/// The AMD card and the running kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Host {
    card: Card,
    kernel: Option<Kernel>,
}

impl Host {
    /// `drm`: `/sys/class/drm`. `kernel_release`: `/proc/sys/kernel/osrelease`.
    pub(super) fn read(drm: &Path, kernel_release: &Path) -> Option<Self> {
        Some(Self {
            card: deciding_card(amd_cards(drm))?,
            kernel: Kernel::read(kernel_release),
        })
    }

    /// This computer, when it runs Linux and has an AMD card.
    fn detect() -> Option<Self> {
        cfg!(target_os = "linux")
            .then(|| Self::read(Path::new(SYSFS_DRM), Path::new(KERNEL_RELEASE)))
            .flatten()
    }

    fn kernel_maps_driver_memory(&self) -> bool {
        self.kernel
            .is_some_and(|kernel| kernel >= Kernel::MAPS_DRIVER_MEMORY)
    }

    /// `(here: 256 MB of 8192 MB, kernel 6.8)` with what is known, or nothing.
    fn here(&self, locale: Locale) -> String {
        let window = self.card.window_mb().map(|(visible, total)| {
            format!("{visible} MB {} {total} MB", locale.pick("de", "of", "de"))
        });
        let kernel = self.kernel.map(|kernel| format!("kernel {kernel}"));
        let known: Vec<String> = window.into_iter().chain(kernel).collect();
        if known.is_empty() {
            String::new()
        } else {
            format!(
                " ({}: {})",
                locale.pick("aqui", "here", "aquí"),
                known.join(", ")
            )
        }
    }

    /// After the engine printed `kvm run failed Bad address`: when it happens
    /// and what should avoid it. `None` on a kernel expected to map the
    /// driver's memory.
    pub(crate) fn kvm_mapping_advice(&self, locale: Locale) -> Option<String> {
        if self.kernel_maps_driver_memory() {
            return None;
        }
        Some(format!(
            "{}{}. {}",
            locale.pick(
                "Isso acontece com placas de vídeo AMD em kernels Linux anteriores ao 6.13 quando a placa expõe uma janela de memória pequena",
                "This happens with AMD graphics on Linux kernels older than 6.13 when the card exposes a small memory window",
                "Ocurre con gráficas AMD en kernels de Linux anteriores al 6.13 cuando la tarjeta expone una ventana de memoria pequeña",
            ),
            self.here(locale),
            remedy(locale)
        ))
    }

    /// `doctor`: the window is smaller than the VRAM and the kernel is older
    /// than 6.13, so a game can freeze Android.
    fn doctor_advice(&self, locale: Locale) -> Option<String> {
        let (visible, total) = self.card.window_mb()?;
        let kernel = self
            .kernel
            .filter(|kernel| *kernel < Kernel::MAPS_DRIVER_MEMORY)?;
        Some(match locale {
            Locale::PtBr => format!(
                "Placa de vídeo AMD: o processador alcança só {visible} MB dos {total} MB da placa (janela de memória pequena) e este kernel Linux ({kernel}) é anterior ao 6.13. Alguns jogos podem congelar o Android quando a virtualização do processador (KVM) não consegue mapear a memória de vídeo. {}",
                remedy(locale)
            ),
            Locale::EnUs => format!(
                "AMD graphics: the processor reaches only {visible} MB of the card's {total} MB (small memory window) and this Linux kernel ({kernel}) is older than 6.13. Some games can freeze Android when the processor virtualization (KVM) can not map graphics memory. {}",
                remedy(locale)
            ),
            Locale::EsEs => format!(
                "Gráfica AMD: el procesador solo alcanza {visible} MB de los {total} MB de la tarjeta (ventana de memoria pequeña) y este kernel de Linux ({kernel}) es anterior al 6.13. Algunos juegos pueden congelar Android cuando la virtualización del procesador (KVM) no puede mapear la memoria gráfica. {}",
                remedy(locale)
            ),
        })
    }
}

/// Expected from the kernel and driver sources; neither was measured.
const fn remedy(locale: Locale) -> &'static str {
    locale.pick(
        "Ativar «Above 4G Decoding» e «Resizable BAR» no firmware (BIOS/UEFI) ou usar o Linux 6.13 ou mais novo deve evitar o problema.",
        "Enabling “Above 4G Decoding” and “Resizable BAR” in the firmware (BIOS/UEFI) or using Linux 6.13 or newer should avoid it.",
        "Activar «Above 4G Decoding» y «Resizable BAR» en el firmware (BIOS/UEFI) o usar Linux 6.13 o posterior debería evitarlo.",
    )
}

/// `doctor`: advice for this computer, when it needs any.
pub(crate) fn doctor_advice(locale: Locale) -> Option<String> {
    Decision::detect(Gpu::Auto, "")?.host.doctor_advice(locale)
}

/// The chosen Vulkan adapter (empty for automatic) clearly belongs to another
/// vendor or to a software renderer. An unknown name does not count: the
/// library does nothing unless the amdgpu user-pointer call is used.
fn names_another_vendor(adapter: &str) -> bool {
    const AMD: [&str; 4] = ["amd", "radeon", "radv", "ati "];
    const OTHERS: [&str; 12] = [
        "nvidia",
        "geforce",
        "quadro",
        "nouveau",
        "intel",
        "llvmpipe",
        "lavapipe",
        "swiftshader",
        "virtio",
        "venus",
        "microsoft",
        "d3d12",
    ];
    let name = adapter.to_ascii_lowercase();
    !AMD.iter().any(|mark| name.contains(mark)) && OTHERS.iter().any(|mark| name.contains(mark))
}

/// This launch can end up on the AMD card: host or automatic GPU mode, and no
/// other vendor's adapter selected.
fn launch_can_use_amd(gpu: Gpu, adapter: &str) -> bool {
    matches!(gpu, Gpu::Host | Gpu::Auto) && !names_another_vendor(adapter)
}

/// The memory mode for this computer. `None`: nothing is done at all, because
/// there is no AMD card or the override switched the behaviour off.
fn policy(choice: Override, host: Option<&Host>) -> Option<Memory> {
    let host = host?;
    Some(match choice {
        Override::Off => return None,
        Override::Stock => Memory::Stock,
        Override::FullSize => Memory::FullSize,
        Override::AllTypes => Memory::AllTypes,
        // The measured way to give games the memory they need: enlarge the
        // one heap Android sees. The kernel does not change the choice, because
        // all types was never run on a kernel that should support it.
        Override::Auto if host.card.small_window() => Memory::FullSize,
        Override::Auto => Memory::Stock,
    })
}

/// What this boot does about the AMD card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    host: Host,
    choice: Override,
    memory: Memory,
}

/// The result of [`Decision::apply`] on an engine command.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Applied {
    /// Add `-VulkanAllocateDeviceMemoryOnly` to the launch's `-feature` list.
    pub(crate) all_memory_types: bool,
    /// One line for the boot details.
    pub(crate) note: String,
}

impl Decision {
    fn new(choice: Override, gpu: Gpu, adapter: &str, host: Option<Host>) -> Option<Self> {
        let host = host.filter(|_| launch_can_use_amd(gpu, adapter))?;
        Some(Self {
            memory: policy(choice, Some(&host))?,
            host,
            choice,
        })
    }

    /// `None` outside Linux, without an AMD card, with a software GPU mode,
    /// with another vendor's adapter selected or with the behaviour switched off.
    pub(crate) fn detect(gpu: Gpu, adapter: &str) -> Option<Self> {
        Self::new(Override::from_environment(), gpu, adapter, Host::detect())
    }

    pub(crate) fn host(&self) -> &Host {
        &self.host
    }

    /// Prepares the engine command. `host_vulkan`: this launch lets Android use
    /// the host's Vulkan device. A library that can not be written never stops
    /// the boot; the note says so.
    pub(crate) fn apply(
        &self,
        command: &mut Command,
        host_vulkan: bool,
        locale: Locale,
    ) -> Applied {
        self.apply_with(
            command,
            host_vulkan,
            install_guard(),
            env::var_os(PRELOAD_VARIABLE),
            locale,
        )
    }

    fn apply_with(
        &self,
        command: &mut Command,
        host_vulkan: bool,
        guard: Result<PathBuf, GuardError>,
        inherited_preload: Option<OsString>,
        locale: Locale,
    ) -> Applied {
        if let Ok(library) = &guard {
            command.env(
                PRELOAD_VARIABLE,
                preload(inherited_preload.as_deref(), library),
            );
        }
        // Without the host's Vulkan device there is no memory to choose, and
        // the full size is the library's doing.
        let memory = if !host_vulkan || (self.memory == Memory::FullSize && guard.is_err()) {
            Memory::Stock
        } else {
            self.memory
        };
        if memory == Memory::FullSize {
            command.env(VISIBLE_VRAM_VARIABLE, VISIBLE_VRAM_ALL);
        } else {
            // A value left in the client's own environment must not reach the engine.
            command.env_remove(VISIBLE_VRAM_VARIABLE);
        }
        Applied {
            all_memory_types: memory == Memory::AllTypes,
            note: self.note(memory, guard.as_ref().err(), locale),
        }
    }

    /// Written before the engine starts: what the client prepared, not what
    /// the engine then did with it.
    fn note(&self, memory: Memory, guard: Option<&GuardError>, locale: Locale) -> String {
        let protection = match guard {
            None => locale
                .pick(
                    "proteção da memória de vídeo preparada",
                    "video memory protection prepared",
                    "protección de la memoria de vídeo preparada",
                )
                .to_owned(),
            Some(error) => format!(
                "{} ({}); {}",
                locale.pick(
                    "não foi possível preparar a proteção da memória de vídeo",
                    "could not prepare the video memory protection",
                    "no se ha podido preparar la protección de la memoria de vídeo",
                ),
                error.reason(locale),
                locale.pick(
                    "o Android inicia sem ela e os jogos podem fechar se o driver gráfico perder o dispositivo",
                    "Android starts without it and games may close if the graphics driver loses the device",
                    "Android se inicia sin ella y los juegos pueden cerrarse si el controlador gráfico pierde el dispositivo",
                )
            ),
        };
        let memory = match (memory, self.host.card.window_mb()) {
            (Memory::FullSize, Some((visible, total))) => match locale {
                Locale::PtBr => format!(
                    "com a GPU do computador, o Android vê os {total} MB inteiros em vez da janela de {visible} MB"
                ),
                Locale::EnUs => format!(
                    "with the host GPU, Android sees the whole {total} MB instead of the {visible} MB window"
                ),
                Locale::EsEs => format!(
                    "con la GPU del equipo, Android ve los {total} MB completos en lugar de la ventana de {visible} MB"
                ),
            },
            (Memory::FullSize, None) => locale
                .pick(
                    "com a GPU do computador, o Android vê a memória de vídeo inteira em vez da janela visível ao processador",
                    "with the host GPU, Android sees the whole video memory instead of the processor-visible window",
                    "con la GPU del equipo, Android ve toda la memoria de vídeo en lugar de la ventana visible para el procesador",
                )
                .to_owned(),
            // Only reached through the override, whose name follows.
            (Memory::AllTypes, _) => locale
                .pick(
                    "todos os tipos de memória liberados",
                    "all memory types released",
                    "todos los tipos de memoria liberados",
                )
                .to_owned(),
            (Memory::Stock, _) => locale
                .pick(
                    "opções de memória do emulador inalteradas",
                    "emulator memory options unchanged",
                    "opciones de memoria del emulador sin cambios",
                )
                .to_owned(),
        };
        format!(
            "{} {protection}; {memory}{}.",
            locale.pick(
                "Placa de vídeo AMD no Linux:",
                "AMD graphics on Linux:",
                "Gráfica AMD en Linux:",
            ),
            self.choice.shown()
        )
    }
}

/// Why the engine starts without the preload library.
#[derive(Debug)]
pub(crate) enum GuardError {
    /// Neither `XDG_DATA_HOME` nor `HOME` names a folder.
    NoDataFolder,
    /// ld.so splits `LD_PRELOAD` on spaces and colons, expands `$` and has no
    /// way to escape them.
    Path(PathBuf),
    Write(PathBuf, io::Error),
}

impl GuardError {
    fn reason(&self, locale: Locale) -> String {
        match self {
            Self::NoDataFolder => locale
                .pick(
                    "pasta de dados do usuário não encontrada",
                    "user data folder not found",
                    "no se ha encontrado la carpeta de datos del usuario",
                )
                .to_owned(),
            Self::Path(folder) => format!(
                "{}: {}",
                locale.pick(
                    "o LD_PRELOAD não aceita espaço, dois-pontos ou $ no caminho",
                    "LD_PRELOAD does not accept a space, colon or $ in the path",
                    "LD_PRELOAD no admite espacios, dos puntos ni $ en la ruta",
                ),
                folder.display()
            ),
            Self::Write(folder, error) => format!("{error}: {}", folder.display()),
        }
    }
}

/// `<data>/RustEmuDroid/lib`, beside the managed `<data>/RustEmuDroid/runtime`.
fn guard_folder() -> Option<PathBuf> {
    crate::runtime_install::default_destination()
        .ok()
        .map(|runtime| runtime.with_file_name("lib"))
}

fn preload_accepts(path: &Path) -> bool {
    !path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b':' | b'$'))
}

/// Writes the embedded library to the per-user folder and returns its path.
fn install_guard() -> Result<PathBuf, GuardError> {
    let folder = guard_folder().ok_or(GuardError::NoDataFolder)?;
    if !preload_accepts(&folder) {
        return Err(GuardError::Path(folder));
    }
    install(&folder, GUARD).map_err(|error| GuardError::Write(folder, error))
}

/// The file name carries a short hash of the contents, so another client
/// version never replaces a library an engine is about to load.
fn library_name(library: &[u8]) -> String {
    let hash = format!("{:x}", Sha256::digest(library));
    format!("libuserptr_guard-{}.so", &hash[..12])
}

/// Reuses the file when it already holds these bytes; otherwise replaces it
/// atomically.
fn install(folder: &Path, library: &[u8]) -> io::Result<PathBuf> {
    let path = folder.join(library_name(library));
    if fs::read(&path).is_ok_and(|existing| existing == library) {
        return Ok(path);
    }
    fs::create_dir_all(folder)?;
    let mut temp = tempfile::NamedTempFile::new_in(folder)?;
    temp.write_all(library)?;
    temp.as_file().sync_all()?;
    make_loadable(temp.path())?;
    temp.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

/// Mode 0755: the temporary file starts readable by its owner only.
#[cfg(unix)]
fn make_loadable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_loadable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// `LD_PRELOAD` for the engine: our library first, then what the user already
/// had. A value that already lists our library is kept as it is.
fn preload(inherited: Option<&OsStr>, library: &Path) -> OsString {
    let ours = library.as_os_str();
    let Some(inherited) = inherited.filter(|value| !value.is_empty()) else {
        return ours.to_owned();
    };
    let listed = inherited
        .as_encoded_bytes()
        .split(|byte| matches!(byte, b' ' | b':'))
        .any(|item| item == ours.as_encoded_bytes());
    if listed {
        return inherited.to_owned();
    }
    let mut value = ours.to_owned();
    value.push(":");
    value.push(inherited);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [Locale; 3] = [Locale::PtBr, Locale::EnUs, Locale::EsEs];
    const MB: u64 = 1024 * 1024;

    fn kernel(major: u32, minor: u32) -> Option<Kernel> {
        Some(Kernel { major, minor })
    }

    fn card(visible_mb: u64, total_mb: u64) -> Card {
        Card {
            visible: Some(visible_mb * MB),
            total: Some(total_mb * MB),
        }
    }

    /// The measured PC: Radeon RX 580 8 GB, 256 MB BAR, Linux 6.8.
    fn rx580() -> Host {
        Host {
            card: card(256, 8192),
            kernel: kernel(6, 8),
        }
    }

    fn sysfs_card(drm: &Path, name: &str, vendor: &str, sizes: Option<(u64, u64)>) {
        let device = drm.join(name).join("device");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("vendor"), format!("{vendor}\n")).unwrap();
        if let Some((visible, total)) = sizes {
            fs::write(
                device.join("mem_info_vis_vram_total"),
                format!("{visible}\n"),
            )
            .unwrap();
            fs::write(device.join("mem_info_vram_total"), format!("{total}\n")).unwrap();
        }
    }

    #[test]
    fn kernel_releases_are_parsed_and_compared() {
        for (release, expected) in [
            ("6.8.0-151-generic\n", kernel(6, 8)),
            ("6.13.0", kernel(6, 13)),
            ("7.0.0-44-generic", kernel(7, 0)),
            ("6.13-rc3", kernel(6, 13)),
            ("6.12.9-arch1-1", kernel(6, 12)),
            ("5.15.167.4-microsoft-standard-WSL2", kernel(5, 15)),
            ("6", None),
            ("6.x", None),
            ("generic", None),
            ("", None),
        ] {
            assert_eq!(Kernel::parse(release), expected, "{release}");
        }
        let limit = Kernel::MAPS_DRIVER_MEMORY;
        assert!(kernel(6, 8).unwrap() < limit);
        assert!(kernel(6, 12).unwrap() < limit);
        assert!(kernel(5, 99).unwrap() < limit);
        assert!(kernel(6, 13).unwrap() >= limit);
        assert!(kernel(6, 14).unwrap() >= limit);
        assert!(kernel(7, 0).unwrap() >= limit);
        assert_eq!(kernel(6, 8).unwrap().to_string(), "6.8");

        let folder = tempfile::tempdir().unwrap();
        let release = folder.path().join("osrelease");
        assert_eq!(Kernel::read(&release), None);
        fs::write(&release, "6.8.0-151-generic\n").unwrap();
        assert_eq!(Kernel::read(&release), kernel(6, 8));
    }

    #[test]
    fn the_environment_variable_overrides_the_choice() {
        for (value, expected) in [
            (None, Override::Auto),
            (Some(""), Override::Auto),
            (Some("on"), Override::Auto),
            (Some("off"), Override::Off),
            (Some(" OFF "), Override::Off),
            (Some("stock"), Override::Stock),
            (Some("full-size"), Override::FullSize),
            (Some("Full-Size"), Override::FullSize),
            (Some("all-types"), Override::AllTypes),
            // The names of an earlier draft select nothing.
            (Some("stock-memory"), Override::Auto),
            (Some("all-memory"), Override::Auto),
            (Some("all_types"), Override::Auto),
        ] {
            assert_eq!(Override::parse(value), expected, "{value:?}");
        }
    }

    #[test]
    fn the_memory_mode_follows_the_kernel_and_the_window() {
        use Memory::{AllTypes, FullSize, Stock};
        let small = card(256, 8192);
        let full = card(8192, 8192);
        let unknown = Card {
            visible: None,
            total: None,
        };
        let half_known = Card {
            visible: Some(256 * MB),
            total: None,
        };
        for (choice, kernel, card, expected) in [
            // The measured PC: 256 MB of 8 GB on Linux 6.8.
            (Override::Auto, kernel(6, 8), &small, Some(FullSize)),
            (Override::Auto, kernel(6, 12), &small, Some(FullSize)),
            (Override::Auto, kernel(5, 15), &small, Some(FullSize)),
            // The window covers the video memory, or the sizes can not be read.
            (Override::Auto, kernel(6, 8), &full, Some(Stock)),
            (Override::Auto, kernel(6, 8), &unknown, Some(Stock)),
            (Override::Auto, kernel(6, 8), &half_known, Some(Stock)),
            (Override::Auto, kernel(6, 12), &full, Some(Stock)),
            // The kernel never changes the mode: all types was not measured
            // on Linux 6.13 or newer, so it is not a default there either.
            (Override::Auto, kernel(6, 13), &small, Some(FullSize)),
            (Override::Auto, kernel(6, 14), &small, Some(FullSize)),
            (Override::Auto, kernel(7, 0), &small, Some(FullSize)),
            (Override::Auto, kernel(6, 13), &full, Some(Stock)),
            (Override::Auto, kernel(6, 13), &unknown, Some(Stock)),
            (Override::Auto, kernel(7, 0), &full, Some(Stock)),
            (Override::Auto, None, &small, Some(FullSize)),
            (Override::Auto, None, &full, Some(Stock)),
            (Override::Auto, None, &unknown, Some(Stock)),
            // The override wins over the card; all types exists only here.
            (Override::Stock, kernel(6, 8), &small, Some(Stock)),
            (Override::Stock, kernel(7, 0), &full, Some(Stock)),
            (Override::FullSize, kernel(7, 0), &small, Some(FullSize)),
            (Override::FullSize, kernel(6, 8), &full, Some(FullSize)),
            (Override::AllTypes, kernel(6, 8), &small, Some(AllTypes)),
            (Override::AllTypes, kernel(6, 13), &full, Some(AllTypes)),
            (Override::AllTypes, None, &unknown, Some(AllTypes)),
            (Override::Off, kernel(6, 8), &small, None),
            (Override::Off, kernel(7, 0), &full, None),
        ] {
            let host = Host {
                card: card.clone(),
                kernel,
            };
            assert_eq!(
                policy(choice, Some(&host)),
                expected,
                "{choice:?} {kernel:?} {card:?}"
            );
        }
        // No AMD card: nothing, whatever the override.
        for choice in [
            Override::Auto,
            Override::Off,
            Override::Stock,
            Override::FullSize,
            Override::AllTypes,
        ] {
            assert_eq!(policy(choice, None), None, "{choice:?}");
        }
        // A window as large as the video memory, or larger, is not small.
        assert!(!card(8192, 8192).small_window());
        assert!(!card(16384, 8192).small_window());
        assert_eq!(card(256, 8192).window_mb(), Some((256, 8192)));
    }

    #[test]
    fn the_adapter_choice_decides_only_when_it_names_another_vendor() {
        for (adapter, another) in [
            ("", false),
            ("AMD Radeon RX 580 Series (RADV POLARIS10)", false),
            ("AMD Radeon 780M Graphics", false),
            ("Radeon RX 580 Series", false),
            ("ATI Radeon HD 5000", false),
            ("An unclassified device", false),
            ("NVIDIA GeForce RTX 5070 Ti", true),
            ("GeForce GTX 1060", true),
            ("Intel(R) UHD Graphics 770 (ADL-S GT1)", true),
            ("Intel(R) Arc(tm) A770 Graphics (DG2)", true),
            ("llvmpipe (LLVM 21.1.8, 256 bits)", true),
            ("SwiftShader Device (LLVM 10.0.0)", true),
            ("Virtio-GPU Venus (Intel(R) UHD Graphics)", true),
        ] {
            assert_eq!(names_another_vendor(adapter), another, "{adapter}");
            for gpu in [Gpu::Host, Gpu::Auto] {
                assert_eq!(launch_can_use_amd(gpu, adapter), !another, "{adapter}");
            }
            for gpu in [Gpu::Software, Gpu::SwiftShader] {
                assert!(!launch_can_use_amd(gpu, adapter), "{adapter}");
            }
        }

        let decide = |choice, gpu, adapter, host| Decision::new(choice, gpu, adapter, host);
        let decision = decide(Override::Auto, Gpu::Auto, "", Some(rx580())).unwrap();
        assert_eq!(decision.memory, Memory::FullSize);
        assert_eq!(decision.host(), &rx580());
        let stock = decide(Override::Stock, Gpu::Host, "", Some(rx580())).unwrap();
        assert_eq!(stock.memory, Memory::Stock);
        let radeon = "AMD Radeon RX 580 Series (RADV POLARIS10)";
        let forced = decide(Override::AllTypes, Gpu::Host, radeon, Some(rx580())).unwrap();
        assert_eq!(forced.memory, Memory::AllTypes);
        assert!(decide(Override::Off, Gpu::Auto, "", Some(rx580())).is_none());
        assert!(decide(Override::Auto, Gpu::Software, "", Some(rx580())).is_none());
        assert!(decide(Override::AllTypes, Gpu::SwiftShader, "", Some(rx580())).is_none());
        let nvidia = "NVIDIA GeForce RTX 5070 Ti";
        assert!(decide(Override::Auto, Gpu::Auto, nvidia, Some(rx580())).is_none());
        assert!(decide(Override::FullSize, Gpu::Auto, nvidia, Some(rx580())).is_none());
        // No AMD card, or not Linux.
        assert!(decide(Override::AllTypes, Gpu::Auto, "", None).is_none());
    }

    #[test]
    fn amd_cards_are_read_from_sysfs() {
        let folder = tempfile::tempdir().unwrap();
        let drm = folder.path().join("drm");
        let release = folder.path().join("osrelease");
        // Nothing there, as on a system without the folder.
        assert!(amd_cards(&drm).is_empty());
        assert_eq!(Host::read(&drm, &release), None);

        fs::create_dir_all(&drm).unwrap();
        fs::write(drm.join("version"), "drm 1.1.0 20060810\n").unwrap();
        sysfs_card(&drm, "card0", "0x8086", None);
        sysfs_card(&drm, "renderD128", "0x1002", Some((1, 2)));
        assert_eq!(Host::read(&drm, &release), None);

        // The measured card, with one of its connectors beside it.
        sysfs_card(&drm, "card1", "0x1002", Some((268435456, 8589934592)));
        sysfs_card(&drm, "card1-DP-1", "0x1002", Some((1, 2)));
        assert_eq!(amd_cards(&drm), [card(256, 8192)]);
        assert_eq!(
            Host::read(&drm, &release),
            Some(Host {
                card: card(256, 8192),
                kernel: None,
            })
        );
        fs::write(&release, "6.8.0-151-generic\n").unwrap();
        assert_eq!(Host::read(&drm, &release), Some(rx580()));

        // An integrated AMD card too: the one with the small window decides.
        sysfs_card(&drm, "card2", "0X1002", Some((2048 * MB, 2048 * MB)));
        assert_eq!(amd_cards(&drm), [card(256, 8192), card(2048, 2048)]);
        assert_eq!(Host::read(&drm, &release).unwrap().card, card(256, 8192));

        // Without the amdgpu memory files the card still counts.
        let other = folder.path().join("other");
        sysfs_card(&other, "card0", "0x1002", None);
        fs::write(
            other.join("card0/device/mem_info_vram_total"),
            "not a number\n",
        )
        .unwrap();
        let host = Host::read(&other, &release).unwrap();
        assert_eq!(
            host.card,
            Card {
                visible: None,
                total: None,
            }
        );
        assert!(!host.card.small_window());

        // Two full-window cards: the one with more memory.
        let pair = folder.path().join("pair");
        sysfs_card(&pair, "card0", "0x1002", Some((512 * MB, 512 * MB)));
        sysfs_card(&pair, "card1", "0x1002", Some((16384 * MB, 16384 * MB)));
        assert_eq!(
            Host::read(&pair, &release).unwrap().card,
            card(16384, 16384)
        );
    }

    #[test]
    fn our_library_is_added_once_and_the_users_preload_is_kept() {
        let library =
            Path::new("/home/user/.local/share/RustEmuDroid/lib/libuserptr_guard-0123456789ab.so");
        let ours = library.to_str().unwrap();
        let compose = |inherited: Option<&str>| {
            preload(inherited.map(OsStr::new), library)
                .into_string()
                .unwrap()
        };
        assert_eq!(compose(None), ours);
        assert_eq!(compose(Some("")), ours);
        assert_eq!(
            compose(Some("/usr/lib/libgamemodeauto.so.0")),
            format!("{ours}:/usr/lib/libgamemodeauto.so.0")
        );
        assert_eq!(
            compose(Some("libMangoHud.so /opt/other.so")),
            format!("{ours}:libMangoHud.so /opt/other.so")
        );
        // Already there, with either separator: unchanged.
        for inherited in [
            ours.to_owned(),
            format!("{ours}:/opt/other.so"),
            format!("/opt/other.so:{ours}"),
            format!("/opt/other.so {ours}"),
        ] {
            assert_eq!(compose(Some(&inherited)), inherited);
        }
        // A different file whose name only contains ours does not count.
        let longer = format!("{ours}.bak");
        assert_eq!(compose(Some(&longer)), format!("{ours}:{longer}"));

        assert!(preload_accepts(library));
        for unusable in [
            "/home/John Doe/.local/share/RustEmuDroid/lib",
            "/data/a:b/RustEmuDroid/lib",
            "/data/$ORIGIN/RustEmuDroid/lib",
            "/data/tab\there/lib",
        ] {
            assert!(!preload_accepts(Path::new(unusable)), "{unusable}");
        }
    }

    #[test]
    fn the_library_is_written_once_and_replaced_when_its_bytes_differ() {
        let folder = tempfile::tempdir().unwrap();
        let lib = folder.path().join("RustEmuDroid").join("lib");
        let bytes = b"\x7fELF first build";
        let name = library_name(bytes);
        assert!(name.starts_with("libuserptr_guard-") && name.ends_with(".so"));
        assert_eq!(name.len(), "libuserptr_guard-.so".len() + 12);
        assert_ne!(name, library_name(b"\x7fELF second build"));

        let path = install(&lib, bytes).unwrap();
        assert_eq!(path, lib.join(&name));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }

        // Same bytes: the file is reused, not rewritten.
        let written = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(install(&lib, bytes).unwrap(), path);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), written);

        // A damaged file is replaced, and nothing temporary stays behind.
        fs::write(&path, b"damaged").unwrap();
        assert_eq!(install(&lib, bytes).unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let files: Vec<_> = fs::read_dir(&lib)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, [std::ffi::OsString::from(&name)]);

        // Another build lives beside it under its own name.
        let other = install(&lib, b"\x7fELF second build").unwrap();
        assert_ne!(other, path);
        assert!(path.is_file() && other.is_file());

        // A folder that can not be created is an error, not a panic.
        let blocked = folder.path().join("file");
        fs::write(&blocked, b"").unwrap();
        assert!(install(&blocked.join("lib"), bytes).is_err());
    }

    /// `Some(Some(value))`: set. `Some(None)`: removed from what the engine
    /// inherits. `None`: left as the client has it.
    fn environment(command: &Command, name: &str) -> Option<Option<String>> {
        command.get_envs().find_map(|(key, value)| {
            (key == name).then(|| value.map(|value| value.to_string_lossy().into_owned()))
        })
    }

    fn host(visible_mb: u64, total_mb: u64, kernel: Option<Kernel>) -> Option<Host> {
        Some(Host {
            card: card(visible_mb, total_mb),
            kernel,
        })
    }

    const GUARD_PATH: &str =
        "/home/user/.local/share/RustEmuDroid/lib/libuserptr_guard-0123456789ab.so";
    const VISIBLE: &str = "RUSTEMUDROID_AMD_VISIBLE_VRAM";

    #[test]
    fn each_memory_mode_prepares_the_engine_and_names_itself_in_the_boot_details() {
        let library = PathBuf::from(GUARD_PATH);
        let apply = |decision: &Decision, host_vulkan: bool, locale: Locale| {
            let mut command = Command::new("emulator");
            let applied =
                decision.apply_with(&mut command, host_vulkan, Ok(library.clone()), None, locale);
            (command, applied)
        };
        let preloaded = Some(Some(GUARD_PATH.to_owned()));

        // Full size: the measured PC. The emulator's options stay.
        let full_size = Decision::new(Override::Auto, Gpu::Auto, "", Some(rx580())).unwrap();
        let (command, applied) = apply(&full_size, true, Locale::EnUs);
        assert!(!applied.all_memory_types);
        assert_eq!(
            applied.note,
            "AMD graphics on Linux: video memory protection prepared; with the host GPU, Android sees the whole 8192 MB instead of the 256 MB window."
        );
        assert_eq!(environment(&command, "LD_PRELOAD"), preloaded);
        assert_eq!(environment(&command, VISIBLE), Some(Some("all".into())));
        assert_eq!(
            apply(&full_size, true, Locale::PtBr).1.note,
            "Placa de vídeo AMD no Linux: proteção da memória de vídeo preparada; com a GPU do computador, o Android vê os 8192 MB inteiros em vez da janela de 256 MB."
        );
        assert_eq!(
            apply(&full_size, true, Locale::EsEs).1.note,
            "Gráfica AMD en Linux: protección de la memoria de vídeo preparada; con la GPU del equipo, Android ve los 8192 MB completos en lugar de la ventana de 256 MB."
        );

        // A newer kernel changes nothing: the same card gets the same mode and note.
        let new_kernel = host(256, 8192, kernel(6, 13));
        let on_new_kernel = Decision::new(Override::Auto, Gpu::Host, "", new_kernel).unwrap();
        assert_eq!(on_new_kernel.memory, Memory::FullSize);
        let (command, on_new) = apply(&on_new_kernel, true, Locale::EnUs);
        assert_eq!(on_new, applied);
        assert_eq!(environment(&command, VISIBLE), Some(Some("all".into())));

        // All types: only through the override. A stale variable never reaches the engine.
        let all_types = Decision::new(Override::AllTypes, Gpu::Host, "", Some(rx580())).unwrap();
        let (command, applied) = apply(&all_types, true, Locale::EnUs);
        assert!(applied.all_memory_types);
        assert_eq!(
            applied.note,
            "AMD graphics on Linux: video memory protection prepared; all memory types released (RUSTEMUDROID_AMD_LINUX=all-types)."
        );
        assert_eq!(environment(&command, "LD_PRELOAD"), preloaded);
        assert_eq!(environment(&command, VISIBLE), Some(None));
        assert_eq!(
            apply(&all_types, true, Locale::PtBr).1.note,
            "Placa de vídeo AMD no Linux: proteção da memória de vídeo preparada; todos os tipos de memória liberados (RUSTEMUDROID_AMD_LINUX=all-types)."
        );
        assert_eq!(
            apply(&all_types, true, Locale::EsEs).1.note,
            "Gráfica AMD en Linux: protección de la memoria de vídeo preparada; todos los tipos de memoria liberados (RUSTEMUDROID_AMD_LINUX=all-types)."
        );
        // The same on a kernel expected to support it: the note claims nothing about the kernel.
        let new_kernel = host(256, 8192, kernel(6, 13));
        let forced_new = Decision::new(Override::AllTypes, Gpu::Host, "", new_kernel).unwrap();
        assert_eq!(apply(&forced_new, true, Locale::EnUs).1, applied);

        // Stock: a window that covers the video memory.
        let full_window = host(8192, 8192, kernel(6, 8));
        let stock = Decision::new(Override::Auto, Gpu::Host, "", full_window).unwrap();
        let (command, applied) = apply(&stock, true, Locale::EnUs);
        assert!(!applied.all_memory_types);
        assert_eq!(
            applied.note,
            "AMD graphics on Linux: video memory protection prepared; emulator memory options unchanged."
        );
        assert_eq!(environment(&command, "LD_PRELOAD"), preloaded);
        assert_eq!(environment(&command, VISIBLE), Some(None));
        assert_eq!(
            apply(&stock, true, Locale::PtBr).1.note,
            "Placa de vídeo AMD no Linux: proteção da memória de vídeo preparada; opções de memória do emulador inalteradas."
        );
        assert_eq!(
            apply(&stock, true, Locale::EsEs).1.note,
            "Gráfica AMD en Linux: protección de la memoria de vídeo preparada; opciones de memoria del emulador sin cambios."
        );

        // The override is named, and decides against the card.
        for (choice, memory, all_types, visible, ending) in [
            (
                Override::Stock,
                Memory::Stock,
                false,
                None,
                "emulator memory options unchanged (RUSTEMUDROID_AMD_LINUX=stock).",
            ),
            (
                Override::FullSize,
                Memory::FullSize,
                false,
                Some("all"),
                "with the host GPU, Android sees the whole 8192 MB instead of the 256 MB window (RUSTEMUDROID_AMD_LINUX=full-size).",
            ),
            (
                Override::AllTypes,
                Memory::AllTypes,
                true,
                None,
                "all memory types released (RUSTEMUDROID_AMD_LINUX=all-types).",
            ),
        ] {
            let decision = Decision::new(choice, Gpu::Auto, "", Some(rx580())).unwrap();
            assert_eq!(decision.memory, memory);
            let (command, applied) = apply(&decision, true, Locale::EnUs);
            assert_eq!(applied.all_memory_types, all_types, "{choice:?}");
            assert_eq!(
                environment(&command, VISIBLE),
                Some(visible.map(String::from)),
                "{choice:?}"
            );
            assert!(applied.note.ends_with(ending), "{}", applied.note);
        }
        // Full size asked for a card whose sizes are unknown: no numbers to show.
        let unknown = Host {
            card: Card {
                visible: None,
                total: None,
            },
            kernel: kernel(6, 8),
        };
        let blind = Decision::new(Override::FullSize, Gpu::Auto, "", Some(unknown)).unwrap();
        assert_eq!(
            apply(&blind, true, Locale::EnUs).1.note,
            "AMD graphics on Linux: video memory protection prepared; with the host GPU, Android sees the whole video memory instead of the processor-visible window (RUSTEMUDROID_AMD_LINUX=full-size)."
        );
        for (locale, start) in [
            (
                Locale::PtBr,
                "preparada; com a GPU do computador, o Android vê a memória",
            ),
            (
                Locale::EnUs,
                "prepared; with the host GPU, Android sees the whole video",
            ),
            (
                Locale::EsEs,
                "preparada; con la GPU del equipo, Android ve toda la memoria",
            ),
        ] {
            let note = apply(&blind, true, locale).1.note;
            assert!(note.contains(start), "{note}");
            assert!(!note.contains("MB"), "{note}");
            assert!(
                note.ends_with(" (RUSTEMUDROID_AMD_LINUX=full-size)."),
                "{note}"
            );
        }

        // Without the host's Vulkan device (the GLES compatibility path) no memory mode applies.
        for decision in [&full_size, &all_types] {
            let (command, applied) = apply(decision, false, Locale::EnUs);
            assert!(!applied.all_memory_types);
            assert!(
                applied
                    .note
                    .contains("prepared; emulator memory options unchanged"),
                "{}",
                applied.note
            );
            assert_eq!(environment(&command, VISIBLE), Some(None));
            assert_eq!(environment(&command, "LD_PRELOAD"), preloaded);
        }

        // The user's own preload is kept behind ours.
        let mut command = Command::new("emulator");
        full_size.apply_with(
            &mut command,
            true,
            Ok(library.clone()),
            Some("/usr/lib/libgamemodeauto.so.0".into()),
            Locale::EnUs,
        );
        assert_eq!(
            environment(&command, "LD_PRELOAD"),
            Some(Some(format!("{GUARD_PATH}:/usr/lib/libgamemodeauto.so.0")))
        );
    }

    #[test]
    fn a_library_that_can_not_be_written_never_stops_the_boot() {
        let failure = || {
            GuardError::Write(
                PathBuf::from("/home/user/.local/share/RustEmuDroid/lib"),
                io::Error::from(io::ErrorKind::PermissionDenied),
            )
        };
        // Full size needs the library: without it the emulator's options are all there is.
        let full_size = Decision::new(Override::Auto, Gpu::Auto, "", Some(rx580())).unwrap();
        let mut command = Command::new("emulator");
        let applied = full_size.apply_with(&mut command, true, Err(failure()), None, Locale::EnUs);
        assert!(!applied.all_memory_types);
        assert_eq!(environment(&command, "LD_PRELOAD"), None);
        assert_eq!(environment(&command, VISIBLE), Some(None));
        assert!(
            applied.note.starts_with(
                "AMD graphics on Linux: could not prepare the video memory protection ("
            ),
            "{}",
            applied.note
        );
        assert!(
            applied.note.ends_with(
                "RustEmuDroid/lib); Android starts without it and games may close if the graphics driver loses the device; emulator memory options unchanged."
            ),
            "{}",
            applied.note
        );

        // All types does not depend on the library.
        let all_types = Decision::new(Override::AllTypes, Gpu::Host, "", Some(rx580())).unwrap();
        let mut command = Command::new("emulator");
        let applied = all_types.apply_with(&mut command, true, Err(failure()), None, Locale::EnUs);
        assert!(applied.all_memory_types);
        assert_eq!(environment(&command, "LD_PRELOAD"), None);
        assert_eq!(environment(&command, VISIBLE), Some(None));
        assert!(
            applied
                .note
                .ends_with("; all memory types released (RUSTEMUDROID_AMD_LINUX=all-types)."),
            "{}",
            applied.note
        );

        for locale in LOCALES {
            for error in [
                failure(),
                GuardError::NoDataFolder,
                GuardError::Path(PathBuf::from(
                    "/home/John Doe/.local/share/RustEmuDroid/lib",
                )),
            ] {
                let note = full_size.note(Memory::Stock, Some(&error), locale);
                for prepared in ["preparada", "prepared"] {
                    assert!(!note.contains(prepared), "{note}");
                }
                assert!(
                    note.ends_with("inalteradas.")
                        || note.ends_with("unchanged.")
                        || note.ends_with("sin cambios."),
                    "{note}"
                );
            }
        }
    }

    #[test]
    fn advice_is_given_only_for_a_small_window_on_an_old_kernel() {
        let host = rx580();
        assert_eq!(
            host.doctor_advice(Locale::EnUs).unwrap(),
            "AMD graphics: the processor reaches only 256 MB of the card's 8192 MB (small memory window) and this Linux kernel (6.8) is older than 6.13. Some games can freeze Android when the processor virtualization (KVM) can not map graphics memory. Enabling “Above 4G Decoding” and “Resizable BAR” in the firmware (BIOS/UEFI) or using Linux 6.13 or newer should avoid it."
        );
        assert_eq!(
            host.kvm_mapping_advice(Locale::EnUs).unwrap(),
            "This happens with AMD graphics on Linux kernels older than 6.13 when the card exposes a small memory window (here: 256 MB of 8192 MB, kernel 6.8). Enabling “Above 4G Decoding” and “Resizable BAR” in the firmware (BIOS/UEFI) or using Linux 6.13 or newer should avoid it."
        );
        for locale in LOCALES {
            for text in [
                host.doctor_advice(locale).unwrap(),
                host.kvm_mapping_advice(locale).unwrap(),
            ] {
                for part in [
                    "256 MB",
                    "8192 MB",
                    "6.8",
                    "6.13",
                    "Above 4G Decoding",
                    "Resizable BAR",
                ] {
                    assert!(text.contains(part), "{part}: {text}");
                }
            }
        }
        let new_kernel = Host {
            kernel: kernel(6, 13),
            ..rx580()
        };
        let full_window = Host {
            card: card(8192, 8192),
            ..rx580()
        };
        let unknown_kernel = Host {
            kernel: None,
            ..rx580()
        };
        assert_eq!(new_kernel.doctor_advice(Locale::EnUs), None);
        assert_eq!(new_kernel.kvm_mapping_advice(Locale::EnUs), None);
        assert_eq!(full_window.doctor_advice(Locale::EnUs), None);
        // Forced with all-types on a full window: the kernel alone explains the failure.
        assert!(
            full_window
                .kvm_mapping_advice(Locale::EnUs)
                .unwrap()
                .contains("small memory window (here: kernel 6.8). Enabling")
        );
        assert_eq!(unknown_kernel.doctor_advice(Locale::EnUs), None);
        assert!(
            unknown_kernel
                .kvm_mapping_advice(Locale::PtBr)
                .unwrap()
                .contains("(aqui: 256 MB de 8192 MB). Ativar")
        );
        let nothing_known = Host {
            card: Card {
                visible: None,
                total: None,
            },
            kernel: None,
        };
        assert!(
            nothing_known
                .kvm_mapping_advice(Locale::EsEs)
                .unwrap()
                .contains("ventana de memoria pequeña. Activar")
        );
    }
}
